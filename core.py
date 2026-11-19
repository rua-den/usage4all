"""Shared logic: config, usage fetch, cache, OAuth token refresh.
Tokens are never logged, cached, shown or sent anywhere except Anthropic's own endpoints. The only write to a
profile's .credentials.json is refresh_if_expired() (same procedure and lock as the claude CLI itself)."""
import hashlib, json, os, re, sys, threading, time, urllib.request, urllib.error

__version__ = "1.1.0"
from datetime import datetime, timezone
from pathlib import Path

HOME = Path.home()
DATA = Path(os.environ.get("LOCALAPPDATA", HOME)) / "claude-usage"
DATA.mkdir(parents=True, exist_ok=True)
CONFIG, CACHE, FLAG, PIDFILE = DATA / "config.json", DATA / "usage_cache.json", DATA / "refresh.flag", DATA / "daemon.pid"
ALERTS, TESTFLAG, EVENTS = DATA / "alerts.json", DATA / "notify_test.flag", DATA / "events.log"
API = "https://api.anthropic.com/api/oauth/usage"
TOKEN_URL = "https://platform.claude.com/v1/oauth/token"   # same endpoint + public client id as the claude CLI
CLIENT_ID = "9d1c250a-e61b-44d9-88ed-5944d1962f5e"
MIN_INTERVAL = 90      # never poll faster than this (API rate-limits per account)
DEFAULTS = {"interval_sec": 180, "warn_pct": 80, "block_pct": 95, "block_enabled": True,
            "autostart": False, "profiles": [], "token_refresh": True,
            "notify_enabled": True, "notify_warn": True, "notify_block": True,
            "widget_enabled": True, "widget_size": 36, "widget_reset": True,
            "widget_anchor": "tray", "widget_offset_x": 0, "widget_offset_y": 0, "aliases": {},
            "widget_locked": True, "widget_sort": "config", "widget_max_cells": 0, "widget_max_len": 0, "dash_rect": None, "widget_show_label": True, "widget_show_5h": True, "widget_show_7d": True,
            "widget_show_reset5": True, "widget_show_reset7": True, "widget_tooltip": True, "widget_label": "alias", "widget_font": 100, "widget_opacity": 100,
            "widget_theme": "auto", "widget_plate": False, "widget_hidden": [], "widget_style": "rings", "widget_layout": "auto", "widget_cx": None, "widget_cy": None, "widget_angle": 0.0}


def log(msg):
    """Append one line to events.log (toasts sent, API errors...). Self-trimming; never raises."""
    try:
        if EVENTS.exists() and EVENTS.stat().st_size > 200_000:
            EVENTS.write_text(EVENTS.read_text("utf-8", errors="ignore")[-100_000:], "utf-8")
        with EVENTS.open("a", encoding="utf-8") as f:
            f.write(datetime.now().strftime("%Y-%m-%d %H:%M:%S ") + msg + "\n")
    except Exception:
        pass


def effective_interval(cfg):
    """Poll interval actually used: never below the floor, and spread out when there are very many accounts
    (about one request per 5 s overall) so a long profile list cannot trigger rate limiting."""
    try: base = int(cfg.get("interval_sec", 180))
    except (TypeError, ValueError): base = 180
    return max(MIN_INTERVAL, base, 5 * len(cfg.get("profiles") or []))


def detect_profiles():
    found = []
    if (HOME / ".claude" / ".credentials.json").exists():
        found.append({"name": "default (desktop)", "path": str(HOME / ".claude")})
    root = HOME / "claude-profiles"
    if root.is_dir():
        for d in sorted(root.iterdir()):
            if (d / ".credentials.json").exists():
                found.append({"name": d.name, "path": str(d)})
    return found


def load_config():
    cfg = dict(DEFAULTS)
    for i in range(6):                                   # tolerate the instant a writer swaps the file in
        try:
            cfg.update(json.loads(CONFIG.read_text("utf-8"))); break
        except FileNotFoundError:
            break
        except Exception:
            if i < 5: time.sleep(0.025)
    if not cfg["profiles"]:
        cfg["profiles"] = detect_profiles()
    cfg["interval_sec"] = max(MIN_INTERVAL, int(cfg["interval_sec"]))
    return cfg


def save_config(cfg):
    """Atomic (temp file + replace): a process killed mid-save, or a reader at the same instant, never sees a
    truncated config.json (load_config would silently fall back to DEFAULTS = every setting lost)."""
    text = json.dumps(cfg, indent=2, ensure_ascii=False)
    tmp = CONFIG.with_name(f"config.{os.getpid()}.{threading.get_ident()}.tmp")   # unique per writer
    tmp.write_text(text, "utf-8")
    for _ in range(25):                                  # Windows refuses to replace a file another process has open
        try:
            os.replace(tmp, CONFIG); return
        except PermissionError:
            time.sleep(0.02)
    try: tmp.unlink()
    except OSError: pass
    CONFIG.write_text(text, "utf-8")                     # last resort: non-atomic, but never lose the update


def load_cache():
    for i in range(6):                                   # tolerate the instant the writer swaps the file
        try:
            return json.loads(CACHE.read_text("utf-8"))
        except Exception:
            if i < 5: time.sleep(0.025)
    return None


# ---- OAuth token refresh: only once the access token has already expired (claude is then not using it) ----
DEAD_MSG = "phiên đăng nhập hết hạn - chạy claude ở profile này rồi /login"
LOCK_STALE = 90        # the CLI (proper-lockfile) touches its lock every 5s and treats one older than 60s as dead
_dead = {}             # credentials path -> sha256 of a refresh token the server rejected (memory only, never logged)


def _fp(token):
    return hashlib.sha256(token.encode("utf-8")).hexdigest()


def _lock_paths(d):
    """The two lock directories the claude CLI creates before refreshing a profile's token (current + legacy)."""
    d = Path(d)
    try: real = Path(os.path.realpath(d))
    except OSError: real = d
    return [d / ".oauth_refresh.lock", Path(str(real) + ".lock")]


def _release(got):
    for p in reversed(got):
        try: p.rmdir()
        except OSError: pass


def _acquire(paths):
    """mkdir-based lock compatible with the CLI. None = someone else holds it (a dead holder's lock is taken over)."""
    got = []
    for p in paths:
        try:
            p.mkdir()
        except FileExistsError:
            try: stale = time.time() - p.stat().st_mtime > LOCK_STALE
            except OSError: stale = False
            try:
                if not stale: raise OSError("locked")
                p.rmdir(); p.mkdir()
            except OSError:
                _release(got); return None
        except OSError:
            _release(got); return None
        got.append(p)
    return got


def _oauth_error(e):
    """OAuth error code from an HTTPError body ("invalid_grant"...). Only a short identifier, never the body."""
    try:
        err = json.loads(e.read().decode("utf-8", "replace")).get("error")
        if isinstance(err, dict): err = err.get("type")
        return err if isinstance(err, str) and re.fullmatch(r"[a-z_]{1,40}", err) else ""
    except Exception:
        return ""


def _post_refresh(rt, scopes):
    """POST the refresh grant (same body as the CLI). Token response dict, or {err, status[, dead, retry_after]}."""
    body = {"grant_type": "refresh_token", "refresh_token": rt, "client_id": CLIENT_ID}
    if scopes: body["scope"] = " ".join(scopes)
    for attempt in (0, 1):
        req = urllib.request.Request(TOKEN_URL, data=json.dumps(body).encode("utf-8"), method="POST",
                                     headers={"Content-Type": "application/json", "User-Agent": "claude-usage/" + __version__})
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                d = json.load(r)
            if not isinstance(d, dict) or not d.get("access_token") or not isinstance(d.get("expires_in"), (int, float)):
                return {"err": "làm mới token: phản hồi không hợp lệ", "status": 0}
            return d
        except urllib.error.HTTPError as e:
            code = _oauth_error(e)
            if code == "invalid_scope" and "scope" in body and attempt == 0:
                del body["scope"]; continue                  # like the CLI: retry once with the originally granted scopes
            if code == "invalid_grant":
                return {"err": DEAD_MSG, "status": 401, "dead": True}
            ra = e.headers.get("Retry-After") if e.headers else None
            return {"err": f"làm mới token: HTTP {e.code}" + (f" {code}" if code else ""), "status": e.code,
                    "retry_after": int(ra) if ra and ra.isdigit() else None}
        except Exception as e:
            return {"err": f"làm mới token: {type(e).__name__}", "status": 0}
    return {"err": "làm mới token: invalid_scope", "status": 400}


def _write_creds(cred, data):
    """Atomic replace (temp file + os.replace, retried while a reader has the file open). Compact JSON like the CLI."""
    text = json.dumps(data, ensure_ascii=False, separators=(",", ":"))
    tmp = cred.with_name(f"{cred.name}.{os.getpid()}.claude-usage.tmp")
    tmp.write_text(text, "utf-8")
    for _ in range(50):
        try:
            os.replace(tmp, cred); return
        except PermissionError:
            time.sleep(0.02)
    try: tmp.unlink()
    except OSError: pass
    cred.write_text(text, "utf-8")   # last resort: the old refresh token may already be void, never drop the new one


def refresh_if_expired(profile):
    """If the profile's access token has expired, get a new one with its refresh token and write it back to
    .credentials.json the way the claude CLI does (same endpoint, client id, lock directories and fields; every
    other field and key is kept). Returns None when nothing had to be done or it worked (fetch() then reads the
    file), else {err, status[, dead, retry_after]}. dead: the server rejected the refresh token -> the user must
    /login; it is not sent again until the file holds a different refresh token."""
    cred = Path(profile["path"]) / ".credentials.json"
    try:
        o = json.loads(cred.read_text("utf-8"))["claudeAiOauth"]
        rt = o.get("refreshToken")
        if o.get("expiresAt", 0) / 1000 >= time.time() or not isinstance(rt, str) or not rt:
            return None                                      # still valid, or nothing to refresh with (fetch reports it)
    except Exception:
        return None                                          # unreadable: fetch() reports it
    rte = o.get("refreshTokenExpiresAt")
    if _dead.get(str(cred)) == _fp(rt) or (isinstance(rte, (int, float)) and rte / 1000 < time.time()):
        return {"err": DEAD_MSG, "status": 401, "dead": True}
    got = _acquire(_lock_paths(profile["path"]))
    if got is None:
        return {"err": "claude đang làm mới token, thử lại sau", "status": 0, "retry_after": 30}
    try:
        old_text = cred.read_text("utf-8")                   # re-read under the lock: claude may have refreshed meanwhile
        data = json.loads(old_text); o = data["claudeAiOauth"]
        if o.get("refreshToken") != rt or o.get("expiresAt", 0) / 1000 >= time.time():
            return None
        cred.with_name(cred.name + ".claude-usage.bak").write_text(old_text, "utf-8")   # before the POST: once the
        r = _post_refresh(rt, o.get("scopes"))               # server rotates the token, nothing may fail before the write
        if "access_token" not in r:
            if r.get("dead"): _dead[str(cred)] = _fp(rt)
            return r
        now_ms = int(time.time() * 1000)
        new = dict(o)
        new["accessToken"] = r["access_token"]
        new["refreshToken"] = r.get("refresh_token") or rt
        new["expiresAt"] = now_ms + int(r["expires_in"] * 1000)
        if isinstance(r.get("scope"), str) and r["scope"].split():
            new["scopes"] = r["scope"].split()
        rte, rte_in = o.get("refreshTokenExpiresAt"), r.get("refresh_token_expires_in")
        if isinstance(rte, (int, float)) and isinstance(rte_in, (int, float)):
            new["refreshTokenExpiresAt"] = min(rte, now_ms + int(rte_in * 1000))   # the CLI never extends it on refresh
        data["claudeAiOauth"] = new
        _write_creds(cred, data)
        log(f"token refreshed: {profile['name']}")
        return None
    except Exception as e:
        return {"err": f"làm mới token: {type(e).__name__}", "status": 0}
    finally:
        _release(got)


def fetch(profile):
    """Ok -> {five, five_reset, week, week_reset}. Failure -> {err, status, retry_after}."""
    cred = Path(profile["path"]) / ".credentials.json"
    try:
        o = json.loads(cred.read_text("utf-8"))["claudeAiOauth"]
    except Exception:
        return {"err": "không đọc được credentials", "status": 0}
    if o.get("expiresAt", 0) / 1000 < time.time():
        return {"err": "token hết hạn - chạy claude ở profile này 1 lần", "status": 401}
    req = urllib.request.Request(API, headers={
        "Authorization": "Bearer " + o["accessToken"], "anthropic-beta": "oauth-2025-04-20"})
    try:
        with urllib.request.urlopen(req, timeout=20) as r:
            d = json.load(r)
    except urllib.error.HTTPError as e:
        ra = e.headers.get("Retry-After") if e.headers else None
        return {"err": f"HTTP {e.code}", "status": e.code, "retry_after": int(ra) if ra and ra.isdigit() else None}
    except Exception as e:
        return {"err": type(e).__name__, "status": 0}
    f, s = d.get("five_hour") or {}, d.get("seven_day") or {}
    return {"five": f.get("utilization") or 0, "five_reset": f.get("resets_at"),
            "week": s.get("utilization") or 0, "week_reset": s.get("resets_at"),
            "extra": parse_extra(d.get("extra_usage")), "scoped": parse_scoped(d.get("limits")), "plan": o.get("subscriptionType")}


def parse_extra(ex):
    """Extra-usage ("credit") block -> {used, limit, pct, currency} in major units, or None when not enabled."""
    try:
        if not ex or not ex.get("is_enabled") or ex.get("monthly_limit") is None:
            return None
        div = 10 ** int(ex.get("decimal_places") if ex.get("decimal_places") is not None else 2)
        used, limit = (ex.get("used_credits") or 0) / div, ex["monthly_limit"] / div
        pct = ex.get("utilization")
        return {"used": used, "limit": limit, "pct": pct if pct is not None else (used / limit * 100 if limit else 0),
                "currency": ex.get("currency") or "USD"}
    except Exception:
        return None


def parse_scoped(limits):
    """Per-model weekly limits (e.g. Fable) -> [{name, pct, resets_at}]."""
    out = []
    for l in limits or []:
        try:
            name = ((l.get("scope") or {}).get("model") or {}).get("display_name")
            if l.get("kind") == "weekly_scoped" and name:
                out.append({"name": name, "pct": l.get("percent") or 0, "resets_at": l.get("resets_at")})
        except Exception:
            pass
    return out


def write_cache(cfg, data, busy=(), refresh=None):
    out = {"ts": time.time(), "block_enabled": cfg["block_enabled"], "block_pct": cfg["block_pct"],
           "warn_pct": cfg["warn_pct"], "busy": sorted(busy), "refresh": refresh, "profiles": {}}
    for p in cfg["profiles"]:
        out["profiles"][p["name"]] = {"path": p["path"], **data.get(p["name"], {})}
    text = json.dumps(out, indent=1)
    tmp = CACHE.with_suffix(".tmp")                      # atomic: readers never see a half-written file
    tmp.write_text(text, "utf-8")
    for _ in range(25):                                  # Windows refuses to replace a file another process has open
        try:
            os.replace(tmp, CACHE); return
        except PermissionError:
            time.sleep(0.02)
    CACHE.write_text(text, "utf-8")                      # last resort: non-atomic, but never lose the update


def iso_ts(iso):
    try: return datetime.fromisoformat(iso).timestamp()
    except Exception: return None


def fmt_reset(iso):
    if not iso:
        return "-"
    try:
        sec = (datetime.fromisoformat(iso) - datetime.now(timezone.utc)).total_seconds()
    except Exception:
        return "-"
    if sec <= 0:
        return "reset"
    h, m = divmod(int(sec // 60), 60)
    d, h = divmod(h, 24)
    return f"{d}d{h:02d}h" if d else f"{h}h{m:02d}m"


def daemon_running():
    try:
        pid = int(PIDFILE.read_text())
        import ctypes
        h = ctypes.windll.kernel32.OpenProcess(0x1000, False, pid)  # QUERY_LIMITED_INFORMATION
        if not h:
            return False
        code = ctypes.c_ulong()
        ctypes.windll.kernel32.GetExitCodeProcess(h, ctypes.byref(code))
        ctypes.windll.kernel32.CloseHandle(h)
        return code.value == 259  # STILL_ACTIVE
    except Exception:
        return False


def launch_cmd(*extra):
    """Command to start this program again (frozen exe or from source)."""
    if getattr(sys, "frozen", False):
        return [sys.executable, *extra]
    return [str(Path(sys.executable).with_name("pythonw.exe")), str(Path(__file__).with_name("main.py")), *extra]


def _auto_alias(name):
    """Automatic short code: DT for the default profile, T01 for claude-team-01, NG for claude-ngoai."""
    if name.startswith("default"):
        return "DT"
    s = name.replace("claude-", "", 1)
    toks = [t for t in re.split(r"[^0-9A-Za-z]+", s) if t]
    if len(toks) >= 2 and toks[-1].isdigit():
        return (toks[0][0] + toks[-1]).upper()
    return s.replace("-", "")[:2].upper() or "?"


def alias(name, cfg, maxlen=None):
    """Label shown for an account: custom text if set, else a short code or a short name (widget_label)."""
    custom = ((cfg.get("aliases") or {}).get(name) or "").strip()
    if custom:
        s = custom
    elif cfg.get("widget_label", "alias") == "short":
        s = "desk" if name.startswith("default") else name.replace("claude-", "", 1)
    else:
        s = _auto_alias(name)
    return s[:maxlen] if maxlen else s


def aliases(names, cfg, maxlen=None):
    """Labels for a whole set of accounts: like alias(), but automatic labels are made unique (longer prefix, then a counter)."""
    out = {n: alias(n, cfg) for n in names}
    custom = {n for n in names if ((cfg.get("aliases") or {}).get(n) or "").strip()}
    groups = {}
    for n in names:
        if n not in custom:
            groups.setdefault(out[n].lower(), []).append(n)
    for g in groups.values():
        if len(g) < 2:
            continue
        base = {n: re.sub(r"[^0-9A-Za-z]", "", n.replace("claude-", "", 1)) or n for n in g}
        for L in range(3, 8):
            cand = {n: base[n][:L].upper() for n in g}
            if len(set(cand.values())) == len(g):
                out.update(cand); break
        else:
            for i, n in enumerate(g, 1):
                out[n] = f"{base[n][:2].upper()}{i}"
    return {n: (s[:maxlen] if maxlen else s) for n, s in out.items()}


def set_autostart(on):
    import winreg
    key = winreg.OpenKey(winreg.HKEY_CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\Run", 0,
                         winreg.KEY_SET_VALUE)
    if on:
        cmd = " ".join(f'"{c}"' for c in launch_cmd("--hidden"))
        winreg.SetValueEx(key, "ClaudeUsage", 0, winreg.REG_SZ, cmd)
    else:
        try:
            winreg.DeleteValue(key, "ClaudeUsage")
        except FileNotFoundError:
            pass

