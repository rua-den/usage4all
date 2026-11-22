//! usage_cache.json: written by the daemon, read by widget / dashboard / hook.

use crate::atomic::atomic_write;
use crate::config::Config;
use crate::paths::Paths;
use serde_json::{Map, Value};
use std::fs;
use std::io;
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// core.load_cache: retry 6 times 25 ms apart (the writer may be swapping the file); `None` on failure.
/// A file holding the JSON `null` also yields `None`, like Python.
pub fn load_cache(paths: &Paths) -> Option<Value> {
    for i in 0..6 {
        let v = fs::read(&paths.cache)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .and_then(|t| serde_json::from_str::<Value>(&t).ok());
        if let Some(v) = v {
            return if v.is_null() { None } else { Some(v) };
        }
        if i < 5 {
            sleep(Duration::from_millis(25));
        }
    }
    None
}

/// Unix time in seconds (Python `time.time()`).
pub fn now_ts() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// The structure `core.write_cache` produces. `profiles` is built from `cfg["profiles"]` in order:
/// `{"path": p.path, **data[name]}` (path first; data keys may override it, like Python's `{**}`).
/// Deviations: a cfg profile item that is not an object with a string `name` is skipped; a `data[name]`
/// that is not an object counts as empty (Python would raise); missing `block_enabled` / `block_pct` /
/// `warn_pct` in cfg become null (Python: KeyError).
pub fn build_cache(
    cfg: &Config,
    data: &Map<String, Value>,
    busy: &[String],
    refresh: Option<Value>,
    ts: f64,
) -> Map<String, Value> {
    let get = |k: &str| cfg.get(k).cloned().unwrap_or(Value::Null);
    let mut busy: Vec<&String> = busy.iter().collect();
    busy.sort();
    let mut out = Map::new();
    out.insert("ts".into(), Value::from(ts));
    out.insert("block_enabled".into(), get("block_enabled"));
    out.insert("block_pct".into(), get("block_pct"));
    out.insert("warn_pct".into(), get("warn_pct"));
    out.insert(
        "busy".into(),
        Value::Array(busy.into_iter().map(|s| Value::String(s.clone())).collect()),
    );
    out.insert("refresh".into(), refresh.unwrap_or(Value::Null));
    let mut profs = Map::new();
    if let Some(Value::Array(list)) = cfg.get("profiles") {
        for p in list {
            let Some(name) = p.get("name").and_then(Value::as_str) else {
                continue;
            };
            let mut entry = Map::new();
            entry.insert("path".into(), p.get("path").cloned().unwrap_or(Value::Null));
            if let Some(Value::Object(d)) = data.get(name) {
                for (k, v) in d {
                    entry.insert(k.clone(), v.clone());
                }
            }
            profs.insert(name.to_string(), Value::Object(entry));
        }
    }
    out.insert("profiles".into(), Value::Object(profs));
    out
}

/// core.write_cache: `usage_cache.tmp` + replace (25 tries, 20 ms apart), last resort a direct write
/// (HANDOFF trap 6). Deviations: JSON is indented by 2 spaces (Python: 1) and non-ASCII is written as
/// UTF-8 instead of `\u` escapes; LF line ends. All semantically identical for any JSON reader.
pub fn write_cache(
    paths: &Paths,
    cfg: &Config,
    data: &Map<String, Value>,
    busy: &[String],
    refresh: Option<Value>,
) -> io::Result<()> {
    write_cache_with(
        paths,
        cfg,
        data,
        busy,
        refresh,
        25,
        Duration::from_millis(20),
    )
}

/// [`write_cache`] with explicit retry parameters (tests).
pub fn write_cache_with(
    paths: &Paths,
    cfg: &Config,
    data: &Map<String, Value>,
    busy: &[String],
    refresh: Option<Value>,
    attempts: u32,
    delay: Duration,
) -> io::Result<()> {
    let out = build_cache(cfg, data, busy, refresh, now_ts());
    let text = serde_json::to_string_pretty(&Value::Object(out)).map_err(io::Error::other)?;
    let tmp = paths.cache.with_extension("tmp");
    atomic_write(&paths.cache, &tmp, &text, attempts, delay)
}
