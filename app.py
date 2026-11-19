"""Background process: poller + tray icon + taskbar widget + notifications.
Opens the dashboard as a child process on demand."""
import ctypes, json, os, random, subprocess, sys, threading, time
import pystray
from PIL import Image, ImageDraw, ImageFont
import core
from widget import Widget

SHOW = core.DATA / "show.flag"
TOAST_BATCH_SEC = 6        # wait this long to merge alerts that arrive together into one toast
TOAST_MIN_GAP_SEC = 15     # never show toasts more often than this
TOAST_MAX_LINES = 4        # accounts listed in one toast; the rest are summarised as '+N account khác'
SAME_WINDOW_SEC = 1800     # reset times within 30 min of the remembered one = same usage window


def color(p, cfg):
    return "#e06c75" if p >= cfg["block_pct"] else "#f5a742" if p >= cfg["warn_pct"] else "#7fd88f"


class App:
    def __init__(self):
        core.PIDFILE.write_text(str(os.getpid()))
        for f in (SHOW, core.FLAG, core.TESTFLAG):
            try: f.unlink()
            except OSError: pass
        self.cfg, self.cfg_mtime = core.load_config(), 0
        if self.cfg["autostart"]: core.set_autostart(True)   # keep registry command current
        self.data, self.dash, self.dash_t0 = {}, None, 0
        self.next_at, self.fails = {}, {}
        self.busy, self.refresh_info = set(), None
        self.pending, self.last_toast = [], 0.0
        self.baseline = not core.ALERTS.exists()   # first run: record what is already high silently, don't announce it
        self.t_start = time.time()
        self.alert_state = self.load_alert_state()
        self.show_lock = threading.Lock()
        self.icon = pystray.Icon("claude-usage", self.make_icon(None), "Claude usage", pystray.Menu(
            pystray.MenuItem("Open dashboard", lambda: self.show(), default=True),
            pystray.MenuItem("Refresh now", lambda: core.FLAG.write_text("1")),
            pystray.MenuItem("Show widget", self.toggle_widget, checked=lambda i: self.cfg["widget_enabled"]),
            pystray.MenuItem("Notifications", self.toggle_notify, checked=lambda i: self.cfg["notify_enabled"]),
            pystray.MenuItem("Send test notification", lambda: core.TESTFLAG.write_text("1")),
            pystray.MenuItem("Quit", lambda: self.quit())))
        self.widget = Widget(self.show, lambda: core.FLAG.write_text("1"), self.quit)

    # ---- dashboard process ----
    @staticmethod
    def dash_window():
        """Handle of the visible dashboard window, or 0. Title alone is not enough: File Explorer names its window after
        the folder, so an Explorer window on a folder called "claude-usage" would be mistaken for the dashboard."""
        u, found = ctypes.windll.user32, []
        CB = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
        def cb(h, _):
            buf = ctypes.create_unicode_buffer(32)
            u.GetWindowTextW(h, buf, 32)
            if buf.value == "claude-usage" and u.IsWindowVisible(h):
                cls = ctypes.create_unicode_buffer(64); u.GetClassNameW(h, cls, 64)
                if cls.value.startswith("WindowsForms"): found.append(h)      # pywebview's WinForms form
            return True
        u.EnumWindows(CB(cb), None)
        return found[0] if found else 0

    def show(self):
        """Open the dashboard / bring it to front. Runs on its own thread: _show may wait a little for a dashboard that
        is just shutting down, and the callers (widget, tray, loop) must never freeze for that."""
        threading.Thread(target=self._show, daemon=True).start()

    def _show(self):
        try:
            self._show_locked()
        except Exception as e:
            core.log(f"open dashboard failed: {e!r}")

    def _show_locked(self):
        u = ctypes.windll.user32
        with self.show_lock:
            alive = self.dash is not None and self.dash.poll() is None
            if alive:
                h = self.dash_window()
                if h:                                         # open -> bring to front
                    u.ShowWindow(h, 9); u.SetForegroundWindow(h); return
                # no window: either still loading, or just closed and the process is being torn down (~2 s)
                try: self.dash.wait(timeout=2.5); alive = False
                except subprocess.TimeoutExpired: pass
                if alive:
                    if time.time() - self.dash_t0 < 20: return    # still loading, don't double-launch
                    # alive but no window = stuck: restart it. /T: in the packaged exe self.dash is the onefile
                    # bootloader, the real dashboard is its child
                    subprocess.run(["taskkill", "/PID", str(self.dash.pid), "/T", "/F"], capture_output=True, creationflags=0x08000000)
            self.dash = subprocess.Popen(core.launch_cmd("dashboard"), creationflags=0x08000000)
            self.dash_t0 = time.time()

    def toggle_widget(self, *_):
        cfg = core.load_config(); cfg["widget_enabled"] = not cfg["widget_enabled"]; core.save_config(cfg)

    def toggle_notify(self, *_):
        cfg = core.load_config(); cfg["notify_enabled"] = not cfg["notify_enabled"]; core.save_config(cfg)

    # ---- tray icon ----
    def make_icon(self, pct, busy=False):
        S = 256
        c = "#6b6b6b" if pct is None else color(pct, self.cfg)
        img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
        d = ImageDraw.Draw(img)
        d.rounded_rectangle((6, 6, S - 6, S - 6), radius=44, fill="#161616", outline=c, width=14)
        t = "?" if pct is None else "!!" if pct >= 100 else str(int(pct))
        try: font = ImageFont.truetype("consolab.ttf", 150 if len(t) < 3 else 120)
        except Exception: font = ImageFont.load_default()
        d.text((S // 2, 108), t, fill=c, font=font, anchor="mm")
        d.rounded_rectangle((44, 186, S - 44, 214), radius=14, fill="#3a3a3a")
        if pct:
            d.rounded_rectangle((44, 186, 44 + max(28, (S - 88) * min(pct, 100) / 100), 214), radius=14, fill=c)
        if busy:                                              # blue dot = "fetching right now"
            d.ellipse((S - 92, 6, S - 6, 92), fill="#61afef", outline="#161616", width=10)
        return img.resize((64, 64), Image.LANCZOS)

    def refresh_icon(self):
        ok = {n: d for n, d in self.data.items() if "five" in d}
        worst = max([max(d["five"], d["week"]) for d in ok.values()] or [0])
        self.icon.icon = self.make_icon(worst if ok else None, bool(self.busy))
        lines = []
        for n, d in self.data.items():
            s = f"5h {d['five']:.0f}% 7d {d['week']:.0f}%" if "five" in d else "chưa có dữ liệu"
            if d.get("err"): s += f"  ⚠ {d['err']}"
            lines.append(f"{n}: {s}")
        head = "Claude usage" + (" - đang cập nhật..." if self.busy else "")
        self.icon.title = (head + "\n" + "\n".join(lines))[:127]

    def write(self):
        core.write_cache(self.cfg, self.data, self.busy, self.refresh_info)

    # ---- polling: per-profile schedule, backoff on 429/errors, last good value is kept ----
    def seed(self):
        """Start from the cache so a restart never triggers an immediate burst of API calls."""
        c = core.load_cache() or {}
        now, iv = time.time(), core.effective_interval(self.cfg)
        for i, p in enumerate(self.cfg["profiles"]):
            old = (c.get("profiles") or {}).get(p["name"]) or {}
            if "five" in old and old.get("fetched_at"):
                self.data[p["name"]] = {k: v for k, v in old.items() if k != "path"}
                self.next_at[p["name"]] = max(old["fetched_at"] + iv, old.get("retry_at") or 0)
            else:
                self.next_at[p["name"]] = now + i * 4          # stagger first fetches

    def poll_one(self, p):
        name, iv = p["name"], core.effective_interval(self.cfg)
        self.busy.add(name); self.write(); self.refresh_icon()   # show "waiting" before the (slow) request
        try:
            r = (core.refresh_if_expired(p) if self.cfg.get("token_refresh", True) else None) or core.fetch(p)
        finally:
            self.busy.discard(name)
        now = time.time()
        prev = {k: v for k, v in self.data.get(name, {}).items() if k not in ("err", "status", "retry_at")}
        if "five" in r:
            self.fails[name] = 0
            self.data[name] = {**r, "fetched_at": now}
            self.next_at[name] = now + iv * (1 + random.random() * 0.1)
        else:
            self.fails[name] = n = self.fails.get(name, 0) + 1
            if r["status"] == 429:
                wait = r.get("retry_after") or min(900, 120 * 2 ** (n - 1))
            elif r["status"] == 0 and r.get("retry_after"):
                wait = r["retry_after"]                          # claude itself is refreshing this token right now
            elif r["status"] == 401 or "credentials" in r["err"]:
                wait = max(iv, 300)                              # token expired / login needed: retry slowly
            else:
                wait = min(600, iv * 2 ** (n - 1))
            wait = max(wait, 30) * (1 + random.random() * 0.1)
            core.log(f"fetch {name}: {r['err']} (fail #{n}), retry in {wait:.0f}s")
            self.data[name] = {**prev, "err": r["err"], "status": r["status"], "retry_at": now + wait}
            self.next_at[name] = now + wait

    def refresh_requested(self):
        """Manual refresh: never faster than 20s per profile and never before a 429 retry window ends.
        Records what happened so the UI can tell the user (started / skipped and why)."""
        now, info = time.time(), {"ts": time.time(), "started": [], "skipped": []}
        for i, p in enumerate(self.cfg["profiles"]):
            n, d = p["name"], self.data.get(p["name"], {})
            if now - d.get("fetched_at", 0) < 20:
                info["skipped"].append({"name": n, "why": "vừa cập nhật xong"})
            elif now < d.get("retry_at", 0):
                info["skipped"].append({"name": n, "why": f"đang chờ giới hạn API, thử lại sau {int(d['retry_at'] - now)}s"})
            else:
                self.next_at[n] = min(self.next_at.get(n, now), now + i * 2)
                self.busy.add(n); info["started"].append(n)
        self.refresh_info = info
        self.write(); self.refresh_icon()

    def publish(self):
        self.write(); self.refresh_icon()
        self.alerts({n: d for n, d in self.data.items() if "five" in d and not d.get("err")})

    # ---- notifications: dedupe per usage window, persisted, batched, rate-limited, configurable ----
    @staticmethod
    def load_alert_state():
        try: return json.loads(core.ALERTS.read_text("utf-8"))
        except Exception: return {}

    def alerts(self, fresh):
        """`fresh` = accounts with up-to-date (non-stale) data. Queues one alert per (account, window, level)
        per usage window. State is always recorded, even when notifications are off, so turning them on later
        does not fire a burst for conditions that already existed."""
        cfg, st, changed = self.cfg, self.alert_state, False
        for n, d in fresh.items():
            for key, label in (("five", "5h"), ("week", "7d")):
                v = d[key]
                lvl = "block" if v >= cfg["block_pct"] else "warn" if v >= cfg["warn_pct"] else None
                if not lvl:
                    continue
                rts = core.iso_ts(d.get(key + "_reset")) or 0.0
                k = f"{n}|{key}|{lvl}"
                if k in st and abs(rts - st[k]) < SAME_WINDOW_SEC:   # API jitters reset time by ~1s: compare loosely
                    continue
                st[k] = rts; changed = True
                if cfg["notify_enabled"] and cfg["notify_" + lvl] and not getattr(self, "baseline", False):
                    self.pending.append({"t": time.time(), "lvl": lvl, "name": n, "label": label, "v": v})
        if getattr(self, "baseline", False) and (len(fresh) >= len(cfg["profiles"]) or time.time() - self.t_start > 120):
            self.baseline, changed = False, True               # every account seen once (or 2 min passed): from now on, announce changes
        if changed:
            try: core.ALERTS.write_text(json.dumps(st), "utf-8")
            except OSError: pass

    def send_toast(self, title, msg):
        core.log(f"toast | {title} | " + msg.replace("\n", " / "))
        try: self.icon.notify(msg[:250], title)
        except Exception as e: core.log(f"toast failed: {e!r}")

    def flush_toasts(self):
        """Merge queued alerts into one compact toast (one line per account) and suggest the emptiest account.
        Waits a few seconds so alerts that arrive together are batched; never more often than TOAST_MIN_GAP_SEC."""
        now = time.time()
        if not self.cfg["notify_enabled"]:
            self.pending.clear()
        if not self.pending or now - self.pending[0]["t"] < TOAST_BATCH_SEC or now - self.last_toast < TOAST_MIN_GAP_SEC:
            return
        items, self.pending = self.pending, []
        self.last_toast = now
        by = {}
        for e in items: by.setdefault(e["name"], []).append(e)
        lines = [f"{n}: " + " · ".join(f"{e['label']} {e['v']:.0f}%" for e in es) for n, es in by.items()]
        if len(lines) > TOAST_MAX_LINES:                       # many accounts crossed at once: keep the toast readable
            lines = lines[:TOAST_MAX_LINES] + [f"+{len(lines) - TOAST_MAX_LINES} account khác"]
        fresh = {n: d for n, d in self.data.items() if "five" in d and not d.get("err") and n not in by}
        if fresh:
            best = min(fresh, key=lambda m: max(fresh[m]["five"], fresh[m]["week"]))
            lines.append(f"Nên dùng: {best} (5h {fresh[best]['five']:.0f}% · 7d {fresh[best]['week']:.0f}%)")
        blocked = any(e["lvl"] == "block" for e in items)
        title = "Claude usage - " + ("CHẶN" if blocked else "sắp hết") + (f" ({len(by)} account)" if len(by) > 1 else "")
        self.send_toast(title, "\n".join(lines))

    def test_toast(self):
        on = self.cfg["notify_enabled"]
        self.send_toast("Claude usage - thử thông báo", "Thông báo đang " + ("BẬT" if on else "TẮT (tin này chỉ là bản thử)"))

    # ---- main loop ----
    def loop(self):
        self.seed(); self.publish()
        while True:
            try:
                try: m = core.CONFIG.stat().st_mtime if core.CONFIG.exists() else 0
                except OSError: m = 0
                if m != self.cfg_mtime:
                    self.cfg, self.cfg_mtime = core.load_config(), m
                    for n, d in self.data.items():              # new interval applies to the next fetch
                        if "fetched_at" in d:
                            self.next_at[n] = max(d["fetched_at"] + core.effective_interval(self.cfg), d.get("retry_at", 0))
                if SHOW.exists():
                    try: SHOW.unlink()
                    except OSError: pass
                    self.show()
                if core.TESTFLAG.exists():
                    try: core.TESTFLAG.unlink()
                    except OSError: pass
                    self.test_toast()
                if core.FLAG.exists():
                    try: core.FLAG.unlink()
                    except OSError: pass
                    self.refresh_requested()
                now = time.time()
                for p in self.cfg["profiles"]:                   # at most one request per tick -> natural spacing
                    if now >= self.next_at.setdefault(p["name"], now):
                        self.poll_one(p); self.publish(); break
                self.flush_toasts()
            except Exception:
                pass
            time.sleep(1)

    def quit(self):
        try: core.PIDFILE.unlink()
        except OSError: pass
        if self.dash and self.dash.poll() is None:            # /T: onefile bootloader + the real dashboard child
            subprocess.run(["taskkill", "/PID", str(self.dash.pid), "/T", "/F"], capture_output=True, creationflags=0x08000000)
        self.icon.stop(); os._exit(0)

    def run(self):
        threading.Thread(target=self.loop, daemon=True).start()
        self.widget.start()
        self.icon.run()


def main(hidden=False):
    try: ctypes.windll.shcore.SetProcessDpiAwareness(2)
    except Exception: pass
    if core.daemon_running():          # already running -> ask it to open the dashboard
        SHOW.write_text("1"); return
    a = App()
    try:
        import hookinstall
        hookinstall.repair(a.cfg["profiles"])      # keep installed hooks pointing at this exe if it moved
    except Exception:
        pass
    if not hidden: a.show()
    a.run()
