# Whale Cabinet

A native Linux file manager with a flat filing-cabinet look (Tauri 2: Rust backend + TypeScript/Vite frontend).
Window class / Wayland app id: `whale-cabinet`.



## Install

```sh
scripts/install.sh              # build + install to ~/.local/bin (Dolphin stays the default)
scripts/install.sh --default    # …and take over from Dolphin: default for folders + "Show in folder" (D-Bus)
scripts/uninstall.sh            # remove it and give folders back to the previous file manager (Dolphin)
```

Everything is per-user; nothing needs root. `--default` runs
`xdg-mime default whale-cabinet.desktop inode/directory` (remembering the previous handler for uninstall) and
installs a `org.freedesktop.FileManager1` D-Bus activation file, so browsers' "Show in folder" starts Whale Cabinet
with the file selected. While it runs, it also claims that D-Bus name from Dolphin.

Only one process runs: a second `whale-cabinet …` (or your SUPER+E keybind) opens a new window on the workspace
you're on, sharing the clipboard, jobs and tags with the others. Ctrl+N does the same from inside.

```sh
whale-cabinet ~/Downloads                  # folders
whale-cabinet file:///etc/hosts            # file:// URIs (a file opens its folder with it selected)
whale-cabinet --select ~/Pictures/cat.jpg  # reveal an item
```

### Hyprland keybind

```lua
-- hyprland.lua / config/keybinds.lua (Lua config)
hl.bind("SUPER + E", hl.dsp.exec_cmd("uwsm app -- whale-cabinet"))
```
```ini
# hyprland.conf (classic config)
bind = SUPER, E, exec, whale-cabinet
```

### AppImage

`npm run tauri build -- --bundles appimage` → `src-tauri/target/release/bundle/appimage/Whale Cabinet_<version>_amd64.AppImage`
(self-contained, ~100 MB because it carries WebKitGTK).

## Build from source

Needs Rust (via [rustup](https://rustup.rs)), Node ≥ 20, WebKitGTK 4.1 and the usual build tools.
Required at runtime (install.sh checks): zoxide (jump box, Frequent), fd (quick open), git (badges), gvfs with its
smb/dav/nfs backends (network locations) and polkit (administrator actions).
The optional tools light up extra features: poppler (PDF previews), ffmpegthumbnailer + ffmpeg (video thumbnails,
media details), libarchive/zip/7-Zip (compress/extract), wl-clipboard (copy/paste with other apps), udisks2 (devices),
libsecret's secret-tool (remembering network passwords).

**Arch / CachyOS / Manjaro**
```sh
sudo pacman -S --needed base-devel webkit2gtk-4.1 curl wget file openssl librsvg libappindicator-gtk3 nodejs npm rustup
sudo pacman -S --needed zoxide fd git gvfs gvfs-smb gvfs-dnssd gvfs-nfs polkit
sudo pacman -S --needed poppler ffmpegthumbnailer ffmpeg libarchive zip 7zip wl-clipboard udisks2   # optional
```

**Fedora**
```sh
sudo dnf install webkit2gtk4.1-devel openssl-devel curl wget file libappindicator-gtk3-devel librsvg2-devel libxdo-devel nodejs npm
sudo dnf group install c-development
sudo dnf install zoxide fd-find git gvfs gvfs-smb gvfs-fuse polkit
sudo dnf install poppler-utils ffmpegthumbnailer ffmpeg-free bsdtar zip p7zip wl-clipboard udisks2   # optional
```

**Debian / Ubuntu**
```sh
sudo apt install build-essential libwebkit2gtk-4.1-dev libssl-dev libxdo-dev libayatana-appindicator3-dev librsvg2-dev curl wget file nodejs npm
sudo apt install zoxide fd-find git gvfs gvfs-backends gvfs-fuse pkexec
sudo apt install poppler-utils ffmpegthumbnailer ffmpeg libarchive-tools zip 7zip wl-clipboard udisks2   # optional
```

Then:
```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # if you don't have Rust
npm install
npm run tauri dev       # run from source with hot reload
scripts/install.sh      # build a release and install it
```
fish users with a user-local rustup: `fish_add_path ~/.cargo/bin`.

## Tests

```sh
(cd src-tauri && cargo test)   # 63 Rust unit tests: file ops, tags/xattrs, theme parsing (dank-colors.css,
                               # dms-colors.json, hyprctl JSON), .desktop/mimeapps/globs, thumbnails, xbel, search, pty,
                               # fuzzy ranking, git porcelain, recently-used.xbel, gio prompts, admin helper, duplicates…
(cd src-tauri && cargo test real_system -- --ignored --nocapture)   # Open With / icons against this machine
scripts/smoke.sh               # launches the app: end-to-end UI run against a temp tree (10k files, odd names),
                               # file ops, DMS re-theme timing, built-in fallback; grim screenshots
```

## Usage

- Sidebar drawers are Places; the active one slides out. Sections (Places, Recent, Frequent, Devices, Network, Tags,
  Tools) collapse with a click on their heading, and stay that way.
- Chevron (or →/←) pulls a folder out inline; double-click or Enter goes into it.
- Click the empty part of the path bar (or Ctrl+L) to type a path; `~` and `file://` work.
- Start typing to filter the current view. Hidden files: Ctrl+H (also honours `.hidden` files).
- Drag from a row's empty space (not its name) to rubber-band select.
- Settings → Item size scales files and folders (Ctrl+scroll zooms one view); Settings → "Double-clicking a folder"
  chooses between opening it (Windows style, default) and pulling it out inline (cabinet style).
- Drag the info panel's left edge to resize it (double-click the edge to reset); "⤢ Open large" pops a preview
  out full size.
- The view refreshes itself when files change on disk.
- Drag items onto a folder, a Places drawer or another pane to move them (hold Ctrl to copy); drag past the
  window edge to hand them to another app. Files dropped in from other apps land where you drop them.
- Long operations show in a progress panel with Cancel; name clashes open a conflict window
  (Skip / Overwrite / Rename / apply to all).

### Finding things: quick open, fuzzy search, zoxide

- **Ctrl+P** opens quick open: type a few letters of anything under your home ("dl rep" → `Downloads/report.pdf`).
  The list comes from `fd` (hidden and git-ignored files skipped) and is ranked by nucleo, the fuzzy matcher from
  Helix; matched letters are highlighted and folders you use a lot (zoxide) rank a little higher. Enter opens,
  Shift+Enter shows the item in its folder, Ctrl+Enter opens it in a new tab.
- **Ctrl+F** has a *Fuzzy* option: letters in order, best matches first (the Relevance sort), and a picker for
  files and folders / files only / folders only. In quick open, Tab switches between All, Files and Folders.
- Every folder you open is fed to `zoxide add`, so your shell's `z` learns from the file manager too.
  **Ctrl+J** (or typing `z foo` in the path bar) jumps by frecency; the sidebar's *Frequent* section lists your top folders.

### Focus mode

F12 hides everything but the files (sidebar, info panel, top and status bars). Touch the top edge of the window
(or press Ctrl+L) and the top bar slides back in; F12 or Esc leaves focus mode. It's remembered.

### Git

Inside a git work tree, items get badges: **M** modified, **+** staged, **?** untracked, **!** conflict; ignored
items are dimmed, and folders show the most important state inside them. The top bar shows the branch with
↑ahead/↓behind. One `git status` per repository, re-run when files (or the repo's `.git`) change. A dotfiles repo
at `~` only shows real changes, not "untracked".

### Recent files

Recent (sidebar) lists what you opened lately, grouped Today / Yesterday / This week / … with bigger rows and
the app each file was opened with; the newest five are right in the sidebar. It's the shared
`~/.local/share/recently-used.xbel`, so files opened from GTK apps show up and files opened here show up there.
Right-click → *Remove from Recent*; the info panel has *Clear history*.

### Network locations

Sidebar → Network → *Connect to server…*: `sftp://host/`, `smb://server/share`, `davs://host/path`, `nfs://host/export`
or `ftp://`. Hosts from `~/.ssh/config` are one click; *Browse network…* lists SMB workgroups, servers and shares.
Whale Cabinet runs `gio mount` and answers its questions in the dialog (user, domain, password; "trust this host?"),
then you browse the gvfs folder like any other. Passwords can be kept in your keyring (secret-tool).
Saved connections live in `user-places.xbel`, so Dolphin sees them too; ⏏ disconnects.

### Administrator actions

- When a copy, move, delete or trash fails with "permission denied", the job card offers *Retry as administrator*;
  rename and new folder/file ask the same. These run in a small helper (`pkexec whale-cabinet --admin-helper`), so
  polkit asks for your password every time and the window itself never runs as root. Conflicts and progress work
  as usual.
- Right-click a file → Administrator → *Open as Administrator* runs its default app as root, or *Edit as
  Administrator* opens a private copy in your normal editor; each save is written back to the original (keeping its
  owner and permissions), asking for the password each time. Use the latter for editors that refuse to run as root.

### Duplicate files

Sidebar → Tools → *Find duplicates…*: choose a folder and a minimum size. Files are compared by size, then the
first 64 KiB, then their full SHA-256 (hard links count once). Sets appear biggest waste first; the info panel
selects the extra copies keeping the newest, the oldest or the one in a folder you pick, then *Move to Trash*
(Ctrl+Z brings them back).

### Previews

- Image, video and PDF thumbnails load lazily in the list and grid, using the shared freedesktop cache
  (`~/.cache/thumbnails`, same as Dolphin/Nautilus). Missing ones are generated (videos need `ffmpegthumbnailer`,
  PDFs `pdftoppm` from poppler).
- The info panel (F11) previews the selection: images, video/audio players, the first page of PDFs,
  syntax-highlighted text/code, and rendered Markdown (tables, task lists, code blocks, relative images and links;
  "View source" toggles). Markdown is sanitized before it's shown.
- Folders show their item count; "Calculate total size" walks the whole tree on demand.

### Tabs, split view, terminal, search, Places, devices

- Tabs and a split view (F3) like Dolphin; each view has its own history, view mode and zoom.
- F4 opens a terminal (your `$SHELL` in a PTY) under the view, coloured from DMS's terminal palette. It `cd`s along
  as you browse — but only while the shell is idle at its prompt, never into a running program.
- Ctrl+F searches recursively from here, your home, or everywhere, by name or by content (text files up to 20 MB);
  results stream in and show where each item lives.
- Places are read from and written to `~/.local/share/user-places.xbel`, so Dolphin and KDE/Qt file dialogs see
  the same bookmarks. Drag a folder onto "+ Drop a folder here" (or right-click → Add to Places), drag drawers to
  reorder, right-click a drawer to rename or remove it. Edits made in Dolphin show up live.
- Devices lists removable and extra drives (via `lsblk`) and network/FUSE mounts; click to mount (udisks2, polkit
  may ask), ⏏ to unmount or safely remove.

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
- Your own tags can have their own colour ("Work" = blue): it shows on the item's tag dots, in the sidebar and
  menus, and colours the item's edge when it has no colour of its own. Right-click → Tags → *New Tag…* creates
  one (name + colour) and applies it to the selection.
- *Manage tags…* (bottom of the sidebar's Tags section, or right-click a tag) renames, recolours and deletes
  tags — renaming or deleting updates every indexed item that has it — and lets you give the nine colours your
  own names (red = "Urgent"). Tag colours and colour names live in Whale Cabinet's settings; the files keep
  plain tag names, so Dolphin still reads them.

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
| Ctrl+L, F6 | Edit location |
| Ctrl+H, Alt+. | Show hidden files |
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
| Ctrl+T / Ctrl+W | New tab / close tab (middle-click a folder or drawer: open in a tab) |
| Ctrl+Tab, Ctrl+PgDn / Ctrl+Shift+Tab, Ctrl+PgUp | Next / previous tab |
| F3 | Split view |
| F4 | Terminal panel (follows the folder) |
| F9 | Show/hide the sidebar |
| Ctrl+F | Search (names, `*`/`?` globs; optional content search) |
| Ctrl+1 / Ctrl+2 / Ctrl+3 | Icons / compact / cabinet list |
| Ctrl+scroll, Ctrl+= / Ctrl+- / Ctrl+0 | Zoom in / out / reset |
| Ctrl+P | Quick open (fuzzy, whole home) |
| Ctrl+J | Jump to a frequent folder (zoxide); also `z foo` in the path bar |
| Ctrl+N | New window |
| F12 | Focus mode |
| Ctrl+Q | Close window |
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
