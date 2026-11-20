//! config.json: DEFAULTS + user file + auto-detected profiles. The config stays an ordered JSON map (as a
//! Python dict): user files may hold floats where ints are expected, and unknown keys must survive a save.

use crate::atomic::atomic_write;
use crate::paths::Paths;
use crate::pyval::{py_int, truthy};
use crate::MIN_INTERVAL;
use serde_json::{json, Map, Value};
use std::fs;
use std::io;
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

pub type Config = Map<String, Value>;

/// core.DEFAULTS: same keys, same order, same values (`None` -> null).
pub fn defaults() -> Config {
    let v = json!({
        "interval_sec": 180, "warn_pct": 80, "block_pct": 95, "block_enabled": true,
        "autostart": false, "profiles": [], "token_refresh": true,
        "notify_enabled": true, "notify_warn": true, "notify_block": true,
        "widget_enabled": true, "widget_size": 36, "widget_reset": true,
        "widget_anchor": "tray", "widget_offset_x": 0, "widget_offset_y": 0, "aliases": {},
        "widget_locked": true, "widget_sort": "config", "widget_max_cells": 0, "widget_max_len": 0,
        "dash_rect": null, "widget_show_label": true, "widget_show_5h": true, "widget_show_7d": true,
        "widget_show_reset5": true, "widget_show_reset7": true, "widget_tooltip": true,
        "widget_label": "alias", "widget_font": 100, "widget_opacity": 100,
        "widget_theme": "auto", "widget_plate": false, "widget_hidden": [], "widget_style": "rings",
        "widget_layout": "auto", "widget_cx": null, "widget_cy": null, "widget_angle": 0.0
    });
    match v {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

/// One configured account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub path: String,
}

impl Profile {
    pub fn to_value(&self) -> Value {
        json!({"name": self.name, "path": self.path})
    }
}

// ---- lenient typed getters (wrong type / missing -> the given default, never a panic) ----

/// Integer: ints as-is, floats truncated toward zero (user files may hold 36.5), everything else -> `default`.
pub fn get_i64(cfg: &Config, key: &str, default: i64) -> i64 {
    match cfg.get(key) {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_u64().map(|_| i64::MAX))
            .or_else(|| {
                n.as_f64()
                    .filter(|f| f.is_finite())
                    .map(|f| f.trunc() as i64)
            })
            .unwrap_or(default),
        _ => default,
    }
}

/// Float: any JSON number, else `default`.
pub fn get_f64(cfg: &Config, key: &str, default: f64) -> f64 {
    match cfg.get(key) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(default),
        _ => default,
    }
}

/// Bool with Python truthiness when the key is present (`if cfg.get(k)`), `default` when missing.
pub fn get_bool(cfg: &Config, key: &str, default: bool) -> bool {
    cfg.get(key).map_or(default, truthy)
}

/// String value, else `default`.
pub fn get_str(cfg: &Config, key: &str, default: &str) -> String {
    match cfg.get(key) {
        Some(Value::String(s)) => s.clone(),
        _ => default.to_string(),
    }
}

/// `profiles` as a list: items that are not objects with string `name` and `path` are skipped.
pub fn profiles(cfg: &Config) -> Vec<Profile> {
    let Some(Value::Array(a)) = cfg.get("profiles") else {
        return Vec::new();
    };
    a.iter()
        .filter_map(|p| {
            Some(Profile {
                name: p.get("name")?.as_str()?.to_string(),
                path: p.get("path")?.as_str()?.to_string(),
            })
        })
        .collect()
}

/// core.detect_profiles: `<home>\.claude` (named "default (desktop)") if it holds `.credentials.json`, then
/// every subdirectory of `<home>\claude-profiles` that does, sorted like Python's Windows `Path` sort
/// (case-insensitive on the name). Only existence is checked; the credentials are never read.
pub fn detect_profiles(home: &Path) -> Vec<Profile> {
    let mut found = Vec::new();
    let def = home.join(".claude");
    if def.join(".credentials.json").exists() {
        found.push(Profile {
            name: "default (desktop)".into(),
            path: def.to_string_lossy().into_owned(),
        });
    }
    let root = home.join("claude-profiles");
    if root.is_dir() {
        let mut dirs: Vec<_> = fs::read_dir(&root)
            .map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).collect())
            .unwrap_or_default();
        dirs.sort_by_key(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default()
        });
        for d in dirs {
            if d.join(".credentials.json").exists() {
                found.push(Profile {
                    name: d
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    path: d.to_string_lossy().into_owned(),
                });
            }
        }
    }
    found
}

/// core.load_config: DEFAULTS, overlaid with the file's top-level keys (existing keys keep their position,
/// new keys are appended, like `dict.update`); detected profiles if `profiles` is falsy;
/// `interval_sec = max(90, int(x))`.
///
/// Missing file -> defaults. Unreadable / invalid JSON / not an object -> up to 6 tries 25 ms apart, then
/// defaults. Deviations from Python:
/// - a non-empty JSON array of pairs (`[["k", 1]]`, accepted by `dict.update`) is treated as invalid;
/// - NaN / Infinity tokens (accepted by Python's json) make the file invalid here;
/// - where Python's `int(interval_sec)` raises (null, list, dict, bad string, NaN...) the program would
///   crash; here `interval_sec` falls back to 180.
pub fn load_config(paths: &Paths, home: &Path) -> Config {
    let mut cfg = defaults();
    for i in 0..6 {
        match fs::read(&paths.config) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => break,
            Err(_) => {}
            Ok(bytes) => {
                let parsed = String::from_utf8(bytes)
                    .ok()
                    .and_then(|t| serde_json::from_str::<Value>(&t).ok());
                match parsed {
                    Some(Value::Object(m)) => {
                        for (k, v) in m {
                            cfg.insert(k, v);
                        }
                        break;
                    }
                    Some(Value::Array(a)) if a.is_empty() => break, // dict.update([]) is a no-op
                    _ => {}
                }
            }
        }
        if i < 5 {
            sleep(Duration::from_millis(25));
        }
    }
    if !cfg.get("profiles").is_some_and(truthy) {
        let det: Vec<Value> = detect_profiles(home)
            .iter()
            .map(Profile::to_value)
            .collect();
        cfg.insert("profiles".into(), Value::Array(det));
    }
    let iv = cfg.get("interval_sec").and_then(py_int).unwrap_or(180);
    cfg.insert("interval_sec".into(), Value::from(iv.max(MIN_INTERVAL)));
    cfg
}

/// Numeric id of the current thread (Python: `threading.get_ident()`), for the temp file name.
fn thread_num() -> String {
    format!("{:?}", std::thread::current().id())
        .chars()
        .filter(char::is_ascii_digit)
        .collect()
}

/// core.save_config: `json.dumps(cfg, indent=2, ensure_ascii=False)` into `config.<pid>.<thread>.tmp`, then
/// replace `config.json` (25 tries, 20 ms apart, on access denied / sharing violation); last resort a direct
/// write. Formatting is semantically (not byte-) identical to Python: LF line ends (Python wrote CRLF).
pub fn save_config(paths: &Paths, cfg: &Config) -> io::Result<()> {
    save_config_with(paths, cfg, 25, Duration::from_millis(20))
}

/// [`save_config`] with explicit retry parameters (tests).
pub fn save_config_with(
    paths: &Paths,
    cfg: &Config,
    attempts: u32,
    delay: Duration,
) -> io::Result<()> {
    let text = serde_json::to_string_pretty(cfg).map_err(io::Error::other)?;
    let tmp = paths.data.join(format!(
        "config.{}.{}.tmp",
        std::process::id(),
        thread_num()
    ));
    atomic_write(&paths.config, &tmp, &text, attempts, delay)
}
