//! events.log: one line per event, self-trimming, never fails the caller.

use crate::paths::Paths;
use chrono::{DateTime, Local};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};

const MAX_BYTES: u64 = 200_000;
const KEEP_CHARS: usize = 100_000;

/// Python `bytes.decode("utf-8", errors="ignore")`: invalid sequences are dropped.
fn decode_ignore(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        out.push_str(chunk.valid());
    }
    out
}

/// Append `"%Y-%m-%d %H:%M:%S " + msg + "\n"` (local time). If the file is over 200_000 bytes it is first
/// rewritten with its last 100_000 characters. Never panics, never returns an error.
///
/// Deviations: lines end with LF (Python's text mode on Windows wrote CRLF); the trim counts the raw
/// characters of the file (Python read with universal newlines, so a CR of a CRLF pair did not count).
pub fn log(paths: &Paths, msg: &str) {
    let _ = log_at(paths, msg, Local::now());
}

/// [`log`] with an explicit clock, returning the I/O result (for tests).
pub fn log_at(paths: &Paths, msg: &str, now: DateTime<Local>) -> io::Result<()> {
    if fs::metadata(&paths.events).is_ok_and(|m| m.len() > MAX_BYTES) {
        let text = decode_ignore(&fs::read(&paths.events)?);
        let n = text.chars().count();
        let start = text
            .char_indices()
            .nth(n.saturating_sub(KEEP_CHARS))
            .map_or(text.len(), |(i, _)| i);
        fs::write(&paths.events, &text[start..])?;
    }
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.events)?;
    let line = format!("{} {}\n", now.format("%Y-%m-%d %H:%M:%S"), msg);
    f.write_all(line.as_bytes())
}
