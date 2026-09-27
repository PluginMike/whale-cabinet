#!/bin/bash
# Remove Whale Cabinet's user install and hand folders back to the previous file manager (Dolphin by default).
# Your tags/ratings (xattrs), bookmarks (user-places.xbel) and ~/.config/whale-cabinet settings are left alone.
set -uo pipefail
APPS="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
ICONS="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor"
DBUS="${XDG_DATA_HOME:-$HOME/.local/share}/dbus-1/services"
CFG="${XDG_CONFIG_HOME:-$HOME/.config}/whale-cabinet"

prev=$(cat "$CFG/previous-file-manager" 2>/dev/null || echo org.kde.dolphin.desktop)
if [[ "$(xdg-mime query default inode/directory 2>/dev/null)" == whale-cabinet.desktop ]]; then
  xdg-mime default "$prev" inode/directory && echo "default file manager restored: $prev"
fi
pkill -x whale-cabinet 2>/dev/null
rm -fv "$HOME/.local/bin/whale-cabinet" "$APPS/whale-cabinet.desktop" \
  "$ICONS/scalable/apps/whale-cabinet.svg" "$ICONS/128x128/apps/whale-cabinet.png" "$ICONS/32x32/apps/whale-cabinet.png"
grep -qs whale-cabinet "$DBUS/org.freedesktop.FileManager1.service" && rm -fv "$DBUS/org.freedesktop.FileManager1.service"
command -v update-desktop-database >/dev/null && update-desktop-database -q "$APPS" || true
echo "Whale Cabinet removed. If you bound it in Hyprland, point that bind back at dolphin."
