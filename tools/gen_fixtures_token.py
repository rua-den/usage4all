"""Generate the Rust parity scenarios for fetch + OAuth token refresh from the Python reference (core.py).

    .venv\\Scripts\\python tools\\gen_fixtures_token.py

Writes rust/crates/core/tests/fixtures/token_scenarios.json (deterministic: run twice, identical bytes).
Each scenario = initial credentials text, lock directories already present, scripted fake-server replies,
a list of calls (core.refresh_if_expired / core.fetch) and what Python did: return values, every request the
server received, final credentials, .bak, events.log lines, locks left behind.

Sandbox rules: LOCALAPPDATA points to a temp dir BEFORE `import core`; profiles live in temp dirs; every token
is a fake "test-token-not-real-..." string; core.API / core.TOKEN_URL point to a local fake HTTP server on
127.0.0.1; urllib is forced to bypass any system proxy; core.time is pinned (NOW). No real credentials, no real
network. Numbers derived from the pinned clock are stored as the string "$NOW(<offset_ms>)" (see tmpl()); the
Rust harness substitutes its own clock and accepts a few seconds of tolerance.
"""
import json
import os
import re
import shutil
import socket
import sys
import tempfile
import threading
import time as _time
import types
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

_TMP = tempfile.mkdtemp(prefix="cu-token-fixtures-")
os.environ["LOCALAPPDATA"] = os.path.join(_TMP, "localappdata")
for _k in list(os.environ):
    if _k.lower().endswith("_proxy"):
        del os.environ[_k]

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
for _s in (sys.stdout, sys.stderr):
    try:
        _s.reconfigure(encoding="utf-8")
    except Exception:
        pass

import core  # noqa: E402

assert str(core.DATA).startswith(_TMP), "core.DATA is not sandboxed"
urllib.request.install_opener(urllib.request.build_opener(urllib.request.ProxyHandler({})))

OUT = ROOT / "rust" / "crates" / "core" / "tests" / "fixtures" / "token_scenarios.json"
NOW = 1_800_000_000.0
NOW_MS = int(NOW * 1000)
DAY = 86_400_000
core.time = types.SimpleNamespace(time=lambda: NOW, sleep=_time.sleep)

# ------------------------------------------------------------------------------------------ fake server
STATE = {"queues": {"usage": [], "token": []}, "requests": []}
KEEP = {"usage": ["authorization", "anthropic-beta"], "token": ["content-type", "user-agent"]}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _serve(self):
        ep = "usage" if self.path == "/api/oauth/usage" else "token" if self.path == "/v1/oauth/token" else "other"
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) if n else b""
        body = None
        if raw:
            try:
                body = json.loads(raw.decode("utf-8"))
            except Exception:
                body = {"$raw": raw.decode("utf-8", "replace")}
        hdrs = {}
        for k in KEEP.get(ep, []):
            v = self.headers.get(k)
            if v is not None:
                hdrs[k] = re.sub(r"^(claude-usage/).*$", r"\1*", v)
        STATE["requests"].append({"ep": ep, "method": self.command, "path": self.path, "headers": hdrs, "body": body})
        q = STATE["queues"].get(ep) or []
        step = q.pop(0) if q else {"status": 599, "body": {"$raw": "unexpected request"}}
        b = step.get("body")
        data = b["$raw"].encode("utf-8") if isinstance(b, dict) and "$raw" in b else json.dumps(b).encode("utf-8") if b is not None else b""
        self.send_response(step["status"])
        self.send_header("Content-Type", "application/json")
        for k, v in (step.get("headers") or {}).items():
            self.send_header(k, v)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    do_GET = do_POST = _serve


SERVER = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
PORT = SERVER.server_address[1]
threading.Thread(target=SERVER.serve_forever, daemon=True).start()


def closed_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


# ---------------------------------------------------------------------------------------------- helpers
def T(off):
    """Placeholder for a clock-relative number inside credentials text."""
    return f"$NOW({off})"


def subst(text):
    text = re.sub(r'"\$NOWF\((-?\d+)\)"', lambda m: repr(float(NOW_MS + int(m.group(1)))), text)
    return re.sub(r'"\$NOW\((-?\d+)\)"', lambda m: str(NOW_MS + int(m.group(1))), text)


def tmpl(v):
    """Replace ints within 100 days of NOW_MS by "$NOW(offset)" (recursively)."""
    if isinstance(v, bool):
        return v
    if isinstance(v, (int, float)) and abs(v - NOW_MS) <= 100 * DAY and v == int(v):
        return f"$NOW({int(v) - NOW_MS})"
    if isinstance(v, list):
        return [tmpl(x) for x in v]
    if isinstance(v, dict):
        return {k: tmpl(x) for k, x in v.items()}
    return v


def dumps(o):
    return json.dumps(o, ensure_ascii=False, separators=(",", ":"))


def oauth(**kw):
    o = {"accessToken": "test-token-not-real-at-old", "refreshToken": "test-token-not-real-rt-old",
         "expiresAt": T(-60000), "refreshTokenExpiresAt": T(20 * DAY),
         "scopes": ["user:inference", "user:profile"], "subscriptionType": "team"}
    for k, v in kw.items():
        if v is KeyError:
            o.pop(k, None)
        else:
            o[k] = v
    return o


def creds(o=None, **root):
    return dumps({"claudeAiOauth": oauth() if o is None else o, "mcpOAuth": {"keep": 1}, **root})


OK = {"access_token": "test-token-not-real-at-new", "refresh_token": "test-token-not-real-rt-new", "expires_in": 28800,
      "refresh_token_expires_in": 90 * 86400, "scope": "user:inference user:profile"}


def ok(**kw):
    d = dict(OK)
    for k, v in kw.items():
        if v is KeyError:
            d.pop(k, None)
        else:
            d[k] = v
    return {"status": 200, "body": d}


def err(status, body=None, headers=None):
    return {"status": status, "body": body, "headers": headers or {}}


def oerr(status, code, headers=None):
    return err(status, {"error": code, "error_description": "secret-ish detail"}, headers)


def raw(status, text, headers=None):
    return {"status": status, "body": {"$raw": text}, "headers": headers or {}}


USAGE_OK = {"five_hour": {"utilization": 12.5, "resets_at": "2027-01-15T12:00:00.000000+00:00"},
            "seven_day": {"utilization": 34, "resets_at": "2027-01-20T00:00:00+00:00"},
            "extra_usage": {"is_enabled": True, "monthly_limit": 5000, "used_credits": 1234, "utilization": 24.68,
                            "currency": "EUR"},
            "limits": [{"kind": "weekly_scoped", "scope": {"model": {"display_name": "Fable"}}, "percent": 41.5,
                        "resets_at": "2027-01-20T00:00:00+00:00"},
                       {"kind": "other", "percent": 3}]}

SCENARIOS = []


def scen(name, creds_text, calls, token=(), usage=(), locks=None, token_down=False, usage_down=False, profile="acc1"):
    SCENARIOS.append({"name": name, "profile": profile, "creds": creds_text, "locks": locks or {},
                      "usage": list(usage), "token": list(token), "token_down": token_down, "usage_down": usage_down,
                      "calls": calls})


def R(**kw):
    return {"fn": "refresh", **kw}


def F(**kw):
    return {"fn": "fetch", **kw}


# ------------------------------------------------------------------------------------------- scenarios
# refresh: nothing to do
scen("refresh_still_valid", creds(oauth(expiresAt=T(3600000))), [R()], token=[ok()])
scen("refresh_expires_in_future_edge", creds(oauth(expiresAt=T(5000))), [R()], token=[ok()])
scen("refresh_no_refresh_token_empty", creds(oauth(refreshToken="")), [R()], token=[ok()])
scen("refresh_no_refresh_token_missing", creds(oauth(refreshToken=KeyError)), [R()], token=[ok()])
scen("refresh_refresh_token_not_string", creds(oauth(refreshToken=12345)), [R()], token=[ok()])
scen("refresh_expires_at_string", creds(oauth(expiresAt="soon")), [R()], token=[ok()])
scen("refresh_expires_at_null", creds(oauth(expiresAt=None)), [R()], token=[ok()])
scen("refresh_missing_file", None, [R()], token=[ok()])
scen("refresh_garbage_file", "not json {", [R()], token=[ok()])
scen("refresh_empty_object", "{}", [R()], token=[ok()])
scen("refresh_oauth_null", '{"claudeAiOauth":null}', [R()], token=[ok()])
scen("refresh_root_list", "[]", [R()], token=[ok()])
# refresh: success paths
scen("refresh_success_rotation", creds(), [R()], token=[ok()])
scen("refresh_success_no_new_refresh_token", creds(), [R()], token=[ok(refresh_token=KeyError)])
scen("refresh_success_empty_new_refresh_token", creds(), [R()], token=[ok(refresh_token="")])
scen("refresh_scope_updated", creds(), [R()], token=[ok(scope="user:inference user:profile user:extra")])
scen("refresh_scope_absent_keeps_scopes", creds(), [R()], token=[ok(scope=KeyError)])
scen("refresh_scope_blank_keeps_scopes", creds(), [R()], token=[ok(scope="   ")])
scen("refresh_scope_added_when_file_has_none", creds(oauth(scopes=KeyError)), [R()], token=[ok()])
scen("refresh_empty_scopes_no_scope_in_body", creds(oauth(scopes=[])), [R()], token=[ok()])
scen("refresh_rte_min_keeps_old", creds(), [R()], token=[ok(refresh_token_expires_in=90 * 86400)])
scen("refresh_rte_min_takes_new_when_smaller", creds(), [R()], token=[ok(refresh_token_expires_in=5 * 86400)])
scen("refresh_rte_absent_in_file_not_added", creds(oauth(refreshTokenExpiresAt=KeyError)), [R()], token=[ok()])
scen("refresh_rte_absent_in_reply_unchanged", creds(), [R()], token=[ok(refresh_token_expires_in=KeyError)])
scen("refresh_rte_string_in_file_untouched", creds(oauth(refreshTokenExpiresAt="never")), [R()], token=[ok()])
scen("refresh_expires_at_missing", creds(oauth(expiresAt=KeyError)), [R()], token=[ok()])
scen("refresh_expires_in_float", creds(), [R()], token=[ok(expires_in=3600.75)])
scen("refresh_expires_in_bool_accepted", creds(), [R()], token=[ok(expires_in=True)])
scen("refresh_unknown_keys_and_order_preserved",
     dumps({"zeta": 1, "claudeAiOauth": {"extraFirst": [1, {"a": None}], "accessToken": "test-token-not-real-at-old",
                                        "refreshToken": "test-token-not-real-rt-old", "expiresAt": T(-60000),
                                        "scopes": ["user:inference"], "rateLimitTier": "default_claude_max_5x",
                                        "subscriptionType": "max", "tail": {"k": "v"}},
            "mcpOAuth": {"srv|abc": {"accessToken": "test-token-not-real-mcp"}}, "organizationUuid": "dummy"}),
     [R()], token=[ok()])
scen("refresh_non_ascii_preserved",
     dumps({"claudeAiOauth": oauth(subscriptionType="Pro-ü-日本-\U0001f600"), "ключ": "æøå"}),
     [R()], token=[ok()], profile="acc-æ")
# refresh: scopes that cannot be joined (error before the POST, after the .bak)
scen("refresh_scopes_non_string_item", creds(oauth(scopes=["a", 1])), [R()], token=[ok()])
scen("refresh_scopes_is_string", creds(oauth(scopes="ab")), [R()], token=[ok()])
# refresh: invalid_scope handling
scen("refresh_invalid_scope_retry_ok", creds(), [R()], token=[oerr(400, "invalid_scope"), ok()])
scen("refresh_invalid_scope_twice", creds(), [R()], token=[oerr(400, "invalid_scope"), oerr(400, "invalid_scope")])
scen("refresh_invalid_scope_without_scopes", creds(oauth(scopes=KeyError)), [R()], token=[oerr(400, "invalid_scope"), ok()])
scen("refresh_invalid_scope_then_invalid_grant", creds(), [R(), R()], token=[oerr(400, "invalid_scope"), oerr(400, "invalid_grant"), ok()])
scen("refresh_invalid_scope_then_429", creds(), [R()], token=[oerr(400, "invalid_scope"), oerr(429, "rate_limited", {"Retry-After": "9"})])
# refresh: dead tokens
scen("refresh_invalid_grant_dead_then_no_resend", creds(), [R(), R()], token=[oerr(400, "invalid_grant"), ok()])
scen("refresh_invalid_grant_type_object", creds(), [R()], token=[err(400, {"error": {"type": "invalid_grant"}})])
scen("refresh_dead_until_file_changes", creds(),
     [R(), R(),
      R(rewrite_before=creds(oauth(refreshToken="test-token-not-real-rt-relogin"))),
      R()],
     token=[oerr(400, "invalid_grant"), ok()])
scen("refresh_rte_already_past", creds(oauth(refreshTokenExpiresAt=T(-60000))), [R()], token=[ok()])
scen("refresh_rte_past_float", creds(oauth(refreshTokenExpiresAt="$NOWF(-60000)")), [R()], token=[ok()])
# refresh: HTTP errors
scen("refresh_http_429_retry_after", creds(), [R()], token=[oerr(429, "rate_limited", {"Retry-After": "120"})])
scen("refresh_http_429_no_header", creds(), [R()], token=[oerr(429, "rate_limited")])
scen("refresh_http_429_retry_after_not_digits", creds(), [R()], token=[oerr(429, "rate_limited", {"Retry-After": "abc"})])
scen("refresh_http_429_retry_after_float", creds(), [R()], token=[oerr(429, "rate_limited", {"Retry-After": "1.5"})])
scen("refresh_http_500_no_body", creds(), [R()], token=[err(500)])
scen("refresh_http_503_retry_after", creds(), [R()], token=[oerr(503, "temporarily_unavailable", {"Retry-After": "7"})])
scen("refresh_http_403_forbidden", creds(), [R()], token=[oerr(403, "forbidden")])
scen("refresh_http_401", creds(), [R()], token=[oerr(401, "invalid_client")])
scen("refresh_http_502_html_body", creds(), [R()], token=[raw(502, "<html>secret</html>")])
scen("refresh_error_type_object", creds(), [R()], token=[err(429, {"error": {"type": "rate_limit_error", "message": "secret"}})])
scen("refresh_error_junk_value", creds(), [R()], token=[err(400, {"error": "Secret-Value 123 !"})])
scen("refresh_error_too_long", creds(), [R()], token=[err(400, {"error": "a" * 41})])
scen("refresh_error_exactly_40", creds(), [R()], token=[err(400, {"error": "a" * 40})])
scen("refresh_error_number", creds(), [R()], token=[err(400, {"error": 42})])
scen("refresh_error_list_body", creds(), [R()], token=[err(400, ["error"])])
scen("refresh_error_uppercase", creds(), [R()], token=[err(400, {"error": "Invalid_Grant"})])
scen("refresh_error_type_not_string", creds(), [R()], token=[err(400, {"error": {"type": 7}})])
# refresh: malformed 200
scen("refresh_malformed_weird_object", creds(), [R()], token=[{"status": 200, "body": {"weird": 1}}])
scen("refresh_malformed_not_json", creds(), [R()], token=[raw(200, "not json")])
scen("refresh_malformed_empty_body", creds(), [R()], token=[raw(200, "")])
scen("refresh_malformed_list", creds(), [R()], token=[{"status": 200, "body": []}])
scen("refresh_malformed_expires_in_string", creds(), [R()], token=[ok(expires_in="28800")])
scen("refresh_malformed_empty_access_token", creds(), [R()], token=[ok(access_token="")])
scen("refresh_malformed_no_access_token", creds(), [R()], token=[ok(access_token=KeyError)])
scen("refresh_malformed_no_expires_in", creds(), [R()], token=[ok(expires_in=KeyError)])
scen("refresh_server_down", creds(), [R()], token_down=True)
# refresh: locks
scen("refresh_lock_fresh_profile", creds(), [R()], token=[ok()], locks={"profile": 5})
scen("refresh_lock_fresh_real", creds(), [R()], token=[ok()], locks={"real": 5})
scen("refresh_lock_fresh_both", creds(), [R()], token=[ok()], locks={"profile": 5, "real": 5})
scen("refresh_lock_stale_profile", creds(), [R()], token=[ok()], locks={"profile": 200})
scen("refresh_lock_stale_real", creds(), [R()], token=[ok()], locks={"real": 200})
scen("refresh_lock_stale_both", creds(), [R()], token=[ok()], locks={"profile": 200, "real": 200})
scen("refresh_lock_age_just_under_stale", creds(), [R()], token=[ok()], locks={"profile": 80})
scen("refresh_lock_fresh_real_stale_profile", creds(), [R()], token=[ok()], locks={"profile": 200, "real": 5})
scen("refresh_lock_then_released_then_ok", creds(), [R(), R(unlock=True)], token=[ok()], locks={"profile": 5})
# refresh: the CLI refreshes between our first read and the lock
cli_new = creds(oauth(accessToken="test-token-not-real-at-cli", refreshToken="test-token-not-real-rt-cli",
                      expiresAt=T(28800000)))
scen("refresh_cli_refreshed_before_lock", creds(), [R(rewrite_before_lock=cli_new)], token=[ok()])
scen("refresh_cli_same_rt_but_valid", creds(), [R(rewrite_before_lock=creds(oauth(expiresAt=T(28800000))))], token=[ok()])
scen("refresh_cli_new_rt_still_expired", creds(),
     [R(rewrite_before_lock=creds(oauth(refreshToken="test-token-not-real-rt-cli")))], token=[ok()])
scen("refresh_garbage_under_lock", creds(), [R(rewrite_before_lock="not json")], token=[ok()])
scen("refresh_missing_key_under_lock", creds(), [R(rewrite_before_lock="{}")], token=[ok()])
scen("refresh_list_under_lock", creds(), [R(rewrite_before_lock="[]")], token=[ok()])
scen("refresh_oauth_null_under_lock", creds(), [R(rewrite_before_lock='{"claudeAiOauth":null}')], token=[ok()])
scen("refresh_oauth_list_under_lock", creds(), [R(rewrite_before_lock='{"claudeAiOauth":[]}')], token=[ok()])
scen("refresh_expires_at_string_under_lock", creds(),
     [R(rewrite_before_lock=creds(oauth(expiresAt="x")))], token=[ok()])
scen("refresh_file_deleted_under_lock", creds(), [R(rewrite_before_lock="$DELETE")], token=[ok()])
# fetch
HDR = None
scen("fetch_success_full", creds(oauth(expiresAt=T(3600000))), [F()], usage=[{"status": 200, "body": USAGE_OK}])
scen("fetch_utilization_zero_or_null", creds(oauth(expiresAt=T(3600000))), [F()],
     usage=[{"status": 200, "body": {"five_hour": {"utilization": None, "resets_at": None}, "seven_day": {"utilization": 0.0}}}])
scen("fetch_blocks_missing_or_null", creds(oauth(expiresAt=T(3600000))), [F()],
     usage=[{"status": 200, "body": {"five_hour": None, "extra_usage": None, "limits": None}}])
scen("fetch_empty_object", creds(oauth(expiresAt=T(3600000), subscriptionType=KeyError)), [F()],
     usage=[{"status": 200, "body": {}}])
scen("fetch_blocks_empty_falsy_values", creds(oauth(expiresAt=T(3600000))), [F()],
     usage=[{"status": 200, "body": {"five_hour": [], "seven_day": "", "limits": []}}])
scen("fetch_five_hour_list_raises", creds(oauth(expiresAt=T(3600000))), [F()],
     usage=[{"status": 200, "body": {"five_hour": [1]}}])
scen("fetch_body_list_raises", creds(oauth(expiresAt=T(3600000))), [F()], usage=[{"status": 200, "body": []}])
scen("fetch_body_null_raises", creds(oauth(expiresAt=T(3600000))), [F()], usage=[{"status": 200, "body": None}])
scen("fetch_not_json", creds(oauth(expiresAt=T(3600000))), [F()], usage=[raw(200, "<html>")])
scen("fetch_empty_200", creds(oauth(expiresAt=T(3600000))), [F()], usage=[raw(200, "")])
scen("fetch_201_is_success", creds(oauth(expiresAt=T(3600000))), [F()],
     usage=[{"status": 201, "body": {"five_hour": {"utilization": 1}}}])
for code, hd in ((401, None), (403, None), (429, {"Retry-After": "30"}), (429, None), (429, {"Retry-After": "1.5"}),
                 (429, {"Retry-After": "0"}), (500, None), (503, {"Retry-After": "120"}), (404, None)):
    scen(f"fetch_http_{code}_{'ra' + hd['Retry-After'].replace('.', '_') if hd else 'plain'}",
         creds(oauth(expiresAt=T(3600000))), [F()], usage=[err(code, {"error": "x"}, hd)])
scen("fetch_expired_token", creds(), [F()], usage=[{"status": 200, "body": USAGE_OK}])
scen("fetch_expires_at_missing_is_expired", creds(oauth(expiresAt=KeyError)), [F()], usage=[{"status": 200, "body": USAGE_OK}])
scen("fetch_expires_at_string_raises", creds(oauth(expiresAt="soon")), [F()], usage=[{"status": 200, "body": USAGE_OK}])
scen("fetch_expires_at_null_raises", creds(oauth(expiresAt=None)), [F()], usage=[{"status": 200, "body": USAGE_OK}])
scen("fetch_missing_file", None, [F()], usage=[{"status": 200, "body": USAGE_OK}])
scen("fetch_garbage_file", "nope", [F()], usage=[{"status": 200, "body": USAGE_OK}])
scen("fetch_no_oauth_key", "{}", [F()], usage=[{"status": 200, "body": USAGE_OK}])
scen("fetch_oauth_null_raises", '{"claudeAiOauth":null}', [F()], usage=[{"status": 200, "body": USAGE_OK}])
scen("fetch_no_access_token_raises", creds(oauth(expiresAt=T(3600000), accessToken=KeyError)), [F()],
     usage=[{"status": 200, "body": USAGE_OK}])
scen("fetch_access_token_number_raises", creds(oauth(expiresAt=T(3600000), accessToken=5)), [F()],
     usage=[{"status": 200, "body": USAGE_OK}])
scen("fetch_server_down", creds(oauth(expiresAt=T(3600000))), [F()], usage_down=True)
scen("fetch_after_refresh_uses_new_token", creds(), [R(), F()], token=[ok()],
     usage=[{"status": 200, "body": USAGE_OK}])


# ---------------------------------------------------------------------------------------------- runner
def lock_dirs(profile_dir):
    return {"profile": Path(profile_dir) / ".oauth_refresh.lock", "real": Path(str(os.path.realpath(profile_dir)) + ".lock")}


def run_scenario(i, sc):
    base = Path(_TMP) / f"s{i}"
    shutil.rmtree(base, ignore_errors=True)
    pdir = base / "profile"
    pdir.mkdir(parents=True)
    cred = pdir / ".credentials.json"
    if sc["creds"] is not None:
        cred.write_text(subst(sc["creds"]), "utf-8")
    locks = lock_dirs(pdir)
    for k, age in sc["locks"].items():
        locks[k].mkdir()
        os.utime(locks[k], (NOW - age, NOW - age))
    core._dead.clear()
    core.EVENTS.write_text("", "utf-8")
    STATE["requests"].clear()
    STATE["queues"]["usage"] = [dict(s) for s in sc["usage"]]
    STATE["queues"]["token"] = [dict(s) for s in sc["token"]]
    up = closed_port() if sc["usage_down"] else PORT
    tp = closed_port() if sc["token_down"] else PORT
    core.API = f"http://127.0.0.1:{up}/api/oauth/usage"
    core.TOKEN_URL = f"http://127.0.0.1:{tp}/v1/oauth/token"
    profile = {"name": sc["profile"], "path": str(pdir)}
    results = []
    real_acq = core._acquire
    for call in sc["calls"]:
        if call.get("unlock"):
            for p in locks.values():
                if p.exists():
                    p.rmdir()
        if call.get("rewrite_before") is not None:
            cred.write_text(subst(call["rewrite_before"]), "utf-8")
        hook = call.get("rewrite_before_lock")

        def acq(paths, hook=hook):
            if hook == "$DELETE":
                cred.unlink()
            elif hook is not None:
                cred.write_text(subst(hook), "utf-8")
            return real_acq(paths)

        core._acquire = acq
        try:
            fn = core.refresh_if_expired if call["fn"] == "refresh" else core.fetch
            try:
                r = fn(profile)
            except Exception as e:  # noqa: BLE001 - recording Python's behaviour
                r = {"raises": type(e).__name__}
        finally:
            core._acquire = real_acq
        results.append(tmpl(r))
    final = bak = None
    compact = None
    if cred.exists():
        text = cred.read_text("utf-8")
        try:
            obj = json.loads(text)
            final = tmpl(obj)
            compact = text == json.dumps(obj, ensure_ascii=False, separators=(",", ":"))
        except Exception:
            final = {"$text": text}
    bp = pdir / ".credentials.json.claude-usage.bak"
    if bp.exists():
        btext = bp.read_text("utf-8")
        try:
            bak = tmpl(json.loads(btext))
        except Exception:
            bak = {"$text": btext}
    events = [l[20:] for l in core.EVENTS.read_text("utf-8").splitlines() if l]
    tmp_left = sorted(p.name for p in pdir.iterdir() if p.name.endswith(".tmp"))
    logs_text = core.EVENTS.read_text("utf-8")
    assert "not-real" not in logs_text, "token leaked into events.log"
    out = dict(sc)
    out["expected"] = {"results": results, "requests": tmpl(list(STATE["requests"])), "final_creds": final,
                       "final_compact": compact, "bak": bak, "events": events,
                       "locks_after": {k: locks[k].exists() for k in ("profile", "real")}, "tmp_left": tmp_left}
    return out


def main():
    names = [s["name"] for s in SCENARIOS]
    assert len(names) == len(set(names)), "duplicate scenario names"
    out = [run_scenario(i, sc) for i, sc in enumerate(SCENARIOS)]
    OUT.parent.mkdir(parents=True, exist_ok=True)
    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        json.dump(out, f, ensure_ascii=False, indent=1, allow_nan=False)
        f.write("\n")
    print(f"token_scenarios: {len(out)} scenarios")
    SERVER.shutdown()
    shutil.rmtree(_TMP, ignore_errors=True)


if __name__ == "__main__":
    main()
