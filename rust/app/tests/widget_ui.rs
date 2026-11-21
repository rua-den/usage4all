//! UI test of the real widget inside the real tray process. Sandboxed LOCALAPPDATA/USERPROFILE (fake home with one
//! profile without credentials: the daemon records an error entry, no token is read and no request is sent),
//! registry/hook writes disabled. The widget is placed at an explicit position (widget_cx/cy), so it does not
//! depend on the taskbar. Windows are found by process id: the user's own (Python) widget uses the same class.
//!
//! Drives the widget with the messages the mouse would send. It cannot judge pixels: image quality is checked
//! with `claude-usage preview` + eyes. The right-click popup menu is modal and is not exercised; its commands are
//! run through the same code path with a private message (WM_APP+20).
//!
//! ONE test function on purpose (GUI tests must not run concurrently).

use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, SendMessageW, WM_APP,
    WM_COMMAND, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
};

const ID_WIDGET: usize = 3;
const ID_QUIT: usize = 6;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_CMD: u32 = WM_APP + 20;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn find(class: &str, pid: Option<u32>) -> usize {
    let c = wide(class);
    let mut prev = std::ptr::null_mut();
    loop {
        // SAFETY: window enumeration by class with NUL-terminated name.
        let h = unsafe { FindWindowExW(std::ptr::null_mut(), prev, c.as_ptr(), std::ptr::null()) };
        if h.is_null() {
            return 0;
        }
        let mut p = 0u32;
        // SAFETY: valid window handle, valid out pointer.
        unsafe { GetWindowThreadProcessId(h, &mut p) };
        if pid.is_none_or(|x| x == p) {
            return h as usize;
        }
        prev = h;
    }
}

fn send(h: usize, msg: u32, wp: usize, lp: isize) {
    // SAFETY: cross-process SendMessage to our own windows.
    unsafe { SendMessageW(h as _, msg, wp, lp) };
}

fn mouse(x: i32, y: i32) -> isize {
    ((y as isize & 0xFFFF) << 16) | (x as isize & 0xFFFF)
}

fn visible(h: usize) -> bool {
    h != 0 && unsafe { IsWindowVisible(h as _) != 0 }
}

fn rect(h: usize) -> (i32, i32, i32, i32) {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: valid window, valid out pointer.
    unsafe { GetWindowRect(h as _, &mut r) };
    (r.left, r.top, r.right - r.left, r.bottom - r.top)
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

fn cfg(p: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(p).unwrap_or_default()).unwrap_or(Value::Null)
}

#[test]
fn widget_end_to_end() {
    assert_eq!(
        find("ClaudeUsageTray", None),
        0,
        "another Rust tray is running: quit it first"
    );
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    let home = tmp.path().join("home");
    let prof = home.join(".claude-test");
    fs::create_dir_all(&prof).unwrap();
    let data = local.join("claude-usage");
    fs::create_dir_all(&data).unwrap();
    let cfgp = data.join("config.json");
    let write_cfg = |v: Value| fs::write(&cfgp, v.to_string()).unwrap();
    write_cfg(json!({
        "autostart": false, "notify_enabled": false, "widget_enabled": true, "widget_locked": true,
        "widget_cx": 800.0, "widget_cy": 500.0, "widget_style": "rings", "widget_theme": "dark",
        "profiles": [{"name": "claude-test", "path": prof.to_string_lossy()}]
    }));

    let child = Command::new(env!("CARGO_BIN_EXE_claude-usage"))
        .arg("--hidden")
        .env("LOCALAPPDATA", &local)
        .env("USERPROFILE", &home)
        .env_remove("HOMEDRIVE")
        .env_remove("HOMEPATH")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("CU_NO_AUTOSTART", "1")
        .env("CU_NO_REPAIR", "1")
        .env("CU_WIDGET_IGNORE_FULLSCREEN", "1")
        .env("CU_WIDGET_NO_TRACK", "1")
        .spawn()
        .expect("spawn exe");
    let pid = child.id();
    let _guard = Guard(child);

    // 1. widget window appears at the configured position
    wait_for("widget window", 15, || {
        visible(find("ClaudeUsageWidget", Some(pid)))
    });
    let w = find("ClaudeUsageWidget", Some(pid));
    let tray = find("ClaudeUsageTray", Some(pid));
    assert_ne!(tray, 0);
    let (x, y, ww, wh) = rect(w);
    assert!(ww > 20 && wh > 20, "size {ww}x{wh}");
    assert!(
        (x + ww / 2 - 800).abs() <= 2 && (y + wh / 2 - 500).abs() <= 2,
        "centre ({}, {})",
        x + ww / 2,
        y + wh / 2
    );
    let locked_size = (ww, wh);

    // 2. menu "show widget" toggle (WM_COMMAND to the tray) hides and shows it
    send(tray, WM_COMMAND, ID_WIDGET, 0);
    wait_for("hidden", 5, || !visible(w));
    assert_eq!(cfg(&cfgp)["widget_enabled"], false);
    send(tray, WM_COMMAND, ID_WIDGET, 0);
    wait_for("shown again", 5, || visible(w));

    // 3. right-click menu command "next style": config changes and the size follows the new style
    send(w, WM_CMD, 8, 0);
    assert_eq!(cfg(&cfgp)["widget_style"], "bars");
    wait_for("bars size", 5, || {
        rect(w).2 != locked_size.0 || rect(w).3 != locked_size.1
    });
    send(w, WM_CMD, 8, 0); // cards
    send(w, WM_CMD, 8, 0); // segments
    send(w, WM_CMD, 8, 0); // arc
    send(w, WM_CMD, 8, 0); // rings again
    assert_eq!(cfg(&cfgp)["widget_style"], "rings");
    wait_for("rings size back", 5, || {
        rect(w).2 == locked_size.0 && rect(w).3 == locked_size.1
    });

    // 4. hover: card appears after a moment, disappears on leave
    assert!(!visible(find("ClaudeUsageTip", Some(pid))));
    send(w, WM_MOUSEMOVE, 0, mouse(5, 5));
    wait_for("tip visible", 5, || {
        visible(find("ClaudeUsageTip", Some(pid)))
    });
    send(w, WM_MOUSELEAVE, 0, 0);
    wait_for("tip hidden", 5, || {
        !visible(find("ClaudeUsageTip", Some(pid)))
    });

    // 5. unlock -> edit frame (window grows); drag the body -> position saved
    send(w, WM_CMD, 5, 0);
    assert_eq!(cfg(&cfgp)["widget_locked"], false);
    wait_for("edit frame", 5, || {
        rect(w).2 > locked_size.0 && rect(w).3 > locked_size.1
    });
    let (_, _, ew, eh) = rect(w);
    // SAFETY: plain query.
    let scale = unsafe { GetDpiForSystem() } as f64 / 96.0;
    let (m, mt) = ((22.0 * scale) as i32, (42.0 * scale) as i32);
    assert_eq!((ew - 2 * m, eh - m - mt), locked_size, "frame margins");
    let (bx, by) = (m + locked_size.0 / 2, mt + locked_size.1 / 2);
    send(w, WM_LBUTTONDOWN, 1, mouse(bx, by));
    send(w, WM_MOUSEMOVE, 1, mouse(bx + 40, by + 30));
    send(w, WM_LBUTTONUP, 0, mouse(bx + 40, by + 30));
    wait_for("drag saved", 5, || {
        cfg(&cfgp)["widget_cx"]
            .as_f64()
            .is_some_and(|v| (v - 840.0).abs() < 1.5)
    });
    let c = cfg(&cfgp);
    assert!(
        (c["widget_cy"].as_f64().unwrap() - 530.0).abs() < 1.5,
        "{c}"
    );
    // resize handle: a larger size is saved
    let (sx, sy) = (m + locked_size.0, mt + locked_size.1);
    send(w, WM_LBUTTONDOWN, 1, mouse(sx, sy));
    send(w, WM_MOUSEMOVE, 1, mouse(sx + 60, sy + 60));
    send(w, WM_LBUTTONUP, 0, mouse(sx + 60, sy + 60));
    wait_for("size saved", 5, || {
        cfg(&cfgp)["widget_size"].as_f64().is_some_and(|v| v > 36.5)
    });
    // rotate handle: an angle is saved (handle sits above the top edge, centred)
    let (rx, ry) = (rect(w).2 / 2, mt - (26.0 * scale) as i32);
    send(w, WM_LBUTTONDOWN, 1, mouse(rx, ry));
    send(w, WM_MOUSEMOVE, 1, mouse(rx + 80, ry + 40));
    send(w, WM_LBUTTONUP, 0, mouse(rx + 80, ry + 40));
    wait_for("angle saved", 5, || {
        cfg(&cfgp)["widget_angle"]
            .as_f64()
            .is_some_and(|v| v.abs() > 1.0)
    });
    // padlock: locks again
    send(w, WM_CMD, 6, 0); // reset position/size/angle first (also a command under test)
    let c = cfg(&cfgp);
    assert!(
        c["widget_cx"].is_null() && c["widget_angle"] == 0.0 && c["widget_size"] == 36,
        "{c}"
    );
    send(w, WM_CMD, 5, 0);
    assert_eq!(cfg(&cfgp)["widget_locked"], true);

    // 6. quit through the tray: process ends, windows are gone
    send(tray, WM_COMMAND, ID_QUIT, 0);
    let mut child = _guard;
    wait_for("exit", 10, || child.0.try_wait().unwrap().is_some());
    assert_eq!(find("ClaudeUsageWidget", Some(pid)), 0);
}
