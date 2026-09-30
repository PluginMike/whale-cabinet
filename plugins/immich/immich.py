#!/usr/bin/env python3
"""Whale Cabinet plugin: an Immich server through its API (x-api-key): browse, smart search, upload (paste /
drop into a location; into an album adds it there, into Favorites marks it), delete (to Immich's trash). Locations:
  immich:/timeline            months (from /timeline/buckets, which carries the counts)
  immich:/timeline/YYYY-MM-DD  that month's photos (/search/metadata, taken in it)
  immich:/albums, immich:/albums/<id>, immich:/favorites, immich:/search/<query> (smart search)
Items are immich:/asset/<id>/<file name>; thumbnails and originals are downloaded into the cache folder.
Standard library only."""
import json, mimetypes, os, shutil, subprocess, sys, tempfile, threading, urllib.error, urllib.parse, urllib.request, uuid
from datetime import datetime, timezone

out = threading.Lock()
def say(msg):
    with out:
        print(json.dumps(msg), flush=True)

cfg = {"url": "", "apikey": "", "cache": ""}
PAGE = 1000

# ---------- API ----------
def api(method, path, body=None, raw=False, data=None, ctype="application/json", timeout=60):
    if not cfg["url"] or not cfg["apikey"]:
        raise Exception("Set the Immich server URL and API key in Settings → Plugins → Immich")
    headers = {"x-api-key": cfg["apikey"], "Accept": "application/json" if not raw else "*/*", "Content-Type": ctype}
    if data is not None:
        headers["Content-Length"] = str(os.fstat(data.fileno()).st_size)
    req = urllib.request.Request(cfg["url"] + "/api" + path, method=method, data=data if data is not None else json.dumps(body).encode() if body is not None else None, headers=headers)
    try:
        r = urllib.request.urlopen(req, timeout=timeout)
    except urllib.error.HTTPError as e:
        if e.code == 401:
            raise Exception("Immich refused the API key (Settings → Plugins → Immich)")
        if e.code == 403:
            raise Exception(f"The Immich API key isn't allowed to do this ({path}); give it the read permissions listed in its settings")
        try:
            msg = json.load(e).get("message")
        except Exception:
            msg = None
        raise Exception(f"Immich: {msg or f'HTTP {e.code}'}")
    except urllib.error.URLError as e:
        raise Exception(f"Can't reach Immich at {cfg['url']}: {e.reason}")
    if raw:
        return r
    with r:
        text = r.read()
        return json.loads(text) if text else None

def search(kind, body, limit=None):
    """All assets of a /search/<kind> query (paged), newest first."""
    items, page = [], 1
    while page:
        a = api("POST", f"/search/{kind}", {**body, "page": page, "size": min(PAGE, limit or PAGE), "withExif": True})["assets"]
        items += a["items"]
        page = int(a["nextPage"]) if a.get("nextPage") and (limit is None or len(items) < limit) else None
    return items[:limit] if limit else items

# ---------- entries ----------
def ms(iso):
    try:
        return int(datetime.fromisoformat(iso.replace("Z", "+00:00")).timestamp() * 1000)
    except (AttributeError, ValueError):
        return 0

def asset(a):
    x = a.get("exifInfo") or {}
    t = ms(a.get("fileCreatedAt"))
    info = []
    if x.get("dateTimeOriginal"):
        info.append(["Taken", datetime.fromisoformat(x["dateTimeOriginal"].replace("Z", "+00:00")).astimezone().strftime("%d %b %Y, %H:%M")])
    cam = " ".join(v for v in [x.get("make"), x.get("model")] if v)
    if cam:
        info.append(["Camera", cam])
    if x.get("exifImageWidth"):
        info.append(["Pixels", f"{x['exifImageWidth']} × {x['exifImageHeight']}"])
    lens = [f"f/{x['fNumber']}" if x.get("fNumber") else "", f"{x['exposureTime']} s" if x.get("exposureTime") else "", f"ISO {x['iso']}" if x.get("iso") else "", f"{x['focalLength']:g} mm" if x.get("focalLength") else ""]
    if any(lens):
        info.append(["Exposure", "  ".join(v for v in lens if v)])
    place = ", ".join(v for v in [x.get("city"), x.get("state"), x.get("country")] if v)
    if place:
        info.append(["Place", place])
    if a.get("type") == "VIDEO" and a.get("duration"):
        info.append(["Length", a["duration"].split(".")[0].removeprefix("00:")])
    if x.get("description"):
        info.append(["Description", x["description"]])
    if a.get("isFavorite"):
        info.append(["Favorite", "★"])
    return {"name": a["originalFileName"], "path": f"immich:/asset/{a['id']}/{a['originalFileName']}", "size": x.get("fileSizeInByte") or 0,
            "mtime": t, "score": t, "thumb": True, "where": place, "info": info}

def folder(name, loc, count, t=0):
    return {"name": name, "path": loc, "dir": True, "count": f"{count:,} item{'' if count == 1 else 's'}", "mtime": t, "score": t}

def month_after(day):
    y, m = int(day[:4]), int(day[5:7])
    return f"{y + (m == 12):04d}-{m % 12 + 1:02d}-01"

def listing(loc):
    parts = loc.split(":", 1)[1].strip("/").split("/", 1)
    top, rest = parts[0], (parts[1] if len(parts) > 1 else "")
    if top == "timeline" and not rest:
        months = api("GET", "/timeline/buckets?visibility=timeline")
        return "Timeline", [folder(datetime.fromisoformat(b["timeBucket"][:10]).strftime("%B %Y"), f"immich:/timeline/{b['timeBucket'][:10]}", b["count"], ms(b["timeBucket"][:10] + "T00:00:00Z")) for b in months]
    if top == "timeline":
        # NOTE: months are cut in UTC; photos taken just after local midnight on the 1st can land in the month before
        day = rest[:10]
        title = datetime.fromisoformat(day).strftime("%B %Y")
        return title, [asset(a) for a in search("metadata", {"takenAfter": day + "T00:00:00.000Z", "takenBefore": month_after(day) + "T00:00:00.000Z", "order": "desc", "visibility": "timeline"})]
    if top == "albums" and not rest:
        return "Albums", [folder(a["albumName"] or "Untitled", f"immich:/albums/{a['id']}", a["assetCount"], ms(a.get("endDate") or a.get("updatedAt"))) for a in api("GET", "/albums")]
    if top == "albums":
        name = api("GET", f"/albums/{urllib.parse.quote(rest)}?withoutAssets=true")["albumName"]
        return name, [asset(a) for a in search("metadata", {"albumIds": [rest], "order": "desc"})]
    if top == "favorites":
        return "Favorites", [asset(a) for a in search("metadata", {"isFavorite": True, "order": "desc"})]
    if top == "search":
        q = urllib.parse.unquote(rest)
        return f"“{q}”", smart(q, 250)
    raise Exception(f"No such Immich location: {loc}")

def smart(q, n):
    # relevance order: the best match first
    hits = [asset(a) for a in search("smart", {"query": q}, limit=n)]
    for i, e in enumerate(hits):
        e["score"] = n - i
    return hits

# ---------- files ----------
def asset_id(path):
    if not path.startswith("immich:/asset/"):
        raise Exception(f"Not an Immich photo: {path}")
    return path[len("immich:/asset/"):].split("/", 1)[0]

def download(url_path, dst):
    if os.path.exists(dst) and os.path.getsize(dst) > 0:
        return dst
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    tmp = f"{dst}.part{threading.get_ident()}"
    try:
        with api("GET", url_path, raw=True) as r, open(tmp, "wb") as f:
            shutil.copyfileobj(r, f, 1 << 20)
        os.replace(tmp, dst)
    finally:
        if os.path.exists(tmp):
            os.remove(tmp)
    return dst

def thumb(path, size):
    size = "preview" if size == "preview" else "thumbnail"
    i = asset_id(path)
    return download(f"/assets/{i}/thumbnail?size={size}", os.path.join(cfg["cache"], "thumbs", f"{i}-{size}"))

def fetch(path):
    i = asset_id(path)
    name = os.path.basename(path.split("/", 3)[3]) or i
    return download(f"/assets/{i}/original", os.path.join(cfg["cache"], "originals", i, name))

def upload_one(path):
    """POST /assets (multipart, streamed from a temp file); returns (asset id, was it already there)."""
    st = os.stat(path)
    when = datetime.fromtimestamp(st.st_mtime, timezone.utc).isoformat()
    boundary = uuid.uuid4().hex
    fields = {"deviceAssetId": f"{os.path.basename(path)}-{st.st_size}-{int(st.st_mtime)}", "deviceId": "whale-cabinet",
              "fileCreatedAt": when, "fileModifiedAt": when, "filename": os.path.basename(path)}
    with tempfile.TemporaryFile(dir=cfg["cache"]) as body:
        for k, v in fields.items():
            body.write(f'--{boundary}\r\nContent-Disposition: form-data; name="{k}"\r\n\r\n{v}\r\n'.encode())
        name = os.path.basename(path).replace('"', "_")
        body.write(f'--{boundary}\r\nContent-Disposition: form-data; name="assetData"; filename="{name}"\r\n'
                   f'Content-Type: {mimetypes.guess_type(path)[0] or "application/octet-stream"}\r\n\r\n'.encode())
        with open(path, "rb") as f:
            shutil.copyfileobj(f, body, 1 << 20)
        body.write(f"\r\n--{boundary}--\r\n".encode())
        body.seek(0)
        r = api("POST", "/assets", data=body, ctype=f"multipart/form-data; boundary={boundary}", timeout=3600)
    return r["id"], r.get("status") == "duplicate"

def upload(loc, files):
    files = [f for f in files if os.path.isfile(f)]
    if not files:
        raise Exception("Only files can be uploaded (not folders)")
    ids, dupes = [], 0
    for f in files:
        i, dup = upload_one(f)
        ids.append(i)
        dupes += dup
    where = "Immich"
    if loc.startswith("immich:/albums/"):
        album = loc.rsplit("/", 1)[1]
        api("PUT", f"/albums/{urllib.parse.quote(album)}/assets", {"ids": ids})
        where = "the album"
    elif loc.startswith("immich:/favorites"):
        api("PUT", "/assets", {"ids": ids, "isFavorite": True})
        where = "Favorites"
    new = len(ids) - dupes
    say({"event": "reload"})
    return {"message": f"Uploaded {new} item{'' if new == 1 else 's'} to {where}" + (f" ({dupes} already in Immich)" if dupes else "")}

# ---------- requests ----------
def handle(method, p):
    if method == "init":
        s = p.get("settings", {})
        cfg.update(url=s.get("url", "").strip().rstrip("/").removesuffix("/api"), apikey=s.get("apikey", "").strip(), cache=p.get("cache", "/tmp"))
        if cfg["url"] and "://" not in cfg["url"]:
            cfg["url"] = "https://" + cfg["url"]
        return {"sidebar": [{"title": "Timeline", "loc": "immich:/timeline"}, {"title": "Albums", "loc": "immich:/albums"}, {"title": "Favorites", "loc": "immich:/favorites"}]}
    if method == "list":
        title, entries = listing(p["loc"])
        return {"title": title, "entries": entries}
    if method == "thumb":
        return {"file": thumb(p["path"], p.get("size"))}
    if method == "fetch":
        return {"file": fetch(p["path"])}
    if method == "search":
        q = p["q"].strip()
        return {"entries": smart(q, 40), "loc": "immich:/search/" + urllib.parse.quote(q, safe="")}
    if method == "delete":
        ids = [asset_id(x) for x in p["paths"]]
        api("DELETE", "/assets", {"ids": ids, "force": False})
        return {"message": f"Moved {len(ids)} item{'' if len(ids) == 1 else 's'} to Immich's trash"}
    if method == "upload":
        return upload(p.get("loc", ""), p["files"])
    if method == "action" and p["id"] == "upload":
        return upload("", p["paths"])
    if method == "action" and p["id"] == "web":
        for path in p["paths"]:
            i = asset_id(path) if path.startswith("immich:/asset/") else None
            url = f"{cfg['url']}/photos/{i}" if i else f"{cfg['url']}/albums/{path.rsplit('/', 1)[1]}" if path.startswith("immich:/albums/") else cfg["url"]
            subprocess.Popen(["xdg-open", url], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        return {"message": "Opening Immich in your browser…"}
    raise Exception(f"unknown request {method}")

def run(r):
    try:
        say({"id": r["id"], "result": handle(r["method"], r.get("params") or {})})
    except Exception as e:
        say({"id": r["id"], "error": str(e)})

if __name__ == "__main__":
    for line in sys.stdin:
        threading.Thread(target=run, args=(json.loads(line),), daemon=True).start()
