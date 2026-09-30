#!/usr/bin/env python3
"""python3 plugins/nextcloud/test_nextcloud.py — status mapping and nextcloud.cfg parsing."""
import os, sys, tempfile
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
home = tempfile.mkdtemp()
os.environ["HOME"] = home
os.makedirs(f"{home}/.config/Nextcloud")
open(f"{home}/.config/Nextcloud/nextcloud.cfg", "w").write(
    "[Accounts]\n0\\url=https://cloud.example\n0\\dav_user=me\n0\\Folders\\1\\localPath=/home/me/Nextcloud/\n"
    "0\\Folders\\1\\targetPath=/\n0\\FoldersWithPlaceholders\\2\\localPath=/home/me/Work/\n0\\FoldersWithPlaceholders\\2\\targetPath=/Work\n")
import nextcloud as nc

assert nc.badge("OK") == {"text": "✓", "title": "Synced", "state": "ok"}
assert nc.badge("OK+SWM")["state"] == "shared"
assert nc.badge("SYNC+SWM") == {"text": "↻", "title": "Syncing · shared", "state": "sync"}
assert nc.badge("NOP") is None and nc.badge(None) is None
roots = sorted(a["local"] for a in nc.accounts())
assert roots == ["/home/me/Nextcloud", "/home/me/Work"], roots
a, rel = nc.remote("/home/me/Work/q3/plan.odt")
assert (a["url"], a["user"], rel) == ("https://cloud.example", "me", "/Work/q3/plan.odt"), (a, rel)
assert nc.web_url("/home/me/Nextcloud/a b.txt") == "https://cloud.example/apps/files/?dir=%2F&scrollto=a+b.txt"
try:
    nc.remote("/home/me/Desktop/x")
    raise AssertionError("outside the synced folders")
except Exception as e:
    assert "Not in a Nextcloud folder" in str(e)
print("ok")
