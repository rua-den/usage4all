//! Alerts and toasts: dedupe per usage window, baseline on first run, batching, rate limit.

use super::{
    Env, Pending, Poller, Toast, BASELINE_MAX_SEC, SAME_WINDOW_SEC, TOAST_BATCH_SEC,
    TOAST_MAX_CHARS, TOAST_MAX_LINES, TOAST_MIN_GAP_SEC,
};
use crate::config::get_f64;
use crate::iso::iso_ts;
use crate::pyval::{fixed, py_num, truthy};
use serde_json::{Map, Value};

impl<E: Env> Poller<E> {
    fn cfg_truthy(&self, key: &str) -> bool {
        self.cfg.get(key).is_none_or(truthy)
    }

    /// `App.alerts`. `fresh` = accounts with up-to-date (non-stale) data. Queues one alert per
    /// (account, window, level) per usage window. State is recorded even when notifications are off, so
    /// turning them on later does not fire a burst for conditions that already existed.
    /// Deviation: a stored state value that is not a number counts as "different window" (Python raises).
    pub fn alerts(&mut self, fresh: &Map<String, Value>) {
        let block = get_f64(&self.cfg, "block_pct", 95.0);
        let warn = get_f64(&self.cfg, "warn_pct", 80.0);
        let mut changed = false;
        for (n, d) in fresh {
            for (key, label) in [("five", "5h"), ("week", "7d")] {
                let Some(v) = d.get(key).and_then(py_num) else {
                    continue;
                };
                let lvl = if v >= block {
                    "block"
                } else if v >= warn {
                    "warn"
                } else {
                    continue;
                };
                let rts = d
                    .get(format!("{key}_reset"))
                    .and_then(Value::as_str)
                    .and_then(iso_ts)
                    .unwrap_or(0.0);
                let k = format!("{n}|{key}|{lvl}");
                if let Some(old) = self.alert_state.get(&k).and_then(py_num) {
                    if (rts - old).abs() < SAME_WINDOW_SEC {
                        continue; // API jitters the reset time by ~1 s: compare loosely
                    }
                }
                self.alert_state.insert(k, Value::from(rts));
                changed = true;
                if self.cfg_truthy("notify_enabled")
                    && self.cfg_truthy(&format!("notify_{lvl}"))
                    && !self.baseline
                {
                    let t = self.env.now();
                    self.pending.push(Pending {
                        t,
                        lvl,
                        name: n.clone(),
                        label,
                        v,
                    });
                }
            }
        }
        if self.baseline {
            let nprof = match self.cfg.get("profiles") {
                Some(Value::Array(a)) => a.len(),
                _ => 0,
            };
            if fresh.len() >= nprof || self.env.now() - self.t_start > BASELINE_MAX_SEC {
                self.baseline = false; // every account seen once (or 2 min passed): announce changes now
                changed = true;
            }
        }
        if changed {
            self.env.save_alert_state(&self.alert_state);
        }
    }

    fn send_toast(&mut self, title: &str, msg: &str) {
        self.env
            .log(&format!("toast | {title} | {}", msg.replace('\n', " / ")));
        let t = Toast {
            title: title.to_string(),
            body: msg.chars().take(TOAST_MAX_CHARS).collect(),
        };
        if let Err(e) = self.env.send_toast(&t) {
            self.env.log(&format!("toast failed: {e}"));
        }
    }

    /// `App.flush_toasts`: merge queued alerts into one compact toast (one line per account) and suggest
    /// the emptiest account. Waits `TOAST_BATCH_SEC` so alerts that arrive together are batched; never more
    /// often than `TOAST_MIN_GAP_SEC`.
    pub fn flush_toasts(&mut self) {
        let now = self.env.now();
        if !self.cfg_truthy("notify_enabled") {
            self.pending.clear();
        }
        let Some(first) = self.pending.first() else {
            return;
        };
        if now - first.t < TOAST_BATCH_SEC || now - self.last_toast < TOAST_MIN_GAP_SEC {
            return;
        }
        let items = std::mem::take(&mut self.pending);
        self.last_toast = now;
        let mut by: Vec<(&str, Vec<&Pending>)> = Vec::new();
        for e in &items {
            match by.iter_mut().find(|(n, _)| *n == e.name) {
                Some((_, v)) => v.push(e),
                None => by.push((&e.name, vec![e])),
            }
        }
        let mut lines: Vec<String> = by
            .iter()
            .map(|(n, es)| {
                let parts: Vec<String> = es
                    .iter()
                    .map(|e| format!("{} {}%", e.label, fixed(e.v, 0)))
                    .collect();
                format!("{n}: {}", parts.join(" · "))
            })
            .collect();
        if lines.len() > TOAST_MAX_LINES {
            // many accounts crossed at once: keep the toast readable
            let extra = lines.len() - TOAST_MAX_LINES;
            lines.truncate(TOAST_MAX_LINES);
            lines.push(format!("+{extra} account khác"));
        }
        let fresh = self.fresh_data(|n| !by.iter().any(|(b, _)| *b == n));
        let mut best: Option<(&String, f64, f64, f64)> = None; // first minimum in data order
        for (m, d) in &fresh {
            let five = d.get("five").and_then(py_num).unwrap_or(0.0);
            let week = d.get("week").and_then(py_num).unwrap_or(0.0);
            let worst = five.max(week);
            if best.is_none_or(|(_, _, _, w)| worst < w) {
                best = Some((m, five, week, worst));
            }
        }
        if let Some((m, five, week, _)) = best {
            lines.push(format!(
                "Nên dùng: {m} (5h {}% · 7d {}%)",
                fixed(five, 0),
                fixed(week, 0)
            ));
        }
        let blocked = items.iter().any(|e| e.lvl == "block");
        let title = format!(
            "Claude usage - {}{}",
            if blocked { "CHẶN" } else { "sắp hết" },
            if by.len() > 1 {
                format!(" ({} account)", by.len())
            } else {
                String::new()
            }
        );
        let body = lines.join("\n");
        self.send_toast(&title, &body);
    }

    /// `App.test_toast`.
    pub fn test_toast(&mut self) {
        let on = self.cfg_truthy("notify_enabled");
        let msg = format!(
            "Thông báo đang {}",
            if on {
                "BẬT"
            } else {
                "TẮT (tin này chỉ là bản thử)"
            }
        );
        self.send_toast("Claude usage - thử thông báo", &msg);
    }
}
