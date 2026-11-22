//! Pure layout logic of the taskbar widget (ported from `widget.py`): which accounts are shown, in which order,
//! how big a cell is, how many fit (`+N` cell), where the hover card goes. No drawing here; the renderer
//! (rust/app/src/widget) draws on top of these numbers. Parity with Python: `tests/parity_widget.rs`.

use crate::alias::{alias, aliases};
use crate::config::{get_f64, Config};
use crate::pyval::{py_int, py_num, truthy};
use serde_json::{json, Map, Value};

pub const MORE: &str = "__more__";
pub const STYLES: [&str; 5] = ["rings", "bars", "cards", "segments", "arc"];
pub const STYLE_NAMES: [(&str, &str); 5] = [
    ("rings", "Vòng tròn"),
    ("bars", "Thanh ngang"),
    ("cards", "Thẻ số lớn"),
    ("segments", "Ô vuông (terminal)"),
    ("arc", "Đồng hồ bán nguyệt"),
];

/// Longest label shown per style (`LABEL_MAX`).
pub fn label_max(style: &str) -> usize {
    if style == "arc" {
        10
    } else {
        8
    }
}

pub type Items = Vec<(String, Value)>;

fn num(cfg: &Config, key: &str, default: f64, lo: f64, hi: f64) -> f64 {
    match cfg.get(key) {
        None => default,
        Some(v) => match py_num(v) {
            Some(x) if !x.is_nan() => x.max(lo).min(hi),
            _ => default,
        },
    }
}

pub fn font_scale(cfg: &Config) -> f64 {
    num(cfg, "widget_font", 100.0, 80.0, 150.0) / 100.0
}

pub fn opacity_alpha(cfg: &Config) -> u8 {
    (255.0 * num(cfg, "widget_opacity", 100.0, 30.0, 100.0) / 100.0) as u8
}

/// `cfg.get(key, default)` as a Python bool.
pub fn flag(cfg: &Config, key: &str, default: bool) -> bool {
    cfg.get(key).map_or(default, truthy)
}

pub fn pct_color(p: f64, cfg: &Config) -> [u8; 3] {
    let block = get_f64(cfg, "block_pct", 95.0);
    let warn = get_f64(cfg, "warn_pct", 80.0);
    if p >= block {
        [224, 108, 117]
    } else if p >= warn {
        [245, 167, 66]
    } else {
        [127, 216, 143]
    }
}

/// Background of one account cell (RGBA).
pub fn cell_fill(cfg: &Config, light: bool, hovered: bool) -> [u8; 4] {
    let h = i32::from(hovered);
    if flag(cfg, "widget_plate", false) {
        let (r, g, b, a) = if light {
            (248, 248, 248, 214)
        } else {
            (22, 22, 24, 208)
        };
        let d = (if light { -14 } else { 26 }) * h;
        let c = |v: i32| (v + d).clamp(0, 255) as u8;
        return [c(r), c(g), c(b), a];
    }
    match (light, hovered) {
        (true, true) => [0, 0, 0, 20],
        (false, true) => [255, 255, 255, 26],
        _ => [0, 0, 0, 2],
    }
}

fn score(p: &Value) -> f64 {
    match p.get("five") {
        Some(f) => {
            let f = py_num(f).unwrap_or(0.0);
            let w = p.get("week").and_then(py_num).unwrap_or(0.0);
            f.max(w)
        }
        None => -1.0,
    }
}

pub fn sort_profiles(cfg: &Config, items: Items) -> Items {
    if cfg.get("widget_sort").and_then(Value::as_str) != Some("usage") {
        return items;
    }
    let mut v = items;
    v.sort_by(|a, b| score(&b.1).partial_cmp(&score(&a.1)).unwrap()); // stable, like Python sorted
    v
}

pub fn visible_profiles(cfg: &Config, cache: Option<&Value>) -> Items {
    let hidden: Vec<&str> = match cfg.get("widget_hidden") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    let items: Items = match cache.and_then(|c| c.get("profiles")) {
        Some(Value::Object(m)) => m
            .iter()
            .filter(|(n, _)| !hidden.contains(&n.as_str()))
            .map(|(n, p)| (n.clone(), p.clone()))
            .collect(),
        _ => vec![],
    };
    sort_profiles(cfg, items)
}

/// Pixels the widget may take along its stacking axis. `screen` = (width, height).
pub fn stack_limit(cfg: &Config, vertical: bool, docked: bool, screen: (i64, i64)) -> i64 {
    let explicit = cfg.get("widget_max_len").and_then(py_int).unwrap_or(0);
    if explicit > 0 {
        return explicit;
    }
    let span = if vertical { screen.1 } else { screen.0 } as f64;
    (span * if docked { 0.5 } else { 0.9 }) as i64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shows {
    pub label: bool,
    pub s5: bool,
    pub s7: bool,
    pub r5: bool,
    pub r7: bool,
}

pub fn shows(cfg: &Config) -> Shows {
    let master = flag(cfg, "widget_reset", true);
    let mut s = Shows {
        label: flag(cfg, "widget_show_label", true),
        s5: flag(cfg, "widget_show_5h", true),
        s7: flag(cfg, "widget_show_7d", true),
        r5: master && flag(cfg, "widget_show_reset5", true),
        r7: master && flag(cfg, "widget_show_reset7", true),
    };
    if !(s.label || s.s5 || s.s7) {
        s.label = true;
    }
    s
}

/// Visible usage windows in display order: (tag, usage key, reset key).
pub fn metrics(cfg: &Config) -> Vec<(&'static str, &'static str, &'static str)> {
    let s = shows(cfg);
    let mut v = vec![];
    if s.s5 {
        v.push(("5h", "five", "five_reset"));
    }
    if s.s7 {
        v.push(("7d", "week", "week_reset"));
    }
    v
}

pub fn reset_lines(cfg: &Config) -> Vec<(&'static str, &'static str)> {
    let s = shows(cfg);
    let mut v = vec![];
    if s.r5 {
        v.push(("5h", "five_reset"));
    }
    if s.r7 {
        v.push(("7d", "week_reset"));
    }
    v
}

/// Geometry of the cell grid (what `render` returns besides the canvas), in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dims {
    pub w: i64,
    pub h: i64,
    pub cw: f64,
    pub ch: f64,
}

/// Cell size for `names` (display order, `+N` cell excluded) drawn in `style`. Mirrors the size computation at
/// the top of `render` / `render_cards`; `n_cells` includes the `+N` cell.
pub fn cell_dims(
    cfg: &Config,
    names: &[String],
    n_cells: usize,
    scale: f64,
    vertical: bool,
    bar_thick: f64,
    size: f64,
) -> Dims {
    let style = cfg.get("widget_style").and_then(Value::as_str).unwrap_or("rings");
    let sh = shows(cfg);
    let ms = metrics(cfg);
    let rs = reset_lines(cfg);
    let u = size / 36.0 * scale;
    let (cw, ch);
    if STYLES.contains(&style) && style != "rings" {
        let nm = ms.len();
        let base_h: f64 = match style {
            "cards" => [22.0, 36.0, 49.0][nm],
            "arc" => {
                if nm > 0 {
                    if sh.label {
                        40.0
                    } else {
                        31.0
                    }
                } else {
                    22.0
                }
            }
            _ => [18.0, 25.0, 38.0][nm],
        };
        let res = 10.0 * rs.len() as f64;
        const CW: f64 = 44.0;
        cw = if vertical {
            (CW * u).max(bar_thick - 4.0 * scale)
        } else {
            (CW + 4.0) * u
        };
        ch = (base_h + res + 4.0) * u;
    } else {
        let d = 34.0 * u;
        let pad = 5.0 * u;
        let labels = aliases(names, cfg, label_max("rings"));
        let below = sh.label
            && !ms.is_empty()
            && labels.iter().any(|(_, v)| v.chars().count() > 3);
        let nlines = rs.len() + usize::from(below);
        let lbl = if below { 10.5 * u } else { 0.0 };
        if vertical {
            cw = (bar_thick - 4.0 * scale).max(d + 2.0 * pad);
            ch = pad
                + d
                + lbl
                + (if rs.is_empty() {
                    0.0
                } else {
                    rs.len() as f64 * 10.5 * u + 3.0 * u
                })
                + pad;
        } else {
            cw = pad + d + (if nlines > 0 { 6.0 * u + 47.0 * u } else { 0.0 }) + pad;
            ch = d + 8.0 * u;
        }
    }
    let n = n_cells.max(1) as f64;
    let (w, h) = if vertical {
        (cw as i64, (ch * n) as i64)
    } else {
        ((cw * n) as i64, ch as i64)
    };
    Dims { w, h, cw, ch }
}

/// plan_layout: `(size, shown)`. `measure(size)` = length of one cell along the stacking axis; when `None` the
/// default measure (the longest-label account alone) is used.
pub fn plan_layout(
    cfg: &Config,
    items: &Items,
    scale: f64,
    vertical: bool,
    bar_thick: f64,
    max_len: i64,
    measure: Option<&dyn Fn(f64) -> f64>,
) -> (f64, usize) {
    let n = items.len();
    let size = get_f64(cfg, "widget_size", 36.0);
    if n == 0 {
        return (size, 0);
    }
    let style = cfg.get("widget_style").and_then(Value::as_str).unwrap_or("rings");
    let _ = style;
    let default_measure;
    let measure: &dyn Fn(f64) -> f64 = match measure {
        Some(m) => m,
        None => {
            let mut best = &items[0];
            let mut best_len = alias(&best.0, cfg, label_max("rings")).chars().count();
            for it in &items[1..] {
                let l = alias(&it.0, cfg, label_max("rings")).chars().count();
                if l > best_len {
                    best = it;
                    best_len = l;
                }
            }
            let names = vec![best.0.clone()];
            let cfg2 = cfg.clone();
            default_measure = move |sz: f64| {
                let d = cell_dims(&cfg2, &names, 1, scale, vertical, bar_thick, sz);
                if vertical {
                    d.ch
                } else {
                    d.cw
                }
            };
            &default_measure
        }
    };
    let cap = cfg.get("widget_max_cells").and_then(py_int).unwrap_or(0);
    let want = if cap > 0 { n.min(cap as usize) } else { n };
    let need = if want < n { want + 1 } else { n };
    let floor = 20.0_f64.max(size * 0.6);
    let max_len = max_len as f64;
    let mut sz = size;
    if measure(sz) * need as f64 > max_len {
        for _ in 0..3 {
            sz = floor.max(sz * max_len / (measure(sz) * need as f64) * 0.97);
            if sz <= floor + 0.01 || measure(sz) * need as f64 <= max_len {
                break;
            }
        }
    }
    let slots = ((max_len / measure(sz)).floor() as i64).max(1) as usize;
    if need <= slots {
        return (sz, want);
    }
    (sz, 1.max(slots as i64 - 1) as usize)
}

/// The profiles to draw: the first `shown`, plus a `+N` cell for the rest.
pub fn apply_plan(items: &Items, shown: usize) -> Items {
    let mut head: Items = items.iter().take(shown).cloned().collect();
    if shown < items.len() {
        let mut m = Map::new();
        m.insert("more".into(), json!(items.len() - shown));
        head.push((MORE.into(), Value::Object(m)));
    }
    head
}

/// Hover card placement beside the widget, level with the pointer, kept on screen.
/// `win` = (x, y, w, h), `size` = card (w, h), `screen` = (w, h).
pub fn tip_position(
    win: (f64, f64, f64, f64),
    mouse_y: f64,
    size: (f64, f64),
    screen: (f64, f64),
    gap: f64,
) -> (i64, i64) {
    let (wx, wy, ww, _wh) = win;
    let _ = wy;
    let (tw, th) = size;
    let (sw, sh) = screen;
    let x = if wx + ww / 2.0 > sw / 2.0 {
        wx - tw - gap
    } else {
        wx + ww + gap
    };
    let x = gap.max(x.min(sw - tw - gap));
    let y = gap.max((mouse_y - th / 2.0).min(sh - th - gap));
    (x as i64, y as i64)
}

/// Clockwise rotation on screen (y down).
pub fn rot(x: f64, y: f64, ang: f64) -> (f64, f64) {
    let a = ang.to_radians();
    (x * a.cos() - y * a.sin(), x * a.sin() + y * a.cos())
}
