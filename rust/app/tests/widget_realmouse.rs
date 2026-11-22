//! REAL pointer test of the widget: the cursor is moved with SetCursorPos/SendInput over the test's own widget
//! window (sandboxed tray process), WITHOUT `CU_WIDGET_NO_TRACK`, so Windows' own `TrackMouseEvent` →
//! `WM_MOUSELEAVE` path is exercised: hover shows the tip, leaving hides it, coming back shows it again.
//! The user's cursor is moved for a few seconds and restored afterwards. Sandboxed LOCALAPPDATA/USERPROFILE,
//! no token, no network, no registry/hook writes. Observation: the test-only `WM_APP+21` state dump.
//!
//! ONE test function on purpose (GUI tests must not run concurrently).
#![cfg(any(debug_assertions, feature = "e2e"))]

use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetCursorPos, GetWindowThreadProcessId, IsWindowVisible, SendMessageW,
    SetCursorPos, WM_APP,
};

const WM_DUMP: u32 = WM_APP + 21;

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

fn sleep(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

struct Guard(Child);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Puts the user's cursor back, also when an assertion fails.
struct CursorBack(POINT);
impl CursorBack {
    fn new() -> CursorBack {
        let mut p = POINT { x: 0, y: 0 };
        // SAFETY: valid out pointer.
        unsafe { GetCursorPos(&mut p) };
        CursorBack(p)
    }
}
impl Drop for CursorBack {
    fn drop(&mut self) {
        // SAFETY: plain call.
        unsafe { SetCursorPos(self.0.x, self.0.y) };
    }
}

/// Move the real pointer to (x, y) in physical pixels: teleport, then a 1px hardware-style jiggle so the system
/// really delivers WM_MOUSEMOVE to the window under the pointer.
fn move_to(x: i32, y: i32) {
    // SAFETY: plain calls; the INPUT structs are fully initialised.
    unsafe {
        SetCursorPos(x, y);
        for d in [1, -1] {
            let mi = MOUSEINPUT {
                dx: d,
                dy: 0,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE,
                time: 0,
                dwExtraInfo: 0,
            };
            let inp = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 { mi },
            };
            SendInput(1, &inp, std::mem::size_of::<INPUT>() as i32);
            sleep(30);
        }
    }
}

struct Rig {
    data: PathBuf,
    w: usize,
}

impl Rig {
    fn dump(&self) -> Value {
        // SAFETY: cross-process SendMessage to our own widget window.
        unsafe { SendMessageW(self.w as _, WM_DUMP, 0, 0) };
        serde_json::from_str(&fs::read_to_string(self.data.join("widget_state.json")).unwrap())
            .unwrap()
    }
    fn until(&self, what: &str, secs: u64, mut pred: impl FnMut(&Value) -> bool) -> Value {
        let t = Instant::now();
        loop {
            let d = self.dump();
            if pred(&d) {
                return d;
            }
            assert!(
                t.elapsed() < Duration::from_secs(secs),
                "timeout waiting for: {what}; last state: {d}"
            );
            sleep(80);
        }
    }
    /// Like `until`, but keeps the pointer on (x, y): a human hand on the mouse during the test must not make it flaky.
    fn until_at(
        &self,
        (x, y): (i32, i32),
        what: &str,
        secs: u64,
        mut pred: impl FnMut(&Value) -> bool,
    ) -> Value {
        let t = Instant::now();
        loop {
            let mut p = POINT { x: 0, y: 0 };
            // SAFETY: valid out pointer.
            unsafe { GetCursorPos(&mut p) };
            if (p.x - x).abs() > 1 || (p.y - y).abs() > 1 {
                move_to(x, y);
            }
            let d = self.dump();
            if pred(&d) {
                return d;
            }
            assert!(
                t.elapsed() < Duration::from_secs(secs),
                "timeout waiting for: {what}; last state: {d}"
            );
            sleep(80);
        }
    }
    /// Screen centre (physical px) of cell `i` of the (unrotated, vertical) stack.
    fn cell_center(d: &Value, i: usize) -> (i32, i32) {
        let fr = &d["frame"];
        let (cw, ch) = (f(&d["cell_w"]), f(&d["cell_h"]));
        let (fx, fy) = (f(&fr["m"]) + cw / 2.0, f(&fr["mt"]) + (i as f64 + 0.5) * ch);
        let (org, img) = (&d["org"], &d["img"]);
        // unrotated: frame (wp x hp) is centred in the image, the image sits at org
        let x = f(&org[0]) + f(&img[0]) / 2.0 + (fx - f(&fr["wp"]) / 2.0);
        let y = f(&org[1]) + f(&img[1]) / 2.0 + (fy - f(&fr["hp"]) / 2.0);
        (x.round() as i32, y.round() as i32)
    }
}

fn spawn(local: &Path, home: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_claude-usage"))
        .arg("--hidden")
        .env("LOCALAPPDATA", local)
        .env("USERPROFILE", home)
        .env_remove("HOMEDRIVE")
        .env_remove("HOMEPATH")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CU_WIDGET_NO_TRACK") // the whole point: Windows' real mouse tracking
        .env("CU_NO_AUTOSTART", "1")
        .env("CU_NO_REPAIR", "1")
        .env("CU_NO_DASHBOARD", "1")
        .spawn()
        .expect("spawn exe")
}

#[test]
fn widget_real_pointer_hover_and_leave() {
    // No "no other tray" guard on purpose: every window is looked up by the pid of the process this test spawned
    // (the user's own tray may be running; its widget sits elsewhere and the sandbox has its own pid file).
    // SAFETY: process-wide, same context as the widget so cursor coordinates are physical pixels.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let data = local.join("claude-usage");
    fs::create_dir_all(&data).unwrap();
    let names = ["claude-a", "claude-b", "claude-c", "claude-d"];
    let mut profs = Map::new();
    let mut plist = vec![];
    for (i, n) in names.iter().enumerate() {
        plist.push(json!({"name": n, "path": home.join(n).to_string_lossy()}));
        profs.insert(
            (*n).to_string(),
            json!({"five": 20.0 + i as f64 * 15.0, "week": 30.0 + i as f64 * 10.0,
                   "five_reset": "2999-01-01T00:00:00+00:00", "week_reset": "2999-02-01T00:00:00+00:00",
                   "fetched_at": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64(),
                   "plan": "team"}),
        );
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    fs::write(
        data.join("usage_cache.json"),
        json!({"ts": ts, "block_enabled": false, "block_pct": 95, "warn_pct": 80, "busy": [], "profiles": profs}).to_string(),
    )
    .unwrap();
    // plate on: the widget pixels are not fully transparent, so Windows hit-tests them (layered window)
    fs::write(
        data.join("config.json"),
        json!({"autostart": false, "notify_enabled": false, "interval_sec": 3600, "profiles": plist,
               "widget_enabled": true, "widget_locked": true, "widget_style": "rings", "widget_theme": "dark",
               "widget_layout": "vertical", "widget_tooltip": true, "widget_plate": true,
               "widget_anchor": "tray", "widget_cx": 1400.0, "widget_cy": 500.0})
        .to_string(),
    )
    .unwrap();

    let child = spawn(&local, &home);
    let pid = child.id();
    let _guard = Guard(child);
    let t0 = Instant::now();
    loop {
        let w = find("ClaudeUsageWidget", Some(pid));
        // SAFETY: plain call on a handle (or 0).
        if w != 0 && unsafe { IsWindowVisible(w as _) } != 0 {
            break;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(20),
            "widget window did not appear"
        );
        sleep(100);
    }
    let rig = Rig {
        data: data.clone(),
        w: find("ClaudeUsageWidget", Some(pid)),
    };
    let d = rig.dump();
    let (sw, sh) = (f(&d["screen"][0]) as i32, f(&d["screen"][1]) as i32);
    assert!(
        d["built_angle"].as_f64().unwrap_or(0.0).abs() < 0.01,
        "test assumes an unrotated widget"
    );

    let _back = CursorBack::new(); // restores the user's pointer at the end (also on panic)
    let away = (sw / 8, sh / 8); // far from the widget (it sits on the right half)
    move_to(away.0, away.1);
    let d0 = rig.until("quiet state before hovering", 3, |d| {
        d["hover"] == -1 && d["tip_visible"] == false
    });

    // 1. real hover over cell 0 -> hover index 0 and the tip appears after the 0.3 s delay
    let (x0, y0) = Rig::cell_center(&d0, 0);
    move_to(x0, y0);
    rig.until_at((x0, y0), "real pointer over cell 0 -> hover 0", 3, |d| {
        d["hover"] == 0
    });
    let tip = rig.until_at((x0, y0), "tip shows after the hover delay", 4, |d| {
        d["tip_visible"] == true
    });
    println!("tip at {} size {}", tip["tip_pos"], tip["tip_size"]);

    // 2. staying still must not produce a spurious WM_MOUSELEAVE
    sleep(800);
    let d = rig.dump();
    assert!(
        d["hover"] == 0 && d["tip_visible"] == true,
        "hover lost while the pointer stood still: {d}"
    );

    // 3. real move to the next cell -> hover index follows (tip stays or switches, never stuck on 0)
    let (x1, y1) = Rig::cell_center(&d0, 1);
    move_to(x1, y1);
    rig.until_at((x1, y1), "real pointer over cell 1 -> hover 1", 3, |d| {
        d["hover"] == 1
    });
    rig.until_at((x1, y1), "tip visible for cell 1", 4, |d| {
        d["tip_visible"] == true
    });

    // 4. real LEAVE: pointer goes far away -> Windows sends WM_MOUSELEAVE (TrackMouseEvent) -> tip hides
    move_to(away.0, away.1);
    rig.until("real WM_MOUSELEAVE hides the tip", 3, |d| {
        d["hover"] == -1 && d["tip_visible"] == false
    });

    // 5. coming back works again (tracking re-armed on every move)
    move_to(x0, y0);
    rig.until_at((x0, y0), "hover again after leaving", 3, |d| {
        d["hover"] == 0
    });
    rig.until_at((x0, y0), "tip again after leaving", 4, |d| {
        d["tip_visible"] == true
    });
    move_to(away.0, away.1);
    rig.until("hidden again", 3, |d| {
        d["hover"] == -1 && d["tip_visible"] == false
    });

    let log = fs::read_to_string(data.join("events.log")).unwrap_or_default();
    assert!(!log.contains("panic"), "panic in events.log: {log}");
}
