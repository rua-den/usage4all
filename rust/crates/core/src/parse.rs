//! Pure JSON transforms of the usage API response: core.parse_extra / core.parse_scoped.

use crate::pyval::{py_int, py_num, truthy};
use serde_json::{Map, Value};

/// core.parse_extra: the extra-usage ("credit") block -> `{used, limit, pct, currency}` in major units, or
/// `None` when not enabled or on any error (Python: `except Exception: return None`).
///
/// Deviation: `decimal_places` outside -300..=300 gives `None` (Python big-int arithmetic would still work
/// or overflow depending on the operands).
pub fn parse_extra(ex: &Value) -> Option<Value> {
    if !truthy(ex) {
        return None;
    }
    let ex = ex.as_object()?; // `.get` on a truthy non-dict raises AttributeError -> None
    if !ex.get("is_enabled").is_some_and(truthy) {
        return None;
    }
    let limit_raw = ex.get("monthly_limit").filter(|v| !v.is_null())?;
    let dp = match ex.get("decimal_places") {
        Some(v) if !v.is_null() => py_int(v)?,
        _ => 2,
    };
    if !(-300..=300).contains(&dp) {
        return None;
    }
    let div = 10f64.powf(dp as f64);
    let used_raw = ex.get("used_credits").filter(|v| truthy(v));
    let used = match used_raw {
        Some(v) => py_num(v)? / div,
        None => 0.0 / div,
    };
    let limit = py_num(limit_raw)? / div;
    let pct = match ex.get("utilization").filter(|v| !v.is_null()) {
        Some(p) => p.clone(),
        None if limit != 0.0 => Value::from(used / limit * 100.0),
        None => Value::from(0),
    };
    let currency = ex
        .get("currency")
        .filter(|v| truthy(v))
        .cloned()
        .unwrap_or_else(|| Value::from("USD"));
    let mut out = Map::new();
    out.insert("used".into(), Value::from(used));
    out.insert("limit".into(), Value::from(limit));
    out.insert("pct".into(), pct);
    out.insert("currency".into(), currency);
    Some(Value::Object(out))
}

/// core.parse_scoped: per-model weekly limits -> `[{name, pct, resets_at}]`. An item that is not a dict (or
/// whose fields have the wrong shape) is skipped, as Python's per-item `except Exception: pass`.
/// Deviation: if `limits` is a truthy non-iterable (number, bool) Python raises TypeError; here -> empty.
pub fn parse_scoped(limits: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    let Value::Array(items) = limits else {
        return out; // falsy -> `or []`; dict/str iterate to non-dict items (skipped); numbers raise in Python
    };
    for l in items {
        let Some(l) = l.as_object() else { continue };
        let Some(scope) = dig(l.get("scope")) else {
            continue;
        };
        let Some(model) = dig(scope.get("model")) else {
            continue;
        };
        let name = model.get("display_name").cloned().unwrap_or(Value::Null);
        if l.get("kind").and_then(Value::as_str) == Some("weekly_scoped") && truthy(&name) {
            let mut o = Map::new();
            o.insert("name".into(), name);
            o.insert(
                "pct".into(),
                l.get("percent")
                    .filter(|v| truthy(v))
                    .cloned()
                    .unwrap_or_else(|| Value::from(0)),
            );
            o.insert(
                "resets_at".into(),
                l.get("resets_at").cloned().unwrap_or(Value::Null),
            );
            out.push(Value::Object(o));
        }
    }
    out
}

/// `(x or {})` followed by `.get(...)`: falsy -> empty map, object -> itself, other truthy -> AttributeError
/// (`None` here, the item is skipped).
fn dig(v: Option<&Value>) -> Option<Map<String, Value>> {
    match v {
        None => Some(Map::new()),
        Some(x) if !truthy(x) => Some(Map::new()),
        Some(Value::Object(m)) => Some(m.clone()),
        Some(_) => None,
    }
}
