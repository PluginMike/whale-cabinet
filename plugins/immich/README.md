# Immich

Your [Immich](https://immich.app) photos in the file manager: browse, search, download, upload and delete.

## What it does

- A sidebar section with **Timeline** (a folder per month), **Albums** and **Favorites**, shown as thumbnails,
  with the photo's details (date, camera, exposure, place) in the info panel.
- **Space** previews a photo; **Enter** opens the original (downloaded to `~/.cache/whale-cabinet/plugins/immich/`).
- **Copy / Copy To… / drag out** copy the originals anywhere, or into other apps.
- **Smart search**: Ctrl+F inside Immich (or Ctrl+Shift+P → *Immich: search…*). Describe what's in the picture;
  Enter opens a result, Shift+Enter shows all of them.
- **Upload**: paste (Ctrl+V) or drop files onto an Immich location or its sidebar entry, or right-click local
  photos → Immich → *Upload to Immich*. Into an album adds them to it; into Favorites marks them.
- **Delete**: Del moves photos to Immich's trash (after asking), so they can be restored there.
- Right-click → *Open in Immich* opens the photo in the web app.

## Setup

1. In Immich: Account Settings → API Keys → *New API Key*. For browsing it needs `asset.read`, `asset.view`,
   `asset.download`, `album.read` and `timeline.read`; for uploading and deleting also `asset.upload`,
   `albumAsset.create`, `asset.update` and `asset.delete`.
2. Settings → Plugins → Immich: your server URL (`https://photos.example.com`) and the API key (kept in your keyring).

## Notes

- Months are cut in UTC, so a photo taken just after midnight on the 1st can show up in the month before.
- Uses only Python's standard library.
