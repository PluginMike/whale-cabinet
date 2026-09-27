#!/bin/bash
# End-to-end smoke test. Builds a temp tree (10k files + awkward names), runs the dev app with the built-in
# self-test (src/selftest.ts), answers its ACT markers (touch files, toggle DMS dark/light, next wallpaper),
# takes grim screenshots on SHOT markers, then re-runs the theme suite with an empty $HOME to check the
# built-in fallback. Exits non-zero on any failure.
#   WC_SUITES=browse,theme scripts/smoke.sh     # pick suites (default: all)
set -u
cd "$(dirname "$0")/.."
T=$(mktemp -d /tmp/whale-smoke.XXXXXX)
python3 - "$T" <<'PY'
import os, sys
t = sys.argv[1]; big = f"{t}/big"; play = f"{t}/play"
os.makedirs(big); os.makedirs(f"{play}/sub dir/inner"); os.makedirs(f"{t}/fakehome")
for i in range(10000): open(f"{big}/file {i}.{['txt','png','rs','mp3','zip'][i%5]}", "w").close()
for i in range(300): os.mkdir(f"{big}/folder {i}")
for n in ["a.md", "b 10.txt", "b 9.txt", "new\nline", "ünïcode 💾.pdf", ".secret", "sub dir/x.png", "sub dir/inner/deep.rs"]:
    open(f"{play}/{n}", "w").close()
os.symlink("nowhere", f"{play}/dangling"); os.mkfifo(f"{play}/pipe")
PY
export PATH=$HOME/.cargo/bin:$PATH WC_SELFTEST=$T CARGO_HOME=${CARGO_HOME:-$HOME/.cargo} RUSTUP_HOME=${RUSTUP_HOME:-$HOME/.rustup}
shot() {
  command -v grim >/dev/null && command -v hyprctl >/dev/null || return 0
  g=$(hyprctl clients -j | python3 -c "import json,sys;c=[c for c in json.load(sys.stdin) if c['class']=='whale-cabinet'][0];print(f\"{c['at'][0]},{c['at'][1]} {c['size'][0]}x{c['size'][1]}\")") && grim -g "$g" "$T/$1.png"
}
run() {  # $1 = label, rest = env overrides
  local label=$1; shift
  local fails=1
  while IFS= read -r l; do
    case "$l" in
      *"[selftest] SHOT "*) shot "$label-${l##* }" ;;
      *"[selftest] ACT touch"*) touch "$T/play/live1" "$T/play/live2" ;;
      *"[selftest] ACT theme-toggle"*) dms ipc call theme toggle >/dev/null 2>&1; (sleep 1.5; pin_ws) & ;;
      *"[selftest] ACT wallpaper-next"*) dms ipc call wallpaper next >/dev/null 2>&1; (sleep 1.5; pin_ws) & ;;
      *"DONE fails="*) fails=${l##*=}; pkill -x whale-cabinet ;;
      *"panicked"*|*"error["*) echo "$l" ;;
    esac
    [[ "$l" == *"[selftest]"* ]] && echo "[$label] ${l#*\[selftest\] }"
  done < <(env "$@" timeout 300 npx tauri dev 2>&1)
  return $(( fails > 0 ))
}
# Keep test windows off real screens: if a headless output named WCTEST exists (hyprctl output create
# headless WCTEST), send whale-cabinet windows to its workspace. Runtime rule only; re-added per run since
# DMS theme changes reload Hyprland's config.
pin_ws() {
  local ws; ws=$(hyprctl monitors -j 2>/dev/null | python3 -c "import json,sys;print(next((str(m['activeWorkspace']['id']) for m in json.load(sys.stdin) if m['name']=='WCTEST'),''))")
  [[ -n "$ws" ]] && hyprctl eval "hl.window_rule({ match = { class = \"^(whale-cabinet.*)\$\" }, workspace = \"$ws silent\" })" >/dev/null
}
rc=0
pin_ws
run main || rc=1
[[ ",${WC_SUITES:-theme}," == *",theme,"* ]] && { pin_ws; WC_SUITES=theme run fallback HOME="$T/fakehome" || rc=1; }
echo "screenshots + test tree: $T"
exit $rc
