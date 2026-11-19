"""End-to-end test of the real background app (tray + poller + notifications) with an isolated data dir.
Reads the real profiles' tokens (read-only) and calls the real usage API a few times; never touches the real config,
settings.json or the installed app's data.  Takes ~4-5 minutes.   Run:  .venv\\Scripts\\python tests\\e2e_app.py"""
import json, os, subprocess, sys, tempfile, time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SANDBOX = Path(tempfile.mkdtemp(prefix="cu_e2e_")); DATA = SANDBOX / "claude-usage"; DATA.mkdir()
SHOTS = Path(os.environ.get("E2E_SHOTS", SANDBOX))
sys.path.insert(0, str(ROOT))
real_profiles = __import__("core").detect_profiles()            # computed before LOCALAPPDATA is redirected
CFG, CACHE, EVENTS, ALERTS = DATA / "config.json", DATA / "usage_cache.json", DATA / "events.log", DATA / "alerts.json"
FLAG, TESTFLAG = DATA / "refresh.flag", DATA / "notify_test.flag"
results = []
def check(name, ok, detail=""):
    ok = bool(ok); results.append(ok); print(f"  {'ok  ' if ok else 'FAIL'} {name}" + (f"  [{detail}]" if detail else ""), flush=True)

def write_cfg(**kw):
    cfg = {"interval_sec": 90, "warn_pct": 1, "block_pct": 99, "block_enabled": False, "widget_enabled": False,
           "autostart": False, "profiles": real_profiles, **kw}
    CFG.write_text(json.dumps(cfg), "utf-8")

def start():
    env = {**os.environ, "LOCALAPPDATA": str(SANDBOX)}
    return subprocess.Popen([str(ROOT / ".venv/Scripts/python.exe"), str(ROOT / "main.py"), "--hidden"], env=env,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=0x08000000)

def stop(p):
    p.kill(); p.wait(); (DATA / "daemon.pid").unlink(missing_ok=True); time.sleep(1.5)

def cache():
    try: return json.loads(CACHE.read_text("utf-8"))
    except Exception: return None

def toasts(): return [l for l in EVENTS.read_text("utf-8").splitlines() if " toast |" in l] if EVENTS.exists() else []
def reset_state(first_run=False):
    for f in (CACHE, EVENTS, ALERTS, FLAG, TESTFLAG): f.unlink(missing_ok=True)
    if not first_run: ALERTS.write_text("{}")          # "not a first run": every high account counts as a new crossing

def shot(name):
    try:
        from PIL import ImageGrab
        im = ImageGrab.grab(all_screens=False); w, h = im.size
        im.crop((w - 520, h - 330, w, h)).save(SHOTS / f"{name}.png")
    except Exception as e: print("   (screenshot failed:", e, ")")

print(f"sandbox: {SANDBOX}\n== A: notifications ON (warn at 1% so every account triggers), fresh state")
write_cfg(notify_enabled=True, notify_warn=True, notify_block=True); reset_state()
p = start(); t0 = time.time(); seen_busy, saw_shot, max_gap = [], False, 0
while time.time() - t0 < 38:
    c = cache()
    if c and c.get("busy"): seen_busy.append((round(time.time() - t0, 1), tuple(c["busy"])))
    n = len(toasts())
    if n and not saw_shot: time.sleep(1.2); shot("toast_visible"); saw_shot = True
    time.sleep(0.04)
c = cache() or {}
got_data = [n for n, v in (c.get("profiles") or {}).items() if "five" in v]
check("data fetched for every profile", len(got_data) == len(real_profiles), f"{len(got_data)}/{len(real_profiles)} have data; errors: " + str({n: v.get('err') for n, v in (c.get('profiles') or {}).items() if v.get('err')}))
check("'waiting' (busy) state was visible while fetching", len(seen_busy) >= len(real_profiles), f"{len(seen_busy)} samples, e.g. {seen_busy[:3]}")
check("busy state cleared afterwards", c.get("busy") == [], str(c.get("busy")))
tl = toasts(); check("toast(s) fired once the threshold was crossed (batched)", 1 <= len(tl) <= 3, f"{len(tl)} toast(s)")
for l in tl: print("      ", l)
first_round = len(tl)
print("   waiting for the 2nd poll round (interval 90s) to prove repeats do NOT re-notify ...", flush=True)
f0 = {n: v.get("fetched_at") for n, v in (cache() or {}).get("profiles", {}).items()}
t1 = time.time()
while time.time() - t1 < 125: time.sleep(2)
f1 = {n: v.get("fetched_at") for n, v in (cache() or {}).get("profiles", {}).items()}
check("2nd round really polled again", sum(1 for n in f1 if f1[n] and f0.get(n) and f1[n] > f0[n] + 60) >= 1, f"{sum(1 for n in f1 if f1[n] and f0.get(n) and f1[n] > f0[n] + 60)} accounts refetched")
check("no extra toast after repeated polls", len(toasts()) == first_round, f"{first_round} -> {len(toasts())}")
check("alert state persisted to disk", ALERTS.exists() and len(json.loads(ALERTS.read_text())) >= 1)
stop(p)

print("== A2: FIRST RUN (no alerts.json) with accounts already high -> silent baseline")
stop(p); reset_state(first_run=True); write_cfg(notify_enabled=True); p = start(); time.sleep(32)
check("first run does not announce what is already high", len(toasts()) == 0, f"{len(toasts())} toasts")
check("...but remembers it (state file written)", ALERTS.exists() and len(json.loads(ALERTS.read_text())) >= 1)
stop(p); reset_state(); write_cfg(notify_enabled=True, warn_pct=1, block_pct=99)
p = start(); time.sleep(36); stop(p)          # build real alert state, then restart in B
print("== B: restart with the same state -> must NOT re-announce")
before = len(toasts()); p = start(); time.sleep(25)
check("restart is silent", len(toasts()) == before, f"{before} -> {len(toasts())}")
stop(p)

print("== C: notifications OFF, fresh state")
reset_state(); write_cfg(notify_enabled=False); p = start(); time.sleep(32)
check("no toast while OFF", len(toasts()) == 0, f"{len(toasts())} toasts")
check("state still recorded while OFF", ALERTS.exists() and len(json.loads(ALERTS.read_text())) >= 1)
TESTFLAG.write_text("1"); time.sleep(3)
tt = toasts(); check("'send test notification' works while OFF and says it is OFF", len(tt) == 1 and "TẮT" in tt[0], tt[-1] if tt else "none")
write_cfg(notify_enabled=True); time.sleep(14)
check("switching ON does not replay old conditions", len(toasts()) == 1, f"{len(toasts())} toasts")
TESTFLAG.write_text("1"); time.sleep(3)
tt = toasts(); check("test notification says ON after switching on", len(tt) == 2 and "BẬT" in tt[-1], tt[-1] if tt else "none")
shot("toast_test")

print("== D: manual refresh feedback")
time.sleep(10); c0 = cache(); FLAG.write_text("1"); seen = []; t2 = time.time()
while time.time() - t2 < 12:
    c = cache()
    if c and c.get("busy"): seen.append(tuple(c["busy"]))
    time.sleep(0.04)
r = (cache() or {}).get("refresh") or {}
check("refresh reports what it did", bool(r) and (r.get("started") or r.get("skipped")), json.dumps(r, ensure_ascii=False)[:160])
check("refresh showed waiting state for started accounts", (not r.get("started")) or len(seen) >= 1, f"{len(seen)} busy samples")
FLAG.write_text("1"); time.sleep(2.5); r2 = (cache() or {}).get("refresh") or {}
check("immediate second refresh is skipped (rate limit)", len(r2.get("started", [])) == 0 and len(r2.get("skipped", [])) >= 1, json.dumps(r2, ensure_ascii=False)[:160])
stop(p)

print(f"\n{sum(results)}/{len(results)} checks passed   (screenshots, if any, in {SHOTS})")
sys.exit(0 if all(results) else 1)
