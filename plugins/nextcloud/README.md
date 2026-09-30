# Nextcloud

Sync status and sharing for your Nextcloud folders, through the **Nextcloud desktop client** that's already
running (the same local socket Dolphin's and Nautilus' overlays use).

## What it does

- **Badges** on everything in your synced folders (read from `~/.config/Nextcloud/nextcloud.cfg`), updated live
  while the client syncs:
  **✓** synced (blue when shared) · **↻** syncing · **!** a problem · **⊘** excluded
- **Right-click → Nextcloud**
  - *Share…* opens the client's share dialog
  - *Copy Public Link* puts the item's public link on the clipboard (reusing one that exists, or creating it)
  - *Open in Browser* shows the item in Nextcloud's web interface

## Setup

1. Have the Nextcloud desktop client running and logged in.
2. Allow the plugin when Whale Cabinet asks.
3. For *Copy Public Link* only: create an app password in Nextcloud (Settings → Security → *Create new app
   password*) and paste it into Settings → Plugins → Nextcloud. It's kept in your keyring.

## Notes

- If the client isn't connected to your server, *Share…* says so; the other actions still work.
- To browse the server without the client, use Network → *Connect to server…* with
  `davs://your.server/remote.php/dav/files/USER/`.
- Test: `python3 test_nextcloud.py`
