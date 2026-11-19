//! Pure, GUI-free, network-free daemon logic: poll scheduler, alerts/toasts and tray state (port of the
//! non-UI parts of `app.py`). Everything with a side effect (clock, randomness, fetch, cache/alert file
//! writes, icon, toast, log) goes through the [`Env`] trait so the Python behaviour can be replayed
//! deterministically (see `tools/gen_fixtures_daemon.py` and `tests/parity_daemon.rs`).

mod alerts;
mod poll;
mod tray;

pub use tray::{color, TrayView};

use crate::config::{profiles, Config, Profile};
use crate::pyval::truthy;
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashMap};

/// app.TOAST_BATCH_SEC: wait this long to merge alerts that arrive together.
pub const TOAST_BATCH_SEC: f64 = 6.0;
/// app.TOAST_MIN_GAP_SEC: never show toasts more often than this.
pub const TOAST_MIN_GAP_SEC: f64 = 15.0;
/// app.TOAST_MAX_LINES: accounts listed in one toast.
pub const TOAST_MAX_LINES: usize = 4;
/// app.SAME_WINDOW_SEC: reset times within 30 min of the remembered one = same usage window.
pub const SAME_WINDOW_SEC: f64 = 1800.0;
/// Seconds after start at which the first-run baseline ends even if an account never reported.
pub const BASELINE_MAX_SEC: f64 = 120.0;
/// Characters of a toast body that are actually sent (`msg[:250]`).
pub const TOAST_MAX_CHARS: usize = 250;

/// Result of a fetch, shaped like Python's: `{five, five_reset, week, week_reset, ...}` on success, or
/// `{err, status[, retry_after]}` on failure (`status` 0 = no HTTP status; `retry_after` seconds or null).
pub type FetchResult = Map<String, Value>;

/// A toast ready to be shown (body already cut to 250 characters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub title: String,
    pub body: String,
}

/// One queued alert (app.pending item).
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    /// Time the alert was queued.
    pub t: f64,
    /// `"warn"` or `"block"`.
    pub lvl: &'static str,
    pub name: String,
    /// `"5h"` or `"7d"`.
    pub label: &'static str,
    pub v: f64,
}

/// Side effects and inputs of the daemon logic. The real app implements this with the system clock,
/// `rand`, Phase 3's fetch / token refresh, `cache::write_cache`, `alerts.json`, the tray and `log`.
pub trait Env {
    /// Unix seconds (`time.time()`).
    fn now(&self) -> f64;
    /// Uniform in [0, 1) (`random.random()`).
    fn rand(&mut self) -> f64;
    /// `core.refresh_if_expired(p)`: `None` = nothing to do / it worked (then `fetch` reads the file),
    /// else an error result. Only called when `cfg.token_refresh` is truthy. An empty map counts as `None`
    /// (Python `or`).
    fn refresh_if_expired(&mut self, p: &Profile) -> Option<FetchResult>;
    /// `core.fetch(p)`.
    fn fetch(&mut self, p: &Profile) -> FetchResult;
    /// `core.write_cache(cfg, data, busy, refresh)` (forward to `cache::write_cache`).
    fn write_cache(
        &mut self,
        cfg: &Config,
        data: &Map<String, Value>,
        busy: &[String],
        refresh: Option<Value>,
    );
    /// Tray icon + tooltip changed.
    fn refresh_icon(&mut self, view: &TrayView);
    /// One `core.log` line.
    fn log(&mut self, msg: &str);
    /// Persist the alert state map to `alerts.json` (failures ignored, like Python).
    fn save_alert_state(&mut self, st: &Map<String, Value>);
    /// Show a toast. `Err(e)` is logged as `toast failed: {e}`.
    fn send_toast(&mut self, t: &Toast) -> Result<(), String>;
}

/// Inputs of one loop iteration that come from outside (config file change, flag files).
#[derive(Debug, Clone, Default)]
pub struct TickInput {
    /// The freshly loaded config when `config.json`'s mtime changed since the last tick.
    pub config: Option<Config>,
    /// `notify_test.flag` was present.
    pub test_toast: bool,
    /// `refresh.flag` was present.
    pub refresh: bool,
}

/// State of `app.App` minus the GUI.
pub struct Poller<E: Env> {
    pub env: E,
    pub cfg: Config,
    pub data: Map<String, Value>,
    pub next_at: HashMap<String, f64>,
    pub fails: HashMap<String, u32>,
    pub busy: BTreeSet<String>,
    pub refresh_info: Option<Value>,
    pub pending: Vec<Pending>,
    pub last_toast: f64,
    pub baseline: bool,
    pub t_start: f64,
    pub alert_state: Map<String, Value>,
}

impl<E: Env> Poller<E> {
    /// `alert_state`: contents of `alerts.json` (`None` = the file does not exist = first run = baseline).
    pub fn new(env: E, cfg: Config, alert_state: Option<Map<String, Value>>) -> Self {
        let t_start = env.now();
        Poller {
            env,
            cfg,
            data: Map::new(),
            next_at: HashMap::new(),
            fails: HashMap::new(),
            busy: BTreeSet::new(),
            refresh_info: None,
            pending: Vec::new(),
            last_toast: 0.0,
            baseline: alert_state.is_none(),
            t_start,
            alert_state: alert_state.unwrap_or_default(),
        }
    }

    /// Start of `App.loop`: `seed(); publish()`.
    pub fn start(&mut self, cache: Option<&Value>) {
        self.seed(cache);
        self.publish();
    }

    /// One iteration of the body of `App.loop` (minus SHOW flag and GUI): config reload, test toast,
    /// manual refresh, at most ONE due profile (config order) polled + published, then toast flush.
    pub fn tick(&mut self, input: TickInput) {
        if let Some(c) = input.config {
            self.set_config(c);
        }
        if input.test_toast {
            self.test_toast();
        }
        if input.refresh {
            self.refresh_requested();
        }
        let now = self.env.now();
        for p in profiles(&self.cfg) {
            let due = now >= *self.next_at.entry(p.name.clone()).or_insert(now);
            if due {
                self.poll_one(&p);
                self.publish();
                break;
            }
        }
        self.flush_toasts();
    }

    /// `App.write`.
    pub(crate) fn write(&mut self) {
        let busy: Vec<String> = self.busy.iter().cloned().collect();
        let refresh = self.refresh_info.clone();
        self.env.write_cache(&self.cfg, &self.data, &busy, refresh);
    }

    /// `App.publish`: write cache, refresh the icon, evaluate alerts on fresh data.
    pub fn publish(&mut self) {
        self.write();
        self.refresh_icon();
        let fresh = self.fresh_data(|_| true);
        self.alerts(&fresh);
    }

    /// Accounts with up-to-date data: `"five" in d and not d.get("err")` and `keep(name)`.
    pub(crate) fn fresh_data(&self, keep: impl Fn(&str) -> bool) -> Map<String, Value> {
        self.data
            .iter()
            .filter(|(n, d)| {
                keep(n)
                    && d.as_object().is_some_and(|o| {
                        o.contains_key("five") && !o.get("err").is_some_and(truthy)
                    })
            })
            .map(|(n, d)| (n.clone(), d.clone()))
            .collect()
    }
}

/// Python `str(v)` for the values that show up in messages.
pub(crate) fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        other => other.to_string(),
    }
}
