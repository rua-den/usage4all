//! Port of `tests/e2e_dashboard.py` (the Python dashboard E2E) to the real Rust dashboard process (WebView2):
//! the page script (`tests/js/legacy_dashboard.js`) drives ui.html through the `--e2e` hook while this test plays the
//! role of the Python driver thread (edits cache / flags, inspects config + settings.json). Sandboxed
//! LOCALAPPDATA/USERPROFILE, fake profiles WITHOUT credentials, no network, no registry/hook writes outside the sandbox.
//! The window stays hidden (e2e mode), the page still lays out and animates, so layout assertions are real.
//!
//! ONE test function on purpose (GUI tests must not run concurrently).
#![cfg(any(debug_assertions, feature = "e2e"))]

mod common;
use common::*;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, ShowWindow, SW_HIDE};

const JS: &str = include_str!("js/legacy_dashboard.js");
const NAMES: [&str; 4] = ["default (desktop)", "claude-da", "claude-hn", "claude-ngoai"];

fn entry(i: usize, name: &str, now: f64) -> Value {
    let mut e = json!({
        "five": 20 + 20 * i, "week": 30 + 10 * i,
        "five_reset": "2999-01-01T00:00:00+00:00", "week_reset": "2999-02-01T00:00:00+00:00",
        "fetched_at": now, "retry_at": null, "err": null,
    });
    if name == "claude-hn" {
        e["err"] = json!("HTTP 429");
        e["retry_at"] = json!(now + 150.0);
    }
    if name == "default (desktop)" {
        e["plan"] = json!("team");
        e["extra"] = json!({"used": 0.98, "limit": 25.0, "pct": 3.92, "currency": "USD"});
        e["scoped"] = json!([{"name": "Fable", "pct": 21, "resets_at": "2999-01-02T00:00:00+00:00"}]);
    }
    e
}

fn hook_cmd(settings: &Value, last: bool) -> Option<String> {
    let g = settings["hooks"]["UserPromptSubmit"].as_array()?;
    let grp = if last { g.last()? } else { g.first()? };
    grp["hooks"][0]["command"].as_str().map(str::to_string)
}

#[test]
fn dashboard_legacy_suite() {
    assert!(
        window().is_null(),
        "a dashboard window is already open: close it before running this test"
    );
    let names: Vec<String> = NAMES.iter().map(|s| s.to_string()).collect();
    let sb = Sandbox::new(&names);
    // claude-da already has settings + another hook: both must survive install / uninstall
    let da = sb.profiles[1].1.join("settings.json");
    fs::write(
        &da,
        json!({"model": "keep-me", "hooks": {"UserPromptSubmit": [{"hooks": [{"type": "command", "command": "other.exe"}]}]}})
            .to_string(),
    )
    .unwrap();
    sb.write_cache(entry, &["claude-da"], None);

    let mut run = Run::start(&sb.local, &sb.home, sb.root.path(), "legacy", JS, "");
    let flag = sb.data.join("refresh.flag");
    let tflag = sb.data.join("notify_test.flag");

    run.stage("ready");
    let t = run.elapsed();
    run.rust_check("page ready quickly", t.as_secs_f64() < 6.0, format!("{:.1}s", t.as_secs_f64()));
    run.go("ready");

    run.stage("idle1");
    sb.write_cache(entry, &[], None); // fetch finished
    run.go("idle1");

    run.stage("refresh_result");
    run.rust_check("refresh button wrote refresh.flag", flag.exists(), "");
    let _ = fs::remove_file(&flag);
    sb.write_cache(
        entry,
        &["claude-da"],
        Some(json!({"ts": now_secs(), "started": ["claude-da"], "skipped": [{"name": "x", "why": "vừa cập nhật xong"}]})),
    );
    run.go("refresh_result");

    run.stage("idle2");
    sb.write_cache(entry, &[], None);
    run.go("idle2");

    run.stage("pre_notify");
    let _ = fs::remove_file(&tflag);
    run.go("pre_notify");
    run.stage("notify_flag");
    run.rust_check("'send test notification' raises the flag for the app", tflag.exists(), "");
    run.go("notify_flag");

    run.stage("pre_r");
    let _ = fs::remove_file(&flag);
    run.go("pre_r");
    run.stage("r_in_input");
    run.rust_check("'r' in an input wrote no refresh.flag", !flag.exists(), "");
    run.go("r_in_input");
    run.stage("r_key");
    run.rust_check("'r' key wrote refresh.flag", flag.exists(), "");
    run.go("r_key");

    run.stage("hook_installed");
    let mut all_ok = true;
    for (_, d) in &sb.profiles {
        let s = read_json(&d.join("settings.json"));
        all_ok &= hook_cmd(&s, true).is_some_and(|c| c.ends_with(" hook") && c.contains("claude-usage"));
    }
    let d = read_json(&da);
    run.rust_check(
        "install hook: all profiles get it, existing settings + other hooks preserved",
        all_ok && d["model"] == "keep-me" && hook_cmd(&d, false).as_deref() == Some("other.exe"),
        d.to_string(),
    );
    run.go("hook_installed");

    run.stage("hook_uninstalled");
    let d = read_json(&da);
    run.rust_check(
        "uninstall hook removes only ours",
        d["hooks"]["UserPromptSubmit"] == json!([{"hooks": [{"type": "command", "command": "other.exe"}]}]),
        d.to_string(),
    );
    let ng = fs::read_to_string(sb.profiles[3].1.join("settings.json")).unwrap_or_default();
    run.rust_check("no trace of claude-usage left in the untouched-before profile", !ng.contains("claude-usage"), ng);
    run.go("hook_uninstalled");

    run.stage("minimized");
    let h = window();
    // SAFETY: queries/hides our own test window (a minimized window shows up in the taskbar: hide it again).
    let iconic = !h.is_null() && unsafe { IsIconic(h) } != 0;
    if !h.is_null() {
        unsafe { ShowWindow(h, SW_HIDE) };
    }
    run.rust_check("minimize button minimizes the window", iconic, "");
    run.go("minimized");

    run.stage("before_close");
    run.go("before_close");
    let exited = run.exited(15);
    run.rust_check("close (✕) ends the process", exited, "");
    run.rust_check(
        "close right after a change still saves it",
        sb.cfg()["widget_font"] == 135,
        sb.cfg()["widget_font"].to_string(),
    );
    wait_for("window gone", 5, || window().is_null());

    // Esc closes the same way (a pending change is flushed too)
    let s = sb.root.path().join("esc.js");
    let o = sb.root.path().join("esc.out");
    fs::write(
        &s,
        format!(
            "{PRELUDE}\nawait waitFor(()=>document.querySelectorAll('.acc').length===4,8000);await sleep(800);\
             setv('wfont','140');\
             document.dispatchEvent(new KeyboardEvent('keydown',{{key:'Escape',bubbles:true}}));await sleep(8000);return 1;"
        ),
    )
    .unwrap();
    let mut esc = spawn(&sb.local, &sb.home, Some((Path::new(&s), Path::new(&o))));
    let t = std::time::Instant::now();
    let mut esc_exit = false;
    while t.elapsed().as_secs() < 20 {
        if esc.0.try_wait().unwrap().is_some() {
            esc_exit = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    run.rust_check("close (Esc) ends the process", esc_exit, "");
    run.rust_check(
        "close (Esc) right after a change still saves it",
        sb.cfg()["widget_font"] == 140,
        sb.cfg()["widget_font"].to_string(),
    );
    let log = fs::read_to_string(sb.data.join("events.log")).unwrap_or_default();
    run.rust_check("no JS error / startup failure logged", !log.contains("js error") && !log.contains("dashboard:"), log);

    report("e2e_dashboard.py -> Rust", &run.checks);
}
