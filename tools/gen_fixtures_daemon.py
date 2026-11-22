"""Generate the Rust parity fixture for the daemon logic (poll scheduler, alerts/toasts, tray tooltip/icon state)
by running the real Python `app.App` methods in a sandbox.

    .venv\\Scripts\\python tools\\gen_fixtures_daemon.py

Writes rust/crates/core/tests/fixtures/daemon.json (compared by rust/crates/core/tests/parity_daemon.rs).

Sandbox rules (same as gen_fixtures*.py): LOCALAPPDATA and USERPROFILE point to fresh temp dirs BEFORE core/app
are imported; nothing real is read or written; no window, tray, widget or network is created.
`app.App` is built with `App.__new__` (attributes set like `__init__` does, minus tray/widget). Time, randomness,
fetch and token refresh are replaced by a fake clock, a fixed random sequence and scripted results. The per-tick
logic is exercised by running the real `App.loop` for exactly one iteration (the 1 s sleep raises StopLoop).

Every step records the ordered side effects ("events": fetch/refresh calls, cache writes, icon updates, log
lines, toasts, alerts.json saves) and the full observable state afterwards.
"""
import copy
import json
import os
import sys
import tempfile
from pathlib import Path

_TMP = tempfile.mkdtemp(prefix="cu-fixtures-daemon-")
os.environ["LOCALAPPDATA"] = os.path.join(_TMP, "localappdata")
HOME = os.path.join(_TMP, "home")
os.makedirs(HOME)
os.environ["USERPROFILE"] = HOME
os.environ.pop("HOMEDRIVE", None)
os.environ.pop("HOMEPATH", None)
os.environ.pop("CLAUDE_CONFIG_DIR", None)

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

import core  # noqa: E402
import app as appmod  # noqa: E402

assert str(core.DATA).startswith(_TMP), "core.DATA is not sandboxed"
assert str(appmod.SHOW).startswith(_TMP), "app.SHOW is not sandboxed"
OUT = ROOT / "rust" / "crates" / "core" / "tests" / "fixtures" / "daemon.json"

T0 = 1_700_000_000.0
DEFAULT_RAND = [0.07, 0.5, 0.93, 0.31, 0.0, 0.999, 0.25, 0.6]


# ------------------------------------------------------------------------------------------------ fakes
class ScriptError(BaseException):
    """A scenario bug (unscripted fetch): must not be swallowed by the loop's `except Exception`."""


class StopLoop(BaseException):
    pass


class FakeTime:
    def __init__(self):
        self.t = T0

    def time(self):
        return self.t

    def sleep(self, _s):
        raise StopLoop()


class FakeRandom:
    def __init__(self):
        self.seq, self.used = DEFAULT_RAND, 0

    def random(self):
        v = self.seq[self.used % len(self.seq)]
        self.used += 1
        return v


class FakeIcon:
    def __init__(self):
        self.icon, self.title = None, ""

    def notify(self, msg, title):
        EV.append(["toast", title, msg])


Path_ = type(Path())


class RecPath(Path_):
    """core.ALERTS: records every write as an event."""

    def write_text(self, data, *a, **k):
        EV.append(["alerts_save", json.loads(data)])
        return super().write_text(data, *a, **k)


CLOCK, RND = FakeTime(), FakeRandom()
appmod.time, appmod.random = CLOCK, RND
EV = []
FQ, RQ, DEFAULT_FETCH = [], [], [None]
core.ALERTS = RecPath(str(core.ALERTS))
CACHE_IN = [None]
core.load_cache = lambda: copy.deepcopy(CACHE_IN[0])
core.log = lambda m: EV.append(["log", m])
_real_write_cache = core.write_cache


def _write_cache(cfg, data, busy=(), refresh=None):
    _real_write_cache(cfg, data, busy, refresh)
    out = json.loads(core.CACHE.read_text("utf-8"))
    out.pop("ts")
    EV.append(["write", out])


core.write_cache = _write_cache


def _pop(q, name, kind, default, must):
    EV.append([kind, name])
    if q and q[0]["name"] == name:
        e = q.pop(0)
        CLOCK.t += e.get("advance", 0.0)
        return copy.deepcopy(e["res"])
    if default is not None:
        return copy.deepcopy(default)
    if must:
        raise ScriptError(f"unscripted {kind} for {name}")
    return None


core.fetch = lambda p: _pop(FQ, p["name"], "fetch", DEFAULT_FETCH[0], True)
core.refresh_if_expired = lambda p: _pop(RQ, p["name"], "refresh", None, False)


def tray_of(pct, busy, cfg, title):
    """Decisions of App.make_icon (no drawing), replicated from its code."""
    c = "#6b6b6b" if pct is None else appmod.color(pct, cfg)
    t = "?" if pct is None else "!!" if pct >= 100 else str(int(pct))
    bar = None if not pct else 44 + max(28, (256 - 88) * min(pct, 100) / 100)
    return ["icon", title, pct, c, t, bar, busy]


# ------------------------------------------------------------------------------------------------ runner
FILECFG = {}
MTIME = [1000]


def write_config(overrides):
    FILECFG.update(overrides)
    if "profile_names" in FILECFG:
        FILECFG["profiles"] = [{"name": n, "path": "C:\\fake\\" + n} for n in FILECFG.pop("profile_names")]
    core.save_config(FILECFG)
    MTIME[0] += 1
    os.utime(core.CONFIG, (MTIME[0], MTIME[0]))


def make_app(sc):
    for f in (core.CONFIG, core.CACHE, core.FLAG, core.ALERTS, core.TESTFLAG, appmod.SHOW):
        f.unlink(missing_ok=True)
    FILECFG.clear()
    CLOCK.t, RND.seq, RND.used = sc["t0"], sc["rand"], 0
    FQ.clear(), RQ.clear()
    DEFAULT_FETCH[0] = sc["default_fetch"]
    CACHE_IN[0] = None
    write_config({"profile_names": sc["profiles"], **sc["cfg"]})
    cfg0 = core.load_config()
    if sc["alerts"] is not None:
        core.ALERTS.write_text(json.dumps(sc["alerts"]), "utf-8")
    EV.clear()
    a = appmod.App.__new__(appmod.App)
    a.cfg, a.cfg_mtime = cfg0, 0
    a.data, a.next_at, a.fails, a.busy, a.refresh_info = {}, {}, {}, set(), None
    a.pending, a.last_toast = [], 0.0
    a.baseline = not core.ALERTS.exists()
    a.t_start = CLOCK.t
    a.alert_state = a.load_alert_state()
    a.icon = FakeIcon()
    a.make_icon = lambda pct, busy=False: (pct, busy)
    real_refresh_icon = appmod.App.refresh_icon

    def refresh_icon():
        real_refresh_icon(a)
        pct, busy = a.icon.icon
        EV.append(tray_of(pct, busy, a.cfg, a.icon.title))

    a.refresh_icon = refresh_icon
    return a, cfg0


def run_tick(a, step):
    """One real iteration of App.loop. Returns the loaded config if it was reloaded."""
    if step.get("flag_refresh"):
        core.FLAG.write_text("1")
    if step.get("flag_test"):
        core.TESTFLAG.write_text("1")
    assert not appmod.SHOW.exists()
    m = core.CONFIG.stat().st_mtime if core.CONFIG.exists() else 0
    reload = m != a.cfg_mtime
    first = [True]
    real_publish = appmod.App.publish

    def publish():
        if first[0]:                      # the `self.seed(); self.publish()` at the top of loop()
            first[0] = False
            return
        real_publish(a)

    reached, err = [False], []
    real_flush = appmod.App.flush_toasts

    def flush():
        reached[0] = True
        try:
            real_flush(a)
        except Exception as e:  # noqa: BLE001
            err.append(e)
            raise

    a.seed, a.publish, a.flush_toasts = (lambda: None), publish, flush
    try:
        a.loop()
    except StopLoop:
        pass
    finally:
        del a.seed, a.publish, a.flush_toasts
    assert reached[0] and not err, "an exception was swallowed inside App.loop"
    return copy.deepcopy(a.cfg) if reload else None


def observe(a):
    j = lambda v: json.loads(json.dumps(v))  # noqa: E731  (deep copy + JSON-only check, keeps order)
    return {
        "now": CLOCK.t,
        "rand_used": RND.used,
        "data": j(a.data),
        "next_at": j(dict(sorted(a.next_at.items()))),
        "fails": j(dict(sorted(a.fails.items()))),
        "busy": sorted(a.busy),
        "refresh_info": j(a.refresh_info),
        "pending": j(a.pending),
        "last_toast": a.last_toast,
        "baseline": a.baseline,
        "alert_state": j(a.alert_state),
    }


def run_scenario(sc):
    a, cfg0 = make_app(sc)
    steps_out = []
    for st in sc["steps"]:
        CLOCK.t += st.get("dt", 0.0)
        EV.clear()
        FQ.extend(copy.deepcopy(st.get("fetch", [])))
        RQ.extend(copy.deepcopy(st.get("refresh", [])))
        rec = {k: v for k, v in st.items() if k != "set"}
        op = st["op"]
        if op == "start":
            CACHE_IN[0] = copy.deepcopy(st.get("cache"))
            a.seed()
            a.publish()
        elif op == "tick":
            if "set" in st:
                write_config(st["set"])
            rec["config"] = run_tick(a, st)
        elif op == "refresh":
            a.refresh_requested()
        elif op == "test_toast":
            a.test_toast()
        elif op == "flush":
            a.flush_toasts()
        elif op == "publish":
            a.publish()
        elif op == "set_data":
            a.data = copy.deepcopy(st["data"])
        else:
            raise ValueError(op)
        rec["events"] = copy.deepcopy(EV)
        rec["state"] = observe(a)
        steps_out.append(rec)
    if DEBUG[0]:
        print("LEFTOVER", FQ, RQ)
    else:
        assert not FQ and not RQ, f"{sc['name']}: unused scripted results {FQ} {RQ}"
    return {
        "name": sc["name"], "t0": sc["t0"], "rand": sc["rand"], "cfg": cfg0, "alerts": sc["alerts"],
        "default_fetch": sc["default_fetch"], "steps": steps_out,
    }


# ------------------------------------------------------------------------------------------------ DSL
R5 = "2026-01-01T10:00:00+00:00"
R7 = "2026-01-05T10:00:00+00:00"


def ok(five=10, week=10, r5=R5, r7=R7, **extra):
    return {"five": five, "five_reset": r5, "week": week, "week_reset": r7, **extra}


def err(msg="HTTP 500", status=500, **kw):
    return {"err": msg, "status": status, **kw}


def e429(retry_after=None):
    return {"err": "HTTP 429", "status": 429, "retry_after": retry_after}


def F(name, res, advance=0.0):
    e = {"name": name, "res": res}
    if advance:
        e["advance"] = advance
    return e


def start(cache=None, **kw):
    return {"op": "start", "cache": cache, **kw}


def tick(dt=0.0, **kw):
    return {"op": "tick", "dt": dt, **kw}


def ticks(n, dt=1.0, **kw):
    return [tick(dt, **kw) for _ in range(n)]


def op(name, dt=0.0, **kw):
    return {"op": name, "dt": dt, **kw}


def flat(*parts):
    out = []
    for p in parts:
        out.extend(p if isinstance(p, list) else [p])
    return out


SCEN = []
DEBUG = [False]


def scen(name, profiles, steps, cfg=None, alerts=None, rand=None, default_fetch=None, t0=T0):
    SCEN.append({"name": name, "profiles": profiles, "steps": flat(*steps), "cfg": cfg or {}, "alerts": alerts,
                 "rand": rand or DEFAULT_RAND, "default_fetch": default_fetch, "t0": t0})


def old_cache(profiles):
    """Every profile fetched long ago: all due immediately (one per tick)."""
    return cache_of(profiles, **{n: {**ok(1, 1), "fetched_at": T0 - 100000.0} for n in profiles})


def cache_of(profiles, **by):
    """A usage_cache.json-like Value: profiles = {name: {path, ...}}."""
    return {"ts": T0, "profiles": {n: {"path": "C:\\fake\\" + n, **by.get(n, {})} for n in profiles}}


ABC = ["a", "b", "c"]
D10 = ok(10, 10)

# ---- start / seed / staggering
scen("first_run_no_cache_stagger", ABC, [start(), ticks(14, 1.0)], default_fetch=D10, alerts={})
scen("first_run_with_fresh_cache", ABC, [
    start(cache_of(ABC, **{n: {**ok(30, 40), "fetched_at": T0 - 10} for n in ABC})),
    ticks(3, 1.0), tick(170.0), ticks(4, 1.0)], default_fetch=D10, alerts={})
scen("cache_old_retry_and_partial", ABC, [
    start(cache_of(ABC,
                   a={**ok(30, 40), "fetched_at": T0 - 1000},
                   b={**ok(31, 41), "fetched_at": T0 - 100, "err": "HTTP 429", "status": 429, "retry_at": T0 + 600},
                   c={"err": "HTTP 500", "status": 500, "retry_at": T0 + 50})),
    tick(0.0), tick(1.0), tick(1.0), tick(700.0), ticks(3, 1.0)], default_fetch=D10, alerts={})
scen("cache_fetched_at_zero_and_missing_profile", ABC, [
    start({"profiles": {"a": {"path": "x", **ok(1, 2), "fetched_at": 0}, "b": {"path": "x", **ok(1, 2), "fetched_at": T0 - 5},
                        "zzz": {"path": "x", **ok(9, 9), "fetched_at": T0}}}),
    ticks(3, 3.0)], default_fetch=D10, alerts={})
scen("cache_profile_order_vs_cfg", ["c", "a", "b"], [
    start(cache_of(["a", "b", "c"], a={**ok(20, 20), "fetched_at": T0 - 500}, c={**ok(20, 20), "fetched_at": T0 - 500},
                   b={**ok(20, 20), "fetched_at": T0 - 500})),
    ticks(4, 1.0)], default_fetch=D10, alerts={})
N14 = [f"claude-{i:02d}" for i in range(14)]
scen("many_profiles_one_request_per_tick", N14, [start(), ticks(30, 4.0)], default_fetch=D10, alerts={})
N20 = [f"p{i:02d}" for i in range(20)]
scen("interval_widens_with_many_profiles", N20, [start(), ticks(24, 5.0)], cfg={"interval_sec": 90}, default_fetch=D10, alerts={})

# ---- success / 429 / recovery / backoff
scen("success_then_429_no_retry_after", ["a"], [
    start(), tick(fetch=[F("a", ok(40, 50))]),
    tick(100.0), tick(100.0, fetch=[F("a", e429())]), tick(50.0), tick(300.0, fetch=[F("a", ok(41, 51))]), tick(200.0)],
    default_fetch=D10, alerts={})
scen("429_with_retry_after", ["a"], [
    start(), tick(fetch=[F("a", ok(40, 50))]),
    tick(200.0, fetch=[F("a", e429(45))]), tick(60.0, fetch=[F("a", e429(10))]), tick(40.0, fetch=[F("a", e429(0))]),
    tick(300.0, fetch=[F("a", e429(None))]), tick(1000.0)], default_fetch=D10, alerts={})
scen("repeated_429_exponential_and_cap", ["a"], [
    start(), [tick(1000.0, fetch=[F("a", e429())]) for _ in range(9)], tick(1000.0, fetch=[F("a", ok(1, 1))]),
    tick(1000.0, fetch=[F("a", e429())])], default_fetch=D10, alerts={})
scen("repeated_generic_errors_iv180", ["a"], [
    start(), [tick(1000.0, fetch=[F("a", err("HTTP 500", 500))]) for _ in range(6)]], default_fetch=D10, alerts={})
scen("repeated_generic_errors_iv90", ["a"], [
    start(), [tick(1000.0, fetch=[F("a", err("timeout", 0))]) for _ in range(6)]],
    cfg={"interval_sec": 90}, default_fetch=D10, alerts={})
scen("error_401_and_credentials", ["a", "b", "c"], [
    start(), tick(fetch=[F("a", err("HTTP 401", 401))]), tick(10.0, fetch=[F("b", err("thiếu credentials trong file", 0))]),
    tick(10.0, fetch=[F("c", err("HTTP 403", 403))]), tick(600.0, fetch=[F("a", err("HTTP 401", 401))])],
    cfg={"interval_sec": 500}, default_fetch=D10, alerts={})
scen("error_401_low_interval", ["a"], [
    start(), tick(fetch=[F("a", err("HTTP 401", 401))]), tick(2000.0, fetch=[F("a", err("HTTP 401", 401))])],
    cfg={"interval_sec": 90}, default_fetch=D10, alerts={})
scen("status0_retry_after", ["a", "b", "c"], [
    start(), tick(fetch=[F("a", err("claude đang làm mới token", 0, retry_after=30))]),
    tick(4.0, fetch=[F("b", err("claude đang làm mới token", 0, retry_after=5))]),
    tick(4.0, fetch=[F("c", err("mạng lỗi", 0, retry_after=None))]),
    tick(3000.0, fetch=[F("a", err("x", 0, retry_after=200))])], default_fetch=D10, alerts={})
scen("token_refresh_composition", ["a", "b", "c"], [
    start(),
    tick(refresh=[F("a", err("phiên đăng nhập hết hạn", 0, dead=True))], fetch=[]),
    tick(4.0, refresh=[F("b", {})], fetch=[F("b", ok(5, 6))]),
    tick(4.0, fetch=[F("c", ok(7, 8))]),
    tick(10.0, set={"token_refresh": False}),
    tick(300.0, fetch=[F("a", ok(1, 2))]),
    tick(300.0, fetch=[F("b", ok(2, 3))]), tick(1.0, fetch=[F("c", ok(2, 3))]),
    tick(0.0, set={"token_refresh": True}), tick(1000.0, refresh=[F("a", err("x", 0, retry_after=40))])],
    default_fetch=D10, alerts={})
scen("failure_without_previous_data", ["a", "b"], [
    start(), tick(fetch=[F("a", err("HTTP 500", 500))]), tick(5.0, fetch=[F("b", err("HTTP 429", 429, retry_after=77))]),
    tick(400.0, fetch=[F("a", ok(1, 2, plan="max"))])], default_fetch=D10, alerts={})
scen("failure_keeps_previous_numbers_and_extra_keys", ["a"], [
    start(), tick(fetch=[F("a", ok(40, 50, plan="max", extra_usage={"used": 1.5}))]),
    tick(400.0, fetch=[F("a", err("HTTP 502", 502))]), tick(1000.0, fetch=[F("a", err("HTTP 502", 502))]),
    tick(1000.0, fetch=[F("a", ok(41, 51))])], default_fetch=D10, alerts={})
scen("fetch_advances_clock_uses_time_after_fetch", ["a", "b"], [
    start(), tick(fetch=[F("a", ok(1, 1), advance=12.5)]), tick(1.0, fetch=[F("b", err("slow", 0), advance=30.25)]),
    ticks(2, 100.0)], default_fetch=D10, alerts={})

# ---- config changes
scen("config_interval_change", ABC, [
    start(), ticks(9, 4.0), tick(1.0, set={"interval_sec": 600}), ticks(3, 100.0), tick(0.0, set={"interval_sec": 90}),
    ticks(4, 50.0)], default_fetch=D10, alerts={})
scen("config_interval_change_with_retry_window", ["a"], [
    start(), tick(fetch=[F("a", e429(900))]), tick(1.0, set={"interval_sec": 90}), tick(1.0, set={"interval_sec": 1000}),
    tick(1000.0)], default_fetch=D10, alerts={})
scen("config_profile_added_and_removed", ABC, [
    start(), ticks(9, 4.0),
    tick(10.0, set={"profile_names": ["a", "b", "c", "d"]}), tick(1.0), tick(1.0),
    tick(10.0, set={"profile_names": ["a", "d"]}), ticks(3, 100.0),
    tick(1.0, set={"profile_names": ["x", "y", "a"]}), ticks(4, 1.0)], default_fetch=D10, alerts={})
scen("config_added_profile_due_immediately_before_others", ["a", "b"], [
    start(cache_of(["a", "b"], a={**ok(5, 5), "fetched_at": T0}, b={**ok(5, 5), "fetched_at": T0})),
    tick(5.0), tick(1.0, set={"profile_names": ["a", "b", "new"]}), ticks(2, 1.0)], default_fetch=D10, alerts={})

# ---- manual refresh
scen("refresh_fresh_429_and_started", ABC, [
    start(cache_of(ABC, a={**ok(1, 1), "fetched_at": T0 - 5},
                   b={**ok(1, 1), "fetched_at": T0 - 100, "err": "HTTP 429", "status": 429, "retry_at": T0 + 200.7},
                   c={**ok(1, 1), "fetched_at": T0 - 100})),
    op("refresh"), ticks(3, 1.0), op("refresh", 300.0), ticks(4, 1.0)], default_fetch=D10, alerts={})
scen("refresh_stagger_i_times_2", N14[:8], [
    start(cache_of(N14[:8], **{n: {**ok(1, 1), "fetched_at": T0 - 50} for n in N14[:8]})),
    tick(1.0), op("refresh", 1.0), ticks(10, 1.0)], default_fetch=D10, alerts={})
scen("refresh_no_data_and_overdue", ABC, [
    start(), op("refresh", 100.0), ticks(4, 1.0), op("refresh", 1.0)], default_fetch=D10, alerts={})
scen("refresh_flag_in_tick_then_poll_same_tick", ABC, [
    start(cache_of(ABC, **{n: {**ok(1, 1), "fetched_at": T0 - 500} for n in ABC})),
    tick(1.0, flag_refresh=True), ticks(3, 1.0), tick(5.0, flag_refresh=True), tick(1.0)], default_fetch=D10, alerts={})

# ---- alerts
scen("alert_warn_then_block", ABC, [
    start(), tick(fetch=[F("a", ok(85, 10))]), tick(7.0), tick(1.0), tick(200.0, fetch=[F("a", ok(96, 10))]),
    ticks(3, 1.0), tick(8.0), tick(20.0), tick(300.0, fetch=[F("a", ok(97, 10))]), tick(10.0), tick(10.0)],
    default_fetch=D10, alerts={})
J1 = "2026-01-01T10:00:01+00:00"
J2 = "2026-01-01T10:00:00.700000+00:00"
J3 = "2026-01-01T09:59:59.300000+00:00"
scen("alert_same_window_jitter", ["a"], [
    start(), tick(fetch=[F("a", ok(85, 10))]), tick(8.0),
    [tick(200.0, fetch=[F("a", ok(85, 10, r5=r))]) for r in (J1, J2, J3, J1, J2, R5)], tick(10.0)],
    default_fetch=D10, alerts={})
scen("alert_reset_29_and_31_minutes", ["a"], [
    start(), tick(fetch=[F("a", ok(85, 10))]), tick(8.0),
    tick(200.0, fetch=[F("a", ok(85, 10, r5="2026-01-01T10:29:00+00:00"))]), tick(8.0),
    tick(200.0, fetch=[F("a", ok(85, 10, r5="2026-01-01T10:31:00+00:00"))]), tick(8.0), tick(20.0),
    tick(200.0, fetch=[F("a", ok(85, 10, r5="2026-01-01T10:00:00+00:00"))]), tick(8.0), tick(20.0)],
    default_fetch=D10, alerts={})
scen("alert_reset_missing_or_invalid", ["a", "b"], [
    start(), tick(fetch=[F("a", ok(85, 90, r5=None, r7="not a date"))]), tick(4.0, fetch=[F("b", ok(85, 10, r5=""))]), tick(10.0),
    tick(300.0, fetch=[F("a", ok(85, 90, r5=None, r7=None))]), tick(1.0, fetch=[F("b", ok(85, 10, r5=None))]), tick(20.0)],
    default_fetch=D10, alerts={})
scen("alert_week_and_five_same_account_block_title", ABC, [
    start(), tick(fetch=[F("a", ok(90, 96))]), tick(7.0)], default_fetch=D10, alerts={})
scen("alert_week_only", ABC, [start(), tick(fetch=[F("a", ok(10, 82))]), tick(7.0)], default_fetch=D10, alerts={})
scen("alert_half_even_rounding", ["a", "b", "c", "d"], [
    start(), tick(fetch=[F("a", ok(84.5, 85.5))]), tick(4.0, fetch=[F("b", ok(94.5, 80.5))]),
    tick(4.0, fetch=[F("c", ok(0.5, 1.5))]), tick(4.0, fetch=[F("d", ok(95.0, 79.999))]), tick(7.0), tick(20.0)],
    default_fetch=ok(2.5, 3.5), alerts={})
scen("alert_baseline_all_seen_then_new_crossing", ABC, [
    start(), tick(fetch=[F("a", ok(92, 10))]), tick(4.0, fetch=[F("b", ok(5, 5))]), tick(4.0, fetch=[F("c", ok(85, 5))]),
    tick(8.0), tick(300.0, fetch=[F("a", ok(92, 10)), F("b", ok(96, 5)), F("c", ok(85, 5))]), ticks(3, 1.0), tick(8.0), tick(20.0)],
    default_fetch=D10)
scen("alert_baseline_timeout_120s", ABC, [
    start(), tick(fetch=[F("a", ok(92, 10))]), tick(4.0, fetch=[F("b", err("HTTP 500", 500))]),
    tick(4.0, fetch=[F("c", err("HTTP 500", 500))]), tick(100.0), tick(10.0), tick(10.0), tick(1.0), tick(30.0)],
    default_fetch=D10)
scen("alert_baseline_timeout_then_new_crossing", ["a", "b"], [
    start(), tick(fetch=[F("a", ok(92, 10))]), tick(4.0, fetch=[F("b", err("HTTP 500", 500))]),
    tick(130.0, fetch=[F("a", ok(92, 10))]), tick(1.0, fetch=[F("b", ok(97, 10))]), tick(8.0), tick(20.0)],
    cfg={"interval_sec": 90}, default_fetch=D10)
scen("alert_baseline_timeout_exact_boundary", ["a", "b"], [
    start(), tick(fetch=[F("a", ok(92, 10))]), tick(4.0, fetch=[F("b", err("x", 0))]), tick(116.0), tick(1.0), tick(1.0)],
    default_fetch=D10, cfg={"interval_sec": 5000})
scen("alert_restart_does_not_repeat", ABC, [
    start(), tick(fetch=[F("a", ok(85, 10))]), tick(8.0)],
    alerts={"a|five|warn": 1767261600.0}, default_fetch=D10)
scen("alert_notifications_off_then_on_no_burst", ABC, [
    start(), tick(fetch=[F("a", ok(85, 10))]), tick(8.0), tick(1.0, set={"notify_enabled": True}),
    tick(300.0, fetch=[F("a", ok(85, 10))]), tick(8.0), tick(20.0),
    tick(300.0, fetch=[F("a", ok(96, 10))]), tick(8.0), tick(20.0)],
    cfg={"notify_enabled": False}, default_fetch=D10, alerts={})
scen("alert_warn_block_toggles_independent", ABC, [
    start(), tick(fetch=[F("a", ok(85, 10))]), tick(8.0), tick(300.0, fetch=[F("a", ok(96, 10))]), tick(8.0), tick(20.0),
    tick(1.0, set={"notify_warn": True, "notify_block": False}),
    tick(300.0, fetch=[F("b", ok(85, 10))]), tick(1.0, fetch=[F("c", ok(96, 10))]), tick(8.0), tick(20.0)],
    cfg={"notify_warn": False, "notify_block": True}, default_fetch=D10, alerts={})
scen("alert_batching_three_accounts_one_toast", ABC, [
    start(), tick(fetch=[F("a", ok(85, 10))]), tick(2.0, fetch=[F("b", ok(96, 10))]), tick(2.0, fetch=[F("c", ok(90, 10))]),
    tick(1.0), tick(2.0), tick(2.0), tick(20.0)], cfg={"interval_sec": 5000}, default_fetch=D10, alerts={},
    rand=[0.0])
scen("alert_min_gap_15s_between_toasts", ABC, [
    start(), tick(fetch=[F("a", ok(85, 10))]), tick(5.0, fetch=[F("b", ok(96, 10))]), tick(2.0), tick(1.0, fetch=[F("c", ok(90, 10))]),
    tick(7.0), tick(7.0), tick(1.0), tick(3.0)],
    cfg={"interval_sec": 5000}, default_fetch=D10, alerts={}, rand=[0.0])
N9 = [f"claude-{i}" for i in range(9)]
scen("alert_more_than_four_accounts", N9 + ["spare"], [
    start(old_cache(N9 + ["spare"])), [tick(1.0, fetch=[F(n, ok(90, 10))]) for n in N9], tick(7.0), tick(1.0)],
    cfg={"interval_sec": 5000}, default_fetch=D10, alerts={}, rand=[0.0])
scen("alert_toast_body_truncated_to_250", ["x" * 70 + str(i) for i in range(4)] + ["y"], [
    start(old_cache(["x" * 70 + str(i) for i in range(4)] + ["y"])), [tick(1.0, fetch=[F("x" * 70 + str(i), ok(90, 91))]) for i in range(4)], tick(7.0), tick(1.0)],
    cfg={"interval_sec": 5000}, default_fetch=D10, alerts={}, rand=[0.0])
scen("alert_nen_dung_suggestion_and_ties", ["a", "b", "c", "d", "e"], [
    start(), tick(fetch=[F("a", ok(90, 10))]), tick(4.0, fetch=[F("b", ok(30, 20))]), tick(4.0, fetch=[F("c", ok(5, 30))]),
    tick(4.0, fetch=[F("d", ok(30, 30))]), tick(4.0, fetch=[F("e", ok(30, 12))]), tick(8.0), tick(20.0)],
    cfg={"interval_sec": 5000}, default_fetch=D10, alerts={}, rand=[0.0])
scen("alert_nen_dung_excludes_alerting_and_stale", ["a", "b", "c"], [
    start(), tick(fetch=[F("a", ok(10, 10))]), tick(4.0, fetch=[F("b", ok(1, 1))]), tick(4.0, fetch=[F("c", ok(2, 2))]),
    tick(300.0, fetch=[F("a", ok(90, 10)), F("b", err("HTTP 500", 500))]), tick(1.0), tick(8.0), tick(20.0)],
    default_fetch=D10, alerts={})
scen("alert_nen_dung_none_left", ["a"], [start(), tick(fetch=[F("a", ok(90, 10))]), tick(8.0)], default_fetch=D10, alerts={})
scen("alert_stale_data_never_alerts", ABC, [
    start(), op("set_data", data={"a": {**ok(99, 99), "fetched_at": T0, "err": "HTTP 429", "status": 429}, "b": {**ok(50, 50), "fetched_at": T0}}),
    op("publish"), op("flush", 10.0),
    op("set_data", data={"a": {**ok(99, 99), "fetched_at": T0}}), op("publish"), op("flush", 10.0)],
    default_fetch=D10, alerts={})
scen("alert_new_usage_window_alerts_again", ["a"], [
    start(), tick(fetch=[F("a", ok(85, 10))]), tick(8.0),
    tick(200.0, fetch=[F("a", ok(10, 10))]),
    tick(200.0, fetch=[F("a", ok(90, 10, r5="2026-01-01T15:00:00+00:00"))]), tick(8.0), tick(20.0)],
    default_fetch=D10, alerts={})
scen("alert_thresholds_from_config", ABC, [
    start(), tick(fetch=[F("a", ok(60, 10))]), tick(4.0, fetch=[F("b", ok(70, 10))]), tick(4.0, fetch=[F("c", ok(69.9, 10))]), tick(8.0)],
    cfg={"warn_pct": 60, "block_pct": 70}, default_fetch=D10, alerts={})
scen("test_toast_both_states", ABC, [
    start(), op("test_toast"), tick(1.0, flag_test=True), tick(1.0, set={"notify_enabled": False}, flag_test=True),
    op("test_toast")], default_fetch=D10, alerts={})
scen("alert_pending_cleared_when_disabled", ABC, [
    start(), tick(fetch=[F("a", ok(85, 10))]), tick(1.0, set={"notify_enabled": False}), tick(10.0),
    tick(0.0, set={"notify_enabled": True}), tick(300.0, fetch=[F("a", ok(85, 10))]), tick(20.0)],
    default_fetch=D10, alerts={})

# ---- tray tooltip / icon
scen("tray_tooltip_busy_and_colors", ["a", "b", "c", "d"], [
    start(), tick(fetch=[F("a", ok(79.99, 10))]), tick(4.0, fetch=[F("b", ok(80, 10))]), tick(4.0, fetch=[F("c", ok(10, 95))]),
    tick(4.0, fetch=[F("d", ok(100, 0))]), tick(200.0, fetch=[F("a", ok(0, 0))]),
    tick(1.0, fetch=[F("b", err("HTTP 500", 500))]), tick(1.0, fetch=[F("c", ok(0, 0))]), tick(1.0, fetch=[F("d", ok(0, 0))]),
    op("refresh", 1000.0)], cfg={"interval_sec": 90}, default_fetch=D10, alerts={}, rand=[0.0])
scen("tray_no_data_and_zero", ["a", "b"], [
    start(), tick(fetch=[F("a", err("chưa đăng nhập", 0))]), tick(4.0, fetch=[F("b", ok(0, 0))])],
    default_fetch=D10, alerts={})
scen("tray_tooltip_truncation_127_chars", ["tài-khoản-số-một-rất-dài-" + str(i) for i in range(6)], [
    start(old_cache(["tài-khoản-số-một-rất-dài-" + str(i) for i in range(6)])), [tick(1.0, fetch=[F("tài-khoản-số-một-rất-dài-" + str(i), ok(10 + i, 20 + i))]) for i in range(6)],
    tick(10000.0, fetch=[F("tài-khoản-số-một-rất-dài-0", err("lỗi mạng rất dài " * 10, 0))])],
    cfg={"interval_sec": 5000}, default_fetch=D10, alerts={}, rand=[0.0])
scen("tray_tooltip_exact_127_boundary", ["n1"], [
    start(), tick(fetch=[F("n1", ok(10, 20))]),
    tick(1.0, set={"profile_names": ["n1"]}), op("set_data", data={"n1": {**ok(10, 20), "err": "e" * 70}}), op("publish"),
    op("set_data", data={"n1": {**ok(10, 20), "err": "e" * 76}}), op("publish"),
    op("set_data", data={"n1": {**ok(10, 20), "err": "e" * 77}}), op("publish"),
    op("set_data", data={"n1": {**ok(10, 20), "err": "é" * 200}}), op("publish")],
    default_fetch=D10, alerts={})
scen("tray_removed_profile_stays_in_tooltip", ABC, [
    start(), ticks(9, 4.0), tick(1.0, set={"profile_names": ["a"]}), ticks(2, 1.0)], default_fetch=D10, alerts={})


# ------------------------------------------------------------------------------------------------ main
def show(name):
    """Debug helper: print the compact event trace of one scenario (`--show NAME`)."""
    DEBUG[0] = True
    sc = [s for s in SCEN if s["name"] == name][0]
    FQ.clear(), RQ.clear()
    try:
        r = run_scenario(sc)
    except AssertionError as e:
        print("ASSERT:", e)
        return
    for i, st in enumerate(r["steps"]):
        print(i, st["op"], "dt", st.get("dt"), "now+", st["state"]["now"] - r["t0"])
        for e in st["events"]:
            if e[0] == "write":
                e = ["write", "busy=" + str(e[1]["busy"])]
            elif e[0] == "icon":
                e = ["icon", e[1].replace("\n", " | "), e[3], e[4], e[6]]
            print("    ", e)


def main():
    if len(sys.argv) > 2 and sys.argv[1] == "--show":
        show(sys.argv[2])
        return
    out = [run_scenario(sc) for sc in SCEN]
    OUT.parent.mkdir(parents=True, exist_ok=True)
    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        json.dump(out, f, ensure_ascii=False, indent=1, allow_nan=False)
        f.write("\n")
    toasts = sum(1 for s in out for st in s["steps"] for e in st["events"] if e[0] == "toast")
    print(f"daemon: {len(out)} scenarios, {sum(len(s['steps']) for s in out)} steps, {toasts} toasts")


if __name__ == "__main__":
    main()
