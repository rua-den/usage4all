//! claude-usage dashboard: its own short-lived process (RAM is freed on close), frameless wry/WebView2 window that
//! shows the same `ui.html` as the Python build through a small pywebview-compatible bridge (`shim.js`).
//! Port of `dashboard.py`; the data/actions half is in `api.rs`, the geometry rules in `geom.rs`.
#![windows_subsystem = "windows"]

mod api;
mod geom;
mod win;

use api::Core;
use geom::{MIN_H, MIN_W};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tao::dpi::{LogicalSize, PhysicalSize};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
use tao::window::{Window, WindowBuilder};
use wry::{WebContext, WebViewBuilder};

const UI_HTML: &str = include_str!("../../../ui.html");
const SHIM: &str = include_str!("shim.js");
const PERSIST_DELAY: Duration = Duration::from_millis(800);
const READY_TIMEOUT: Duration = Duration::from_secs(4);

enum Ev {
    Call(u64, String, Vec<Value>),
}

struct Geo {
    user_sized: bool,
    maximized: bool,
    normal: Option<[i32; 4]>,
    persist_at: Option<Instant>,
}

struct App {
    core: Core,
    hwnd: isize,
    geo: Geo,
    ready: bool,
    /// `--e2e` result path: multi-stage scripts hand control to the test through `<out>.<stage>` files.
    #[cfg(any(debug_assertions, feature = "e2e"))]
    e2e_out: Option<String>,
}

impl App {
    fn persist_soon(&mut self) {
        self.geo.persist_at = Some(Instant::now() + PERSIST_DELAY);
    }

    /// Remember the size the user picked (the normal rect while maximized). Only when the user chose one.
    fn persist_now(&mut self) {
        self.geo.persist_at = None;
        if !self.geo.user_sized {
            return;
        }
        let r = match (self.geo.maximized, self.geo.normal) {
            (true, Some(n)) => n,
            _ => win::rect(self.hwnd),
        };
        if r[2] < MIN_W / 2 || r[3] < MIN_H / 2 {
            return; // window already gone: keep the last good size
        }
        self.core.set_dash_rect(Some(r));
    }

    /// `hide`: hide at once (feels instant), save the size, end the process.
    fn close(&mut self) -> ! {
        win::hide(self.hwnd);
        self.persist_now();
        win::terminate()
    }

    fn restore_saved(&mut self) {
        let (sw, sh) = win::screen();
        if let Some(r) = geom::restore_rect(&self.core.saved_rect(), sw, sh) {
            win::place(self.hwnd, r);
            self.geo.user_sized = true;
        }
    }

    fn call(&mut self, window: &Window, m: &str, a: &[Value]) -> Result<Value, String> {
        let f = |i: usize| a.get(i).and_then(Value::as_f64).unwrap_or(0.0);
        let c = &self.core;
        Ok(match m {
            "state" => c.state(),
            "test_notify" => c.flag(&c.paths.testflag),
            "refresh" => c.flag(&c.paths.flag),
            "hook_status" => c.hook_status(),
            "hook_install" => c.hook_install(),
            "hook_uninstall" => c.hook_uninstall(),
            "widget_unlock" => c.widget_unlock(),
            "widget_reset" => c.widget_reset(),
            "save" => c.save(a.first().unwrap_or(&Value::Null)),
            "hide" => self.close(),
            "minimize" => {
                window.set_minimized(true);
                Value::Null
            }
            "rect" => json!(win::rect(self.hwnd)),
            "set_rect" => {
                // resize by the user (edge handles): from now on the window no longer follows the content height
                self.geo.user_sized = true;
                self.geo.maximized = false;
                let r = [f(0) as i32, f(1) as i32, f(2) as i32, f(3) as i32];
                win::place(self.hwnd, [r[0], r[1], r[2].max(MIN_W), r[3].max(MIN_H)]);
                self.persist_soon();
                Value::Bool(true)
            }
            "work_area" => json!(win::work_area(self.hwnd)),
            "toggle_max" => {
                match (self.geo.maximized, self.geo.normal) {
                    (true, Some(n)) => {
                        win::place(self.hwnd, n);
                        self.geo.maximized = false;
                    }
                    _ => {
                        self.geo.normal = Some(win::rect(self.hwnd));
                        win::place(self.hwnd, win::work_area(self.hwnd));
                        self.geo.maximized = true;
                        self.geo.user_sized = true;
                    }
                }
                self.persist_soon();
                Value::Bool(true)
            }
            "auto_fit" => {
                // forget the chosen size: the window follows the content again
                self.geo.user_sized = false;
                self.geo.maximized = false;
                self.core.set_dash_rect(None);
                Value::Bool(true)
            }
            "win_state" => {
                json!({"user_sized": self.geo.user_sized, "maximized": self.geo.maximized})
            }
            // e2e only: `e2e_stage(name, data)` writes `<out>.<name>` (the test inspects the sandbox, edits files,
            // then creates `<out>.<name>.go`); `e2e_go(name)` tells whether the test released the stage.
            #[cfg(any(debug_assertions, feature = "e2e"))]
            "e2e_stage" | "e2e_go" => {
                let Some(out) = &self.e2e_out else {
                    return Err("not in e2e mode".into());
                };
                let name = a.first().and_then(Value::as_str).unwrap_or("");
                if m == "e2e_stage" {
                    let _ = std::fs::write(
                        format!("{out}.{name}"),
                        a.get(1).map_or("null".into(), Value::to_string),
                    );
                    Value::Bool(true)
                } else {
                    Value::Bool(std::path::Path::new(&format!("{out}.{name}.go")).exists())
                }
            }
            "page_ready" => {
                self.ready = true;
                Value::Bool(true)
            }
            "fit" => {
                if self.geo.user_sized || self.geo.maximized {
                    return Ok(Value::Bool(false)); // the user's size wins
                }
                let scale = window.scale_factor();
                let cur = win::rect(self.hwnd);
                win::place(self.hwnd, geom::fit_rect(cur, f(0), scale, win::screen().1));
                Value::Bool(true)
            }
            _ => return Err(format!("unknown method: {m}")),
        })
    }
}

/// WebView2 data folder for this run inside our own data dir; older ones (left behind because the process is
/// killed on close) are swept in the background. A folder still in use is locked and simply stays.
fn storage_dir(data: &Path) -> PathBuf {
    let root = data.join("webview");
    let mine = root.join(std::process::id().to_string());
    let keep = mine.clone();
    std::thread::spawn(move || {
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten() {
                if e.path() != keep && e.path().is_dir() {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
    });
    mine
}

fn arg_after(flag: &str) -> Option<(String, String)> {
    let v: Vec<String> = std::env::args().collect();
    let i = v.iter().position(|a| a == flag)?;
    Some((v.get(i + 1)?.clone(), v.get(i + 2)?.clone()))
}

fn main() {
    let Ok(paths) = cu_core::paths::Paths::from_env() else {
        std::process::exit(1);
    };
    if let Some(existing) = win::find_existing() {
        win::focus(existing); // single instance: bring the open dashboard forward
        return;
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let core = Core {
        home: cu_core::paths::home_dir(),
        tray_exe: exe.with_file_name("claude-usage.exe"),
        autostart_ok: !cfg!(debug_assertions) && std::env::var_os("CU_NO_AUTOSTART").is_none(),
        paths,
    };
    #[cfg(any(debug_assertions, feature = "e2e"))]
    let e2e = arg_after("--e2e");
    #[cfg(not(any(debug_assertions, feature = "e2e")))]
    let e2e: Option<(String, String)> = {
        let _ = arg_after;
        None
    };

    let n_profiles = core
        .cfg()
        .get("profiles")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let scale = win::system_scale();
    let width = geom::initial_width(n_profiles, f64::from(win::screen().0) / scale);

    let event_loop = EventLoopBuilder::<Ev>::with_user_event().build();
    let proxy: EventLoopProxy<Ev> = event_loop.create_proxy();
    let window = match WindowBuilder::new()
        .with_title("claude-usage")
        .with_decorations(false)
        .with_undecorated_shadow(false)
        .with_visible(false)
        .with_inner_size(LogicalSize::new(f64::from(width), 640.0))
        .with_min_inner_size(PhysicalSize::new(MIN_W as u32, MIN_H as u32))
        .with_window_classname(win::CLASS)
        .build(&event_loop)
    {
        Ok(w) => w,
        Err(e) => fail(&core, &format!("window: {e}")),
    };

    let mut ctx = WebContext::new(Some(storage_dir(&core.paths.data)));
    let handler_proxy = proxy.clone();
    let built = WebViewBuilder::new_with_web_context(&mut ctx)
        .with_html(core.page(UI_HTML))
        .with_initialization_script(SHIM)
        .with_background_color((10, 10, 10, 255))
        .with_incognito(true)
        .with_ipc_handler(move |req| {
            if let Ok(v) = serde_json::from_str::<Value>(req.body()) {
                let id = v["id"].as_u64().unwrap_or(0);
                let m = v["m"].as_str().unwrap_or("").to_string();
                let a = v["a"].as_array().cloned().unwrap_or_default();
                let _ = handler_proxy.send_event(Ev::Call(id, m, a));
            }
        })
        .build(&window);
    let webview = match built {
        Ok(w) => w,
        Err(e) => fail(
            &core,
            &format!("WebView2 không khởi động được: {e}\nCài Microsoft Edge WebView2 Runtime rồi mở lại."),
        ),
    };

    let mut app = App {
        hwnd: window.hwnd() as isize,
        core,
        geo: Geo {
            user_sized: false,
            maximized: false,
            normal: None,
            persist_at: None,
        },
        ready: false,
        #[cfg(any(debug_assertions, feature = "e2e"))]
        e2e_out: e2e.as_ref().map(|(_, o)| o.clone()),
    };
    app.restore_saved();
    let started = Instant::now();
    let mut shown = false;
    let mut e2e_started = false;

    event_loop.run(move |event, _, flow| {
        match event {
            Event::UserEvent(Ev::Call(id, m, a)) => {
                if m == "__drag" {
                    if !app.geo.maximized {
                        let _ = window.drag_window();
                    }
                } else if m == "__log" {
                    let msg = a.first().and_then(Value::as_str).unwrap_or("");
                    cu_core::log::log(&app.core.paths, &format!("dashboard js error: {}", msg.chars().take(300).collect::<String>()));
                } else if m == "__e2e" {
                    if let Some((_, out)) = &e2e {
                        let _ = std::fs::write(out, a.first().map_or("null".into(), Value::to_string));
                    }
                } else {
                    let r = app.call(&window, &m, &a);
                    if id != 0 {
                        let js = match r {
                            Ok(v) => format!("__cu_resolve({id},true,{v})"),
                            Err(e) => format!("__cu_resolve({id},false,{})", json!(e)),
                        };
                        let _ = webview.evaluate_script(&js);
                    }
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => app.close(), // Alt+F4 / taskbar "close": same fast path as the ✕ button
            _ => {}
        }
        // the page has sized the window (or 4 s passed): now it may be shown, without a visible jump
        if !shown && (app.ready || started.elapsed() >= READY_TIMEOUT) {
            shown = true;
            if e2e.is_none() || e2e_visible() {
                window.set_visible(true);
                window.set_focus();
            }
        }
        if let Some((script, _)) = &e2e {
            if shown && !e2e_started {
                e2e_started = true;
                let body = std::fs::read_to_string(script).unwrap_or_default();
                let _ = webview.evaluate_script(&format!(
                    "(async()=>{{try{{const r=await (async()=>{{{body}\n}})();__cu_send('__e2e',[r])}}catch(e){{__cu_send('__e2e',[{{error:String(e)}}])}}}})()"
                ));
            }
        }
        if app.geo.persist_at.is_some_and(|t| Instant::now() >= t) {
            app.persist_now();
        }
        let mut next: Option<Instant> = app.geo.persist_at;
        if !shown {
            let t = started + READY_TIMEOUT;
            next = Some(next.map_or(t, |n| n.min(t)));
        }
        *flow = next.map_or(ControlFlow::Wait, ControlFlow::WaitUntil);
    });
}

/// Test-only (debug builds / `--features e2e`): `CU_E2E_VISIBLE=1` shows the window even under `--e2e`, so a test
/// can drive it with the real mouse. Not compiled into the shipped release.
#[cfg(any(debug_assertions, feature = "e2e"))]
fn e2e_visible() -> bool {
    std::env::var_os("CU_E2E_VISIBLE").is_some()
}
#[cfg(not(any(debug_assertions, feature = "e2e")))]
fn e2e_visible() -> bool {
    false
}

/// Startup failure: say so (events.log + a message box), never crash silently.
fn fail(core: &Core, msg: &str) -> ! {
    cu_core::log::log(&core.paths, &format!("dashboard: {msg}"));
    win::message_box(msg);
    std::process::exit(1)
}
