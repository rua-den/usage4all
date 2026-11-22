//! Python-semantics helpers over `serde_json::Value` (truthiness, `int()`, numeric coercion).

use serde_json::Value;

/// Python truthiness: None/False/0/0.0/""/[]/{} are falsy.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i != 0
            } else if let Some(u) = n.as_u64() {
                u != 0
            } else {
                n.as_f64().is_some_and(|f| f != 0.0)
            }
        }
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python `int(str)` for the common forms: optional surrounding whitespace, sign, ASCII digits with single
/// underscores between digits. Deviation: values beyond i64 saturate instead of becoming big ints; non-ASCII
/// digits are not accepted.
pub fn parse_py_int_str(s: &str) -> Option<i64> {
    let t = s.trim();
    let (neg, digits) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let b = digits.as_bytes();
    if b.is_empty() || !b[0].is_ascii_digit() || !b[b.len() - 1].is_ascii_digit() {
        return None;
    }
    let mut acc: i128 = 0;
    let mut prev_us = false;
    for &c in b {
        if c == b'_' {
            if prev_us {
                return None;
            }
            prev_us = true;
            continue;
        }
        if !c.is_ascii_digit() {
            return None;
        }
        prev_us = false;
        acc = (acc * 10 + i128::from(c - b'0')).min(i128::from(i64::MAX) + 1);
    }
    let v = if neg { -acc } else { acc };
    Some(v.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64)
}

/// Python `int(x)`: int stays, float truncates toward zero, bool -> 0/1, str -> trimmed integer parse.
/// `None` where Python raises (TypeError/ValueError/OverflowError): null, list, dict, bad string, non-finite.
pub fn py_int(v: &Value) -> Option<i64> {
    match v {
        Value::Bool(b) => Some(i64::from(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i)
            } else if n.as_u64().is_some() {
                Some(i64::MAX) // saturate (Python: big int)
            } else {
                let f = n.as_f64()?;
                f.is_finite().then(|| f.trunc() as i64) // `as` saturates
            }
        }
        Value::String(s) => parse_py_int_str(s),
        _ => None,
    }
}

/// Numeric operand as Python arithmetic sees it: numbers, and bools (bool is an int). `None` = TypeError.
pub fn py_num(v: &Value) -> Option<f64> {
    match v {
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

/// Python float floor division `a // b` (b != 0), ported from CPython's `float_floor_div`.
pub fn py_floordiv(a: f64, b: f64) -> f64 {
    let mut m = a % b;
    let mut div = (a - m) / b;
    if m != 0.0 && ((b < 0.0) != (m < 0.0)) {
        m += b;
        div -= 1.0;
    }
    let _ = m;
    if div != 0.0 {
        let fl = div.floor();
        if div - fl > 0.5 {
            fl + 1.0
        } else {
            fl
        }
    } else {
        0.0_f64.copysign(a / b)
    }
}

/// Python `f"{x:.Nf}"` (round-half-even on the exact binary value). Rust's `{:.N}` formatting is exact and
/// ties-to-even as well (verified by the parity fixtures), so this is a thin wrapper kept in one place.
pub fn fixed(x: f64, prec: usize) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x < 0.0 { "-inf".into() } else { "inf".into() };
    }
    format!("{x:.prec$}")
}
