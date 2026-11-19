"""Generate the Rust parity fixtures for the hook (promptguard.py) and hookinstall.py from the Python code.

    .venv\\Scripts\\python tools\\gen_fixtures_hook.py

Writes rust/crates/core/tests/fixtures/hook.json and hookinstall.json. Same sandbox rules as gen_fixtures.py:
LOCALAPPDATA and USERPROFILE (home) point to temp dirs BEFORE core is imported; no real profile/settings.json
is read or written. Path tokens in the inputs ({HOME}, {CWD}, {DRIVE}, {EXISTING_EXE}) are replaced by real
values here and by the Rust test on its side, so the fixtures contain no machine-specific paths.
`hookinstall.command` is monkeypatched to a fixed fake exe so the output is deterministic.
"""
import json
import os
import shutil
import sys
import tempfile
import types
from pathlib import Path

_TMP = tempfile.mkdtemp(prefix="cu-fixtures-hook-")
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
import hookinstall  # noqa: E402
import promptguard  # noqa: E402

assert str(core.DATA).startswith(_TMP), "core.DATA is not sandboxed"
OUT = ROOT / "rust" / "crates" / "core" / "tests" / "fixtures"

CWD = os.getcwd()
DRIVE = os.path.splitdrive(CWD)[0]
EXISTING_EXE = os.path.join(_TMP, "existing", "claude-usage.exe")
os.makedirs(os.path.dirname(EXISTING_EXE))
Path(EXISTING_EXE).write_bytes(b"x")
FAKE_EXE = "C:\\cu-fixture\\claude-usage.exe"
FAKE_CMD = f'"{FAKE_EXE}" hook'
hookinstall.command = lambda: FAKE_CMD


def subst(v):
    if isinstance(v, str):
        return (v.replace("{HOME}", HOME).replace("{CWD}", CWD)
                .replace("{DRIVE}", DRIVE).replace("{EXISTING_EXE}", EXISTING_EXE))
    if isinstance(v, list):
        return [subst(x) for x in v]
    if isinstance(v, dict):
        return {subst(k): subst(x) for k, x in v.items()}
    return v


def save(name, cases):
    OUT.mkdir(parents=True, exist_ok=True)
    with open(OUT / (name + ".json"), "w", encoding="utf-8", newline="\n") as f:
        json.dump(cases, f, ensure_ascii=False, indent=1, allow_nan=False)
        f.write("\n")
    print(f"{name}: {len(cases)} cases")


# ------------------------------------------------------------------------------------------------ hook
NOW = 1_800_000_000.0
HOOK = []


def prof(path, five=None, week=None, **kw):
    d = {"path": path}
    if five is not None:
        d["five"] = five
    if week is not None:
        d["week"] = week
    d.update(kw)
    return d


def cache(profiles, ts=NOW - 10, be=True, pct=95, **extra):
    c = {"ts": ts, "block_enabled": be, "block_pct": pct, "warn_pct": 80, "busy": [], "refresh": None,
         "profiles": profiles}
    c.update(extra)
    return c


def H(name, c=None, text=None, env="{HOME}\\p1", now=NOW, deviation=False, omit=()):
    """c = cache object (keys in `omit` are dropped), text = raw file text; neither = no cache file."""
    if c is not None:
        c = {k: v for k, v in c.items() if k not in omit}
    inp = {"cache": c, "cache_text": text, "env_config_dir": env, "now": now}
    HOOK.append({"name": name, "input": inp, "deviation": deviation})


P1 = "{HOME}\\p1"
OK_P = {"a": prof("{HOME}\\pa", 10, 10)}


def blk(**kw):
    """A cache in which p1 is over the limit."""
    profs = {"p1": prof(P1, 96, 10)}
    profs.update(kw.pop("others", {"alt": prof("{HOME}\\alt", 20, 30)}))
    return cache(profs, **kw)


# fail-open branches
H("no cache file")
H("empty file", text="")
H("invalid json", text="{ nope")
H("json null", text="null")
H("json list", text="[]")
H("json number", text="5")
H("utf-8 BOM", text="\ufeff" + json.dumps(blk()))
H("block disabled false", blk(be=False))
H("block disabled 0", blk(be=0))
H("block disabled null", blk(be=None))
H("block disabled empty string", blk(be=""))
H("block_enabled missing", blk(), omit=("block_enabled",))
H("block_enabled truthy int 1", blk(be=1))
H("block_enabled truthy string", blk(be="no"))
H("ts missing", blk(), omit=("ts",))
H("ts string", blk(ts="1800000000"))
H("ts null", blk(ts=None))
H("ts list", blk(ts=[1]))
H("ts bool true is ancient", blk(ts=True))
H("ts age 901 -> stale", blk(ts=NOW - 901))
H("ts age exactly 900 -> blocks", blk(ts=NOW - 900))
H("ts age 899.5 -> blocks", blk(ts=NOW - 899.5))
H("ts in the future -> blocks", blk(ts=NOW + 500))
H("ts int", blk(ts=int(NOW) - 5))
H("profiles missing", blk(), omit=("profiles",))
H("profiles list", {**blk(), "profiles": []})
H("profiles null", {**blk(), "profiles": None})
H("profiles empty", cache({}))
H("current profile not found", cache({"x": prof("{HOME}\\other", 99, 99)}))
H("current has no five (err)", cache({"p1": {"path": P1, "err": "HTTP 429", "status": 429}}))
H("current five present but week missing", cache({"p1": {"path": P1, "five": 99}}))
H("current five null", cache({"p1": {"path": P1, "five": None, "week": 99}}))
H("current week null", cache({"p1": {"path": P1, "five": 99, "week": None}}))
H("current five string", cache({"p1": prof(P1, "99", 99)}))
H("block_pct missing", blk(), omit=("block_pct",))
H("block_pct string", blk(pct="95"))
H("block_pct null", blk(pct=None))
H("block_pct list", blk(pct=[95]))
H("profile entry without path before match", cache({"junk": {"five": 1}, "p1": prof(P1, 99, 99)}))
H("profile path non-string before match", cache({"junk": {"path": 5, "five": 1}, "p1": prof(P1, 99, 99)}))
H("profile path null before match", cache({"junk": {"path": None}, "p1": prof(P1, 99, 99)}))
H("profile entry is a string before match", cache({"junk": "x", "p1": prof(P1, 99, 99)}))
H("profile entry is a list before match", cache({"junk": [], "p1": prof(P1, 99, 99)}))
H("malformed entry AFTER the match is ignored by the lookup but not by best",
  cache({"p1": prof(P1, 99, 99), "junk": {"five": 1}}))
H("malformed non-dict entry after match: null -> fail open", cache({"p1": prof(P1, 99, 99), "junk": None}))
H("after match: number entry -> fail open", cache({"p1": prof(P1, 99, 99), "junk": 7}))
H("after match: empty list entry ignored", cache({"p1": prof(P1, 99, 99), "junk": []}))
H("after match: list containing five -> fail open", cache({"p1": prof(P1, 99, 99), "junk": ["five"]}))
H("after match: string without five ignored", cache({"p1": prof(P1, 99, 99), "junk": "abc"}))
H("after match: string containing five -> fail open", cache({"p1": prof(P1, 99, 99), "junk": "five"}))
H("after match: bad entry without path is fine", cache({"p1": prof(P1, 99, 99), "junk": {"err": "x"}}))
H("empty profile name -> allow", cache({"": prof(P1, 99, 99)}))
H("other has five but no week -> fail open", cache({"p1": prof(P1, 99, 99), "o": {"path": "x", "five": 5}}))
H("other five null -> fail open", cache({"p1": prof(P1, 99, 99), "o": {"path": "x", "five": None, "week": 1}}))
H("cache is a list of profiles", text=json.dumps({"ts": NOW, "block_enabled": True, "block_pct": 1,
                                                    "profiles": [{"path": P1, "five": 9, "week": 9}]}))
H("NaN ts (python json accepts it; rust fails open)",
  text='{"ts": NaN, "block_enabled": true, "block_pct": 95, "profiles": '
       + json.dumps({"p1": prof(P1, 99, 99)}) + "}", deviation=True)

# thresholds
H("below limit", cache({"p1": prof(P1, 94.9, 10)}))
H("below limit both", cache({"p1": prof(P1, 10, 20)}))
H("equal limit five (int)", cache({"p1": prof(P1, 95, 10), "o": prof("x", 1, 2)}))
H("equal limit week", cache({"p1": prof(P1, 10, 95), "o": prof("x", 1, 2)}))
H("above limit week-driven", cache({"p1": prof(P1, 10, 99), "o": prof("x", 1, 2)}))
H("above limit five-driven", cache({"p1": prof(P1, 100, 3), "o": prof("x", 1, 2)}))
H("just below limit float", cache({"p1": prof(P1, 94.99999999, 94.9)}))
H("equal limit float", cache({"p1": prof(P1, 95.0, 0.0), "o": prof("x", 1, 2)}))
H("block_pct float 95.0 prints 95.0", cache({"p1": prof(P1, 96, 10)}, pct=95.0))
H("block_pct int 95 prints 95", cache({"p1": prof(P1, 96, 10)}, pct=95))
H("block_pct float 90.5", cache({"p1": prof(P1, 91, 10)}, pct=90.5))
H("block_pct float 1e-05", cache({"p1": prof(P1, 1, 0)}, pct=1e-05))
H("block_pct float 1e16", cache({"p1": prof(P1, 99, 99)}, pct=1e16))
H("block_pct tiny float hit", cache({"p1": prof(P1, 3, 0)}, pct=0.1))
H("block_pct float 0.0001", cache({"p1": prof(P1, 3, 0)}, pct=0.0001))
H("block_pct bool true", cache({"p1": prof(P1, 3, 0)}, pct=True))
H("block_pct bool false", cache({"p1": prof(P1, 3, 0)}, pct=False))
H("block_pct 0 blocks at zero usage", cache({"p1": prof(P1, 0, 0)}, pct=0))
H("block_pct negative", cache({"p1": prof(P1, -0.4, -3)}, pct=-1))
H("block_pct 100 at 100", cache({"p1": prof(P1, 100, 100)}, pct=100))
H("block_pct 100.0 at 99.99", cache({"p1": prof(P1, 99.99, 100)}, pct=100.0))
H("five bool true", cache({"p1": {"path": P1, "five": True, "week": False}}, pct=1))
H("large ints", cache({"p1": prof(P1, 250, 100000)}, pct=100))

# rounding of {:.0f}
H("round 0.5 and 1.5", cache({"p1": prof(P1, 0.5, 1.5)}, pct=0))
H("round 2.5 and 3.5", cache({"p1": prof(P1, 2.5, 3.5)}, pct=0))
H("round 94.5 and 95.5", cache({"p1": prof(P1, 94.5, 95.5)}, pct=94))
H("round 98.5 and 99.5", cache({"p1": prof(P1, 98.5, 99.5)}, pct=90))
H("round 99.49999 and 99.50001", cache({"p1": prof(P1, 99.49999, 99.50001)}, pct=90))
H("round 0.49999999999999994", cache({"p1": prof(P1, 0.49999999999999994, 0.5000000000000001)}, pct=0))
H("round negative -0.4 and -0.5", cache({"p1": prof(P1, -0.4, -0.5)}, pct=-5))
H("round -1.5 -2.5", cache({"p1": prof(P1, -1.5, -2.5)}, pct=-5))
H("many digits", cache({"p1": prof(P1, 12.3456789, 77.7777777)}, pct=10))
H("round 100.5", cache({"p1": prof(P1, 100.5, 0.5)}, pct=0))
H("float five equal to int week", cache({"p1": prof(P1, 96.0, 96)}))

# path matching
H("cache path with trailing slash", cache({"p1": prof("{HOME}\\p1\\", 99, 99)}))
H("cache path with forward slashes", cache({"p1": prof("{HOME}/p1", 99, 99)}))
H("cache path other case", cache({"p1": prof("{HOME}\\P1", 99, 99)}))
H("cache path upper-cased drive", cache({"p1": prof("{HOME}\\p1".upper(), 99, 99)}))
H("cache path with dotdot", cache({"p1": prof("{HOME}\\x\\..\\p1", 99, 99)}))
H("cache path with dot", cache({"p1": prof("{HOME}\\.\\p1\\.", 99, 99)}))
H("cache path double separators", cache({"p1": prof("{HOME}\\\\p1\\\\", 99, 99)}))
H("env path with trailing slash", cache({"p1": prof(P1, 99, 99)}), env="{HOME}\\p1\\")
H("env path with forward slashes", cache({"p1": prof(P1, 99, 99)}), env="{HOME}/p1")
H("env path with dotdot", cache({"p1": prof(P1, 99, 99)}), env="{HOME}\\a\\b\\..\\..\\p1")
H("env path other case", cache({"p1": prof(P1, 99, 99)}), env="{HOME}\\P1".upper())
H("relative cache path", cache({"p1": prof("rel\\p1", 99, 99)}), env="{CWD}\\rel\\p1")
H("relative cache path with dotdot", cache({"p1": prof("..\\sibling\\p1", 99, 99)}),
  env="{CWD}\\..\\sibling\\p1")
H("relative env path", cache({"p1": prof("{CWD}\\rel2\\p1", 99, 99)}), env="rel2\\p1")
H("both relative", cache({"p1": prof("rel3", 99, 99)}), env="rel3\\.")
H("rooted without drive", cache({"p1": prof("\\rooted\\p", 99, 99)}), env="{DRIVE}\\rooted\\p")
H("drive root with trailing slash", cache({"p1": prof("{DRIVE}/", 99, 99)}), env="{DRIVE}\\")
H("drive root vs no slash", cache({"p1": prof("{DRIVE}\\", 99, 99)}), env="{DRIVE}\\.")
H("vietnamese path different case", cache({"p1": prof("{HOME}\\Đồ Án\\p", 99, 99)}),
  env="{HOME}\\đồ án\\p")
H("unc path trailing slash and case", cache({"p1": prof("\\\\SRV\\Share\\p", 99, 99)}),
  env="\\\\srv\\share\\p\\")
H("unc path forward slashes", cache({"p1": prof("//srv/share/p", 99, 99)}), env="\\\\srv\\share\\p")
H("empty cache path means cwd", cache({"p1": prof("", 99, 99)}), env="{CWD}")
H("path differs by one char", cache({"p1": prof("{HOME}\\p10", 99, 99)}))
H("default config dir (env unset)", cache({"default": prof("{HOME}\\.claude", 99, 99)}), env=None)
H("default config dir when env empty", cache({"default": prof("{HOME}\\.claude", 99, 99)}), env="")
H("env set so default profile does not match", cache({"default": prof("{HOME}\\.claude", 99, 99)}))
H("default dir not in cache", cache({"x": prof("{HOME}\\x", 99, 99)}), env=None)
H("duplicate path first wins (first lacks five)",
  cache({"a": {"path": P1, "err": "x"}, "b": prof(P1, 99, 99)}))
H("duplicate path first wins (first blocks)",
  cache({"a": prof(P1, 99, 99), "b": prof(P1, 1, 1), "c": prof("{HOME}\\c", 5, 5)}))

# best profile selection
H("best: single other", blk())
H("best: no other profiles", cache({"p1": prof(P1, 99, 99)}))
H("best: others without five -> ?", cache({"p1": prof(P1, 99, 99), "o": {"path": "x", "err": "e"},
                                           "o2": {"path": "y"}}))
H("best: tie keeps first in file order",
  cache({"p1": prof(P1, 99, 99), "a": prof("{HOME}\\a", 50, 10), "b": prof("{HOME}\\b", 10, 50),
         "c": prof("{HOME}\\c", 60, 60)}))
H("best: min of several uses max(five,week)",
  cache({"p1": prof(P1, 99, 99), "a": prof("{HOME}\\a", 5, 90), "b": prof("{HOME}\\b", 40, 41),
         "c": prof("{HOME}\\c", 39, 10)}))
H("best: other listed before current", cache({"z": prof("{HOME}\\z", 3, 3), "p1": prof(P1, 99, 99),
                                              "y": prof("{HOME}\\y", 2, 2)}))
H("best: tie between int and float",
  cache({"p1": prof(P1, 99, 99), "a": prof("{HOME}\\a", 20, 30), "b": prof("{HOME}\\b", 30.0, 1)}))
H("best: other over limit still can be best",
  cache({"p1": prof(P1, 99, 99), "a": prof("{HOME}\\a", 100, 100), "b": prof("{HOME}\\b", 99, 100)}))
H("best: other with error but with five",
  cache({"p1": prof(P1, 99, 99), "a": prof("{HOME}\\a", 10, 10, err="HTTP 429"), "b": prof("{HOME}\\b", 50, 50)}))
H("best: name with quotes and Vietnamese",
  cache({"p1": prof(P1, 99, 99), "Tài khoản 'phụ'": prof("{HOME}\\a", 1, 1)}))
H("current name with Vietnamese and quotes",
  cache({"Công ty \"A\"": prof(P1, 99, 99), "b": prof("{HOME}\\b", 1, 1)}))
H("best: other bool values", cache({"p1": prof(P1, 99, 99), "a": {"path": "x", "five": True, "week": 0}}))
H("current with extra keys", cache({"p1": prof(P1, 99, 99, five_reset="2026-11-19T10:00:00Z", extra=[1])}))
H("four profile realistic",
  cache({"main": prof("{HOME}\\.claude", 31, 12), "work": prof(P1, 97.0, 45.5),
         "side": prof("{HOME}\\s", 12.25, 80), "old": {"path": "{HOME}\\o", "err": "HTTP 401"}},
        pct=95))


def run_hook_case(case):
    inp = subst(case["input"])
    d = Path(os.environ["LOCALAPPDATA"]) / "claude-usage"
    d.mkdir(parents=True, exist_ok=True)
    f = d / "usage_cache.json"
    f.unlink(missing_ok=True)
    if inp["cache_text"] is not None:
        f.write_bytes(inp["cache_text"].encode("utf-8"))
    elif inp["cache"] is not None:
        f.write_bytes(json.dumps(inp["cache"], ensure_ascii=False).encode("utf-8"))
    if inp["env_config_dir"] is None:
        os.environ.pop("CLAUDE_CONFIG_DIR", None)
    else:
        os.environ["CLAUDE_CONFIG_DIR"] = inp["env_config_dir"]
    got = []
    promptguard._err = got.append
    promptguard.time = types.SimpleNamespace(time=lambda: inp["now"])
    code = promptguard.run()
    assert len(got) <= 1
    return {"code": code, "stderr": got[0] if got else None}


hook_cases = []
for c in HOOK:
    c["expected"] = run_hook_case(c)
    hook_cases.append(c)
assert len({c["name"] for c in hook_cases}) == len(hook_cases), "duplicate case names"
save("hook", hook_cases)
n_block = sum(1 for c in hook_cases if c["expected"]["code"] == 2)
print(f"  hook: {n_block} blocking cases, {len(hook_cases) - n_block} allowing cases")

# ------------------------------------------------------------------------------------------ hookinstall
OLD_CMD = '"C:\\old\\claude-usage.exe" hook'
OTHER = {"type": "command", "command": "my-own-hook.exe"}
HI = []


def ours(cmd=OLD_CMD):
    return {"type": "command", "command": cmd}


def grp(*hooks, **kw):
    g = {"hooks": list(hooks)}
    g.update(kw)
    return g


def P(name="p", settings=None, text=None, bak=None, no_dir=False):
    return {"name": name, "settings": settings, "settings_text": text, "bak_text": bak, "no_dir": no_dir}


def C(name, profiles, ops, deviation=False):
    if isinstance(profiles, dict):
        profiles = [profiles]
    HI.append({"name": name, "input": {"profiles": profiles, "ops": ops}, "deviation": deviation})


RICH = {"model": "x", "permissions": {"allow": ["Bash(ls)"], "deny": []}, "env": {"A": "1"},
        "hooks": {"PreToolUse": [grp({"type": "command", "command": "echo hi"})],
                  "UserPromptSubmit": [grp(OTHER)]}, "tail": 1.5}

C("install into absent file", P(), ["install", "status"])
C("install into absent profile dir", P(no_dir=True), ["install", "status"])
C("install into empty object", P(settings={}), ["install"])
C("install is idempotent", P(), ["install", "install", "install", "status"])
C("install into rich settings keeps everything and backs up", P(settings=RICH), ["install", "install"])
C("install replaces stale entry of ours", P(settings={"hooks": {"UserPromptSubmit": [grp(ours())]}}), ["install"])
C("install with ours and other in the same group",
  P(settings={"a": 1, "hooks": {"UserPromptSubmit": [grp(OTHER, ours()), grp(ours("claude-usage hook"))]}}),
  ["install"])
C("install with ours twice", P(settings={"hooks": {"UserPromptSubmit": [grp(ours()), grp(ours(OLD_CMD + "  "))]}}),
  ["install", "status"])
C("install keeps preexisting backup", P(settings=RICH, bak="OLD BACKUP"), ["install", "install"])
C("install with unicode content", P(settings={"model": "Tiếng Việt – đủ", "b": ["ü", "日本"]}), ["install"])
C("install numbers keep int/float", P(settings={"i": 5, "f": 1.0, "e": 1e3, "neg": -2, "big": 12345678901,
                                                 "n": None, "t": True}), ["install"])
C("install hooks empty object moves to the end", P(settings={"hooks": {}, "a": 1, "b": 2}), ["install"])
C("install hooks with only other event", P(settings={"hooks": {"Stop": [grp(OTHER)]}, "z": 0}), ["install"])
C("install UserPromptSubmit empty list", P(settings={"hooks": {"UserPromptSubmit": [], "Stop": []}}), ["install"])
C("install UserPromptSubmit falsy zero", P(settings={"hooks": {"UserPromptSubmit": 0, "Stop": [1]}}), ["install"])
C("install group without hooks key is dropped", P(settings={"hooks": {"UserPromptSubmit": [{"matcher": "x"}, grp(OTHER)]}}),
  ["install"])
C("install group with empty hooks list is dropped",
  P(settings={"hooks": {"UserPromptSubmit": [grp(), grp(OTHER), grp()]}}), ["install"])
C("install non-dict hook entries are kept", P(settings={"hooks": {"UserPromptSubmit": [grp("x", 5, None, OTHER)]}}),
  ["install"])
C("install matcher group metadata kept",
  P(settings={"hooks": {"UserPromptSubmit": [grp(OTHER, matcher="*", extra={"k": [1, 2]})]}}), ["install"])
C("install key order: UserPromptSubmit before other event",
  P(settings={"hooks": {"UserPromptSubmit": [grp(ours())], "Stop": [grp(OTHER)]}}), ["install"])
C("install invalid json untouched", P(text="{ this is not json"), ["install", "status"])
C("install json null untouched", P(text="null"), ["install", "status"])
C("install truncated json untouched", P(text='{"a": 1,'), ["install"])
C("install BOM json untouched", P(text="\ufeff{}"), ["install", "status"])
C("install empty file untouched", P(text=""), ["install", "status"])
C("install list document raises", P(text="[]"), ["install"])
C("status list document raises", P(text="[1]"), ["status"])
C("install number document raises", P(text="5"), ["install"])
C("install hooks null raises", P(settings={"hooks": None}), ["install"])
C("install hooks list raises", P(settings={"hooks": [1]}), ["install"])
C("install hooks string raises", P(settings={"hooks": "x"}), ["install"])
C("install UserPromptSubmit string raises", P(settings={"hooks": {"UserPromptSubmit": "x"}}), ["install"])
C("install UserPromptSubmit dict raises", P(settings={"hooks": {"UserPromptSubmit": {"a": 1}}}), ["install"])
C("install group is a string raises", P(settings={"hooks": {"UserPromptSubmit": ["x"]}}), ["install"])
C("install group hooks null raises", P(settings={"hooks": {"UserPromptSubmit": [{"hooks": None}]}}), ["install"])
C("install command non-string raises",
  P(settings={"hooks": {"UserPromptSubmit": [grp({"type": "command", "command": 5})]}}), ["install"])
C("status command null raises",
  P(settings={"hooks": {"UserPromptSubmit": [grp({"command": None})]}}), ["status"])
C("status stops at first hit (later garbage never reached)",
  P(settings={"hooks": {"UserPromptSubmit": [grp(ours()), "garbage"]}}), ["status"])
C("status garbage group before hit raises",
  P(settings={"hooks": {"UserPromptSubmit": ["garbage", grp(ours())]}}), ["status"])
C("status group hooks is dict -> not installed", P(settings={"hooks": {"UserPromptSubmit": [{"hooks": {"a": 1}}]}}),
  ["status"])
C("status group hooks is string -> not installed", P(settings={"hooks": {"UserPromptSubmit": [{"hooks": "abc"}]}}),
  ["status"])
C("status group hooks number raises", P(settings={"hooks": {"UserPromptSubmit": [{"hooks": 3}]}}), ["status"])

C("uninstall absent file", P(), ["uninstall"])
C("uninstall absent profile dir", P(no_dir=True), ["uninstall"])
C("uninstall empty object", P(settings={}), ["uninstall"])
C("install then uninstall leaves empty object", P(), ["install", "uninstall", "status", "uninstall"])
C("uninstall rich keeps others", P(settings=RICH), ["install", "uninstall", "status"])
C("uninstall keeps other events", P(settings={"hooks": {"Stop": [grp(OTHER)], "UserPromptSubmit": [grp(ours())]}}),
  ["uninstall"])
C("uninstall removes only the ours entry of a mixed group",
  P(settings={"hooks": {"UserPromptSubmit": [grp(OTHER, ours(), OTHER)]}}), ["uninstall"])
C("uninstall empty groups removed when something removed",
  P(settings={"hooks": {"UserPromptSubmit": [{"matcher": "x"}, grp(ours()), grp()]}}), ["uninstall"])
C("uninstall nothing of ours leaves file byte-identical (empty groups kept)",
  P(settings={"hooks": {"UserPromptSubmit": [grp(), {"matcher": "q"}]}, "a": [1,2]}), ["uninstall"])
C("uninstall does not create a backup", P(settings={"hooks": {"UserPromptSubmit": [grp(ours())]}}), ["uninstall"])
C("uninstall keeps hooks object when other events remain",
  P(settings={"hooks": {"UserPromptSubmit": [grp(ours())], "Stop": []}}), ["uninstall"])
C("uninstall hooks empty object removed when n>0",
  P(settings={"hooks": {"UserPromptSubmit": [grp(ours())]}, "x": 1}), ["uninstall"])
C("uninstall marker variants",
  P(settings={"hooks": {"UserPromptSubmit": [grp(
      ours("Claude-Usage.EXE hook"), ours("claude-usage hook   "), ours("claude-usage hook now"),
      ours("claude-usagehook"), ours("echo hook"), ours("C:\\Tools\\CLAUDE-USAGE\\run.exe hook"))]}}),
  ["status", "uninstall"])
C("uninstall invalid json", P(text="{ no"), ["uninstall"])
C("uninstall list document raises", P(text="[]"), ["uninstall"])
C("uninstall hooks null raises", P(settings={"hooks": None}), ["uninstall"])

C("status variants none", P(settings={"hooks": {"UserPromptSubmit": [grp(OTHER)]}}), ["status"])
C("status no hooks key", P(settings={"a": 1}), ["status"])
C("status installed", P(settings={"hooks": {"UserPromptSubmit": [grp(ours())]}}), ["status"])
C("status falsy containers", P(settings={"hooks": {"UserPromptSubmit": ""}}), ["status"])
C("status hooks falsy zero", P(settings={"hooks": 0}), ["status"])
C("status absent", P(), ["status"])

C("multi profile mix", [P("a", settings={"x": 1}), P("b", text="{bad"), P("c"), P("d", settings=RICH, bak="B"),
                         P("e", no_dir=True)],
  ["status", "install", "status", "uninstall", "status"])

# repair (Python: always rewrites on any mismatch; Rust R1: only when the old exe no longer exists)
C("repair stale nonexistent exe", P(settings={"hooks": {"UserPromptSubmit": [grp(ours())]}, "k": 1}), ["repair"])
C("repair same command leaves file", P(settings={"hooks": {"UserPromptSubmit": [grp(ours(FAKE_CMD))]}}), ["repair"])
C("repair absent file", P(), ["repair"])
C("repair empty object", P(settings={}), ["repair"])
C("repair no ours", P(settings=RICH), ["repair"])
C("repair invalid json", P(text="{bad"), ["repair"])
C("repair does not make a backup", P(settings={"hooks": {"UserPromptSubmit": [grp(ours())]}}), ["repair"])
C("repair python dev-form command nonexistent",
  P(settings={"hooks": {"UserPromptSubmit": [grp(ours('"C:\\nope\\python.exe" "C:\\nope\\claude-usage\\main.py" hook'))]}}),
  ["repair", "status"])
C("repair unquoted command nonexistent",
  P(settings={"hooks": {"UserPromptSubmit": [grp(OTHER, ours("C:\\old dir\\claude-usage.exe hook"))]}}), ["repair"])
C("repair keeps other hooks and order",
  P(settings={"a": 1, "hooks": {"Stop": [grp(OTHER)], "UserPromptSubmit": [grp(OTHER, ours())]}}), ["repair", "repair"])
C("repair several stale entries collapse into one",
  P(settings={"hooks": {"UserPromptSubmit": [grp(ours()), grp(ours('"D:\\gone\\claude-usage.exe" hook'))]}}), ["repair"])
C("repair after install is a no-op", P(), ["install", "repair"])
C("repair list document skipped", P(text="[]"), ["repair"])
C("repair falsy document skipped", P(text="0"), ["repair"])
C("repair ours with existing exe: python rewrites, rust leaves alone (R1)",
  P(settings={"hooks": {"UserPromptSubmit": [grp(ours('"{EXISTING_EXE}" hook'))]}}), ["repair"], deviation=True)
C("repair mixed: one existing one missing rewrites all (R1)",
  P(settings={"hooks": {"UserPromptSubmit": [grp(ours('"{EXISTING_EXE}" hook')), grp(ours())]}}), ["repair"])
C("repair multi profile",
  [P("a", settings={"hooks": {"UserPromptSubmit": [grp(ours())]}}), P("b", text="[]"),
   P("c", settings={"hooks": {"UserPromptSubmit": [grp(ours(FAKE_CMD))]}})], ["repair"])


def doc_state(p):
    out = {"exists": p.exists()}
    if out["exists"]:
        text = p.read_bytes().decode("utf-8")
        try:
            out["json"] = json.loads(text)
        except ValueError:
            out["text"] = text
    bak = p.with_name(p.name + ".claude-usage.bak")
    out["bak_exists"] = bak.exists()
    if out["bak_exists"]:
        out["bak_text"] = bak.read_bytes().decode("utf-8")
    return out


def run_hi_case(idx, case):
    inp = subst(case["input"])
    base = Path(_TMP) / "hi" / str(idx)
    profiles, files = [], []
    for i, s in enumerate(inp["profiles"]):
        d = base / f"prof{i}"
        if not s["no_dir"]:
            d.mkdir(parents=True)
        f = d / "settings.json"
        if s["settings_text"] is not None:
            f.write_bytes(s["settings_text"].encode("utf-8"))
        elif s["settings"] is not None:
            f.write_bytes(json.dumps(s["settings"], indent=2, ensure_ascii=False).encode("utf-8"))
        if s["bak_text"] is not None:
            f.with_name(f.name + ".claude-usage.bak").write_bytes(s["bak_text"].encode("utf-8"))
        profiles.append({"name": s["name"], "path": str(d)})
        files.append(f)
    results = []
    for op in inp["ops"]:
        try:
            r = getattr(hookinstall, op)(profiles)
            results.append(None if op == "repair" else [list(x) for x in r])
        except Exception as e:  # noqa: BLE001 - recording Python's behaviour
            results.append({"raises": type(e).__name__})
            break
    exp = {"results": results, "files": [doc_state(f) for f in files]}
    shutil.rmtree(base, ignore_errors=True)
    return exp


hi_cases = []
for i, c in enumerate(HI):
    c["expected"] = run_hi_case(i, c)
    hi_cases.append(c)
assert len({c["name"] for c in hi_cases}) == len(hi_cases), "duplicate case names"
save("hookinstall", hi_cases)
print("  hookinstall: raises in", sum(1 for c in hi_cases if any(isinstance(r, dict) for r in c["expected"]["results"])),
      "cases")

shutil.rmtree(_TMP, ignore_errors=True)
