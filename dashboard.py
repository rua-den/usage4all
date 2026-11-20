"""Dashboard window (pywebview/WebView2). Runs as its own short-lived process so its RAM is freed on close.
The window is frameless, so resizing / maximizing is done here with plain Win32 calls driven by the page's edge handles."""
import ctypes, json, os, sys, threading, time
from ctypes import wintypes as wt
from pathlib import Path
import webview
import core

# PRIVATE copies of user32 / kernel32: ctypes.windll.user32 is shared with pywebview, and argtypes set on it change
# pywebview's own calls too (its window.move passes None for SetWindowPos width/height -> TypeError -> the window
# could no longer be dragged). Never set argtypes / restype on ctypes.windll.* in this process.
user32 = ctypes.WinDLL("user32", use_last_error=True)
user32.SetWindowPos.argtypes = [wt.HWND, wt.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wt.UINT]
user32.MonitorFromWindow.restype = ctypes.c_void_p
user32.MonitorFromWindow.argtypes = [wt.HWND, wt.DWORD]
user32.GetMonitorInfoW.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
MIN_W, MIN_H = 420, 330
_page_ready = threading.Event()
CFG_LOCK = threading.RLock()
GEO_LOCK = threading.RLock()     # window geometry: a content fit must never interleave with a restore / user resize     # one load-modify-save of config.json at a time (js_api calls each run on their own thread)
k32 = ctypes.WinDLL("kernel32", use_last_error=True)
k32.GetCurrentProcess.restype = wt.HANDLE
k32.TerminateProcess.argtypes = [wt.HANDLE, wt.UINT]
k32.TerminateProcess.restype = wt.BOOL


class MONITORINFO(ctypes.Structure):
    _fields_ = [("cbSize", wt.DWORD), ("rcMonitor", wt.RECT), ("rcWork", wt.RECT), ("dwFlags", wt.DWORD)]


def quit_now(code=0):
    """End the process immediately. TerminateProcess skips the slow DLL-detach teardown of WebView2 / .NET that
    os._exit (ExitProcess) waits for; WebView2's own msedgewebview2 processes exit when their host is gone."""
    try:
        k32.TerminateProcess(k32.GetCurrentProcess(), code)
    finally:
        os._exit(code)


def resource(name):
    return Path(getattr(sys, "_MEIPASS", Path(__file__).parent)) / name


def initial_width(n_profiles, screen_w):
    """Default window width: enough columns for the accounts (the account grid adds a column per ~380 px)."""
    w = 760 if n_profiles <= 4 else 1120 if n_profiles <= 8 else 1480
    return int(min(w, screen_w - 80))


class Api:
    _window = None          # private: pywebview walks every public attribute of js_api (infinite recursion on a Window)
    _hwnd = 0
    _normal = None          # [x, y, w, h] to go back to after maximizing
    _save_t = None
    user_sized = False
    maximized = False

    # ---- state / actions used by the page ----
    def state(self):
        cfg = core.load_config()
        return {"cfg": cfg, "cache": core.load_cache(), "version": core.__version__, "interval": core.effective_interval(cfg)}
    def test_notify(self): core.TESTFLAG.write_text("1"); return True
    def hook_status(self):
        import hookinstall; return hookinstall.status(core.load_config()["profiles"])
    def hook_install(self):
        import hookinstall; return hookinstall.install(core.load_config()["profiles"])
    def hook_uninstall(self):
        import hookinstall; return hookinstall.uninstall(core.load_config()["profiles"])
    def refresh(self): core.FLAG.write_text("1"); return True
    def hide(self):
        # close for real: hide the window at once (feels instant), save the size, then kill the process.
        # Not window.destroy() + os._exit(): pywebview's normal shutdown (WebView2 dispose + wait) took ~2 s.
        try: user32.ShowWindowAsync(self.hwnd(), 0)          # SW_HIDE, never blocks on the UI thread
        except Exception: pass
        self.shutdown()
    def shutdown(self):
        """Save the window size, then end the process. CFG_LOCK waits for a save in progress, so config.json
        is never cut off mid-write, and no later save can run once we hold it."""
        if self._save_t: self._save_t.cancel()
        CFG_LOCK.acquire()
        self._persist_now()
        quit_now()
    def widget_unlock(self):
        with CFG_LOCK:
            cfg = core.load_config(); cfg["widget_locked"] = False; core.save_config(cfg); return True
    def widget_reset(self):
        with CFG_LOCK:
            cfg = core.load_config()
            cfg.update(widget_cx=None, widget_cy=None, widget_angle=0.0, widget_size=36, widget_layout="auto", widget_locked=True)
            core.save_config(cfg); return True
    def minimize(self): self._window.minimize()
    def save(self, v):
        try:
            with CFG_LOCK:
                cfg = core.load_config(); cfg.update(v)
                cfg["interval_sec"] = max(core.MIN_INTERVAL, int(cfg["interval_sec"]))
                core.save_config(cfg)
            core.set_autostart(cfg["autostart"])
            return True
        except Exception:
            return False

    # ---- window geometry (physical pixels) ----
    def hwnd(self):
        if self._hwnd and user32.IsWindow(self._hwnd):
            return self._hwnd
        found, pid = [], os.getpid()
        CB = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
        def cb(h, _):
            buf = ctypes.create_unicode_buffer(32); user32.GetWindowTextW(h, buf, 32)
            owner = wt.DWORD(); user32.GetWindowThreadProcessId(h, ctypes.byref(owner))
            if buf.value == "claude-usage" and owner.value == pid:          # may still be hidden (shown once sized)
                cls = ctypes.create_unicode_buffer(64); user32.GetClassNameW(h, cls, 64)
                if cls.value.startswith("WindowsForms"): found.append(h)
            return True
        user32.EnumWindows(CB(cb), None)
        self._hwnd = found[0] if found else 0
        return self._hwnd

    def rect(self):
        r = wt.RECT(); user32.GetWindowRect(self.hwnd(), ctypes.byref(r))
        return [r.left, r.top, r.right - r.left, r.bottom - r.top]

    def _place(self, x, y, w, h):
        user32.SetWindowPos(self.hwnd(), None, int(x), int(y), int(max(w, 1)), int(max(h, 1)), 0x4 | 0x10)   # NOZORDER | NOACTIVATE

    def set_rect(self, x, y, w, h):
        """Resize by the user (edge handles). From now on the window no longer follows the content height."""
        with GEO_LOCK:
            self.user_sized, self.maximized = True, False
            self._place(x, y, max(w, MIN_W), max(h, MIN_H))
        self._persist_soon()
        return True

    def work_area(self):
        mi = MONITORINFO(); mi.cbSize = ctypes.sizeof(MONITORINFO)
        user32.GetMonitorInfoW(user32.MonitorFromWindow(self.hwnd(), 2), ctypes.byref(mi))   # nearest monitor
        w = mi.rcWork
        return [w.left, w.top, w.right - w.left, w.bottom - w.top]

    def toggle_max(self):
        with GEO_LOCK:
            if self.maximized and self._normal:
                self._place(*self._normal); self.maximized = False
            else:
                self._normal = self.rect(); self._place(*self.work_area()); self.maximized, self.user_sized = True, True
        self._persist_soon()
        return True

    def auto_fit(self):
        """Forget the chosen size: the window follows the content again."""
        self.user_sized = self.maximized = False
        with CFG_LOCK:
            cfg = core.load_config(); cfg["dash_rect"] = None; core.save_config(cfg)
        return True

    def win_state(self): return {"user_sized": self.user_sized, "maximized": self.maximized}

    def page_ready(self):
        """The page has sized the window (auto-fit, or the saved size): now it may be shown, without a visible jump."""
        _page_ready.set(); return True

    def fit(self, h):
        with GEO_LOCK:
            if self.user_sized or self.maximized:           # the user's size wins
                return False
            try:
                # own SetWindowPos, not window.resize(): pywebview passes SWP_SHOWWINDOW, which would show the window
                # while it is still hidden and being sized (visible jump on open). h is in CSS px = logical px.
                scale = user32.GetDpiForWindow(self.hwnd()) / 96 or 1
                top = user32.GetSystemMetrics(1) - int(40 * scale)
                x, y, w, _ = self.rect()
                self._place(x, y, w, int(max(MIN_H * scale, min(h * scale, top))))
            except Exception:
                pass
            return True

    # ---- remember the size the user picked ----
    def _persist_now(self):
        if not self.user_sized:
            return
        try:
            r = self._normal if (self.maximized and self._normal) else self.rect()
            if r[2] < MIN_W // 2 or r[3] < MIN_H // 2:          # window already gone: keep the last good size
                return
            with CFG_LOCK:
                cfg = core.load_config(); cfg["dash_rect"] = r; core.save_config(cfg)
        except Exception:
            pass

    def _persist_soon(self):
        if self._save_t: self._save_t.cancel()
        self._save_t = threading.Timer(0.8, self._persist_now); self._save_t.daemon = True; self._save_t.start()

    def restore_saved(self):
        """After the window exists: put it back where/how big the user left it (kept at least partly on screen)."""
        r = core.load_config().get("dash_rect")
        if not (isinstance(r, list) and len(r) == 4 and all(isinstance(v, (int, float)) for v in r)):
            return
        sw, sh = user32.GetSystemMetrics(0), user32.GetSystemMetrics(1)
        x, y, w, h = r
        w, h = min(max(w, MIN_W), sw), min(max(h, MIN_H), sh)
        x, y = min(max(x, -w + 160), sw - 160), min(max(y, 0), sh - 80)
        with GEO_LOCK:
            self._place(x, y, w, h); self.user_sized = True
        try: self._window.evaluate_js("syncWin()")           # tell the page, so it stops asking for auto-fit
        except Exception: pass


def storage_dir():
    """WebView2 data folder for this run, inside our own data dir. pywebview's default (a %TEMP% folder it deletes
    only on its slow normal shutdown) is left behind when we kill the process, so we clean up our own old ones."""
    root = core.DATA / "webview"
    mine = root / str(os.getpid())
    def sweep():
        import shutil
        for d in root.glob("*"):
            if d != mine and d.is_dir():
                shutil.rmtree(d, ignore_errors=True)          # a folder still in use is locked and simply stays
    threading.Thread(target=sweep, daemon=True).start()
    return str(mine)


def page():
    """ui.html with the current state inlined, so numbers show the instant the window appears."""
    init = json.dumps({"cfg": core.load_config(), "cache": core.load_cache()}).replace("</", "<\\/")
    return resource("ui.html").read_text("utf-8").replace("/*__INIT__*/null", init, 1)


def run():
    try: ctypes.windll.shcore.SetProcessDpiAwareness(2)      # physical pixels everywhere (matches the page's devicePixelRatio maths)
    except Exception: pass
    api = Api()
    cfg = core.load_config()
    scale = user32.GetDpiForSystem() / 96
    w = initial_width(len(cfg["profiles"]), user32.GetSystemMetrics(0) / scale)
    api._window = webview.create_window("claude-usage", html=page(), js_api=api, width=w, height=640, min_size=(MIN_W, MIN_H),
                                        frameless=True, easy_drag=False, background_color="#0a0a0a", hidden=True)
    api._window.events.closing += lambda: api.hide()          # Alt+F4 / taskbar "close": same fast path as the ✕ button
    api._window.events.closed += lambda: quit_now()

    def after_start():
        for _ in range(100):                                  # wait for the window handle, then restore the saved size
            if api.hwnd(): break
            time.sleep(0.1)
        api.restore_saved()
        _page_ready.wait(4)                                   # page has fitted itself (never wait forever)
        api._window.show()
    webview.start(after_start, private_mode=True, storage_path=storage_dir())
    quit_now()
