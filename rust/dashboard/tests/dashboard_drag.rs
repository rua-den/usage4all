//! REAL mouse drag of the frameless dashboard by its title bar (SendInput), exercising the whole chain:
//! page `mousedown` on `.pywebview-drag-region` → `mousemove` with the button held → shim `__drag` → host
//! `drag_window()` (native move loop). The window is the test's own sandbox dashboard (made visible with the
//! test-only `CU_E2E_VISIBLE`, set topmost); the user's cursor is moved for a few seconds and restored.
//! Controls: pressing on a non-drag area, or on the title bar without moving, must NOT move the window.
//! After the drag the window is closed with WM_CLOSE and the remembered position (`dash_rect`) is checked.
//!
//! Sandboxed LOCALAPPDATA/USERPROFILE, fake profiles without credentials, no network.
//! ONE test function on purpose (GUI tests must not run concurrently).
#![cfg(any(debug_assertions, feature = "e2e"))]

mod common;
use common::*;
use serde_json::{json, Value};
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, GetWindowRect, GetWindowThreadProcessId, PostMessageW, SetCursorPos, SetWindowPos,
    HWND_TOPMOST, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, WM_CLOSE,
};

const JS: &str = r#"
await waitFor(() => document.querySelectorAll('.acc').length === 2, 8000);
await sleep(800);
const r = document.querySelector('.drag').getBoundingClientRect();
const fr = document.querySelector('footer').getBoundingClientRect();
await stage('ready', { dpr: devicePixelRatio, drag: [r.left, r.top, r.width, r.height], foot: [fr.left, fr.top, fr.width, fr.height],
  maxb: $('maxb').textContent });
return { done: true };
"#;

/// The shim starts the native move loop only once the pointer has left a 3 px radius, so the window lags the
/// pointer by the distance covered until then (one drag step, ~6 px here): `moved` is in [want - 12, want + 3].
fn within(moved: i32, want: i32) -> bool {
    (want - 12..=want + 3).contains(&moved)
}

fn sleep(ms: u64) {
    std::thread::sleep(std::time::Duration::from_millis(ms));
}

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
        // SAFETY: plain call. Also releases nothing: the button is released by the test itself (see `Button`).
        unsafe { SetCursorPos(self.0.x, self.0.y) };
    }
}

fn send(flags: u32, dx: i32, dy: i32) {
    let mi = MOUSEINPUT { dx, dy, mouseData: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 };
    let inp = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi } };
    // SAFETY: fully initialised INPUT.
    unsafe { SendInput(1, &inp, std::mem::size_of::<INPUT>() as i32) };
}

/// Absolute pointer move in physical pixels (no pointer acceleration, unlike relative moves).
fn move_abs(x: i32, y: i32) {
    // SAFETY: plain metric queries.
    let (vx, vy, vw, vh) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    let nx = ((x - vx) as f64 * 65535.0 / f64::from(vw - 1)).round() as i32;
    let ny = ((y - vy) as f64 * 65535.0 / f64::from(vh - 1)).round() as i32;
    send(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, nx, ny);
}

/// Left button held down: always released on drop (also on a failed assertion), so the user's mouse is never stuck.
struct Button(bool);
impl Button {
    fn press() -> Button {
        send(MOUSEEVENTF_LEFTDOWN, 0, 0);
        Button(true)
    }
    fn release(&mut self) {
        if self.0 {
            send(MOUSEEVENTF_LEFTUP, 0, 0);
            self.0 = false;
        }
    }
}
impl Drop for Button {
    fn drop(&mut self) {
        self.release();
    }
}

fn win_rect(h: HWND) -> [i32; 4] {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: valid window, valid out pointer.
    unsafe { GetWindowRect(h, &mut r) };
    [r.left, r.top, r.right - r.left, r.bottom - r.top]
}

fn client_origin(h: HWND) -> (i32, i32) {
    let mut p = POINT { x: 0, y: 0 };
    // SAFETY: valid window, valid in/out pointer.
    unsafe { ClientToScreen(h, &mut p) };
    (p.x, p.y)
}

/// Press at `from`, move to `to` in `steps` absolute steps, release. Returns after the system move loop ended.
fn drag(from: (i32, i32), to: (i32, i32), steps: i32) {
    move_abs(from.0, from.1);
    sleep(150);
    let mut b = Button::press();
    sleep(150);
    for i in 1..=steps {
        move_abs(from.0 + (to.0 - from.0) * i / steps, from.1 + (to.1 - from.1) * i / steps);
        sleep(20);
    }
    sleep(150);
    b.release();
    sleep(400);
}

#[test]
fn dashboard_titlebar_drag_with_real_mouse() {
    assert!(
        window().is_null(),
        "a dashboard window is already open (the user's?): close it before running this test"
    );
    // SAFETY: process-wide, same context as the dashboard so cursor coordinates are physical pixels.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let names: Vec<String> = vec!["claude-a".into(), "claude-b".into()];
    let sb = Sandbox::new(&names);
    sb.write_cache(
        |i, _, now| {
            json!({"five": 20 + i * 30, "week": 10 + i * 20, "plan": "team",
                   "five_reset": "2999-01-01T00:00:00+00:00", "week_reset": "2999-02-01T00:00:00+00:00",
                   "fetched_at": now, "retry_at": null, "err": null})
        },
        &[],
        None,
    );
    // a remembered, modest window well inside the screen (physical px), so the drag has room on every side
    let mut cfg = sb.cfg();
    cfg["dash_rect"] = json!([300, 200, 700, 520]);
    std::fs::write(sb.data.join("config.json"), cfg.to_string()).unwrap();

    std::env::set_var("CU_E2E_VISIBLE", "1"); // inherited by the spawned dashboard
    let mut run = Run::start(&sb.local, &sb.home, sb.root.path(), "drag", JS, "");
    let d: Value = run.stage("ready");
    let mut checks: Vec<(String, bool, String)> = vec![];

    let hwnd = {
        wait_for("dashboard window", 10, || !window().is_null());
        window()
    };
    let mut pid = 0u32;
    // SAFETY: valid window, valid out pointer.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    checks.push(("the window under test is the process we spawned".into(), pid == run.guard.0.id(), format!("{pid}")));
    // our own window, on top of everything, never stealing focus
    // SAFETY: plain call on our own window.
    unsafe { SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) };
    sleep(300);

    let _back = CursorBack::new();
    let dpr = d["dpr"].as_f64().unwrap_or(1.0);
    let n = |v: &Value, i: usize| v[i].as_f64().unwrap_or(0.0);
    let (ox, oy) = client_origin(hwnd);
    let css = |x: f64, y: f64| (ox + (x * dpr).round() as i32, oy + (y * dpr).round() as i32);
    // inside the title text (a child of .pywebview-drag-region) and inside the footer (no drag region)
    let title = css(n(&d["drag"], 0) + 60.0, n(&d["drag"], 1) + n(&d["drag"], 3) / 2.0);
    let foot = css(n(&d["foot"], 0) + 20.0, n(&d["foot"], 1) + n(&d["foot"], 3) / 2.0);
    let r0 = win_rect(hwnd);
    println!("window {r0:?} title point {title:?} footer point {foot:?} dpr {dpr}");
    checks.push((
        "title bar is on screen and the remembered rect was restored".into(),
        r0[0] == 300 && r0[1] == 200 && r0[2] == 700 && r0[3] == 520,
        format!("{r0:?}"),
    ));

    // controls: no movement without the drag region, and a plain click on the title does not move either
    drag(foot, (foot.0 + 160, foot.1 + 90), 12);
    let r1 = win_rect(hwnd);
    checks.push(("control: dragging from a non-drag area (footer) does not move the window".into(), r1 == r0, format!("{r0:?} -> {r1:?}")));
    drag(title, (title.0 + 1, title.1), 1);
    let r1 = win_rect(hwnd);
    checks.push(("control: clicking the title bar without moving does not move the window".into(), r1 == r0, format!("{r0:?} -> {r1:?}")));

    // the real thing: press on the title bar, move (+150, +90), release
    let (dx, dy) = (150, 90);
    drag(title, (title.0 + dx, title.1 + dy), 25);
    let r2 = win_rect(hwnd);
    checks.push((
        "real title-bar drag moves the window by the pointer delta minus the first few px (size unchanged)".into(),
        within(r2[0] - r0[0], dx) && within(r2[1] - r0[1], dy) && r2[2] == r0[2] && r2[3] == r0[3],
        format!("{r0:?} -> {r2:?} (wanted +{dx},+{dy})"),
    ));

    // a second drag back (left/up) works too: the shim re-arms after each gesture
    let t2 = (title.0 + dx, title.1 + dy);
    drag(t2, (t2.0 - 60, t2.1 - 40), 15);
    let r3 = win_rect(hwnd);
    checks.push((
        "a second drag works (shim re-armed)".into(),
        within(r2[0] - r3[0], 60) && within(r2[1] - r3[1], 40),
        format!("{r2:?} -> {r3:?}"),
    ));

    // close like Alt+F4: the new position is remembered (dash_rect), process ends
    run.go("ready");
    // SAFETY: posting WM_CLOSE to our own window.
    unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
    let exited = run.exited(15);
    checks.push(("WM_CLOSE ends the process".into(), exited, String::new()));
    let saved = sb.cfg()["dash_rect"].clone();
    let sv: Vec<i32> = saved.as_array().map(|a| a.iter().map(|x| x.as_f64().unwrap_or(-1.0) as i32).collect()).unwrap_or_default();
    checks.push((
        "dash_rect remembers the dragged position and size".into(),
        sv.len() == 4 && (sv[0] - r3[0]).abs() <= 2 && (sv[1] - r3[1]).abs() <= 2 && sv[2] == r0[2] && sv[3] == r0[3],
        format!("saved {saved} window {r3:?}"),
    ));
    checks.append(&mut run.checks);
    report("real title-bar drag (SendInput) -> Rust dashboard", &checks);
}
