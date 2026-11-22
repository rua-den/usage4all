"""Generate the Rust parity fixtures from the Python reference code (core.py + 3 helpers of widget.py).

    .venv\\Scripts\\python tools\\gen_fixtures.py

Writes rust/crates/core/tests/fixtures/*.json: one list of {"input": ..., "expected": ...} per function, plus
_meta.json. `expected` is {"raises": "<ExceptionName>"} where Python raises (Rust must then just not panic and
return its documented fallback). Everything runs in a throw-away sandbox: LOCALAPPDATA and core.HOME point to
temp dirs, the fake profiles hold a `.credentials.json` containing only `{}`; no real token, no real profile.
Time is pinned by replacing core.datetime / widget.datetime / widget.time; each case records its "now".

Time-zone dependent cases (fmt_when, naive ISO strings) are generated with the machine's local zone; the Rust
test must run on the same machine/zone (see _meta.json).
"""
import calendar
import json
import os
import shutil
import sys
import tempfile
import time
import types
from pathlib import Path

# Sandbox BEFORE importing core (core creates DATA at import time).
_TMP = tempfile.mkdtemp(prefix="cu-fixtures-")
os.environ["LOCALAPPDATA"] = os.path.join(_TMP, "localappdata")

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
for _s in (sys.stdout, sys.stderr):
    try:
        _s.reconfigure(encoding="utf-8")
    except Exception:
        pass

import core  # noqa: E402
import widget  # noqa: E402
from datetime import datetime as _real_dt, timedelta, timezone  # noqa: E402

assert str(core.DATA).startswith(_TMP), "core.DATA is not sandboxed"
OUT = ROOT / "rust" / "crates" / "core" / "tests" / "fixtures"

CLOCK = {"now": 0.0}


class FakeDT(_real_dt):
    """datetime whose now() is the pinned CLOCK."""

    @classmethod
    def now(cls, tz=None):
        return cls.fromtimestamp(CLOCK["now"], tz)


core.datetime = FakeDT
widget.datetime = FakeDT
widget.time = types.SimpleNamespace(time=lambda: CLOCK["now"])


def run(fn, *args):
    try:
        return fn(*args)
    except Exception as e:  # noqa: BLE001 - recording Python's behaviour
        return {"raises": type(e).__name__}


def save(name, cases):
    OUT.mkdir(parents=True, exist_ok=True)
    with open(OUT / (name + ".json"), "w", encoding="utf-8", newline="\n") as f:
        json.dump(cases, f, ensure_ascii=False, indent=1, allow_nan=False)
        f.write("\n")
    print(f"{name}: {len(cases)} cases")
    return len(cases)


def utc(ts):
    return _real_dt.fromtimestamp(ts, timezone.utc)


# ---------------------------------------------------------------- alias / aliases
NAMES = [
    "default (desktop)", "default", "claude-da", "claude-hn", "claude-ngoai", "claude-team-01",
    "claude-team-02", "claude-team-10", "claude-alpha", "claude-alps", "claude-al", "claude-a", "a",
    "claude-", "claude--", "", "-", "--x", "claude-x-y", "claude-x-y-7", "x-1", "claude-Team-Beta-3",
    "team", "claude-é", "éa", "日本語", "claude-日本-02", "claude-ab-cd", "  claude-sp ace",
    "claude claude-", "myclaude-x", "Claude-Upper", "claude-9", "9-9", "claude-a1-b2-c3",
    "claude-hà-nội", "default-2", "Default", "claude-claude-x", "ß-1", "claude-ngoai-vn-12",
]
ALIAS_CFGS = [
    {},
    {"widget_label": "short"},
    {"widget_label": "alias", "aliases": {"claude-da": "Đà Nẵng", "claude-hn": "  Hà Nội  ", "default (desktop)": "   ",
                                          "claude-ngoai": "", "claude-team-01": "😀x", "team": "ab\u00e9\u00e8cd"}},
    {"widget_label": "short", "aliases": {"claude-da": "xyz", "x-1": "  "}},
    {"widget_label": 5, "aliases": None},
    {"widget_label": None, "aliases": {}},
    {"aliases": {"claude-da": 5}},  # Python raises (non-string custom label)
]


def alias_cases():
    cases = []
    for cfg in ALIAS_CFGS:
        for ml in (None, 3):
            for n in NAMES:
                cases.append({"input": {"name": n, "cfg": cfg, "maxlen": ml},
                              "expected": run(core.alias, n, cfg, ml)})
    for ml in (0, 1, 2, 5, 100):
        for n in ("claude-team-01", "default (desktop)", "日本語", "claude-hà-nội"):
            for cfg in ({}, {"widget_label": "short"}, ALIAS_CFGS[2]):
                cases.append({"input": {"name": n, "cfg": cfg, "maxlen": ml},
                              "expected": run(core.alias, n, cfg, ml)})
    return cases


GROUPS = [
    ["default (desktop)", "claude-da", "claude-hn", "claude-ngoai", "claude-team-01", "claude-team-02"],
    ["claude-alpha", "claude-alps", "claude-al"],
    ["claude-alpha", "claude-alps"],
    ["claude-ab", "claude-a-b", "claude-a.b"],
    ["claude-Da", "claude-da", "claude-dx"],
    ["default (desktop)", "default (2)"],
    ["claude-team-01", "claude-team-02", "claude-team-03", "claude-tango-01", "claude-tango-02"],
    ["claude-al", "claude-alpha", "claude-alps", "claude-alpine", "claude-alpha2"],
    ["claude-abc", "claude-abd", "claude-abe", "claude-abf"],
    ["x", "x", "y"],
    ["claude-hn", "claude-hn"],
    ["日本", "日本語", "本日"],
    ["claude-é1", "claude-é2", "claude-e1"],
    ["claude-", "claude--", "claude"],
    ["claude-sam-1", "claude-sam-2", "claude-sue-1", "claude-sue-2", "claude-sue-3"],
    [], ["claude-da"],
    ["claude-abcdefgh", "claude-abcdefgi"],
    ["claude-abcdefgh", "claude-abcdefgh2", "claude-abcdefgh3"],
    NAMES,
]
GROUP_CFGS = [
    {},
    {"widget_label": "short"},
    {"aliases": {"claude-da": "DA", "claude-alpha": "Alpha!", "claude-ab": "q"}},
    {"aliases": {"claude-al": "AL", "claude-dx": " "}},
]


def aliases_cases():
    cases = []
    for cfg in GROUP_CFGS:
        for ml in (None, 4):
            for g in GROUPS:
                res = run(core.aliases, g, cfg, ml)
                exp = res if isinstance(res, dict) and list(res) == ["raises"] else [[k, v] for k, v in res.items()]
                cases.append({"input": {"names": g, "cfg": cfg, "maxlen": ml}, "expected": exp})
    return cases


# ---------------------------------------------------------------- iso_ts
def iso_cases():
    strs = [
        "2026-11-19T12:00:00+00:00", "2026-11-19T12:00:00Z", "2026-11-19T12:00:00.123456+00:00",
        "2026-11-19T12:00:00.000000Z", "2026-11-19T12:00:00.5Z", "2026-11-19T12:00:00.1234567Z",
        "2026-11-19T12:00:00.123456789012Z", "2026-11-19T12:00:00,5Z", "2026-11-19 12:00:00+00:00",
        "2026-11-19 12:00:00Z", "2026-11-19x12:00Z", "2026-11-19T12:00:00+07:00", "2026-11-19T12:00:00-05:00",
        "2026-11-19T12:00:00-0500", "2026-11-19T12:00:00+0530", "2026-11-19T12:00+05", "2026-11-19T12-05",
        "2026-11-19T12:00:00+05:30:15", "2026-11-19T12:00:00+05:30:15.5", "2026-11-19T12:00:00+0530:00",
        "2026-11-19T12:00:00+23:59", "2026-11-19T12:00:00-23:59", "2026-11-19T12:00:00+24:00",
        "2026-11-19T12:00:00+01:60", "2026-11-19T12:00:00+0060", "2026-11-19T12:00:00-00:00",
        "2026-11-19T12:00:00 +01:00", "2026-11-19T12:00:00\t+01:00", "2026-11-19T12:00:00  +01:00",
        "2026-11-19T12:00:00.5 +01:00", "2026-11-19T12:00 +01:00", "2026-11-19T12 +01:00",
        "2026-11-19T12:00:00 Z", "2026-11-19T12:00:00 ", "2026-11-19T12:00:00+01:00 ",
        "2026-11-19T12:00:00z", "2026-11-19T12:00:00Zx", "2026-11-19T12:00:00+01:00Z",
        "2026-11-19T24:00:00Z", "2026-11-19T24:00Z", "2026-11-19T24Z", "2026-11-19T24:00:00.000Z",
        "2026-11-19T24:00:01Z", "2026-11-19T24:01Z", "2026-11-19T25:00:00Z", "2026-11-19T23:60:00Z",
        "2026-11-19T23:59:60Z", "2026-11-19T23:59:59.999999Z", "2026-11-19T12Z", "2026-11-19T1200Z",
        "2026-11-19T120000Z", "2026-11-19T120000.25Z", "2026-11-19T12:0000Z", "2026-11-19T1:00Z",
        "20261119T120000Z", "20261119T1200Z", "20261119 12Z", "20261119", "20261119T12:00:00Z",
        "2026-1119T12:00Z", "202611-19T12:00Z", "2026-11-1T12:00Z", "2026-W47-3", "2026-11-19T", "2026-11-19Z",
        "2026-11-19T12:00:00.Z", "2026-11-19T12:00:00.12aZ", "2026-11-19T12:00:00+", "2026-11-19T12:00:00+1",
        "2026-11-19T12:00:00+001", "2026-11-19T12:00:00+01:", "2026-11-19T12:00:00+01:0",
        "2026-11-19T12:00:00+0100000", "2026-02-29T00:00:00Z", "2024-02-29T00:00:00Z", "2026-13-01T00:00:00Z",
        "2026-00-10T00:00:00Z", "2026-11-00T00:00:00Z", "2026-11-31T00:00:00Z", "0001-01-01T00:00:00+01:00",
        "0001-01-01T00:00:00Z", "0000-01-01T00:00:00Z", "9999-12-31T23:59:59.999999Z", "9999-12-31T23:59:59-01:00",
        "1969-12-31T23:59:59Z", "1970-01-01T00:00:00Z", "1970-01-01T00:00:00.5Z", "2000-02-29T12:00:00Z",
        "2026-11-19\u00a012:00:00Z", "２０２６-11-19T12:00:00Z", "2026-11-19T12:00:00.５Z",
        "2026-W47", "2026-W47-3", "2026W47", "2026W473", "2026-W47-3T12:00Z", "2026-W47T12:00Z", "2026W473T12:00Z",
        "2026W47T12:00Z", "2026-W47-0", "2026-W47-8", "2026-W53-1", "2026-W00-1", "2026-W47-3 12:00:00+07:00",
        "2026-W47-31", "2026-W471", "2026W4731200", "2026W47-3", "2026-W4", "2026-W47-", "2026-W47x", "2025-W53-1",
        "2026-11-19T12:00:00.+01:00", "2026-11-19T12:00.Z", "2026-11-19T12:00:00.-05", "2026-11-19T12.Z",
        "2026-11-19T12:00:00..Z", "2026-11-19T12:00:00.5.Z", "2026-11-19T12:00:00. Z", "2026-11-19T12:00:00.+",
        "2026-11-19T12:00:00x+01:00", "2026-11-19T12:00:00;Z", "2026-11-19T12:00:00:Z", "2026-11-19T12:00:001Z",
        "2026-11-19T12:001Z", "2026-11-19T121Z", "2026-11-19T12:00:00.5xZ", "2026-11-19T12:00:00xx+01:00",
        "2026-11-19T12:00x:00Z", "2026-11-19T12:00:00éZ", "2026-11-19T12:00:00 .Z",
        "", " ", "abc", "garbage", "2026", "2026-11", "12:00:00", "T12:00:00", "null", "None",
        " 2026-11-19T12:00:00Z", "2026-11-19T12:00:00Z\n",
        # API-like
        "2026-11-19T14:00:00.123456+00:00", "2026-11-19T18:59:59.999999+00:00", "2026-11-26T09:00:00.000000+00:00",
    ]
    cases = [{"input": {"iso": s}, "expected": run(core.iso_ts, s)} for s in strs]
    # naive (local time of this machine); all after 1970 so Windows mktime works
    naive = ["2026-11-19T12:00:00", "2026-11-19 12:00:00.25", "2026-11-19", "20261119", "2026-11-19T00:00",
             "2026-11-19T23:59:59.999999", "2026-07-01T12:00:00", "2026-01-01T00:00:00", "1999-12-31T23:59:59",
             "2026-11-19T24:00", "2026-11-19T12"]
    cases += [{"input": {"iso": s}, "expected": run(core.iso_ts, s), "tz_dependent": True} for s in naive]
    return cases


# ---------------------------------------------------------------- fmt_reset
def fmt_reset_cases():
    cases = []
    base = calendar.timegm((2026, 11, 19, 12, 0, 0))
    deltas = [-86400, -100, -1, -0.5, -0.000001, 0, 0.000001, 0.4, 0.5, 0.999999, 1, 30, 59, 59.5, 59.999999, 60,
              60.5, 61, 89.5, 90, 119, 120, 599, 600, 3599, 3599.5, 3600, 3601, 3659, 3660, 5399, 5400, 5460, 7199,
              7200, 36000, 86339, 86340, 86399, 86399.999, 86400, 86401, 86460, 89999, 90000, 172799, 172800,
              172800 + 3599, 604800, 604800 + 61, 30 * 86400 + 86399, 400 * 86400, 5 * 3600 + 29 * 60 + 59]
    for i, dlt in enumerate(deltas):
        CLOCK["now"] = float(base)
        d = utc(base + dlt)
        forms = [d.isoformat(), d.isoformat().replace("+00:00", "Z"),
                 d.astimezone(timezone(timedelta(hours=7))).isoformat(),
                 d.astimezone(timezone(timedelta(hours=-5, minutes=-30))).isoformat(sep=" ")]
        for iso in (forms[i % 4], forms[(i + 1) % 4]):
            cases.append({"input": {"iso": iso, "now": CLOCK["now"]}, "expected": run(core.fmt_reset, iso)})
    # fractional "now"
    for now in (base + 0.25, base + 0.5, base + 0.75, base + 0.123456):
        CLOCK["now"] = now
        for dlt in (0, 30.25, 59.75, 60.0, 3600.5):
            iso = utc(base + dlt).isoformat()
            cases.append({"input": {"iso": iso, "now": now}, "expected": run(core.fmt_reset, iso)})
    CLOCK["now"] = float(base)
    for iso in (None, "", "garbage", "2026-11-19T13:00:00", "2026-11-20", "20261119", "2026-11-19T24:00:00",
                "2026-11-19T12:00:00z", "2026-W47-3", " ", "2026-11-19T12:00:00+25:00", "0001-01-01T00:00:00Z",
                "9999-12-31T23:59:59Z", "2026-11-19T12:00:00.5+00:00", "2026-11-19T12:00:00,5Z", "2026-11-19T12Z",
                "2026-11-19T12:00:00 +00:00"):
        cases.append({"input": {"iso": iso, "now": CLOCK["now"]}, "expected": run(core.fmt_reset, iso)})
    return cases


# ---------------------------------------------------------------- fmt_when
def fmt_when_cases():
    cases = []

    def add(now_dt, iso):
        CLOCK["now"] = now_dt.timestamp()
        cases.append({"input": {"iso": iso, "now": CLOCK["now"]}, "expected": run(widget.fmt_when, iso),
                      "tz_dependent": True})

    now = _real_dt(2026, 11, 19, 14, 30, 0)           # local
    offs = [timedelta(hours=-12, minutes=-10), timedelta(hours=-14, minutes=-30), timedelta(minutes=-1), timedelta(0),
            timedelta(minutes=1), timedelta(hours=9, minutes=29, seconds=59), timedelta(hours=9, minutes=30),
            timedelta(hours=9, minutes=29, seconds=59, microseconds=999999), timedelta(hours=33, minutes=30),
            timedelta(hours=34), timedelta(hours=42, minutes=29, seconds=59), timedelta(hours=57),
            timedelta(days=2), timedelta(days=2, hours=10), timedelta(days=3), timedelta(days=4), timedelta(days=5),
            timedelta(days=6), timedelta(days=7), timedelta(days=13), timedelta(days=14), timedelta(days=60),
            timedelta(days=43, hours=3), timedelta(days=365), timedelta(days=-1), timedelta(days=-30),
            timedelta(days=-400), timedelta(hours=-36)]
    for o in offs:
        t = now + o
        aware = t.astimezone(timezone.utc)
        for iso in (aware.isoformat(), aware.isoformat().replace("+00:00", "Z"), t.isoformat(), t.isoformat(sep=" ")):
            add(now, iso)
    # every weekday + single/double-digit day/month, now far from the target
    for day in range(16, 23):
        add(now, _real_dt(2026, 11, day, 7, 0).astimezone(timezone.utc).isoformat())
    for (y, m, d) in ((2026, 12, 3), (2027, 1, 5), (2027, 2, 28), (2028, 2, 29), (2026, 12, 31), (2027, 1, 1),
                      (2026, 11, 20), (2026, 11, 21), (2026, 12, 10), (2026, 10, 9)):
        add(now, _real_dt(y, m, d, 9, 5).astimezone(timezone.utc).isoformat())
    # clocks near midnight / month and year ends
    for now2 in (_real_dt(2026, 11, 19, 23, 59, 30), _real_dt(2026, 11, 19, 0, 0, 5), _real_dt(2026, 11, 30, 23, 0, 0),
                 _real_dt(2026, 12, 31, 23, 59, 59), _real_dt(2027, 2, 28, 12, 0, 0), _real_dt(2026, 11, 19, 0, 0, 0)):
        for o in (timedelta(seconds=-20), timedelta(seconds=10), timedelta(seconds=40), timedelta(hours=12),
                  timedelta(hours=24), timedelta(hours=24, seconds=40), timedelta(hours=47), timedelta(hours=49),
                  timedelta(days=9)):
            t = now2 + o
            add(now2, t.astimezone(timezone.utc).isoformat())
            add(now2, t.isoformat())
    for iso in ("", "garbage", "2026-11-19", "20261120", "2026-11-19T24:00", "2026-W47-3", "2026-11-19T12:00:00z",
                "2026-11-19T07:00:00.5+07:00"):
        add(now, iso)
    return cases


# ---------------------------------------------------------------- ago_text / money
def ago_cases():
    cases = []
    now = 1795000000.0
    CLOCK["now"] = now
    for ts in (None, 0, 0.0):
        cases.append({"input": {"ts": ts, "now": now}, "expected": run(widget.ago_text, ts)})
    for d in (-1000, -5, -0.5, 0, 0.4, 0.5, 0.6, 1, 1.5, 2.5, 3.5, 10, 29.5, 30, 59, 59.5, 60, 89, 89.4, 89.5,
              89.51, 89.99, 90, 90.5, 119, 120, 149.9999, 150, 151, 209.9, 210, 270, 599, 600, 3599, 5399, 5399.4,
              5399.9, 5399.999, 5400, 5401, 8999, 9000, 9001, 12600, 16200, 43200, 86399, 86400, 172800, 360000,
              8640000):
        ts = now - d
        cases.append({"input": {"ts": ts, "now": now}, "expected": run(widget.ago_text, ts)})
    for now2 in (1795000000.25, 1795000000.5):
        CLOCK["now"] = now2
        for d in (0.25, 0.5, 89.25, 89.75, 5399.75):
            ts = now2 - d
            cases.append({"input": {"ts": ts, "now": now2}, "expected": run(widget.ago_text, ts)})
    return cases


def money_cases():
    cases = []
    vals = [0, 0.0, -0.0, 0.5, 0.125, 0.375, 0.005, 0.015, 0.025, 1.005, 2.675, 0.1, 0.2, 0.3, 1, 5, 12.5, 99.99,
            999.99, 999.995, 1000, 1000.0, 1234.5, 12345.678, 123456.789, 1234567.891, 9999999.995, 1e9, 123456789012,
            1e15, 1e16, 2.5e22, 0.98, 25.0, 3.92, -0.001, -0.005, -0.125, -1, -999.999, -1000, -1234567.891,
            -0.0049, 1e-9, 5e-324, 1.7976931348623157e308]
    for cur in ("USD", "EUR", "GBP"):
        for v in vals:
            cases.append({"input": {"v": v, "cur": cur}, "expected": run(widget.money, v, cur)})
    for cur in ("JPY", "VND", "usd", "", "A B", "€", "Đồng", "XYZ"):
        for v in (0, 0.125, 1234567.891, -2.675, 1e9, 12.5):
            cases.append({"input": {"v": v, "cur": cur}, "expected": run(widget.money, v, cur)})
    return cases


# ---------------------------------------------------------------- effective_interval
def effective_cases():
    def prof(n):
        return [{"name": f"p{i}", "path": f"C:\\p{i}"} for i in range(n)]
    cfgs = []
    for iv in (180, 90, 89, 60, 0, -5, 300, 3600, "300", " 300 ", "+200", "-7", "1_000", "1__0", "_1", "1_", "1e3",
               "abc", "", " ", "0x10", "3.5", 300.9, -1.5, 1000.0, 89.99, 1e6, True, False, None, [1], [], {"a": 1},
               {}):
        cfgs.append({"interval_sec": iv, "profiles": prof(2)})
    cfgs.append({"profiles": prof(2)})
    cfgs.append({})
    for n in (0, 1, 17, 18, 19, 20, 36, 40, 100, 250):
        cfgs.append({"interval_sec": 180, "profiles": prof(n)})
        cfgs.append({"interval_sec": 600, "profiles": prof(n)})
        cfgs.append({"profiles": prof(n)})
    for pr in (None, False, 0, "", {}, [], "abcdef", "a" * 40, {"a": 1, "b": 2}, 5, True, 1.5):
        cfgs.append({"interval_sec": 100, "profiles": pr})
    cfgs.append({"interval_sec": "x", "profiles": prof(30)})
    return [{"input": {"cfg": c}, "expected": run(core.effective_interval, c)} for c in cfgs]


# ---------------------------------------------------------------- parse_extra / parse_scoped
def extra_cases():
    ok = {"is_enabled": True, "monthly_limit": 2500, "used_credits": 98, "utilization": 3.92, "currency": "USD"}
    exs = [None, {}, [], 0, False, "", 5, 1.5, True, [1], "abc", {"is_enabled": False, "monthly_limit": 1},
           {"is_enabled": True}, {"is_enabled": True, "monthly_limit": None}, {"monthly_limit": 100}, ok]

    def v(**kw):
        d = dict(ok)
        d.update(kw)
        return d
    for k, vals in (
        ("is_enabled", (1, "yes", 0, "", [], {}, "false", None, 2.5, [0])),
        ("monthly_limit", (None, 0, 0.0, -100, 1e3, 12345678901, "x", "100", True, False, [1], {}, 2500.5, 1)),
        ("used_credits", (None, 0, 0.0, 12.5, "x", "5", True, False, [], [1], {}, 123456789012, 2600, -50, 1)),
        ("decimal_places", (0, 1, 3, 4, 6, 15, None, "2", "x", 2.9, -1, -2, True, False, [], {}, " 3 ", "1_0")),
        ("utilization", (0, 0.0, None, "n/a", False, True, 150, 12.345678, -1, [], {}, "", 1e-9, 99.999999999)),
        ("currency", ("EUR", "GBP", "", None, 5, "JPY", "usd", "Đồng", False, [], "A B")),
    ):
        for x in vals:
            exs.append(v(**{k: x}))
    for k in ("is_enabled", "monthly_limit", "used_credits", "decimal_places", "utilization", "currency"):
        d = dict(ok)
        d.pop(k, None)
        exs.append(d)
    exs += [
        {"is_enabled": True, "monthly_limit": 0, "used_credits": 5},
        {"is_enabled": True, "monthly_limit": 1000, "used_credits": 333},
        {"is_enabled": True, "monthly_limit": 3, "used_credits": 1, "decimal_places": 0},
        {"is_enabled": True, "monthly_limit": 7, "used_credits": 2, "decimal_places": 3},
        {"is_enabled": True, "monthly_limit": 100, "used_credits": 1, "decimal_places": 2, "extra": {"x": 1}},
        {"is_enabled": True, "monthly_limit": 100, "used_credits": 0.1, "decimal_places": 2},
        {"is_enabled": True, "monthly_limit": 1, "used_credits": 3, "decimal_places": 2},
        {"is_enabled": True, "monthly_limit": 2500, "used_credits": 98, "decimal_places": 2},
        {"is_enabled": True, "monthly_limit": 123456789, "used_credits": 987654321, "decimal_places": 4},
    ]
    return [{"input": {"ex": e}, "expected": run(core.parse_extra, e)} for e in exs]


def scoped_cases():
    def L(**kw):
        d = {"kind": "weekly_scoped", "scope": {"model": {"display_name": "Fable"}}, "percent": 21,
             "resets_at": "2026-11-26T09:00:00+00:00"}
        d.update(kw)
        return d
    lims = [None, [], {}, "x", "", 0, 5, True, False, 1.5, {"a": 1}, {"kind": "weekly_scoped"}, "weekly_scoped",
            [L()], [L(), L(scope={"model": {"display_name": "Opus"}}, percent=0, resets_at=None)],
            [L(kind="other")], [L(kind=None)], [{"scope": {"model": {"display_name": "X"}}}],
            [L(scope=None)], [L(scope={})], [L(scope=[])], [L(scope="x")], [L(scope=5)], [L(scope={"model": None})],
            [L(scope={"model": 5})], [L(scope={"model": "m"})], [L(scope={"model": []})],
            [L(scope={"model": {}})], [L(scope={"model": {"display_name": None}})],
            [L(scope={"model": {"display_name": ""}})], [L(scope={"model": {"display_name": 5}})],
            [L(scope={"model": {"display_name": 0}})], [L(scope={"model": {"display_name": "Đ"}})],
            [L(scope={"model": {"display_name": ["a"]}})], [L(scope={"other": 1})],
            [L(percent=None)], [L(percent=0)], [L(percent=0.0)], [L(percent="12")], [L(percent=12.5)],
            [L(percent=False)], [L(percent=True)], [L(percent=[])], [L(percent=[1])],
            [{"kind": "weekly_scoped", "scope": {"model": {"display_name": "NoPct"}}}],
            [L(resets_at=None)], [{"kind": "weekly_scoped", "scope": {"model": {"display_name": "NoReset"}}, "percent": 3}],
            [L(resets_at=5)], [1, "s", None, [1], [L()], L()], [1, L(), "x", L(percent=7)],
            [L(), L(), L()], [L(), 5, L(scope="bad"), L(percent=9, scope={"model": {"display_name": "Z"}})],
            [[L()]], [None], [[]], [{}], [{"scope": None}], [L(extra="ignored")],
            [L(kind="weekly_scoped ")], [L(kind="WEEKLY_SCOPED")], [L(kind=["weekly_scoped"])],
            [L(**{"percent": 99.5, "scope": {"model": {"display_name": "A", "id": 1}, "x": 2}})],
            ]
    return [{"input": {"limits": x}, "expected": run(core.parse_scoped, x)} for x in lims]


# ---------------------------------------------------------------- home layouts / detect_profiles / load_config
def make_home(layout):
    """layout: {"default": bool, "profiles_dir": bool, "entries": [[name, kind]]} kind: dir_creds|dir_nocreds|file"""
    home = Path(tempfile.mkdtemp(dir=_TMP, prefix="home-"))
    if layout.get("default"):
        (home / ".claude").mkdir()
        (home / ".claude" / ".credentials.json").write_text("{}", "utf-8")
    if layout.get("profiles_dir"):
        root = home / "claude-profiles"
        root.mkdir()
        for name, kind in layout.get("entries", []):
            if kind == "file":
                (root / name).write_text("x", "utf-8")
            else:
                (root / name).mkdir()
                if kind == "dir_creds":
                    (root / name / ".credentials.json").write_text("{}", "utf-8")
    return home


def unhome(obj, home):
    s = str(home)
    if isinstance(obj, str):
        return obj.replace(s, "{HOME}")
    if isinstance(obj, list):
        return [unhome(x, home) for x in obj]
    if isinstance(obj, dict):
        return {unhome(k, home): unhome(v, home) for k, v in obj.items()}
    return obj


LAYOUTS = [
    {},
    {"default": True},
    {"profiles_dir": True},
    {"default": True, "profiles_dir": True, "entries": [
        ["claude-hn", "dir_creds"], ["claude-da", "dir_creds"], ["Claude-Zed", "dir_creds"], ["alpha", "dir_creds"],
        ["Beta", "dir_creds"], ["beta2", "dir_nocreds"], ["notes.txt", "file"], ["_under", "dir_creds"],
        ["claude-ngoai", "dir_creds"], ["ÉCLAIR", "dir_creds"], ["zulu", "dir_creds"], ["Zulu2", "dir_creds"],
        ["10", "dir_creds"], ["9", "dir_creds"], ["a b", "dir_creds"], ["a-b", "dir_creds"], ["a_b", "dir_creds"]]},
    {"default": False, "profiles_dir": True, "entries": [["only", "dir_nocreds"], ["f", "file"]]},
    {"default": True, "profiles_dir": False},
]


def detect_cases():
    cases = []
    for lay in LAYOUTS:
        home = make_home(lay)
        core.HOME = home
        cases.append({"input": {"layout": lay}, "expected": unhome(run(core.detect_profiles), home)})
    return cases


CFG_TEXTS = [
    None, "{}", " {} ", "", "   ", "{not json", "{\"a\": 1,}", "[1, 2]", "[]", "null", "5", "\"str\"", "true", "{",
    "\ufeff{\"interval_sec\": 300}",
    '{"interval_sec": 300, "warn_pct": 70, "block_pct": 90}',
    '{"widget_size": 36.5, "widget_cx": 100.25, "widget_cy": 5, "widget_angle": 15.5, "dash_rect": [1.5, 2, 3, 4]}',
    '{"zzz": 1, "interval_sec": 200, "aaa": {"x": [1, 2], "y": null}, "mmm": [true, false]}',
    '{"widget_style": "bars", "unknown_future_key": "keep me", "aliases": {"claude-da": "Đà"}}',
    '{"interval_sec": 100, "interval_sec": 300}',
    '{"profiles": []}', '{"profiles": null}', '{"profiles": false}', '{"profiles": 0}', '{"profiles": ""}',
    '{"profiles": {}}', '{"profiles": 0.0}',
    '{"profiles": [{"name": "x", "path": "C:\\\\p"}]}', '{"profiles": "abc"}', '{"profiles": [1, 2]}',
    '{"profiles": {"a": 1}}', '{"profiles": true}', '{"profiles": 7}',
    '{"interval_sec": "300"}', '{"interval_sec": " 250 "}', '{"interval_sec": 45}', '{"interval_sec": 89}',
    '{"interval_sec": 90}', '{"interval_sec": 91}', '{"interval_sec": 300.9}', '{"interval_sec": 89.9}',
    '{"interval_sec": true}', '{"interval_sec": false}', '{"interval_sec": null}', '{"interval_sec": "abc"}',
    '{"interval_sec": []}', '{"interval_sec": {}}', '{"interval_sec": 1e2}', '{"interval_sec": 1e3}',
    '{"interval_sec": -100}', '{"interval_sec": "1_0"}', '{"interval_sec": "-5"}', '{"interval_sec": ""}',
    '{"interval_sec": NaN}', '{"interval_sec": 12345678901}',
    '{"widget_angle": 0, "widget_cx": 12, "widget_cy": 34.5}',
    '{"widget_hidden": ["claude-da"], "widget_label": "short", "widget_font": 120.5, "widget_opacity": 55}',
    '{"block_enabled": false, "token_refresh": 0, "autostart": 1}',
    '{"dash_rect": null}', '{"dash_rect": [10, 20, 800.5, 600]}', '{"aliases": null}',
    '{"x": "\u00e9\u4e2d\U0001F600"}',
    '{\n  "interval_sec": 120,\n  "profiles": [\n    {"name": "claude-da", "path": "C:\\\\Users\\\\x\\\\claude-profiles\\\\claude-da"}\n  ]\n}\n',
]


def load_config_cases():
    cases = []
    cfgpath = core.CONFIG
    for i, text in enumerate(CFG_TEXTS):
        for lay in (LAYOUTS[0], LAYOUTS[3]) if i < 3 or text in ('{"profiles": []}', '{"interval_sec": 300}') else (LAYOUTS[1],):
            home = make_home(lay)
            core.HOME = home
            if cfgpath.exists():
                cfgpath.unlink()
            if text is not None:
                cfgpath.write_bytes(text.encode("utf-8"))
            res = run(core.load_config)
            cases.append({"input": {"config_text": text, "layout": lay}, "expected": unhome(res, home)})
    if cfgpath.exists():
        cfgpath.unlink()
    return cases


# ---------------------------------------------------------------- write_cache
def write_cache_cases():
    cases = []
    cdir = Path(_TMP) / "cache"
    cdir.mkdir(exist_ok=True)
    core.CACHE = cdir / "usage_cache.json"
    base = {"block_enabled": True, "block_pct": 95, "warn_pct": 80}
    pa, pb, pc = ({"name": "claude-da", "path": "C:\\p\\da"}, {"name": "claude-hn", "path": "C:\\p\\hn"},
                  {"name": "default (desktop)", "path": "C:\\Users\\x\\.claude"})
    ok = {"five": 23.0, "five_reset": "2026-11-19T14:00:00+00:00", "week": 49.0, "week_reset": "2026-11-26T09:00:00+00:00",
          "fetched_at": 1795000000.5, "plan": "team", "extra": {"used": 0.98, "limit": 25.0, "pct": 3.92, "currency": "USD"},
          "scoped": [{"name": "Fable", "pct": 21, "resets_at": "2026-11-26T09:00:00+00:00"}]}
    err = {"err": "HTTP 429", "status": 429, "retry_at": 1795000100.25, "retry_after": None}
    scen = [
        dict(cfg={**base, "profiles": []}, data={}, busy=[], refresh=None),
        dict(cfg={**base, "profiles": [pa]}, data={}, busy=[], refresh=None),
        dict(cfg={**base, "profiles": [pa, pb, pc]}, data={"claude-da": ok, "claude-hn": err}, busy=["claude-hn", "claude-da"], refresh=None),
        dict(cfg={**base, "profiles": [pc, pb, pa]}, data={"claude-da": ok, "claude-hn": {**ok, **err}, "default (desktop)": ok}, busy=[], refresh=None),
        dict(cfg={**base, "profiles": [pa]}, data={"claude-da": {"path": "OVERRIDE", "five": 1}}, busy=[], refresh=None),
        dict(cfg={**base, "profiles": [pa]}, data={"claude-da": {"five": 1, "path": "late"}, "ghost": ok}, busy=["z", "Z", "a", "é", "B"], refresh={"ts": 1795000000.5, "started": ["claude-da"], "skipped": [{"name": "claude-hn", "why": "vừa cập nhật"}]}),
        dict(cfg={**base, "profiles": [pa, pa]}, data={"claude-da": ok}, busy=["claude-da", "claude-da"], refresh={}),
        dict(cfg={"block_enabled": False, "block_pct": 99.5, "warn_pct": 0, "profiles": [pb, pa], "interval_sec": 300, "zzz": 1}, data={"claude-hn": {"extra": None, "scoped": []}}, busy=[], refresh=0),
        dict(cfg={**base, "profiles": [{"name": "Việt Nam", "path": "D:\\Đường dẫn"}]}, data={"Việt Nam": {"err": "không đọc được credentials", "status": 0}}, busy=["Việt Nam"], refresh=None),
        dict(cfg={**base, "profiles": [pa, pb]}, data={"claude-da": {}, "claude-hn": {"five": 0, "week": 0.0}}, busy=[], refresh=False),
        dict(cfg={**base, "profiles": [pa]}, data={"claude-da": ok}, busy=["x"], refresh=[1, 2]),
    ]
    for s in scen:
        if core.CACHE.exists():
            core.CACHE.unlink()
        core.write_cache(s["cfg"], s["data"], s["busy"], s["refresh"])
        out = json.loads(core.CACHE.read_text("utf-8"))
        assert isinstance(out.pop("ts"), float)
        cases.append({"input": s, "expected": out})
    return cases


def main():
    counts = {}
    try:
        counts["alias"] = save("alias", alias_cases())
        counts["aliases"] = save("aliases", aliases_cases())
        counts["iso_ts"] = save("iso_ts", iso_cases())
        counts["fmt_reset"] = save("fmt_reset", fmt_reset_cases())
        counts["fmt_when"] = save("fmt_when", fmt_when_cases())
        counts["ago_text"] = save("ago_text", ago_cases())
        counts["money"] = save("money", money_cases())
        counts["effective_interval"] = save("effective_interval", effective_cases())
        counts["parse_extra"] = save("parse_extra", extra_cases())
        counts["parse_scoped"] = save("parse_scoped", scoped_cases())
        counts["detect_profiles"] = save("detect_profiles", detect_cases())
        counts["load_config"] = save("load_config", load_config_cases())
        counts["write_cache"] = save("write_cache", write_cache_cases())
        local = _real_dt.now().astimezone()
        meta = {
            "generator": "tools/gen_fixtures.py",
            "python": sys.version.split()[0],
            "tz_name": time.tzname[0] + "/" + time.tzname[1],
            "utc_offset_now": str(local.utcoffset()),
            "note": "Cases with tz_dependent=true (fmt_when, naive iso_ts) were generated with this machine's "
                    "local time zone; the Rust parity test must run in the same zone.",
            "placeholder": "{HOME} in load_config/detect_profiles expectations = the fake home dir of the test",
            "counts": counts,
        }
        with open(OUT / "_meta.json", "w", encoding="utf-8", newline="\n") as f:
            json.dump(meta, f, ensure_ascii=False, indent=1)
            f.write("\n")
    finally:
        shutil.rmtree(_TMP, ignore_errors=True)


if __name__ == "__main__":
    main()
