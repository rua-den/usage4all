"""E2E: dashboard with MANY profiles + resizing / maximizing / responsive layout / remembered size (needs a desktop).
Runs two dashboard launches against one sandbox (phase 1: interact, phase 2: check the size was remembered).
Run:  .venv\\Scripts\\python tests\\e2e_responsive.py"""
import json, os, subprocess, sys, tempfile, threading, time
from datetime import datetime, timezone, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
N = 14

if len(sys.argv) == 1:                                           # ---- parent: build the sandbox, run both phases ----
    sandbox = Path(tempfile.mkdtemp(prefix="cu_resp_")); (sandbox / "claude-usage").mkdir()
    code = 0
    for phase in ("1", "2"):
        r = subprocess.run([sys.executable, __file__, phase, str(sandbox)], timeout=240)
        code |= r.returncode
        time.sleep(2)
    sys.exit(code)

PHASE, SANDBOX = sys.argv[1], Path(sys.argv[2])
os.environ["LOCALAPPDATA"] = str(SANDBOX)
sys.path.insert(0, str(ROOT))
import core, dashboard, webview
iso = lambda s: (datetime.now(timezone.utc) + timedelta(seconds=s)).isoformat()
if PHASE == "1":
    profiles = []
    for i in range(N):
        d = SANDBOX / f"profile-{i + 1:02d}"; d.mkdir(); profiles.append({"name": f"claude-profile-{i + 1:02d}", "path": str(d)})
    data = {p["name"]: {"five": (i * 7) % 100, "week": (i * 11) % 100, "five_reset": iso(3600 * (i % 5 + 1)), "week_reset": iso(86400 * (i % 6 + 1)), "fetched_at": time.time(), "plan": "team"}
            for i, p in enumerate(profiles)}
    data[profiles[3]["name"]].update(err="HTTP 429", retry_at=time.time() + 120)
    cfg = dict(core.DEFAULTS, profiles=profiles); core.save_config(cfg); core.write_cache(cfg, data)
CFG = core.load_config()
results = []
def check(name, ok, detail=""):
    ok = bool(ok); results.append(ok); print(f"  {'ok  ' if ok else 'FAIL'} {name}" + (f"  [{detail}]" if detail else ""), flush=True)

api = dashboard.Api(); t0 = time.time()
scale = dashboard.user32.GetDpiForSystem() / 96
sw, sh = dashboard.user32.GetSystemMetrics(0), dashboard.user32.GetSystemMetrics(1)
dashboard.ctypes.windll.shcore.SetProcessDpiAwareness(2)
api._window = webview.create_window("claude-usage", html=dashboard.page(), js_api=api, frameless=True, easy_drag=False, min_size=(dashboard.MIN_W, dashboard.MIN_H),
                                    width=dashboard.initial_width(len(CFG["profiles"]), sw / scale), height=640, background_color="#0a0a0a")
api._window.events.closed += lambda: os._exit(0 if all(results) else 1)

def driver():
    w = api._window; js = lambda c: w.evaluate_js(c)
    for _ in range(200):
        try:
            if js("!!(window.pywebview && pywebview.api && document.querySelectorAll('.acc').length)"): break
        except Exception: pass
        time.sleep(0.1)
    for _ in range(100):
        if api.hwnd(): break
        time.sleep(0.1)
    api.restore_saved()                                          # what dashboard.run() does after start
    time.sleep(1.8)
    dpr = js("window.devicePixelRatio")
    def info(): return json.loads(js("""(()=>{const m=document.querySelector('main'),d=document.documentElement,a=[...document.querySelectorAll('.acc')];
      return JSON.stringify({iw:innerWidth,ih:innerHeight,hs:d.scrollWidth>innerWidth+1,ms:m.scrollHeight,mc:m.clientHeight,cols:new Set(a.map(x=>Math.round(x.getBoundingClientRect().left))).size,
        setCols:getComputedStyle(document.getElementById('gen')).gridTemplateColumns.split(' ').length,user:userSized,maxed:maxed,compact:document.body.classList.contains('compact'),n:a.length})})()"""))
    def drag(edge, dx, dy):                                      # real pointer events on the page's own edge handle
        js(f"""(()=>{{const h=document.querySelector('.rz.{edge}');const o={{bubbles:true,pointerId:1,screenX:1000,screenY:700}};
          h.dispatchEvent(new PointerEvent('pointerdown',o))}})()"""); time.sleep(0.5)
        js(f"window.dispatchEvent(new PointerEvent('pointermove',{{bubbles:true,pointerId:1,screenX:{1000 + dx / dpr},screenY:{700 + dy / dpr}}}))"); time.sleep(0.4)
        js("window.dispatchEvent(new PointerEvent('pointerup',{bubbles:true,pointerId:1}))"); time.sleep(0.9)

    if PHASE == "1":
        i = info()
        check(f"{N} profiles are all listed", i["n"] == N, str(i["n"]))
        check("default window is wide for many accounts and shows several columns", i["iw"] >= 1100 and i["cols"] >= 3, f"width={i['iw']} columns={i['cols']}")
        check("auto-fit: nothing to scroll, window fits the screen", i["ms"] <= i["mc"] + 1 and not i["hs"] and api.rect()[3] <= sh, f"{i} rect={api.rect()}")
        r0 = api.rect()
        drag("br", -(r0[2] - 760 * dpr), -(r0[3] - 520 * dpr))
        r1, i = api.rect(), info()
        check("dragging the bottom-right corner shrinks the window", abs(r1[2] - 760 * dpr) < 4 and abs(r1[3] - 520 * dpr) < 4 and r1[0] == r0[0] and r1[1] == r0[1], f"{r0} -> {r1}")
        time.sleep(1.6)
        check("a user-chosen size is kept (no auto-fit fighting it)", api.rect() == r1 and i["user"] is True, f"{api.rect()}")
        check("narrower window -> fewer columns, no horizontal scrolling", i["cols"] < 3 and not i["hs"], f"{i['cols']} columns, hs={i['hs']}")
        check(f"short window with {N} accounts scrolls inside (content taller than the window, page itself doesn't)", i["ms"] > i["mc"] + 20 and not i["hs"], f"main {i['ms']}/{i['mc']}")
        drag("l", 300 * dpr, 0); drag("t", 0, 120 * dpr)
        r2 = api.rect(); check("left and top edges resize too (opposite edge stays put)", r2[0] + r2[2] == r1[0] + r1[2] and r2[1] + r2[3] == r1[1] + r1[3] and r2[2] < r1[2] and r2[3] < r1[3], f"{r1} -> {r2}")
        drag("tl", 5000 * dpr, 5000 * dpr)
        r3, i = api.rect(), info()
        check("cannot shrink below the minimum size; layout collapses to ONE column and nothing overflows", r3[2] >= dashboard.MIN_W * dpr - 2 and r3[3] >= dashboard.MIN_H * dpr - 2 and i["cols"] == 1 and not i["hs"] and i["setCols"] <= 2, f"rect={r3} {i}")
        api.set_rect(150, 90, 900 * dpr, 640 * dpr); time.sleep(0.8)
        # maximize / restore
        before = api.rect(); js("toggleMax()"); time.sleep(1.2)
        wa = api.work_area(); mx = api.rect(); i = info()
        check("maximize fills the monitor's work area (taskbar stays visible)", all(abs(a - b) <= 2 for a, b in zip(mx, wa)) and i["maxed"] is True and js("document.getElementById('maxb').textContent") == "❐", f"{mx} vs work area {wa}")
        check("maximized window shows more columns", i["cols"] >= 3 and not i["hs"], f"{i['cols']} columns")
        js("toggleMax()"); time.sleep(1.0); rr = api.rect()
        check("restore returns to the previous size and position", rr == before and info()["maxed"] is False, f"{before} -> {rr}")
        js("document.querySelector('.drag').dispatchEvent(new MouseEvent('dblclick',{bubbles:true}))"); time.sleep(1.0)
        check("double-clicking the title bar maximizes", api.rect() == wa or all(abs(a - b) <= 2 for a, b in zip(api.rect(), wa)))
        js("document.querySelector('.drag').dispatchEvent(new MouseEvent('dblclick',{bubbles:true}))"); time.sleep(1.0)
        # remember the size
        api.set_rect(120, 80, 980 * dpr, 600 * dpr); time.sleep(1.6)
        saved = core.load_config().get("dash_rect")
        check("the chosen size/position is written to config", isinstance(saved, list) and abs(saved[2] - 980 * dpr) <= 3 and abs(saved[3] - 600 * dpr) <= 3, str(saved))
        print(f"\n{sum(results)}/{len(results)} checks passed (phase 1)", flush=True)
    else:
        rect = api.rect(); saved = core.load_config().get("dash_rect"); i = info()
        check("2nd launch reopens at the remembered size", saved and all(abs(a - b) <= 3 for a, b in zip(rect, saved)) and i["user"] is True, f"saved={saved} now={rect}")
        js("autoFit()"); time.sleep(2.0)
        i = info(); cfgn = core.load_config()
        check("'tự vừa' forgets the size: window follows the content again", cfgn.get("dash_rect") is None and i["user"] is False and i["ms"] <= i["mc"] + 1, f"{i} dash_rect={cfgn.get('dash_rect')}")
        print(f"\n{sum(results)}/{len(results)} checks passed (phase 2)", flush=True)
    js("document.querySelector('.wb.x').click()")
    time.sleep(8); os._exit(3)

threading.Thread(target=driver, daemon=True).start()
webview.start()
os._exit(3)
