//! Tray icon rasteriser: port of `App.make_icon` (256-unit design space, rendered 64x64 with anti-aliasing).
//! No fonts: the number is drawn with a 5x7 bitmap font so the result is deterministic and dependency-free.

pub const SIZE: usize = 64;
const S: f32 = 256.0;

/// What to draw (from `TrayView`): colour of outline/text/bar, text ("?", "!!", "42"), bar right edge in
/// design units (None = empty track only), busy dot.
pub struct IconSpec<'a> {
    pub color: &'a str,
    pub text: &'a str,
    pub bar_right: Option<f64>,
    pub busy: bool,
}

type Rgb = [f32; 3];

fn hex(c: &str) -> Rgb {
    let p = |i: usize| {
        u8::from_str_radix(c.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0) as f32 / 255.0
    };
    [p(1), p(3), p(5)]
}

/// Signed distance to a rounded box given by corner points (design units); negative inside.
fn sd_round_box(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> f32 {
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (hx, hy) = ((x1 - x0) / 2.0 - r, (y1 - y0) / 2.0 - r);
    let (qx, qy) = ((x - cx).abs() - hx, (y - cy).abs() - hy);
    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
}

fn cov(sd: f32) -> f32 {
    // one output pixel = 4 design units
    (0.5 - sd / 4.0).clamp(0.0, 1.0)
}

/// 5x7 glyphs, one row per entry (bit 4 = leftmost).
fn glyph(c: char) -> [u8; 7] {
    match c {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        '!' => [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04],
        _ => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04], // '?'
    }
}

/// Coverage of the text at design point (x, y). Text is centred on (128, 108), glyph cell 16 units.
fn text_cov(text: &str, x: f32, y: f32) -> f32 {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return 0.0;
    }
    let (cell, gap) = if chars.len() < 3 {
        (16.0, 16.0)
    } else {
        (13.0, 12.0)
    };
    let w = chars.len() as f32 * 5.0 * cell + (chars.len() as f32 - 1.0) * gap;
    let (left, top) = (128.0 - w / 2.0, 108.0 - 3.5 * cell);
    let mut best = 0.0f32;
    for (i, ch) in chars.iter().enumerate() {
        let gx = left + i as f32 * (5.0 * cell + gap);
        let g = glyph(*ch);
        for (row, bits) in g.iter().enumerate() {
            for col in 0..5 {
                if bits >> (4 - col) & 1 == 1 {
                    let (x0, y0) = (gx + col as f32 * cell, top + row as f32 * cell);
                    let sd = sd_round_box(x, y, x0, y0, x0 + cell, y0 + cell, 2.0);
                    best = best.max(cov(sd));
                }
            }
        }
    }
    best
}

fn over(dst: &mut [f32; 4], c: Rgb, a: f32) {
    // dst is premultiplied RGBA
    let k = 1.0 - a;
    for i in 0..3 {
        dst[i] = c[i] * a + dst[i] * k;
    }
    dst[3] = a + dst[3] * k;
}

/// Render to 64x64 premultiplied BGRA (the layout `CreateDIBSection` + `CreateIconIndirect` expect).
pub fn render(spec: &IconSpec) -> Vec<u8> {
    let c = hex(spec.color);
    let bg = hex("#161616");
    let track = hex("#3a3a3a");
    let blue = hex("#61afef");
    let mut out = vec![0u8; SIZE * SIZE * 4];
    for py in 0..SIZE {
        for px in 0..SIZE {
            let (x, y) = (
                (px as f32 + 0.5) * S / SIZE as f32,
                (py as f32 + 0.5) * S / SIZE as f32,
            );
            let mut p = [0f32; 4];
            let body = sd_round_box(x, y, 6.0, 6.0, S - 6.0, S - 6.0, 44.0);
            over(&mut p, bg, cov(body));
            over(&mut p, c, cov(body) * cov(-(body + 14.0))); // 14-wide inner outline
            over(&mut p, c, text_cov(spec.text, x, y));
            over(
                &mut p,
                track,
                cov(sd_round_box(x, y, 44.0, 186.0, S - 44.0, 214.0, 14.0)),
            );
            if let Some(r) = spec.bar_right {
                over(
                    &mut p,
                    c,
                    cov(sd_round_box(x, y, 44.0, 186.0, r as f32, 214.0, 14.0)),
                );
            }
            if spec.busy {
                let d = ((x - (S - 49.0)).powi(2) + (y - 49.0).powi(2)).sqrt() - 43.0;
                over(&mut p, bg, cov(d));
                over(&mut p, blue, cov(d + 10.0));
            }
            let o = (py * SIZE + px) * 4;
            out[o] = (p[2] * 255.0 + 0.5) as u8;
            out[o + 1] = (p[1] * 255.0 + 0.5) as u8;
            out[o + 2] = (p[0] * 255.0 + 0.5) as u8;
            out[o + 3] = (p[3] * 255.0 + 0.5) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(b: &[u8], x: usize, y: usize) -> [u8; 4] {
        let o = (y * SIZE + x) * 4;
        [b[o], b[o + 1], b[o + 2], b[o + 3]]
    }
    fn spec(
        color: &'static str,
        text: &'static str,
        bar: Option<f64>,
        busy: bool,
    ) -> IconSpec<'static> {
        IconSpec {
            color,
            text,
            bar_right: bar,
            busy,
        }
    }

    #[test]
    fn corners_transparent_body_dark_outline_coloured() {
        let b = render(&spec("#7fd88f", "42", Some(100.0), false));
        assert_eq!(px(&b, 0, 0)[3], 0, "outside the rounded body");
        assert_eq!(
            px(&b, 32, 14),
            [0x16, 0x16, 0x16, 255],
            "body fill #161616 (BGR order is symmetric)"
        );
        // outline: x = 6..20 design units -> pixel 2 (design ~10)
        assert_eq!(
            px(&b, 2, 32),
            [0x8f, 0xd8, 0x7f, 255],
            "outline in the colour (BGRA)"
        );
    }

    #[test]
    fn bar_fill_follows_percentage_and_busy_dot_only_when_busy() {
        let full = render(&spec("#e06c75", "!!", Some(212.0), false));
        let half = render(&spec("#e06c75", "50", Some(128.0), false));
        let none = render(&spec("#6b6b6b", "?", None, false));
        let at = |b: &[u8], x| px(b, x, 50);
        assert_eq!(at(&full, 50), [0x75, 0x6c, 0xe0, 255]);
        assert_eq!(at(&half, 20), [0x75, 0x6c, 0xe0, 255]);
        assert_eq!(
            at(&half, 50),
            [0x3a, 0x3a, 0x3a, 255],
            "track beyond the fill"
        );
        assert_eq!(at(&none, 20), [0x3a, 0x3a, 0x3a, 255]);
        let busy = render(&spec("#7fd88f", "1", None, true));
        let idle = render(&spec("#7fd88f", "1", None, false));
        assert_eq!(px(&busy, 51, 12), [0xef, 0xaf, 0x61, 255], "blue dot");
        assert_ne!(px(&idle, 51, 12), px(&busy, 51, 12));
    }

    #[test]
    fn text_changes_pixels_and_is_inside_the_icon() {
        let a = render(&spec("#7fd88f", "88", None, false));
        let b = render(&spec("#7fd88f", "11", None, false));
        let c = render(&spec("#7fd88f", "", None, false));
        assert_ne!(a, b);
        assert_ne!(a, c);
        let lit = |v: &[u8]| {
            (0..SIZE * SIZE)
                .filter(|i| v[i * 4 + 1] == 0xd8 && v[i * 4 + 3] == 255)
                .count()
        };
        assert!(lit(&a) > lit(&b) && lit(&b) > lit(&c));
        for t in ["0", "5", "99", "!!", "?"] {
            assert_eq!(
                render(&spec("#7fd88f", t, None, false)).len(),
                SIZE * SIZE * 4
            );
        }
    }
}
