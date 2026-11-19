//! `datetime.fromisoformat` (Python 3.14) for the strings the API sends, and `core.iso_ts`.
//!
//! Accepted (as Python 3.14 does): `YYYY-MM-DD` or `YYYYMMDD`, optionally followed by ONE arbitrary
//! separator character (`T`, space, ...) and a time `HH[:MM[:SS[.f+]]]` / `HH[MM[SS[.f+]]]` (`.` or `,`,
//! any number of fraction digits, only the first 6 kept), optionally followed by `Z` or a UTC offset
//! `+-HH[[:]MM[[:]SS[.f+]]]`; hour 24 only as exact midnight (next day). ISO week dates (`2026-W47-3`,
//! `2026W473`) are accepted too. Python's quirk "one extra ASCII character may sit between the time and the
//! offset" is reproduced. Deviations: non-ASCII digits are not accepted as digits.

use chrono::{Duration, Local, NaiveDate, NaiveDateTime, TimeZone};

/// A parsed ISO timestamp. `offset_us` is `None` for a naive datetime (local time in Python).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parsed {
    pub naive: NaiveDateTime,
    pub offset_us: Option<i64>,
}

const MICROS: i64 = 1_000_000;

fn digits(c: &[char]) -> Option<u32> {
    if c.is_empty() || !c.iter().all(char::is_ascii_digit) {
        return None;
    }
    c.iter()
        .try_fold(0u32, |a, d| Some(a * 10 + d.to_digit(10)?))
}

/// ISO week date `YYYY-Www[-D]` / `YYYYWww[D]` (exactly that text; day defaults to Monday).
fn week_date(c: &[char]) -> Option<NaiveDate> {
    let (y, w, d) = match c.len() {
        8 if c[4] == '-' && c[5] == 'W' => (digits(&c[0..4])?, digits(&c[6..8])?, 1),
        10 if c[4] == '-' && c[5] == 'W' && c[8] == '-' => {
            (digits(&c[0..4])?, digits(&c[6..8])?, digits(&c[9..10])?)
        }
        7 if c[4] == 'W' => (digits(&c[0..4])?, digits(&c[5..7])?, 1),
        8 if c[4] == 'W' => (digits(&c[0..4])?, digits(&c[5..7])?, digits(&c[7..8])?),
        _ => return None,
    };
    if y == 0 || !(1..=7).contains(&d) {
        return None;
    }
    let wd = chrono::Weekday::try_from((d - 1) as u8).ok()?;
    NaiveDate::from_isoywd_opt(y as i32, w, wd)
}

/// Date part and the index of the date/time separator (Python's `_find_isoformat_datetime_separator`).
fn parse_date(c: &[char]) -> Option<(NaiveDate, usize)> {
    if c.len() < 7 {
        return None;
    }
    if c.len() >= 6 && c[4] == '-' && c[5] == 'W' {
        let pos = if c.len() > 8 && c[8] == '-' {
            if c.len() == 9 {
                return None;
            }
            if c.len() > 10 && c[10].is_ascii_digit() {
                8
            } else {
                10
            }
        } else {
            8
        };
        return Some((week_date(c.get(..pos)?)?, pos));
    }
    if c[4] == 'W' {
        let mut idx = 7;
        while idx < c.len() && c[idx].is_ascii_digit() {
            idx += 1;
        }
        let pos = if idx < 9 {
            idx
        } else if idx % 2 == 0 {
            7
        } else {
            8
        };
        return Some((week_date(c.get(..pos)?)?, pos));
    }
    let (y, m, d, pos) = if c[4] == '-' {
        if c.len() < 10 || c[7] != '-' {
            return None;
        }
        (digits(&c[0..4])?, digits(&c[5..7])?, digits(&c[8..10])?, 10)
    } else {
        if c.len() < 8 {
            return None;
        }
        (digits(&c[0..4])?, digits(&c[4..6])?, digits(&c[6..8])?, 8)
    };
    if y == 0 {
        return None;
    }
    Some((NaiveDate::from_ymd_opt(y as i32, m, d)?, pos))
}

/// Python's `_parse_hh_mm_ss_ff`: (h, m, s, micro, had_fraction). No range checks here.
fn parse_hms(t: &[char]) -> Option<(u32, u32, u32, u32, bool)> {
    let len = t.len();
    let mut comps = [0u32; 3];
    let (mut pos, mut has_sep) = (0usize, false);
    #[allow(clippy::needless_range_loop)] // mirrors the Python loop over component index
    for comp in 0..3 {
        if len - pos < 2 {
            return None;
        }
        comps[comp] = digits(&t[pos..pos + 2])?;
        pos += 2;
        let next = t.get(pos).copied();
        if comp == 0 {
            has_sep = next == Some(':');
        }
        if next.is_none() || comp >= 2 {
            break;
        }
        if has_sep && next != Some(':') {
            return None;
        }
        if has_sep {
            pos += 1;
        }
    }
    let (mut us, mut frac) = (0u32, false);
    if pos < len {
        if t[pos] != '.' && t[pos] != ',' {
            return None;
        }
        pos += 1;
        let rem = &t[pos..];
        if rem.is_empty() || !rem.iter().all(char::is_ascii_digit) {
            return None;
        }
        let take = rem.len().min(6);
        us = digits(&rem[..take])? * 10u32.pow(6 - take as u32);
        frac = true;
    }
    Some((comps[0], comps[1], comps[2], us, frac))
}

/// Parse like `datetime.fromisoformat`; `None` where Python raises.
pub fn parse_iso(s: &str) -> Option<Parsed> {
    let c: Vec<char> = s.chars().collect();
    let (date, pos) = parse_date(&c)?;
    if pos == c.len() {
        return Some(Parsed {
            naive: date.and_hms_opt(0, 0, 0)?,
            offset_us: None,
        });
    }
    let tstr = &c[pos + 1..]; // any single separator character
    if tstr.len() < 2 {
        return None;
    }
    let tz_pos = ['-', '+', 'Z']
        .iter()
        .find_map(|ch| tstr.iter().position(|x| x == ch));
    let (timestr, tz) = match tz_pos {
        Some(i) => (&tstr[..i], Some(i)),
        None => (tstr, None),
    };
    let mut hms = parse_hms(timestr);
    // Python quirk: with an offset present, ONE extra ASCII character may sit between the time and the
    // offset sign (`12:00:00 +01:00`, `12:00:00.Z`, `12:00x:00`-style typos are still rejected).
    if hms.is_none() && tz.is_some() && timestr.last().is_some_and(char::is_ascii) {
        hms = parse_hms(&timestr[..timestr.len() - 1]).filter(|r| !r.4);
    }
    let (h, mi, se, us, _) = hms?;
    let mut extra_day = false;
    if h == 24 {
        if mi != 0 || se != 0 || us != 0 {
            return None;
        }
        extra_day = true;
    } else if h > 23 {
        return None;
    }
    if mi > 59 || se > 59 {
        return None;
    }
    let mut naive = date.and_hms_micro_opt(if extra_day { 0 } else { h }, mi, se, us)?;
    if extra_day {
        naive = naive.checked_add_signed(Duration::days(1))?;
    }
    let offset_us = match tz {
        None => None,
        Some(i) if tstr[i] == 'Z' => {
            if i != tstr.len() - 1 {
                return None;
            }
            Some(0)
        }
        Some(i) => {
            let (oh, om, os, ous, _) = parse_hms(&tstr[i + 1..])?;
            let mag = (i64::from(oh) * 3600 + i64::from(om) * 60 + i64::from(os)) * MICROS
                + i64::from(ous);
            if mag >= 24 * 3600 * MICROS {
                return None;
            }
            Some(if tstr[i] == '-' { -mag } else { mag })
        }
    };
    Some(Parsed { naive, offset_us })
}

/// Epoch microseconds of an aware datetime.
pub fn aware_epoch_us(p: &Parsed, offset_us: i64) -> i64 {
    let u = p.naive.and_utc();
    u.timestamp() * MICROS + i64::from(u.timestamp_subsec_micros()) - offset_us
}

/// Epoch seconds (whole) of a naive datetime interpreted in the local time zone, like `mktime` with fold=0:
/// an ambiguous time (DST end) takes the earlier instant. Deviation: for a time inside a DST gap Python uses
/// the offset from before the transition; here it is approximated with the offset 3 hours earlier.
pub fn local_naive_secs(naive: &NaiveDateTime) -> Option<i64> {
    match Local.from_local_datetime(naive) {
        chrono::LocalResult::Single(d) => Some(d.timestamp()),
        chrono::LocalResult::Ambiguous(a, _) => Some(a.timestamp()),
        chrono::LocalResult::None => {
            let before = naive.checked_sub_signed(Duration::hours(3))?;
            let off = Local
                .from_local_datetime(&before)
                .earliest()?
                .offset()
                .local_minus_utc();
            Some(naive.and_utc().timestamp() - i64::from(off))
        }
    }
}

/// core.iso_ts: `datetime.fromisoformat(iso).timestamp()`; `None` when invalid. A string without offset is
/// local time (as in Python).
pub fn iso_ts(iso: &str) -> Option<f64> {
    let p = parse_iso(iso)?;
    match p.offset_us {
        Some(off) => Some(aware_epoch_us(&p, off) as f64 / MICROS as f64),
        None => {
            let secs = local_naive_secs(&p.naive)?;
            Some(secs as f64 + f64::from(p.naive.and_utc().timestamp_subsec_micros()) / 1e6)
        }
    }
}
