#!/usr/bin/env python3
"""Whale Cabinet plugin: Nextcloud sync badges and sharing through the desktop client's local socket
($XDG_RUNTIME_DIR/Nextcloud/socket, the one Dolphin/Nautilus overlays use). Synced folders come from
~/.config/Nextcloud/nextcloud.cfg. "Copy Public Link" talks to the server's sharing API with an app password
(this client version has no socket command for it); "Open in Browser" opens the web UI's URL for it. Standard library only."""
import base64, json, os, posixpath, re, socket, subprocess, sys, threading, urllib.error, urllib.parse, urllib.request

out = threading.Lock()
def say(msg):
    with out:
        print(json.dumps(msg), flush=True)

settings = {}

# ---------- accounts and synced folders (nextcloud.cfg, Qt INI) ----------
def accounts():
    """[{url, user, local, remote}] per synced folder."""
    try:
        text = open(os.path.expanduser("~/.config/Nextcloud/nextcloud.cfg"), encoding="utf-8").read()
    except OSError:
        return []
    acc, folders = {}, {}
    for line in text.splitlines():
        m = re.match(r"^(\d+)\\(url|dav_user)=(.*)$", line)
        if m:
            acc.setdefault(m[1], {})[m[2]] = m[3].strip()
        m = re.match(r"^(\d+)\\Folders(?:WithPlaceholders)?\\(\d+)\\(localPath|targetPath)=(.*)$", line)
        if m:
            folders.setdefault((m[1], m[2]), {"acc": m[1]})[m[3]] = m[4].strip()
    res = []
    for f in folders.values():
        a = acc.get(f["acc"], {})
        if f.get("localPath"):
            res.append({"url": a.get("url", ""), "user": a.get("dav_user", ""), "local": f["localPath"].rstrip("/") or "/", "remote": f.get("targetPath", "/") or "/"})
    return res

def account_for(path):
    best = None
    for a in accounts():
        if (path == a["local"] or path.startswith(a["local"] + "/")) and (not best or len(a["local"]) > len(best["local"])):
            best = a
    if not best:
        raise Exception("Not in a Nextcloud folder")
    return best

# ---------- the desktop client's socket ----------
class Client:
    def __init__(self):
        self.sock = None
        self.lock = threading.Lock()   # connect/send
        self.cond = threading.Condition()
        self.status = {}               # path → last status line part ("OK", "SYNC+SWM"…)
        self.fresh = set()             # answered since we asked
        self.asking = {}               # path → how many requests wait for it
        self.replies = []              # other answers ("SHARE:NOTCONNECTED:/path")

    def send(self, lines):
        with self.lock:
            if not self.sock:
                try:
                    s = socket.socket(socket.AF_UNIX)
                    s.connect(os.path.join(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"), "Nextcloud/socket"))
                except OSError:
                    raise Exception("The Nextcloud desktop client isn't running")
                self.sock = s
                threading.Thread(target=self.read, args=(s,), daemon=True).start()
            try:
                self.sock.sendall("".join(l + "\n" for l in lines).encode())
            except OSError:
                self.sock = None
                raise Exception("Lost the connection to the Nextcloud client")

    def read(self, s):
        for raw in s.makefile("rb"):
            line = raw.decode("utf-8", "replace").rstrip("\n")
            if line.startswith("STATUS:"):
                _, st, path = line.split(":", 2)
                with self.cond:
                    old = self.status.get(path)
                    self.status[path] = st
                    self.fresh.add(path)
                    self.cond.notify_all()
                    pushed = not self.asking.get(path)
                # the client pushes status changes while it syncs
                if pushed and old != st:
                    say({"event": "badges", "paths": [path]})
            elif line.startswith("UPDATE_VIEW:"):
                say({"event": "badges", "paths": [line.split(":", 1)[1].rstrip("/")]})
            else:
                with self.cond:
                    self.replies = self.replies[-50:] + [line]
                    self.cond.notify_all()
        with self.lock:
            if self.sock is s:
                self.sock = None
        say({"event": "badges"})  # client gone (or restarted): ask again

    def statuses(self, paths, timeout=3.0):
        with self.cond:
            for p in paths:
                self.fresh.discard(p)
                self.asking[p] = self.asking.get(p, 0) + 1
        try:
            self.send([f"RETRIEVE_FILE_STATUS:{p}" for p in paths])
            with self.cond:
                self.cond.wait_for(lambda: all(p in self.fresh for p in paths), timeout)
                return {p: self.status.get(p) for p in paths}
        finally:
            with self.cond:
                for p in paths:
                    self.asking[p] -= 1

    def ask(self, cmd, path, timeout=3.0):
        """Send CMD:path and wait for the client's CMD:<answer>:path (None if it says nothing)."""
        want = lambda l: l.startswith(cmd + ":") and l.endswith(":" + path)
        with self.cond:
            self.replies = [l for l in self.replies if not want(l)]
        self.send([f"{cmd}:{path}"])
        with self.cond:
            self.cond.wait_for(lambda: any(want(l) for l in self.replies), timeout)
            hit = next((l for l in self.replies if want(l)), None)
        return hit and hit[len(cmd) + 1:-len(path) - 1]

client = Client()

BADGES = {
    "OK": ("✓", "Synced", "ok"), "SYNC": ("↻", "Syncing", "sync"), "NEW": ("↻", "Waiting to sync", "sync"),
    "ERROR": ("!", "Sync error (see the Nextcloud client)", "error"), "WARNING": ("!", "Sync problem (see the Nextcloud client)", "warn"),
    "IGNORE": ("⊘", "Not synced (excluded)", "warn"),
}
def badge(st):
    if not st:
        return None
    base, _, flags = st.partition("+")
    b = BADGES.get(base)
    if not b:
        return None  # NOP, NONE
    text, title, state = b
    if "SWM" in flags:  # shared with me / by me
        title += " · shared"
        if state == "ok":
            state = "shared"
    return {"text": text, "title": title, "state": state}

# ---------- public link (OCS sharing API) ----------
def ocs(method, url, user, body=None):
    pw = settings.get("apppassword")
    if not pw:
        raise Exception("Copy Public Link needs an app password: Settings → Plugins → Nextcloud")
    req = urllib.request.Request(url, method=method, data=urllib.parse.urlencode(body).encode() if body else None, headers={
        "OCS-APIRequest": "true", "Accept": "application/json",
        "Authorization": "Basic " + base64.b64encode(f"{user}:{pw}".encode()).decode()})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.load(r)["ocs"]["data"]
    except urllib.error.HTTPError as e:
        if e.code == 401:
            raise Exception("Nextcloud refused the app password")
        try:
            msg = json.load(e)["ocs"]["meta"]["message"]
        except Exception:
            msg = f"HTTP {e.code}"
        raise Exception(f"Nextcloud: {msg}")
    except urllib.error.URLError as e:
        raise Exception(f"Can't reach Nextcloud: {e.reason}")

def remote(path):
    a = account_for(path)
    return a, posixpath.normpath(posixpath.join("/", a["remote"], os.path.relpath(path, a["local"])))

def web_url(path):
    """The item in Nextcloud's web UI (works whether or not the desktop client is connected)."""
    a, rel = remote(path)
    q = {"dir": rel} if os.path.isdir(path) else {"dir": posixpath.dirname(rel), "scrollto": posixpath.basename(rel)}
    return a["url"].rstrip("/") + "/apps/files/?" + urllib.parse.urlencode(q)

def public_link(path):
    a, rel = remote(path)
    api = a["url"].rstrip("/") + "/ocs/v2.php/apps/files_sharing/api/v1/shares"
    for s in ocs("GET", api + "?" + urllib.parse.urlencode({"path": rel, "reshares": "false"}), a["user"]) or []:
        if s.get("share_type") == 3 and s.get("url"):
            return s["url"], False
    return ocs("POST", api, a["user"], {"path": rel, "shareType": 3, "permissions": 1})["url"], True

# ---------- requests ----------
def handle(method, p):
    global settings
    if method == "init":
        settings = p.get("settings", {})
        return {"roots": sorted({a["local"] for a in accounts()})}
    if method == "badges":
        try:
            st = client.statuses(p["paths"])
        except Exception:
            return {}  # no client: no badges
        return {path: badge(s) for path, s in st.items()}
    if method == "action":
        path = p["paths"][0]
        if p["id"] == "share":
            answer = client.ask("SHARE", path)
            if answer == "NOTCONNECTED":
                raise Exception("The Nextcloud client isn't connected to your server (check its window); Copy Public Link still works")
            if answer == "NOP":
                raise Exception("The Nextcloud client doesn't sync this item")
            return {"message": "Opening Nextcloud's share dialog…"}
        if p["id"] == "browser":
            subprocess.Popen(["xdg-open", web_url(path)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
            return {"message": "Opening it in your browser…"}
        if p["id"] == "publiclink":
            url, new = public_link(path)
            return {"copy": url, "message": f"{'Created and copied' if new else 'Copied'} the public link: {url}"}
    raise Exception(f"unknown request {method}")

def run(r):
    try:
        say({"id": r["id"], "result": handle(r["method"], r.get("params") or {})})
    except Exception as e:
        say({"id": r["id"], "error": str(e)})

if __name__ == "__main__":
    for line in sys.stdin:
        threading.Thread(target=run, args=(json.loads(line),), daemon=True).start()
