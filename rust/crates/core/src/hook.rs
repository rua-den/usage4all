//! Claude Code UserPromptSubmit hook: port of `promptguard.run`. Blocks (exit 2 + stderr text) when the current
//! profile is at/over the block threshold; every uncertain situation fails open (exit 0).
//!
//! The Python function is the oracle (`tools/gen_fixtures_hook.py` -> `tests/fixtures/hook.json`). Python
//! signals "anything unexpected" by raising inside one big `try` -> exit 0; here every such branch returns
//! [`HookResult::allow`].
//!
//! Deviations (all fail-open, none reachable with a cache written by the daemon): the cache must be strict
//! JSON (Python also accepts `NaN`/`Infinity`); string-valued `five`/`week` of *other* profiles (Python would
//! compare them as strings) make the hook allow; integers beyond u64 are read as floats.

use crate::paths::home_dir;
use crate::pyval::{fixed, py_num, truthy};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// Cache older than this many seconds is ignored.
pub const MAX_AGE_SECS: f64 = 900.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookResult {
    /// 0 = allow, 2 = block.
    pub code: i32,
    /// UTF-8 text for stderr when blocking.
    pub stderr: Option<String>,
}

impl HookResult {
    pub fn allow() -> Self {
        HookResult {
            code: 0,
            stderr: None,
        }
    }
}

/// `%LOCALAPPDATA%\claude-usage\usage_cache.json`, falling back to `<home>` when the variable is unset.
/// Read-only: nothing is created (unlike `Paths::from_env`).
pub fn cache_path_from_env() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(home_dir);
    base.join("claude-usage").join("usage_cache.json")
}

/// `CLAUDE_CONFIG_DIR` (empty counts as unset, like Python's `or`), else `<home>\.claude`.
pub fn resolve_config_dir(env: Option<std::ffi::OsString>, home: &Path) -> PathBuf {
    match env.filter(|v| !v.is_empty()) {
        Some(v) => PathBuf::from(v),
        None => home.join(".claude"),
    }
}

/// Python `os.path.normcase(os.path.abspath(p))` on Windows: absolute (GetFullPathNameW, resolves `.`/`..`
/// and `/`, no symlink resolution), no trailing separator (except for a root), lower-cased, `\` separators.
pub fn norm_path(p: &str) -> Option<String> {
    let abs: PathBuf = if p.is_empty() {
        std::env::current_dir().ok()?
    } else {
        std::path::absolute(p).ok()?
    };
    let mut s = abs.to_string_lossy().replace('/', "\\");
    let root = root_len(&s);
    while s.len() > root && s.ends_with('\\') {
        s.pop();
    }
    Some(s.to_lowercase())
}

/// Length of the root prefix (`C:\`, `\\server\share\`, `\`) that a trailing-separator trim must keep.
fn root_len(s: &str) -> usize {
    let b = s.as_bytes();
    if b.len() >= 3 && b[1] == b':' && b[2] == b'\\' {
        return 3;
    }
    if s.starts_with("\\\\") && !s.starts_with("\\\\?\\") {
        let mut seps = 0;
        for (i, c) in s.char_indices().skip(2) {
            if c == '\\' {
                seps += 1;
                if seps == 2 {
                    return i + 1;
                }
            }
        }
        return s.len();
    }
    usize::from(s.starts_with('\\'))
}

/// Python `str()` of a JSON number/bool as used in the f-string `{lim}`.
fn py_str(v: &Value) -> Option<String> {
    match v {
        Value::Bool(b) => Some(if *b { "True" } else { "False" }.into()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i.to_string())
            } else if let Some(u) = n.as_u64() {
                Some(u.to_string())
            } else {
                n.as_f64().map(py_float_repr)
            }
        }
        _ => None,
    }
}

/// Python `repr(float)` (shortest round-trip digits; exponent form when exp10 < -4 or >= 16).
pub fn py_float_repr(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f < 0.0 { "-inf" } else { "inf" }.into();
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0.0" } else { "0.0" }.into();
    }
    let sci = format!("{:e}", f.abs()); // e.g. "9.5e1"
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp10: i32 = exp.parse().unwrap_or(0);
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let sign = if f < 0.0 { "-" } else { "" };
    let body = if !(-4..16).contains(&exp10) {
        let m = if digits.len() > 1 {
            format!("{}.{}", &digits[..1], &digits[1..])
        } else {
            digits
        };
        format!(
            "{m}e{}{:02}",
            if exp10 < 0 { '-' } else { '+' },
            exp10.abs()
        )
    } else if exp10 < 0 {
        format!("0.{}{}", "0".repeat((-exp10 - 1) as usize), digits)
    } else {
        let int_len = exp10 as usize + 1;
        if digits.len() <= int_len {
            format!("{}{}.0", digits, "0".repeat(int_len - digits.len()))
        } else {
            format!("{}.{}", &digits[..int_len], &digits[int_len..])
        }
    };
    format!("{sign}{body}")
}

/// Python `max(a, b)` on two numbers (only the value matters here).
fn max2(a: f64, b: f64) -> f64 {
    if b > a {
        b
    } else {
        a
    }
}

/// `max(q["five"], q["week"])` for a profile known to have "five". `None` = Python would raise.
fn load_of(q: &Map<String, Value>) -> Option<f64> {
    let five = py_num(q.get("five")?)?;
    let week = py_num(q.get("week")?)?;
    Some(max2(five, week))
}

/// Port of `promptguard.run` after the cache has been read. `cache` is the parsed `usage_cache.json`
/// (None = missing/invalid), `now` Unix seconds, `config_dir` the current `CLAUDE_CONFIG_DIR`/default dir.
pub fn evaluate(cache: Option<&Value>, now: f64, config_dir: &Path) -> HookResult {
    decide(cache, now, config_dir).unwrap_or_else(HookResult::allow)
}

fn decide(cache: Option<&Value>, now: f64, config_dir: &Path) -> Option<HookResult> {
    let c = cache?.as_object()?;
    if !c.get("block_enabled").is_some_and(truthy) {
        return None;
    }
    let ts = py_num(c.get("ts")?)?;
    if now - ts > MAX_AGE_SECS {
        return None;
    }
    let me = norm_path(&config_dir.to_string_lossy())?;
    let profs = c.get("profiles")?.as_object()?;

    // next((n, p) ... if normcase(abspath(p["path"])) == me): lazy, stops at the first match; a malformed
    // entry reached before the match makes Python raise.
    let mut cur = None;
    for (n, p) in profs {
        let path = p.as_object()?.get("path")?.as_str()?;
        if norm_path(path)? == me {
            cur = Some((n, p.as_object()?));
            break;
        }
    }
    let (cur_name, cur_p) = cur?;
    if cur_name.is_empty() || !cur_p.contains_key("five") {
        return None;
    }
    let lim_v = c.get("block_pct")?;
    let lim = py_num(lim_v)?;
    let five = py_num(cur_p.get("five")?)?;
    let week = py_num(cur_p.get("week")?)?;
    if max2(five, week) < lim {
        return None;
    }

    // ok = [(n, max(five, week)) for n, q in profs.items() if "five" in q and n != cur]  (whole list is built
    // before min, so a bad entry anywhere raises)
    let mut best: Option<(&String, f64)> = None;
    for (n, q) in profs {
        let has_five = match q {
            Value::Object(o) => o.contains_key("five"),
            // `"five" in <list/str>` works in Python (and then q["five"] raises when it is true)
            Value::Array(a) => {
                if a.iter().any(|x| x.as_str() == Some("five")) {
                    return None;
                }
                false
            }
            Value::String(s) => {
                if s.contains("five") {
                    return None;
                }
                false
            }
            _ => return None,
        };
        if !has_five || n == cur_name {
            continue;
        }
        let v = load_of(q.as_object()?)?;
        if best.is_none_or(|(_, bv)| v < bv) {
            best = Some((n, v));
        }
    }
    let best_name = best.map_or("?", |(n, _)| n.as_str());

    let msg = format!(
        "Profile '{}' đã {}% (5h) / {}% (7d) >= {}%. Đổi sang profile '{}' hoặc tắt chặn trong claude-usage.\n",
        cur_name,
        fixed(five, 0),
        fixed(week, 0),
        py_str(lim_v)?,
        best_name
    );
    Some(HookResult {
        code: 2,
        stderr: Some(msg),
    })
}

/// Read the cache file once (no retries, like Python) and evaluate. Any read/parse failure allows.
pub fn run(cache_file: &Path, now: f64, config_dir: &Path) -> HookResult {
    let cache = std::fs::read(cache_file)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok());
    evaluate(cache.as_ref(), now, config_dir)
}

/// Everything from the environment: cache location, current time, `CLAUDE_CONFIG_DIR` / home.
pub fn run_from_env() -> HookResult {
    let cfg = resolve_config_dir(std::env::var_os("CLAUDE_CONFIG_DIR"), &home_dir());
    run(&cache_path_from_env(), crate::cache::now_ts(), &cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_repr_matches_python() {
        for (f, s) in [
            (95.0, "95.0"),
            (0.5, "0.5"),
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (1.5e-5, "1.5e-05"),
            (0.0001, "0.0001"),
            (123.456, "123.456"),
            (-2.5, "-2.5"),
            (0.1 + 0.2, "0.30000000000000004"),
        ] {
            assert_eq!(py_float_repr(f), s);
        }
    }

    #[test]
    fn trailing_separator_trim_keeps_roots() {
        assert_eq!(root_len("C:\\"), 3);
        assert_eq!(root_len("\\\\srv\\share\\x"), 12);
        assert_eq!(root_len("\\\\srv\\share"), 11);
    }
}
