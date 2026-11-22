//! UI test of the real dashboard process (WebView2). Sandboxed LOCALAPPDATA/USERPROFILE, registry/hook writes
//! disabled, no real token, no network. The window stays hidden in this mode: a JS script (`--e2e`, only in debug
//! builds or with `--features e2e`) drives the real page and writes its
//! result to a file. Pixels are not inspected (needs human eyes); behaviour is.
//!
//! ONE test function on purpose (GUI tests must not run concurrently).
#![cfg(any(debug_assertions, feature = "e2e"))]

use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_CLOSE};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn window() -> HWND {
    // SAFETY: NUL-terminated class name.
    unsafe { FindWindowW(wide("ClaudeUsageDashboard").as_ptr(), std::ptr::null()) }
}

struct Guard(Child);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("timeout waiting for: {what}");
}

fn spawn(local: &Path, home: &Path, script: Option<(&Path, &Path)>) -> Guard {
    let mut c = Command::new(env!("CARGO_BIN_EXE_claude-usage-dashboard"));
    if let Some((s, o)) = script {
        c.arg("--e2e").arg(s).arg(o);
    }
    c.env("LOCALAPPDATA", local)
        .env("USERPROFILE", home)
        .env_remove("HOMEDRIVE")
        .env_remove("HOMEPATH")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("CU_NO_AUTOSTART", "1")
        .env("CU_NO_REPAIR", "1");
    Guard(c.spawn().expect("spawn dashboard"))
}

fn run_script(local: &Path, home: &Path, dir: &Path, name: &str, body: &str) -> (Guard, Value) {
    let s = dir.join(format!("{name}.js"));
    let o = dir.join(format!("{name}.out"));
    fs::write(&s, body).unwrap();
    let g = spawn(local, home, Some((&s, &o)));
    wait_for("script result", 40, || o.exists());
    std::thread::sleep(Duration::from_millis(100)); // the file is written in one call; be safe
    let v = serde_json::from_str(&fs::read_to_string(&o).unwrap()).unwrap();
    (g, v)
}

const SCRIPT1: &str = r#"
await new Promise(r => setTimeout(r, 700));
const $ = id => document.getElementById(id);
const out = {};
out.init = !!window.__INIT__;
out.accs = $('accs').innerText;
out.sub = $('sub').textContent;
out.hookst = $('hookst').textContent;
out.state = await api.state();
out.win0 = await api.win_state();
out.fit_first = await api.fit(520);
out.rect_fit = await api.rect();
out.dpr = window.devicePixelRatio;
// settings go through the page's own save(): fields -> api.save -> config.json
$('warn').value = '77';
$('interval').value = '30';
await save(true);
await api.refresh();
await api.test_notify();
out.hook_install = await api.hook_install();
out.hook_status = await api.hook_status();
await refresh();                       // the page's refresh button path (also writes refresh.flag)
await api.widget_unlock();
// window geometry
await api.set_rect(50, 60, 900, 700);
out.rect_set = await api.rect();
out.win1 = await api.win_state();
out.fit_after_user = await api.fit(520);
await api.toggle_max();
out.win_max = await api.win_state();
out.wa = await api.work_area();
out.rect_max = await api.rect();
await api.toggle_max();
out.rect_back = await api.rect();
out.win_back = await api.win_state();
try { await api.no_such_method(); out.unknown = 'resolved'; } catch (e) { out.unknown = String(e); }
return out;
"#;

const SCRIPT2: &str = r#"
await new Promise(r => setTimeout(r, 500));
const out = { win: await api.win_state(), rect: await api.rect() };
setTimeout(() => api.hide(), 50);       // the page's own close path
return out;
"#;

#[test]
fn dashboard_process_end_to_end() {
    assert!(
        window().is_null(),
        "a dashboard window is already open: close it before running this test"
    );
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    let home = tmp.path().join("home");
    let prof = tmp.path().join("prof");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&prof).unwrap();
    fs::write(prof.join("settings.json"), "{}").unwrap();
    let data = local.join("claude-usage");
    fs::create_dir_all(&data).unwrap();
    fs::write(
        data.join("config.json"),
        json!({"profiles": [{"name": "p1", "path": prof.to_string_lossy()}], "autostart": false})
            .to_string(),
    )
    .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    fs::write(
        data.join("usage_cache.json"),
        json!({"ts": now, "block_enabled": true, "block_pct": 95, "warn_pct": 80, "busy": [], "refresh": null,
               "profiles": {"p1": {"path": prof.to_string_lossy(), "five": 42.0, "week": 7.0,
                   "five_reset": "2999-01-01T00:00:00+00:00", "week_reset": "2999-01-01T00:00:00+00:00",
                   "fetched_at": now, "retry_at": now + 180.0, "err": null}}})
        .to_string(),
    )
    .unwrap();

    // ---- run 1: render, save, flags, hooks, geometry, single instance, close by WM_CLOSE ----
    let (mut main, r) = run_script(&local, &home, tmp.path(), "s1", SCRIPT1);
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["init"], true, "state inlined into the page");
    assert!(r["accs"].as_str().unwrap().contains("p1"), "{r}");
    assert!(r["accs"].as_str().unwrap().contains("42%"), "{r}");
    assert!(r["sub"].as_str().unwrap().contains("1 accounts"), "{r}");
    assert_eq!(r["state"]["cfg"]["warn_pct"], 80);
    assert_eq!(r["state"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(r["hookst"], "hook: đã cài 0/1 profile");
    assert_eq!(r["win0"], json!({"user_sized": false, "maximized": false}));
    assert_eq!(r["fit_first"], true);
    assert_eq!(
        r["rect_fit"][3].as_i64().unwrap(),
        (520.0 * r["dpr"].as_f64().unwrap()) as i64,
        "window followed the content height: {r}"
    );

    let cfg: Value =
        serde_json::from_str(&fs::read_to_string(data.join("config.json")).unwrap()).unwrap();
    assert_eq!(cfg["warn_pct"], 77, "save() from the page");
    assert_eq!(cfg["interval_sec"], 90, "interval floor");
    assert_eq!(cfg["widget_locked"], false, "widget_unlock");
    assert!(data.join("refresh.flag").exists() && data.join("notify_test.flag").exists());
    assert_eq!(r["hook_install"], json!([["p1", "đã cài"]]));
    assert_eq!(r["hook_status"], json!([["p1", true]]));
    let st = fs::read_to_string(prof.join("settings.json")).unwrap();
    assert!(
        st.contains("claude-usage.exe") && st.contains("hook"),
        "{st}"
    );

    assert_eq!(r["rect_set"], json!([50, 60, 900, 700]));
    assert_eq!(r["win1"], json!({"user_sized": true, "maximized": false}));
    assert_eq!(
        r["fit_after_user"], false,
        "the user's size wins over auto fit"
    );
    assert_eq!(r["win_max"], json!({"user_sized": true, "maximized": true}));
    assert_eq!(r["rect_max"], r["wa"], "maximized = work area");
    assert_eq!(r["rect_back"], json!([50, 60, 900, 700]));
    assert_eq!(
        r["win_back"],
        json!({"user_sized": true, "maximized": false})
    );
    assert!(
        r["unknown"].as_str().unwrap().contains("unknown method"),
        "{r}"
    );

    // single instance: a second dashboard focuses the first and exits
    let h = window();
    assert!(!h.is_null(), "dashboard window class registered");
    let mut second = spawn(&local, &home, None);
    wait_for("second dashboard exits", 10, || {
        second.0.try_wait().unwrap().is_some()
    });
    assert!(main.0.try_wait().unwrap().is_none(), "first keeps running");

    // close request (Alt+F4 path): saves the size and ends the process
    // SAFETY: posting WM_CLOSE to our own test window.
    unsafe { PostMessageW(h, WM_CLOSE, 0, 0) };
    wait_for("process exit on close", 10, || {
        main.0.try_wait().unwrap().is_some()
    });
    let cfg: Value =
        serde_json::from_str(&fs::read_to_string(data.join("config.json")).unwrap()).unwrap();
    assert_eq!(
        cfg["dash_rect"],
        json!([50, 60, 900, 700]),
        "size remembered"
    );
    wait_for("window gone", 5, || window().is_null());

    // ---- run 2: the saved size is restored; the page's hide() ends the process ----
    let (mut main2, r2) = run_script(&local, &home, tmp.path(), "s2", SCRIPT2);
    assert_eq!(
        r2["win"],
        json!({"user_sized": true, "maximized": false}),
        "{r2}"
    );
    assert_eq!(r2["rect"], json!([50, 60, 900, 700]), "{r2}");
    wait_for("process exit on hide()", 10, || {
        main2.0.try_wait().unwrap().is_some()
    });
    let log = fs::read_to_string(data.join("events.log")).unwrap_or_default();
    assert!(
        !log.contains("js error") && !log.contains("dashboard:"),
        "{log}"
    );
    assert!(
        data.join("webview").exists(),
        "webview data dir under the sandboxed data dir"
    );
}
