"""Unit tests (no GUI, no network, no real credentials). Run:  .venv\\Scripts\\python tests\\test_unit.py"""
import io, json, os, subprocess, sys, tempfile, threading, time, traceback, urllib.error
try: sys.stdout.reconfigure(encoding='utf-8', errors='replace')   # tracebacks may contain non-cp1252 characters
except Exception: pass
from datetime import datetime, timezone, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
TMP = Path(tempfile.mkdtemp(prefix="cu_test_"))
os.environ["LOCALAPPDATA"] = str(TMP)                       # isolate everything from the real install
import core
core.DATA = TMP / "claude-usage"; core.DATA.mkdir(parents=True, exist_ok=True)
core.CONFIG, core.CACHE, core.FLAG = core.DATA / "config.json", core.DATA / "usage_cache.json", core.DATA / "refresh.flag"
core.ALERTS, core.TESTFLAG, core.PIDFILE = core.DATA / "alerts.json", core.DATA / "notify_test.flag", core.DATA / "daemon.pid"
import app as appmod, hookinstall, widget
REAL_FETCH = core.fetch          # some tests replace core.fetch with fakes; keep the real one for the fetch-parsing tests
from PIL import Image

PASS, FAIL = [], []
def test(fn):
    try: fn(); PASS.append(fn.__name__); print(f"  ok   {fn.__name__}")
    except Exception: FAIL.append(fn.__name__); print(f"  FAIL {fn.__name__}\n" + "".join("       " + l for l in traceback.format_exc().splitlines(True)))
    return fn

def iso_in(sec): return (datetime.now(timezone.utc) + timedelta(seconds=sec)).isoformat()
def reset_cfg(**kw):
    for f in (core.CONFIG, core.CACHE, core.FLAG, core.ALERTS, core.TESTFLAG):
        f.unlink(missing_ok=True)
    cfg = dict(core.DEFAULTS, profiles=[{"name": n, "path": str(TMP / n)} for n in ("a", "b", "c")], **kw)
    core.save_config(cfg); return core.load_config()

class FakeIcon:
    def __init__(self): self.toasts, self.icon, self.title = [], None, ""
    def notify(self, msg, title): self.toasts.append((title, msg))

def make_app(**cfgkw):
    cfg = reset_cfg(**cfgkw)
    a = appmod.App.__new__(appmod.App)
    a.cfg, a.data, a.next_at, a.fails, a.busy, a.refresh_info = cfg, {}, {}, {}, set(), None
    a.pending, a.last_toast, a.alert_state, a.icon = [], 0.0, a.load_alert_state(), FakeIcon()
    return a

def fresh(five=10, week=10, reset5=18000, reset7=400000, **extra):
    return {"five": five, "five_reset": iso_in(reset5), "week": week, "week_reset": iso_in(reset7), "fetched_at": time.time(), **extra}

def age_pending(a, sec=10):
    for e in a.pending: e["t"] -= sec

print("== core")
@test
def interval_is_clamped_to_minimum():
    reset_cfg(interval_sec=10); assert core.load_config()["interval_sec"] == core.MIN_INTERVAL == 90
    reset_cfg(interval_sec=500); assert core.load_config()["interval_sec"] == 500

@test
def alias_rules():
    cfg = {"aliases": {"x": "ZZ"}}
    assert core.alias("default (desktop)", cfg) == "DT" and core.alias("claude-ngoai", cfg) == "NG" and core.alias("x", cfg) == "ZZ"

@test
def fmt_reset_formats():
    assert core.fmt_reset(None) == "-" and core.fmt_reset(iso_in(-5)) == "reset"
    assert core.fmt_reset(iso_in(2 * 3600 + 13 * 60 + 30)) == "2h13m" and core.fmt_reset(iso_in(26 * 3600 + 120)).startswith("1d02h")

def profile_dir(name, expires_in=3600):
    d = TMP / name; d.mkdir(exist_ok=True)
    (d / ".credentials.json").write_text(json.dumps({"claudeAiOauth": {"accessToken": "test-token-not-real", "expiresAt": (time.time() + expires_in) * 1000}}))
    return {"name": name, "path": str(d)}

@test
def fetch_maps_http_errors_and_retry_after():
    p = profile_dir("a"); real = core.urllib.request.urlopen
    def boom(code, ra=None):
        def f(*a, **k):
            h = {"Retry-After": ra} if ra else {}
            raise urllib.error.HTTPError("u", code, "x", __import__("email.message").message.Message() if not h else _msg(h), None)
        return f
    def _msg(h):
        import email.message; m = email.message.Message()
        for k, v in h.items(): m[k] = v
        return m
    try:
        core.urllib.request.urlopen = boom(429, "77"); r = core.fetch(p); assert r["status"] == 429 and r["retry_after"] == 77, r
        core.urllib.request.urlopen = boom(429); r = core.fetch(p); assert r["status"] == 429 and r["retry_after"] is None, r
        core.urllib.request.urlopen = boom(500); assert core.fetch(p)["status"] == 500
        core.urllib.request.urlopen = lambda *a, **k: (_ for _ in ()).throw(TimeoutError()); r = core.fetch(p); assert r["status"] == 0 and "err" in r
    finally: core.urllib.request.urlopen = real

@test
def fetch_expired_token_and_success_parse():
    assert core.fetch(profile_dir("a", expires_in=-10))["status"] == 401
    p = profile_dir("a"); real = core.urllib.request.urlopen
    class R(io.BytesIO):
        def __enter__(s): return s
        def __exit__(s, *a): pass
    core.urllib.request.urlopen = lambda *a, **k: R(json.dumps({"five_hour": {"utilization": 12.0, "resets_at": "2026-01-01T00:00:00+00:00"}, "seven_day": {"utilization": 34.0, "resets_at": None}}).encode())
    try: r = core.fetch(p)
    finally: core.urllib.request.urlopen = real
    assert r["five"] == 12.0 and r["week"] == 34.0 and r["week_reset"] is None and "err" not in r

# ---- OAuth refresh (fake server; tokens are dummies) ----
class _Resp(io.BytesIO):
    def __enter__(s): return s
    def __exit__(s, *a): pass

def oauth_profile(name, expires_in=-60, rt="rt-old-not-real", **extra):
    d = TMP / name; d.mkdir(exist_ok=True)
    for l in core._lock_paths(d):
        if l.exists(): l.rmdir()
    o = {"accessToken": "at-old-not-real", "refreshToken": rt, "expiresAt": int((time.time() + expires_in) * 1000),
         "refreshTokenExpiresAt": int((time.time() + 20 * 86400) * 1000), "scopes": ["user:inference", "user:profile"],
         "subscriptionType": "team", **extra}
    (d / ".credentials.json").write_text(json.dumps({"claudeAiOauth": o, "mcpOAuth": {"keep": 1}}), "utf-8")
    core._dead.clear()
    return {"name": name, "path": str(d)}

def fake_token_server(calls, reply=None, errors=()):
    """urlopen replacement: records (url, body); raises queued HTTP errors (code, oauth error) first, then replies."""
    errs = list(errors)
    def f(req, timeout=None):
        calls.append((req.full_url, json.loads(req.data.decode())))
        if errs:
            code, err = errs.pop(0)
            raise urllib.error.HTTPError(req.full_url, code, "x", {}, io.BytesIO(json.dumps({"error": err, "error_description": "secret-ish detail"}).encode()))
        return _Resp(json.dumps(reply or {"access_token": "at-new-not-real", "refresh_token": "rt-new-not-real", "expires_in": 28800,
                                          "refresh_token_expires_in": 90 * 86400, "scope": "user:inference user:profile"}).encode())
    return f

def creds(p): return json.loads((Path(p["path"]) / ".credentials.json").read_text("utf-8"))

@test
def refresh_writes_new_tokens_back_like_the_cli():
    p = oauth_profile("r1"); calls = []; real = core.urllib.request.urlopen; before = creds(p)
    core.urllib.request.urlopen = fake_token_server(calls)
    try: r = core.refresh_if_expired(p)
    finally: core.urllib.request.urlopen = real
    assert r is None, r
    url, body = calls[0]
    assert url == core.TOKEN_URL and body == {"grant_type": "refresh_token", "refresh_token": "rt-old-not-real",
                                              "client_id": core.CLIENT_ID, "scope": "user:inference user:profile"}, body
    after = creds(p); o = after["claudeAiOauth"]
    assert o["accessToken"] == "at-new-not-real" and o["refreshToken"] == "rt-new-not-real"
    assert abs(o["expiresAt"] / 1000 - time.time() - 28800) < 30
    assert o["refreshTokenExpiresAt"] == before["claudeAiOauth"]["refreshTokenExpiresAt"], "refresh never extends the refresh-token expiry"
    assert after["mcpOAuth"] == {"keep": 1} and o["subscriptionType"] == "team", "other fields/keys must be kept"
    text = (Path(p["path"]) / ".credentials.json").read_text("utf-8"); assert ", " not in text and ": " not in text, "compact JSON like the CLI"
    assert json.loads((Path(p["path"]) / ".credentials.json.claude-usage.bak").read_text("utf-8")) == before
    assert not any(l.exists() for l in core._lock_paths(p["path"])), "locks must be released"
    assert not list(Path(p["path"]).glob("*.tmp")), "no temp file left"
    log = core.EVENTS.read_text("utf-8"); assert "token refreshed: r1" in log and "not-real" not in log, "tokens never reach the log"

@test
def refresh_does_nothing_when_not_needed():
    calls = []; real = core.urllib.request.urlopen; core.urllib.request.urlopen = fake_token_server(calls)
    try:
        p = oauth_profile("r2", expires_in=3600); before = creds(p)
        assert core.refresh_if_expired(p) is None and creds(p) == before
        p = oauth_profile("r2", rt=""); assert core.refresh_if_expired(p) is None                  # nothing to refresh with
        assert core.refresh_if_expired({"name": "x", "path": str(TMP / "missing")}) is None
        p = oauth_profile("r2", refreshTokenExpiresAt=int((time.time() - 60) * 1000))
        assert core.refresh_if_expired(p)["dead"], "refresh token past its own expiry: login needed, no request"
    finally: core.urllib.request.urlopen = real
    assert calls == [], calls
    assert core.fetch(oauth_profile("r2"))["status"] == 401, "fetch alone still reports an expired token"

@test
def refresh_waits_for_the_cli_lock_and_takes_over_a_dead_one():
    calls = []; real = core.urllib.request.urlopen; core.urllib.request.urlopen = fake_token_server(calls)
    try:
        for i, lock in enumerate(core._lock_paths(TMP / "r3")):
            p = oauth_profile("r3"); lock.mkdir()
            r = core.refresh_if_expired(p)
            assert r and r["status"] == 0 and r["retry_after"] == 30 and calls == [], (i, r)
            assert lock.exists(), "never remove a live lock held by claude"
            assert creds(p)["claudeAiOauth"]["accessToken"] == "at-old-not-real"
            old = time.time() - core.LOCK_STALE - 30; os.utime(lock, (old, old))          # holder died long ago
            assert core.refresh_if_expired(p) is None and len(calls) == 1, "stale lock is taken over"
            assert not lock.exists() and creds(p)["claudeAiOauth"]["accessToken"] == "at-new-not-real"
            calls.clear()
    finally: core.urllib.request.urlopen = real

@test
def refresh_skips_when_claude_refreshed_meanwhile():
    p = oauth_profile("r4"); calls = []; real, real_acq = core.urllib.request.urlopen, core._acquire
    def acquire_after_cli(paths):                            # claude writes a fresh token just before we get the lock
        d = creds(p); d["claudeAiOauth"].update(accessToken="at-cli-not-real", refreshToken="rt-cli-not-real", expiresAt=int((time.time() + 28800) * 1000))
        (Path(p["path"]) / ".credentials.json").write_text(json.dumps(d), "utf-8")
        return real_acq(paths)
    core.urllib.request.urlopen, core._acquire = fake_token_server(calls), acquire_after_cli
    try: assert core.refresh_if_expired(p) is None
    finally: core.urllib.request.urlopen, core._acquire = real, real_acq
    assert calls == [] and creds(p)["claudeAiOauth"]["refreshToken"] == "rt-cli-not-real"

@test
def rejected_refresh_token_is_not_retried_until_it_changes():
    p = oauth_profile("r5"); calls = []; real = core.urllib.request.urlopen; before = creds(p)
    core.urllib.request.urlopen = fake_token_server(calls, errors=[(400, "invalid_grant")])
    try:
        r = core.refresh_if_expired(p)
        assert r["dead"] and r["status"] == 401 and r["err"] == core.DEAD_MSG and "secret" not in r["err"], r
        assert creds(p) == before, "file untouched on failure"
        assert core.refresh_if_expired(p)["dead"] and len(calls) == 1, "same rejected token must not be sent again"
        d = creds(p); d["claudeAiOauth"]["refreshToken"] = "rt-relogin-not-real"           # user ran /login
        (Path(p["path"]) / ".credentials.json").write_text(json.dumps(d), "utf-8")
        assert core.refresh_if_expired(p) is None and len(calls) == 2
    finally: core.urllib.request.urlopen = real
    assert not any(l.exists() for l in core._lock_paths(p["path"]))

@test
def refresh_errors_invalid_scope_retry_and_transient():
    calls = []; real = core.urllib.request.urlopen
    try:
        p = oauth_profile("r6"); core.urllib.request.urlopen = fake_token_server(calls, errors=[(400, "invalid_scope")])
        assert core.refresh_if_expired(p) is None and "scope" in calls[0][1] and "scope" not in calls[1][1], calls
        for code, err in ((429, "rate_limited"), (500, ""), (403, "forbidden")):
            p = oauth_profile("r6"); before = creds(p)
            core.urllib.request.urlopen = fake_token_server(calls, errors=[(code, err)])
            r = core.refresh_if_expired(p); assert r["status"] == code and not r.get("dead") and creds(p) == before, r
        p = oauth_profile("r6"); core.urllib.request.urlopen = fake_token_server(calls, reply={"weird": 1})
        r = core.refresh_if_expired(p); assert r["status"] == 0 and creds(p)["claudeAiOauth"]["accessToken"] == "at-old-not-real", r
        core.urllib.request.urlopen = lambda *a, **k: (_ for _ in ()).throw(TimeoutError())
        r = core.refresh_if_expired(oauth_profile("r6")); assert r["status"] == 0 and "TimeoutError" in r["err"], r
    finally: core.urllib.request.urlopen = real

@test
def cache_write_is_atomic_under_concurrent_reads():
    cfg = reset_cfg(); data = {"a": fresh()}; stop, bad, werr, writes = False, [], [], [0]
    def writer():
        while not stop:
            try: core.write_cache(cfg, data, {"a"}, None); writes[0] += 1
            except Exception as e: werr.append(repr(e))
            time.sleep(0.002)    # ~250 writes/s: ~1000x harsher than the app (one write every few seconds)
    core.write_cache(cfg, data); t = threading.Thread(target=writer); t.start()
    try:
        for _ in range(1500):
            time.sleep(0.001)                               # real readers poll every >=100 ms; don't starve the writer
            c = core.load_cache()                           # the real reader used by widget/dashboard
            if c is None or c["profiles"]["a"].get("five") != 10: bad.append(c)
            try: json.loads(core.CACHE.read_text("utf-8"))  # raw reader, like the hook
            except FileNotFoundError: pass
            except (json.JSONDecodeError, PermissionError) as e: pass   # raw readers may retry; only load_cache must be solid
    finally: stop = True; t.join()
    assert not werr, f"writer crashed: {werr[:2]}"
    assert writes[0] > 50, f"writer starved ({writes[0]} writes)"
    assert not bad, f"{len(bad)} bad reads through load_cache()"

print("== polling / 429 / waiting indicator")
@test
def backoff_keeps_last_good_value_and_recovers():
    a = make_app(interval_sec=180); p = a.cfg["profiles"][0]
    seq = iter([{"five": 10, "five_reset": None, "week": 20, "week_reset": None}] + [{"err": "HTTP 429", "status": 429, "retry_after": None}] * 2
               + [{"err": "HTTP 429", "status": 429, "retry_after": 300}, {"five": 11, "five_reset": None, "week": 21, "week_reset": None}])
    core.fetch = lambda _p: next(seq); waits = []
    for _ in range(5):
        a.poll_one(p); waits.append(a.next_at["a"] - time.time())
    assert a.data["a"]["five"] == 11 and "err" not in a.data["a"] and a.fails["a"] == 0           # recovered
    assert 100 < waits[1] < 140 and 220 < waits[2] < 280 and 290 < waits[3] < 340, waits            # 120s, 240s, Retry-After
    a2 = make_app(); core.fetch = lambda _p: {"five": 5, "five_reset": None, "week": 6, "week_reset": None}; a2.poll_one(p)
    core.fetch = lambda _p: {"err": "HTTP 429", "status": 429, "retry_after": None}; a2.poll_one(p)
    assert a2.data["a"]["five"] == 5 and a2.data["a"]["err"] == "HTTP 429", "last good value must survive a 429"

@test
def poll_refreshes_expired_token_first_unless_disabled():
    real_ref = core.refresh_if_expired; seen = []
    try:
        core.fetch = lambda _p: (seen.append("fetch"), {"five": 1, "five_reset": None, "week": 1, "week_reset": None})[1]
        core.refresh_if_expired = lambda _p: (seen.append("refresh"), None)[1]
        a = make_app(); a.poll_one(a.cfg["profiles"][0]); assert seen == ["refresh", "fetch"] and a.data["a"]["five"] == 1, seen
        seen.clear(); a = make_app(token_refresh=False); a.poll_one(a.cfg["profiles"][0]); assert seen == ["fetch"], seen
        seen.clear(); a = make_app()
        core.refresh_if_expired = lambda _p: (seen.append("refresh"), {"err": core.DEAD_MSG, "status": 401, "dead": True})[1]
        a.data["a"] = fresh(five=42); a.poll_one(a.cfg["profiles"][0])
        assert seen == ["refresh"], "a failed refresh must not fire the usage request"
        assert a.data["a"]["five"] == 42 and a.data["a"]["err"] == core.DEAD_MSG and a.next_at["a"] - time.time() >= 299
        core.refresh_if_expired = lambda _p: {"err": "claude đang làm mới token, thử lại sau", "status": 0, "retry_after": 30}
        a.poll_one(a.cfg["profiles"][0]); assert 29 < a.next_at["a"] - time.time() < 34, "claude holds the lock: retry soon"
    finally: core.refresh_if_expired = real_ref

@test
def never_polls_faster_than_interval_even_on_success():
    a = make_app(interval_sec=300); core.fetch = lambda _p: {"five": 1, "five_reset": None, "week": 1, "week_reset": None}
    a.poll_one(a.cfg["profiles"][0]); assert a.next_at["a"] - time.time() >= 299

@test
def waiting_indicator_is_visible_during_the_request():
    a = make_app(); seen = {}
    def slow_fetch(p):
        c = core.load_cache(); seen["busy_in_cache"] = list(c["busy"]); seen["icon_busy"] = bool(a.busy)
        return {"five": 1, "five_reset": None, "week": 1, "week_reset": None}
    core.fetch = slow_fetch; a.poll_one(a.cfg["profiles"][0])
    assert seen["busy_in_cache"] == ["a"] and seen["icon_busy"], seen          # UI/widget can see it *while* waiting
    assert a.busy == set(), "busy must clear after the request"
    def raising(p): raise RuntimeError("boom")
    core.fetch = raising
    try: a.poll_one(a.cfg["profiles"][0])
    except RuntimeError: pass
    assert a.busy == set(), "busy must clear even if the request blows up"

@test
def refresh_reports_started_and_skipped_with_reasons():
    a = make_app(); now = time.time()
    a.data = {"a": fresh(), "b": {**fresh(), "fetched_at": now - 100, "retry_at": now + 200, "err": "HTTP 429"}, "c": {**fresh(), "fetched_at": now - 100}}
    a.refresh_requested(); i = a.refresh_info
    assert i["started"] == ["c"] and {s["name"]: s["why"] for s in i["skipped"]}.keys() == {"a", "b"}, i
    assert "429" in [s for s in i["skipped"] if s["name"] == "b"][0]["why"] or "giới hạn" in [s for s in i["skipped"] if s["name"] == "b"][0]["why"]
    assert a.busy == {"c"} and core.load_cache()["refresh"]["started"] == ["c"] and core.load_cache()["busy"] == ["c"]

@test
def restart_does_not_trigger_immediate_api_burst():
    a = make_app(interval_sec=180); a.data = {n: fresh() for n in "abc"}; a.write()
    a2 = appmod.App.__new__(appmod.App); a2.cfg, a2.data, a2.next_at = core.load_config(), {}, {}
    core.write_cache(a.cfg, a.data); a2.seed()
    assert all(a2.next_at[n] > time.time() + 100 for n in "abc"), "fresh cache must delay the first fetch"
    core.CACHE.unlink(); a3 = appmod.App.__new__(appmod.App); a3.cfg, a3.data, a3.next_at = core.load_config(), {}, {}; a3.seed()
    t = sorted(a3.next_at.values()); assert t[1] - t[0] >= 3 and t[2] - t[1] >= 3, "no cache: first fetches must be staggered"

print("== notifications")
def cross(a, five=85, week=10, names=("a",), reset5=18000):
    a.data = {n: fresh(five=five if n in names else 5, week=week, reset5=reset5) for n in "abc"}
    a.alerts({n: d for n, d in a.data.items()})

@test
def one_toast_per_window_even_with_jittery_reset_times():
    a = make_app()
    for i in range(50):                                              # API drifts reset by ~±0.7s between calls
        cross(a, reset5=18000 + (i % 3 - 1) * 0.7)
    assert len(a.pending) == 1, f"expected 1 alert, got {len(a.pending)}"
    age_pending(a); a.flush_toasts(); assert len(a.icon.toasts) == 1
    for _ in range(50): cross(a, reset5=18000 + 0.5); a.flush_toasts()
    assert len(a.icon.toasts) == 1, "repeat polls must not toast again"

@test
def no_repeat_after_restart():
    a = make_app(); cross(a); age_pending(a); a.flush_toasts(); assert len(a.icon.toasts) == 1
    b = appmod.App.__new__(appmod.App); b.cfg, b.pending, b.last_toast, b.icon = a.cfg, [], 0.0, FakeIcon()
    b.alert_state = b.load_alert_state()                              # what a fresh process loads from disk
    cross(b); age_pending(b); b.flush_toasts()
    assert b.icon.toasts == [], "restart must not re-announce the same window"

@test
def new_usage_window_alerts_again():
    a = make_app(); cross(a, reset5=18000); age_pending(a); a.flush_toasts()
    a.last_toast = 0; cross(a, five=10, reset5=18000); cross(a, five=90, reset5=18000 + 5 * 3600)   # 5h window rolled over
    age_pending(a); a.flush_toasts(); assert len(a.icon.toasts) == 2

@test
def master_switch_off_sends_nothing_and_does_not_burst_when_turned_on():
    a = make_app(notify_enabled=False); cross(a); age_pending(a); a.flush_toasts()
    assert a.icon.toasts == [] and a.pending == []
    a.cfg["notify_enabled"] = True; cross(a); age_pending(a); a.flush_toasts()
    assert a.icon.toasts == [], "turning notifications on must not replay conditions that already existed"
    a.last_toast = 0; cross(a, five=96, reset5=18000); age_pending(a); a.flush_toasts()
    assert len(a.icon.toasts) == 1, "a *new* level (block) after enabling must notify"

@test
def warn_and_block_toggles_are_independent():
    a = make_app(notify_warn=False, notify_block=True)
    cross(a, five=85); age_pending(a); a.flush_toasts(); assert a.icon.toasts == [], "warn off"
    cross(a, five=96); age_pending(a); a.flush_toasts(); assert len(a.icon.toasts) == 1 and "CHẶN" in a.icon.toasts[0][0]
    b = make_app(notify_warn=True, notify_block=False)
    cross(b, five=96); age_pending(b); b.flush_toasts(); assert b.icon.toasts == [], "block off"

@test
def simultaneous_alerts_are_merged_into_one_toast_and_rate_limited():
    a = make_app(); cross(a, names=("a", "b", "c"))
    assert len(a.pending) == 3
    a.flush_toasts(); assert a.icon.toasts == [], "must wait to batch"
    age_pending(a); a.flush_toasts()
    assert len(a.icon.toasts) == 1 and "(3 account)" in a.icon.toasts[0][0], a.icon.toasts
    assert a.icon.toasts[0][1].count("\n") == 2 and len(a.icon.toasts[0][1]) < 250, "one line per account, within the toast length limit"
    a.data = {n: fresh(five=96) for n in "abc"}; a.alerts(a.data)       # new level right after: inside the 15s gap
    age_pending(a); a.flush_toasts(); assert len(a.icon.toasts) == 1, "min gap between toasts"
    a.last_toast -= 20; a.flush_toasts(); assert len(a.icon.toasts) == 2

@test
def stale_or_errored_data_never_alerts():
    a = make_app(); a.data = {"a": fresh(five=99, err="HTTP 429")}
    a.publish_alerts = a.alerts; a.alerts({n: d for n, d in a.data.items() if "five" in d and not d.get("err")})
    assert a.pending == []

@test
def first_run_is_silent_then_announces_new_crossings():
    a = make_app(); a.baseline, a.t_start = True, time.time()
    a.data = {"a": fresh(five=92), "b": fresh(five=5)}; a.alerts(dict(a.data))        # 2 of 3 accounts seen: still learning
    assert a.pending == [] and a.baseline is True
    a.data["c"] = fresh(five=85); a.alerts(dict(a.data))                              # all 3 seen
    assert a.pending == [] and a.baseline is False, "already-high accounts must not be announced on first run"
    a.data["b"] = fresh(five=96); a.alerts(dict(a.data))                              # a genuinely new crossing afterwards
    assert len(a.pending) == 1 and a.pending[0]["name"] == "b" and a.pending[0]["lvl"] == "block", a.pending
    assert core.ALERTS.exists(), "baseline must be persisted so the next start is not a 'first run' again"
    b = make_app(); b.baseline, b.t_start = True, time.time() - 500; b.data = {"a": fresh(five=92)}; b.alerts(dict(b.data))
    assert b.baseline is False and b.pending == [], "baseline ends after 2 minutes even if an account never reports"

@test
def alert_message_suggests_the_emptiest_account():
    a = make_app(); a.data = {"a": fresh(five=90), "b": fresh(five=5, week=5), "c": fresh(five=50)}; a.alerts(a.data)
    age_pending(a); a.flush_toasts(); msg = a.icon.toasts[0][1]
    assert "a: 5h 90%" in msg and "Nên dùng: b" in msg, msg

@test
def test_notification_button_works_in_both_states():
    a = make_app(); a.test_toast(); assert "BẬT" in a.icon.toasts[-1][1]
    a.cfg["notify_enabled"] = False; a.test_toast(); assert "TẮT" in a.icon.toasts[-1][1] and len(a.icon.toasts) == 2

@test
def quiet_when_nothing_crosses_a_threshold():
    a = make_app()
    for _ in range(200): cross(a, five=79, week=50); a.flush_toasts()
    assert a.pending == [] and a.icon.toasts == []

print("== hook (exit codes Claude Code relies on)")
def run_hook(cache, cfg_dir, env_extra=None):
    d = TMP / "hookenv" / "claude-usage"; d.mkdir(parents=True, exist_ok=True)
    f = d / "usage_cache.json"; f.unlink(missing_ok=True)
    if cache is not None: f.write_text(cache if isinstance(cache, str) else json.dumps(cache))
    env = {**os.environ, "LOCALAPPDATA": str(TMP / "hookenv"), "CLAUDE_CONFIG_DIR": str(cfg_dir), **(env_extra or {})}
    return subprocess.run([sys.executable, str(ROOT / "main.py"), "hook"], env=env, capture_output=True, timeout=30)

def hc(five, week, ts=None, block=95, block_enabled=True, path="X"):
    return {"ts": ts or time.time(), "block_enabled": block_enabled, "block_pct": block,
            "profiles": {"me": {"path": path, "five": five, "week": week}, "other": {"path": "Y", "five": 1, "week": 1}}}

@test
def hook_allows_below_and_blocks_at_or_above_threshold():
    X = TMP / "X"; X.mkdir(exist_ok=True)
    assert run_hook(hc(50, 50, path=str(X)), X).returncode == 0
    assert run_hook(hc(94.9, 10, path=str(X)), X).returncode == 0
    r = run_hook(hc(95, 10, path=str(X)), X); assert r.returncode == 2 and b"me" in r.stderr, r
    assert run_hook(hc(10, 99, path=str(X)), X).returncode == 2, "7d over threshold must block too"

@test
def hook_fails_open_when_unsure():
    X = TMP / "X"
    assert run_hook(hc(99, 99, path=str(X), block_enabled=False), X).returncode == 0, "blocking disabled"
    assert run_hook(hc(99, 99, ts=time.time() - 3600, path=str(X)), X).returncode == 0, "stale cache"
    assert run_hook(hc(99, 99, path=str(X)), TMP / "unrelated").returncode == 0, "profile not in cache"
    assert run_hook(None, X).returncode == 0, "no cache"
    assert run_hook("{not json", X).returncode == 0, "corrupt cache"

@test
def hook_message_is_valid_utf8_vietnamese():
    X = TMP / "X"; r = run_hook(hc(99, 10, path=str(X)), X); r.stderr.decode("utf-8"); assert "Đổi sang" in r.stderr.decode("utf-8")

print("== hook install / uninstall")
def prof(name): d = TMP / "prof" / name; d.mkdir(parents=True, exist_ok=True); return {"name": name, "path": str(d)}, d / "settings.json"
@test
def install_creates_settings_and_is_idempotent():
    p, f = prof("fresh"); f.unlink(missing_ok=True)
    hookinstall.install([p]); hookinstall.install([p]); hookinstall.install([p])
    d = json.loads(f.read_text()); g = d["hooks"]["UserPromptSubmit"]
    assert len(g) == 1 and g[0]["hooks"][0]["command"].endswith(" hook") and hookinstall.status([p]) == [("fresh", True)]

@test
def install_preserves_existing_settings_and_other_hooks_and_backs_up():
    p, f = prof("rich")
    orig = {"model": "x", "permissions": {"allow": ["Bash(ls)"]}, "hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": "echo hi"}]}],
            "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "my-own-hook.exe"}]}]}}
    f.write_text(json.dumps(orig)); hookinstall.install([p]); hookinstall.install([p])
    d = json.loads(f.read_text())
    assert d["model"] == "x" and d["permissions"] == orig["permissions"] and d["hooks"]["PreToolUse"] == orig["hooks"]["PreToolUse"]
    cmds = [h["command"] for g in d["hooks"]["UserPromptSubmit"] for h in g["hooks"]]
    assert "my-own-hook.exe" in cmds and sum(c.endswith(" hook") and "claude-usage" in c.lower() for c in cmds) == 1, cmds
    bak = f.with_name("settings.json.claude-usage.bak"); assert bak.exists() and json.loads(bak.read_text()) == orig, "original must be backed up untouched"

@test
def uninstall_removes_only_ours_and_cleans_empty_containers():
    p, f = prof("rich2"); f.write_text(json.dumps({"hooks": {"UserPromptSubmit": [{"hooks": [{"type": "command", "command": "my-own-hook.exe"}]}]}}))
    hookinstall.install([p]); hookinstall.uninstall([p])
    assert json.loads(f.read_text()) == {"hooks": {"UserPromptSubmit": [{"hooks": [{"type": "command", "command": "my-own-hook.exe"}]}]}}
    p2, f2 = prof("only"); f2.unlink(missing_ok=True); hookinstall.install([p2]); hookinstall.uninstall([p2])
    assert json.loads(f2.read_text()) == {}, "no empty 'hooks' skeleton left behind"
    assert hookinstall.uninstall([p2])[0][1] == "không có hook"

@test
def corrupt_settings_is_never_overwritten():
    p, f = prof("bad"); f.write_text("{ this is not json"); r = hookinstall.install([p])
    assert f.read_text() == "{ this is not json" and "bỏ qua" in r[0][1] and hookinstall.status([p])[0][1] is None

@test
def repair_rewrites_hook_when_exe_moved():
    p, f = prof("moved"); hookinstall.install([p]); d = json.loads(f.read_text())
    d["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"] = '"C:\\old\\claude-usage.exe" hook'; f.write_text(json.dumps(d))
    hookinstall.repair([p]); cmd = json.loads(f.read_text())["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"]
    assert cmd == hookinstall.command()

print("== widget rendering / geometry")
CACHE_W = {"busy": [], "profiles": {"a": {"five": 40, "week": 60, "five_reset": iso_in(3600), "week_reset": iso_in(86400)},
                                    "b": {"five": 90, "week": 20, "five_reset": iso_in(600), "week_reset": iso_in(8000), "err": "HTTP 429"},
                                    "c": {"err": "HTTP 429", "status": 429}}}
@test
def spinner_only_appears_on_busy_cells_and_animates_in_every_style():
    cfg = core.load_config(); busy = {**CACHE_W, "busy": ["a"]}
    for style in widget.STYLES:
        cfg["widget_style"] = style
        still = lambda c, ph: widget.render(cfg, c, 1.0, -1, True, 48, 36, ph)[0].tobytes()
        assert still(CACHE_W, 0) == still(CACHE_W, 5), f"{style}: no spinner when idle"
        assert still(busy, 0) != still(CACHE_W, 0), f"{style}: spinner must be drawn when busy"
        assert still(busy, 0) != still(busy, 3), f"{style}: spinner must rotate"

@test
def every_style_renders_all_layouts_sizes_and_edge_cases():
    cfg = core.load_config()
    for style in widget.STYLES:
        cfg["widget_style"] = style
        for vertical in (True, False):
            for size in (20, 36, 90):
                for reset in (True, False):
                    cfg["widget_reset"] = reset
                    cv, W, H, cw, ch = widget.render(cfg, CACHE_W, 1.5, -1, vertical, 48, size)
                    assert W > 0 and H > 0 and cw > 0 and ch > 0, (style, vertical, size, reset)
                    for edit in (False, True):
                        for ang in (0, 35, 90, -120):
                            img, geo = widget.compose(cv, W, H, 1.5, edit, ang); assert img.width > 0 and img.height > 0
        widget.render(cfg, None, 1.0, -1, True, 48, 36); widget.render(cfg, {"profiles": {}}, 1.0, -1, False, 0, 36)
        widget.render(cfg, CACHE_W, 1.0, 1, True, 48, 36)          # hover highlight

@test
def styles_look_different_and_unknown_style_falls_back_to_rings():
    cfg = core.load_config(); seen = {}
    for style in widget.STYLES:
        cfg["widget_style"] = style; seen[style] = widget.render(cfg, CACHE_W, 1.0, -1, True, 48, 36)[0].tobytes()
    assert len(set(seen.values())) == len(widget.STYLES), "every style must be visually distinct"
    cfg["widget_style"] = "does-not-exist"; assert widget.render(cfg, CACHE_W, 1.0, -1, True, 48, 36)[0].tobytes() == seen["rings"]

@test
def cards_fit_inside_a_48px_vertical_taskbar():
    cfg = core.load_config()
    for style in widget.STYLES:
        cfg["widget_style"] = style; cv, W, H, cw, ch = widget.render(cfg, CACHE_W, 1.0, -1, True, 48, 36)
        assert W <= 48, f"{style}: widget is {W}px wide, taskbar is 48px"

print("== appearance options (labels, font, plate, theme, hidden, opacity)")
@test
def label_modes_custom_names_and_length_cap():
    cfg = {"aliases": {"claude-da": "Công ty"}, "widget_label": "alias"}
    assert core.alias("claude-da", cfg) == "Công ty" and core.alias("claude-hn", cfg) == "HN" and core.alias("default (desktop)", cfg) == "DT"
    cfg["widget_label"] = "short"
    assert core.alias("claude-hn", cfg) == "hn" and core.alias("claude-ngoai", cfg) == "ngoai" and core.alias("default (desktop)", cfg) == "desk"
    assert core.alias("claude-da", cfg, 4) == "Công", "length cap"
    assert core.alias("claude-da", {"aliases": {"claude-da": "   "}}) == "DA", "blank custom name falls back to automatic"

@test
def long_labels_are_shrunk_then_cut_with_ellipsis_never_overflowing():
    f, t = widget.fit_label("segoeuib.ttf", 40, "A very long account name", 120)
    assert t.endswith("…") and f.getlength(t) <= 120, (t, f.getlength(t))
    f, t = widget.fit_label("segoeuib.ttf", 20, "ok", 200); assert t == "ok"
    f = widget.fit_font("segoeui.ttf", 40, "wide text", 100)
    assert f.getlength("wide text") <= 100 and f.size < 40, (f.size, f.getlength("wide text"))

@test
def appearance_options_change_the_picture_in_every_style():
    base = core.load_config(); base["widget_reset"] = True
    def px(style, **kw): return widget.render(dict(base, widget_style=style, **kw), CACHE_W, 1.0, -1, True, 48, 36)[0].tobytes()
    for style in widget.STYLES:
        ref = px(style)
        for kw in ({"widget_plate": True}, {"widget_theme": "light"}, {"widget_font": 130}, {"widget_label": "short"}, {"aliases": {"a": "Mine", "b": "Two"}}):
            assert px(style, **kw) != ref, f"{style}: {kw} had no visible effect"
        assert px(style, widget_theme="dark") != px(style, widget_theme="light"), f"{style}: forced themes must differ"

@test
def hidden_accounts_disappear_from_the_widget_and_empty_set_is_safe():
    cfg = core.load_config()
    for style in widget.STYLES:
        cfg["widget_style"] = style
        full = widget.render(cfg, CACHE_W, 1.0, -1, True, 48, 36)[2]
        two = widget.render(dict(cfg, widget_hidden=["a"]), CACHE_W, 1.0, -1, True, 48, 36)[2]
        assert two < full * 0.8, f"{style}: hiding 1 of 3 accounts must shorten the widget ({full} -> {two})"
        none = widget.render(dict(cfg, widget_hidden=["a", "b", "c"]), CACHE_W, 1.0, -1, True, 48, 36); assert none[1] > 0 and none[2] > 0

@test
def extreme_appearance_settings_still_fit_a_48px_taskbar():
    cfg = dict(core.load_config(), widget_font=150, widget_label="short", widget_plate=True, widget_reset=True, aliases={"a": "Tên rất dài của tôi", "b": "WWWWWWWWWW"})
    for style in widget.STYLES:
        cfg["widget_style"] = style
        for vertical in (True, False):
            cv, W, H, cw, ch = widget.render(cfg, CACHE_W, 1.0, -1, vertical, 48, 36)
            if vertical: assert W <= 48, f"{style}: {W}px wide at 150% font with long labels"
        img, _ = widget.compose(cv, W, H, 1.0, True, 33); assert img.width > 0

@test
def opacity_and_font_values_are_clamped_and_tolerate_garbage():
    assert widget.opacity_alpha({"widget_opacity": 100}) == 255 and widget.opacity_alpha({"widget_opacity": 0}) == int(255 * 0.3)
    assert widget.opacity_alpha({"widget_opacity": 9999}) == 255 and widget.opacity_alpha({"widget_opacity": "abc"}) == 255 and widget.opacity_alpha({}) == 255
    assert widget.font_scale({"widget_font": 1}) == 0.8 and widget.font_scale({"widget_font": 999}) == 1.5 and widget.font_scale({"widget_font": None}) == 1.0

print("== what-to-show checkboxes / hover card / extra usage data")
import itertools, shutil
@test
def extra_usage_and_scoped_limits_are_parsed_from_the_api_payload():
    assert core.parse_extra({"is_enabled": True, "monthly_limit": 2500, "used_credits": 98.0, "utilization": 3.92, "currency": "USD", "decimal_places": 2}) == {"used": 0.98, "limit": 25.0, "pct": 3.92, "currency": "USD"}
    e = core.parse_extra({"is_enabled": True, "monthly_limit": 2500, "used_credits": 0.0, "utilization": None}); assert e["pct"] == 0 and e["used"] == 0
    assert core.parse_extra(None) is None and core.parse_extra({"is_enabled": False, "monthly_limit": 5}) is None and core.parse_extra("junk") is None
    sc = core.parse_scoped([{"kind": "session", "percent": 5}, {"kind": "weekly_scoped", "percent": 21, "resets_at": "x", "scope": {"model": {"display_name": "Fable"}}}, {"kind": "weekly_scoped", "scope": None}, "junk"])
    assert sc == [{"name": "Fable", "pct": 21, "resets_at": "x"}], sc

@test
def fetch_returns_credit_scoped_and_plan():
    p = profile_dir("a"); real = core.urllib.request.urlopen
    class R(io.BytesIO):
        def __enter__(s): return s
        def __exit__(s, *a): pass
    body = {"five_hour": {"utilization": 12.0, "resets_at": "r5"}, "seven_day": {"utilization": 34.0, "resets_at": "r7"},
            "extra_usage": {"is_enabled": True, "monthly_limit": 2500, "used_credits": 98.0, "utilization": 3.92, "currency": "USD", "decimal_places": 2},
            "limits": [{"kind": "weekly_scoped", "percent": 21, "resets_at": "rs", "scope": {"model": {"display_name": "Fable"}}}]}
    core.urllib.request.urlopen = lambda *a, **k: R(json.dumps(body).encode())
    try: r = REAL_FETCH(p)
    finally: core.urllib.request.urlopen = real
    assert r["extra"]["used"] == 0.98 and r["scoped"][0]["name"] == "Fable" and "five" in r and "plan" in r

@test
def show_flags_defaults_master_switch_and_never_empty():
    assert widget.shows({}) == {"label": True, "s5": True, "s7": True, "r5": True, "r7": True}
    assert [m[0] for m in widget.metrics({"widget_show_5h": False})] == ["7d"] and [m[0] for m in widget.reset_lines({"widget_show_reset5": False})] == ["7d"]
    assert widget.reset_lines({"widget_reset": False}) == [], "legacy master switch hides both reset lines"
    s = widget.shows({"widget_show_label": False, "widget_show_5h": False, "widget_show_7d": False}); assert s["label"] is True, "never an empty widget"

@test
def every_combination_of_checkboxes_renders_in_every_style_and_fits_the_taskbar():
    base = core.load_config(); keys = ("widget_show_label", "widget_show_5h", "widget_show_7d", "widget_show_reset5", "widget_show_reset7")
    n = 0
    for style in widget.STYLES:
        for combo in itertools.product((True, False), repeat=5):
            cfg = dict(base, widget_style=style, **dict(zip(keys, combo)))
            for vertical in (True, False):
                cv, W, H, cw, ch = widget.render(cfg, CACHE_W, 1.0, -1, vertical, 48, 36)
                assert W > 0 and H > 0 and (not vertical or W <= 48), (style, combo, vertical, W)
                n += 1
            img, _ = widget.compose(cv, W, H, 1.0, True, 20); assert img.width > 0
    assert n == 5 * 32 * 2

@test
def each_checkbox_changes_the_picture_and_fewer_items_means_a_smaller_widget():
    base = core.load_config()
    def px(style, **kw): return widget.render(dict(base, widget_style=style, **kw), CACHE_W, 1.0, -1, True, 48, 36)
    for style in widget.STYLES:
        ref = px(style)[0].tobytes()
        for key in ("widget_show_label", "widget_show_5h", "widget_show_7d", "widget_show_reset5", "widget_show_reset7"):
            assert px(style, **{key: False})[0].tobytes() != ref, f"{style}: unticking {key} changed nothing"
        full_h = px(style)[2]; lean_h = px(style, widget_show_reset5=False, widget_show_reset7=False)[2]
        assert lean_h < full_h, f"{style}: hiding the reset lines must shorten the widget ({full_h} -> {lean_h})"
        assert px(style, widget_show_label=False, widget_show_7d=False, widget_reset=False)[2] <= full_h

TIP_P = {"five": 90, "week": 37, "five_reset": iso_in(8000), "week_reset": iso_in(140000), "fetched_at": time.time() - 12, "plan": "team",
         "extra": {"used": 0.98, "limit": 25.0, "pct": 3.92, "currency": "USD"}, "scoped": [{"name": "Fable", "pct": 21, "resets_at": iso_in(140000)}]}
@test
def hover_card_renders_every_state_and_shows_credit_when_available():
    cfg = core.load_config()
    full = widget.render_tip("claude-hn", TIP_P, cfg, 1.5)
    no_extra = widget.render_tip("claude-hn", dict(TIP_P, extra=None, scoped=[]), cfg, 1.5)
    stale = widget.render_tip("claude-hn", dict(TIP_P, err="HTTP 429", retry_at=time.time() + 200), cfg, 1.5)
    bad = widget.render_tip("x", {"err": "token hết hạn", "status": 401}, cfg, 1.5)
    light = widget.render_tip("claude-hn", TIP_P, dict(cfg, widget_theme="light"), 1.5)
    assert full.height > no_extra.height, "credit + per-model limit rows must add height"
    assert bad.height < no_extra.height and all(i.width > 100 for i in (full, no_extra, stale, bad, light))
    assert light.tobytes() != full.tobytes()
    widget.render_tip("a-very-long-account-name-that-keeps-going-and-going", {}, cfg, 1.0)        # no data at all: must not crash

@test
def reset_time_text_and_money_formatting():
    assert widget.fmt_when(None) == "-"
    soon = widget.fmt_when(iso_in(600)); assert len(soon) >= 5 and ":" in soon
    assert "ngày mai" in widget.fmt_when(iso_in(26 * 3600)) or widget.fmt_when(iso_in(26 * 3600)).count("/") == 1
    far = widget.fmt_when(iso_in(5 * 86400)); assert "/" in far and any(d in far for d in widget.VI_DAYS)
    assert widget.money(0.98, "USD") == "$0.98" and widget.money(1234.5, "USD") == "$1,234.50" and widget.money(5, "VND") == "5.00 VND"
    assert widget.ago_text(time.time() - 5) == "5s trước" and "phút" in widget.ago_text(time.time() - 600) and widget.ago_text(None) == "chưa có"

@test
def hover_card_placement_stays_on_screen_and_beside_the_widget():
    screen = (1920, 1200)
    x, y = widget.tip_position((1850, 600, 44, 292), 700, (250, 300), screen)          # widget at the right edge -> card on its left
    assert x + 250 <= 1850 and 0 <= y <= 1200 - 300
    x, y = widget.tip_position((10, 600, 44, 292), 700, (250, 300), screen); assert x >= 54, "widget at the left edge -> card on its right"
    x, y = widget.tip_position((1850, 600, 44, 292), 5, (250, 300), screen); assert y >= 0, "never above the top"
    x, y = widget.tip_position((1850, 600, 44, 292), 1195, (250, 300), screen); assert y + 300 <= 1200, "never below the bottom"
    x, y = widget.tip_position((1000, 600, 44, 292), 700, (3000, 300), screen); assert x >= 0, "card wider than screen still clamps"

@test
def dashboard_page_scripts_parse_as_javascript():
    node = shutil.which("node")
    if not node: print("       (node not installed: skipped)"); return
    html = (ROOT / "ui.html").read_text("utf-8"); import re
    for i, src in enumerate(re.findall(r"<script>([\s\S]*?)</script>", html)):
        r = subprocess.run([node, "-e", "new Function(require('fs').readFileSync(0,'utf8'))"], input=src.encode("utf-8"), capture_output=True, timeout=60)
        assert r.returncode == 0, f"script {i}: " + r.stderr.decode("utf-8", "replace")[:200].encode("ascii", "replace").decode()

print("== many profiles")
MANY = ["default (desktop)", "claude-da", "claude-hn", "claude-ngoai"] + [f"claude-team-{i:02d}" for i in range(1, 11)]
def many_cache(n=14, busy=()):
    return {"busy": list(busy), "profiles": {nm: {"five": (i * 17) % 100, "week": (i * 23) % 100, "five_reset": iso_in(3600 * (i % 5 + 1)), "week_reset": iso_in(86400 * (i % 6 + 1))}
                                              for i, nm in enumerate(MANY[:n])}}

@test
def automatic_labels_are_unique_even_with_many_similar_profile_names():
    lab = core.aliases(MANY + ["claude-hn2", "claude-hnx"], {})
    assert len(set(lab.values())) == len(lab), lab
    assert lab["default (desktop)"] == "DT" and lab["claude-team-07"] == "T07" and lab["claude-ngoai"] == "NG" and lab["claude-da"] == "DA"
    assert core.aliases(["claude-a", "claude-a-b", "claude-ab"], {}).values().__len__() == 3
    cust = core.aliases(["claude-da", "claude-hn"], {"aliases": {"claude-da": "Công ty"}}, 4); assert cust == {"claude-da": "Công", "claude-hn": "HN"}
    assert core.alias("claude-team-03", {}) == "T03" and core.aliases([], {}) == {}

@test
def plan_layout_shrinks_then_cuts_and_respects_user_cap():
    m = lambda sz: sz * 2.0                         # fake: one cell is 2 px per unit of size
    cfg = dict(core.load_config(), widget_size=36, widget_max_cells=0)
    items = [(n, {"five": 1, "week": 1}) for n in MANY]
    sz, shown = widget.plan_layout(cfg, items[:3], 1.0, True, 48, 600, m); assert (sz, shown) == (36, 3), "few accounts: untouched"
    sz, shown = widget.plan_layout(cfg, items[:10], 1.0, True, 48, 600, m); assert 21.6 <= sz < 36 and shown == 10 and m(sz) * 10 <= 600, (sz, shown)
    sz, shown = widget.plan_layout(cfg, items, 1.0, True, 48, 600, m)
    assert abs(sz - 21.6) < 0.2 and 1 <= shown < 14 and m(sz) * (shown + 1) <= 600 + 1, (sz, shown)    # floor 60%, then 'N + the +N cell'
    sz, shown = widget.plan_layout(dict(cfg, widget_max_cells=5), items, 1.0, True, 48, 600, m); assert (sz, shown) == (36, 5), "user cap"
    assert widget.plan_layout(cfg, [], 1.0, True, 48, 600, m)[1] == 0
    sz, shown = widget.plan_layout(cfg, items, 1.0, True, 48, 50, m); assert shown >= 1, "never zero cells"

@test
def many_profiles_never_outgrow_the_limit_in_any_style_and_layout():
    cache = many_cache(14)
    for style in widget.STYLES:
        for vertical in (True, False):
            cfg = dict(core.load_config(), widget_style=style, widget_sort="usage")
            limit = widget.stack_limit(cfg, vertical, True, (1920, 1200))
            items = widget.visible_profiles(cfg, cache)
            size, shown = widget.plan_layout(cfg, items, 1.0, vertical, 48, limit)
            view = {"profiles": widget.apply_plan(items, shown)}
            cv, W, H, cw, ch = widget.render(cfg, view, 1.0, -1, vertical, 48, size)
            along = H if vertical else W
            assert along <= limit * 1.03, f"{style} vertical={vertical}: {along}px > limit {limit}"
            assert (shown == 14) == (widget.MORE not in view["profiles"]), "the +N cell appears exactly when something was cut"
            if vertical: assert W <= 48
            img, _ = widget.compose(cv, W, H, 1.0, True, 25); assert img.width > 0

@test
def usage_sort_puts_the_busiest_accounts_first_so_they_survive_the_cut():
    cache = many_cache(14); cfg = dict(core.load_config(), widget_sort="usage")
    items = widget.visible_profiles(cfg, cache); worst = [max(p["five"], p["week"]) for _, p in items]
    assert worst == sorted(worst, reverse=True), worst
    assert [n for n, _ in widget.visible_profiles(dict(cfg, widget_sort="config"), cache)] == MANY
    assert [n for n, _ in widget.visible_profiles(dict(cfg, widget_hidden=[MANY[0]]), cache)].count(MANY[0]) == 0
    broken = {"profiles": {"a": {"err": "x"}, "b": {"five": 5, "week": 6}}}; assert [n for n, _ in widget.visible_profiles(cfg, broken)] == ["b", "a"], "accounts without data go last"

@test
def plus_n_cell_and_its_hover_card_render():
    cfg = core.load_config(); hidden = [(n, p) for n, p in many_cache(14)["profiles"].items()][8:]
    for style in widget.STYLES:
        for vertical in (True, False):
            v = {"profiles": {**dict(list(many_cache(14)["profiles"].items())[:3]), widget.MORE: {"more": 11}}}
            cv, W, H, *_ = widget.render(dict(cfg, widget_style=style), v, 1.0, 3, vertical, 48, 36); assert W > 0 and H > 0
    tip = widget.render_more_tip(hidden, cfg, 1.5); assert tip.width > 100 and tip.height > 60
    many = widget.render_more_tip([(f"n{i}", {"five": 1, "week": 1}) for i in range(60)], cfg, 1.0); assert many.height < 400, "very long lists are capped"
    widget.render_more_tip([], cfg, 1.0)

@test
def toast_lists_at_most_four_accounts_and_summarises_the_rest():
    a = make_app(); a.data = {n: fresh(five=90) for n in MANY[:9]}; a.alerts(dict(a.data)); age_pending(a); a.flush_toasts()
    title, body = a.icon.toasts[0]; lines = body.split(chr(10))
    assert "(9 account)" in title and "+5 account khác" in body, (title, body)
    assert sum(1 for l in lines if l.startswith(("claude", "default"))) == 4 and len(body) < 250, body

@test
def poll_interval_widens_with_many_accounts_and_tolerates_junk():
    assert core.effective_interval({"interval_sec": 10, "profiles": []}) == 90
    assert core.effective_interval({"interval_sec": 180, "profiles": [1] * 14}) == 180
    assert core.effective_interval({"interval_sec": 180, "profiles": [1] * 60}) == 300
    assert core.effective_interval({"interval_sec": "junk"}) == 180 and core.effective_interval({}) == 180

@test
def a_poll_round_with_many_accounts_spreads_requests_one_per_tick():
    a = make_app(); a.cfg["profiles"] = [{"name": n, "path": "x"} for n in MANY]
    for i, p in enumerate(a.cfg["profiles"]): a.next_at[p["name"]] = 0.0
    calls = []; core.fetch = lambda p: (calls.append(p["name"]), {"five": 1, "five_reset": None, "week": 1, "week_reset": None})[1]
    due = lambda: [p for p in a.cfg["profiles"] if time.time() >= a.next_at.get(p["name"], 0)]
    ticks = 0
    while due() and ticks < 30:                                    # the loop in App.loop(): at most ONE request per tick
        a.poll_one(due()[0]); ticks += 1
    assert ticks == 14 and len(set(calls)) == 14, (ticks, len(calls))
    assert all(a.next_at[n] > time.time() + 150 for n in MANY), "after a round nobody is due again for a full interval"

@test
def stack_limit_defaults_and_override():
    cfg = {}
    assert widget.stack_limit(cfg, True, True, (1920, 1200)) == 600 and widget.stack_limit(cfg, False, True, (1920, 1200)) == 960
    assert widget.stack_limit(cfg, True, False, (1920, 1200)) == 1080, "free-floating widget may use most of the screen"
    assert widget.stack_limit({"widget_max_len": 321}, True, True, (1920, 1200)) == 321 and widget.stack_limit({"widget_max_len": "junk"}, True, True, (1920, 1200)) == 600

@test
def rotation_hit_test_roundtrip():
    import math
    w = widget.Widget.__new__(widget.Widget); cfg = core.load_config(); w.cfg = dict(cfg, widget_locked=False); w.scale = 1.0
    cv, W, H, cw, ch = widget.render(cfg, CACHE_W, 1.0, -1, True, 48, 36)
    for ang in (0, 30, 90, 145, -60):
        w.img, w.geo = widget.compose(cv, W, H, 1.0, True, ang); w.built_angle = ang
        for name in ("rot", "size", "lock"):
            fx, fy = w.geo[name]; g = w.geo
            vx, vy = w.rot(fx - g["Wp"] / 2, fy - g["Hp"] / 2, ang)           # frame -> window client coords
            cx, cy = vx + w.img.width / 2, vy + w.img.height / 2
            assert w.hit(round(cx), round(cy)) == name, (ang, name)
        vx, vy = w.rot(0, 0, ang); assert w.hit(round(w.img.width / 2), round(w.img.height / 2)) in ("body", None)

@test
def dpi_scaled_rendering_stays_proportional():
    cfg = core.load_config(); a = widget.render(cfg, CACHE_W, 1.0, -1, True, 48, 36); b = widget.render(cfg, CACHE_W, 2.0, -1, True, 96, 36)
    assert abs(b[2] / a[2] - 2) < 0.1

print(f"\n{len(PASS)} passed, {len(FAIL)} failed" + (": " + ", ".join(FAIL) if FAIL else ""))
sys.exit(1 if FAIL else 0)

