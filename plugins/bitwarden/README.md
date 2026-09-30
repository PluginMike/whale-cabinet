# Bitwarden

Search your Bitwarden vault from the command palette and copy a password, username or TOTP code. Works with
bitwarden.com and self-hosted servers (Bitwarden, Vaultwarden).

## What it does

Ctrl+Shift+P → *Bitwarden: search…*, then type part of an item's name, username or folder:

- **Enter** copies the password
- **Shift+Enter** copies the username
- **Ctrl+Enter** copies the TOTP code

Passwords and codes are copied marked as sensitive (so clipboard managers can skip them) and cleared after
30 seconds, unless you've copied something else since.

## Setup

1. Install [rbw](https://github.com/doy/rbw), the unofficial Bitwarden CLI: `sudo pacman -S rbw` (Arch),
   `cargo install rbw` elsewhere. It also needs `pinentry` and `wl-clipboard`.
2. Settings → Plugins → Bitwarden: your server URL (leave empty for bitwarden.com) and email.
3. Search. The first time, rbw asks you to log in; after that it asks for your master password when the vault
   is locked.

## Security

The plugin never sees your master password: rbw's agent unlocks the vault through pinentry and keeps it
unlocked for its `lock_timeout` (`rbw config set lock_timeout 3600`). Secrets go from rbw straight to the
clipboard; search results only carry names, usernames and folders.

Test: `python3 test_bitwarden.py`
