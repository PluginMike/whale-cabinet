# Whale Cabinet

A native Linux file manager with a flat filing-cabinet look (Tauri 2: Rust backend + TypeScript/Vite frontend).
Window class / Wayland app id: `whale-cabinet`.

Status: **Milestone 1 (core browsing)** done. Theming from DMS/Hyprland comes next.

## Build & run from source (Arch/CachyOS)

Needs `webkit2gtk-4.1`, `base-devel`, `librsvg`, Node ≥ 20 and Rust (rustup).

```sh
npm install
npm run tauri dev             # dev build with hot reload
npm run tauri build           # release binary: src-tauri/target/release/whale-cabinet
whale-cabinet ~/Downloads     # optional start folder (path or file:// URI)
```

fish users with a user-local rustup: `fish_add_path ~/.cargo/bin`.

## Tests

```sh
(cd src-tauri && cargo test)  # Rust unit tests (listing, path resolution, disk space)
scripts/smoke.sh              # e2e: temp tree with 10k files + odd names, drives the real UI, grim screenshots
```

## Usage

- Sidebar drawers are Places; the active one slides out.
- Chevron (or →/←) pulls a folder out inline; double-click or Enter goes into it.
- Click the empty part of the path bar (or Ctrl+L) to type a path; `~` and `file://` work.
- Start typing to filter the current view. Hidden files: Ctrl+H (also honours `.hidden` files).
- Drag from a row's empty space (not its name) to rubber-band select.
- The view refreshes itself when files change on disk.

## Shortcuts

| Key | Action |
|---|---|
| Alt+← / Alt+→ / mouse back/forward | Back / Forward |
| Alt+↑, Backspace | Up |
| Alt+Home | Home |
| Ctrl+L | Edit location |
| Ctrl+H | Show hidden files |
| Ctrl+I, or just type | Filter |
| ↑ ↓ PgUp PgDn Home End | Move (Shift extends, Ctrl moves focus only) |
| Ctrl+Space | Toggle focused item |
| Ctrl+A | Select all |
| → / ← | Pull folder out / push back (or go to parent row) |
| Enter | Open |
| Esc | Clear filter, then selection |
| F5 | Reload |
