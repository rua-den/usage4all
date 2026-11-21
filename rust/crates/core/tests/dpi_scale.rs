//! DPI behaviour of the widget layout as pure math. Only ONE physical monitor is attached to the dev machine, so
//! "drag the widget to a monitor with another DPI" cannot be done for real; what can be proven here is that the
//! layout scales linearly with the DPI scale and that the budget (screen size in PHYSICAL px) is always respected
//! at 100% … 300% (the Python fixtures cover scales 1.0 and 1.5 against the real Python code).

use cu_core::config::{defaults, Config};
use cu_core::widget::*;
use serde_json::{json, Value};

const SCALES: [f64; 7] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0];

fn cfg(style: &str) -> Config {
    let mut c = defaults();
    c.insert("widget_style".into(), json!(style));
    c
}

fn cache(n: usize) -> Value {
    let mut p = serde_json::Map::new();
    for i in 0..n {
        p.insert(
            format!("claude-profile-{i:02}"),
            json!({"five": (i * 7 % 100) as f64, "week": (i * 11 % 100) as f64, "fetched_at": 1.0}),
        );
    }
    json!({"ts": 1.0, "profiles": p, "busy": []})
}

fn items(c: &Config, n: usize) -> Items {
    visible_profiles(c, Some(&cache(n)))
}

fn names(it: &Items) -> Vec<String> {
    it.iter().map(|i| i.0.clone()).collect()
}

#[test]
fn cell_size_scales_linearly_with_dpi() {
    for style in ["rings", "bars", "cards", "segments", "arc"] {
        let c = cfg(style);
        let it = items(&c, 4);
        let n = names(&it);
        for vertical in [true, false] {
            let base = cell_dims(&c, &n, 4, 1.0, vertical, 40.0, 36.0);
            for s in SCALES {
                let d = cell_dims(&c, &n, 4, s, vertical, 40.0 * s, 36.0);
                // bar thickness scales too (it is the taskbar thickness, in physical px): ratios stay ~ s
                let tol = 0.06 * s * base.cw.max(base.ch) + 2.0;
                assert!(
                    (d.cw - base.cw * s).abs() <= tol && (d.ch - base.ch * s).abs() <= tol,
                    "{style} vertical={vertical} scale={s}: {}x{} vs {}x{} * {s}",
                    d.cw,
                    d.ch,
                    base.cw,
                    base.ch
                );
            }
        }
    }
}

#[test]
fn plan_respects_the_physical_screen_at_every_scale() {
    for style in ["rings", "bars", "cards", "segments", "arc"] {
        let c = cfg(style);
        for n in [1usize, 4, 9, 14] {
            let it = items(&c, n);
            for s in SCALES {
                let screen = ((1920.0 * s).round() as i64, (1080.0 * s).round() as i64);
                for vertical in [true, false] {
                    let limit = stack_limit(&c, vertical, true, screen);
                    let (size, shown) = plan_layout(&c, &it, s, vertical, 40.0 * s, limit, None);
                    assert!(shown >= 1 && shown <= n, "{style} n={n} scale={s}: shown={shown}");
                    let view = apply_plan(&it, shown);
                    let real: Vec<String> = view.iter().filter(|i| i.0 != MORE).map(|i| i.0.clone()).collect();
                    let d = cell_dims(&c, &real, view.len(), s, vertical, 40.0 * s, size);
                    let along = if vertical { d.ch } else { d.cw } * view.len() as f64;
                    assert!(
                        along <= limit as f64 + 1.0,
                        "{style} n={n} scale={s} vertical={vertical}: {along} > limit {limit} (size {size}, shown {shown})"
                    );
                }
            }
        }
    }
}

#[test]
fn few_accounts_keep_their_logical_size_at_every_scale() {
    // 4 accounts on a screen that scales with the DPI: nothing is shrunk or cut, whatever the scale
    let c = cfg("rings");
    let it = items(&c, 4);
    for s in SCALES {
        let screen = ((1920.0 * s).round() as i64, (1080.0 * s).round() as i64);
        let limit = stack_limit(&c, true, true, screen);
        let (size, shown) = plan_layout(&c, &it, s, true, 40.0 * s, limit, None);
        assert_eq!((size, shown), (36.0, 4), "scale {s}");
    }
}

#[test]
fn hover_card_stays_on_the_physical_screen() {
    for s in SCALES {
        let (sw, sh) = (1920.0 * s, 1080.0 * s);
        let (tw, th) = (300.0 * s, 220.0 * s);
        let gap = 8.0 * s;
        for (wx, mouse_y) in [(10.0, 5.0), (sw - 60.0 * s, sh - 3.0), (sw / 2.0 - 20.0, sh / 2.0), (0.0, sh)] {
            let (x, y) = tip_position((wx, 100.0, 44.0 * s, 200.0 * s), mouse_y, (tw, th), (sw, sh), gap);
            assert!(
                x as f64 >= gap - 1.0 && (x as f64 + tw) <= sw - gap + 1.0 && y as f64 >= gap - 1.0 && (y as f64 + th) <= sh - gap + 1.0,
                "scale {s} widget x={wx} mouse_y={mouse_y}: card at ({x},{y}) {tw}x{th} on {sw}x{sh}"
            );
        }
    }
}
