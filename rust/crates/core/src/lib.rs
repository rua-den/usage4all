//! cu-core: pure data/config/cache/format logic of claude-usage, ported from `core.py` (+ 3 helpers of
//! `widget.py`). The Python code is the reference: parity is checked against fixtures generated from it
//! (`tools/gen_fixtures.py` -> `tests/fixtures/*.json`, compared in `tests/parity.rs`).
//!
//! Out of scope in this phase: fetch / token refresh / locks / HTTP, daemon_running, autostart, hook, GUI.

pub mod alias;
pub mod atomic;
pub mod cache;
pub mod config;
pub mod fmt;
pub mod iso;
pub mod log;
pub mod parse;
pub mod paths;
pub mod pyval;

pub use serde_json::{Map, Value};

/// Poll interval floor (core.MIN_INTERVAL).
pub const MIN_INTERVAL: i64 = 90;
