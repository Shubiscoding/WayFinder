// script.js — All UI logic for WayFinder.
// Commands, rendering, keyboard navigation, Tauri invoke calls.

const { invoke } = window.__TAURI__.core;

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
    app: {
        hint: 'Open apps',
        placeholder: 'app name',
        empty: 'No applications found',
        search: searchApps,
    },
    con: {
        hint: 'Convert images',
        placeholder: 'type the name of an image to convert',
        placeholder2: 'target format (e.g. png)',
        empty: 'Type the name of an image to convert',
        search: searchConvert,
    },
    bow: {
        hint: 'Search the web',
        placeholder: 'search the web',
        empty: 'Type to search',
        search: searchWeb,
    },
    sys: {
        hint: 'System commands',
        placeholder: 'quit, settings...',
        empty: 'No matching commands',
        search: searchSys,
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

// Loading screen DOM references
const loadingBox     = document.getElementById('loading-box');
const loadingMsg     = document.getElementById('loading-msg');
const loadingBarWrap = document.getElementById('loading-bar-wrap');
const loadingBar     = document.getElementById('loading-bar');
const loadingDetail  = document.getElementById('loading-detail');
const loadingHint    = document.getElementById('loading-hint');

// Settings modal DOM references (Phase 5.5.5)
const settingsBox   = document.getElementById('settings-box');
const settingsPath  = document.getElementById('settings-path');
const settingsError = document.getElementById('settings-error');
const settingsJson  = document.getElementById('settings-json');
const settingsOpen  = document.getElementById('settings-open');
const settingsClose = document.getElementById('settings-close');

let mode        = 'fil';
let menuIndex   = 0;
let items       = [];
let rows        = [];
let index       = 0;
let statusTimer;

// State for con: mode (image converter file picker)
let pickedSourcePath = null;
let pickedFileName   = '';
let pickedStem       = '';
let pickedExt        = '';
let supportedTargets = [];

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
// Loading screen (reusable)
//
// Nothing here runs on its own — the screen only appears when you call it:
//
//   const stop = showLoading('watcher');   // show the "watcher" preset
//   ...do work...
//   stop();                                // hide it again
//
//   await withLoading(() => invoke('some_command'), 'watcher');
//       // shows the screen, awaits the task, always hides it afterwards
//
//   showLoading('watcher', { detail: 'Almost there', dismissible: true });
//   showLoading('Any custom text…');       // unknown names are used as the message
//   showLoading({ message: 'One-off', timeoutMs: 5000 });
//   hideLoading();                         // force-hide every loading screen
//
// Calls can overlap: the newest one is displayed, and when it finishes the
// previous one is shown again. The screen hides once all of them are done.
//
// To add a new loading screen, add one entry to LOADING_PRESETS.
// ---------------------------------------------------------------------------

/**
 * Preset table (same idea as COMMANDS). Every field is optional:
 *   message      main line of text
 *   detail       smaller second line
 *   dismissible  true → Esc cancels it (default false)
 *   timeoutMs    auto-hide after this many ms (default 0 = never)
 *   onCancel     function called when the user cancels with Esc
 */
const LOADING_PRESETS = {
    watcher: {
        message: 'Watcher is loading data…',
    },
    sync: {
        message: 'Watcher is loading data…',
        detail: 'Preparing index…',
        progress: null,
    },
};

const LOADING_DEFAULTS = {
    message: 'Loading…',
    detail: '',
    progress: null,
    dismissible: false,
    timeoutMs: 0,
    onCancel: null,
};

const loadingStack = [];   // active loading requests, newest last

/** Turn a preset name / message string / options object into full options. */
function resolveLoading(spec, overrides) {
    const base = typeof spec === 'string'
        ? (LOADING_PRESETS[spec] ?? { message: spec })
        : (spec ?? {});
    return { ...LOADING_DEFAULTS, ...base, ...overrides };
}

/** Draw whatever is on top of the stack (or hide the screen if empty). */
function renderLoading() {
    const top = loadingStack.at(-1);
    if (!top) {
        if (!loadingBox.hidden) {
            loadingBox.hidden = true;
            input.focus();
        }
        return;
    }
    loadingMsg.textContent = top.message;
    loadingDetail.textContent = top.detail;
    loadingDetail.hidden = !top.detail;
    loadingHint.hidden = !top.dismissible;

    if (top.progress !== null && top.progress !== undefined) {
        const pct = Math.max(0, Math.min(100, Math.round(top.progress)));
        loadingBar.style.setProperty('--progress', `${pct}%`);
        loadingBarWrap.hidden = false;
    } else {
        loadingBarWrap.hidden = true;
    }

    const wasHidden = loadingBox.hidden;
    loadingBox.hidden = false;
    if (wasHidden) loadingBox.focus();
}

function releaseLoading(entry) {
    const i = loadingStack.indexOf(entry);
    if (i === -1) return;            // already released — safe to call twice
    clearTimeout(entry.timer);
    loadingStack.splice(i, 1);
    renderLoading();
}

/** True while any loading screen is active. */
const isLoading = () => loadingStack.length > 0;

/**
 * Show a loading screen. Returns a `stop()` function that hides this one,
 * with a `.update(updates)` method to update progress, detail, or message.
 * @param {string|object} spec  preset name, message text, or options object
 * @param {object} overrides    options that replace the preset's values
 */
function showLoading(spec = 'watcher', overrides = {}) {
    const entry = resolveLoading(spec, overrides);
    loadingStack.push(entry);
    if (entry.timeoutMs > 0) {
        entry.timer = setTimeout(() => releaseLoading(entry), entry.timeoutMs);
    }
    renderLoading();
    const stopFn = () => releaseLoading(entry);
    stopFn.update = (updates) => {
        Object.assign(entry, updates);
        renderLoading();
    };
    return stopFn;
}

/** Update the topmost active loading screen with new message, detail, or progress. */
function updateLoading(updates) {
    const top = loadingStack.at(-1);
    if (!top) return;
    Object.assign(top, updates);
    renderLoading();
}

/** Force-hide every loading screen. */
function hideLoading() {
    loadingStack.forEach((entry) => clearTimeout(entry.timer));
    loadingStack.length = 0;
    renderLoading();
}

/** Run a task (promise or function) behind a loading screen. */
async function withLoading(task, spec = 'watcher', overrides = {}) {
    const stop = showLoading(spec, overrides);
    try {
        return await (typeof task === 'function' ? task() : task);
    } finally {
        stop();
    }
}

/** Esc on a dismissible loading screen: hide it and run its onCancel. */
function cancelLoading() {
    const top = loadingStack.at(-1);
    if (!top?.dismissible) return false;
    releaseLoading(top);
    top.onCancel?.();
    return true;
}

// Keep keystrokes from leaking to the page while the loading screen has focus.
loadingBox.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') cancelLoading();
    e.preventDefault();
});

// ---------------------------------------------------------------------------
// Settings modal (Phase 5.5.5)
// ---------------------------------------------------------------------------

async function openSettings() {
    try {
        const info = await invoke('get_config');
        settingsPath.textContent = info.path;
        settingsJson.textContent = info.raw_json;
        if (info.error) {
            settingsError.textContent = info.error;
            settingsError.hidden = false;
        } else {
            settingsError.textContent = '';
            settingsError.hidden = true;
        }
        settingsBox.hidden = false;
        settingsBox.focus();
    } catch (err) {
        showError(`Could not load settings: ${err}`);
    }
}

function closeSettings() {
    settingsBox.hidden = true;
    input.focus();
}

settingsOpen?.addEventListener('click', () => {
    invoke('open_config').catch((err) => showError(String(err)));
});

settingsClose?.addEventListener('click', closeSettings);

settingsBox?.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
        closeSettings();
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

/** Search installed applications via the Rust backend. Returns a Promise<item[]>. */
async function searchApps(query) {
    if (indexing) {
        return [{
            tag: '…',
            title: 'Indexing your apps',
            sub: 'Results will appear when done',
            run: () => {},
        }];
    }
    try {
        const hits = await invoke('search_apps', { query: query.trim() });
        return hits.map((h) => ({
            tag: 'app',
            title: h.name,
            sub: h.path,
            run: () => launchApp(h.id),
        }));
    } catch (err) {
        showError(`Application search failed: ${err}`);
        return [];
    }
}

async function launchApp(id) {
    try {
        await invoke('launch_app', { id });
    } catch (err) {
        showError(typeof err === 'string' ? err : (err?.message || String(err)));
    }
}

// ---------------------------------------------------------------------------
// Image conversion (Phase 4 / con: rework)
// ---------------------------------------------------------------------------

function unpickFile() {
    pickedSourcePath = null;
    pickedFileName   = '';
    pickedStem       = '';
    pickedExt        = '';
    supportedTargets = [];
    input2.hidden = toTag.hidden = true;
    input2.value = '';
    bar.classList.remove('convert');
    fitInput();
}

async function pickFile(h) {
    pickedSourcePath = h.path;
    pickedFileName   = h.name;
    const dotIdx = h.name.lastIndexOf('.');
    pickedStem = dotIdx !== -1 ? h.name.slice(0, dotIdx) : h.name;
    pickedExt = (h.extension || (dotIdx !== -1 ? h.name.slice(dotIdx + 1) : '')).toLowerCase();

    try {
        supportedTargets = await invoke('conversion_targets', { ext: pickedExt });
    } catch (_err) {
        supportedTargets = [];
    }

    input.value = h.name;
    toTag.hidden = false;
    input2.hidden = false;
    bar.classList.add('convert');
    fitInput();

    input2.value = `${pickedStem}.`;
    input2.focus();
    input2.setSelectionRange(input2.value.length, input2.value.length);
    update();
}

async function executeConversion(sourcePath, targetName) {
    try {
        const res = await invoke('convert_file', { sourcePath, targetName });
        say(res.message);
    } catch (err) {
        showError(typeof err === 'string' ? err : (err?.message || String(err)));
    }
}

async function searchConvert() {
    if (!pickedSourcePath) {
        if (indexing) {
            return [{
                tag: '…',
                title: 'Indexing your files',
                sub: 'Results will appear when done',
                run: () => {},
            }];
        }
        const q = input.value.trim();
        if (!q) return [];
        try {
            const hits = await invoke('search_convertible', { query: q });
            return hits.map((h) => ({
                tag: h.extension ? `.${h.extension.toLowerCase()}` : 'img',
                title: h.name,
                sub: h.path,
                run: () => pickFile(h),
            }));
        } catch (err) {
            showError(`Search failed: ${err}`);
            return [];
        }
    }

    // A file was picked, now user is editing target name in input2
    const raw = input2.value.trim();
    let resolvedTargetName = '';
    let targetExt = '';

    if (!raw || raw === '.') {
        resolvedTargetName = '';
        targetExt = '';
    } else if (!raw.includes('.')) {
        // User typed just an extension, e.g. "png"
        targetExt = raw.toLowerCase();
        resolvedTargetName = `${pickedStem}.${raw}`;
    } else if (raw.startsWith('.')) {
        // User typed ".png"
        targetExt = raw.slice(1).toLowerCase();
        resolvedTargetName = `${pickedStem}.${raw.slice(1)}`;
    } else {
        // User edited full name, e.g. "Lemon.png" or "Custom.png"
        const dotIdx = raw.lastIndexOf('.');
        targetExt = raw.slice(dotIdx + 1).toLowerCase();
        resolvedTargetName = raw;
    }

    // Determine parent folder for display in action row sub-line
    const parentFolder = pickedSourcePath.replace(/[\\/][^\\/]+$/, '') || pickedSourcePath;

    if (!targetExt) {
        const sampleTargets = supportedTargets.filter((t) => t !== pickedExt).slice(0, 3).join(', ');
        return [{
            tag: 'con',
            title: `Enter target extension${sampleTargets ? ` (e.g. ${sampleTargets})` : ''}`,
            sub: `Supported formats for ${pickedFileName}: ${supportedTargets.join(', ')}`,
            run: () => {},
        }];
    }

    // Canonical format check (jfif/jpeg/jpg, tif/tiff)
    const isSameFormat = targetExt === pickedExt ||
        (['jpeg', 'jfif', 'jpg'].includes(targetExt) && ['jpeg', 'jfif', 'jpg'].includes(pickedExt)) ||
        (['tiff', 'tif'].includes(targetExt) && ['tiff', 'tif'].includes(pickedExt));

    if (isSameFormat) {
        const altTargets = supportedTargets.filter((t) => t !== pickedExt).join(', ');
        return [{
            tag: '!',
            title: `Same format (already .${pickedExt})`,
            sub: `Supported formats: ${altTargets}`,
            run: () => {},
        }];
    }

    if (!supportedTargets.includes(targetExt)) {
        return [{
            tag: '!',
            title: `Unsupported format: .${targetExt}`,
            sub: `Supported formats for ${pickedFileName}: ${supportedTargets.join(', ')}`,
            run: () => {},
        }];
    }

    return [{
        tag: `.${targetExt}`,
        title: `Convert ${pickedFileName} → ${resolvedTargetName}`,
        sub: parentFolder,
        run: () => executeConversion(pickedSourcePath, resolvedTargetName),
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

/** System commands: Settings, Quit WayFinder (Phase 5.5.2 & 5.5.5). */
function searchSys(query) {
    const q = (query || '').trim().toLowerCase();
    const actions = [
        {
            tag: 'sys',
            title: 'Settings',
            sub: 'View configuration (read-only)',
            run: () => openSettings(),
        },
        {
            tag: 'sys',
            title: 'Quit WayFinder',
            sub: 'Exit the application completely',
            run: () => invoke('quit_app'),
        },
    ];

    if (!q) return actions;
    return actions.filter(
        (a) => a.title.toLowerCase().includes(q) || a.sub.toLowerCase().includes(q)
    );
}

// ---------------------------------------------------------------------------
// Mode switching
// ---------------------------------------------------------------------------

function setMode(key, rest = '') {
    if (document.activeElement === input2) input.focus();
    mode = key;
    unpickFile();
    input.value = rest;
    input2.value = '';
    input2.hidden = toTag.hidden = true;
    input2.placeholder = COMMANDS[key].placeholder2 || '';
    bar.classList.remove('convert');
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
    if (mode !== 'con' || !pickedSourcePath) {
        input.style.width = '';
        return;
    }
    input.style.width = `${(input.value || input.placeholder).length + 1}ch`;
}

function getQuery() {
    if (mode === 'con') {
        if (pickedSourcePath) return `${pickedFileName} to ${input2.value}`;
        return input.value.trim();
    }
    return input.value.trim();
}

/** Search again after the user pauses typing. */
function scheduleUpdate() {
    clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => update(), DEBOUNCE_MS);
}

async function update(animate = false) {
    fitInput();
    const query = getQuery();

    // Each search gets a number. If a newer search starts while this one is
    // waiting for Rust, this one is out of date and is thrown away.
    const ticket = ++searchTicket;

    let result = [];
    if (query || mode === 'app') {
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
    launcher.classList.toggle('compact', !query && items.length === 0);
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
    if (isLoading()) {         // loading screen is up: ignore typing and navigation
        if (e.key === 'Escape') cancelLoading();
        e.preventDefault();
        return;
    }
    if (settingsBox && !settingsBox.hidden) {
        if (e.key === 'Escape') {
            closeSettings();
            e.preventDefault();
            return;
        }
    }
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
        !pickedSourcePath &&
        items.length > 0
    ) {
        items[index]?.run();
    } else if (
        e.key === 'Tab' &&
        mode === 'con' &&
        pickedSourcePath
    ) {
        if (e.target === input && !e.shiftKey) {
            input2.focus();
            input2.setSelectionRange(input2.value.length, input2.value.length);
        } else if (e.target === input2 && e.shiftKey) {
            input.focus();
        }
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
    if (isLoading()) {
        input.value = '';
        return;
    }
    const match = input.value.match(/^([a-z]{3}):\s?/i);
    const key = match?.[1].toLowerCase();
    if (COMMANDS[key]) {
        setMode(key, input.value.slice(match[0].length));
    } else {
        if (mode === 'con' && pickedSourcePath) {
            unpickFile();
        }
        scheduleUpdate();
    }
});

input2.addEventListener('input', () => {
    if (isLoading()) {
        input2.value = '';
        return;
    }
    scheduleUpdate();
});

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
// Treat the error box, loading screen and settings modal as "inside" — clicking them must not close the launcher.
document.addEventListener('mousedown', (e) => {
    if (!launcher.hidden
        && !launcher.contains(e.target)
        && !errorBox.contains(e.target)
        && !loadingBox.contains(e.target)
        && (!settingsBox || !settingsBox.contains(e.target))) {
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
    if (isLoading()) {
        loadingBox.focus();
    } else {
        input.focus();
    }
}

// ---------------------------------------------------------------------------
// Startup sync gate & progress (Phase 5.6)
// ---------------------------------------------------------------------------

function formatSyncProgress(data) {
    const { phase, processed, total, elapsed_ms, wait_remaining_s } = data;
    const elapsedSec = (elapsed_ms / 1000).toFixed(1);

    if (phase === 'Waiting') {
        const secs = wait_remaining_s ?? 30;
        return {
            message: 'Watcher is loading data…',
            detail: `Starting in ${secs} s`,
            progress: null,
        };
    }

    if (phase === 'Crawling') {
        // First crawl: total is unknown. Report only processed + elapsed. No fake percentage.
        const detail = `${elapsedSec} s elapsed · ${processed} items`;
        return {
            message: 'Watcher is loading data…',
            detail,
            progress: null,
        };
    }

    if (phase === 'Reconciling') {
        // Startup reconcile: total is known (DB row count).
        let detail = `${elapsedSec} s elapsed`;
        let progress = null;

        if (typeof total === 'number' && total > 0) {
            progress = Math.min(100, Math.round((processed / total) * 100));
            // ETA = elapsed / processed * (total - processed)
            // Show it only after about 1 s and a meaningful processed count (>= 50).
            if (elapsed_ms >= 1000 && processed >= 50) {
                if (processed >= total) {
                    detail += ' · Almost done…';
                } else {
                    const remaining = total - processed;
                    const etaSec = Math.round((elapsed_ms / 1000 / processed) * remaining);
                    detail += ` · about ${etaSec} s left`;
                }
            }
        } else {
            detail += ` · ${processed} items`;
        }

        return {
            message: 'Watcher is loading data…',
            detail,
            progress,
        };
    }

    return {
        message: 'Watcher is loading data…',
        detail: '',
        progress: null,
    };
}

let syncStopFn = null;

async function initSync() {
    try {
        const status = await invoke('sync_status');
        if (status.phase !== 'Ready') {
            indexing = true;
            const initial = formatSyncProgress(status);
            syncStopFn = showLoading('sync', initial);
        } else {
            indexing = false;
        }
    } catch (_err) {
        indexing = false; // never block search because of a status failure
    }
}

// Rust emits this when the hotkey shows the window or tray requests settings.
const tauriListen = window.__TAURI__?.event?.listen;
if (tauriListen) {
    tauriListen('launcher-shown', resetLauncher);
    tauriListen('open-settings', openSettings);

    tauriListen('sync-progress', (event) => {
        const data = event.payload;
        indexing = data.phase !== 'Ready';
        if (data.phase !== 'Ready') {
            const formatted = formatSyncProgress(data);
            if (!syncStopFn && !isLoading()) {
                syncStopFn = showLoading('sync', formatted);
            } else {
                updateLoading(formatted);
            }
        }
    });

    tauriListen('sync-complete', (event) => {
        indexing = false;
        if (syncStopFn) {
            syncStopFn();
            syncStopFn = null;
        }
        hideLoading();
        say('Index complete');
        input.focus();
        update();

        const payload = event.payload;
        if (payload?.error) {
            showError(`Startup sync failed while indexing files:\n\n${payload.error}\n\nWayFinder will continue running, but recent file changes might not appear in search results. Check your folders in Settings (Ctrl+,) or restart the app to try again.`);
        }
    });
}

// ---------------------------------------------------------------------------
// Bootstrap
// ---------------------------------------------------------------------------

buildMenu();
setMode('fil');
initSync();
invoke('take_startup_warning').then((msg) => msg && showError(msg)).catch(() => {});
