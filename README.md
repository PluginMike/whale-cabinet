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
- Drag items onto a folder, a Places drawer or another pane to move them (hold Ctrl to copy); drag past the
  window edge to hand them to another app. Files dropped in from other apps land where you drop them.
- Long operations show in a progress panel with Cancel; name clashes open a conflict window
  (Skip / Overwrite / Rename / apply to all).

### Previews

- Image, video and PDF thumbnails load lazily in the list and grid, using the shared freedesktop cache
  (`~/.cache/thumbnails`, same as Dolphin/Nautilus). Missing ones are generated (videos need `ffmpegthumbnailer`,
  PDFs `pdftoppm` from poppler).
- The info panel (F11) previews the selection: images, video/audio players, the first page of PDFs,
  syntax-highlighted text/code, and rendered Markdown (tables, task lists, code blocks, relative images and links;
  "View source" toggles). Markdown is sanitized before it's shown.
- Folders show their item count; "Calculate total size" walks the whole tree on demand.

### Right-click menu, Open With, Properties

- The context menu has Open, **Open With** (apps registered for the file type from `.desktop` files and
  `mimeapps.list`, default first, with icons from your icon theme) and **Other Application…** (searchable,
  with "Always use this", which writes `~/.config/mimeapps.list`), Open in New Tab, Open Terminal Here
  (Settings → Terminal, else `$TERMINAL`, else kitty/foot/ghostty/alacritty/…), cut/copy/paste/rename/duplicate,
  Copy Path, Compress (zip, tar.gz/xz/zst, 7z — whatever is installed) and Extract Here (bsdtar, 7z fallback),
  Colour and Tags submenus, Trash/Delete and Properties.
- Double-click / Enter opens files with their default app from `mimeapps.list` (xdg-open as fallback).
- Properties (its own `whale-cabinet-dialog` window): General (editable name, type, location, size/contents,
  created/modified/accessed, link target), Permissions (owner/group, read/write/execute; optional recursive apply —
  files never gain an execute bit they didn't have), Details (image size, EXIF, audio/video tags and streams via
  `ffprobe`), Checksums (MD5 / SHA-256 on demand, compare with a pasted value).

### Tags, colours, ratings (shared with Dolphin)

- Tags live in the `user.xdg.tags` extended attribute (comma-separated), ratings in `user.baloo.rating`
  (0–10, two per star) — exactly where Dolphin/Baloo keep them, so they carry over both ways.
- A colour is a tag named `color:red` (any of red, orange, yellow, green, teal, blue, purple, pink, grey)
  or `color:#rrggbb`; it shows as the folder's tab or the file's paper edge.
- Filesystems without xattrs (FAT, some network mounts) fall back to `~/.config/whale-cabinet/tags.json`.
- The sidebar's Tags section lists every tag with a count; click one to see everything tagged with it.
  The index (`~/.config/whale-cabinet/tag-index.json`) is seeded by a background scan of your home folder
  and kept current as you browse and edit.
- Edit tags, colour and rating in the info panel (autocompletes known tags; works on a multi-selection),
  or drag items onto a tag in the sidebar.

### Data safety

- Copies go to a hidden temp file and are renamed into place, so a cancelled or failed copy never leaves a
  half-written file under the real name and never truncates the file it would replace.
- Moves across filesystems copy, `fsync`, re-read and compare a CRC32 of every file, and only then delete the source.
- Renames and moves use `renameat2(RENAME_NOREPLACE)`: nothing is silently overwritten.
- "Overwrite" between a file and a folder sends the old item to the Trash instead of deleting it.
- Symlinks are copied as links and deleted without following; pipes/sockets/devices are skipped, not read.

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
| Ctrl+C / Ctrl+X / Ctrl+V | Copy / Cut / Paste (system clipboard, `text/uri-list`) |
| F2 | Rename inline |
| Ctrl+D | Duplicate |
| Ctrl+Shift+N, F10 | New folder |
| Ctrl+Alt+N | New file |
| Del / Shift+Del | Move to Trash / Delete permanently (asks first) |
| Ctrl+R (in Trash) | Restore |
| Ctrl+Z | Undo last operation |
| Menu key, Shift+F10 | Context menu |
| Alt+Enter | Properties |
| Shift+F4 | Open terminal here |
| Space | Quick Look (←/→ flip through files, Esc closes) |
| F11 | Show/hide the info panel |

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
