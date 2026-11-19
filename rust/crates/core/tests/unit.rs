//! Unit tests that do not need Python fixtures: atomic writes, retry/fallback, log trimming, round trips.
//! Everything runs in tempfile directories; nothing touches %LOCALAPPDATA%\claude-usage.

use cu_core::cache::{load_cache, write_cache, write_cache_with};
use cu_core::config::{
    defaults, detect_profiles, get_bool, get_f64, get_i64, get_str, load_config, profiles,
    save_config, save_config_with, Config,
};
use cu_core::fmt::{effective_interval, fmt_reset, money};
use cu_core::log::{log, log_at};
use cu_core::paths::Paths;
use cu_core::pyval::{parse_py_int_str, py_floordiv, py_int, truthy};
use serde_json::{json, Map, Value};
use std::fs;
use std::time::Duration;

fn sandbox() -> (tempfile::TempDir, Paths) {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::with_root(tmp.path().join("claude-usage"));
    (tmp, paths)
}

fn leftovers(paths: &Paths, suffix: &str) -> Vec<String> {
    fs::read_dir(&paths.data)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(suffix))
        .collect()
}

fn obj(v: Value) -> Config {
    match v {
        Value::Object(m) => m,
        _ => panic!("not an object"),
    }
}

// ---- paths ----

#[test]
fn paths_layout() {
    let (_t, p) = sandbox();
    assert!(p.data.is_dir());
    for (path, name) in [
        (&p.config, "config.json"),
        (&p.cache, "usage_cache.json"),
        (&p.flag, "refresh.flag"),
        (&p.pidfile, "daemon.pid"),
        (&p.alerts, "alerts.json"),
        (&p.testflag, "notify_test.flag"),
        (&p.events, "events.log"),
        (&p.show, "show.flag"),
    ] {
        assert_eq!(path, &p.data.join(name));
    }
}

#[test]
fn paths_from_env_uses_localappdata() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::env::set_var("LOCALAPPDATA", tmp.path().join("lad"));
    let p = Paths::from_env_with_home(&home).unwrap();
    assert_eq!(p.data, tmp.path().join("lad").join("claude-usage"));
    assert!(p.data.is_dir());
    std::env::remove_var("LOCALAPPDATA");
    let p = Paths::from_env_with_home(&home).unwrap();
    assert_eq!(p.data, home.join("claude-usage"));
}

// ---- config ----

#[test]
fn defaults_exact_keys_and_order() {
    let d = defaults();
    let keys: Vec<&str> = d.keys().map(String::as_str).collect();
    assert_eq!(keys.len(), 39); // len(core.DEFAULTS)
    assert_eq!(
        &keys[..4],
        ["interval_sec", "warn_pct", "block_pct", "block_enabled"]
    );
    assert_eq!(keys[keys.len() - 1], "widget_angle");
    assert_eq!(d["widget_angle"], json!(0.0));
    assert!(d["dash_rect"].is_null() && d["widget_cx"].is_null() && d["widget_cy"].is_null());
}

#[test]
fn load_missing_file_gives_defaults_and_detection() {
    let (t, p) = sandbox();
    let home = t.path().join("home");
    let cfg = load_config(&p, &home);
    assert_eq!(cfg["interval_sec"], json!(180));
    assert_eq!(cfg["profiles"], json!([]));
}

#[test]
fn save_is_atomic_and_round_trips_unknown_keys_and_floats() {
    let (t, p) = sandbox();
    let home = t.path().join("home");
    let text = r#"{"zzz": [1, 2, {"q": null}], "interval_sec": 250, "widget_size": 36.5,
                   "widget_cx": 100.25, "dash_rect": [1.5, 2, 3, 4], "aliases": {"a": "é"}, "aaa": "x"}"#;
    fs::write(&p.config, text).unwrap();
    let cfg = load_config(&p, &home);
    assert_eq!(cfg["widget_size"], json!(36.5));
    save_config(&p, &cfg).unwrap();
    assert!(
        leftovers(&p, ".tmp").is_empty(),
        "temp file must not remain"
    );
    let again = load_config(&p, &home);
    assert_eq!(Value::Object(cfg.clone()), Value::Object(again.clone()));
    let keys: Vec<_> = again.keys().cloned().collect();
    let pos = |k: &str| keys.iter().position(|x| x == k).unwrap();
    assert!(
        pos("interval_sec") < pos("warn_pct"),
        "existing keys keep their default position"
    );
    assert!(
        pos("zzz") < pos("aaa"),
        "new keys are appended in file order"
    );
    assert_eq!(again["zzz"], json!([1, 2, {"q": null}]));
    assert_eq!(again["dash_rect"], json!([1.5, 2, 3, 4]));
    // not escaped: ensure_ascii=False
    assert!(fs::read_to_string(&p.config).unwrap().contains('é'));
}

#[test]
fn invalid_json_retries_then_defaults() {
    let (t, p) = sandbox();
    fs::write(&p.config, "{broken").unwrap();
    let start = std::time::Instant::now();
    let cfg = load_config(&p, &t.path().join("home"));
    assert!(
        start.elapsed() >= Duration::from_millis(100),
        "5 sleeps of 25 ms"
    );
    assert_eq!(
        Value::Object(cfg),
        Value::Object({
            let mut d = defaults();
            d.insert("profiles".into(), json!([]));
            d
        })
    );
}

#[test]
fn load_picks_up_a_file_that_becomes_valid_during_the_retries() {
    let (t, p) = sandbox();
    fs::write(&p.config, "{broken").unwrap();
    let path = p.config.clone();
    let h = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        fs::write(path, r#"{"warn_pct": 55}"#).unwrap();
    });
    let cfg = load_config(&p, &t.path().join("home"));
    h.join().unwrap();
    assert_eq!(cfg["warn_pct"], json!(55));
}

#[test]
fn interval_floor_and_python_int_semantics() {
    for (v, want) in [
        (json!(300.9), 300),
        (json!("250"), 250),
        (json!(true), 90),
        (json!(10), 90),
        (json!(null), 180),
        (json!([1]), 180),
        (json!("abc"), 180),
        (json!(-5.5), 90),
    ] {
        let (t, p) = sandbox();
        fs::write(&p.config, json!({"interval_sec": v}).to_string()).unwrap();
        let cfg = load_config(&p, &t.path().join("home"));
        assert_eq!(cfg["interval_sec"], json!(want), "interval_sec {v}");
    }
}

#[test]
fn py_helpers() {
    assert_eq!(py_int(&json!(-0.9)), Some(0));
    assert_eq!(py_int(&json!(-1.9)), Some(-1));
    assert_eq!(py_int(&json!(false)), Some(0));
    assert_eq!(py_int(&json!("  +7 ")), Some(7));
    assert_eq!(py_int(&json!("1_000")), Some(1000));
    assert_eq!(py_int(&json!("1__0")), None);
    assert_eq!(py_int(&json!("")), None);
    assert_eq!(py_int(&json!(null)), None);
    assert_eq!(parse_py_int_str("99999999999999999999"), Some(i64::MAX));
    assert!(
        !truthy(&json!(0.0)) && !truthy(&json!("")) && !truthy(&json!({})) && truthy(&json!([0]))
    );
    assert_eq!(py_floordiv(7.0, 2.0), 3.0);
    assert_eq!(py_floordiv(179.99999999999997, 60.0), 2.0);
}

#[test]
fn lenient_getters() {
    let c = obj(
        json!({"i": 36.9, "f": 5, "b": 0, "s": "x", "bad": [1], "p": [
        {"name": "a", "path": "C:\\a"}, {"name": 5}, "junk", {"name": "b", "path": "C:\\b"}]}),
    );
    assert_eq!(get_i64(&c, "i", 0), 36);
    assert_eq!(get_i64(&c, "bad", 7), 7);
    assert_eq!(get_i64(&c, "missing", 9), 9);
    assert_eq!(get_f64(&c, "f", 0.0), 5.0);
    assert!(!get_bool(&c, "b", true));
    assert!(get_bool(&c, "missing", true));
    assert_eq!(get_str(&c, "s", "d"), "x");
    assert_eq!(get_str(&c, "i", "d"), "d");
    let mut c2 = c.clone();
    c2.insert("profiles".into(), c["p"].clone());
    let ps = profiles(&c2);
    assert_eq!(ps.len(), 2);
    assert_eq!(ps[1].name, "b");
}

#[test]
fn detect_profiles_sorting_and_existence_only() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path();
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::write(home.join(".claude").join(".credentials.json"), "{}").unwrap();
    for n in ["b", "A", "c"] {
        fs::create_dir_all(home.join("claude-profiles").join(n)).unwrap();
        fs::write(
            home.join("claude-profiles")
                .join(n)
                .join(".credentials.json"),
            "not even json",
        )
        .unwrap();
    }
    fs::create_dir_all(home.join("claude-profiles").join("nocreds")).unwrap();
    let names: Vec<_> = detect_profiles(home).into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["default (desktop)", "A", "b", "c"]);
}

// ---- atomic writes: retry + last-resort fallback (Windows file sharing semantics) ----

#[cfg(windows)]
mod windows_sharing {
    use super::*;
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    #[test]
    fn save_config_retries_while_file_is_exclusively_open() {
        let (t, p) = sandbox();
        fs::write(&p.config, r#"{"warn_pct": 1}"#).unwrap();
        let holder = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&p.config)
            .unwrap();
        let h = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            drop(holder);
        });
        let mut cfg = load_config_ignoring_lock(&t);
        cfg.insert("warn_pct".into(), json!(77));
        save_config(&p, &cfg).unwrap();
        h.join().unwrap();
        let text = fs::read_to_string(&p.config).unwrap();
        assert!(text.contains("77"), "{text}");
        assert!(leftovers(&p, ".tmp").is_empty());
    }

    fn load_config_ignoring_lock(t: &tempfile::TempDir) -> Config {
        let mut d = defaults();
        d.insert("profiles".into(), json!([]));
        let _ = t;
        d
    }

    #[test]
    fn save_config_falls_back_to_direct_write_when_replace_never_succeeds() {
        let (_t, p) = sandbox();
        fs::write(&p.config, r#"{"warn_pct": 1}"#).unwrap();
        // readers/writers allowed, delete NOT allowed: rename-over fails forever, a direct write works
        let _holder = OpenOptions::new()
            .read(true)
            .share_mode(1 | 2)
            .open(&p.config)
            .unwrap();
        let mut cfg = defaults();
        cfg.insert("warn_pct".into(), json!(88));
        save_config_with(&p, &cfg, 3, Duration::from_millis(5)).unwrap();
        let text = fs::read_to_string(&p.config).unwrap();
        assert!(text.contains("88"), "{text}");
        assert!(
            leftovers(&p, ".tmp").is_empty(),
            "tmp is removed before the fallback"
        );
    }

    #[test]
    fn write_cache_retries_then_falls_back() {
        let (_t, p) = sandbox();
        let cfg =
            obj(json!({"block_enabled": true, "block_pct": 95, "warn_pct": 80, "profiles": []}));
        write_cache(&p, &cfg, &Map::new(), &[], None).unwrap();
        // 1) exclusive for a short time: retried until it works
        let holder = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&p.cache)
            .unwrap();
        let h = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            drop(holder);
        });
        write_cache(&p, &cfg, &Map::new(), &["b".into(), "a".into()], None).unwrap();
        h.join().unwrap();
        let v = load_cache(&p).unwrap();
        assert_eq!(v["busy"], json!(["a", "b"]));
        assert!(leftovers(&p, ".tmp").is_empty());
        // 2) never replaceable: direct write
        let _holder = OpenOptions::new()
            .read(true)
            .share_mode(1 | 2)
            .open(&p.cache)
            .unwrap();
        write_cache_with(
            &p,
            &cfg,
            &Map::new(),
            &["z".into()],
            Some(json!({"x": 1})),
            2,
            Duration::from_millis(5),
        )
        .unwrap();
        let v = load_cache(&p).unwrap();
        assert_eq!(v["busy"], json!(["z"]));
        assert_eq!(v["refresh"], json!({"x": 1}));
        assert!(leftovers(&p, ".tmp").is_empty());
    }
}

#[test]
fn write_cache_structure_and_load_cache() {
    let (_t, p) = sandbox();
    assert!(load_cache(&p).is_none());
    let cfg = obj(
        json!({"block_enabled": false, "block_pct": 90, "warn_pct": 70, "x": 1,
        "profiles": [{"name": "a", "path": "C:\\a"}, {"name": "b", "path": "C:\\b"}]}),
    );
    let data = obj(json!({"b": {"five": 1.5, "path": "OVR"}, "ghost": {"five": 9}}));
    write_cache(&p, &cfg, &data, &["q".into(), "Q".into()], None).unwrap();
    let v = load_cache(&p).unwrap();
    assert!(v["ts"].is_f64());
    assert_eq!(v["block_enabled"], json!(false));
    assert_eq!(v["busy"], json!(["Q", "q"]));
    assert_eq!(v["refresh"], Value::Null);
    let profs = v["profiles"].as_object().unwrap();
    assert_eq!(profs.keys().collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(profs["a"], json!({"path": "C:\\a"}));
    assert_eq!(profs["b"], json!({"path": "OVR", "five": 1.5}));
    fs::write(&p.cache, "null").unwrap();
    assert!(load_cache(&p).is_none());
}

// ---- log ----

#[test]
fn log_appends_formatted_lines() {
    let (_t, p) = sandbox();
    log(&p, "hello é");
    log(&p, "second");
    let text = fs::read_to_string(&p.events).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    let b = lines[0].as_bytes();
    assert!(
        b[4] == b'-'
            && b[7] == b'-'
            && b[10] == b' '
            && b[13] == b':'
            && b[16] == b':'
            && b[19] == b' '
    );
    assert!(lines[0].ends_with(" hello é") && lines[1].ends_with(" second"));
}

#[test]
fn log_trims_to_last_100k_characters_over_200k_bytes() {
    let (_t, p) = sandbox();
    // 150_000 two-byte characters = 300_000 bytes; the last chars are distinguishable
    let mut text: String = "é".repeat(149_990);
    text.push_str("0123456789");
    fs::write(&p.events, &text).unwrap();
    log_at(&p, "after", chrono::Local::now()).unwrap();
    let got = fs::read_to_string(&p.events).unwrap();
    // Python's behaviour: the last 100_000 characters of the old text, then the new line right after them
    let line_len = got.chars().count() - 100_000;
    assert!(
        line_len > 6
            && got.ends_with(
                " after
"
            ),
        "{line_len}"
    );
    let old_part: String = got.chars().take(100_000).collect();
    assert!(old_part.ends_with("0123456789"));
    assert!(old_part.starts_with('é'));
    // under the limit: untouched
    fs::write(&p.events, "x".repeat(150_000)).unwrap();
    log(&p, "y");
    let got = fs::read_to_string(&p.events).unwrap();
    assert!(got.starts_with(&"x".repeat(150_000)) && got.len() > 150_000);
}

#[test]
fn log_never_panics_or_errors() {
    let (_t, p) = sandbox();
    fs::create_dir_all(&p.events).unwrap(); // events.log is a directory: every write fails
    log(&p, "ignored");
    assert!(log_at(&p, "x", chrono::Local::now()).is_err());
    let bad = Paths::with_root(p.data.join("a").join("b").join("nope"));
    fs::remove_dir_all(&bad.data).unwrap();
    log(&bad, "ignored");
}

// ---- fmt with the real clock ----

#[test]
fn real_clock_wrappers() {
    let t = chrono::Utc::now() + chrono::Duration::minutes(150) + chrono::Duration::seconds(30);
    let iso = t.to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    assert_eq!(fmt_reset(Some(&iso)), "2h30m");
    assert_eq!(fmt_reset(Some("")), "-");
    assert_eq!(money(1234.5, "USD"), "$1,234.50");
    let mut c = Config::new();
    c.insert("profiles".into(), json!([1, 2, 3]));
    assert_eq!(effective_interval(&c), 180);
}
