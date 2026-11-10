# Contributing to WayFinder

Thanks for helping! WayFinder is a small, fast Spotlight-style launcher for
Windows. It is maintained by one person, so the rules below keep the code
easy to read and review.

## Our priorities
1. Fast and light: low idle memory and CPU, instant open.
2. Simple, readable code that one person can maintain.
3. Keyboard-first.

## How to contribute
1. Open an issue first for anything bigger than a small fix, so we can agree
   on the approach before you write code.
2. Fork the repo and branch from `updates` (never from or into `main`).
   Name your branch `feature/<short-name>` or `fix/<short-name>`.
3. Keep each pull request small and focused on one change.
4. Open the pull request against `updates`.

## Code guidelines
- Prefer simple, explicit code over clever code. Small functions with clear
  names. Comment the "why".
- Frontend stays vanilla JS. No new frameworks or build tools.
- Rust: no `unwrap()` or `panic!` in app code. Return `Result` with
  plain-English error messages that say what happened and what to try.
- New Cargo dependencies need a reason: say how big they are and why a
  smaller option will not do.
- Never hardcode paths. Never assume a tool is installed.
- More detail lives in `docs/agent-guidelines/`.

## Running it
    npm install
    npm run tauri dev

## License
By contributing you agree your work is released under the MIT License.
