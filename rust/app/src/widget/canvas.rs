//! Tiny software canvas: premultiplied RGBA f32, anti-aliased shapes from signed distance fields, glyph masks
//! blended through a colour. The widget draws at 3x and downsamples (`finish`), like the Python renderer.

pub type Rgba = [u8; 4];

pub struct Canvas {
    pub w: usize,
    pub h: usize,
    px: Vec<[f32; 4]>,
}

/// Final image: premultiplied BGRA bytes (what `UpdateLayeredWindow` wants).
pub struct Img {
    pub w: usize,
    pub h: usize,
    pub bgra: Vec<u8>,
}

#[inline]
fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

impl Canvas {
    pub fn new(w: usize, h: usize) -> Canvas {
        Canvas {
            w,
            h,
            px: vec![[0.0; 4]; w * h],
        }
    }

    /// `src over dst` with coverage `cov` in 0..=1.
    #[inline]
    fn put(&mut self, x: usize, y: usize, c: Rgba, cov: f32) {
        let a = c[3] as f32 / 255.0 * cov;
        if a <= 0.0 {
            return;
        }
        let d = &mut self.px[y * self.w + x];
        let k = 1.0 - a;
        d[0] = c[0] as f32 / 255.0 * a + d[0] * k;
        d[1] = c[1] as f32 / 255.0 * a + d[1] * k;
        d[2] = c[2] as f32 / 255.0 * a + d[2] * k;
        d[3] = a + d[3] * k;
    }

    fn bounds(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> (usize, usize, usize, usize) {
        let lo = |v: f32, m: usize| (v.floor().max(0.0) as usize).min(m);
        let hi = |v: f32, m: usize| (v.ceil().max(0.0) as usize).min(m);
        (
            lo(x0 - 1.0, self.w),
            lo(y0 - 1.0, self.h),
            hi(x1 + 1.0, self.w),
            hi(y1 + 1.0, self.h),
        )
    }

    /// Run `cov(px, py)` over the bounding box (pixel centres) and blend.
    fn shape(&mut self, bb: (f32, f32, f32, f32), c: Rgba, f: impl Fn(f32, f32) -> f32) {
        let (ix0, iy0, ix1, iy1) = self.bounds(bb.0, bb.1, bb.2, bb.3);
        for y in iy0..iy1 {
            for x in ix0..ix1 {
                let cov = f(x as f32 + 0.5, y as f32 + 0.5);
                if cov > 0.0 {
                    self.put(x, y, c, cov);
                }
            }
        }
    }

    /// Signed distance to a rounded rectangle (negative inside).
    fn sd_rrect(px: f32, py: f32, cx: f32, cy: f32, hx: f32, hy: f32, r: f32) -> f32 {
        let qx = (px - cx).abs() - hx + r;
        let qy = (py - cy).abs() - hy + r;
        (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
    }

    pub fn fill_rrect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32, c: Rgba) {
        let (cx, cy, hx, hy) = (
            (x0 + x1) / 2.0,
            (y0 + y1) / 2.0,
            (x1 - x0) / 2.0,
            (y1 - y0) / 2.0,
        );
        if hx <= 0.0 || hy <= 0.0 {
            return;
        }
        let r = r.min(hx).min(hy).max(0.0);
        self.shape((x0, y0, x1, y1), c, move |px, py| {
            clamp01(0.5 - Self::sd_rrect(px, py, cx, cy, hx, hy, r))
        });
    }

    /// Outline drawn INWARD from the box edge (PIL semantics).
    pub fn stroke_rrect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32, wd: f32, c: Rgba) {
        let (cx, cy, hx, hy) = (
            (x0 + x1) / 2.0,
            (y0 + y1) / 2.0,
            (x1 - x0) / 2.0,
            (y1 - y0) / 2.0,
        );
        if hx <= 0.0 || hy <= 0.0 {
            return;
        }
        let r = r.min(hx).min(hy).max(0.0);
        self.shape((x0, y0, x1, y1), c, move |px, py| {
            let d = Self::sd_rrect(px, py, cx, cy, hx, hy, r);
            clamp01(0.5 - d).min(clamp01(d + wd + 0.5))
        });
    }

    pub fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, c: Rgba) {
        self.fill_rrect(cx - r, cy - r, cx + r, cy + r, r, c);
    }

    /// Circle outline of outer radius `r`, `wd` thick (inward).
    pub fn ring(&mut self, cx: f32, cy: f32, r: f32, wd: f32, c: Rgba) {
        self.stroke_rrect(cx - r, cy - r, cx + r, cy + r, r, wd, c);
    }

    /// Arc band of outer radius `r`, `wd` thick, from `a0` to `a1` degrees (clockwise from 3 o'clock, y down),
    /// flat caps (PIL `ImageDraw.arc`).
    pub fn arc(&mut self, cx: f32, cy: f32, r: f32, wd: f32, a0: f32, a1: f32, c: Rgba) {
        let span = (a1 - a0).clamp(0.0, 360.0);
        if span <= 0.0 {
            return;
        }
        self.shape((cx - r, cy - r, cx + r, cy + r), c, move |px, py| {
            let (dx, dy) = (px - cx, py - cy);
            let rho = (dx * dx + dy * dy).sqrt();
            let radial = clamp01(r - rho + 0.5).min(clamp01(rho - (r - wd) + 0.5));
            if radial <= 0.0 {
                return 0.0;
            }
            if span >= 359.99 {
                return radial;
            }
            let th = dy.atan2(dx).to_degrees();
            let mut t = (th - a0).rem_euclid(360.0);
            if t > 180.0 + span / 2.0 {
                t -= 360.0;
            }
            let edge = t.min(span - t) * rho * std::f32::consts::PI / 180.0;
            radial * clamp01(0.5 + edge)
        });
    }

    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, wd: f32, c: Rgba) {
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len2 = (dx * dx + dy * dy).max(1e-6);
        let h = wd / 2.0;
        self.shape(
            (
                x0.min(x1) - h,
                y0.min(y1) - h,
                x0.max(x1) + h,
                y0.max(y1) + h,
            ),
            c,
            move |px, py| {
                let t = (((px - x0) * dx + (py - y0) * dy) / len2).clamp(0.0, 1.0);
                let (qx, qy) = (x0 + t * dx - px, y0 + t * dy - py);
                clamp01(h - (qx * qx + qy * qy).sqrt() + 0.5)
            },
        );
    }

    pub fn fill_rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, c: Rgba) {
        self.shape((x0, y0, x1, y1), c, move |px, py| {
            clamp01(px - x0 + 0.5)
                .min(clamp01(x1 - px + 0.5))
                .min(clamp01(py - y0 + 0.5))
                .min(clamp01(y1 - py + 0.5))
        });
    }

    /// Blend a coverage mask (one byte per pixel) at integer position `(x, y)` through colour `c`.
    pub fn mask(&mut self, x: i32, y: i32, mw: usize, mh: usize, cov: &[u8], c: Rgba) {
        for my in 0..mh {
            let py = y + my as i32;
            if py < 0 || py as usize >= self.h {
                continue;
            }
            for mx in 0..mw {
                let px = x + mx as i32;
                if px < 0 || px as usize >= self.w {
                    continue;
                }
                let v = cov[my * mw + mx];
                if v > 0 {
                    self.put(px as usize, py as usize, c, v as f32 / 255.0);
                }
            }
        }
    }

    /// Copy `src` into self at integer offset (source over, used to place the cells inside the padded frame).
    pub fn paste(&mut self, src: &Canvas, ox: usize, oy: usize) {
        for y in 0..src.h {
            if oy + y >= self.h {
                break;
            }
            for x in 0..src.w {
                if ox + x >= self.w {
                    break;
                }
                self.px[(oy + y) * self.w + ox + x] = src.px[y * src.w + x];
            }
        }
    }

    /// Downsample by `ss` (box filter) while rotating `angle` degrees clockwise about the centre; the output
    /// is enlarged to hold the rotated frame (PIL `rotate(expand=True)`). `opaque_scale` multiplies alpha.
    pub fn finish(&self, ss: usize, angle: f64) -> Img {
        let (fw0, fh0) = (self.w as f64 / ss as f64, self.h as f64 / ss as f64);
        let (a, s, c) = (
            angle.to_radians(),
            angle.to_radians().sin(),
            angle.to_radians().cos(),
        );
        let _ = a;
        let (fw, fh) = if angle == 0.0 {
            (self.w / ss, self.h / ss)
        } else {
            (
                ((fw0 * c.abs() + fh0 * s.abs()).round() as usize).max(1),
                ((fw0 * s.abs() + fh0 * c.abs()).round() as usize).max(1),
            )
        };
        let (fw, fh) = (fw.max(1), fh.max(1));
        let mut out = vec![0u8; fw * fh * 4];
        let sub = ss.max(1);
        let n = (sub * sub) as f32;
        for oy in 0..fh {
            for ox in 0..fw {
                let mut acc = [0.0f32; 4];
                for sy in 0..sub {
                    for sx in 0..sub {
                        // point in final-pixel units relative to the output centre
                        let rx = ox as f64 + (sx as f64 + 0.5) / sub as f64 - fw as f64 / 2.0;
                        let ry = oy as f64 + (sy as f64 + 0.5) / sub as f64 - fh as f64 / 2.0;
                        // inverse rotation (we rotate clockwise on screen)
                        let (ux, uy) = (rx * c + ry * s, -rx * s + ry * c);
                        let sxp = ((ux + fw0 / 2.0) * ss as f64).floor();
                        let syp = ((uy + fh0 / 2.0) * ss as f64).floor();
                        if sxp >= 0.0
                            && syp >= 0.0
                            && (sxp as usize) < self.w
                            && (syp as usize) < self.h
                        {
                            let p = &self.px[syp as usize * self.w + sxp as usize];
                            for i in 0..4 {
                                acc[i] += p[i];
                            }
                        }
                    }
                }
                let o = (oy * fw + ox) * 4;
                // premultiplied BGRA
                out[o] = (acc[2] / n * 255.0 + 0.5).min(255.0) as u8;
                out[o + 1] = (acc[1] / n * 255.0 + 0.5).min(255.0) as u8;
                out[o + 2] = (acc[0] / n * 255.0 + 0.5).min(255.0) as u8;
                out[o + 3] = (acc[3] / n * 255.0 + 0.5).min(255.0) as u8;
            }
        }
        Img {
            w: fw,
            h: fh,
            bgra: out,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filled_circle_is_opaque_inside_clear_outside() {
        let mut c = Canvas::new(60, 60);
        c.fill_circle(30.0, 30.0, 20.0, [255, 0, 0, 255]);
        let img = c.finish(3, 0.0);
        assert_eq!((img.w, img.h), (20, 20));
        let at = |x: usize, y: usize| &img.bgra[(y * img.w + x) * 4..][..4];
        assert_eq!(at(10, 10), &[0, 0, 255, 255]); // premultiplied BGRA of opaque red
        assert_eq!(at(0, 0)[3], 0);
    }

    #[test]
    fn arc_covers_only_its_angles() {
        let mut c = Canvas::new(100, 100);
        // top semicircle (180..360 degrees, clockwise from 3 o'clock with y down)
        c.arc(50.0, 50.0, 40.0, 10.0, 180.0, 360.0, [0, 255, 0, 255]);
        let a = |x: usize, y: usize| c.px[y * c.w + x][3];
        assert!(a(50, 14) > 0.99, "top of the ring is covered");
        assert!(a(50, 86) < 0.01, "bottom is empty");
        assert!(a(50, 50) < 0.01, "centre is empty");
    }

    #[test]
    fn rotation_expands_the_frame() {
        let c = Canvas::new(300, 90);
        let img = c.finish(3, 90.0);
        assert_eq!((img.w, img.h), (30, 100));
    }
}
