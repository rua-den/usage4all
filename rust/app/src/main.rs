//! claude-usage            -> dashboard + tray (single instance)        [not implemented yet]
//! claude-usage --hidden   -> same, window hidden (autostart)           [not implemented yet]
//! claude-usage dashboard  -> dashboard process                         [not implemented yet]
//! claude-usage hook       -> Claude Code UserPromptSubmit hook (exit 0 allow / 2 block)
//!
//! `main` dispatches on argv before any other initialisation: the hook path touches nothing but env vars,
//! one file read and stderr (no COM, no windows, no threads).
#![windows_subsystem = "windows"]

use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};

fn main() {
    let first = std::env::args_os().nth(1);
    if first.as_deref() == Some(std::ffi::OsStr::new("hook")) {
        std::process::exit(hook_main());
    }
    let mode = match first.as_deref().and_then(|a| a.to_str()) {
        Some("dashboard") => "dashboard",
        _ if std::env::args_os().any(|a| a == "--hidden") => "--hidden",
        _ => "tray",
    };
    // Never fail the caller over logging.
    let _ = catch_unwind(|| {
        if let Ok(paths) = cu_core::paths::Paths::from_env() {
            cu_core::log::log(&paths, &format!("not implemented yet: {mode}"));
        }
    });
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
