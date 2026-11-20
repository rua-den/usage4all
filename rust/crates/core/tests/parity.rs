//! Parity with the Python reference: every fixture in tests/fixtures/*.json was produced by running the
//! Python function itself (tools/gen_fixtures.py). Cases with `tz_dependent` assume the zone of the
//! generating machine (see fixtures/_meta.json).

use cu_core::alias::{alias, aliases};
use cu_core::cache::write_cache;
use cu_core::config::{detect_profiles, load_config, Config, Profile};
use cu_core::fmt::{ago_text, effective_interval, fmt_reset_at, fmt_when, money};
use cu_core::iso::iso_ts;
use cu_core::parse::{parse_extra, parse_scoped};
use cu_core::paths::Paths;
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

fn load(name: &str) -> Vec<Value> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(format!("{name}.json"));
    let text = fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    match serde_json::from_str(&text).expect("fixture JSON") {
        Value::Array(a) => a,
        _ => panic!("fixture {name} is not a list"),
    }
}

fn close(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= 1e-9_f64.max(1e-12 * a.abs().max(b.abs()))
}

/// Semantic JSON equality: numbers within tolerance (5 == 5.0), objects compared in key ORDER too.
fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_i64(), y.as_i64()) {
            (Some(i), Some(j)) => i == j,
            _ => close(
                x.as_f64().unwrap_or(f64::NAN),
                y.as_f64().unwrap_or(f64::NAN),
            ),
        },
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| json_eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y.iter())
                    .all(|((k1, v1), (k2, v2))| k1 == k2 && json_eq(v1, v2))
        }
        _ => a == b,
    }
}

fn raises(expected: &Value) -> bool {
    expected
        .as_object()
        .is_some_and(|o| o.len() == 1 && o.contains_key("raises"))
}

fn map_of(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

/// Collects mismatches and fails once at the end with a readable list.
struct Report {
    name: &'static str,
    total: usize,
    fails: Vec<String>,
}

impl Report {
    fn new(name: &'static str) -> Self {
        Report {
            name,
            total: 0,
            fails: Vec::new(),
        }
    }
    fn check(&mut self, ok: bool, input: &Value, expected: &Value, got: impl std::fmt::Debug) {
        self.total += 1;
        if !ok {
            self.fails.push(format!(
                "input={input}\n  expected={expected}\n  got={got:?}"
            ));
        }
    }
    fn finish(self) {
        let n = self.fails.len();
        assert!(
            n == 0,
            "{}: {n}/{} cases differ from Python:\n{}",
            self.name,
            self.total,
            self.fails
                .iter()
                .take(15)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
        println!("{}: {} cases OK", self.name, self.total);
    }
}

fn maxlen_of(v: &Value) -> usize {
    v.as_u64().unwrap_or(0) as usize
}

#[test]
fn alias_parity() {
    let mut r = Report::new("alias");
    for c in load("alias") {
        let i = &c["input"];
        let got = alias(
            i["name"].as_str().unwrap(),
            &map_of(&i["cfg"]),
            maxlen_of(&i["maxlen"]),
        );
        // `raises` = Python raised (non-string custom alias): Rust only has to produce something sane.
        let ok = raises(&c["expected"]) || c["expected"].as_str() == Some(got.as_str());
        r.check(ok, i, &c["expected"], got);
    }
    r.finish();
}

#[test]
fn aliases_parity() {
    let mut r = Report::new("aliases");
    for c in load("aliases") {
        let i = &c["input"];
        let names: Vec<String> = i["names"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect();
        let got = aliases(&names, &map_of(&i["cfg"]), maxlen_of(&i["maxlen"]));
        let got_v = Value::Array(
            got.iter()
                .map(|(n, s)| Value::Array(vec![Value::from(n.as_str()), Value::from(s.as_str())]))
                .collect(),
        );
        let ok = raises(&c["expected"]) || json_eq(&got_v, &c["expected"]);
        r.check(ok, i, &c["expected"], got);
    }
    r.finish();
}

#[test]
fn iso_ts_parity() {
    let mut r = Report::new("iso_ts");
    for c in load("iso_ts") {
        let i = &c["input"];
        let got = iso_ts(i["iso"].as_str().unwrap());
        let ok = match (&c["expected"], got) {
            (Value::Null, None) => true,
            (Value::Number(n), Some(g)) => close(n.as_f64().unwrap(), g),
            _ => false,
        };
        r.check(ok, i, &c["expected"], got);
    }
    r.finish();
}

#[test]
fn fmt_reset_parity() {
    let mut r = Report::new("fmt_reset");
    for c in load("fmt_reset") {
        let i = &c["input"];
        let got = fmt_reset_at(i["iso"].as_str(), i["now"].as_f64().unwrap());
        r.check(
            c["expected"].as_str() == Some(got.as_str()),
            i,
            &c["expected"],
            got,
        );
    }
    r.finish();
}

#[test]
fn fmt_when_parity() {
    let mut r = Report::new("fmt_when");
    for c in load("fmt_when") {
        let i = &c["input"];
        let got = fmt_when(i["iso"].as_str().unwrap(), i["now"].as_f64().unwrap());
        r.check(
            c["expected"].as_str() == Some(got.as_str()),
            i,
            &c["expected"],
            got,
        );
    }
    r.finish();
}

#[test]
fn ago_text_parity() {
    let mut r = Report::new("ago_text");
    for c in load("ago_text") {
        let i = &c["input"];
        let got = ago_text(i["ts"].as_f64(), i["now"].as_f64().unwrap());
        r.check(
            c["expected"].as_str() == Some(got.as_str()),
            i,
            &c["expected"],
            got,
        );
    }
    r.finish();
}

#[test]
fn money_parity() {
    let mut r = Report::new("money");
    for c in load("money") {
        let i = &c["input"];
        let got = money(i["v"].as_f64().unwrap(), i["cur"].as_str().unwrap());
        r.check(
            c["expected"].as_str() == Some(got.as_str()),
            i,
            &c["expected"],
            got,
        );
    }
    r.finish();
}

#[test]
fn effective_interval_parity() {
    let mut r = Report::new("effective_interval");
    for c in load("effective_interval") {
        let i = &c["input"];
        let got = effective_interval(&map_of(&i["cfg"]));
        let ok = raises(&c["expected"]) || c["expected"].as_i64() == Some(got);
        r.check(ok, i, &c["expected"], got);
    }
    r.finish();
}

#[test]
fn parse_extra_parity() {
    let mut r = Report::new("parse_extra");
    for c in load("parse_extra") {
        let i = &c["input"];
        let got = parse_extra(&i["ex"]).unwrap_or(Value::Null);
        r.check(json_eq(&got, &c["expected"]), i, &c["expected"], got);
    }
    r.finish();
}

#[test]
fn parse_scoped_parity() {
    let mut r = Report::new("parse_scoped");
    for c in load("parse_scoped") {
        let i = &c["input"];
        let got = Value::Array(parse_scoped(&i["limits"]));
        // Python raising (non-iterable limits) -> Rust returns an empty list
        let ok = if raises(&c["expected"]) {
            got == Value::Array(vec![])
        } else {
            json_eq(&got, &c["expected"])
        };
        r.check(ok, i, &c["expected"], got);
    }
    r.finish();
}

// ---- home layouts ----

fn make_home(root: &Path, layout: &Value) -> PathBuf {
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    if layout["default"].as_bool() == Some(true) {
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(home.join(".claude").join(".credentials.json"), "{}").unwrap();
    }
    if layout["profiles_dir"].as_bool() == Some(true) {
        let pr = home.join("claude-profiles");
        fs::create_dir_all(&pr).unwrap();
        for e in layout["entries"].as_array().into_iter().flatten() {
            let (name, kind) = (e[0].as_str().unwrap(), e[1].as_str().unwrap());
            if kind == "file" {
                fs::write(pr.join(name), "x").unwrap();
            } else {
                fs::create_dir_all(pr.join(name)).unwrap();
                if kind == "dir_creds" {
                    fs::write(pr.join(name).join(".credentials.json"), "{}").unwrap();
                }
            }
        }
    }
    home
}

fn with_home(v: &Value, home: &Path) -> Value {
    let h = home.to_string_lossy();
    match v {
        Value::String(s) => Value::String(s.replace("{HOME}", &h)),
        Value::Array(a) => Value::Array(a.iter().map(|x| with_home(x, home)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| (k.replace("{HOME}", &h), with_home(x, home)))
                .collect(),
        ),
        _ => v.clone(),
    }
}

#[test]
fn detect_profiles_parity() {
    let mut r = Report::new("detect_profiles");
    for c in load("detect_profiles") {
        let i = &c["input"];
        let tmp = tempfile::tempdir().unwrap();
        let home = make_home(tmp.path(), &i["layout"]);
        let got = Value::Array(
            detect_profiles(&home)
                .iter()
                .map(Profile::to_value)
                .collect(),
        );
        let exp = with_home(&c["expected"], &home);
        r.check(json_eq(&got, &exp), i, &exp, got);
    }
    r.finish();
}

#[test]
fn load_config_parity() {
    let mut r = Report::new("load_config");
    for c in load("load_config") {
        let i = &c["input"];
        let tmp = tempfile::tempdir().unwrap();
        let home = make_home(tmp.path(), &i["layout"]);
        let paths = Paths::with_root(tmp.path().join("data"));
        if let Some(t) = i["config_text"].as_str() {
            fs::write(&paths.config, t.as_bytes()).unwrap();
        }
        let got: Config = load_config(&paths, &home);
        let got_v = Value::Object(got.clone());
        if raises(&c["expected"]) {
            // Python would crash on int(interval_sec); Rust documents the 180 fallback.
            let ok = got.get("interval_sec").and_then(Value::as_i64) == Some(180)
                && got.contains_key("profiles");
            r.check(ok, i, &c["expected"], got_v);
        } else {
            let exp = with_home(&c["expected"], &home);
            r.check(json_eq(&got_v, &exp), i, &exp, got_v);
        }
    }
    r.finish();
}

#[test]
fn write_cache_parity() {
    let mut r = Report::new("write_cache");
    for c in load("write_cache") {
        let i = &c["input"];
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(tmp.path());
        let busy: Vec<String> = i["busy"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect();
        let refresh = match &i["refresh"] {
            Value::Null => None,
            v => Some(v.clone()),
        };
        write_cache(
            &paths,
            &map_of(&i["cfg"]),
            &map_of(&i["data"]),
            &busy,
            refresh,
        )
        .unwrap();
        let mut got: Value =
            serde_json::from_str(&fs::read_to_string(&paths.cache).unwrap()).unwrap();
        let ts = got.as_object_mut().unwrap().shift_remove("ts");
        assert!(ts.is_some_and(|t| t.is_f64()), "ts must be a float");
        r.check(json_eq(&got, &c["expected"]), i, &c["expected"], got);
    }
    r.finish();
}
