#!/usr/bin/env python3
"""python3 plugins/bitwarden/test_bitwarden.py — against stub rbw / wl-copy / wl-paste (no real vault or clipboard)."""
import json, os, stat, sys, tempfile, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
bin_ = tempfile.mkdtemp()
state = os.path.join(bin_, "state")
def stub(name, body):
    p = os.path.join(bin_, name)
    open(p, "w").write("#!/usr/bin/env python3\nimport json, os, sys\nS = %r\n" % state + body)
    os.chmod(p, 0o755)
stub("rbw", r'''
a = sys.argv[1:]
log = open(S + ".log", "a"); log.write(" ".join(a) + "\n")
unlocked = os.path.exists(S + ".unlocked")
if a[0] == "unlocked": sys.exit(0 if unlocked else 1)
if a[0] == "unlock": open(S + ".unlocked", "w").close(); sys.exit(0)
if a[0] == "config" and a[1] == "show": print(json.dumps({"email": "old@x", "base_url": None})); sys.exit(0)
if a[0] in ("config", "stop-agent"): sys.exit(0)
if not unlocked: print("vault locked", file=sys.stderr); sys.exit(1)
if a[0] == "list": print("id1\tGitHub\tme@work\tWork\nid2\tgitlab.example\tadmin\t\nid3\tBank\tmike\tHome"); sys.exit(0)
if a[0] == "get": print(json.dumps({"id": a[-1], "data": {"username": "me@work", "password": "hunter2"}})); sys.exit(0)
if a[0] == "code": (print("123456"), sys.exit(0)) if a[-1] == "id1" else (print("rbw code: not a totp item", file=sys.stderr), sys.exit(1))
''')
stub("wl-copy", r'''
if "--clear" in sys.argv: open(S + ".clip", "w").close(); sys.exit(0)
open(S + ".clip", "w").write(sys.stdin.read()); open(S + ".flags", "w").write(" ".join(sys.argv[1:]))
''')
stub("wl-paste", r'''print(open(S + ".clip").read(), end="")''')
os.environ["PATH"] = bin_ + os.pathsep + os.environ["PATH"]
import bitwarden as bw
bw.CLEAR_AFTER = 0.3
clip = lambda: open(state + ".clip").read()

bw.handle("init", {"settings": {"url": "https://vault.example.com/", "email": "me@x"}})
log = open(state + ".log").read()
assert "config set email me@x" in log and "config set base_url https://vault.example.com" in log and "stop-agent" in log, log
hits = bw.handle("search", {"q": "git"})["entries"]
assert [h["name"] for h in hits] == ["GitHub", "gitlab.example"], hits
assert hits[0] == {"name": "GitHub", "path": "bitwarden:id1", "where": "me@work · Work"}
assert "unlock" in open(state + ".log").read(), "locked vault gets unlocked first"
assert [h["name"] for h in bw.handle("search", {"q": "work git"})["entries"]] == ["GitHub"], "every term must match"
assert "hunter2" not in json.dumps(hits), "search results carry no secrets"
r = bw.handle("pick", {"path": "bitwarden:id1", "how": "open"})
assert clip() == "hunter2" and "--sensitive" in open(state + ".flags").read(), r
time.sleep(0.6)
assert clip() == "", "password cleared after the timeout"
bw.handle("pick", {"path": "bitwarden:id1", "how": "reveal"})
assert clip() == "me@work" and "--sensitive" not in open(state + ".flags").read()
time.sleep(0.6)
assert clip() == "me@work", "usernames stay"
bw.handle("pick", {"path": "bitwarden:id1", "how": "tab"})
assert clip() == "123456"
bw.handle("pick", {"path": "bitwarden:id2", "how": "open"})
time.sleep(0.1)
open(state + ".clip", "w").write("something else")  # copied something else meanwhile
time.sleep(0.6)
assert clip() == "something else", "doesn't clear what isn't ours"
try:
    bw.handle("pick", {"path": "bitwarden:id3", "how": "tab"}); raise AssertionError
except Exception as e:
    assert "no TOTP" in str(e), e
print("ok")
