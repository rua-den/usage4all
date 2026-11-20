//! core.fetch: one usage request for one profile.
//!
//! Return shape (a `serde_json::Value` object, key order as Python):
//! * ok:  `{five, five_reset, week, week_reset, extra, scoped, plan}`
//! * err: `{err, status[, retry_after]}`; `retry_after` is an int or null (HTTP errors only).
//!
//! Deviations from Python (all on paths where Python *raised* out of `fetch` and crashed the poll tick; the Rust
//! version returns `{err: "<PythonExceptionName>", status: 0}` instead): missing / non-string `accessToken`
//! (`KeyError` / `TypeError`), non-numeric `expiresAt` (`TypeError`), non-object `claudeAiOauth` or response
//! body (`AttributeError`). Transport failures use the names documented on [`Http::request`]; a 2xx body that is
//! not JSON gives `JSONDecodeError` (`UnicodeDecodeError` when it is not UTF-8), as in Python.

use crate::config::Profile;
use crate::http::{Http, Response};
use crate::parse::{parse_extra, parse_scoped};
use crate::pyval::{py_num, truthy};
use serde_json::{json, Map, Value};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const API: &str = "https://api.anthropic.com/api/oauth/usage";
pub const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub const DEAD_MSG: &str = "phiên đăng nhập hết hạn - chạy claude ở profile này rồi /login";

/// Base URLs. Release callers use [`Endpoints::release`]; tests build their own (loopback fake server).
/// There is deliberately no environment-variable override.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub usage: String,
    pub token: String,
}

impl Endpoints {
    pub fn release() -> Endpoints {
        Endpoints {
            usage: API.to_string(),
            token: TOKEN_URL.to_string(),
        }
    }
}

/// Seconds since the epoch as Python's `time.time()`.
pub(crate) fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

pub(crate) fn err_value(err: &str, status: i64) -> Value {
    json!({"err": err, "status": status})
}

/// `int(ra) if ra and ra.isdigit() else None`
pub(crate) fn retry_after_value(ra: Option<&str>) -> Value {
    match ra {
        Some(s) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => {
            Value::from(s.parse::<i64>().unwrap_or(i64::MAX))
        }
        _ => Value::Null,
    }
}

/// Name of the Python exception an I/O failure would have raised.
pub(crate) fn io_exc_name(e: &std::io::Error) -> &'static str {
    match e.kind() {
        std::io::ErrorKind::NotFound => "FileNotFoundError",
        std::io::ErrorKind::PermissionDenied => "PermissionError",
        std::io::ErrorKind::AlreadyExists => "FileExistsError",
        _ => "OSError",
    }
}

/// Python `json.loads(path.read_text("utf-8"))` -> Value, or the exception name.
pub(crate) fn read_json(path: &Path) -> Result<(String, Value), &'static str> {
    let bytes = std::fs::read(path).map_err(|e| io_exc_name(&e))?;
    let text = String::from_utf8(bytes).map_err(|_| "UnicodeDecodeError")?;
    let v = serde_json::from_str(&text).map_err(|_| "JSONDecodeError")?;
    Ok((text, v))
}

/// Python `json.load(r)` on an HTTP body.
pub(crate) fn parse_body(body: &[u8]) -> Result<Value, &'static str> {
    let text = std::str::from_utf8(body).map_err(|_| "UnicodeDecodeError")?;
    serde_json::from_str(text).map_err(|_| "JSONDecodeError")
}

pub fn fetch(http: &Http, ep: &Endpoints, profile: &Profile) -> Value {
    match fetch_inner(http, ep, profile) {
        Ok(v) | Err(v) => v,
    }
}

fn fetch_inner(http: &Http, ep: &Endpoints, profile: &Profile) -> Result<Value, Value> {
    let cred = Path::new(&profile.path).join(".credentials.json");
    let o = match read_json(&cred)
        .ok()
        .and_then(|(_, v)| v.as_object().and_then(|r| r.get("claudeAiOauth").cloned()))
    {
        Some(o) => o,
        None => return Err(err_value("không đọc được credentials", 0)),
    };
    let o = o
        .as_object()
        .ok_or_else(|| err_value("AttributeError", 0))?;
    let exp = o.get("expiresAt").cloned().unwrap_or(Value::from(0));
    let exp = py_num(&exp).ok_or_else(|| err_value("TypeError", 0))?;
    if exp / 1000.0 < now_secs() {
        return Err(err_value(
            "token hết hạn - chạy claude ở profile này 1 lần",
            401,
        ));
    }
    let tok = match o.get("accessToken") {
        None => return Err(err_value("KeyError", 0)),
        Some(Value::String(s)) => s,
        Some(_) => return Err(err_value("TypeError", 0)),
    };
    let auth = format!("Bearer {tok}");
    let resp: Response = match http.request(
        "GET",
        &ep.usage,
        &[
            ("Authorization", &auth),
            ("anthropic-beta", "oauth-2025-04-20"),
        ],
        None,
        Duration::from_secs(20),
    ) {
        Ok(r) => r,
        Err(name) => return Err(err_value(name, 0)),
    };
    if !resp.is_success() {
        let mut m = Map::new();
        m.insert("err".into(), Value::String(format!("HTTP {}", resp.status)));
        m.insert("status".into(), Value::from(resp.status));
        m.insert(
            "retry_after".into(),
            retry_after_value(resp.retry_after.as_deref()),
        );
        return Err(Value::Object(m));
    }
    let d = parse_body(&resp.body).map_err(|n| err_value(n, 0))?;
    let d = d
        .as_object()
        .ok_or_else(|| err_value("AttributeError", 0))?;
    // `(d.get(k) or {})` then `.get(...)`
    let block = |k: &str| -> Result<Map<String, Value>, Value> {
        match d.get(k) {
            Some(v) if truthy(v) => v
                .as_object()
                .cloned()
                .ok_or_else(|| err_value("AttributeError", 0)),
            _ => Ok(Map::new()),
        }
    };
    let f = block("five_hour")?;
    let s = block("seven_day")?;
    // `x.get("utilization") or 0`
    let util = |m: &Map<String, Value>| match m.get("utilization") {
        Some(v) if truthy(v) => v.clone(),
        _ => Value::from(0),
    };
    let get = |m: &Map<String, Value>, k: &str| m.get(k).cloned().unwrap_or(Value::Null);
    let mut out = Map::new();
    out.insert("five".into(), util(&f));
    out.insert("five_reset".into(), get(&f, "resets_at"));
    out.insert("week".into(), util(&s));
    out.insert("week_reset".into(), get(&s, "resets_at"));
    out.insert(
        "extra".into(),
        parse_extra(d.get("extra_usage").unwrap_or(&Value::Null)).unwrap_or(Value::Null),
    );
    out.insert(
        "scoped".into(),
        Value::Array(parse_scoped(d.get("limits").unwrap_or(&Value::Null))),
    );
    out.insert("plan".into(), get(o, "subscriptionType"));
    Ok(Value::Object(out))
}
