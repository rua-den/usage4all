//! claude-usage            -> tray + background poller (single instance; a 2nd run writes show.flag)
//! claude-usage --hidden   -> same (autostart); there is no window to hide yet
//! claude-usage dashboard  -> starts claude-usage-dashboard.exe (next to this exe)
//! claude-usage hook       -> Claude Code UserPromptSubmit hook (exit 0 allow / 2 block)
//!
//! `main` dispatches on argv before any other initialisation: the hook path touches nothing but env vars,
//! one file read and stderr (no COM, no windows, no threads).
#![windows_subsystem = "windows"]

mod daemon;
mod icon;
mod toast;
mod transport;
mod tray;
mod widget;

use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};

fn main() {
    let first = std::env::args_os().nth(1);
    if first.as_deref() == Some(std::ffi::OsStr::new("hook")) {
        std::process::exit(hook_main());
    }
    let mode = match first.as_deref().and_then(|a| a.to_str()) {
        Some("dashboard") => "dashboard",
        Some("preview") => "preview",
        _ => "tray",
    };
    // Never fail the caller over logging.
    let code = catch_unwind(|| {
        let Ok(paths) = cu_core::paths::Paths::from_env() else {
            return 1;
        };
        match mode {
            "dashboard" => {
                // same effect as the tray menu: the dashboard process (single instance, focuses an open one)
                if std::env::var_os("CU_NO_DASHBOARD").is_some() {
                    cu_core::log::log(&paths, "dashboard skipped (CU_NO_DASHBOARD)");
                    return 0;
                }
                let exe = std::env::current_exe()
                    .unwrap_or_default()
                    .with_file_name("claude-usage-dashboard.exe");
                match std::process::Command::new(&exe).spawn() {
                    Ok(_) => 0,
                    Err(e) => {
                        cu_core::log::log(&paths, &format!("dashboard spawn failed: {e}"));
                        1
                    }
                }
            }
            "preview" => {
                // claude-usage preview <dir>: render widget sample PNGs (no window, no desktop capture)
                let dir = std::env::args_os()
                    .nth(2)
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| paths.data.join("preview"));
                widget::preview(&dir).map_or(1, |_| 0)
            }
            _ if daemon::daemon_running(&paths) => {
                // Already running: ask it to open the dashboard (App.main).
                let _ = std::fs::write(&paths.show, "1");
                0
            }
            _ => tray::run(paths),
        }
    })
    .unwrap_or(1);
    std::process::exit(code);
}

/// Exit code of the hook. Any panic (or failure to deliver the message) fails open: exit 0.
fn hook_main() -> i32 {
    std::panic::set_hook(Box::new(|_| {}));
    catch_unwind(AssertUnwindSafe(|| {
        let r = cu_core::hook::run_from_env();
        if let Some(msg) = r.stderr {
            let mut e = std::io::stderr().lock();
            let _ = e.write_all(msg.as_bytes());
            let _ = e.flush();
        }
        r.code
    }))
    .unwrap_or(0)
}
