//! Port of `tests/e2e_responsive.py` (many profiles, resizing from the edge handles, maximize/restore, remembered
//! size, auto-fit) to the real Rust dashboard process. Two launches against one sandbox (phase 1 interacts and
//! closes, phase 2 checks the size was remembered) + three launches for `initial_width` with 1 / 5 / 9 profiles.
//! Sandboxed LOCALAPPDATA/USERPROFILE, fake profiles without credentials, no network. Window stays hidden.
//!
//! ONE test function on purpose (GUI tests must not run concurrently).
#![cfg(any(debug_assertions, feature = "e2e"))]

mod common;
use common::*;
use serde_json::{json, Value};

const JS: &str = include_str!("js/responsive.js");
const N: usize = 14;

fn entry(i: usize, _name: &str, now: f64) -> Value {
    let mut e = json!({
        "five": (i * 7) % 100, "week": (i * 11) % 100, "plan": "team",
        "five_reset": "2999-01-01T00:00:00+00:00", "week_reset": "2999-02-01T00:00:00+00:00",
        "fetched_at": now, "retry_at": null, "err": null,
    });
    if i == 3 {
        e["err"] = json!("HTTP 429");
        e["retry_at"] = json!(now + 120.0);
    }
    e
}

#[test]
fn dashboard_responsive_suite() {
    assert!(
        window().is_null(),
        "a dashboard window is already open: close it before running this test"
    );
    let names: Vec<String> = (1..=N).map(|i| format!("claude-profile-{i:02}")).collect();
    let sb = Sandbox::new(&names);
    sb.write_cache(entry, &[], None);
    let mut checks = vec![];

    // phase 1: interact
    let mut run = Run::start(&sb.local, &sb.home, sb.root.path(), "resp1", JS, &format!("const PHASE='1'; const N={N};"));
    run.stage("phase1_done");
    run.go("phase1_done");
    let exited = run.exited(15);
    run.rust_check("phase 1: close (✕) ends the process", exited, "");
    checks.append(&mut run.checks);
    drop(run);
    wait_for("window gone", 5, || window().is_null());

    // phase 2: same sandbox, the size must be remembered
    let saved = sb.cfg()["dash_rect"].clone();
    let mut run = Run::start(&sb.local, &sb.home, sb.root.path(), "resp2", JS, &format!("const PHASE='2'; const N={N};"));
    run.stage("phase2_done");
    run.go("phase2_done");
    let exited = run.exited(15);
    run.rust_check("phase 2: close (✕) ends the process", exited, "");
    run.rust_check(
        "phase 2: config had dash_rect before the 2nd launch",
        saved.as_array().is_some_and(|a| a.len() == 4),
        saved.to_string(),
    );
    let cfg = sb.cfg();
    run.rust_check("phase 2: auto-fit cleared dash_rect in config", cfg["dash_rect"].is_null(), cfg["dash_rect"].to_string());
    checks.append(&mut run.checks);
    drop(run);
    wait_for("window gone", 5, || window().is_null());

    // initial_width for 1 / 5 / 9 profiles (the page reports its own width; the window is not user-sized yet)
    for (n, want) in [(1usize, 760.0), (5, 1120.0), (9, 1480.0)] {
        let names: Vec<String> = (1..=n).map(|i| format!("p{i}")).collect();
        let sb = Sandbox::new(&names);
        let js = "await sleep(1500); return { iw: innerWidth, sw: screen.width };";
        let mut run = Run::start(&sb.local, &sb.home, sb.root.path(), &format!("w{n}"), js, "");
        let v = run.finish();
        let exp = f64::min(want, v["sw"].as_f64().unwrap() - 80.0);
        let iw = v["iw"].as_f64().unwrap();
        checks.push((
            format!("initial width with {n} profile(s) = {exp} (cap: screen - 80)"),
            (iw - exp).abs() <= 2.0,
            format!("innerWidth={iw}"),
        ));
        drop(run);
        wait_for("window gone", 5, || window().is_null());
    }
    report("e2e_responsive.py -> Rust", &checks);
}
