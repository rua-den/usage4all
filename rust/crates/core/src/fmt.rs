//! Display helpers: reset countdown, "ago", absolute reset time, money, effective poll interval.
//! Every function that depends on the clock takes `now` (Unix seconds, UTC) so it can be tested; the
//! `*_now` wrappers use the real clock.

use crate::cache::now_ts;
use crate::config::Config;
use crate::iso::{aware_epoch_us, iso_ts, parse_iso};
use crate::pyval::{fixed, py_floordiv, py_int};
use crate::MIN_INTERVAL;
use chrono::{Datelike, Duration, Local, TimeZone, Timelike};
use serde_json::Value;

/// widget.VI_DAYS (Monday first).
pub const VI_DAYS: [&str; 7] = ["T2", "T3", "T4", "T5", "T6", "T7", "CN"];

/// core.effective_interval: `max(90, int(interval_sec), 5 * len(profiles or []))`.
/// Deviations: where Python's `int()` raises ValueError/TypeError it uses 180 (same here); where
/// `len(profiles)` would raise (a truthy number/bool) the count is 0 here; a string counts its characters
/// like Python's `len`.
pub fn effective_interval(cfg: &Config) -> i64 {
    let base = match cfg.get("interval_sec") {
        None => 180,
        Some(v) => py_int(v).unwrap_or(180),
    };
    let n = match cfg.get("profiles") {
        Some(Value::Array(a)) => a.len(),
        Some(Value::Object(o)) => o.len(),
        Some(Value::String(s)) => s.chars().count(),
        _ => 0,
    };
    MIN_INTERVAL.max(base).max(5 * n as i64)
}

/// core.fmt_reset with an explicit `now`. `"-"` for empty/invalid/naive input (Python: aware - naive raises
/// TypeError), `"reset"` when already past, else `"{d}d{h:02}h"` or `"{h}h{m:02}m"`.
pub fn fmt_reset_at(iso: Option<&str>, now: f64) -> String {
    let Some(iso) = iso.filter(|s| !s.is_empty()) else {
        return "-".into();
    };
    let Some(p) = parse_iso(iso) else {
        return "-".into();
    };
    let Some(off) = p.offset_us else {
        return "-".into();
    };
    let now_us = (now * 1e6).round_ties_even() as i64;
    let sec = (aware_epoch_us(&p, off) - now_us) as f64 / 1e6;
    if sec <= 0.0 {
        return "reset".into();
    }
    let mins = py_floordiv(sec, 60.0) as i64;
    let (h, m) = (mins / 60, mins % 60);
    let (d, h) = (h / 24, h % 24);
    if d > 0 {
        format!("{d}d{h:02}h")
    } else {
        format!("{h}h{m:02}m")
    }
}

/// [`fmt_reset_at`] with the real clock.
pub fn fmt_reset(iso: Option<&str>) -> String {
    fmt_reset_at(iso, now_ts())
}

/// widget.ago_text: `"chưa có"` for no/zero timestamp, else seconds / minutes / hours ago
/// (`{:.0}`, round-half-even like Python).
pub fn ago_text(ts: Option<f64>, now: f64) -> String {
    let Some(ts) = ts.filter(|t| *t != 0.0) else {
        return "chưa có".into();
    };
    let s = (now - ts).max(0.0);
    if s < 90.0 {
        format!("{}s trước", fixed(s, 0))
    } else if s < 5400.0 {
        format!("{} phút trước", fixed(s / 60.0, 0))
    } else {
        format!("{} giờ trước", fixed(s / 3600.0, 0))
    }
}

/// [`ago_text`] with the real clock.
pub fn ago_text_now(ts: Option<f64>) -> String {
    ago_text(ts, now_ts())
}

/// widget.money: `$1,234.50` for USD/EUR/GBP symbols, else `1,234.50 CUR`. Python `f"{v:,.2f}"`.
pub fn money(v: f64, cur: &str) -> String {
    let body = group_thousands(v);
    match cur {
        "USD" => format!("${body}"),
        "EUR" => format!("€{body}"),
        "GBP" => format!("£{body}"),
        _ => format!("{body} {cur}"),
    }
}

/// Python `f"{v:,.2f}"`.
fn group_thousands(v: f64) -> String {
    if !v.is_finite() {
        return fixed(v, 2);
    }
    let neg = v.is_sign_negative();
    let s = fixed(v.abs(), 2);
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    let mut out = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    format!("{}{out}.{frac}", if neg { "-" } else { "" })
}

/// widget.fmt_when: absolute LOCAL time of a reset: `02:20`, `02:20 ngày mai`, `07:00 T6 13/11`.
/// `now` is Unix seconds; the local time zone decides "today"/"tomorrow". `"-"` for an invalid string.
pub fn fmt_when(iso: &str, now: f64) -> String {
    let Some(ts) = iso_ts(iso) else {
        return "-".into();
    };
    let local = |t: f64| {
        let secs = t.floor();
        Local
            .timestamp_opt(secs as i64, ((t - secs) * 1e9) as u32)
            .single()
    };
    let (Some(dt), Some(nw)) = (local(ts), local(now)) else {
        return "-".into();
    };
    let (d, today) = (dt.date_naive(), nw.date_naive());
    let day = if d == today {
        String::new()
    } else if Some(d) == today.checked_add_signed(Duration::days(1)) {
        " ngày mai".to_string()
    } else {
        format!(
            " {} {}/{}",
            VI_DAYS[d.weekday().num_days_from_monday() as usize],
            d.day(),
            d.month()
        )
    };
    format!("{:02}:{:02}{day}", dt.hour(), dt.minute())
}

/// [`fmt_when`] with the real clock.
pub fn fmt_when_now(iso: &str) -> String {
    fmt_when(iso, now_ts())
}
