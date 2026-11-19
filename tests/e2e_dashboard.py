"""E2E test of the dashboard window: drives the real page with JS clicks (needs a desktop).
Sandbox data dir + sandbox profiles, so hook install / uninstall never touch the real settings.json.
Run:  .venv\\Scripts\\python tests\\e2e_dashboard.py [x|esc]"""
import json, os, sys, tempfile, threading, time
from datetime import datetime, timezone, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SANDBOX = Path(tempfile.mkdtemp(prefix="cu_d_")); os.environ["LOCALAPPDATA"] = str(SANDBOX)
sys.path.insert(0, str(ROOT))
import core, dashboard, webview
HOW = sys.argv[1] if len(sys.argv) > 1 else "x"
iso = lambda s: (datetime.now(timezone.utc) + timedelta(seconds=s)).isoformat()
profiles = []
for n in ("default (desktop)", "claude-da", "claude-hn", "claude-ngoai"):
    d = SANDBOX / n; d.mkdir(); profiles.append({"name": n, "path": str(d)})
(SANDBOX / "claude-da" / "settings.json").write_text(json.dumps({"model": "keep-me", "hooks": {"UserPromptSubmit": [{"hooks": [{"type": "command", "command": "other.exe"}]}]}}))
data = {p["name"]: {"five": 20 + 20 * i, "week": 30 + 10 * i, "five_reset": iso(3600 * (i + 1)), "week_reset": iso(86400 * (i + 1)), "fetched_at": time.time()} for i, p in enumerate(profiles)}
data["claude-hn"].update(err="HTTP 429", retry_at=time.time() + 150)
data["default (desktop)"].update(plan="team", extra={"used": 0.98, "limit": 25.0, "pct": 3.92, "currency": "USD"}, scoped=[{"name": "Fable", "pct": 21, "resets_at": iso(86400)}])
cfg = dict(core.DEFAULTS, profiles=profiles); core.save_config(cfg); core.write_cache(cfg, data, busy={"claude-da"})
SHOTS = Path(os.environ.get("E2E_SHOTS", SANDBOX)); results = []
def check(name, ok, detail=""):
    ok = bool(ok); results.append(ok); print(f"  {'ok  ' if ok else 'FAIL'} {name}" + (f"  [{detail}]" if detail else ""), flush=True)

api = dashboard.Api(); t0 = time.time()
api._window = webview.create_window("claude-usage", html=dashboard.page(), js_api=api, width=760, height=640, min_size=(620, 380), frameless=True, easy_drag=False)
api._window.events.closed += lambda: os._exit(0 if all(results) else 1)

def driver():
    w = api._window; js = lambda c: w.evaluate_js(c)
    for _ in range(200):
        try:
            if js("!!(window.pywebview && pywebview.api && document.querySelectorAll('.acc').length)"): break
        except Exception: pass
        time.sleep(0.1)
    check("page ready quickly", time.time() - t0 < 6, f"{time.time()-t0:.1f}s")
    time.sleep(1.8)
    check("4 accounts rendered", js("document.querySelectorAll('.acc').length") == 4)
    check("spinner shown for the account being fetched", js("document.querySelectorAll('.acc .spin').length") == 1 and "claude-da" in js("document.querySelector('.acc .spin').parentElement.textContent"))
    check("footer shows 'updating' state while busy", "đang cập nhật" in js("document.getElementById('stat').textContent"))
    check("refresh button stays clickable while the app polls in the background", js("document.getElementById('rbtn').disabled") is False)
    check("429 account shows cached value + warning + countdown", "dữ liệu cũ" in js("document.body.innerText") and "thử lại sau" in js("document.body.innerText"))
    m = json.loads(js("(()=>{const m=document.querySelector('main'),d=document.documentElement;return JSON.stringify({a:m.scrollHeight,b:m.clientHeight,c:d.scrollHeight,d:innerHeight,hs:d.scrollWidth>innerWidth})})()"))
    check("NO scrollbar needed", m["a"] <= m["b"] + 1 and m["c"] <= m["d"] + 1 and not m["hs"], str(m))
    js("window.__AVAIL=740;lastFit=0;fit()"); time.sleep(1.2)                 # simulate a 768px-high laptop screen
    m2 = json.loads(js("(()=>{const m=document.querySelector('main'),d=document.documentElement;return JSON.stringify({compact:document.body.classList.contains('compact'),a:m.scrollHeight,b:m.clientHeight,need:natural(),c:d.scrollHeight,d:innerHeight})})()"))
    check("small screen: compact mode keeps everything visible without scrolling", m2["compact"] and m2["need"] <= 740 - 24 + 4 and m2["c"] <= m2["d"] + 1, str(m2))
    js("window.__AVAIL=0;lastFit=0;fit()"); time.sleep(1.0)
    check("normal screen: compact mode is off again", js("document.body.classList.contains('compact')") is False)
    core.write_cache(cfg, data, busy=())                       # fetch finished
    time.sleep(2.6)
    check("spinner gone and button enabled when idle", js("document.querySelectorAll('.acc .spin').length") == 0 and js("document.getElementById('rbtn').disabled") is False)
    # --- no flicker: polls that bring nothing new must not rebuild the list (that restarts the spinner/pulse animations)
    js("document.querySelector('.acc').__mark=1"); time.sleep(4.5)
    check("account list is not rebuilt when nothing changed (no flicker)", js("document.querySelector('.acc').__mark===1") is True)
    # --- refresh feedback
    js("refresh()"); time.sleep(0.6)
    check("click refresh -> instant 'requesting' feedback", "đang" in js("document.getElementById('rbtn').textContent") and js("document.getElementById('rbtn').disabled") is True)
    core.write_cache(cfg, data, busy=("claude-da",), refresh={"ts": time.time(), "started": ["claude-da"], "skipped": [{"name": "x", "why": "vừa cập nhật xong"}]}); time.sleep(1.5)
    check("refresh result toast shown (what started / skipped)", "đang refresh 1 account" in js("document.getElementById('toast').textContent"), js("document.getElementById('toast').textContent"))
    core.write_cache(cfg, data, busy=()); time.sleep(2.5)
    # the (faded) toast sits over refresh/save: it must never swallow clicks
    js("toast('widget đã mở khoá: kéo để di chuyển, ổ khoá để khoá lại',200)"); time.sleep(0.6)
    covered = json.loads(js("""JSON.stringify([...document.querySelectorAll('button')].filter(b=>b.offsetParent).filter(b=>{const r=b.getBoundingClientRect();
        return [[.5,.5],[.15,.25],[.85,.25],[.15,.75],[.85,.75]].some(([fx,fy])=>{const e=document.elementFromPoint(r.left+r.width*fx,r.top+r.height*fy);
        return !(e===b||b.contains(e))})}).map(b=>b.textContent.trim()))"""))
    check("no button covered by toast / overlay (clicks land first time)", not covered, str(covered))
    # --- notification settings persist
    js("document.getElementById('nen').checked=false;document.getElementById('nwarn').checked=false;document.getElementById('nblock').checked=true;save()"); time.sleep(1)
    c = json.loads(core.CONFIG.read_text("utf-8"))
    check("notification toggles saved (master off, warn off, block on)", c["notify_enabled"] is False and c["notify_warn"] is False and c["notify_block"] is True)
    js("document.getElementById('nen').checked=true;document.getElementById('nwarn').checked=true;save()"); time.sleep(1)
    c = json.loads(core.CONFIG.read_text("utf-8")); check("notification toggles saved (back on)", c["notify_enabled"] and c["notify_warn"])
    core.TESTFLAG.unlink(missing_ok=True); js("testNotify()"); time.sleep(.6)
    check("'send test notification' raises the flag for the app", core.TESTFLAG.exists())
    js("document.getElementById('wstyle').value='cards';save()"); time.sleep(1)
    check("widget style selector saved", json.loads(core.CONFIG.read_text("utf-8"))["widget_style"] == "cards")
    js("document.getElementById('wstyle').value='rings';save()"); time.sleep(0.8)
    # --- appearance section: collapsed by default, auto-applies, per-account names + visibility
    check("appearance tab is not shown by default (general tab is)", js("document.getElementById('look').hidden") is True and js("document.getElementById('gen').hidden") is False)
    h_closed = js("document.documentElement.scrollHeight")
    js("tab('look')"); time.sleep(1.2)
    m3 = json.loads(js("(()=>{const m=document.querySelector('main'),d=document.documentElement;return JSON.stringify({a:m.scrollHeight,b:m.clientHeight,c:d.scrollHeight,d:innerHeight,rows:document.querySelectorAll('#acclabels .al').length})})()"))
    check("appearance tab has one row per account and needs NO scrollbar", m3["rows"] == 4 and m3["a"] <= m3["b"] + 1 and m3["c"] <= m3["d"] + 1, str(m3))
    js("""(()=>{const set=(id,v)=>{const e=document.getElementById(id);e.value=v;e.dispatchEvent(new Event('input',{bubbles:true}))};
      set('wlabel','short');set('wfont','125');set('wopac','80');set('wtheme','light');
      const p=document.getElementById('wplate');p.checked=true;p.dispatchEvent(new Event('change',{bubbles:true}));
      const names=document.querySelectorAll('#acclabels input[data-n]');names[0].value='Chính';names[0].dispatchEvent(new Event('input',{bubbles:true}));
      const shows=document.querySelectorAll('#acclabels input[data-show]');shows[3].checked=false;shows[3].dispatchEvent(new Event('change',{bubbles:true}))})()""")
    time.sleep(1.5)                                            # no save button pressed: must auto-apply
    c = json.loads(core.CONFIG.read_text("utf-8"))
    check("changes auto-apply without pressing save", c["widget_label"] == "short" and c["widget_font"] == 125 and c["widget_opacity"] == 80 and c["widget_theme"] == "light" and c["widget_plate"] is True,
          str({k: c.get(k) for k in ("widget_label", "widget_font", "widget_opacity", "widget_theme", "widget_plate")}))
    check("custom account name saved, blank ones stay automatic", c["aliases"] == {"default (desktop)": "Chính"}, str(c.get("aliases")))
    check("unticked account is hidden from the widget", c["widget_hidden"] == ["claude-ngoai"], str(c.get("widget_hidden")))
    js("""(()=>{const set=(id,v)=>{const e=document.getElementById(id);e.value=v;e.dispatchEvent(new Event('change',{bubbles:true}))};set('wsort','usage');set('wmax','7')})()"""); time.sleep(1.5)
    c = json.loads(core.CONFIG.read_text("utf-8"))
    check("widget sort order and max-accounts cap are saved", c["widget_sort"] == "usage" and c["widget_max_cells"] == 7, f"{c.get('widget_sort')} / {c.get('widget_max_cells')}")
    js("""(()=>{const set=(id,v)=>{const e=document.getElementById(id);e.value=v;e.dispatchEvent(new Event('change',{bubbles:true}))};set('wsort','config');set('wmax','0')})()"""); time.sleep(1.2)
    # --- what-to-show checkboxes
    flags = ("widget_show_label", "widget_show_5h", "widget_show_7d", "widget_show_reset5", "widget_show_reset7", "widget_tooltip")
    check("all 'what to show' boxes start ticked and are saved as True", all(c[k] is True for k in flags), str({k: c.get(k) for k in flags}))
    js("""(()=>{const f=(id,v)=>{const e=document.getElementById(id);e.checked=v;e.dispatchEvent(new Event('change',{bubbles:true}))};
      f('sl',false);f('s7',false);f('r5',false);f('wtip',false)})()"""); time.sleep(1.5)
    c = json.loads(core.CONFIG.read_text("utf-8"))
    check("unticking hides exactly those items", (c["widget_show_label"], c["widget_show_5h"], c["widget_show_7d"], c["widget_show_reset5"], c["widget_show_reset7"], c["widget_tooltip"]) == (False, True, False, False, True, False),
          str({k: c.get(k) for k in flags}))
    check("reset master stays on while one reset line is still shown", c["widget_reset"] is True)
    js("""(()=>{const f=(id,v)=>{const e=document.getElementById(id);e.checked=v;e.dispatchEvent(new Event('change',{bubbles:true}))};f('r7',false)})()"""); time.sleep(1.2)
    check("both reset lines off -> master reset flag off", json.loads(core.CONFIG.read_text("utf-8"))["widget_reset"] is False)
    js("""(()=>{const f=(id,v)=>{const e=document.getElementById(id);e.checked=v;e.dispatchEvent(new Event('change',{bubbles:true}))};['sl','s7','r5','r7','wtip'].forEach(i=>f(i,true))})()"""); time.sleep(1.5)
    c = json.loads(core.CONFIG.read_text("utf-8")); check("everything can be ticked back on", all(c[k] is True for k in flags) and c["widget_reset"] is True)
    tt = js("document.querySelector('.acc').title")
    check("hovering an account row in the dashboard shows full details (5h, 7d, per-model, credit)", all(s in tt for s in ("5 giờ", "7 ngày", "Fable", "Credit dùng thêm: $0.98 / $25.00")), tt.replace(chr(10), " | ")[:160])
    for avail in (1080, 900, 768):                                # laptop screens with the appearance section open
        js(f"window.__AVAIL={avail};lastFit=0;fit()"); time.sleep(1.0)
        mm = json.loads(js("(()=>JSON.stringify({compact:document.body.classList.contains('compact'),need:natural(),inner:innerHeight,doc:document.documentElement.scrollHeight}))()"))
        check(f"screen {avail}px high + appearance open: window fits the screen", mm["need"] <= avail - 24 + 4 and mm["doc"] <= mm["inner"] + 1, str(mm))
    js("window.__AVAIL=0;lastFit=0;fit()"); time.sleep(0.8)
    js("tab('gen')"); time.sleep(1.0)
    h_now = js("document.documentElement.scrollHeight")
    check("switching back to the general tab restores its height", abs(h_now - h_closed) < 15, f"before={h_closed} open={m3['c']} after={h_now} inner={js('innerHeight')} compact={js('document.body.classList.contains(\"compact\")')} lastFit={js('lastFit')}")
    js("""(()=>{const set=(id,v)=>{const e=document.getElementById(id);e.value=v;e.dispatchEvent(new Event('input',{bubbles:true}))};
      set('wlabel','alias');set('wfont','100');set('wopac','100');set('wtheme','auto');
      const p=document.getElementById('wplate');p.checked=false;p.dispatchEvent(new Event('change',{bubbles:true}));
      document.querySelectorAll('#acclabels input[data-n]')[0].value='';document.querySelectorAll('#acclabels input[data-show]')[3].checked=true;
      document.querySelectorAll('#acclabels input[data-show]')[3].dispatchEvent(new Event('change',{bubbles:true}))})()"""); time.sleep(1.5)
    c = json.loads(core.CONFIG.read_text("utf-8")); check("everything can be set back to defaults", c["widget_label"] == "alias" and c["aliases"] == {} and c["widget_hidden"] == [] and c["widget_plate"] is False)
    # --- interval floor
    js("document.getElementById('interval').value=10;save()"); time.sleep(1)
    check("interval below 90 is clamped to 90", json.loads(core.CONFIG.read_text("utf-8"))["interval_sec"] == 90)
    # --- hook buttons (sandbox profiles only)
    js("hook('install')"); time.sleep(1.4)
    ok_all = all(json.loads((Path(p["path"]) / "settings.json").read_text())["hooks"]["UserPromptSubmit"][-1]["hooks"][0]["command"].endswith(" hook") for p in profiles)
    da = json.loads((SANDBOX / "claude-da" / "settings.json").read_text())
    check("install hook: all profiles get it, existing settings + other hooks preserved", ok_all and da["model"] == "keep-me" and any(h["command"] == "other.exe" for g in da["hooks"]["UserPromptSubmit"] for h in g["hooks"]))
    check("hook status text shows 4/4", "4/4" in js("document.getElementById('hookst').textContent"), js("document.getElementById('hookst').textContent"))
    js("hook('uninstall')"); time.sleep(1.4)
    da = json.loads((SANDBOX / "claude-da" / "settings.json").read_text())
    check("uninstall hook removes only ours", da["hooks"]["UserPromptSubmit"] == [{"hooks": [{"type": "command", "command": "other.exe"}]}] and not (SANDBOX / "claude-ngoai" / "settings.json").read_text().count("claude-usage") and "0/4" in js("document.getElementById('hookst').textContent"))
    # --- widget buttons
    js("document.querySelectorAll('button.b')[1].click()"); time.sleep(.6)
    check("unlock widget button", json.loads(core.CONFIG.read_text("utf-8"))["widget_locked"] is False)
    js("document.querySelectorAll('button.b')[2].click()"); time.sleep(.6)
    c = json.loads(core.CONFIG.read_text("utf-8")); check("reset widget button", c["widget_locked"] is True and c["widget_cx"] is None and c["widget_size"] == 36)
    # --- drag the window by its title bar (pywebview's own move; broke once because we set argtypes on the shared user32)
    r0 = api.rect()
    js("""(()=>{const d=document.querySelector('.drag'),r=d.getBoundingClientRect();
      d.dispatchEvent(new MouseEvent('mousedown',{bubbles:true,clientX:r.left+60,clientY:r.top+10,screenX:500,screenY:400,button:0}))})()""")
    for i in range(1, 5):
        js(f"window.dispatchEvent(new MouseEvent('mousemove',{{bubbles:true,screenX:{500 + i * 30},screenY:{400 + i * 20}}}))"); time.sleep(.15)
    js("window.dispatchEvent(new MouseEvent('mouseup',{bubbles:true}))"); time.sleep(.5)
    r1 = api.rect()
    check("window can be dragged by the title bar", abs(r1[0] - r0[0]) > 20 and r1[2:] == r0[2:], f"{r0} -> {r1}")
    js("document.querySelector('.wb').click()"); time.sleep(.8); check("minimize button does not crash", True)
    w.restore(); time.sleep(.5)
    # closing right after a change must still save it (pending auto-save is flushed first), then end the process
    def quit_check(code=0):
        c = json.loads(core.CONFIG.read_text("utf-8"))
        check(f"close ({HOW}) right after a change still saves it", c["widget_font"] == 130, c["widget_font"])
        print(f"\n{sum(results)}/{len(results)} checks passed; closed via {HOW}", flush=True)
        os._exit(0 if all(results) else 1)
    dashboard.quit_now = quit_check
    js("(()=>{const e=document.getElementById('wfont');e.value='130';e.dispatchEvent(new Event('input',{bubbles:true}))})()")
    if HOW == "x": js("document.querySelector('.wb.x').click()")
    else: js("document.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape'}))")
    time.sleep(8); os._exit(3)                                 # window must have closed the process by now

threading.Thread(target=driver, daemon=True).start()
webview.start()
os._exit(3)
