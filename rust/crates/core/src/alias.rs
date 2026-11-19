//! Account labels: core.alias / core.aliases / core._auto_alias. All slicing is by character (Python slices
//! code points).

use crate::config::Config;
use serde_json::Value;
use std::collections::HashSet;

fn take(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `name.replace("claude-", "", 1)`: first occurrence anywhere.
fn strip_claude(name: &str) -> String {
    name.replacen("claude-", "", 1)
}

/// `re.sub(r"[^0-9A-Za-z]", "", s)`
fn alnum(s: &str) -> String {
    s.chars().filter(char::is_ascii_alphanumeric).collect()
}

/// core._auto_alias: `DT` for the default profile, `T01` for claude-team-01, `NG` for claude-ngoai.
pub fn auto_alias(name: &str) -> String {
    if name.starts_with("default") {
        return "DT".into();
    }
    let s = strip_claude(name);
    let toks: Vec<&str> = s
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    if toks.len() >= 2 {
        let last = toks[toks.len() - 1];
        if last.chars().all(|c| c.is_ascii_digit()) {
            return format!("{}{}", take(toks[0], 1), last).to_uppercase();
        }
    }
    let r = take(&s.replace('-', ""), 2).to_uppercase();
    if r.is_empty() {
        "?".into()
    } else {
        r
    }
}

/// The custom label of `name`: `cfg.aliases[name]` when it is a string whose `.strip()` is non-empty.
/// Deviation: Python raises when `aliases` is truthy but not a dict, or the entry is a truthy non-string;
/// both count as "no custom label" here.
fn custom_alias(cfg: &Config, name: &str) -> Option<String> {
    let Value::Object(m) = cfg.get("aliases")? else {
        return None;
    };
    let t = m.get(name)?.as_str()?.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// core.alias: custom text if set, else a short code or a short name (`widget_label`). `maxlen` 0 = no cut.
pub fn alias(name: &str, cfg: &Config, maxlen: usize) -> String {
    let s = if let Some(c) = custom_alias(cfg, name) {
        c
    } else if cfg.get("widget_label").and_then(Value::as_str) == Some("short") {
        if name.starts_with("default") {
            "desk".to_string()
        } else {
            strip_claude(name)
        }
    } else {
        auto_alias(name)
    };
    if maxlen > 0 {
        take(&s, maxlen)
    } else {
        s
    }
}

/// An insertion-ordered string map (Python dict) kept as a small vector.
fn set(v: &mut Vec<(String, String)>, k: &str, val: String) {
    match v.iter_mut().find(|(n, _)| n == k) {
        Some(e) => e.1 = val,
        None => v.push((k.to_string(), val)),
    }
}

fn get<'a>(v: &'a [(String, String)], k: &str) -> &'a str {
    v.iter().find(|(n, _)| n == k).map_or("", |e| e.1.as_str())
}

/// core.aliases: labels for a whole set of accounts. Like [`alias`], but automatic labels are made unique
/// (longer prefix of 3..=7 characters, then `XX1`, `XX2`...). The result keeps the order of `names`
/// (first occurrence of each name); duplicate names behave as in Python.
pub fn aliases(names: &[String], cfg: &Config, maxlen: usize) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for n in names {
        set(&mut out, n, alias(n, cfg, 0));
    }
    let custom: HashSet<&str> = names
        .iter()
        .filter(|n| custom_alias(cfg, n).is_some())
        .map(String::as_str)
        .collect();
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for n in names {
        if custom.contains(n.as_str()) {
            continue;
        }
        let key = get(&out, n).to_lowercase();
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some(g) => g.1.push(n.clone()),
            None => groups.push((key, vec![n.clone()])),
        }
    }
    for (_, g) in &groups {
        if g.len() < 2 {
            continue;
        }
        // dict keyed by name (duplicates collapse, last value wins), in first-occurrence order
        let mut base: Vec<(String, String)> = Vec::new();
        for n in g {
            let b = alnum(&strip_claude(n));
            set(&mut base, n, if b.is_empty() { n.clone() } else { b });
        }
        let mut done = false;
        for l in 3..8 {
            let cand: Vec<(String, String)> = base
                .iter()
                .map(|(n, b)| (n.clone(), take(b, l).to_uppercase()))
                .collect();
            let uniq: HashSet<&str> = cand.iter().map(|(_, c)| c.as_str()).collect();
            if uniq.len() == g.len() {
                for (n, c) in cand {
                    set(&mut out, &n, c);
                }
                done = true;
                break;
            }
        }
        if !done {
            for (i, n) in g.iter().enumerate() {
                let b = get(&base, n);
                set(
                    &mut out,
                    n,
                    format!("{}{}", take(b, 2).to_uppercase(), i + 1),
                );
            }
        }
    }
    out.into_iter()
        .map(|(n, s)| {
            let s = if maxlen > 0 { take(&s, maxlen) } else { s };
            (n, s)
        })
        .collect()
}
