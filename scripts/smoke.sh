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
os.makedirs(f"{t}/ops/target")
open(f"{t}/ops/a.md", "w").write("# hello\n")
for n in ["a.md", "b 10.txt", "b 9.txt", "new\nline", "ünïcode 💾.pdf", ".secret", "sub dir/x.png", "sub dir/inner/deep.rs"]:
    open(f"{play}/{n}", "w").close()
os.symlink("nowhere", f"{play}/dangling"); os.mkfifo(f"{play}/pipe")
PY
# a demo plugin for the plugins suite: badges under play/, a demo: location with two "photos", search, actions
mkdir -p "$T/plugins/demo" && cp src-tauri/icons/32x32.png "$T/plugins/demo/thumb.png"
cat > "$T/plugins/demo/plugin.json" <<'JSON'
{"title": "Demo", "description": "Test plugin", "exec": "demo.py", "badges": true, "scheme": "demo", "search": true,
 "settings": [{"key": "url", "label": "URL"}],
 "actions": [{"id": "hello", "label": "Say hello", "roots": true}, {"id": "img", "label": "Images only", "mime": ["image/*"], "roots": true},
             {"id": "own", "label": "Own thing", "own": true}]}
JSON
cat > "$T/plugins/demo/demo.py" <<'PY'
#!/usr/bin/env python3
import json, os, shutil, sys, threading
HERE = os.path.dirname(os.path.abspath(__file__)); T = os.path.dirname(os.path.dirname(HERE))
out = threading.Lock(); cache = ""
def say(m):
    with out: print(json.dumps(m), flush=True)
def photos(): return [{"name": f"pic {i}.png", "path": f"demo:/p/{i}/pic {i}.png", "thumb": True, "size": 1000 + i, "mtime": 1700000000000 + i, "score": i, "info": [["Camera", "Test"]]} for i in range(2)]
def handle(m, a):
    global cache
    if m == "init": cache = a["cache"]; return {"roots": [T + "/play"], "sidebar": [{"title": "Demo photos", "loc": "demo:/"}]}
    if m == "list" and a["loc"] == "demo:/": return {"title": "Demo", "entries": [{"name": "Album", "path": "demo:/album", "dir": True, "count": "2 photos", "score": 9}] + photos()}
    if m == "list" and a["loc"] == "demo:/album": return {"title": "Album", "entries": photos()}
    if m == "thumb": return {"file": HERE + "/thumb.png"}
    if m == "fetch": dst = os.path.join(cache, os.path.basename(a["path"])); shutil.copy(HERE + "/thumb.png", dst); return {"file": dst}
    if m == "badges": return {p: {"text": "✓", "title": "Synced", "state": "ok"} for p in a["paths"]}
    if m == "action": open(T + "/acted", "w").write(a["id"] + " " + " ".join(a["paths"])); say({"event": "badges"}); return {"message": "done " + a["id"]}
    if m == "search": return {"entries": [e for e in photos() if a["q"].lower() in e["name"]], "loc": "demo:/album"}
    raise Exception("unknown " + m)
def run(r):
    try: say({"id": r["id"], "result": handle(r["method"], r.get("params", {}))})
    except Exception as e: say({"id": r["id"], "error": str(e)})
for line in sys.stdin: threading.Thread(target=run, args=(json.loads(line),)).start()
PY
chmod +x "$T/plugins/demo/demo.py"
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
      *"[selftest] ACT dialog-class"*) if hyprctl clients -j | grep -q '"class": "whale-cabinet-dialog"'; then echo "[$label] PASS dialog window class is whale-cabinet-dialog"; else echo "[$label] FAIL dialog window class"; fails_extra=1; fi ;;
      *"DONE fails="*) fails=${l##*=}; pkill -x whale-cabinet ;;
      *"panicked"*|*"error["*) echo "$l" ;;
    esac
    [[ "$l" == *"[selftest]"* ]] && echo "[$label] ${l#*\[selftest\] }"
  done < <(env "$@" timeout 300 npx tauri dev 2>&1)
  return $(( fails > 0 || ${fails_extra:-0} ))
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
