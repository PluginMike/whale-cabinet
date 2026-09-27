#!/bin/bash
# End-to-end smoke test: builds a temp tree (10k files + awkward names), runs the dev app with the
# built-in self-test (src/selftest.ts), screenshots it with grim when available, exits non-zero on failure.
set -u
cd "$(dirname "$0")/.."
T=$(mktemp -d /tmp/whale-smoke.XXXXXX)
python3 - "$T" <<'PY'
import os, sys
t = sys.argv[1]; big = f"{t}/big"; play = f"{t}/play"
os.makedirs(big); os.makedirs(f"{play}/sub dir/inner")
for i in range(10000): open(f"{big}/file {i}.{['txt','png','rs','mp3','zip'][i%5]}", "w").close()
for i in range(300): os.mkdir(f"{big}/folder {i}")
for n in ["a.md", "b 10.txt", "b 9.txt", "new\nline", "ünïcode 💾.pdf", ".secret", "sub dir/x.png", "sub dir/inner/deep.rs"]:
    open(f"{play}/{n}", "w").close()
os.symlink("nowhere", f"{play}/dangling"); os.mkfifo(f"{play}/pipe")
PY
export PATH=$HOME/.cargo/bin:$PATH WC_SELFTEST=$T
shot() {
  command -v grim >/dev/null && command -v hyprctl >/dev/null || return 0
  g=$(hyprctl clients -j | python3 -c "import json,sys;c=[c for c in json.load(sys.stdin) if c['class']=='whale-cabinet'][0];print(f\"{c['at'][0]},{c['at'][1]} {c['size'][0]}x{c['size'][1]}\")") && grim -g "$g" "$T/$1.png"
}
fails=1
while IFS= read -r l; do
  case "$l" in
    *"[selftest] SHOT "*) shot "${l##* }" ;;
    *"[selftest] TOUCH"*) touch "$T/play/live1" "$T/play/live2" ;;
    *"DONE fails="*) fails=${l##*=}; shot final; pkill -x whale-cabinet ;;
  esac
  [[ "$l" == *"[selftest]"* ]] && echo "$l"
done < <(timeout 300 npx tauri dev 2>&1)
echo "screenshots + test tree: $T"
exit $(( fails > 0 ))
