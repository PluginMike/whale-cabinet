#!/usr/bin/env python3
"""Whale Cabinet plugin: search a Bitwarden vault (bitwarden.com or self-hosted, e.g. Vaultwarden) through rbw
and copy a password / username / TOTP code. rbw's agent holds the unlocked vault and asks for the master
password with pinentry, so it never passes through here. Secrets go straight from rbw to wl-copy (on stdin,
marked sensitive so clipboard managers can skip them) and are cleared after 30 s unless something else was
copied meanwhile. The item list kept here has names, usernames and folders only. Standard library only."""
import json, shutil, subprocess, sys, threading, time

out = threading.Lock()
def say(msg):
    with out:
        print(json.dumps(msg), flush=True)

CLEAR_AFTER = 30
cache = {"items": [], "at": 0.0}
setup = {}
lock = threading.Lock()  # one rbw unlock / list at a time (pinentry must not pop up twice)

def rbw(*args, timeout=30):
    try:
        return subprocess.run(["rbw", *args], capture_output=True, text=True, timeout=timeout, stdin=subprocess.DEVNULL)
    except FileNotFoundError:
        raise Exception("rbw isn't installed (sudo pacman -S rbw)")
    except subprocess.TimeoutExpired:
        raise Exception(f"rbw {args[0]} took too long")

def fail(r, what):
    msg = (r.stderr or r.stdout).strip().splitlines()
    raise Exception(f"rbw {what}: {msg[-1] if msg else f'exit {r.returncode}'}")

def configure(url, email):
    """Point rbw at your server and account (only when they differ: a change means logging in again)."""
    if not email:
        return
    r = rbw("config", "show")
    cur = json.loads(r.stdout) if r.returncode == 0 and r.stdout.strip().startswith("{") else {}
    want = {"email": email, "base_url": url or None}
    if all(cur.get(k) == v for k, v in want.items()):
        return
    rbw("stop-agent")  # the agent keeps the old config
    for k, v in want.items():
        r = rbw("config", "set", k, v) if v else rbw("config", "unset", k)
        if r.returncode != 0:
            fail(r, f"config {k}")

def unlock():
    """Unlocked vault (asking through pinentry, logging in first when needed)."""
    if rbw("unlocked").returncode == 0:
        return
    r = rbw("unlock", timeout=300)
    if r.returncode != 0 and "log" in (r.stderr + r.stdout).lower():  # "not logged in"
        r = rbw("login", timeout=300)
        if r.returncode != 0:
            fail(r, "login")
        r = rbw("unlock", timeout=300)
    if r.returncode != 0:
        fail(r, "unlock")

def items():
    with lock:
        unlock()
        if time.time() - cache["at"] > 60:
            r = rbw("list", "--fields", "id,name,user,folder")
            if r.returncode != 0:
                fail(r, "list")
            cache["items"] = [dict(zip(["id", "name", "user", "folder"], (l.split("\t") + ["", "", "", ""])[:4])) for l in r.stdout.splitlines() if l.strip()]
            cache["at"] = time.time()
        return cache["items"]

def rank(it, terms):
    """Every term must appear in name, user or folder; names that start with the first term come first."""
    hay = f"{it['name']}\n{it['user']}\n{it['folder']}".lower()
    if not all(t in hay for t in terms):
        return None
    name = it["name"].lower()
    return (0 if name.startswith(terms[0]) else 1 if terms[0] in name else 2, len(name))

def search(q):
    terms = q.lower().split()
    hits = sorted((r, it) for it in items() if (r := rank(it, terms)) is not None)
    return [{"name": it["name"] or "(no name)", "path": f"bitwarden:{it['id']}", "where": " · ".join(x for x in [it["user"], it["folder"]] if x)} for _, it in hits[:60]]

def secret(item_id, how):
    if how == "tab":
        r = rbw("code", item_id)
        if r.returncode != 0:
            raise Exception("This item has no TOTP code" if "totp" in r.stderr.lower() else (r.stderr.strip() or "rbw code failed"))
        return r.stdout.strip(), "TOTP code"
    r = rbw("get", "--raw", item_id)
    if r.returncode != 0:
        fail(r, "get")
    data = json.loads(r.stdout).get("data") or {}
    value = data.get("username" if how == "reveal" else "password")
    if not value:
        raise Exception(f"This item has no {'username' if how == 'reveal' else 'password'}")
    return value, "username" if how == "reveal" else "password"

def copy(value, sensitive):
    if not shutil.which("wl-copy"):
        raise Exception("Copying needs wl-clipboard (wl-copy)")
    subprocess.run(["wl-copy", *(["--sensitive"] if sensitive else [])], input=value, text=True, check=True)
    if sensitive:
        def clear():
            time.sleep(CLEAR_AFTER)
            now = subprocess.run(["wl-paste", "--no-newline"], capture_output=True, text=True).stdout
            if now == value:  # only if it's still ours
                subprocess.run(["wl-copy", "--clear"])
        threading.Thread(target=clear, daemon=True).start()

def handle(method, p):
    if method == "init":
        s = p.get("settings", {})
        try:
            configure(s.get("url", "").strip().rstrip("/"), s.get("email", "").strip())
        except Exception as e:
            setup["error"] = str(e)  # shown when you search, not as a startup error
        return {}
    if method == "search":
        if setup.get("error"):
            raise Exception(setup["error"])
        return {"entries": search(p["q"])}
    if method == "pick":
        value, what = secret(p["path"].split(":", 1)[1], p.get("how", "open"))
        copy(value, what != "username")
        return {"message": f"Copied the {what}" + (f" (cleared in {CLEAR_AFTER} s)" if what != "username" else "")}
    raise Exception(f"unknown request {method}")

def run(r):
    try:
        say({"id": r["id"], "result": handle(r["method"], r.get("params") or {})})
    except Exception as e:
        say({"id": r["id"], "error": str(e)})

if __name__ == "__main__":
    for line in sys.stdin:
        threading.Thread(target=run, args=(json.loads(line),), daemon=True).start()
