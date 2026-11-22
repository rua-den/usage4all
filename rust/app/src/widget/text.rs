//! Text for the software canvas: GDI draws white glyphs on black into a small DIB (grayscale anti-aliasing), the
//! red channel is the coverage mask. Fonts are Segoe UI / Segoe UI Bold (GDI falls back for missing glyphs).

use super::canvas::{Canvas, Rgba};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::SIZE;
use windows_sys::Win32::Graphics::Gdi::*;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Font {
    pub bold: bool,
    pub px: i32,
}

struct Gdi {
    dc: HDC,
    fonts: HashMap<Font, HFONT>,
}

thread_local! {
    static GDI: RefCell<Option<Gdi>> = const { RefCell::new(None) };
}

fn with_gdi<R>(f: impl FnOnce(&mut Gdi) -> R) -> R {
    GDI.with(|g| {
        let mut g = g.borrow_mut();
        let gd = g.get_or_insert_with(|| Gdi {
            // SAFETY: a memory DC compatible with the screen; lives as long as the thread.
            dc: unsafe { CreateCompatibleDC(null_mut()) },
            fonts: HashMap::new(),
        });
        f(gd)
    })
}

impl Gdi {
    fn select(&mut self, font: Font) {
        let h = *self.fonts.entry(font).or_insert_with(|| {
            let mut lf: LOGFONTW = unsafe { std::mem::zeroed() };
            lf.lfHeight = -font.px.max(1);
            lf.lfWeight = if font.bold { 700 } else { 400 };
            lf.lfCharSet = DEFAULT_CHARSET;
            lf.lfQuality = ANTIALIASED_QUALITY;
            for (i, c) in "Segoe UI".encode_utf16().enumerate() {
                lf.lfFaceName[i] = c;
            }
            // SAFETY: fully initialised LOGFONTW.
            unsafe { CreateFontIndirectW(&lf) }
        });
        // SAFETY: valid DC and font.
        unsafe { SelectObject(self.dc, h) };
    }
}

/// Advance width of `s` in pixels.
pub fn measure(font: Font, s: &str) -> f32 {
    if s.is_empty() {
        return 0.0;
    }
    let w: Vec<u16> = s.encode_utf16().collect();
    with_gdi(|g| {
        g.select(font);
        let mut sz = SIZE { cx: 0, cy: 0 };
        // SAFETY: valid DC with the font selected.
        unsafe { GetTextExtentPoint32W(g.dc, w.as_ptr(), w.len() as i32, &mut sz) };
        sz.cx as f32
    })
}

/// A font of size <= `px` whose rendering of `text` is at most `max_w` wide (shrinks down to ~55%).
pub fn fit_font(bold: bool, px: f32, text: &str, max_w: f32) -> Font {
    let mut px = (px as i32).max(6);
    let floor = ((px as f32 * 0.55) as i32).max(6);
    let mut f = Font { bold, px };
    while px > floor && measure(f, text) > max_w {
        px -= 1;
        f = Font { bold, px };
    }
    f
}

/// Like [`fit_font`], but text still too wide is cut and ends with an ellipsis.
pub fn fit_label(bold: bool, px: f32, text: &str, max_w: f32) -> (Font, String) {
    let f = fit_font(bold, px, text, max_w);
    if measure(f, text) <= max_w {
        return (f, text.to_string());
    }
    let mut chars: Vec<char> = text.chars().collect();
    while chars.len() > 1 {
        chars.pop();
        let t: String = chars.iter().collect::<String>() + "…";
        if measure(f, &t) <= max_w {
            return (f, t);
        }
    }
    (f, chars.into_iter().collect())
}

/// Rasterised text: coverage mask (`mw` x `mh`), padding, and the text's own width/height.
struct Raster {
    mask: Vec<u8>,
    mw: usize,
    mh: usize,
    pad: i32,
    tw: f32,
    th: f32,
}

fn raster(font: Font, w16: &[u16]) -> Option<Raster> {
    with_gdi(|g| {
        g.select(font);
        let mut sz = SIZE { cx: 0, cy: 0 };
        // SAFETY: valid DC with the font selected.
        unsafe { GetTextExtentPoint32W(g.dc, w16.as_ptr(), w16.len() as i32, &mut sz) };
        let pad = (font.px / 3 + 2).max(2);
        let (mw, mh) = ((sz.cx + 2 * pad) as usize, (sz.cy + 2 * pad) as usize);
        if sz.cx <= 0 || sz.cy <= 0 || mw > 4096 || mh > 1024 {
            return None;
        }
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: mw as i32,
                biHeight: -(mh as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [RGBQUAD {
                rgbBlue: 0,
                rgbGreen: 0,
                rgbRed: 0,
                rgbReserved: 0,
            }],
        };
        let mut bits: *mut core::ffi::c_void = null_mut();
        // SAFETY: standard DIB section (zero-initialised = black), freed before returning.
        unsafe {
            let bmp = CreateDIBSection(g.dc, &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if bmp.is_null() || bits.is_null() {
                return None;
            }
            let old = SelectObject(g.dc, bmp);
            SetBkMode(g.dc, TRANSPARENT as i32);
            SetTextColor(g.dc, 0x00FF_FFFF);
            SetTextAlign(g.dc, TA_LEFT | TA_TOP);
            TextOutW(g.dc, pad, pad, w16.as_ptr(), w16.len() as i32);
            let px = std::slice::from_raw_parts(bits as *const u8, mw * mh * 4);
            let mask: Vec<u8> = px.iter().skip(1).step_by(4).copied().collect();
            SelectObject(g.dc, old);
            DeleteObject(bmp);
            Some(Raster {
                mask,
                mw,
                mh,
                pad,
                tw: sz.cx as f32,
                th: sz.cy as f32,
            })
        }
    })
}

/// Draw `s` with anchor `"lm"`, `"mm"` or `"rm"` (horizontal l/m/r, vertical middle of the font cell).
pub fn put_text(cv: &mut Canvas, x: f32, y: f32, s: &str, font: Font, col: [u8; 3], anchor: &str) {
    if s.is_empty() {
        return;
    }
    let w16: Vec<u16> = s.encode_utf16().collect();
    let Some(r) = raster(font, &w16) else { return };
    let left = match anchor.as_bytes().first() {
        Some(b'm') => x - r.tw / 2.0,
        Some(b'r') => x - r.tw,
        _ => x,
    };
    let top = y - r.th / 2.0;
    let (px, py) = (
        (left - r.pad as f32).round() as i32,
        (top - r.pad as f32).round() as i32,
    );
    cv.mask(
        px,
        py,
        r.mw,
        r.mh,
        &r.mask,
        [col[0], col[1], col[2], 255] as Rgba,
    );
}
