//! Pure window-geometry rules of dashboard.py (physical pixels), testable without a window.

use serde_json::Value;

pub const MIN_W: i32 = 420;
pub const MIN_H: i32 = 330;

/// `initial_width`: columns for the accounts (the grid adds one per ~380 px), kept inside the screen.
pub fn initial_width(n_profiles: usize, screen_w: f64) -> i32 {
    let w = if n_profiles <= 4 {
        760
    } else if n_profiles <= 8 {
        1120
    } else {
        1480
    };
    (w as f64).min(screen_w - 80.0) as i32
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(f64::from(*b as u8)), // Python: bool is an int
        _ => None,
    }
}

/// `restore_saved`: the saved `dash_rect` ([x, y, w, h], ints or floats) clamped to the screen so at least part of
/// the window stays reachable. `None` when the value is not a list of 4 numbers.
pub fn restore_rect(saved: &Value, sw: i32, sh: i32) -> Option<[i32; 4]> {
    let a = saved.as_array().filter(|a| a.len() == 4)?;
    let n: Vec<f64> = a.iter().map(num).collect::<Option<_>>()?;
    let (sw, sh) = (f64::from(sw), f64::from(sh));
    let w = n[2].max(f64::from(MIN_W)).min(sw);
    let h = n[3].max(f64::from(MIN_H)).min(sh);
    let x = n[0].max(-w + 160.0).min(sw - 160.0);
    let y = n[1].max(0.0).min(sh - 80.0);
    Some([x as i32, y as i32, w as i32, h as i32])
}

/// `fit`: follow the content height `h_css` (CSS px) at DPI `scale`, between MIN_H and the screen height - 40.
pub fn fit_rect(cur: [i32; 4], h_css: f64, scale: f64, screen_h: i32) -> [i32; 4] {
    let top = f64::from(screen_h - (40.0 * scale) as i32);
    let h = (f64::from(MIN_H) * scale).max((h_css * scale).min(top));
    [cur[0], cur[1], cur[2], h as i32]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn initial_width_by_profile_count() {
        assert_eq!(initial_width(0, 1920.0), 760);
        assert_eq!(initial_width(4, 1920.0), 760);
        assert_eq!(initial_width(5, 1920.0), 1120);
        assert_eq!(initial_width(8, 1920.0), 1120);
        assert_eq!(initial_width(9, 1920.0), 1480);
        assert_eq!(initial_width(14, 1000.0), 920); // screen - 80
    }

    #[test]
    fn restore_clamps_like_python() {
        // ordinary rect kept
        assert_eq!(
            restore_rect(&json!([100, 50, 800, 600]), 1920, 1080),
            Some([100, 50, 800, 600])
        );
        // too small -> minimum; floats accepted (truncated)
        assert_eq!(
            restore_rect(&json!([10.9, 20.9, 100, 100.5]), 1920, 1080),
            Some([10, 20, 420, 330])
        );
        // bigger than the screen -> screen; far right/bottom -> 160 / 80 px stay visible
        assert_eq!(
            restore_rect(&json!([5000, 5000, 4000, 4000]), 1920, 1080),
            Some([1760, 1000, 1920, 1080])
        );
        // far left: only 160 px of the window remain on screen; negative y -> 0
        assert_eq!(
            restore_rect(&json!([-9000, -50, 800, 600]), 1920, 1080),
            Some([-640, 0, 800, 600])
        );
    }

    #[test]
    fn restore_rejects_bad_shapes() {
        for v in [
            json!(null),
            json!([1, 2, 3]),
            json!([1, 2, 3, "x"]),
            json!({"a": 1}),
            json!("rect"),
        ] {
            assert_eq!(restore_rect(&v, 1920, 1080), None, "{v}");
        }
    }

    #[test]
    fn fit_follows_content_within_limits() {
        let cur = [10, 20, 800, 500];
        assert_eq!(fit_rect(cur, 640.0, 1.0, 1080), [10, 20, 800, 640]);
        assert_eq!(fit_rect(cur, 100.0, 1.0, 1080), [10, 20, 800, 330]); // MIN_H
        assert_eq!(fit_rect(cur, 5000.0, 1.0, 1080), [10, 20, 800, 1040]); // screen - 40
        assert_eq!(fit_rect(cur, 640.0, 1.5, 1620), [10, 20, 800, 960]); // scaled
        assert_eq!(fit_rect(cur, 100.0, 1.5, 1620), [10, 20, 800, 495]);
    }

    /// One monitor only on the dev machine: the per-DPI behaviour of the window rules is proven as pure math.
    #[test]
    fn rules_hold_at_every_dpi_scale() {
        for scale in [1.0_f64, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0] {
            let (sw, sh) = ((1920.0 * scale) as i32, (1080.0 * scale) as i32);
            // initial_width takes the LOGICAL screen width: the physical window never exceeds screen - 80 logical px
            for n in [1, 5, 9, 14] {
                let w = f64::from(initial_width(n, f64::from(sw) / scale));
                assert!(w * scale <= f64::from(sw) - 80.0 * scale + 1.0, "n={n} scale={scale}");
            }
            // fit stays between the scaled minimum and the screen height - 40 logical px
            for h_css in [50.0, 640.0, 900.0, 5000.0] {
                let r = fit_rect([10, 20, 800, 500], h_css, scale, sh);
                assert!(
                    f64::from(r[3]) >= f64::from(MIN_H) * scale - 1.0 && r[3] <= sh - (40.0 * scale) as i32,
                    "h_css={h_css} scale={scale}: {r:?}"
                );
                if (330.0..=1000.0).contains(&h_css) {
                    assert!((f64::from(r[3]) - h_css * scale).abs() <= 1.0, "follows the content: {r:?}");
                }
            }
            // a remembered rect always keeps part of the window reachable on a screen of any scale
            let r = restore_rect(&json!([sw * 3, sh * 3, sw * 2, sh * 2]), sw, sh).unwrap();
            assert!(r[0] <= sw - 160 && r[1] <= sh - 80 && r[2] <= sw && r[3] <= sh, "scale={scale}: {r:?}");
        }
    }
}
