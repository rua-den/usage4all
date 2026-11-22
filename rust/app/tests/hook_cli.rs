//! Runs the real `claude-usage.exe hook` as a subprocess (windows subsystem, stdin/stdout/stderr piped like
//! Claude Code does) with LOCALAPPDATA / CLAUDE_CONFIG_DIR / USERPROFILE pointing at temp dirs.

use serde_json::json;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

fn write_cache(local: &Path, v: &serde_json::Value) {
    let d = local.join("claude-usage");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("usage_cache.json"),
        serde_json::to_string(v).unwrap(),
    )
    .unwrap();
}

fn run(args: &[&str], local: &Path, cfg: Option<&Path>, home: &Path) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_claude-usage"));
    c.args(args)
        .env("LOCALAPPDATA", local)
        .env("USERPROFILE", home)
        .env_remove("HOMEDRIVE")
        .env_remove("HOMEPATH")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match cfg {
        Some(p) => c.env("CLAUDE_CONFIG_DIR", p),
        None => c.env_remove("CLAUDE_CONFIG_DIR"),
    };
    let mut child = c.spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"{}").unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn blocks_with_utf8_vietnamese_stderr() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    let cfg = tmp.path().join("profiles").join("Công ty");
    std::fs::create_dir_all(&cfg).unwrap();
    write_cache(
        &local,
        &json!({"ts": now(), "block_enabled": true, "block_pct": 95,
            "profiles": {"Tài khoản": {"path": cfg.to_string_lossy(), "five": 96.5, "week": 10},
                          "khác": {"path": "C:\\other", "five": 20, "week": 30}}}),
    );
    let o = run(&["hook"], &local, Some(&cfg), tmp.path());
    assert_eq!(o.status.code(), Some(2));
    let err = String::from_utf8(o.stderr).expect("stderr must be valid UTF-8");
    assert_eq!(
        err,
        "Profile 'Tài khoản' đã 96% (5h) / 10% (7d) >= 95%. Đổi sang profile 'khác' hoặc tắt chặn trong claude-usage.\n"
    );
    assert!(o.stdout.is_empty());
}

#[test]
fn allows_below_limit_without_output() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    write_cache(
        &local,
        &json!({"ts": now(), "block_enabled": true, "block_pct": 95,
            "profiles": {"a": {"path": tmp.path().to_string_lossy(), "five": 10, "week": 10}}}),
    );
    let o = run(&["hook"], &local, Some(tmp.path()), tmp.path());
    assert_eq!(o.status.code(), Some(0));
    assert!(o.stderr.is_empty() && o.stdout.is_empty());
}

#[test]
fn fails_open_without_cache_stale_cache_and_garbage() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    std::fs::create_dir_all(&local).unwrap();
    assert_eq!(
        run(&["hook"], &local, Some(tmp.path()), tmp.path())
            .status
            .code(),
        Some(0)
    );
    let over = |ts: f64| {
        json!({"ts": ts, "block_enabled": true, "block_pct": 50,
            "profiles": {"a": {"path": tmp.path().to_string_lossy(), "five": 99, "week": 99}}})
    };
    write_cache(&local, &over(now() - 901.0));
    assert_eq!(
        run(&["hook"], &local, Some(tmp.path()), tmp.path())
            .status
            .code(),
        Some(0)
    );
    write_cache(&local, &over(now()));
    assert_eq!(
        run(&["hook"], &local, Some(tmp.path()), tmp.path())
            .status
            .code(),
        Some(2)
    );
    std::fs::write(
        local.join("claude-usage").join("usage_cache.json"),
        b"\xff\xfe not json",
    )
    .unwrap();
    let o = run(&["hook"], &local, Some(tmp.path()), tmp.path());
    assert_eq!(o.status.code(), Some(0));
    assert!(o.stderr.is_empty());
}

#[test]
fn default_config_dir_is_home_dot_claude() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    // path written with forward slashes and other case: normcase(abspath()) semantics
    let p = home
        .join(".claude")
        .to_string_lossy()
        .replace('\\', "/")
        .to_uppercase();
    write_cache(
        &local,
        &json!({"ts": now(), "block_enabled": 1, "block_pct": 95.0,
            "profiles": {"default": {"path": p, "five": 100, "week": 0}}}),
    );
    let o = run(&["hook"], &local, None, &home);
    assert_eq!(o.status.code(), Some(2));
    let err = String::from_utf8(o.stderr).unwrap();
    assert!(
        err.contains("'default' đã 100% (5h) / 0% (7d) >= 95.0%."),
        "{err}"
    );
    assert!(err.contains("profile '?'"));
}

#[test]
fn hook_does_not_create_any_files() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local"); // does not exist
    let o = run(&["hook"], &local, Some(tmp.path()), tmp.path());
    assert_eq!(o.status.code(), Some(0));
    assert!(
        !local.exists(),
        "the hook must not create %LOCALAPPDATA%\\claude-usage"
    );
}

#[test]
fn dashboard_mode_starts_the_dashboard_exe_unless_disabled() {
    // The tray modes (no args / --hidden) are covered by tray_ui.rs; running them here would start a real tray.
    // CU_NO_DASHBOARD keeps this test from opening a real window.
    std::env::set_var("CU_NO_DASHBOARD", "1");
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    let o = run(&["dashboard"], &local, None, tmp.path());
    assert_eq!(o.status.code(), Some(0));
    let log = std::fs::read_to_string(local.join("claude-usage").join("events.log")).unwrap();
    assert!(log.contains("dashboard skipped"), "{log}");
}
