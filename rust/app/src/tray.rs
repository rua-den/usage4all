//! Tray process: hidden top-level window + notification-area icon + context menu + balloon toasts, with the
//! daemon loop on a worker thread (a slow HTTP call never freezes the menu). Port of the UI half of `app.py`.
//!
//! Threading: the UI thread owns every Win32 object. The worker only posts messages (`WM_*`) and
//! hands data over through [`Shared`] mutexes.

use crate::daemon::{self, Daemon, RealEnv};
use crate::icon::{self, IconSpec};
use crate::transport::Ureq;
use cu_core::config;
use cu_core::daemon::{Toast, TrayView};
use cu_core::hookinstall::{self, HookProfile};
use cu_core::log::log;
use cu_core::paths::{home_dir, Paths};
use serde_json::Value;
use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, GetDC, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const CLASS: &str = "ClaudeUsageTray";
pub const TITLE: &str = "claude-usage-tray";

const WM_TRAY: u32 = WM_APP + 10;
const WM_ICON: u32 = WM_APP + 1;
const WM_TOAST: u32 = WM_APP + 2;
const WM_SHOW: u32 = WM_APP + 3;

pub const ID_OPEN: usize = 1;
pub const ID_REFRESH: usize = 2;
pub const ID_WIDGET: usize = 3;
pub const ID_NOTIFY: usize = 4;
pub const ID_TEST: usize = 5;
pub const ID_QUIT: usize = 6;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Copy `s` into a fixed UTF-16 buffer, always NUL-terminated (truncates; never splits a surrogate pair).
fn fill(dst: &mut [u16], s: &str) {
    let mut n = 0;
    for ch in s.chars() {
        let mut b = [0u16; 2];
        let u = ch.encode_utf16(&mut b);
        if n + u.len() >= dst.len() {
            break;
        }
        dst[n..n + u.len()].copy_from_slice(u);
        n += u.len();
    }
    dst[n] = 0;
}

#[derive(Default)]
struct Shared {
    view: Mutex<Option<TrayView>>,
    toasts: Mutex<VecDeque<Toast>>,
}

struct State {
    paths: Paths,
    shared: Arc<Shared>,
    hicon: isize,
    taskbar_created: u32,
    /// WinRT toast with our own AUMID (release only); false = tray balloon.
    winrt: bool,
}

fn nid(hwnd: HWND) -> NOTIFYICONDATAW {
    // SAFETY: NOTIFYICONDATAW is plain data; all-zero is a valid empty value.
    let mut n: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    n.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    n.hWnd = hwnd;
    n.uID = 1;
    n
}

/// 64x64 premultiplied BGRA -> HICON (0 on failure).
fn make_hicon(bgra: &[u8]) -> isize {
    // SAFETY: standard DIB-section + CreateIconIndirect sequence; every GDI object is released.
    unsafe {
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: icon::SIZE as i32,
            biHeight: -(icon::SIZE as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bits: *mut core::ffi::c_void = null_mut();
        let dc = GetDC(null_mut());
        let color = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        ReleaseDC(null_mut(), dc);
        if color.is_null() || bits.is_null() {
            return 0;
        }
        std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());
        let mask = CreateBitmap(icon::SIZE as i32, icon::SIZE as i32, 1, 1, null());
        let info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let h = CreateIconIndirect(&info);
        DeleteObject(color);
        DeleteObject(mask);
        h as isize
    }
}

fn add_or_update(hwnd: HWND, st: &mut State, add: bool) {
    let view = st.shared.view.lock().ok().and_then(|g| g.clone());
    let (bytes, tip) = match &view {
        Some(v) => (
            icon::render(&IconSpec {
                color: v.color,
                text: &v.text,
                bar_right: v.bar_right,
                busy: v.busy,
            }),
            v.tooltip.clone(),
        ),
        None => (
            icon::render(&IconSpec {
                color: "#6b6b6b",
                text: "?",
                bar_right: None,
                busy: false,
            }),
            "Claude usage".to_string(),
        ),
    };
    let h = make_hicon(&bytes);
    let mut n = nid(hwnd);
    n.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE;
    n.uCallbackMessage = WM_TRAY;
    n.hIcon = h as _;
    fill(&mut n.szTip, &tip);
    // SAFETY: n is fully initialised; the old icon is destroyed after the shell got the new one.
    unsafe {
        Shell_NotifyIconW(if add { NIM_ADD } else { NIM_MODIFY }, &n);
        if st.hicon != 0 {
            DestroyIcon(st.hicon as _);
        }
    }
    st.hicon = h;
}

fn balloon(hwnd: HWND, t: &Toast) {
    let mut n = nid(hwnd);
    n.uFlags = NIF_INFO;
    n.dwInfoFlags = NIIF_INFO;
    fill(&mut n.szInfoTitle, &t.title);
    fill(&mut n.szInfo, &t.body);
    // SAFETY: valid NOTIFYICONDATAW.
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &n) };
}

/// Toast: WinRT (own AUMID) when enabled, otherwise - or when WinRT fails - the tray balloon.
fn notify(hwnd: HWND, st: &mut State, t: &Toast) {
    if st.winrt {
        match catch_unwind(AssertUnwindSafe(|| crate::toast::show(&t.title, &t.body))) {
            Ok(Ok(())) => return,
            Ok(Err(e)) => log(
                &st.paths,
                &format!("winrt toast failed ({e}); using balloon"),
            ),
            Err(_) => log(&st.paths, "winrt toast panicked; using balloon"),
        }
        st.winrt = false; // do not retry a broken path on every toast
    }
    balloon(hwnd, t);
}

fn toggle(paths: &Paths, key: &str) {
    let mut cfg = config::load_config(paths, &home_dir());
    let cur = cfg.get(key).and_then(Value::as_bool).unwrap_or(true);
    cfg.insert(key.into(), Value::Bool(!cur));
    let _ = config::save_config(paths, &cfg);
}

fn cfg_bool(paths: &Paths, key: &str) -> bool {
    config::load_config(paths, &home_dir())
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn touch(p: &std::path::Path) {
    let _ = std::fs::write(p, "1");
}

/// Execute a menu command (also reachable through WM_COMMAND, which the UI tests use).
fn command(hwnd: HWND, st: &mut State, id: usize) {
    match id {
        ID_OPEN => open_dashboard(&st.paths),
        ID_REFRESH => touch(&st.paths.flag),
        ID_WIDGET => toggle(&st.paths, "widget_enabled"),
        ID_NOTIFY => toggle(&st.paths, "notify_enabled"),
        ID_TEST => touch(&st.paths.testflag),
        ID_QUIT => {
            // SAFETY: valid window handle owned by this thread.
            unsafe { DestroyWindow(hwnd) };
        }
        _ => {}
    }
}

/// Open the dashboard: its own process (`claude-usage-dashboard.exe` next to this exe). A dashboard that is already
/// open focuses itself and the new process exits, so repeated clicks never stack windows. A missing exe or
/// WebView2 is reported with a message instead of a crash (the tray and the hook do not need either).
fn open_dashboard(paths: &Paths) {
    use std::os::windows::process::CommandExt;
    log(paths, "open dashboard requested");
    if std::env::var_os("CU_NO_DASHBOARD").is_some() {
        return; // UI tests of the tray must not open a real window
    }
    let exe = std::env::current_exe()
        .unwrap_or_default()
        .with_file_name("claude-usage-dashboard.exe");
    if !exe.exists() {
        log(paths, "dashboard exe missing");
        message_box(
            "Không tìm thấy claude-usage-dashboard.exe cạnh claude-usage.exe: cài lại bản đầy đủ.",
        );
        return;
    }
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let spawned = std::process::Command::new(&exe)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    match spawned {
        Ok(child) => bind_to_job(&child),
        Err(e) => {
            log(paths, &format!("dashboard spawn failed: {e}"));
            message_box("Không mở được dashboard.");
        }
    }
}

/// One process-wide Job Object (KILL_ON_JOB_CLOSE): children assigned to it die when the tray process ends, even
/// when it is killed. The handle is intentionally never closed. Best effort: failure only means no auto-kill.
fn bind_to_job(child: &std::process::Child) {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use windows_sys::Win32::System::JobObjects::*;
    static JOB: OnceLock<usize> = OnceLock::new();
    let job = *JOB.get_or_init(|| {
        // SAFETY: creates an unnamed job and sets its limit flags; the handle lives for the whole process.
        unsafe {
            let h = CreateJobObjectW(null(), null());
            if h.is_null() {
                return 0;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                0
            } else {
                h as usize
            }
        }
    });
    if job != 0 {
        // SAFETY: valid job and process handles.
        unsafe { AssignProcessToJobObject(job as _, child.as_raw_handle() as _) };
    }
}

fn message_box(text: &str) {
    let (t, c) = (wide(text), wide("claude-usage"));
    // SAFETY: NUL-terminated strings; runs on a short-lived thread so the tray loop keeps going.
    std::thread::spawn(move || unsafe {
        MessageBoxW(null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | MB_ICONWARNING);
    });
}

fn show_menu(hwnd: HWND, st: &State) {
    // SAFETY: standard popup-menu sequence on the owning thread.
    unsafe {
        let m = CreatePopupMenu();
        let add = |id: usize, text: &str, checked: Option<bool>, default: bool| {
            let mut f = MF_STRING;
            if checked == Some(true) {
                f |= MF_CHECKED;
            }
            AppendMenuW(m, f, id, wide(text).as_ptr());
            if default {
                SetMenuDefaultItem(m, id as u32, 0);
            }
        };
        add(ID_OPEN, "Open dashboard", None, true);
        add(ID_REFRESH, "Refresh now", None, false);
        add(
            ID_WIDGET,
            "Show widget",
            Some(cfg_bool(&st.paths, "widget_enabled")),
            false,
        );
        add(
            ID_NOTIFY,
            "Notifications",
            Some(cfg_bool(&st.paths, "notify_enabled")),
            false,
        );
        add(ID_TEST, "Send test notification", None, false);
        add(ID_QUIT, "Quit", None, false);
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd);
        TrackPopupMenu(
            m,
            TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            pt.x,
            pt.y,
            0,
            hwnd,
            null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(m);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if p.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let st = &mut *p;
    let r = catch_unwind(AssertUnwindSafe(|| match msg {
        WM_TRAY => {
            match lp as u32 {
                WM_LBUTTONUP => command(hwnd, st, ID_OPEN), // pystray's default item runs on a single left click
                WM_RBUTTONUP => show_menu(hwnd, st),
                _ => {}
            }
            Some(0)
        }
        WM_COMMAND => {
            command(hwnd, st, wp & 0xffff);
            Some(0)
        }
        WM_ICON => {
            add_or_update(hwnd, st, false);
            Some(0)
        }
        WM_TOAST => {
            let next = st.shared.toasts.lock().ok().and_then(|mut q| q.pop_front());
            if let Some(t) = next {
                notify(hwnd, st, &t);
            }
            Some(0)
        }
        WM_SHOW => {
            open_dashboard(&st.paths);
            Some(0)
        }
        WM_DESTROY => {
            let n = nid(hwnd);
            Shell_NotifyIconW(NIM_DELETE, &n);
            if st.hicon != 0 {
                DestroyIcon(st.hicon as _);
                st.hicon = 0;
            }
            PostQuitMessage(0);
            Some(0)
        }
        m if m == st.taskbar_created && m != 0 => {
            add_or_update(hwnd, st, true); // explorer restarted: the icon is gone, add it again
            Some(0)
        }
        _ => None,
    }));
    match r {
        Ok(Some(v)) => v,
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn worker(paths: Paths, hwnd_raw: usize, shared: Arc<Shared>, stop: Arc<AtomicBool>) {
    let post = move |m: u32| {
        // SAFETY: PostMessageW is thread-safe; a dead window just makes it fail.
        unsafe {
            PostMessageW(hwnd_raw as HWND, m, 0, 0);
        }
    };
    let mut env = RealEnv::new(paths.clone(), Box::new(Ureq));
    {
        let sh = shared.clone();
        env.on_icon = Box::new(move |v| {
            if let Ok(mut g) = sh.view.lock() {
                *g = Some(v.clone());
            }
            post(WM_ICON);
        });
        let sh = shared.clone();
        env.on_toast = Box::new(move |t| {
            sh.toasts
                .lock()
                .map_err(|e| e.to_string())?
                .push_back(t.clone());
            post(WM_TOAST);
            Ok(())
        });
    }
    let mut d = Daemon::new(env);
    // R1 policy: debug builds never touch settings.json; release only repairs a hook whose exe vanished.
    if std::env::var_os("CU_NO_REPAIR").is_none() {
        if let Ok(exe) = std::env::current_exe() {
            let profs: Vec<HookProfile> = config::profiles(&d.poller.cfg)
                .iter()
                .map(|p| HookProfile::new(p.name.clone(), &p.path))
                .collect();
            let lp = paths.clone();
            hookinstall::repair(&profs, &exe, false, &mut |m| log(&lp, m));
        }
    }
    // App.__init__: keep the registry command pointing at this exe when autostart is on.
    if daemon::autostart_allowed()
        && d.poller.cfg.get("autostart").and_then(Value::as_bool) == Some(true)
    {
        if let Ok(exe) = std::env::current_exe() {
            daemon::set_autostart(true, &exe);
        }
    }
    while !stop.load(Ordering::SeqCst) {
        let _ = catch_unwind(AssertUnwindSafe(|| d.step()));
        if std::mem::take(&mut d.show_requested) {
            post(WM_SHOW);
        }
        for _ in 0..10 {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Run the tray app until Quit.
pub fn run(paths: Paths) -> i32 {
    crate::widget::init_dpi();
    // SAFETY: Win32 setup/teardown on the UI thread; the worker only posts messages.
    unsafe {
        let hinst = GetModuleHandleW(null());
        let class = wide(CLASS);
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: null_mut(),
            hCursor: null_mut(),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide(TITLE).as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        if hwnd.is_null() {
            log(&paths, "tray: cannot create window");
            return 1;
        }
        daemon::startup(&paths);
        let shared = Arc::new(Shared::default());
        let st = Box::into_raw(Box::new(State {
            paths: paths.clone(),
            shared: shared.clone(),
            hicon: 0,
            taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            winrt: daemon::autostart_allowed()
                && std::env::current_exe().is_ok_and(|e| crate::toast::register(&e)),
        }));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, st as isize);
        add_or_update(hwnd, &mut *st, true);
        crate::widget::win::spawn(paths.clone(), hwnd as isize);
        let stop = Arc::new(AtomicBool::new(false));
        let (p2, s2, st2) = (paths.clone(), shared.clone(), stop.clone());
        let hw = hwnd as usize;
        let th = std::thread::spawn(move || worker(p2, hw, s2, st2));
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        stop.store(true, Ordering::SeqCst);
        let _ = th.join();
        daemon::shutdown(&paths);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        drop(Box::from_raw(st));
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_truncates_and_terminates() {
        let mut b = [0xffffu16; 8];
        fill(&mut b, "abcdefghij");
        assert_eq!(&b[..7], &"abcdefg".encode_utf16().collect::<Vec<_>>()[..]);
        assert_eq!(b[7], 0);
        let mut b = [0xffffu16; 4];
        fill(&mut b, "ab\u{1F600}"); // 2 units + NUL do not leave room for a surrogate pair: dropped whole
        assert_eq!((b[0], b[1], b[2]), (b'a' as u16, b'b' as u16, 0));
        let mut b = [1u16; 3];
        fill(&mut b, "");
        assert_eq!(b[0], 0);
    }
}
