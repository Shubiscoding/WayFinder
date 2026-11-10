// script.js — All UI logic for WayFinder.
// Commands, rendering, keyboard navigation, Tauri invoke calls.

const { invoke } = window.__TAURI__.core;

// ---------------------------------------------------------------------------
// Unit / temperature conversion tables (pure JS, no Rust needed)
// ---------------------------------------------------------------------------

const UNITS = {
    length: { m: 1, km: 1000, cm: 0.01, mi: 1609.344, ft: 0.3048, in: 0.0254 },
    mass: { kg: 1, g: 0.001, lb: 0.45359237, oz: 0.028349523125 },
};

const has = (obj, key) => Object.hasOwn(obj, key);

const TEMPS = {
    c: [(c) => c, (c) => c],
    f: [(f) => ((f - 32) * 5) / 9, (c) => (c * 9) / 5 + 32],
    k: [(k) => k - 273.15, (c) => c + 273.15],
};

// ---------------------------------------------------------------------------
// Command table — one entry per mode
// ---------------------------------------------------------------------------

const COMMANDS = {
    fil: {
        hint: 'Find files',
        placeholder: 'file name',
        empty: 'No files found',
        search: searchFiles,
    },
    fol: {
        hint: 'Find folders',
        placeholder: 'folder name',
        empty: 'No folders found',
        search: searchFolders,
    },
    con: {
        hint: 'Convert units or files',
        placeholder: '10 km or report.docx',
        placeholder2: 'mi or pdf',
        empty: 'Try: 10 km to mi',
        search: searchConvert,
    },
    bow: {
        hint: 'Search the web',
        placeholder: 'search the web',
        empty: 'Type to search',
        search: searchWeb,
    },
};

// ---------------------------------------------------------------------------
// DOM references
// ---------------------------------------------------------------------------

const input    = document.getElementById('input');
const chip     = document.getElementById('chip');
const list     = document.getElementById('results');
const menu     = document.getElementById('menu');
const KEYS     = Object.keys(COMMANDS);
const launcher = document.querySelector('.launcher');
const input2   = document.getElementById('input2');
const toTag    = document.getElementById('to');
const bar      = document.querySelector('.search');
const status   = document.getElementById('status');
const errorBox = document.getElementById('error-box');
const errorMsg = document.getElementById('error-msg');
const errorClose = document.getElementById('error-close');

let mode        = 'fil';
let menuIndex   = 0;
let items       = [];
let rows        = [];
let index       = 0;
let statusTimer;

// True while the first crawl is still running (we show "Indexing…").
let indexing = false;

// Search timing. Wait a moment after typing stops before searching, and
// ignore answers that belong to an older query.
const DEBOUNCE_MS = 80;
const MAX_ITEMS = 5;       // never show more than 5 suggestions
let searchTicket = 0;      // newest search wins
let debounceTimer;

// ---------------------------------------------------------------------------
// Error box
// ---------------------------------------------------------------------------

/** Show a plain-English error in the modal error box. */
function showError(message) {
    errorMsg.textContent = message;
    errorBox.hidden = false;
    errorBox.focus();
}

errorClose.addEventListener('click', () => {
    errorBox.hidden = true;
    input.focus();
});

errorBox.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
        errorBox.hidden = true;
        input.focus();
    }
});

// ---------------------------------------------------------------------------
// Status line (small transient messages)
// ---------------------------------------------------------------------------

function say(message) {
    status.textContent = message;
    clearTimeout(statusTimer);
    statusTimer = setTimeout(() => (status.textContent = ''), 2000);
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

const includes = (text, query) =>
    text.toLowerCase().includes(query.toLowerCase());

/** Create a DOM element with optional class and text. */
function el(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
}

// ---------------------------------------------------------------------------
// File opening — custom Tauri command (Phase 2 fix)
//
// Uses the opener plugin's open_path, which calls Windows ShellExecuteW
// directly (no shell text parsing). Safe even for file names that contain
// &, %, spaces, etc.
// ---------------------------------------------------------------------------

/**
 * Open a file or folder with the system default app.
 * Closes the launcher on success; keeps it open and shows an error box on failure.
 */
async function openPath(path) {
    try {
        await invoke('open_path', { path });
        dismissLauncher(); // close the panel once the file is handed to the OS
    } catch (err) {
        showError(`Could not open "${path}": ${err}`);
    }
}

// ---------------------------------------------------------------------------
// Search functions
// ---------------------------------------------------------------------------

/** Search files via the Rust backend. Returns a Promise<item[]>. */
async function searchFiles(query) {
    if (indexing) {
        return [{
            tag: '…',
            title: 'Indexing your files',
            sub: 'Results will appear when done',
            run: () => {},
        }];
    }
    try {
        const hits = await invoke('search_files', { query });
        return hits.map((h) => ({
            tag: h.extension ? `.${h.extension}` : (h.is_dir ? 'dir' : 'file'),
            title: h.name,
            sub: h.path,
            run: () => openPath(h.path),
        }));
    } catch (err) {
        showError(`File search failed: ${err}`);
        return [];
    }
}

/** Search folders via the Rust backend. Returns a Promise<item[]>. */
async function searchFolders(query) {
    if (indexing) {
        return [{
            tag: '…',
            title: 'Indexing your folders',
            sub: 'Results will appear when done',
            run: () => {},
        }];
    }
    try {
        const hits = await invoke('search_folders', { query });
        return hits.map((h) => ({
            tag: 'dir',
            title: h.name,
            sub: h.path,
            run: () => openPath(h.path),
        }));
    } catch (err) {
        showError(`Folder search failed: ${err}`);
        return [];
    }
}

// ---------------------------------------------------------------------------
// Unit / file conversion
// ---------------------------------------------------------------------------

function convertUnits(value, from, to) {
    if (has(TEMPS, from) && has(TEMPS, to)) return TEMPS[to][1](TEMPS[from][0](value));
    for (const group of Object.values(UNITS)) {
        if (has(group, from) && has(group, to)) return (value * group[from]) / group[to];
    }
    return null;
}

async function runConvert(source, targetExt) {
    try {
        const res = await invoke('convert_file', { source, targetExt });
        say(res.message);
    } catch (err) {
        showError(err);
    }
}

function convertFile(query) {
    const match = query.trim().match(/^(.+?)(?:\.([a-z0-9]+))?\s+to\s+\.?([a-z0-9]+)$/i);
    if (!match) return [];
    const [, base, from, to] = match;
    const target = to.toLowerCase();
    const source = from ? `${base}.${from}` : base;
    const displayName = base.split(/[\\/]/).pop();
    const targetName = `${displayName}.${target}`;

    const supportedImageExts = ['png', 'jpg', 'jpeg', 'jfif', 'webp', 'bmp', 'tiff', 'tif'];
    const fromExt = (from || '').toLowerCase();
    const isSupported = fromExt
        ? supportedImageExts.includes(fromExt) && supportedImageExts.includes(target)
        : supportedImageExts.includes(target);

    const subText = isSupported
        ? `Convert ${source} → ${targetName}`
        : `Convert ${fromExt ? `.${fromExt}` : ''} to .${target} (pure-Rust image engine supports png, jpg, webp, bmp, tiff)`;

    return [{
        tag: `.${target}`,
        title: targetName,
        sub: subText,
        run: () => runConvert(source, target),
    }];
}

function searchConvert(query) {
    const match = query
        .trim()
        .match(/^(-?\d*\.?\d+)\s*([a-z]+)\s+(?:to|in)\s+([a-z]+)$/i);
    if (!match) return convertFile(query);
    const [, raw, from, to] = match;
    const result = convertUnits(parseFloat(raw), from.toLowerCase(), to.toLowerCase());
    if (result === null || !Number.isFinite(result)) return [];
    const text = `${Number(result.toPrecision(6))} ${to}`;
    return [{
        tag: '=',
        title: text,
        sub: `${raw} ${from}`,
        run: () =>
            navigator.clipboard
                ?.writeText(text)
                .then(() => say(`Copied ${text}`), () => say(text)),
    }];
}

/** Open a web address in the default browser (the webview cannot do it). */
async function openUrl(url) {
    try {
        await invoke('open_url', { url });
    } catch (err) {
        showError(`${err}`);
    }
}

function searchWeb(query) {
    if (!query.trim()) return [];
    return [{
        tag: 'web',
        title: query,
        sub: 'Search on DuckDuckGo',
        run: () => openUrl(`https://duckduckgo.com/?q=${encodeURIComponent(query)}`),
    }];
}

// ---------------------------------------------------------------------------
// Mode switching
// ---------------------------------------------------------------------------

function setMode(key, rest = '') {
    if (key !== 'con' && document.activeElement === input2) input.focus();
    mode = key;
    input.value = rest;
    input2.value = '';
    input2.hidden = toTag.hidden = key !== 'con';
    input2.placeholder = COMMANDS[key].placeholder2 || '';
    bar.classList.toggle('convert', key === 'con');
    chip.textContent = `#${key}`;
    input.placeholder = COMMANDS[key].placeholder;
    chip.classList.remove('pop');
    void chip.offsetWidth;
    chip.classList.add('pop');
    update(true);
}

// ---------------------------------------------------------------------------
// Result list rendering
// ---------------------------------------------------------------------------

function setIndex(next) {
    index = (next + items.length) % items.length;
    rows.forEach((row, i) => row.classList.toggle('active', i === index));
    rows[index]?.scrollIntoView({ block: 'nearest' });
}

/**
 * Build one result row.
 * Layout: [tag badge] [.info block: .title on top, .sub below]
 * This gives a large file name and a smaller path, stacked vertically.
 */
function buildRow(item, i) {
    const row = el('li', i === index ? 'active' : '');
    row.setAttribute('role', 'option');
    row.style.setProperty('--i', i);

    const badge = el('span', 'tag', item.tag);

    const info = el('div', 'info');
    info.append(
        el('span', 'title', item.title),
        el('span', 'sub',   item.sub),
    );

    row.append(badge, info);
    row.addEventListener('mousemove', () => i !== index && setIndex(i));
    row.addEventListener('click', () => item.run());
    return row;
}

function render(animate) {
    list.replaceChildren();
    list.classList.remove('enter');
    rows = [];

    if (!items.length) {
        list.append(el('li', 'empty', COMMANDS[mode].empty));
        return;
    }

    items.forEach((item, i) => {
        const row = buildRow(item, i);
        rows.push(row);
        list.append(row);
    });

    if (animate) {
        void list.offsetWidth;
        list.classList.add('enter');
    }
}

function fitInput() {
    if (mode !== 'con') {
        input.style.width = '';
        return;
    }
    input.style.width = `${(input.value || input.placeholder).length + 1}ch`;
}

function getQuery() {
    const first = input.value.trim();
    if (mode !== 'con') return first;
    const second = input2.value.trim();
    return first || second ? `${first} to ${second}` : '';
}

/** Search again after the user pauses typing. */
function scheduleUpdate() {
    clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => update(), DEBOUNCE_MS);
}

async function update(animate = false) {
    fitInput();
    const query = getQuery();
    launcher.classList.toggle('compact', !query);

    // Each search gets a number. If a newer search starts while this one is
    // waiting for Rust, this one is out of date and is thrown away.
    const ticket = ++searchTicket;

    let result = [];
    if (query) {
        try {
            result = await COMMANDS[mode].search(query);
        } catch (err) {
            console.error('[search]', err);
            result = [];
        }
    }
    if (ticket !== searchTicket) return;

    items = result.slice(0, MAX_ITEMS);
    index = 0;
    render(animate);
}

// ---------------------------------------------------------------------------
// Menu (chip dropdown)
// ---------------------------------------------------------------------------

function setMenuIndex(next) {
    menuIndex = (next + KEYS.length) % KEYS.length;
    [...menu.children].forEach((li, i) =>
        li.classList.toggle('active', i === menuIndex),
    );
}

function openMenu() {
    menu.hidden = false;
    chip.setAttribute('aria-expanded', 'true');
    setMenuIndex(KEYS.indexOf(mode));
}

function closeMenu() {
    menu.hidden = true;
    chip.setAttribute('aria-expanded', 'false');
}

function buildMenu() {
    KEYS.forEach((key) => {
        const li = el('li');
        li.setAttribute('role', 'option');
        li.append(el('span', 'tag', `#${key}`), el('span', 'title', COMMANDS[key].hint));
        li.addEventListener('click', () => {
            setMode(key);
            closeMenu();
        });
        menu.append(li);
    });
}

// ---------------------------------------------------------------------------
// Keyboard handling
// ---------------------------------------------------------------------------

function onKeydown(e) {
    if (e.isComposing) return; // Japanese/Chinese/Korean IME confirmation
    const menuOpen = !menu.hidden;
    const step = e.key === 'ArrowDown' ? 1 : e.key === 'ArrowUp' ? -1 : 0;

    if (e.altKey && e.key === 'ArrowDown') {
        menuOpen ? closeMenu() : openMenu();
    } else if (step && menuOpen) {
        setMenuIndex(menuIndex + step);
    } else if (step && items.length) {
        setIndex(index + step);
    } else if (e.key === 'Enter' && menuOpen) {
        setMode(KEYS[menuIndex]);
        closeMenu();
    } else if (e.key === 'Enter') {
        items[index]?.run();
    } else if (
        e.key === 'Tab' &&
        mode === 'con' &&
        (e.target === input ? input.value.trim() && !e.shiftKey : e.shiftKey)
    ) {
        (e.target === input ? input2 : input).focus();
        closeMenu();
    } else if (e.key === 'Tab') {
        setMode(
            KEYS[(KEYS.indexOf(mode) + (e.shiftKey ? -1 : 1) + KEYS.length) % KEYS.length],
        );
        closeMenu();
    } else if (e.key === 'Escape') {
        // Esc hides the launcher (global Ctrl+Space brings it back).
        if (menuOpen) {
            closeMenu();
        } else if (input.value || input2.value) {
            setMode(mode); // clear input, stay open
        } else {
            dismissLauncher(); // hide the OS window
        }
    } else if (e.key === 'Backspace' && e.target === input2 && !input2.value) {
        input.focus();
    } else {
        return;
    }
    e.preventDefault();
}

input.addEventListener('keydown', onKeydown);
input2.addEventListener('keydown', onKeydown);

// ---------------------------------------------------------------------------
// Input events
// ---------------------------------------------------------------------------

input.addEventListener('input', () => {
    const match = input.value.match(/^([a-z]{3}):\s?/i);
    const key = match?.[1].toLowerCase();
    if (COMMANDS[key]) {
        setMode(key, input.value.slice(match[0].length));
    } else {
        const split =
            mode === 'con' && input.value.match(/^(.*\S)\s+to\s(.*)$/i);
        if (split) {
            input.value = split[1];
            input2.value = split[2];
            input2.focus();
        }
        scheduleUpdate();
    }
});

input2.addEventListener('input', () => scheduleUpdate());

// ---------------------------------------------------------------------------
// Chip button and menu interactions
// ---------------------------------------------------------------------------

chip.addEventListener('mousedown', (e) => e.preventDefault());
chip.addEventListener('click', () => (menu.hidden ? openMenu() : closeMenu()));
menu.addEventListener('mousedown', (e) => e.preventDefault());
list.addEventListener('mousedown', (e) => e.preventDefault());

document.addEventListener('mousedown', (e) => {
    if (!menu.contains(e.target) && e.target !== chip) closeMenu();
});

// ---------------------------------------------------------------------------
// Show / hide the launcher window (Phase 2: global hotkey integration)
//
// State machine:
//   OS window hidden  ──[Ctrl+Space]──►  OS window shown → launcher resets
//   Launcher visible  ──[Esc / outside]──►  animate out → OS window hidden
//
// The global Ctrl+Space shortcut is registered in Rust (main.rs). It calls
// window.show() / window.hide(). We mirror that on the JS side:
//   • When the OS window gets focus (shown by the shortcut) we reset the
//     launcher so it's always clean on open.
//   • When the user dismisses via Escape or outside-click we call
//     hide_window so the OS window is actually hidden — not just the div.
// ---------------------------------------------------------------------------

const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)');

/** Finish the close animation and hide the OS window. */
function finishClose() {
    launcher.hidden = true;
    launcher.classList.remove('closing');
    // Hide the actual OS window so the global shortcut toggles correctly.
    invoke('hide_window').catch(() => {});
}

/** Hide the launcher (with animation). Also hides the OS window afterward. */
function dismissLauncher() {
    closeMenu();
    if (reduceMotion.matches) {
        finishClose();
    } else {
        launcher.classList.add('closing');
    }
}

launcher.addEventListener('animationend', (e) => {
    if (e.animationName === 'sweep-out') finishClose();
});

// Click outside the panel → dismiss.
// Treat the error box as "inside" — clicking it must not close the launcher.
document.addEventListener('mousedown', (e) => {
    if (!launcher.hidden
        && !launcher.contains(e.target)
        && !errorBox.contains(e.target)) {
        dismissLauncher();
    }
});

function resetLauncher() {
    launcher.classList.remove('closing');
    launcher.hidden = false;
    // Replay the open sweep even if the element was never display:none.
    launcher.style.animation = 'none';
    void launcher.offsetWidth;
    launcher.style.animation = '';
    setMode(mode);   // clears the query and re-renders
    input.focus();
}

// Rust emits this when the hotkey shows the window.
const tauriListen = window.__TAURI__?.event?.listen;
if (tauriListen) tauriListen('launcher-shown', resetLauncher);

// ---------------------------------------------------------------------------
// Indexing state polling
// ---------------------------------------------------------------------------

let indexComplete = true;

async function checkIndexReady() {
    try {
        const s = await invoke('index_status');
        const wasIndexing = indexing;
        // Only block search while there is nothing to search yet.
        indexing = !s.complete && !s.has_rows;

        if (!s.complete) {
            indexComplete = false;
            setTimeout(checkIndexReady, 2000);   // keep watching until the crawl ends
        } else if (!indexComplete) {
            indexComplete = true;
            say('Index complete');
        }
        if (wasIndexing && !indexing) update();
    } catch (_err) {
        indexing = false; // never block search because of a status failure
    }
}

// ---------------------------------------------------------------------------
// Bootstrap
// ---------------------------------------------------------------------------

buildMenu();
setMode('fil');
checkIndexReady();
invoke('take_startup_warning').then((msg) => msg && showError(msg)).catch(() => {});
