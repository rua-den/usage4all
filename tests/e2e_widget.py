"""E2E test of the taskbar widget with real Win32 mouse messages (needs a desktop). Uses a sandbox data dir and a fake cache.
Run:  .venv\\Scripts\\python tests\\e2e_widget.py"""
import ctypes, json, math, os, sys, tempfile, time
from datetime import datetime, timezone, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SANDBOX = Path(tempfile.mkdtemp(prefix="cu_w_")); os.environ["LOCALAPPDATA"] = str(SANDBOX)
sys.path.insert(0, str(ROOT))
ctypes.windll.shcore.SetProcessDpiAwareness(2)
import core, widget
SHOTS = Path(os.environ.get("E2E_SHOTS", SANDBOX))
results = []
def check(name, ok, detail=""):
    ok = bool(ok); results.append(ok); print(f"  {'ok  ' if ok else 'FAIL'} {name}" + (f"  [{detail}]" if detail else ""), flush=True)

iso = lambda s: (datetime.now(timezone.utc) + timedelta(seconds=s)).isoformat()
profiles = [{"name": n, "path": str(SANDBOX / n)} for n in ("default (desktop)", "claude-da", "claude-hn", "claude-ngoai")]
data = {p["name"]: {"five": 30 + 15 * i, "week": 40 + 5 * i, "five_reset": iso(3600 * (i + 1)), "week_reset": iso(86400 * (i + 1)), "fetched_at": time.time()} for i, p in enumerate(profiles)}
cfg = dict(core.DEFAULTS, profiles=profiles, widget_locked=False, widget_cx=None, widget_cy=None, widget_angle=0.0, widget_size=36)
data[profiles[0]["name"]].update(plan="team", extra={"used": 0.98, "limit": 25.0, "pct": 3.92, "currency": "USD"}, scoped=[{"name": "Fable", "pct": 21, "resets_at": iso(86400)}])
core.save_config(cfg); core.write_cache(cfg, data)

opened = []
w = widget.Widget(lambda: opened.append(1), lambda: None, lambda: None); w.start()
u = ctypes.windll.user32
def post(msg, x, y): u.PostMessageW(w.hwnd, msg, 0, ((y & 0xFFFF) << 16) | (x & 0xFFFF)); time.sleep(0.25)
def client(sx, sy): return int(round(sx - w.org[0])), int(round(sy - w.org[1]))
def screen_of(fx, fy):
    g = w.geo; rx, ry = w.rot(fx - g["Wp"] / 2, fy - g["Hp"] / 2, w.built_angle)
    return w.org[0] + w.img.width / 2 + rx, w.org[1] + w.img.height / 2 + ry
def conf(): return json.loads(core.CONFIG.read_text("utf-8"))
def drag(kind, dx, dy):
    g = w.geo; fx, fy = {"body": (g["M"] + g["W"] / 2, g["Mt"] + g["H"] / 2), "size": g["size"], "rot": g["rot"], "lock": g["lock"]}[kind]
    sx, sy = screen_of(fx, fy); post(0x201, *client(sx, sy))
    for i in range(1, 7): post(0x200, *client(sx + dx * i / 6, sy + dy * i / 6))
    post(0x202, *client(sx + dx, sy + dy)); time.sleep(0.4); return conf()

time.sleep(4)
check("widget visible next to the taskbar tray", w.visible and w.geo is not None, f"org={w.org} img={w.img.size}")
base = w.content_center(w.taskbar())
c = drag("body", -300, -250)
check("MOVE: content centre follows the mouse exactly", abs(c["widget_cx"] - (base[0] - 300)) < 2 and abs(c["widget_cy"] - (base[1] - 250)) < 2, f"({c['widget_cx']:.0f},{c['widget_cy']:.0f})")
cx, cy, s0 = c["widget_cx"], c["widget_cy"], c["widget_size"]
hx, hy = screen_of(*w.geo["size"]); d0 = math.hypot(hx - cx, hy - cy); ux, uy = (hx - cx) / d0, (hy - cy) / d0
c = drag("size", ux * d0 * .5, uy * d0 * .5)
check("RESIZE: size x1.5", abs(c["widget_size"] - s0 * 1.5) < 3, f"{s0} -> {c['widget_size']:.1f}")
rx, ry = screen_of(*w.geo["rot"]); c = drag("rot", (cx + 150) - rx, cy - ry)
check("ROTATE: snaps to 90 degrees", c["widget_angle"] == 90.0, str(c["widget_angle"]))
rx, ry = screen_of(*w.geo["rot"]); c = drag("rot", cx + 90 * math.sin(math.radians(35)) - rx, cy - 90 * math.cos(math.radians(35)) - ry)
check("ROTATE: free angle (~35) and centre stays put", abs(c["widget_angle"] - 35) < 2 and abs(c["widget_cx"] - cx) < 1 and abs(c["widget_cy"] - cy) < 1, f"angle={c['widget_angle']}")
(Path(os.environ.get("E2E_SHOTS", SANDBOX)) / "w_edit.png").parent.mkdir(exist_ok=True); w.img.save(SHOTS / "w_edit.png")
c = drag("lock", 0, 0); check("padlock locks the widget", c["widget_locked"] is True)
time.sleep(1.2)
bx, by = screen_of(w.geo["M"] + w.geo["W"] / 2, w.geo["Mt"] + w.geo["H"] / 2)
post(0x201, *client(bx, by)); post(0x202, *client(bx, by)); check("locked: click opens the dashboard", len(opened) == 1)
before = conf(); post(0x201, *client(bx, by)); post(0x200, *client(bx + 80, by + 80)); post(0x202, *client(bx + 80, by + 80)); time.sleep(.3)
check("locked: dragging does NOT move it", conf()["widget_cx"] == before["widget_cx"] and len(opened) == 1)

print("-- waiting indicator")
idle = w.img.tobytes(); w.tick(force=True)
core.write_cache(core.load_config(), data, busy={"claude-da"}); time.sleep(1.6)
check("fast animation timer starts when something is being fetched", w.fast is True)
a = w.img.tobytes(); time.sleep(0.5); b = w.img.tobytes()
check("spinner is drawn and moves", a != idle and a != b)
core.write_cache(core.load_config(), data); time.sleep(1.6)
check("animation timer stops when idle (no wasted CPU)", w.fast is False)

print("-- stale / error cells and layouts render")
d2 = {**data, "claude-hn": {**data["claude-hn"], "err": "HTTP 429", "retry_at": time.time() + 100}, "claude-ngoai": {"err": "HTTP 429", "status": 429}}
core.write_cache(core.load_config(), d2); time.sleep(1.6); ok1 = w.img.width > 0
cfg2 = core.load_config(); cfg2["widget_layout"] = "horizontal"; core.save_config(cfg2); time.sleep(1.6)
check("horizontal layout + stale + error cells render", ok1 and w.vertical is False and w.img.width > w.img.height, f"img={w.img.size}")
cfg2["widget_layout"] = "auto"; cfg2["widget_enabled"] = False; core.save_config(cfg2); time.sleep(1.6)
check("'hide widget' really hides it", w.visible is False)
print("-- styles")
cfg3 = core.load_config(); cfg3.update(widget_enabled=True, widget_locked=True, widget_cx=None, widget_cy=None, widget_layout="auto", widget_angle=0.0, widget_size=36); core.save_config(cfg3); time.sleep(1.8)
imgs = {}
for st in widget.STYLES:
    cfg3["widget_style"] = st; core.save_config(cfg3); time.sleep(1.8); imgs[st] = w.img.tobytes()
check("changing the style in config redraws the live widget (all 5 distinct)", len(set(imgs.values())) == len(widget.STYLES))
cfg3["widget_style"] = "cards"; core.save_config(cfg3); time.sleep(1.8)
bx, by = screen_of(w.geo["M"] + w.geo["W"] / 2, w.geo["Mt"] + 20); cxl, cyl = client(bx, by)
widget.user32.TrackMouseEvent = lambda *a: 1       # no real cursor over the widget: stop Windows sending WM_MOUSELEAVE back
u.SendMessageW.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_size_t, ctypes.c_ssize_t]
u.SendMessageW(w.hwnd, 0x200, 0, ((cyl & 0xFFFF) << 16) | (cxl & 0xFFFF))   # synchronous: runs on the widget thread, like a real mouse move
h0 = w.hover
by2 = by + w.cell_h; cx2, cy2 = client(bx, by2); u.SendMessageW(w.hwnd, 0x200, 0, ((cy2 & 0xFFFF) << 16) | (cx2 & 0xFFFF))
check("hover maps to the right card on a card-style widget (1st, then 2nd)", h0 == 0 and w.hover == 1,
      f"hover {h0} -> {w.hover}; client2=({cx2},{cy2}) frame={tuple(round(v) for v in w.to_frame(cx2, cy2))} cell_h={w.cell_h:.0f} W,H={w.geo['W']},{w.geo['H']} img={w.img.size} vertical={w.vertical}")
w.img.save(SHOTS / "w_cards_live.png")
print("-- hover detail card")
sw, sh = u.GetSystemMetrics(0), u.GetSystemMetrics(1)
cfg3.update(widget_style="cards", widget_tooltip=True, widget_locked=True, widget_cx=None, widget_cy=None, widget_angle=0.0); core.save_config(cfg3); time.sleep(1.8)
u.FindWindowW.restype = ctypes.c_void_p
class RECT(ctypes.Structure): _fields_ = [("l", ctypes.c_long), ("t", ctypes.c_long), ("r", ctypes.c_long), ("b", ctypes.c_long)]
def tip_rect():
    h = u.FindWindowW("ClaudeUsageTip", None)
    if not h or not u.IsWindowVisible(h): return None
    r = RECT(); u.GetWindowRect(h, ctypes.byref(r)); return r
def move_to_cell(i):
    fx, fy = w.geo["M"] + w.geo["W"] / 2, w.geo["Mt"] + i * w.cell_h + 20
    sx, sy = screen_of(fx, fy); cxl, cyl = client(sx, sy)
    u.SendMessageW(w.hwnd, 0x200, 0, ((cyl & 0xFFFF) << 16) | (cxl & 0xFFFF))
u.SendMessageW(w.hwnd, 0x2A3, 0, 0); time.sleep(1.0)            # the previous check left the pointer "on" the widget
check("no card before hovering", tip_rect() is None)
move_to_cell(0); time.sleep(0.15); early = tip_rect(); time.sleep(0.9); r1 = tip_rect()
check("card appears only after a short hover delay", early is None and r1 is not None, f"early={early is not None} later={r1 is not None}")
wr = (w.org[0], w.org[1], w.img.width, w.img.height)
check("card sits beside the widget (not on top of it) and fully on screen", r1 is not None and r1.r <= wr[0] + 1 and r1.l >= 0 and r1.t >= 0 and r1.b <= sh,
      f"card=({r1.l},{r1.t},{r1.r},{r1.b}) widget_x={wr[0]}" if r1 else "")
img1 = w.tip_img.tobytes(); h1 = w.tip_img.height
move_to_cell(1); time.sleep(0.9)
check("moving to another account switches the card (that account has no credit row)", w.tip_img.tobytes() != img1 and w.tip_img.height < h1, f"{h1} -> {w.tip_img.height}")
w.tip_img.save(SHOTS / "w_tip.png")
u.SendMessageW(w.hwnd, 0x2A3, 0, 0); time.sleep(0.6)
check("card disappears when the pointer leaves", tip_rect() is None)
cfg3.update(widget_tooltip=False); core.save_config(cfg3); time.sleep(1.8); move_to_cell(0); time.sleep(1.0)
check("card can be switched off in settings", tip_rect() is None)
u.SendMessageW(w.hwnd, 0x2A3, 0, 0); cfg3.update(widget_tooltip=True); core.save_config(cfg3); time.sleep(1.5)
print("-- appearance (live)")
h_all, img_all = w.img.height, w.img.tobytes()
cfg3.update(widget_hidden=["claude-ngoai"]); core.save_config(cfg3); time.sleep(1.8)
check("hiding an account shortens the live widget", w.img.height < h_all * 0.85, f"{h_all} -> {w.img.height}")
cfg3.update(widget_hidden=[], aliases={"default (desktop)": "Chính"}, widget_label="short", widget_font=130, widget_plate=True); core.save_config(cfg3); time.sleep(1.8)
check("custom name / font / plate redraw the live widget", w.img.tobytes() != img_all and w.img.width <= 48, f"w={w.img.width}")
cfg3.update(widget_opacity=50); core.save_config(cfg3); time.sleep(1.8)
check("opacity setting is applied to the window alpha", widget.opacity_alpha(w.cfg) == int(255 * 0.5), str(widget.opacity_alpha(w.cfg)))
cfg3.update(aliases={}, widget_label="alias", widget_font=100, widget_plate=False, widget_opacity=100, widget_style="rings"); core.save_config(cfg3); time.sleep(1.5)
print("-- many accounts (live)")
many = [{"name": f"claude-team-{i:02d}", "path": str(SANDBOX / f"t{i}")} for i in range(1, 15)]
dm = {p["name"]: {"five": (i * 17) % 100, "week": (i * 23) % 100, "five_reset": iso(3600 * (i % 5 + 1)), "week_reset": iso(86400 * (i % 6 + 1)), "fetched_at": time.time()} for i, p in enumerate(many)}
cfg5 = core.load_config()
cfg5.update(profiles=many, widget_style="rings", widget_enabled=True, widget_locked=True, widget_cx=None, widget_cy=None, widget_angle=0.0, widget_size=36,
            widget_sort="usage", widget_layout="auto", widget_tooltip=True, widget_hidden=[], widget_max_cells=0)
core.save_config(cfg5); core.write_cache(cfg5, dm); time.sleep(2.5)
limit = widget.stack_limit(cfg5, w.vertical, True, (sw, sh)); along = w.img.height if w.vertical else w.img.width
check("14 accounts: the widget stays within the screen budget", along <= limit * 1.03 and w.img.height <= sh, f"{along}px along the axis, limit {limit}px")
check("14 accounts fit by SHRINKING first (no cut needed here)", widget.MORE not in dict(w.view) and len(w.view) == 14 and w.plan[0] < 36, f"size {w.plan[0]:.1f} (was 36), showing {len(w.view)}")
worst = max(dm, key=lambda n: max(dm[n]["five"], dm[n]["week"]))
check("sorted by usage: the busiest account is first", w.view[0][0] == worst, f"{w.view[0][0]} (expected {worst})")
cfg5.update(widget_max_cells=9); core.save_config(cfg5); time.sleep(2.3)
check("when a cap/limit forces a cut, the rest become one '+N' cell at the end", len(w.view) == 10 and w.view[-1][0] == widget.MORE and w.view[-1][1]["more"] == 5, f"{len(w.view)} cells, more={w.view[-1][1].get('more')}")
u.SendMessageW(w.hwnd, 0x2A3, 0, 0); time.sleep(0.8)
move_to_cell(len(w.view) - 1); time.sleep(1.2); rt = tip_rect(); mt = w.tip_img
check("hovering the '+N' cell lists the accounts that are not shown", rt is not None and mt is not None and mt.height > 80 and w.tip_key and w.tip_key[0] == widget.MORE, f"card {mt.size if mt else None}")
u.SendMessageW(w.hwnd, 0x2A3, 0, 0); time.sleep(0.6)
cfg5.update(widget_max_cells=4); core.save_config(cfg5); time.sleep(2.3)
check("user cap 'show at most 4' is honoured", len(w.view) == 5 and w.view[-1][0] == widget.MORE and w.view[-1][1]["more"] == 10, f"{len(w.view)} cells")
cfg5.update(widget_style="cards"); core.save_config(cfg5); time.sleep(2.2)
check("works in another style too, still inside the budget", (w.img.height if w.vertical else w.img.width) <= limit * 1.03 + 2, f"{w.img.size}")
print(f"\n{sum(results)}/{len(results)} checks passed")
os._exit(0 if all(results) else 1)
