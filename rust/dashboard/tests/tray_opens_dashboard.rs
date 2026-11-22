//! Tray -> dashboard hand-off with the REAL exes (sandboxed LOCALAPPDATA/USERPROFILE, no registry/hook writes, no
//! token, no network): the tray menu item "Open dashboard" starts `claude-usage-dashboard.exe`, which shows its
//! window (visible), a second click does not stack another process, and closing the window ends the process.
//! The dashboard window really appears on screen for a moment. Needs `claude-usage.exe` built in the same target
//! profile directory (`cargo build -p claude-usage -p claude-usage-dashboard`); skipped with a message otherwise.
//!
//! ONE test function on purpose (GUI tests must not run concurrently).

use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IsWindowVisible, PostMessageW, SendMessageW, WM_CLOSE, WM_COMMAND,
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn find(class: &str) -> HWND {
    // SAFETY: NUL-terminated class name.
    unsafe { FindWindowW(wide(class).as_ptr(), std::ptr::null()) }
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

struct KillDashboards;
impl Drop for KillDashboards {
    fn drop(&mut self) {
        let _ = Command::new("taskkill")
            .args(["/F", "/IM", "claude-usage-dashboard.exe"])
            .output();
    }
}

fn dashboards() -> usize {
    let out = Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq claude-usage-dashboard.exe", "/NH"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .matches("claude-usage-dashboard.ex") // tasklist truncates the name to 25 chars
        .count()
}

#[test]
fn tray_menu_opens_single_dashboard() {
    let dash = PathBuf::from(env!("CARGO_BIN_EXE_claude-usage-dashboard"));
    let tray = dash.with_file_name("claude-usage.exe");
    if !tray.exists() {
        eprintln!("SKIPPED: {} not built", tray.display());
        return;
    }
    assert!(
        find("ClaudeUsageTray").is_null() && find("ClaudeUsageDashboard").is_null(),
        "a tray or dashboard is already open: quit it before running this test"
    );
    assert_eq!(dashboards(), 0, "a dashboard process is already running");

    let tmp = tempfile::tempdir().unwrap();
    let (local, home) = (tmp.path().join("local"), tmp.path().join("home"));
    let data = local.join("claude-usage");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::write(data.join("config.json"), r#"{"autostart": false}"#).unwrap();
    let mut cmd = Command::new(&tray);
    cmd.arg("--hidden")
        .env("LOCALAPPDATA", &local)
        .env("USERPROFILE", &home)
        .env_remove("HOMEDRIVE")
        .env_remove("HOMEPATH")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CU_NO_DASHBOARD")
        .env("CU_NO_AUTOSTART", "1")
        .env("CU_NO_REPAIR", "1");
    let mut tray_proc = Guard(cmd.spawn().unwrap());
    let _cleanup = KillDashboards; // we checked above that none was running before: whatever exists later is ours
    wait_for("tray window", 15, || !find("ClaudeUsageTray").is_null());
    let th = find("ClaudeUsageTray");

    // menu item "Open dashboard" (ID 1)
    // SAFETY: cross-process SendMessage to our own tray window.
    unsafe { SendMessageW(th, WM_COMMAND, 1, 0) };
    wait_for("dashboard window", 20, || {
        !find("ClaudeUsageDashboard").is_null()
    });
    let dh = find("ClaudeUsageDashboard");
    // SAFETY: plain query on a window handle.
    wait_for(
        "dashboard visible",
        15,
        || unsafe { IsWindowVisible(dh) } != 0,
    );
    assert_eq!(dashboards(), 1);

    // more clicks (menu, and the second-instance show.flag path) never stack windows/processes
    unsafe { SendMessageW(th, WM_COMMAND, 1, 0) };
    fs::write(data.join("show.flag"), "1").unwrap();
    wait_for("show.flag consumed", 10, || {
        !data.join("show.flag").exists()
    });
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(dashboards(), 1, "second open must focus, not stack");

    // close request: the dashboard process ends; the tray keeps running
    unsafe { PostMessageW(dh, WM_CLOSE, 0, 0) };
    wait_for("dashboard gone", 10, || dashboards() == 0);
    assert!(
        tray_proc.0.try_wait().unwrap().is_none(),
        "tray still running"
    );
    let log = fs::read_to_string(data.join("events.log")).unwrap_or_default();
    assert!(log.contains("open dashboard requested"), "{log}");
    assert!(
        !log.contains("dashboard exe missing")
            && !log.contains("spawn failed")
            && !log.contains("js error"),
        "{log}"
    );

    // quit the tray from its menu (ID 6)
    unsafe { SendMessageW(th, WM_COMMAND, 6, 0) };
    wait_for("tray exit", 10, || {
        tray_proc.0.try_wait().unwrap().is_some()
    });
}
