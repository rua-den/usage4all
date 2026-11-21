//! Widget renderer: the five cell styles, the edit overlay (frame + handles + rotation), the hover cards.
//! Same geometry and colours as `widget.py`; drawn at 3x on a software canvas and downsampled.

use super::canvas::{Canvas, Img, Rgba};
use super::text::{fit_font, fit_label, put_text, Font};
use cu_core::alias::aliases;
use cu_core::config::Config;
use cu_core::fmt::{ago_text_now, fmt_reset, fmt_when_now, money};
use cu_core::pyval::fixed;
use cu_core::widget::*;
use serde_json::Value;

pub const SS: usize = 3;
pub const ACCENT: [u8; 3] = [250, 178, 131];
const BLUE: [u8; 3] = [97, 175, 239];
const AMBER: [u8; 3] = [245, 167, 66];
const RED: [u8; 3] = [224, 108, 117];

fn rgba(c: [u8; 3], a: u8) -> Rgba {
    [c[0], c[1], c[2], a]
}

fn pf(p: &Value, k: &str) -> f64 {
    p.get(k).and_then(cu_core::pyval::py_num).unwrap_or(0.0)
}

fn pstr<'a>(p: &'a Value, k: &str) -> Option<&'a str> {
    p.get(k).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn truthy_key(p: &Value, k: &str) -> bool {
    p.get(k).is_some_and(cu_core::pyval::truthy)
}

fn bold(px: f32, text: &str, mw: f32) -> Font {
    fit_font(true, px, text, mw)
}

fn reg(px: f32, text: &str, mw: f32) -> Font {
    fit_font(false, px, text, mw)
}

fn capsule(
    cv: &mut Canvas,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    pct: f64,
    col: [u8; 3],
    track: Rgba,
    alpha: u8,
) {
    cv.fill_rrect(x, y, x + w, y + h, h / 2.0, track);
    if pct > 0.0 {
        let fw = h.max(w * pct.min(100.0) as f32 / 100.0);
        cv.fill_rrect(x, y, x + fw, y + h, h / 2.0, rgba(col, alpha));
    }
}

#[allow(clippy::too_many_arguments)]
fn segments(
    cv: &mut Canvas,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    pct: f64,
    col: [u8; 3],
    track: Rgba,
    alpha: u8,
) {
    let n = 10usize;
    let gap = h * 0.28;
    let sw = (w - gap * (n as f32 - 1.0)) / n as f32;
    let filled = if pct <= 0.0 {
        0
    } else {
        1.max((pct.min(100.0) / 100.0 * n as f64).round_ties_even() as usize)
    };
    for k in 0..n {
        let xs = x + k as f32 * (sw + gap);
        let c = if k < filled { rgba(col, alpha) } else { track };
        cv.fill_rrect(xs, y, xs + sw, y + h, sw.min(h) * 0.3, c);
    }
}

fn more_cell(
    cv: &mut Canvas,
    cx: f32,
    cy: f32,
    n: i64,
    fg: [u8; 3],
    dim: [u8; 3],
    px: f32,
    maxw: f32,
) {
    let t = format!("+{n}");
    put_text(
        cv,
        cx,
        cy - 2.0 * px,
        &t,
        bold(13.0 * px, &t, maxw),
        fg,
        "mm",
    );
    put_text(
        cv,
        cx,
        cy + 9.0 * px,
        "khác",
        reg(7.4 * px, "khác", maxw),
        dim,
        "mm",
    );
}

pub struct Scene<'a> {
    pub cfg: &'a Config,
    pub view: &'a Items,
    pub busy: &'a [String],
    pub scale: f64,
    pub hover: i64,
    pub vertical: bool,
    pub bar_thick: f64,
    pub size: f64,
    pub phase: i64,
    pub light: bool,
}

/// Draw the cells. Returns (canvas at SS x, cell dims).
pub fn render(s: &Scene) -> (Canvas, Dims) {
    let style = s
        .cfg
        .get("widget_style")
        .and_then(Value::as_str)
        .unwrap_or("rings");
    if STYLES.contains(&style) && style != "rings" {
        render_cards(s, style)
    } else {
        render_rings(s)
    }
}

fn real_names(view: &Items) -> Vec<String> {
    view.iter()
        .filter(|i| i.0 != MORE)
        .map(|i| i.0.clone())
        .collect()
}

fn lookup<'a>(l: &'a [(String, String)], n: &str) -> &'a str {
    l.iter().find(|(k, _)| k == n).map_or("", |e| e.1.as_str())
}

fn render_rings(s: &Scene) -> (Canvas, Dims) {
    let cfg = s.cfg;
    let (scale, vertical) = (s.scale, s.vertical);
    let u = (s.size / 36.0 * scale) as f32;
    let (dd, pad) = (34.0 * u, 5.0 * u);
    let (sh, ms, rs) = (shows(cfg), metrics(cfg), reset_lines(cfg));
    let ff = font_scale(cfg) as f32;
    let light = s.light;
    let fg: [u8; 3] = if light { [32, 32, 32] } else { [236, 236, 236] };
    let dim: [u8; 3] = if light { [90, 90, 90] } else { [150, 150, 150] };
    let track: Rgba = if light {
        [0, 0, 0, 38]
    } else {
        [255, 255, 255, 46]
    };
    let names = real_names(s.view);
    let labels = aliases(&names, cfg, label_max("rings"));
    let below = sh.label && !ms.is_empty() && labels.iter().any(|(_, v)| v.chars().count() > 3);
    let nlines = rs.len() + usize::from(below);
    let lbl = if below { 10.5 * u } else { 0.0 };
    let dims = cell_dims(
        cfg,
        &names,
        s.view.len(),
        scale,
        vertical,
        s.bar_thick,
        s.size,
    );
    let (cw, ch) = (dims.cw as f32, dims.ch as f32);
    let mut cv = Canvas::new(dims.w.max(1) as usize * SS, dims.h.max(1) as usize * SS);
    let ss = SS as f32;
    let s_ = u * ss;
    let fa = |t: &str| bold(7.5 * s_ * ff, t, 17.5 * s_);
    let fb = |t: &str, mw: f32| fit_label(true, 7.8 * s_ * ff, t, mw * s_);
    let fl = |t: &str, mw: f32| reg((if vertical { 8.0 } else { 9.0 }) * s_ * ff, t, mw * s_);
    for (i, (name, p)) in s.view.iter().enumerate() {
        let (x0, y0) = if vertical {
            (0.0, i as f32 * ch * ss)
        } else {
            (i as f32 * cw * ss, 0.0)
        };
        let hov = s.hover == i as i64;
        cv.fill_rrect(
            x0 + 1.0,
            y0 + 1.0,
            x0 + cw * ss - 2.0,
            y0 + ch * ss - 2.0,
            (8.0 * u * ss).floor(),
            cell_fill(cfg, light, hov),
        );
        let cx = x0 + (if vertical { cw / 2.0 } else { pad + dd / 2.0 }) * ss;
        let cy = y0 + (if vertical { pad + dd / 2.0 } else { ch / 2.0 }) * ss;
        if let Some(n) = p.get("more").and_then(Value::as_i64) {
            cv.ring(cx, cy, 17.0 * s_, 3.6 * s_, track);
            more_cell(&mut cv, cx, cy, n, fg, dim, s_ * ff * 0.9, 24.0 * s_);
            continue;
        }
        let ok = p.get("five").is_some();
        let stale = truthy_key(p, "err");
        let a: u8 = if stale { 120 } else { 255 };
        let specs: Vec<(f32, f32, Option<usize>)> = if ms.len() != 1 {
            vec![
                (17.0, 3.6, (!ms.is_empty()).then_some(0)),
                (11.6, 3.6, (ms.len() > 1).then_some(1)),
            ]
        } else {
            vec![(15.5, 5.2, Some(0))]
        };
        for (r_out, th, m) in specs {
            let (r, t) = (r_out * s_, th * s_);
            cv.ring(cx, cy, r, t, track);
            if let (true, Some(m)) = (ok, m) {
                let raw = pf(p, ms[m].1);
                let v = raw.min(100.0);
                let col = rgba(pct_color(raw, cfg), a);
                if v >= 99.5 {
                    cv.ring(cx, cy, r, t, col);
                } else if v > 0.0 {
                    cv.arc(cx, cy, r, t, -90.0, -90.0 + 360.0 * v as f32 / 100.0, col);
                }
            }
        }
        if s.busy.iter().any(|b| b == name) {
            let ph = s.phase as f32;
            cv.arc(
                cx,
                cy,
                20.2 * u * ss,
                2.4 * u * ss,
                ph * 30.0,
                ph * 30.0 + 90.0,
                rgba(BLUE, 255),
            );
        }
        let label = lookup(&labels, name).to_string();
        let inside = if !ok {
            "!".to_string()
        } else if sh.label && !below {
            label.clone()
        } else if !ms.is_empty() {
            fixed(pf(p, ms[0].1), 0)
        } else {
            label.clone()
        };
        put_text(
            &mut cv,
            cx,
            cy,
            &inside,
            fa(&inside),
            if ok { fg } else { RED },
            "mm",
        );
        if stale && ok {
            cv.fill_circle(
                cx + 12.0 * u * ss,
                cy - 14.0 * u * ss,
                3.0 * u * ss,
                rgba(AMBER, 255),
            );
        }
        let tx = x0 + (pad + dd + 6.0 * u) * ss;
        let ys: Vec<f32> = (0..nlines)
            .map(|j| cy + (j as f32 - (nlines as f32 - 1.0) / 2.0) * 10.5 * u * ss)
            .collect();
        if below && ok {
            let (f, t) = fb(&label, if vertical { cw / u - 4.0 } else { 46.0 });
            if vertical {
                put_text(&mut cv, cx, cy + (dd / 2.0 + 7.0 * u) * ss, &t, f, fg, "mm");
            } else {
                put_text(&mut cv, tx, ys[0], &t, f, fg, "lm");
            }
        }
        if !ok {
            let msg = if p.get("status").and_then(Value::as_i64) == Some(429) {
                "chờ"
            } else {
                "lỗi"
            };
            let (px, py) = if vertical {
                (cx, cy + (dd / 2.0 + 9.0 * u + lbl) * ss)
            } else {
                (tx, cy)
            };
            if !rs.is_empty() || !vertical {
                put_text(
                    &mut cv,
                    px,
                    py,
                    msg,
                    fl(msg, 30.0),
                    RED,
                    if vertical { "mm" } else { "lm" },
                );
            }
            continue;
        }
        for (ln, (lab, rk)) in rs.iter().enumerate() {
            let t = fmt_reset(pstr(p, rk));
            if vertical {
                let y = cy + (dd / 2.0 + 8.0 * u + lbl + ln as f32 * 10.5 * u) * ss;
                let txt = format!("{lab} {t}");
                put_text(
                    &mut cv,
                    cx,
                    y,
                    &txt,
                    fl(&txt, cw / u - 4.0),
                    if ln == 0 { fg } else { dim },
                    "mm",
                );
            } else {
                let y = ys[ln + usize::from(below)];
                put_text(&mut cv, tx, y, lab, fl(lab, 11.0), dim, "lm");
                put_text(&mut cv, tx + 12.0 * u * ss, y, &t, fl(&t, 34.0), fg, "lm");
            }
        }
    }
    (cv, dims)
}

fn render_cards(s: &Scene, style: &str) -> (Canvas, Dims) {
    let cfg = s.cfg;
    let (scale, vertical) = (s.scale, s.vertical);
    let u = (s.size / 36.0 * scale) as f32;
    let (sh, ms, rs) = (shows(cfg), metrics(cfg), reset_lines(cfg));
    let ff = font_scale(cfg) as f32;
    let light = s.light;
    let fg: [u8; 3] = if light { [32, 32, 32] } else { [238, 238, 238] };
    let dim: [u8; 3] = if light { [95, 95, 95] } else { [145, 145, 145] };
    let track: Rgba = if light {
        [0, 0, 0, 42]
    } else {
        [255, 255, 255, 38]
    };
    let nm = ms.len();
    const CW: f32 = 44.0;
    let names = real_names(s.view);
    let lab = aliases(&names, cfg, label_max(style));
    let base_h: f32 = match style {
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
    let res = 10.0 * rs.len() as f32;
    let dims = cell_dims(
        cfg,
        &names,
        s.view.len(),
        scale,
        vertical,
        s.bar_thick,
        s.size,
    );
    let (cw, ch) = (dims.cw as f32, dims.ch as f32);
    let mut cv = Canvas::new(dims.w.max(1) as usize * SS, dims.h.max(1) as usize * SS);
    let ss = SS as f32;
    let k = u * ss;
    let bd = |t: &str, px: f32, mw: f32| bold(px * k * ff, t, mw * k);
    let rg = |t: &str, px: f32, mw: f32| reg(px * k * ff, t, mw * k);
    let bl = |t: &str, px: f32, mw: f32| fit_label(true, px * k * ff, t, mw * k);
    for (i, (name, p)) in s.view.iter().enumerate() {
        let (x0, y0) = if vertical {
            (0.0, i as f32 * ch * ss)
        } else {
            (i as f32 * cw * ss, 0.0)
        };
        let ox = x0 + (cw / 2.0 - CW * u / 2.0) * ss;
        let oy = y0 + 2.0 * k;
        let pt = |x: f32, y: f32| (ox + x * k, oy + y * k);
        let ok = p.get("five").is_some();
        let stale = truthy_key(p, "err");
        let a: u8 = if stale { 125 } else { 255 };
        cv.fill_rrect(
            x0 + 1.0,
            y0 + 1.0,
            x0 + cw * ss - 2.0,
            y0 + ch * ss - 2.0,
            (8.0 * k).floor(),
            cell_fill(cfg, light, s.hover == i as i64),
        );
        if let Some(n) = p.get("more").and_then(Value::as_i64) {
            more_cell(
                &mut cv,
                ox + CW * k / 2.0,
                oy + (base_h + res) * k / 2.0,
                n,
                fg,
                dim,
                k * ff,
                CW * k * 0.9,
            );
            continue;
        }
        let alias = lookup(&lab, name).to_string();
        if !ok {
            let msg = if p.get("status").and_then(Value::as_i64) == Some(429) {
                "chờ"
            } else {
                "lỗi"
            };
            let (fa, al) = bl(&alias, 7.8, CW - 6.0);
            let (x, y) = pt(CW / 2.0, 11.0);
            put_text(
                &mut cv,
                x,
                y,
                if sh.label { &al } else { "" },
                fa,
                dim,
                "mm",
            );
            let (x, y) = pt(CW / 2.0, 24.0);
            put_text(&mut cv, x, y, msg, rg(msg, 7.6, CW - 6.0), RED, "mm");
            continue;
        }
        let vals: Vec<(&str, f64, [u8; 3], String)> = ms
            .iter()
            .map(|(tag, key, _)| {
                let v = pf(p, key);
                (*tag, v, pct_color(v, cfg), format!("{}%", fixed(v, 0)))
            })
            .collect();
        if vals.is_empty() {
            let (fa, al) = bl(&alias, 9.5, CW - 8.0);
            let (x, y) = pt(CW / 2.0, base_h / 2.0 + 1.0);
            put_text(&mut cv, x, y, &al, fa, fg, "mm");
        } else if style == "cards" {
            let (tag, _v, col, t) = (&vals[0].0, vals[0].1, vals[0].2, &vals[0].3);
            let (cx0, cy0) = pt(0.0, 0.0);
            let fill: Rgba = if light {
                [0, 0, 0, 10]
            } else {
                [255, 255, 255, 14]
            };
            let edge: Rgba = if light {
                [0, 0, 0, 38]
            } else {
                [255, 255, 255, 34]
            };
            cv.fill_rrect(
                cx0,
                cy0,
                cx0 + CW * k,
                cy0 + (base_h - 1.0) * k,
                (7.0 * k).floor(),
                fill,
            );
            cv.stroke_rrect(
                cx0,
                cy0,
                cx0 + CW * k,
                cy0 + (base_h - 1.0) * k,
                (7.0 * k).floor(),
                (0.8 * k).floor().max(1.0),
                edge,
            );
            cv.fill_rrect(
                cx0 + 2.5 * k,
                cy0 + 6.0 * k,
                cx0 + 5.0 * k,
                cy0 + (base_h - 7.0) * k,
                (1.2 * k).floor(),
                rgba(col, a),
            );
            if sh.label {
                let (fa, al) = bl(&alias, 7.8, 21.0);
                let (x, y) = pt(10.0, 9.0);
                put_text(&mut cv, x, y, &al, fa, fg, "lm");
            }
            let (x, y) = pt(CW - 4.0, 9.0);
            put_text(&mut cv, x, y, tag, rg(tag, 7.6, 8.0), dim, "rm");
            let (x, y) = pt(CW / 2.0 + 3.0, if nm == 2 { 23.0 } else { 24.0 });
            put_text(
                &mut cv,
                x,
                y,
                t,
                bd(t, 15.0, CW - 16.0),
                if stale { dim } else { col },
                "mm",
            );
            if nm == 2 {
                let (tag2, v2, col2, t2) = (&vals[1].0, vals[1].1, vals[1].2, &vals[1].3);
                let (x, y) = pt(10.0, 33.0);
                capsule(&mut cv, x, y, (CW - 16.0) * k, 3.0 * k, v2, col2, track, a);
                let txt = format!("{tag2} {t2}");
                let (x, y) = pt(CW / 2.0 + 3.0, 41.5);
                put_text(
                    &mut cv,
                    x,
                    y,
                    &txt,
                    rg(&txt, 7.6, CW - 12.0),
                    if stale { dim } else { col2 },
                    "mm",
                );
            }
        } else if style == "arc" {
            let (cx, cy) = pt(CW / 2.0, if sh.label { 25.0 } else { 22.0 });
            let specs: Vec<(f32, f32, usize)> = if nm == 2 {
                vec![(17.5, 4.0, 0), (11.5, 2.6, 1)]
            } else {
                vec![(16.0, 5.0, 0)]
            };
            for (r, th, vi) in specs {
                let (rr, tt) = (r * k, th * k);
                let (_, v, col, _) = (&vals[vi].0, vals[vi].1, vals[vi].2, &vals[vi].3);
                cv.arc(cx, cy, rr, tt.floor(), 180.0, 360.0, track);
                if v > 0.0 {
                    cv.arc(
                        cx,
                        cy,
                        rr,
                        tt.floor(),
                        180.0,
                        180.0 + 180.0 * v.min(100.0) as f32 / 100.0,
                        rgba(col, a),
                    );
                }
            }
            let num = &vals[0].3[..vals[0].3.len() - 1];
            put_text(
                &mut cv,
                cx,
                cy - 3.2 * k,
                num,
                bd(num, 11.0, 17.0),
                fg,
                "mm",
            );
            if sh.label {
                let (fa, al) = bl(&alias, 7.8, CW - 2.0);
                put_text(&mut cv, cx, cy + 8.4 * k, &al, fa, dim, "mm");
            } else if nm == 1 {
                let tag = vals[0].0;
                put_text(
                    &mut cv,
                    cx,
                    cy + 6.0 * k,
                    tag,
                    rg(tag, 7.6, 12.0),
                    dim,
                    "mm",
                );
            }
        } else {
            let is_bars = style == "bars";
            let (tag, v, col, t) = (vals[0].0, vals[0].1, vals[0].2, &vals[0].3);
            let right = if tag == "5h" || !sh.label {
                t.clone()
            } else {
                format!("{tag} {t}")
            };
            let fp = bd(&right, 7.8, if right != *t { 22.0 } else { 16.0 });
            let (x, y) = pt(CW - 5.0, 9.0);
            put_text(
                &mut cv,
                x,
                y,
                &right,
                fp,
                if stale { dim } else { col },
                "rm",
            );
            let lead = if sh.label {
                alias.clone()
            } else {
                tag.to_string()
            };
            let maxw = 8.0f32.max(CW - 10.0 - super::text::measure(fp, &right) / k - 2.5);
            let (fa, al) = bl(&lead, 7.8, maxw);
            let (x, y) = pt(5.0, 9.0);
            put_text(
                &mut cv,
                x,
                y,
                &al,
                fa,
                if sh.label { fg } else { dim },
                "lm",
            );
            let (x, y) = pt(5.0, 14.5);
            let (bw, bh) = ((CW - 10.0) * k, (if is_bars { 5.0 } else { 6.0 }) * k);
            if is_bars {
                capsule(&mut cv, x, y, bw, bh, v, col, track, a);
            } else {
                segments(&mut cv, x, y, bw, bh, v, col, track, a);
            }
            if nm == 2 {
                let (tag2, v2, col2, t2) = (vals[1].0, vals[1].1, vals[1].2, &vals[1].3);
                let (x, y) = pt(5.0, 26.5);
                put_text(&mut cv, x, y, tag2, rg(tag2, 7.6, 10.0), dim, "lm");
                let (x, y) = pt(CW - 5.0, 26.5);
                put_text(
                    &mut cv,
                    x,
                    y,
                    t2,
                    rg(t2, 7.6, 16.0),
                    if stale { dim } else { col2 },
                    "rm",
                );
                let (x, y) = pt(5.0, 31.0);
                let (bw, bh) = ((CW - 10.0) * k, (if is_bars { 3.4 } else { 4.0 }) * k);
                if is_bars {
                    capsule(&mut cv, x, y, bw, bh, v2, col2, track, a);
                } else {
                    segments(&mut cv, x, y, bw, bh, v2, col2, track, a);
                }
            }
        }
        let (mx, my) = pt(CW - 4.2, 3.6);
        if s.busy.iter().any(|b| b == name) {
            let r = 3.3 * k;
            cv.ring(mx, my, r, (1.1 * k).floor().max(1.0), rgba(BLUE, 70));
            let ph = s.phase as f32;
            cv.arc(
                mx,
                my,
                r,
                (1.8 * k).floor().max(2.0),
                ph * 30.0,
                ph * 30.0 + 110.0,
                rgba(BLUE, 255),
            );
        } else if stale {
            cv.fill_circle(mx, my, 2.2 * k, rgba(AMBER, 255));
        }
        for (ln, (lb, rk)) in rs.iter().enumerate() {
            let txt = format!("{lb} {}", fmt_reset(pstr(p, rk)));
            let (x, y) = pt(CW / 2.0, base_h + 5.0 + ln as f32 * 9.6);
            put_text(
                &mut cv,
                x,
                y,
                &txt,
                rg(&txt, 7.6, CW - 2.0),
                if ln == 0 { fg } else { dim },
                "mm",
            );
        }
    }
    (cv, dims)
}

/// Frame geometry of the padded (unrotated) image, for hit-testing.
#[derive(Clone, Copy, Debug, Default)]
pub struct Geo {
    pub m: f64,
    pub mt: f64,
    pub w: f64,
    pub h: f64,
    pub wp: f64,
    pub hp: f64,
    pub rot: (f64, f64),
    pub size: (f64, f64),
    pub lock: (f64, f64),
}

/// Add the edit overlay (frame + handles), rotate, downscale.
pub fn compose(cv: &Canvas, w: i64, h: i64, scale: f64, edit: bool, angle: f64) -> (Img, Geo) {
    let (m, mt) = if edit {
        ((22.0 * scale) as i64, (42.0 * scale) as i64)
    } else {
        (0, 0)
    };
    let (wp, hp) = (w + 2 * m, h + m + mt);
    let mut canvas = Canvas::new(wp.max(1) as usize * SS, hp.max(1) as usize * SS);
    canvas.paste(cv, m as usize * SS, mt as usize * SS);
    let (mf, mtf, wf, hf) = (m as f64, mt as f64, w as f64, h as f64);
    let geo = Geo {
        m: mf,
        mt: mtf,
        w: wf,
        h: hf,
        wp: wp as f64,
        hp: hp as f64,
        rot: (mf + wf / 2.0, mtf - 26.0 * scale),
        size: (mf + wf, mtf + hf),
        lock: (mf + 11.0 * scale, mtf + hf + 13.0 * scale),
    };
    if edit {
        let s = (scale * SS as f64) as f32;
        let ss = SS as f64;
        let (x0, y0, x1, y1) = (
            (mf * ss) as f32,
            (mtf * ss) as f32,
            ((mf + wf) * ss) as f32,
            ((mtf + hf) * ss) as f32,
        );
        let lw = 2.0f32.max((1.6 * s).floor());
        let ac = |a| rgba(ACCENT, a);
        let dark: Rgba = [20, 20, 20, 255];
        canvas.fill_rrect(
            x0 - 3.0 * s,
            y0 - 3.0 * s,
            x1 + 3.0 * s,
            y1 + 3.0 * s,
            (10.0 * s).floor(),
            ac(22),
        );
        canvas.stroke_rrect(
            x0 - 3.0 * s,
            y0 - 3.0 * s,
            x1 + 3.0 * s,
            y1 + 3.0 * s,
            (10.0 * s).floor(),
            lw,
            ac(210),
        );
        let (rx, ry) = ((geo.rot.0 * ss) as f32, (geo.rot.1 * ss) as f32);
        canvas.line(rx, ry, rx, y0 - 3.0 * s, lw, ac(210));
        canvas.fill_circle(rx, ry, 7.0 * s, ac(255));
        canvas.ring(rx, ry, 7.0 * s, s.floor().max(1.0), dark);
        canvas.arc(
            rx,
            ry,
            3.2 * s,
            (1.5 * s).floor().max(1.0),
            30.0,
            300.0,
            dark,
        );
        let (sx, sy) = ((geo.size.0 * ss) as f32, (geo.size.1 * ss) as f32);
        canvas.fill_rect(
            sx - 6.0 * s,
            sy - 6.0 * s,
            sx + 6.0 * s,
            sy + 6.0 * s,
            ac(255),
        );
        canvas.stroke_rrect(
            sx - 6.0 * s,
            sy - 6.0 * s,
            sx + 6.0 * s,
            sy + 6.0 * s,
            0.0,
            s.floor().max(1.0),
            dark,
        );
        let (lx, ly) = ((geo.lock.0 * ss) as f32, (geo.lock.1 * ss) as f32);
        canvas.fill_circle(lx, ly, 10.0 * s, [24, 24, 24, 245]);
        canvas.ring(lx, ly, 10.0 * s, lw, ac(255));
        canvas.fill_rect(
            lx - 4.2 * s,
            ly - 0.8 * s,
            lx + 4.2 * s,
            ly + 4.6 * s,
            ac(255),
        );
        canvas.arc(
            lx,
            ly - 2.0 * s,
            3.2 * s,
            (1.4 * s).floor().max(1.0),
            180.0,
            360.0,
            ac(255),
        );
    }
    (canvas.finish(SS, angle), geo)
}

// ---------------------------------------------------------------------------------------------------------
// hover cards

struct TipStyle {
    fg: [u8; 3],
    dim: [u8; 3],
    card: Rgba,
    edge: Rgba,
    track: Rgba,
    fs: f32,
}

fn tip_style(cfg: &Config, light: bool) -> TipStyle {
    TipStyle {
        fg: if light { [28, 28, 30] } else { [238, 238, 238] },
        dim: if light {
            [100, 100, 105]
        } else {
            [150, 150, 155]
        },
        card: if light {
            [250, 250, 251, 248]
        } else {
            [24, 24, 27, 246]
        },
        edge: if light {
            [0, 0, 0, 50]
        } else {
            [255, 255, 255, 46]
        },
        track: if light {
            [0, 0, 0, 40]
        } else {
            [255, 255, 255, 40]
        },
        fs: font_scale(cfg).max(1.0) as f32,
    }
}

const TIP_W: f32 = 250.0;
const PAD: f32 = 12.0;

fn tip_canvas(w: f32, h: f32, scale: f64, st: &TipStyle) -> Canvas {
    let (pw, ph) = (
        (w as f64 * scale).round().max(1.0) as usize,
        (h as f64 * scale).round().max(1.0) as usize,
    );
    let mut cv = Canvas::new(pw * SS, ph * SS);
    let k = scale as f32 * SS as f32;
    cv.fill_rrect(
        1.0,
        1.0,
        w * k - 2.0,
        h * k - 2.0,
        (9.0 * k).floor(),
        st.card,
    );
    cv.stroke_rrect(
        1.0,
        1.0,
        w * k - 2.0,
        h * k - 2.0,
        (9.0 * k).floor(),
        k.floor().max(1.0),
        st.edge,
    );
    cv
}

fn tfont(bold: bool, px: f32, k: f32, fs: f32) -> Font {
    Font {
        bold,
        px: ((px * k * fs) as i32).max(6),
    }
}

fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max - 1).collect::<String>() + "…"
    }
}

/// "+N more" hover card.
pub fn render_more_tip(hidden: &Items, cfg: &Config, scale: f64, light: bool) -> Img {
    let st = tip_style(cfg, light);
    let (row, shown) = (15.0f32, hidden.len().min(18));
    let h =
        PAD + 18.0 + row * shown as f32 + if hidden.len() > shown { row } else { 0.0 } + PAD - 2.0;
    let mut cv = tip_canvas(TIP_W, h, scale, &st);
    let k = scale as f32 * SS as f32;
    let t = |cv: &mut Canvas, x: f32, y: f32, s: &str, f: Font, c: [u8; 3], a: &str| {
        put_text(cv, x * k, y * k, s, f, c, a)
    };
    let mut y = PAD + 6.0;
    let title = format!("{} account nữa (không đủ chỗ)", hidden.len());
    t(
        &mut cv,
        PAD,
        y,
        &title,
        tfont(true, 10.5, k, st.fs),
        st.fg,
        "lm",
    );
    y += 18.0;
    for (name, p) in hidden.iter().take(shown) {
        t(
            &mut cv,
            PAD,
            y,
            &trunc(name, 19),
            tfont(false, 9.0, k, st.fs),
            st.dim,
            "lm",
        );
        if p.get("five").is_some() {
            let (f, w) = (pf(p, "five"), pf(p, "week"));
            let txt = format!("5h {}% · 7d {}%", fixed(f, 0), fixed(w, 0));
            t(
                &mut cv,
                TIP_W - PAD,
                y,
                &txt,
                tfont(true, 9.0, k, st.fs),
                pct_color(f.max(w), cfg),
                "rm",
            );
        } else {
            t(
                &mut cv,
                TIP_W - PAD,
                y,
                "chưa có dữ liệu",
                tfont(false, 8.6, k, st.fs),
                st.dim,
                "rm",
            );
        }
        y += row;
    }
    if hidden.len() > shown {
        let txt = format!("… và {} account nữa", hidden.len() - shown);
        t(
            &mut cv,
            PAD,
            y,
            &txt,
            tfont(false, 8.6, k, st.fs),
            st.dim,
            "lm",
        );
    }
    cv.finish(SS, 0.0)
}

/// Per-account detail card. The body is drawn twice: once on a 1x1 canvas to learn the height, then for real.
pub fn render_tip(name: &str, p: &Value, cfg: &Config, scale: f64, light: bool) -> Img {
    let st = tip_style(cfg, light);
    let mut dry = Canvas::new(1, 1);
    let h = tip_body(&mut dry, name, p, cfg, scale, &st);
    let mut cv = tip_canvas(TIP_W, h, scale, &st);
    tip_body(&mut cv, name, p, cfg, scale, &st);
    cv.finish(SS, 0.0)
}

fn tip_body(
    cv: &mut Canvas,
    name: &str,
    p: &Value,
    cfg: &Config,
    scale: f64,
    st: &TipStyle,
) -> f32 {
    let k = scale as f32 * SS as f32;
    let fs = st.fs;
    let (fg, dim) = (st.fg, st.dim);
    let mut y = PAD + 6.0;
    macro_rules! t {
        ($x:expr, $y:expr, $s:expr, $f:expr, $c:expr, $a:expr) => {
            put_text(cv, $x * k, $y * k, $s, $f, $c, $a)
        };
    }
    t!(PAD, y, &trunc(name, 24), tfont(true, 11.5, k, fs), fg, "lm");
    if let Some(plan) = p.get("plan").filter(|v| cu_core::pyval::truthy(v)) {
        let s = plan
            .as_str()
            .map_or_else(|| plan.to_string(), str::to_string);
        t!(TIP_W - PAD, y, &s, tfont(false, 8.5, k, fs), dim, "rm");
    }
    y += 18.0;
    if p.get("five").is_some() {
        for (label, key, rk) in [
            ("5 giờ", "five", "five_reset"),
            ("7 ngày", "week", "week_reset"),
        ] {
            let v = pf(p, key);
            let col = pct_color(v, cfg);
            t!(PAD, y, label, tfont(false, 9.0, k, fs), dim, "lm");
            t!(
                TIP_W - PAD,
                y,
                &format!("{}%", fixed(v, 0)),
                tfont(true, 10.5, k, fs),
                col,
                "rm"
            );
            y += 10.0;
            capsule(
                cv,
                PAD * k,
                y * k,
                (TIP_W - 2.0 * PAD) * k,
                5.0 * k,
                v,
                col,
                st.track,
                255,
            );
            y += 11.0;
            let rs = pstr(p, rk);
            let line = match rs {
                Some(r) => format!("reset sau {} · {}", fmt_reset(Some(r)), fmt_when_now(r)),
                None => "chưa có mốc reset".to_string(),
            };
            t!(PAD, y, &line, tfont(false, 8.4, k, fs), dim, "lm");
            y += 15.0;
        }
        if let Some(Value::Array(sc)) = p.get("scoped") {
            for s in sc {
                let v = pf(s, "pct");
                let col = pct_color(v, cfg);
                let nm = s.get("name").and_then(Value::as_str).unwrap_or("");
                t!(
                    PAD,
                    y,
                    &format!("{nm} (riêng, 7 ngày)"),
                    tfont(false, 9.0, k, fs),
                    dim,
                    "lm"
                );
                t!(
                    TIP_W - PAD,
                    y,
                    &format!("{}%", fixed(v, 0)),
                    tfont(true, 10.0, k, fs),
                    col,
                    "rm"
                );
                y += 10.0;
                capsule(
                    cv,
                    PAD * k,
                    y * k,
                    (TIP_W - 2.0 * PAD) * k,
                    3.6 * k,
                    v,
                    col,
                    st.track,
                    255,
                );
                y += 12.0;
            }
        }
        match p.get("extra").filter(|v| cu_core::pyval::truthy(v)) {
            Some(ex) => {
                let v = pf(ex, "pct");
                let col = pct_color(v, cfg);
                let cur = ex.get("currency").and_then(Value::as_str).unwrap_or("");
                t!(
                    PAD,
                    y,
                    "Credit dùng thêm (tháng)",
                    tfont(false, 9.0, k, fs),
                    dim,
                    "lm"
                );
                let amt = format!(
                    "{} / {}",
                    money(pf(ex, "used"), cur),
                    money(pf(ex, "limit"), cur)
                );
                t!(TIP_W - PAD, y, &amt, tfont(true, 9.6, k, fs), fg, "rm");
                y += 10.0;
                capsule(
                    cv,
                    PAD * k,
                    y * k,
                    (TIP_W - 2.0 * PAD) * k,
                    3.6 * k,
                    v,
                    col,
                    st.track,
                    255,
                );
                y += 10.0;
                t!(
                    PAD,
                    y,
                    &format!("đã dùng {}% hạn mức credit", fixed(v, 1)),
                    tfont(false, 8.4, k, fs),
                    dim,
                    "lm"
                );
                y += 13.0;
            }
            None => {
                t!(
                    PAD,
                    y,
                    "Credit dùng thêm: chưa bật",
                    tfont(false, 8.6, k, fs),
                    dim,
                    "lm"
                );
                y += 13.0;
            }
        }
    } else {
        let e = pstr(p, "err").unwrap_or("chưa có dữ liệu");
        t!(PAD, y, e, tfont(false, 9.4, k, fs), RED, "lm");
        y += 15.0;
    }
    let mut foot: Option<(String, [u8; 3])> = None;
    if truthy_key(p, "err") && p.get("five").is_some() {
        let wait = match p
            .get("retry_at")
            .and_then(cu_core::pyval::py_num)
            .filter(|v| *v != 0.0)
        {
            Some(r) => format!(
                " · thử lại sau {} phút",
                fixed((r - cu_core::cache::now_ts()).max(0.0) / 60.0, 0)
            ),
            None => String::new(),
        };
        let fetched = p.get("fetched_at").and_then(cu_core::pyval::py_num);
        foot = Some((
            format!(
                "{}: đang hiện dữ liệu cũ ({}){wait}",
                pstr(p, "err").unwrap_or(""),
                ago_text_now(fetched)
            ),
            AMBER,
        ));
    } else if let Some(f) = p
        .get("fetched_at")
        .and_then(cu_core::pyval::py_num)
        .filter(|v| *v != 0.0)
    {
        foot = Some((format!("Cập nhật {}", ago_text_now(Some(f))), dim));
    }
    if let Some((txt, col)) = foot {
        y += 2.0;
        cv.line(
            PAD * k,
            y * k,
            (TIP_W - PAD) * k,
            y * k,
            k.floor().max(1.0) * 0.8,
            st.edge,
        );
        y += 9.0;
        t!(PAD, y, &txt, tfont(false, 8.2, k, fs), col, "lm");
        y += 12.0;
    }
    y + PAD - 4.0
}
