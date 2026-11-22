//! Tray state: tooltip text, worst percentage, colour (the pure parts of `refresh_icon` / `make_icon`).

use super::{py_str, Env, Poller};
use crate::config::{get_f64, Config};
use crate::pyval::{fixed, py_num, truthy};
use serde_json::Value;

/// Tooltip length limit of the Windows tray (`[:127]`, in characters).
pub const TOOLTIP_MAX_CHARS: usize = 127;

/// `app.color`: red at/above `block_pct`, amber at/above `warn_pct`, else green.
pub fn color(p: f64, cfg: &Config) -> &'static str {
    if p >= get_f64(cfg, "block_pct", 95.0) {
        "#e06c75"
    } else if p >= get_f64(cfg, "warn_pct", 80.0) {
        "#f5a742"
    } else {
        "#7fd88f"
    }
}

/// What `refresh_icon` hands to the tray: no drawing, only the decisions `make_icon` takes.
#[derive(Debug, Clone, PartialEq)]
pub struct TrayView {
    /// Highest of 5h/7d over accounts with data; `None` = no account has data (grey "?" icon).
    pub pct: Option<f64>,
    /// Outline / text / bar colour (`#6b6b6b` when `pct` is `None`).
    pub color: &'static str,
    /// Text drawn in the icon: `"?"`, `"!!"` (>= 100) or `int(pct)`.
    pub text: String,
    /// Right edge in px (of the 256 px canvas) of the filled bar, `None` when `pct` is None/0.
    pub bar_right: Option<f64>,
    /// Blue "fetching right now" dot.
    pub busy: bool,
    /// Tooltip, at most 127 characters.
    pub tooltip: String,
}

impl<E: Env> Poller<E> {
    /// Pure part of `App.refresh_icon`.
    pub fn tray_view(&self) -> TrayView {
        let num = |d: &Value, k: &str| d.get(k).and_then(py_num).unwrap_or(0.0);
        let mut worst: Option<f64> = None;
        let mut any = false;
        for d in self.data.values() {
            if d.get("five").is_some() {
                any = true;
                let m = num(d, "five").max(num(d, "week"));
                worst = Some(worst.map_or(m, |w| w.max(m)));
            }
        }
        let pct = any.then(|| worst.unwrap_or(0.0));
        let busy = !self.busy.is_empty();
        let (c, text, bar_right) = match pct {
            None => ("#6b6b6b", "?".to_string(), None),
            Some(p) => {
                let t = if p >= 100.0 {
                    "!!".to_string()
                } else {
                    format!("{}", p.trunc() as i64)
                };
                let bar = (p != 0.0).then(|| 44.0 + 28f64.max(168.0 * p.min(100.0) / 100.0));
                (color(p, &self.cfg), t, bar)
            }
        };
        let mut lines = Vec::new();
        for (n, d) in &self.data {
            let mut s = if d.get("five").is_some() {
                format!(
                    "5h {}% 7d {}%",
                    fixed(num(d, "five"), 0),
                    fixed(num(d, "week"), 0)
                )
            } else {
                "chưa có dữ liệu".to_string()
            };
            if let Some(e) = d.get("err").filter(|e| truthy(e)) {
                s.push_str(&format!("  ⚠ {}", py_str(e)));
            }
            lines.push(format!("{n}: {s}"));
        }
        let head = format!(
            "Claude usage{}",
            if busy { " - đang cập nhật..." } else { "" }
        );
        let tooltip: String = format!("{head}\n{}", lines.join("\n"))
            .chars()
            .take(TOOLTIP_MAX_CHARS)
            .collect();
        TrayView {
            pct,
            color: c,
            text,
            bar_right,
            busy,
            tooltip,
        }
    }

    /// `App.refresh_icon`.
    pub fn refresh_icon(&mut self) {
        let v = self.tray_view();
        self.env.refresh_icon(&v);
    }
}
