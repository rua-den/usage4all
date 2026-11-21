//! UI test of the real tray process (debug or release exe): sandboxed LOCALAPPDATA/USERPROFILE (an empty fake
//! home: no profiles, so no API call and no real token is ever read), registry/hook writes disabled
//! (`CU_NO_AUTOSTART`, `CU_NO_REPAIR`). Drives the hidden window with the same messages the shell sends
//! (WM_COMMAND for menu items, the tray callback message for clicks) and checks the observable results in the
//! sandboxed data dir. It cannot see the notification-area pixels or the balloon: those need human eyes.
//!
//! ONE test function on purpose (GUI tests must not run concurrently).

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, SendMessageW, WM_APP, WM_COMMAND};

const ID_REFRESH: usize = 2;
const ID_WIDGET: usize = 3;
const ID_NOTIFY: usize = 4;
const ID_TEST: usize = 5;
const ID_QUIT: usize = 6;
const WM_TRAY: u32 = WM_APP + 10;
const WM_LBUTTONUP: isize = 0x0202;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn window() -> usize {
    // SAFETY: FindWindowW with a NUL-terminated class name.
    unsafe { FindWindowW(wide("ClaudeUsageTray").as_ptr(), std::ptr::null()) as usize }
}

fn send(h: usize, msg: u32, wp: usize, lp: isize) {
    // SAFETY: cross-process SendMessage to our own tray window; blocks until handled.
    unsafe { SendMessageW(h as _, msg, wp, lp) };
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

struct Guard(Child);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn(local: &Path, home: &Path, args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_claude-usage"))
        .args(args)
        .env("LOCALAPPDATA", local)
        .env("USERPROFILE", home)
        .env_remove("HOMEDRIVE")
        .env_remove("HOMEPATH")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("CU_NO_AUTOSTART", "1")
        .env("CU_NO_REPAIR", "1")
        .env("CU_NO_DASHBOARD", "1")
        .spawn()
        .expect("spawn exe")
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(p).unwrap_or_default()).unwrap_or(Value::Null)
}

fn log_text(data: &Path) -> String {
    fs::read_to_string(data.join("events.log")).unwrap_or_default()
}

#[test]
fn tray_process_end_to_end() {
    assert_eq!(
        window(),
        0,
        "another tray instance is already running: quit it before running this test"
    );
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let data: PathBuf = local.join("claude-usage");
    fs::create_dir_all(&data).unwrap();
    // explicit empty profile list is replaced by auto-detect (fake empty home -> none): no network at all
    fs::write(
        data.join("config.json"),
        r#"{"autostart": false, "notify_enabled": true, "widget_enabled": true}"#,
    )
    .unwrap();

    // 1. starts, creates its window, pid file and first cache
    let mut main = Guard(spawn(&local, &home, &["--hidden"]));
    wait_for("tray window", 15, || window() != 0);
    let h = window();
    let pid = main.0.id();
    wait_for("daemon.pid", 5, || {
        fs::read_to_string(data.join("daemon.pid")).is_ok_and(|t| t.trim() == pid.to_string())
    });
    wait_for("usage_cache.json", 10, || {
        data.join("usage_cache.json").exists()
    });
    let cache = read_json(&data.join("usage_cache.json"));
    assert!(
        cache["profiles"].as_object().unwrap().is_empty() && cache["warn_pct"] == 80,
        "{cache}"
    );

    // 2. second instance: asks the first to open the dashboard and exits
    let mut second = spawn(&local, &home, &[]);
    wait_for("second instance exit", 10, || {
        second.try_wait().unwrap().is_some()
    });
    assert!(second.wait().unwrap().success());
    wait_for("show.flag handled", 10, || {
        log_text(&data).contains("open dashboard requested")
    });
    assert!(!data.join("show.flag").exists());
    assert_eq!(
        fs::read_to_string(data.join("daemon.pid")).unwrap().trim(),
        pid.to_string(),
        "singleton kept"
    );

    // 3. menu: refresh now -> flag written then consumed by the loop -> cache records the refresh request
    send(h, WM_COMMAND, ID_REFRESH, 0);
    wait_for("refresh handled", 10, || {
        !data.join("refresh.flag").exists()
            && read_json(&data.join("usage_cache.json"))["refresh"].is_object()
    });

    // 4. menu: toggles flip config.json (shared with the Python build) and the loop reloads it
    send(h, WM_COMMAND, ID_NOTIFY, 0);
    wait_for("notify off", 5, || {
        read_json(&data.join("config.json"))["notify_enabled"] == false
    });
    send(h, WM_COMMAND, ID_WIDGET, 0);
    wait_for("widget off", 5, || {
        read_json(&data.join("config.json"))["widget_enabled"] == false
    });
    send(h, WM_COMMAND, ID_NOTIFY, 0);
    wait_for("notify on again", 5, || {
        read_json(&data.join("config.json"))["notify_enabled"] == true
    });

    // 5. menu: test notification -> a toast is produced (balloon itself is not observable here)
    send(h, WM_COMMAND, ID_TEST, 0);
    wait_for("toast logged", 10, || {
        log_text(&data).contains("toast | Claude usage - thử thông báo | Thông báo đang BẬT")
    });
    assert!(!data.join("notify_test.flag").exists());
    assert!(!log_text(&data).contains("toast failed"));

    // 6. left click on the icon = default item "Open dashboard"
    let before = log_text(&data).matches("open dashboard requested").count();
    send(h, WM_TRAY, 1, WM_LBUTTONUP);
    wait_for("click opens dashboard", 5, || {
        log_text(&data).matches("open dashboard requested").count() == before + 1
    });

    // 7. quit from the menu: process exits, pid file and window are gone
    send(h, WM_COMMAND, ID_QUIT, 0);
    wait_for("process exit", 10, || main.0.try_wait().unwrap().is_some());
    assert!(main.0.wait().unwrap().success());
    assert!(
        !data.join("daemon.pid").exists(),
        "pid file removed on quit"
    );
    assert_eq!(window(), 0);
}
