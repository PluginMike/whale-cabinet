#!/bin/bash
# Build Whale Cabinet and install it for the current user (no root): ~/.local/bin/whale-cabinet, desktop entry, icons.
#   scripts/install.sh            # install / update (Dolphin stays the default)
#   scripts/install.sh --default  # …and take over from Dolphin: default for folders (inode/directory) and
#                                 #    the FileManager1 D-Bus activation ("Show in folder" from other apps)
#   scripts/install.sh --no-build # install the already-built release binary
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
BIN_DIR="$HOME/.local/bin"
APPS="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
ICONS="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor"
DBUS="${XDG_DATA_HOME:-$HOME/.local/share}/dbus-1/services"
CFG="${XDG_CONFIG_HOME:-$HOME/.config}/whale-cabinet"

# Runtime dependencies: zoxide (jump/frequent), fd (quick open), git (badges), gvfs/gio (network), polkit (admin).
missing=()
command -v zoxide >/dev/null || missing+=(zoxide)
command -v fd >/dev/null || command -v fdfind >/dev/null || missing+=(fd)
command -v git >/dev/null || missing+=(git)
command -v pkexec >/dev/null || missing+=(polkit)
gvfsd=; for f in /usr/lib/gvfsd /usr/libexec/gvfsd /usr/lib/gvfs/gvfsd /usr/libexec/gvfs/gvfsd; do [[ -x $f ]] && gvfsd=$f; done
command -v gio >/dev/null && [[ -n $gvfsd ]] || missing+=(gvfs)
if (( ${#missing[@]} )); then
  . /etc/os-release 2>/dev/null
  case " ${ID:-} ${ID_LIKE:-} " in
    *" arch "*) pkgs="${missing[*]}"; pkgs=${pkgs/gvfs/gvfs gvfs-smb gvfs-dnssd gvfs-nfs}; hint="sudo pacman -S --needed $pkgs" ;;
    *" fedora "*|*" rhel "*) pkgs="${missing[*]}"; pkgs=${pkgs/fd/fd-find}; pkgs=${pkgs/gvfs/gvfs gvfs-smb gvfs-fuse}; hint="sudo dnf install $pkgs" ;;
    *" debian "*|*" ubuntu "*) pkgs="${missing[*]}"; pkgs=${pkgs/fd/fd-find}; pkgs=${pkgs/polkit/pkexec}; pkgs=${pkgs/gvfs/gvfs gvfs-backends gvfs-fuse}; hint="sudo apt install $pkgs" ;;
    *) hint="install: ${missing[*]}" ;;
  esac
  echo "Whale Cabinet needs: ${missing[*]}" >&2
  echo "  $hint" >&2
  exit 1
fi

if [[ " $* " != *" --no-build "* ]]; then
  [[ -d node_modules ]] || npm install
  npm run tauri build -- --no-bundle
fi
BIN=src-tauri/target/release/whale-cabinet
[[ -x $BIN ]] || { echo "no release binary at $BIN — build first" >&2; exit 1; }

# Our .desktop declares inode/directory; if the current folder handler is only an implicit fallback, refreshing
# the user MIME cache below would quietly hand folders to us. Pin the current default first unless --default.
prev=$(xdg-mime query default inode/directory 2>/dev/null || true)
if [[ " $* " != *" --default "* && -n "$prev" && "$prev" != whale-cabinet.desktop ]] && ! grep -qs "^inode/directory=" "${XDG_CONFIG_HOME:-$HOME/.config}/mimeapps.list"; then
  xdg-mime default "$prev" inode/directory
fi

install -Dm755 "$BIN" "$BIN_DIR/whale-cabinet"
install -Dm644 assets/whale-cabinet.desktop "$APPS/whale-cabinet.desktop"
install -Dm644 assets/whale-cabinet.svg "$ICONS/scalable/apps/whale-cabinet.svg"
install -Dm644 src-tauri/icons/128x128.png "$ICONS/128x128/apps/whale-cabinet.png"
install -Dm644 src-tauri/icons/32x32.png "$ICONS/32x32/apps/whale-cabinet.png"
command -v update-desktop-database >/dev/null && update-desktop-database -q "$APPS" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$ICONS" 2>/dev/null || true
echo "installed $BIN_DIR/whale-cabinet"

if [[ " $* " == *" --default "* ]]; then
  mkdir -p "$CFG"
  [[ -n "$prev" && "$prev" != whale-cabinet.desktop ]] && echo "$prev" > "$CFG/previous-file-manager"
  xdg-mime default whale-cabinet.desktop inode/directory
  mkdir -p "$DBUS"
  sed "s|@BIN@|$BIN_DIR/whale-cabinet|" assets/org.freedesktop.FileManager1.service > "$DBUS/org.freedesktop.FileManager1.service"
  echo "default file manager: whale-cabinet.desktop (was: ${prev:-none}; uninstall.sh restores it)"
fi
