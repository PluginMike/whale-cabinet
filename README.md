# Whale Cabinet

A native Linux file manager with a flat filing-cabinet look (Tauri 2: Rust backend + TypeScript/Vite frontend).
Window class / Wayland app id: `whale-cabinet`.



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

## Theming (DankMaterialShell + Hyprland)

Settings (☰) → Theme: **Follow DMS** (default), **Built-in cabinet**, or **Custom** (one seed colour → a Material-style scheme).

With *Follow DMS*, colours come from, in order: DMS `customThemeFile` (when the DMS theme is "custom"),
`~/.cache/DankMaterialShell/dms-colors.json` (the full matugen palette, current dark/light mode),
then `~/.config/gtk-4.0/dank-colors.css`. Those files are watched, so a wallpaper change or dark/light switch
re-themes the app within about a second. Without any of them the built-in cabinet palette is used.

| Material role | Cabinet part |
|---|---|
| surfaceContainerLowest / background | drawer interior (main pane) |
| surfaceContainer | sidebar cabinet body |
| surfaceContainerHigh | drawer fronts |
| primary | selection, active drawer, focus rings |
| primaryContainer | default folder tab |
| surfaceVariant | paper strips |
| outline | dividers |
| error | destructive actions |

Text colours are picked by contrast. The nine preset tag colours are harmonized toward `primary`
(hue pulled up to 15° toward it, Material-style).

From Hyprland (`hyprctl -j getoption`, re-read on `configreloaded`): `decoration:rounding` → corner radius,
`general:gaps_in/out` → spacing, `general:border_size` + `col.active_border` → focus rings (gradient on the
active split pane), `decoration:blur:enabled` → translucent panels. The UI font comes from gsettings
`org.gnome.desktop.interface font-name`.

### Hyprland window rules

The main window's class is `whale-cabinet`; dialogs (Properties, Open With, conflicts, Settings) are
`whale-cabinet-dialog`. There's no titlebar by default (enable the compact one in Settings for floating use).
To let blur show through, set the opacity in Settings below 100% and/or add:

```lua
-- hyprland.lua (Lua config)
hl.window_rule({ match = { class = "^(whale-cabinet)$" }, opacity = "0.95 0.9" })
hl.window_rule({ match = { class = "^(whale-cabinet-dialog)$" }, float = true, center = true })
```

```ini
# hyprland.conf (classic config)
windowrulev2 = opacity 0.95 0.9, class:^(whale-cabinet)$
windowrulev2 = float, class:^(whale-cabinet-dialog)$
windowrulev2 = center, class:^(whale-cabinet-dialog)$
```
