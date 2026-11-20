//! Install / remove the Claude Code UserPromptSubmit hook in each profile's `settings.json`: port of
//! `hookinstall.py`. Only our own entry is touched (command contains `claude-usage` and ends with ` hook`);
//! a one-time backup `settings.json.claude-usage.bak` is kept; invalid JSON is never overwritten.
//!
//! Deviations from Python:
//! * `repair` follows the decided policy R1 (PORT_PLAN "Đã chốt" 1): never in debug builds or when the caller
//!   passes `no_repair`; in release it rewrites only when the exe inside the existing hook command no longer
//!   exists on disk, and reports what it did. Python rewrote on any mismatch.
//! * Shapes where Python raises (settings.json holding a list, `"hooks": null`, a non-string `command`, ...)
//!   are reported as unreadable/skipped and the file is left untouched; a failure in one profile does not
//!   stop the next one. Python's `_strip` mangles a group whose `hooks` is a dict/string; here that is a skip.
//! * JSON is written with LF line ends (Python's `write_text` on Windows wrote CRLF), `fs::copy` for the
//!   backup (Python `copy2` also copies timestamps), strict JSON only (no `NaN`), integers beyond u64 become
//!   floats. Key order and 2-space indentation are kept; semantic equality is the bar.

use crate::pyval::truthy;
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub const MARK: &str = "claude-usage";
const KEY: &str = "UserPromptSubmit";

pub const MSG_UNREADABLE: &str = "bỏ qua: settings.json không đọc được";
pub const MSG_INSTALLED: &str = "đã cài";
pub const MSG_REMOVED: &str = "đã gỡ";
pub const MSG_NONE: &str = "không có hook";

#[derive(Debug, Clone)]
pub struct HookProfile {
    pub name: String,
    pub path: PathBuf,
}

impl HookProfile {
    pub fn new(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        HookProfile {
            name: name.into(),
            path: path.into(),
        }
    }
    fn settings(&self) -> PathBuf {
        self.path.join("settings.json")
    }
}

/// The shell command Claude Code should run for `exe` (release form: `"<exe>" hook`).
pub fn command(exe: &Path) -> String {
    format!("\"{}\" hook", exe.display())
}

/// Python would have raised on this document shape.
#[derive(Debug)]
struct Shape;

fn is_ours(h: &Value) -> Result<bool, Shape> {
    let Value::Object(o) = h else {
        return Ok(false);
    };
    match o.get("command") {
        None => Ok(false),
        Some(Value::String(c)) => {
            Ok(c.to_lowercase().contains(MARK) && c.trim_end().ends_with(" hook"))
        }
        Some(_) => Err(Shape), // c.lower() on a non-string
    }
}

enum Loaded {
    /// Python `None`: unreadable / invalid JSON / JSON `null`.
    Unreadable,
    Doc(Value),
}

fn load(p: &Path) -> Loaded {
    if !p.exists() {
        return Loaded::Doc(Value::Object(Map::new()));
    }
    match fs::read(p)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    {
        Some(Value::Null) | None => Loaded::Unreadable,
        Some(v) => Loaded::Doc(v),
    }
}

fn save(p: &Path, data: &Value, backup: bool) -> std::io::Result<()> {
    if backup && p.exists() {
        let mut name = p.file_name().unwrap_or_default().to_os_string();
        name.push(".claude-usage.bak");
        let bak = p.with_file_name(name);
        if !bak.exists() {
            fs::copy(p, bak)?;
        }
    }
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(data).map_err(std::io::Error::other)?;
    fs::write(p, text)
}

/// `(data.get("hooks") or {}).get("UserPromptSubmit") or []` as a borrowed slice (None = empty/falsy).
fn groups_of(data: &Map<String, Value>) -> Result<Option<&Vec<Value>>, Shape> {
    let Some(h) = data.get("hooks").filter(|h| truthy(h)) else {
        return Ok(None);
    };
    let Value::Object(h) = h else {
        return Err(Shape);
    };
    match h.get(KEY).filter(|g| truthy(g)) {
        None => Ok(None),
        Some(Value::Array(a)) => Ok(Some(a)),
        Some(_) => Err(Shape),
    }
}

/// `g.get("hooks", [])` for read-only scans: lists give their items; a dict/string iterates harmlessly in
/// Python (keys/chars are never "ours"); anything else raises.
fn hooks_of_group(g: &Value) -> Result<&[Value], Shape> {
    let Value::Object(g) = g else {
        return Err(Shape);
    };
    match g.get("hooks") {
        None => Ok(&[]),
        Some(Value::Array(a)) => Ok(a),
        Some(Value::Object(_) | Value::String(_)) => Ok(&[]),
        Some(_) => Err(Shape),
    }
}

/// Our hook entries (clones of the hook objects), in file order.
fn ours(data: &Map<String, Value>) -> Result<Vec<&Value>, Shape> {
    let mut out = Vec::new();
    for g in groups_of(data)?.map_or(&[][..], |v| v) {
        for h in hooks_of_group(g)? {
            if is_ours(h)? {
                out.push(h);
            }
        }
    }
    Ok(out)
}

/// Remove our hook entries; returns how many were removed. Cleans up empty containers we may have created
/// (also removes groups whose `hooks` list ends up empty, like Python).
fn strip(data: &mut Map<String, Value>) -> Result<usize, Shape> {
    let mut n = 0;
    let mut new_groups: Vec<Value> = Vec::new();
    let was_list = groups_of(data)?.is_some();
    if let Some(groups) = groups_of(data)? {
        for g in groups {
            let Value::Object(go) = g else {
                return Err(Shape);
            };
            let hs: &[Value] = match go.get("hooks") {
                None => &[],
                Some(Value::Array(a)) => a,
                Some(_) => return Err(Shape),
            };
            let mut keep = Vec::new();
            for h in hs {
                if !is_ours(h)? {
                    keep.push(h.clone());
                }
            }
            n += hs.len() - keep.len();
            if !keep.is_empty() {
                let mut go = go.clone();
                go.insert("hooks".into(), Value::Array(keep));
                new_groups.push(Value::Object(go));
            }
        }
    }
    if was_list {
        if let Some(Value::Object(h)) = data.get_mut("hooks") {
            h.insert(KEY.into(), Value::Array(new_groups.clone()));
        }
    }
    if data.contains_key("hooks") {
        if new_groups.is_empty() {
            match data.get_mut("hooks") {
                Some(Value::Object(h)) => {
                    h.shift_remove(KEY);
                }
                _ => return Err(Shape),
            }
        }
        if matches!(data.get("hooks"), Some(Value::Object(h)) if h.is_empty()) {
            data.shift_remove("hooks");
        }
    }
    Ok(n)
}

/// `data.setdefault("hooks", {}).setdefault("UserPromptSubmit", []).append({...})`.
fn add_ours(data: &mut Map<String, Value>, cmd: &str) -> Result<(), Shape> {
    let hooks = data
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(h) = hooks else {
        return Err(Shape);
    };
    let list = h.entry(KEY).or_insert_with(|| Value::Array(Vec::new()));
    let Value::Array(list) = list else {
        return Err(Shape);
    };
    let mut hook = Map::new();
    hook.insert("type".into(), "command".into());
    hook.insert("command".into(), cmd.into());
    let mut group = Map::new();
    group.insert("hooks".into(), Value::Array(vec![Value::Object(hook)]));
    list.push(Value::Object(group));
    Ok(())
}

/// `[(name, installed)]`; `None` = settings.json unreadable (or of a shape Python would choke on).
pub fn status(profiles: &[HookProfile]) -> Vec<(String, Option<bool>)> {
    profiles
        .iter()
        .map(|p| {
            let v = match load(&p.settings()) {
                Loaded::Doc(Value::Object(d)) => scan_installed(&d).ok(),
                _ => None,
            };
            (p.name.clone(), v)
        })
        .collect()
}

/// Lazy `any(_is_ours(h) ...)`: an error after the first hit is never reached (as in Python).
fn scan_installed(data: &Map<String, Value>) -> Result<bool, Shape> {
    for g in groups_of(data)?.map_or(&[][..], |v| v) {
        for h in hooks_of_group(g)? {
            if is_ours(h)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub fn install(profiles: &[HookProfile], exe: &Path) -> Vec<(String, String)> {
    let cmd = command(exe);
    profiles
        .iter()
        .map(|p| {
            let f = p.settings();
            let msg = match load(&f) {
                Loaded::Doc(Value::Object(mut d)) => {
                    let done = strip(&mut d).and_then(|_| add_ours(&mut d, &cmd)).is_ok();
                    if done && save(&f, &Value::Object(d), true).is_ok() {
                        MSG_INSTALLED
                    } else {
                        MSG_UNREADABLE
                    }
                }
                _ => MSG_UNREADABLE,
            };
            (p.name.clone(), msg.to_string())
        })
        .collect()
}

pub fn uninstall(profiles: &[HookProfile]) -> Vec<(String, String)> {
    profiles
        .iter()
        .map(|p| {
            let f = p.settings();
            let msg = match load(&f) {
                Loaded::Doc(Value::Object(mut d)) => match strip(&mut d) {
                    Ok(0) => MSG_NONE,
                    Ok(_) if save(&f, &Value::Object(d), false).is_ok() => MSG_REMOVED,
                    _ => MSG_UNREADABLE,
                },
                _ => MSG_UNREADABLE,
            };
            (p.name.clone(), msg.to_string())
        })
        .collect()
}

/// The executable named by a hook command: the first quoted token (`"<exe>" ...`), or for an unquoted
/// command everything before the trailing ` hook`.
pub fn command_exe(cmd: &str) -> Option<&str> {
    let c = cmd.trim_end();
    if let Some(rest) = c.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some(&rest[..end]);
    }
    let exe = c.strip_suffix(" hook")?.trim();
    (!exe.is_empty()).then_some(exe)
}

/// Repair with the R1 policy. `no_repair` (dev/test/e2e callers) and debug builds make this a no-op.
/// Returns the names of the profiles that were rewritten; every rewrite is also passed to `log`.
pub fn repair(
    profiles: &[HookProfile],
    exe: &Path,
    no_repair: bool,
    log: &mut dyn FnMut(&str),
) -> Vec<String> {
    if no_repair || cfg!(debug_assertions) {
        return Vec::new();
    }
    repair_unchecked(profiles, exe, log)
}

/// The repair logic without the build/flag gate (tests call this directly).
pub fn repair_unchecked(
    profiles: &[HookProfile],
    exe: &Path,
    log: &mut dyn FnMut(&str),
) -> Vec<String> {
    let cmd = command(exe);
    let mut done = Vec::new();
    for p in profiles {
        let f = p.settings();
        let Loaded::Doc(Value::Object(mut d)) = load(&f) else {
            continue;
        };
        if d.is_empty() {
            continue;
        }
        let Ok(found) = ours(&d) else { continue };
        let stale_missing: Vec<&str> = found
            .iter()
            .filter_map(|h| h.get("command").and_then(Value::as_str))
            .filter(|c| *c != cmd)
            .filter(|c| command_exe(c).is_some_and(|e| !Path::new(e).exists()))
            .collect();
        if stale_missing.is_empty() {
            continue;
        }
        let old = stale_missing[0].to_string();
        let ok = strip(&mut d).and_then(|_| add_ours(&mut d, &cmd)).is_ok()
            && save(&f, &Value::Object(d), false).is_ok();
        if ok {
            log(&format!(
                "hook repaired: profile {} (old exe missing: {}) -> {}",
                p.name,
                command_exe(&old).unwrap_or("?"),
                exe.display()
            ));
            done.push(p.name.clone());
        }
    }
    done
}
