#!/usr/bin/env python3
"""The console drill, end to end, over HTTP. Standard library only.

Drives a running console (default http://127.0.0.1:3000) exactly as the
buttons do, and exits non-zero if any claim fails:

  * the primaries serve every cell and the flock is in the air;
  * kill -9 host-a: holes open, and the standby closes every one of them;
  * freeze the standby, start host-a: host-a takes back exactly its cells;
  * thaw the standby: it serves nothing and waits;
  * throughout: no stale epoch, no cell served twice, no bird lost, and
    population == released + births + found - lost.

    python3 tools/drill.py [base-url]
"""

import http.cookiejar
import json
import re
import sys
import time
import urllib.request

BASE = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:3000"
jar = http.cookiejar.CookieJar()
op = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))
token = re.search(r'name="csrf-token" content="([^"]+)"', op.open(BASE + "/").read().decode()).group(1)
failures = []


def post(path):
    req = urllib.request.Request(BASE + path, data=b"", method="POST", headers={"x-csrf-token": token})
    with op.open(req, timeout=60) as r:
        return json.load(r)


def frame():
    return json.load(op.open(BASE + "/_frame.json"))


def state():
    return json.load(op.open(BASE + "/_state.json"))


def check(cond, msg):
    print(("  ok   " if cond else "  FAIL ") + msg, flush=True)
    if not cond:
        failures.append(msg)


def invariants(f):
    c = f["counts"]
    if c["stale"] or c["double"]:
        check(False, f"stale {c['stale']}, double {c['double']}")
    if c["lost"]:
        check(False, f"{c['lost']} birds lost")
    want = c["released"] + c["births"] + c["found"] - c["lost"]
    if f["population"] != want:
        check(False, f"population {f['population']} != {want}")


def wait(label, pred, within):
    t = time.time()
    while time.time() - t < within:
        f = frame()
        invariants(f)
        if pred(f):
            print(f"{label} after {time.time() - t:.1f} s", flush=True)
            return f
        time.sleep(0.25)
    check(False, f"never: {label}")
    return frame()


def owners(f, i):
    return sum(1 for o in f["cells"] if o == i)


f = wait("the flock is up", lambda f: f["holes"] == 0 and f["population"] >= 60, 90)
a_cells = owners(f, 0)
check(a_cells > 0 and owners(f, 1) + a_cells == 384, f"host-a serves {a_cells} cells, host-b {owners(f, 1)}")
time.sleep(4)

post("/hosts/host-a/kill")
post("/hosts/host-c/start")
f = wait("holes open", lambda f: f["holes"] > 0, 20)
check(f["holes"] == a_cells, f"{f['holes']} holes: exactly host-a's cells")
f = wait("host-c closes every hole", lambda f: f["holes"] == 0, 150)
check(owners(f, 2) == a_cells, f"host-c serves {owners(f, 2)} cells")
c = f["counts"]
deepest = max([r["by_ms"] for r in f["rewinds"]] or [0])
print(f"takeover: {c['lost']} lost, {c['unhatched']} unhatched, {c['rewound']} came back older (up to {deepest} ms), "
      f"{c['ghosts']} ghosts, {f['held']} copies held", flush=True)
time.sleep(3)

post("/hosts/host-c/freeze")
time.sleep(8)
post("/hosts/host-a/start")
f = wait("host-a takes its cells back", lambda f: f["holes"] == 0 and owners(f, 0) == a_cells, 150)
post("/hosts/host-c/thaw")
time.sleep(5)
f = frame()
invariants(f)
check(owners(f, 2) == 0, "the thawed host-c serves no cell")
c_view = next(h for h in state()["hosts"] if h["name"] == "host-c")["view"]
check(not c_view["held"] and "0-8191" in c_view["waiting"], "the thawed host-c holds nothing and waits")

c = f["counts"]
print(f"final: population {f['population']}, born {c['births']}, lost {c['lost']}, unhatched {c['unhatched']}, "
      f"rewound {c['rewound']}, ghosts {c['ghosts']}, stale {c['stale']}, double {c['double']}", flush=True)
check(c["lost"] == 0, f"no bird lost ({c['lost']})")
if failures:
    print(f"\n{len(failures)} claim(s) failed")
    sys.exit(1)
print("\nthe drill passed")
