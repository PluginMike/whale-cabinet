# Development

## Branches

- **`dev`** is where all development lands. Never commit or push straight to `main`.
- Start every change on a branch off `dev` (`git switch dev && git pull && git switch -c <short-name>`), then open a PR into `dev`.
- **`main`** only moves by one big merge of `dev` when it's time for a release. That merge is done by hand, as a PR from `dev` to `main`.

## Releasing

1. On `dev`, bump the version in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`, plus both lockfiles (`npm install --package-lock-only`, `cargo update -p whale-cabinet`). Do this once per release, not once per change.
2. Merge `dev` into `main`.
3. Tag `main` with `v<version>` and push the tag. `.github/workflows/release.yml` builds the AppImage and .deb and writes the changelog.

## Commits

- Author: `PluginMike <65072741+PluginMike@users.noreply.github.com>` (already in the repo's git config).
- Short one-line messages in plain English ("Fit viewer images to the window").
- No `Co-Authored-By`, "Generated with" or other tool/AI attribution in commits, PRs or code comments.

## Checks before pushing

```sh
(cd src-tauri && cargo test)   # Rust unit tests
npx tsc --noEmit               # TypeScript
scripts/smoke.sh               # end-to-end self-test in the dev app (slower; run for UI changes)
```

`scripts/install.sh` builds and installs the app locally (`~/.local/bin/whale-cabinet`). Restart the running app to pick it up.

## Rules for tests

- Plugin upload, delete and share tests run only against the smoke demo plugin, never against real servers (Immich, Nextcloud, Bitwarden).
- Test windows open on the real screen. Never create a headless or virtual monitor.
