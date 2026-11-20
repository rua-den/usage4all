"""Taskbar widget: per-pixel-alpha layered window.
Per account: outer ring = 5h, inner ring = 7d, optional reset times. Native Win32 + Pillow (no WebView).
Locked: sits next to the tray, click opens the dashboard.
Unlocked (edit mode): drag to move, corner handle to resize, top handle to rotate, padlock to lock."""
import ctypes, functools, math, threading, time
from datetime import datetime, timedelta
from ctypes import wintypes as wt
from PIL import Image, ImageDraw, ImageFont
import core

user32, gdi32, shell32 = ctypes.windll.user32, ctypes.windll.gdi32, ctypes.windll.shell32
LRESULT = ctypes.c_ssize_t
WNDPROC = ctypes.WINFUNCTYPE(LRESULT, wt.HWND, wt.UINT, wt.WPARAM, wt.LPARAM)

user32.DefWindowProcW.argtypes = [wt.HWND, wt.UINT, wt.WPARAM, wt.LPARAM]
user32.DefWindowProcW.restype = LRESULT
user32.CreateWindowExW.restype = wt.HWND
user32.CreateWindowExW.argtypes = [wt.DWORD, wt.LPCWSTR, wt.LPCWSTR, wt.DWORD, ctypes.c_int, ctypes.c_int,
                                   ctypes.c_int, ctypes.c_int, wt.HWND, wt.HMENU, wt.HINSTANCE, wt.LPVOID]
user32.FindWindowW.restype = wt.HWND
user32.FindWindowExW.restype = wt.HWND
user32.FindWindowExW.argtypes = [wt.HWND, wt.HWND, wt.LPCWSTR, wt.LPCWSTR]
user32.GetDC.restype = wt.HDC
user32.UpdateLayeredWindow.argtypes = [wt.HWND, wt.HDC, ctypes.c_void_p, ctypes.c_void_p, wt.HDC,
                                       ctypes.c_void_p, wt.COLORREF, ctypes.c_void_p, wt.DWORD]
user32.LoadCursorW.restype = ctypes.c_void_p
user32.LoadCursorW.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
user32.SetCursor.argtypes = [ctypes.c_void_p]
user32.SetCapture.argtypes = [wt.HWND]
gdi32.CreateCompatibleDC.restype = wt.HDC
gdi32.CreateCompatibleDC.argtypes = [wt.HDC]
gdi32.SelectObject.argtypes = [wt.HDC, wt.HANDLE]
gdi32.SelectObject.restype = wt.HANDLE
gdi32.DeleteObject.argtypes = [wt.HANDLE]
gdi32.DeleteDC.argtypes = [wt.HDC]
gdi32.CreateDIBSection.restype = wt.HBITMAP
gdi32.CreateDIBSection.argtypes = [wt.HDC, ctypes.c_void_p, wt.UINT, ctypes.POINTER(ctypes.c_void_p), wt.HANDLE, wt.DWORD]
user32.SetWindowPos.argtypes = [wt.HWND, wt.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wt.UINT]


class WNDCLASS(ctypes.Structure):
    _fields_ = [("style", wt.UINT), ("lpfnWndProc", WNDPROC), ("cbClsExtra", ctypes.c_int),
                ("cbWndExtra", ctypes.c_int), ("hInstance", wt.HINSTANCE), ("hIcon", wt.HICON),
                ("hCursor", wt.HANDLE), ("hbrBackground", wt.HBRUSH), ("lpszMenuName", wt.LPCWSTR),
                ("lpszClassName", wt.LPCWSTR)]


class BIH(ctypes.Structure):
    _fields_ = [("biSize", wt.DWORD), ("biWidth", wt.LONG), ("biHeight", wt.LONG), ("biPlanes", wt.WORD),
                ("biBitCount", wt.WORD), ("biCompression", wt.DWORD), ("biSizeImage", wt.DWORD),
                ("biXPelsPerMeter", wt.LONG), ("biYPelsPerMeter", wt.LONG), ("biClrUsed", wt.DWORD),
                ("biClrImportant", wt.DWORD)]


class BLEND(ctypes.Structure):
    _fields_ = [("op", ctypes.c_ubyte), ("flags", ctypes.c_ubyte), ("alpha", ctypes.c_ubyte), ("fmt", ctypes.c_ubyte)]


class TME(ctypes.Structure):
    _fields_ = [("cbSize", wt.DWORD), ("dwFlags", wt.DWORD), ("hwndTrack", wt.HWND), ("dwHoverTime", wt.DWORD)]


SS = 3                      # supersampling factor
ACCENT = (250, 178, 131)


@functools.lru_cache(maxsize=256)
def _font(name, px):
    for f in (name, "segoeui.ttf", "arial.ttf"):
        try: return ImageFont.truetype(f, px)
        except Exception: pass
    return ImageFont.load_default()


def fit_font(name, px, text, max_w):
    """A font of size <= px whose rendering of `text` is at most max_w wide (shrinks down to ~55% before giving up)."""
    px = max(6, int(px)); f = _font(name, px); floor = max(6, int(px * 0.55))
    try:
        while px > floor and f.getlength(text) > max_w:
            px -= 1; f = _font(name, px)
    except AttributeError:
        pass
    return f


def fit_label(name, px, text, max_w):
    """Like fit_font, but if even the smallest size is too wide the text is cut and ends with an ellipsis. Returns (font, text)."""
    f = fit_font(name, px, text, max_w)
    try:
        if f.getlength(text) <= max_w: return f, text
        while len(text) > 1:
            text = text[:-1]
            if f.getlength(text + "…") <= max_w: return f, text + "…"
    except AttributeError:
        pass
    return f, text


def light_taskbar():
    try:
        import winreg
        k = winreg.OpenKey(winreg.HKEY_CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        return winreg.QueryValueEx(k, "SystemUsesLightTheme")[0] == 1
    except Exception:
        return False


def is_light(cfg):
    """Text colour theme: follow Windows, or force light/dark text."""
    t = cfg.get("widget_theme", "auto")
    return light_taskbar() if t == "auto" else t == "light"


def _num(cfg, key, default, lo, hi):
    try: return min(hi, max(lo, float(cfg.get(key, default))))
    except (TypeError, ValueError): return float(default)


def font_scale(cfg): return _num(cfg, "widget_font", 100, 80, 150) / 100
def opacity_alpha(cfg): return int(255 * _num(cfg, "widget_opacity", 100, 30, 100) / 100)


def cell_fill(cfg, light, hovered):
    """Background of one account cell: invisible by default, a readable plate if enabled, plus a hover highlight."""
    if cfg.get("widget_plate"):
        r, g, b, a = (248, 248, 248, 214) if light else (22, 22, 24, 208)
        return (max(0, min(255, r + (-14 if light else 26) * hovered)), max(0, min(255, g + (-14 if light else 26) * hovered)),
                max(0, min(255, b + (-14 if light else 26) * hovered)), a)
    return ((0, 0, 0, 20) if light else (255, 255, 255, 26)) if hovered else (0, 0, 0, 2)


def sort_profiles(cfg, items):
    """Keep config order, or show the accounts closest to their limit first (so they survive a '+N' cut)."""
    if cfg.get("widget_sort") != "usage":
        return items
    return sorted(items, key=lambda it: -(max(it[1]["five"], it[1]["week"]) if "five" in it[1] else -1))


def visible_profiles(cfg, cache):
    hidden = set(cfg.get("widget_hidden") or [])
    return sort_profiles(cfg, [(n, p) for n, p in ((cache or {}).get("profiles") or {}).items() if n not in hidden])


MORE = "__more__"


def _more(cv, cx, cy, n, fg, dim, px, maxw):
    """The '+N more' cell shown when not every account fits."""
    t = f"+{n}"
    put_text(cv, (cx, cy - 2 * px), t, fit_font("segoeuib.ttf", 13 * px, t, maxw), fg, "mm")
    put_text(cv, (cx, cy + 9 * px), "khác", fit_font("segoeui.ttf", 7.4 * px, "khác", maxw), dim, "mm")


def stack_limit(cfg, vertical, docked, screen):
    """How many pixels the widget may take along its stacking axis."""
    try:
        explicit = int(cfg.get("widget_max_len") or 0)
    except (TypeError, ValueError):
        explicit = 0
    if explicit > 0:
        return explicit
    span = screen[1] if vertical else screen[0]
    return int(span * (0.5 if docked else 0.9))


def plan_layout(cfg, items, scale, vertical, bar_thick, max_len, measure=None):
    """Decide how to fit many accounts: (size, shown) = the font/ring size to use (shrunk down to 60% if needed) and how many
    accounts to show; when not all fit the last slot becomes a '+N' cell. `measure(size)` = length of one cell (px)."""
    n = len(items)
    size = float(cfg["widget_size"])
    if n == 0:
        return size, 0
    if measure is None:
        longest = max(items, key=lambda it: len(core.alias(it[0], cfg, LABEL_MAX["rings"])))
        def measure(sz):
            r = render(cfg, {"profiles": {longest[0]: longest[1]}}, scale, -1, vertical, bar_thick, sz)
            return r[4] if vertical else r[3]
    try:
        cap = int(cfg.get("widget_max_cells") or 0)
    except (TypeError, ValueError):
        cap = 0
    want = min(n, cap) if cap > 0 else n                     # accounts we would like to show
    need = want + 1 if want < n else n                        # ...plus the '+N' cell when some are left out
    floor = max(20.0, size * 0.6)
    sz = size
    if measure(sz) * need > max_len:
        for _ in range(3):                                    # linear-ish in size: a couple of corrections is enough
            sz = max(floor, sz * max_len / (measure(sz) * need) * 0.97)
            if sz <= floor + 0.01 or measure(sz) * need <= max_len: break
    slots = max(1, int(max_len // measure(sz)))
    if need <= slots:
        return sz, want
    return sz, max(1, slots - 1)                              # still too many even at the smallest size: cut, keep a slot for '+N'


def apply_plan(items, shown):
    """Profiles to draw: the first `shown`, plus a '+N' cell for the rest."""
    head = dict(items[:shown])
    if shown < len(items):
        head[MORE] = {"more": len(items) - shown}
    return head


def render_more_tip(hidden, cfg, scale):
    """Hover card of the '+N' cell: one compact line per account that did not fit."""
    light, fs = is_light(cfg), max(1.0, font_scale(cfg))
    fg = (28, 28, 30) if light else (238, 238, 238)
    dim = (100, 100, 105) if light else (150, 150, 155)
    card, edge = ((250, 250, 251, 248), (0, 0, 0, 50)) if light else ((24, 24, 27, 246), (255, 255, 255, 46))
    W, pad, row = 250, 12, 15
    shown = hidden[:18]
    H = pad + 18 + row * len(shown) + (row if len(hidden) > len(shown) else 0) + pad - 2
    k = scale * SS
    img = Image.new("RGBA", (int(W * k), int(H * k)), (0, 0, 0, 0))
    ImageDraw.Draw(img).rounded_rectangle((1, 1, W * k - 2, H * k - 2), radius=int(9 * k), fill=card, outline=edge, width=max(1, int(k)))
    cv = Image.new("RGBA", img.size, (0, 0, 0, 0))
    def font(bold, px): return _font("segoeuib.ttf" if bold else "segoeui.ttf", max(6, int(px * k * fs)))
    def T(x, y, s, f, col, anchor="lm"): put_text(cv, (x * k, y * k), s, f, col, anchor)
    y = pad + 6
    T(pad, y, f"{len(hidden)} account nữa (không đủ chỗ)", font(True, 10.5), fg); y += 18
    for name, p in shown:
        label = name if len(name) <= 19 else name[:18] + "…"
        T(pad, y, label, font(False, 9), dim)
        if "five" in p:
            T(W - pad, y, f"5h {p['five']:.0f}% · 7d {p['week']:.0f}%", font(True, 9), pct_color(max(p["five"], p["week"]), cfg), "rm")
        else:
            T(W - pad, y, "chưa có dữ liệu", font(False, 8.6), dim, "rm")
        y += row
    if len(hidden) > len(shown):
        T(pad, y, f"… và {len(hidden) - len(shown)} account nữa", font(False, 8.6), dim)
    img.alpha_composite(cv)
    return img.resize((max(1, round(W * scale)), max(1, round(H * scale))), Image.LANCZOS)


def pct_color(p, cfg):
    return (224, 108, 117) if p >= cfg["block_pct"] else (245, 167, 66) if p >= cfg["warn_pct"] else (127, 216, 143)


def put_text(canvas, xy, s, font, color, anchor="la"):
    mask = Image.new("L", canvas.size, 0)
    ImageDraw.Draw(mask).text(xy, s, fill=255, font=font, anchor=anchor)
    layer = Image.new("RGBA", canvas.size, color + (255,))
    layer.putalpha(mask)
    canvas.alpha_composite(layer)


STYLES = ("rings", "bars", "cards", "segments", "arc")
STYLE_NAMES = {"rings": "Vòng tròn", "bars": "Thanh ngang", "cards": "Thẻ số lớn", "segments": "Ô vuông (terminal)", "arc": "Đồng hồ bán nguyệt"}
LABEL_MAX = {"rings": 8, "bars": 8, "segments": 8, "cards": 8, "arc": 10}      # longest label shown per style
BLUE, AMBER = (97, 175, 239), (245, 167, 66)


def _capsule(d, x, y, w, h, pct, col, track, alpha):
    d.rounded_rectangle((x, y, x + w, y + h), radius=h / 2, fill=track)
    if pct > 0:
        d.rounded_rectangle((x, y, x + max(h, w * min(pct, 100) / 100), y + h), radius=h / 2, fill=col + (alpha,))


def _segments(d, x, y, w, h, pct, col, track, alpha, n=10):
    gap = h * 0.28
    sw = (w - gap * (n - 1)) / n
    filled = 0 if pct <= 0 else max(1, round(min(pct, 100) / 100 * n))
    for k in range(n):
        xs = x + k * (sw + gap)
        d.rounded_rectangle((xs, y, xs + sw, y + h), radius=min(sw, h) * 0.3, fill=col + (alpha,) if k < filled else track)


def shows(cfg):
    """Which pieces of information the widget displays (always keeps at least something visible)."""
    master = bool(cfg.get("widget_reset", True))
    s = {"label": bool(cfg.get("widget_show_label", True)), "s5": bool(cfg.get("widget_show_5h", True)),
         "s7": bool(cfg.get("widget_show_7d", True)),
         "r5": master and bool(cfg.get("widget_show_reset5", True)), "r7": master and bool(cfg.get("widget_show_reset7", True))}
    if not (s["label"] or s["s5"] or s["s7"]):
        s["label"] = True
    return s


def metrics(cfg):
    """Visible usage windows in display order: [(tag, usage key, reset key)]."""
    s = shows(cfg)
    return [m for m, on in ((("5h", "five", "five_reset"), s["s5"]), (("7d", "week", "week_reset"), s["s7"])) if on]


def reset_lines(cfg):
    s = shows(cfg)
    return [m for m, on in ((("5h", "five_reset"), s["r5"]), (("7d", "week_reset"), s["r7"])) if on]


def render_cards(cfg, cache, scale, hover, vertical, bar_thick, size, phase, style):
    """Card-style cells (bars / cards / segments / arc). Same card in both layouts; stacked or side by side."""
    u = size / 36 * scale
    sh, ms, rs = shows(cfg), metrics(cfg), reset_lines(cfg)
    profs = visible_profiles(cfg, cache)
    busy = set((cache or {}).get("busy") or [])
    light, ff = is_light(cfg), font_scale(cfg)
    fg = (32, 32, 32) if light else (238, 238, 238)
    dim = (95, 95, 95) if light else (145, 145, 145)
    track = (0, 0, 0, 42) if light else (255, 255, 255, 38)
    nm = len(ms)
    CW = 44
    LAB = core.aliases([n for n, _ in profs if n != MORE], cfg, LABEL_MAX[style])
    if style == "cards": base_h = {2: 49, 1: 36, 0: 22}[nm]
    elif style == "arc": base_h = (40 if sh["label"] else 31) if nm else 22
    else: base_h = {2: 38, 1: 25, 0: 18}[nm]
    RES = 10 * len(rs)
    cw = max(CW * u, bar_thick - 4 * scale) if vertical else (CW + 4) * u
    ch = (base_h + RES + 4) * u
    n = max(len(profs), 1)
    W, H = int(cw if vertical else cw * n), int(ch * n if vertical else ch)
    cv = Image.new("RGBA", (W * SS, H * SS), (0, 0, 0, 0))
    d = ImageDraw.Draw(cv)
    k = u * SS
    B = lambda text, px, mw: fit_font("segoeuib.ttf", px * k * ff, text, mw * k)      # bold / regular text that never overflows mw units
    Rg = lambda text, px, mw: fit_font("segoeui.ttf", px * k * ff, text, mw * k)
    BL = lambda text, px, mw: fit_label("segoeuib.ttf", px * k * ff, text, mw * k)         # label: shrink, then cut with ...
    for i, (name, p) in enumerate(profs):
        x0, y0 = (0, i * ch * SS) if vertical else (i * cw * SS, 0)
        ox, oy = x0 + (cw / 2 - CW * u / 2) * SS, y0 + 2 * k
        P = lambda x, y: (ox + x * k, oy + y * k)
        ok, stale = "five" in p, bool(p.get("err"))
        a = 125 if stale else 255
        d.rounded_rectangle((x0 + 1, y0 + 1, x0 + cw * SS - 2, y0 + ch * SS - 2), radius=int(8 * k), fill=cell_fill(cfg, light, hover == i))
        if p.get("more"):
            _more(cv, ox + CW * k / 2, oy + (base_h + RES) * k / 2, p["more"], fg, dim, k * ff, CW * k * 0.9)
            continue
        alias = LAB[name]
        if not ok:
            msg = "chờ" if p.get("status") == 429 else "lỗi"
            fa, al = BL(alias, 7.8, CW - 6); put_text(cv, P(CW / 2, 11), al if sh["label"] else "", fa, dim, "mm")
            put_text(cv, P(CW / 2, 24), msg, Rg(msg, 7.6, CW - 6), (224, 108, 117), "mm")
            continue
        vals = [(tag, p[key], pct_color(p[key], cfg), f"{p[key]:.0f}%") for tag, key, _ in ms]
        v0 = vals[0] if vals else None
        if not vals:                                                  # label only
            fa, al = BL(alias, 9.5, CW - 8); put_text(cv, P(CW / 2, base_h / 2 + 1), al, fa, fg, "mm")
        elif style == "cards":                                        # glass card + status strip + big number
            tag, v, col, t = v0
            d.rounded_rectangle((ox, oy, ox + CW * k, oy + (base_h - 1) * k), radius=int(7 * k),
                                fill=(255, 255, 255, 14) if not light else (0, 0, 0, 10), outline=(255, 255, 255, 34) if not light else (0, 0, 0, 38), width=max(1, int(0.8 * k)))
            d.rounded_rectangle((ox + 2.5 * k, oy + 6 * k, ox + 5 * k, oy + (base_h - 7) * k), radius=int(1.2 * k), fill=col + (a,))
            if sh["label"]:
                fa, al = BL(alias, 7.8, 21); put_text(cv, P(10, 9), al, fa, fg, "lm")
            put_text(cv, P(CW - 4, 9), tag, Rg(tag, 7.6, 8), dim, "rm")
            put_text(cv, P(CW / 2 + 3, 23 if nm == 2 else 24), t, B(t, 15, CW - 16), col if not stale else dim, "mm")
            if nm == 2:
                tag2, v2, col2, t2 = vals[1]
                _capsule(d, *P(10, 33), (CW - 16) * k, 3 * k, v2, col2, track, a)
                put_text(cv, P(CW / 2 + 3, 41.5), f"{tag2} {t2}", Rg(f"{tag2} {t2}", 7.6, CW - 12), col2 if not stale else dim, "mm")
        elif style == "arc":                                          # semicircle gauge: outer = first metric, inner = second
            cx, cy = P(CW / 2, 25 if sh["label"] else 22)
            specs = ((17.5, 4, vals[0]), (11.5, 2.6, vals[1])) if nm == 2 else ((16, 5, vals[0]),)
            for r, th, (tag, v, col, t) in specs:
                R, T = r * k, th * k
                box = (cx - R, cy - R, cx + R, cy + R)
                d.arc(box, 180, 360, fill=track, width=int(T))
                if v > 0: d.arc(box, 180, 180 + 180 * min(v, 100) / 100, fill=col + (a,), width=int(T))
            num = v0[3][:-1]
            put_text(cv, (cx, cy - 3.2 * k), num, B(num, 11, 17), fg, "mm")
            if sh["label"]:
                fa, al = BL(alias, 7.8, CW - 2); put_text(cv, (cx, cy + 8.4 * k), al, fa, dim, "mm")
            elif nm == 1:
                put_text(cv, (cx, cy + 6 * k), v0[0], Rg(v0[0], 7.6, 12), dim, "mm")
        else:                                                         # bars / segments: lead + first % on top, second row under
            bar = _capsule if style == "bars" else _segments
            tag, v, col, t = v0
            right = t if (tag == "5h" or not sh["label"]) else f"{tag} {t}"
            fp = B(right, 7.8, 22 if right != t else 16)
            put_text(cv, P(CW - 5, 9), right, fp, col if not stale else dim, "rm")
            lead = alias if sh["label"] else tag
            fa, al = BL(lead, 7.8, max(8, CW - 10 - fp.getlength(right) / k - 2.5)); put_text(cv, P(5, 9), al, fa, fg if sh["label"] else dim, "lm")
            bar(d, *P(5, 14.5), (CW - 10) * k, (5 if style == "bars" else 6) * k, v, col, track, a)
            if nm == 2:
                tag2, v2, col2, t2 = vals[1]
                put_text(cv, P(5, 26.5), tag2, Rg(tag2, 7.6, 10), dim, "lm"); put_text(cv, P(CW - 5, 26.5), t2, Rg(t2, 7.6, 16), col2 if not stale else dim, "rm")
                bar(d, *P(5, 31), (CW - 10) * k, (3.4 if style == "bars" else 4) * k, v2, col2, track, a)
        mx, my = P(CW - 4.2, 3.6)                                     # corner marker (drawn last so the card background can't cover it): spinner while fetching, amber dot when stale
        if name in busy:
            R = 3.3 * k; d.ellipse((mx - R, my - R, mx + R, my + R), outline=BLUE + (70,), width=max(1, int(1.1 * k)))
            d.arc((mx - R, my - R, mx + R, my + R), phase * 30, phase * 30 + 110, fill=BLUE + (255,), width=max(2, int(1.8 * k)))
        elif stale:
            R = 2.2 * k; d.ellipse((mx - R, my - R, mx + R, my + R), fill=AMBER + (255,))
        for ln, (lab, rk) in enumerate(rs):
            txt = f"{lab} {core.fmt_reset(p.get(rk))}"
            put_text(cv, P(CW / 2, base_h + 5 + ln * 9.6), txt, Rg(txt, 7.6, CW - 2), fg if ln == 0 else dim, "mm")
    return cv, W, H, cw, ch


def render(cfg, cache, scale, hover, vertical, bar_thick, size, phase=0):
    """Draw the cells. Returns (supersampled RGBA canvas, W, H, cell_w, cell_h) with W/H in physical px."""
    style = cfg.get("widget_style", "rings")
    if style in STYLES and style != "rings":
        return render_cards(cfg, cache, scale, hover, vertical, bar_thick, size, phase, style)
    u = size / 36 * scale                            # 1 "unit"
    D, pad = 34 * u, 5 * u
    sh, ms, rs = shows(cfg), metrics(cfg), reset_lines(cfg)
    profs = visible_profiles(cfg, cache)
    busy = set((cache or {}).get("busy") or [])
    light, ff = is_light(cfg), font_scale(cfg)
    fg = (32, 32, 32) if light else (236, 236, 236)
    dim = (90, 90, 90) if light else (150, 150, 150)
    track = (0, 0, 0, 38) if light else (255, 255, 255, 46)
    labels = core.aliases([n for n, _ in profs if n != MORE], cfg, LABEL_MAX["rings"])
    below = sh["label"] and bool(ms) and any(len(v) > 3 for v in labels.values())   # long label: % inside the ring, label below / beside
    nlines = len(rs) + (1 if below else 0)
    LBL = 10.5 * u if below else 0
    if vertical:   # stacked cells, ring on top, (label,) reset lines underneath
        cw = max(bar_thick - 4 * scale, D + 2 * pad)
        ch = pad + D + LBL + (len(rs) * 10.5 * u + 3 * u if rs else 0) + pad
    else:          # side by side, text to the right of the ring
        cw = pad + D + (6 * u + 47 * u if nlines else 0) + pad
        ch = D + 8 * u
    n = max(len(profs), 1)
    W, H = int(cw if vertical else cw * n), int(ch * n if vertical else ch)
    cv = Image.new("RGBA", (W * SS, H * SS), (0, 0, 0, 0))
    d = ImageDraw.Draw(cv)
    S_ = u * SS
    FA = lambda t: fit_font("segoeuib.ttf", 7.5 * S_ * ff, t, 17.5 * S_)                          # text inside the ring
    FB = lambda t, mw: fit_label("segoeuib.ttf", 7.8 * S_ * ff, t, mw * S_)                       # label (bold, truncated with ...)
    FL = lambda t, mw: fit_font("segoeui.ttf", (8 if vertical else 9) * S_ * ff, t, mw * S_)      # reset lines
    for i, (name, p) in enumerate(profs):
        x0, y0 = (0, i * ch * SS) if vertical else (i * cw * SS, 0)
        d.rounded_rectangle((x0 + 1, y0 + 1, x0 + cw * SS - 2, y0 + ch * SS - 2), radius=int(8 * u * SS), fill=cell_fill(cfg, light, hover == i))
        cx = x0 + (cw / 2 if vertical else pad + D / 2) * SS
        cy = y0 + ((pad + D / 2) if vertical else ch / 2) * SS
        if p.get("more"):
            R, T = 17 * S_, 3.6 * S_
            d.ellipse((cx - R, cy - R, cx + R, cy + R), outline=track, width=int(T))
            _more(cv, cx, cy, p["more"], fg, dim, S_ * ff * 0.9, 24 * S_)
            continue
        ok, stale = "five" in p, bool(p.get("err"))
        a = 120 if stale else 255                    # stale (e.g. 429): last known value, drawn dimmer
        specs = ((17, 3.6, ms[0] if ms else None), (11.6, 3.6, ms[1] if len(ms) > 1 else None)) if len(ms) != 1 else ((15.5, 5.2, ms[0]),)
        for r_out, th, m in specs:
            R, T = r_out * S_, th * S_
            box = (cx - R, cy - R, cx + R, cy + R)
            d.ellipse(box, outline=track, width=int(T))
            if ok and m:
                v = min(p[m[1]], 100)
                col = pct_color(p[m[1]], cfg) + (a,)
                if v >= 99.5:
                    d.ellipse(box, outline=col, width=int(T))
                elif v > 0:
                    d.arc(box, -90, -90 + 360 * v / 100, fill=col, width=int(T))
        if name in busy:                                  # spinner: a rotating blue arc around the outer ring = "fetching"
            R, T = 20.2 * u * SS, 2.4 * u * SS
            d.arc((cx - R, cy - R, cx + R, cy + R), phase * 30, phase * 30 + 90, fill=(97, 175, 239, 255), width=int(T))
        if not ok: inside = "!"
        elif sh["label"] and not below: inside = labels[name]
        elif ms: inside = f"{p[ms[0][1]]:.0f}"
        else: inside = labels[name]
        put_text(cv, (cx, cy), inside, FA(inside), fg if ok else (224, 108, 117), "mm")
        if stale and ok:                             # small amber dot = "showing cached value"
            r, ex, ey = 3 * u * SS, cx + 12 * u * SS, cy - 14 * u * SS
            d.ellipse((ex - r, ey - r, ex + r, ey + r), fill=(245, 167, 66, 255))
        tx = x0 + (pad + D + 6 * u) * SS                  # text column (horizontal layout)
        ys = [cy + (j - (nlines - 1) / 2) * 10.5 * u * SS for j in range(nlines)]   # centred stack of lines (horizontal layout)
        if below and ok:
            f, t = FB(labels[name], (cw / u - 4) if vertical else 46)
            put_text(cv, (cx, cy + (D / 2 + 7 * u) * SS) if vertical else (tx, ys[0]), t, f, fg, "mm" if vertical else "lm")
        if not ok:
            msg = "chờ" if p.get("status") == 429 else "lỗi"
            pos = (cx, cy + (D / 2 + 9 * u + LBL) * SS) if vertical else (tx, cy)
            if rs or not vertical: put_text(cv, pos, msg, FL(msg, 30), (224, 108, 117), "mm" if vertical else "lm")
            continue
        for ln, (lab, rk) in enumerate(rs):
            t = core.fmt_reset(p.get(rk))
            if vertical:
                y = cy + (D / 2 + 8 * u + LBL + ln * 10.5 * u) * SS
                put_text(cv, (cx, y), f"{lab} {t}", FL(f"{lab} {t}", cw / u - 4), fg if ln == 0 else dim, "mm")
            else:
                y = ys[ln + (1 if below else 0)]
                put_text(cv, (tx, y), lab, FL(lab, 11), dim, "lm")
                put_text(cv, (tx + 12 * u * SS, y), t, FL(t, 34), fg, "lm")
    return cv, W, H, cw, ch


def compose(cv, W, H, scale, edit, angle):
    """Add the edit overlay (frame + handles) around the cells, rotate, downscale.
    Returns (final RGBA, geo) where geo describes the unrotated padded frame for hit-testing."""
    M, Mt = (int(22 * scale), int(42 * scale)) if edit else (0, 0)
    Wp, Hp = W + 2 * M, H + M + Mt
    canvas = Image.new("RGBA", (Wp * SS, Hp * SS), (0, 0, 0, 0))
    canvas.paste(cv, (M * SS, Mt * SS))
    geo = {"M": M, "Mt": Mt, "W": W, "H": H, "Wp": Wp, "Hp": Hp,
           "rot": (M + W / 2, Mt - 26 * scale), "size": (M + W, Mt + H), "lock": (M + 11 * scale, Mt + H + 13 * scale)}
    if edit:
        ov = Image.new("RGBA", canvas.size, (0, 0, 0, 0))     # separate layer: ImageDraw overwrites, it does not blend
        d = ImageDraw.Draw(ov)
        s = scale * SS
        x0, y0, x1, y1 = M * SS, Mt * SS, (M + W) * SS, (Mt + H) * SS
        lw = max(2, int(1.6 * s))
        d.rounded_rectangle((x0 - 3 * s, y0 - 3 * s, x1 + 3 * s, y1 + 3 * s), radius=int(10 * s), fill=ACCENT + (22,),
                            outline=ACCENT + (210,), width=lw)
        rx, ry = geo["rot"][0] * SS, geo["rot"][1] * SS
        d.line((rx, ry, rx, y0 - 3 * s), fill=ACCENT + (210,), width=lw)
        d.ellipse((rx - 7 * s, ry - 7 * s, rx + 7 * s, ry + 7 * s), fill=ACCENT + (255,), outline=(20, 20, 20, 255), width=int(s))
        d.arc((rx - 3.2 * s, ry - 3.2 * s, rx + 3.2 * s, ry + 3.2 * s), 30, 300, fill=(20, 20, 20, 255), width=int(1.5 * s))
        sx, sy = geo["size"][0] * SS, geo["size"][1] * SS
        d.rectangle((sx - 6 * s, sy - 6 * s, sx + 6 * s, sy + 6 * s), fill=ACCENT + (255,), outline=(20, 20, 20, 255), width=int(s))
        lx, ly = geo["lock"][0] * SS, geo["lock"][1] * SS
        d.ellipse((lx - 10 * s, ly - 10 * s, lx + 10 * s, ly + 10 * s), fill=(24, 24, 24, 245), outline=ACCENT + (255,), width=lw)
        d.rectangle((lx - 4.2 * s, ly - 0.8 * s, lx + 4.2 * s, ly + 4.6 * s), fill=ACCENT + (255,))                      # padlock body
        d.arc((lx - 3 * s, ly - 5.4 * s, lx + 3 * s, ly + 1.4 * s), 180, 360, fill=ACCENT + (255,), width=int(1.4 * s))  # shackle
        canvas.alpha_composite(ov)
    if angle:
        canvas = canvas.rotate(-angle, resample=Image.BICUBIC, expand=True)   # PIL is CCW-positive; we use clockwise
    final = canvas.resize((max(1, round(canvas.width / SS)), max(1, round(canvas.height / SS))), Image.LANCZOS)
    return final, geo


VI_DAYS = ("T2", "T3", "T4", "T5", "T6", "T7", "CN")


def fmt_when(iso):
    """Absolute local time of a reset: '02:20', '02:20 ngày mai', '07:00 T6 13/11'."""
    ts = core.iso_ts(iso)
    if ts is None:
        return "-"
    dt, now = datetime.fromtimestamp(ts), datetime.now()
    if dt.date() == now.date(): day = ""
    elif dt.date() == (now + timedelta(days=1)).date(): day = " ngày mai"
    else: day = f" {VI_DAYS[dt.weekday()]} {dt.day}/{dt.month}"
    return dt.strftime("%H:%M") + day


def ago_text(ts):
    if not ts:
        return "chưa có"
    s = max(0, time.time() - ts)
    return f"{s:.0f}s trước" if s < 90 else f"{s / 60:.0f} phút trước" if s < 5400 else f"{s / 3600:.0f} giờ trước"


def money(v, cur):
    sym = {"USD": "$", "EUR": "€", "GBP": "£"}.get(cur)
    return f"{sym}{v:,.2f}" if sym else f"{v:,.2f} {cur}"


def render_tip(name, p, cfg, scale):
    """Detail card shown when hovering an account: every usage figure we know for it."""
    light, fs = is_light(cfg), max(1.0, font_scale(cfg))
    fg = (28, 28, 30) if light else (238, 238, 238)
    dim = (100, 100, 105) if light else (150, 150, 155)
    track = (0, 0, 0, 40) if light else (255, 255, 255, 40)
    card, edge = ((250, 250, 251, 248), (0, 0, 0, 50)) if light else ((24, 24, 27, 246), (255, 255, 255, 46))
    W, pad = 250, 12
    k = scale * SS
    cv = Image.new("RGBA", (int(W * k), int(900 * k)), (0, 0, 0, 0))
    d = ImageDraw.Draw(cv)
    def font(bold, px): return _font("segoeuib.ttf" if bold else "segoeui.ttf", max(6, int(px * k * fs)))
    def T(x, y, s, f, col, anchor="lm"): put_text(cv, (x * k, y * k), s, f, col, anchor)
    y = pad + 6
    title = name if len(name) <= 24 else name[:23] + "…"
    T(pad, y, title, font(True, 11.5), fg)
    plan = p.get("plan")
    if plan: T(W - pad, y, str(plan), font(False, 8.5), dim, "rm")
    y += 18
    if "five" in p:
        for label, key, rk in (("5 giờ", "five", "five_reset"), ("7 ngày", "week", "week_reset")):
            v = p[key]; col = pct_color(v, cfg)
            T(pad, y, label, font(False, 9), dim); T(W - pad, y, f"{v:.0f}%", font(True, 10.5), col, "rm"); y += 10
            _capsule(d, pad * k, y * k, (W - 2 * pad) * k, 5 * k, v, col, track, 255); y += 11
            rs = p.get(rk)
            T(pad, y, f"reset sau {core.fmt_reset(rs)} · {fmt_when(rs)}" if rs else "chưa có mốc reset", font(False, 8.4), dim); y += 15
        for sc in p.get("scoped") or []:
            v = sc.get("pct") or 0; col = pct_color(v, cfg)
            T(pad, y, f"{sc['name']} (riêng, 7 ngày)", font(False, 9), dim); T(W - pad, y, f"{v:.0f}%", font(True, 10), col, "rm"); y += 10
            _capsule(d, pad * k, y * k, (W - 2 * pad) * k, 3.6 * k, v, col, track, 255); y += 12
        ex = p.get("extra")
        if ex:
            v = ex.get("pct") or 0; col = pct_color(v, cfg)
            T(pad, y, "Credit dùng thêm (tháng)", font(False, 9), dim)
            T(W - pad, y, f"{money(ex['used'], ex['currency'])} / {money(ex['limit'], ex['currency'])}", font(True, 9.6), fg, "rm"); y += 10
            _capsule(d, pad * k, y * k, (W - 2 * pad) * k, 3.6 * k, v, col, track, 255); y += 10
            T(pad, y, f"đã dùng {v:.1f}% hạn mức credit", font(False, 8.4), dim); y += 13
        else:
            T(pad, y, "Credit dùng thêm: chưa bật", font(False, 8.6), dim); y += 13
    else:
        T(pad, y, p.get("err") or "chưa có dữ liệu", font(False, 9.4), (224, 108, 117)); y += 15
    foot = None
    if p.get("err") and "five" in p:
        retry = p.get("retry_at")
        wait = f" · thử lại sau {max(0, retry - time.time()) / 60:.0f} phút" if retry else ""
        foot = (f"{p['err']}: đang hiện dữ liệu cũ ({ago_text(p.get('fetched_at'))}){wait}", AMBER)
    elif p.get("fetched_at"):
        foot = (f"Cập nhật {ago_text(p['fetched_at'])}", dim)
    if foot:
        y += 2
        d.line((pad * k, y * k, (W - pad) * k, y * k), fill=edge, width=max(1, int(0.8 * k))); y += 9
        T(pad, y, foot[0], font(False, 8.2), foot[1]); y += 12
    H = y + pad - 4
    img = Image.new("RGBA", (int(W * k), int(H * k)), (0, 0, 0, 0))
    ImageDraw.Draw(img).rounded_rectangle((1, 1, W * k - 2, H * k - 2), radius=int(9 * k), fill=card, outline=edge, width=max(1, int(1 * k)))
    img.alpha_composite(cv.crop((0, 0, img.width, img.height)))
    return img.resize((max(1, round(W * scale)), max(1, round(H * scale))), Image.LANCZOS)


def tip_position(win, mouse_y, size, screen, gap=8):
    """Place the detail card beside the widget (on the side with more room), level with the pointer, kept on screen."""
    (wx, wy, ww, wh), (tw, th), (sw, sh) = win, size, screen
    x = wx - tw - gap if wx + ww / 2 > sw / 2 else wx + ww + gap
    x = max(gap, min(x, sw - tw - gap))
    y = max(gap, min(mouse_y - th / 2, sh - th - gap))
    return int(x), int(y)


class Widget(threading.Thread):
    def __init__(self, on_open, on_refresh, on_quit):
        super().__init__(daemon=True)
        self.cb = (on_open, on_refresh, on_quit)
        self.hwnd, self.hover, self.sig, self.cfg, self.cfg_m = None, -1, None, None, 0
        self.visible, self.live, self.drag, self.last_hit, self.down_hit = False, None, None, None, None
        self.geo, self.img, self.cell_w, self.cell_h, self.vertical = None, None, 1, 1, True
        self.org, self.scale, self.pos = (0, 0), 1.0, None
        self.phase, self.fast = 0, False
        self.tip, self.tip_visible, self.tip_key, self.tip_img, self.tip_pos = None, False, None, None, None
        self.hover_t, self.mouse_pt = 0.0, (0, 0)
        self.plan, self.plan_key, self.view, self.view_hidden = None, None, [], []

    # ---- state (config + live values while dragging) ----
    def st(self):
        c, l = self.cfg, self.live or {}
        return {"size": l.get("size", c["widget_size"]), "angle": l.get("angle", c["widget_angle"] or 0.0),
                "cx": l.get("cx", c["widget_cx"]), "cy": l.get("cy", c["widget_cy"]), "edit": not c["widget_locked"]}

    def save_state(self, **kw):
        cfg = core.load_config()
        for k, v in kw.items(): cfg["widget_" + k] = v
        core.save_config(cfg)
        self.cfg = cfg
        try: self.cfg_m = core.CONFIG.stat().st_mtime
        except OSError: pass

    # ---- geometry ----
    def taskbar(self):
        """(rect, tray_rect|None, vertical) of the primary taskbar, or None if hidden."""
        tb = user32.FindWindowW("Shell_TrayWnd", None)
        if not tb or not user32.IsWindowVisible(tb):
            return None
        r = wt.RECT(); user32.GetWindowRect(tb, ctypes.byref(r))
        tray = user32.FindWindowExW(tb, None, "TrayNotifyWnd", None)
        tr = wt.RECT()
        ok = tray and user32.GetWindowRect(tray, ctypes.byref(tr)) and (tr.right > tr.left) and (tr.bottom > tr.top)
        return r, (tr if ok else None), (r.right - r.left) < (r.bottom - r.top)

    def auto_center(self, tbi, w, h):
        """Content centre when auto-anchored next to the tray."""
        r, tr, vertical = tbi
        cfg, s = self.cfg, self.scale
        if vertical:
            x = r.left + ((r.right - r.left) - w) // 2
            y = (tr.top if tr else r.bottom - int(220 * s)) - h - int(8 * s) if cfg["widget_anchor"] == "tray" else r.top + int(8 * s)
        else:
            y = r.top + ((r.bottom - r.top) - h) // 2
            x = (tr.left if tr else r.right - int(260 * s)) - w - int(8 * s) if cfg["widget_anchor"] == "tray" else r.left + int(8 * s)
        return x + cfg["widget_offset_x"] + w / 2, y + cfg["widget_offset_y"] + h / 2

    def fullscreen_busy(self):
        st = ctypes.c_int()
        return shell32.SHQueryUserNotificationState(ctypes.byref(st)) == 0 and st.value in (2, 3, 4)

    @staticmethod
    def rot(x, y, ang):
        a = math.radians(ang)
        return x * math.cos(a) - y * math.sin(a), x * math.sin(a) + y * math.cos(a)   # clockwise on screen (y down)

    def win_topleft(self, ccx, ccy):
        """Window top-left such that the *content* centre sits at (ccx, ccy), whatever the rotation/padding."""
        g = self.geo
        dx, dy = self.rot(g["M"] + g["W"] / 2 - g["Wp"] / 2, g["Mt"] + g["H"] / 2 - g["Hp"] / 2, self.built_angle)
        return round(ccx - dx - self.img.width / 2), round(ccy - dy - self.img.height / 2)

    # ---- drawing ----
    def blit(self, hwnd, img, x, y, alpha):
        """Paint a per-pixel-alpha image into a layered window at (x, y)."""
        w, h = img.size
        r, g, b, a = img.convert("RGBa").split()
        data = Image.merge("RGBA", (b, g, r, a)).tobytes()
        bi = BIH(ctypes.sizeof(BIH), w, -h, 1, 32, 0, 0, 0, 0, 0, 0)
        screen = user32.GetDC(None)
        mem = gdi32.CreateCompatibleDC(screen)
        bits = ctypes.c_void_p()
        bmp = gdi32.CreateDIBSection(screen, ctypes.byref(bi), 0, ctypes.byref(bits), None, 0)
        ctypes.memmove(bits, data, len(data))
        old = gdi32.SelectObject(mem, bmp)
        pt, sz, src = wt.POINT(x, y), wt.SIZE(w, h), wt.POINT(0, 0)
        user32.UpdateLayeredWindow(hwnd, screen, ctypes.byref(pt), ctypes.byref(sz), mem, ctypes.byref(src),
                                   0, ctypes.byref(BLEND(0, 0, alpha, 1)), 2)
        gdi32.SelectObject(mem, old); gdi32.DeleteObject(bmp); gdi32.DeleteDC(mem); user32.ReleaseDC(None, screen)

    def present(self, x, y):
        self.blit(self.hwnd, self.img, x, y, opacity_alpha(self.cfg))
        self.org, self.pos = (x, y), (x, y)

    # ---- hover detail card ----
    def hide_tip(self):
        if self.tip_visible and self.tip:
            user32.ShowWindow(self.tip, 0)
        self.tip_visible, self.tip_key, self.tip_pos = False, None, None

    def update_tip(self, cache):
        c = self.cfg
        profs = self.view                      # exactly what is drawn (same order, incl. the '+N' cell)
        if not (self.tip and c.get("widget_tooltip", True) and not self.drag and self.visible and 0 <= self.hover < len(profs)
                and time.time() - self.hover_t >= 0.3):
            return self.hide_tip()
        name, p = profs[self.hover]
        key = (name, cache.get("ts"), int(time.time() // 5), self.scale, c.get("widget_theme"), c.get("widget_font"), c.get("warn_pct"), c.get("block_pct"))
        if key != self.tip_key:
            self.tip_img = render_more_tip(self.view_hidden, c, self.scale) if name == MORE else render_tip(name, p, c, self.scale)
            self.tip_key, self.tip_pos = key, None
        pos = tip_position((self.org[0], self.org[1], self.img.width, self.img.height), self.mouse_pt[1], self.tip_img.size,
                           (user32.GetSystemMetrics(0), user32.GetSystemMetrics(1)))
        if pos != self.tip_pos or not self.tip_visible:
            self.blit(self.tip, self.tip_img, pos[0], pos[1], 255)
            self.tip_pos = pos
        user32.SetWindowPos(self.tip, wt.HWND(-1), 0, 0, 0, 0, 0x2 | 0x1 | 0x10 | 0x40)
        self.tip_visible = True

    def build(self, cache, tbi):
        s, c = self.st(), self.cfg
        layout = c.get("widget_layout", "auto")
        vertical = (tbi[2] if tbi else True) if layout == "auto" else layout == "vertical"
        thick = (tbi[0].right - tbi[0].left) if (tbi and tbi[2] and vertical) else 0
        # many accounts: shrink / cut with "+N" so the widget never outgrows the screen
        items = visible_profiles(c, cache)
        limit = stack_limit(c, vertical, s["cx"] is None, (user32.GetSystemMetrics(0), user32.GetSystemMetrics(1)))
        key = (tuple(n for n, _ in items), tuple(core.aliases([n for n, _ in items], c, LABEL_MAX["rings"]).values()), round(s["size"], 1), self.scale, vertical, thick, limit,
               c.get("widget_style"), c.get("widget_font"), c.get("widget_label"), c.get("widget_reset"), c.get("widget_show_label"), c.get("widget_show_5h"),
               c.get("widget_show_7d"), c.get("widget_show_reset5"), c.get("widget_show_reset7"), c.get("widget_max_cells"))
        if key != self.plan_key:
            self.plan, self.plan_key = plan_layout(dict(c, widget_size=s["size"]), items, self.scale, vertical, thick, limit), key
        size, shown = self.plan
        view = apply_plan(items, shown)
        self.view, self.view_hidden = list(view.items()), items[shown:]
        view_cache = dict(cache or {}); view_cache["profiles"] = view
        cv, W, H, self.cell_w, self.cell_h = render(c, view_cache, self.scale, -1 if s["edit"] else self.hover, vertical, thick, size, self.phase)
        self.img, self.geo = compose(cv, W, H, self.scale, s["edit"], s["angle"])
        self.vertical, self.built_angle = vertical, s["angle"]

    def content_center(self, tbi):
        s = self.st()
        if s["cx"] is not None and s["cy"] is not None:
            return s["cx"], s["cy"]
        return self.auto_center(tbi, self.geo["W"], self.geo["H"]) if tbi else None

    def set_fast(self, on):
        """Animate the 'fetching' spinner with a fast timer, only while something is being fetched."""
        if on and not self.fast: user32.SetTimer(self.hwnd, 2, 110, None); self.fast = True
        elif not on and self.fast: user32.KillTimer(self.hwnd, 2); self.fast = False

    def hide(self):
        self.hide_tip()
        if self.visible: user32.ShowWindow(self.hwnd, 0); self.visible = False

    def tick(self, force=False):
        try: m = core.CONFIG.stat().st_mtime if core.CONFIG.exists() else 0
        except OSError: m = 0
        if (m != self.cfg_m or self.cfg is None) and not self.drag:
            self.cfg, self.cfg_m, self.sig = core.load_config(), m, None
        c = self.cfg
        self.scale = ctypes.windll.user32.GetDpiForSystem() / 96
        if not c["widget_enabled"] or self.fullscreen_busy():
            return self.hide()
        cache, tbi, s = core.load_cache(), self.taskbar(), self.st()
        if tbi is None and (s["cx"] is None or s["cy"] is None):
            return self.hide()                                 # auto-anchored but no taskbar to anchor to
        busy = bool(cache and cache.get("busy"))
        self.phase = int(time.time() * 8) % 12 if busy else 0
        self.set_fast(busy or self.hover >= 0)
        sig = (cache and cache["ts"], int(time.time() // 20), self.phase, bool(busy), self.hover, self.scale, tbi and tbi[2],
               tbi and tbi[0].right - tbi[0].left, s["size"], s["angle"], s["edit"], c["widget_reset"],
               c["warn_pct"], c["block_pct"], c.get("widget_layout", "auto"), c.get("widget_style", "rings"), c.get("widget_label"), c.get("widget_font"), c.get("widget_opacity"), c.get("widget_theme"), c.get("widget_plate"), c.get("widget_reset"), c.get("widget_show_label"), c.get("widget_show_5h"), c.get("widget_show_7d"),
               c.get("widget_show_reset5"), c.get("widget_show_reset7"), c.get("widget_sort"), c.get("widget_max_cells"), c.get("widget_max_len"),
               tuple(c.get("widget_hidden") or ()), tuple(sorted((c.get("aliases") or {}).items())), light_taskbar())
        redrawn = force or sig != self.sig or self.img is None
        if redrawn:
            self.build(cache, tbi); self.sig = sig
        cc = self.content_center(tbi)
        if cc is None:
            return self.hide()
        pos = self.win_topleft(*cc)
        if redrawn or pos != self.pos or not self.visible:
            self.present(*pos)
        user32.SetWindowPos(self.hwnd, wt.HWND(-1), 0, 0, 0, 0, 0x2 | 0x1 | 0x10 | 0x40)  # topmost, nomove/size/noactivate, show
        self.visible = True
        self.update_tip(cache)

    # ---- hit testing (client px -> unrotated padded frame) ----
    def to_frame(self, x, y):
        g = self.geo
        ux, uy = self.rot(x - self.img.width / 2, y - self.img.height / 2, -self.built_angle)
        return ux + g["Wp"] / 2, uy + g["Hp"] / 2

    def hit(self, x, y):
        if not self.geo or self.img is None or self.cfg is None: return None
        g, s = self.geo, self.scale
        px, py = self.to_frame(x, y)
        if not self.cfg["widget_locked"]:
            for name, r in (("lock", 12), ("rot", 11), ("size", 11)):
                hx, hy = g[name]
                if (px - hx) ** 2 + (py - hy) ** 2 <= (r * s) ** 2: return name
        if g["M"] <= px <= g["M"] + g["W"] and g["Mt"] <= py <= g["Mt"] + g["H"]: return "body"
        return None

    # ---- mouse / drag ----
    def mouse_screen(self, lp):
        return (self.org[0] + ctypes.c_short(lp & 0xFFFF).value, self.org[1] + ctypes.c_short((lp >> 16) & 0xFFFF).value)

    def begin_drag(self, kind, sx, sy):
        s = self.st()
        cc = self.content_center(self.taskbar()) or (sx, sy)
        self.drag = {"k": kind, "sx": sx, "sy": sy, "cx0": cc[0], "cy0": cc[1], "s0": s["size"], "moved": False,
                     "d0": max(math.hypot(sx - cc[0], sy - cc[1]), 1.0)}
        self.live = {"cx": cc[0], "cy": cc[1]}
        user32.SetCapture(self.hwnd)

    def do_drag(self, sx, sy):
        d = self.drag
        if abs(sx - d["sx"]) + abs(sy - d["sy"]) > 2: d["moved"] = True
        if not d["moved"]: return
        if d["k"] == "body":
            self.live["cx"], self.live["cy"] = d["cx0"] + sx - d["sx"], d["cy0"] + sy - d["sy"]
            self.tick()
        elif d["k"] == "size":
            self.live["size"] = max(20.0, min(90.0, d["s0"] * math.hypot(sx - d["cx0"], sy - d["cy0"]) / d["d0"]))
            self.tick(force=True)
        elif d["k"] == "rot":
            ang = math.degrees(math.atan2(sx - d["cx0"], -(sy - d["cy0"])))
            for k in (-180, -90, 0, 90, 180):
                if abs(ang - k) < 5: ang = float(k)
            self.live["angle"] = 180.0 if ang <= -180 else round(ang, 1)
            self.tick(force=True)

    def end_drag(self):
        d, live, self.drag = self.drag, self.live, None
        user32.ReleaseCapture()
        if d["moved"] and live:
            self.save_state(**live)
        self.live, self.sig = None, None

    def wndproc(self, hwnd, msg, wp, lp):
        try:
            return self._wndproc(hwnd, msg, wp, lp)
        except Exception:
            return user32.DefWindowProcW(hwnd, msg, wp, lp)

    def _wndproc(self, hwnd, msg, wp, lp):
        if msg == 0x113:                                   # WM_TIMER (id 1 = slow tick, id 2 = spinner)
            if not self.drag: self.tick()
            return 0
        x, y = ctypes.c_short(lp & 0xFFFF).value, ctypes.c_short((lp >> 16) & 0xFFFF).value
        edit = self.cfg is not None and not self.cfg["widget_locked"]
        if msg == 0x201:                                   # WM_LBUTTONDOWN
            self.down_hit = h = self.hit(x, y)
            if edit and h in ("body", "rot", "size"):
                self.begin_drag(h, *self.mouse_screen(lp))
            return 0
        if msg == 0x200:                                   # WM_MOUSEMOVE
            if self.drag:
                self.do_drag(*self.mouse_screen(lp)); return 0
            self.last_hit = self.hit(x, y)
            if not edit and self.geo:
                px, py = self.to_frame(x, y)
                g = self.geo
                inside = g["M"] <= px <= g["M"] + g["W"] and g["Mt"] <= py <= g["Mt"] + g["H"]
                i = (int((py - g["Mt"]) / max(self.cell_h, 1)) if self.vertical else int((px - g["M"]) / max(self.cell_w, 1))) if inside else -1
                self.mouse_pt = self.mouse_screen(lp)
                if i != self.hover:
                    self.hover, self.hover_t = i, time.time(); self.tick()
            user32.TrackMouseEvent(ctypes.byref(TME(ctypes.sizeof(TME), 2, hwnd, 0)))
            return 0
        if msg == 0x2A3:                                   # WM_MOUSELEAVE
            if self.hover != -1: self.hover = -1; self.tick()
            return 0
        if msg == 0x202:                                   # WM_LBUTTONUP
            if self.drag:
                self.end_drag(); self.tick(force=True); return 0
            h = self.hit(x, y)
            if edit and h == "lock" and self.down_hit == "lock":
                self.save_state(locked=True); self.tick(force=True)
            elif not edit and h == "body":
                self.cb[0]()
            return 0
        if msg == 0x215:                                   # WM_CAPTURECHANGED: drag cancelled by the system
            if self.drag: self.drag = self.live = self.sig = None
            return 0
        if msg == 0x20:                                    # WM_SETCURSOR
            cur = {"body": 32646, "size": 32642, "rot": 32649, "lock": 32649}.get(self.last_hit if edit else None, 32649)
            user32.SetCursor(user32.LoadCursorW(None, cur)); return 1
        if msg == 0x205:                                   # WM_RBUTTONUP
            self.menu(hwnd); return 0
        return user32.DefWindowProcW(hwnd, msg, wp, lp)

    def menu(self, hwnd):
        locked = self.cfg["widget_locked"]
        m = user32.CreatePopupMenu()
        for i, t in ((1, "Open dashboard"), (2, "Refresh now"),
                     (5, "Unlock to move / resize / rotate" if locked else "Lock widget"),
                     (6, "Reset position, size and angle"), (7, "Switch layout (vertical / horizontal)"),
                     (8, "Next style  (now: " + STYLE_NAMES.get(self.cfg.get("widget_style", "rings"), "?") + ")"),
                     (3, "Hide widget"), (4, "Quit")):
            user32.AppendMenuW(m, 0, i, t)
        pt = wt.POINT(); user32.GetCursorPos(ctypes.byref(pt))
        user32.SetForegroundWindow(hwnd)
        cmd = user32.TrackPopupMenu(m, 0x100 | 0x80, pt.x, pt.y, 0, hwnd, None)
        user32.DestroyMenu(m)
        cfg = core.load_config()
        if cmd == 1: self.cb[0]()
        elif cmd == 2: self.cb[1]()
        elif cmd == 3: cfg["widget_enabled"] = False; core.save_config(cfg)
        elif cmd == 4: self.cb[2]()
        elif cmd == 5: cfg["widget_locked"] = not locked; core.save_config(cfg)
        elif cmd == 6:
            cfg.update(widget_cx=None, widget_cy=None, widget_angle=0.0, widget_size=36, widget_layout="auto")
            core.save_config(cfg)
        elif cmd == 7:
            cfg["widget_layout"] = "horizontal" if self.vertical else "vertical"; core.save_config(cfg)
        elif cmd == 8:
            cur = cfg.get("widget_style", "rings"); cur = cur if cur in STYLES else "rings"
            cfg["widget_style"] = STYLES[(STYLES.index(cur) + 1) % len(STYLES)]; core.save_config(cfg)

    def run(self):
        self.proc = WNDPROC(self.wndproc)
        wc = WNDCLASS()
        wc.lpfnWndProc, wc.lpszClassName = self.proc, "ClaudeUsageWidget"
        wc.hInstance = ctypes.windll.kernel32.GetModuleHandleW(None)
        user32.RegisterClassW(ctypes.byref(wc))
        # layered | topmost | toolwindow (no taskbar button) | noactivate
        self.hwnd = user32.CreateWindowExW(0x80000 | 0x8 | 0x80 | 0x08000000, "ClaudeUsageWidget", "claude-usage-widget",
                                           0x80000000, 0, 0, 1, 1, None, None, wc.hInstance, None)
        self.tipproc = WNDPROC(lambda h, m, w_, l: user32.DefWindowProcW(h, m, w_, l))
        tc = WNDCLASS(); tc.lpfnWndProc, tc.lpszClassName, tc.hInstance = self.tipproc, "ClaudeUsageTip", wc.hInstance
        user32.RegisterClassW(ctypes.byref(tc))
        # same flags + WS_EX_TRANSPARENT (0x20): the card never steals the mouse from the widget
        self.tip = user32.CreateWindowExW(0x80000 | 0x8 | 0x80 | 0x08000000 | 0x20, "ClaudeUsageTip", "claude-usage-tip",
                                          0x80000000, 0, 0, 1, 1, None, None, wc.hInstance, None)
        user32.SetTimer(self.hwnd, 1, 1000, None)
        msg = wt.MSG()
        while user32.GetMessageW(ctypes.byref(msg), None, 0, 0) > 0:
            user32.TranslateMessage(ctypes.byref(msg)); user32.DispatchMessageW(ctypes.byref(msg))
