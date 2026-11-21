//! Taskbar widget (port of `widget.py`): software renderer + layered window.
//!
//! * `canvas`: anti-aliased shapes on a premultiplied float canvas, 3x supersampling, rotation.
//! * `text`: GDI glyph masks (Segoe UI).
//! * `render`: the five styles, edit overlay, hover cards (geometry from `cu_core::widget`).
//! * `win`: the window thread (mouse, drag/resize/rotate/lock, menu, timers, tray anchoring).
//! * `preview`: `claude-usage preview <dir>` renders sample PNGs without touching the desktop.

#![allow(clippy::too_many_arguments)] // drawing primitives take geometry + colour by value, like the Python helpers

pub mod canvas;
pub mod render;
pub mod text;
pub mod win;

use canvas::Img;
use cu_core::config::{defaults, Config};
use cu_core::widget::*;
use serde_json::{json, Value};

/// Make the process per-monitor DPI aware (before any window exists), like the Python build.
pub fn init_dpi() {
    use windows_sys::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    // SAFETY: process-wide setting; failure (already set) is harmless.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

fn sample_cache(n: usize) -> Value {
    let names = [
        "default",
        "claude-team-01",
        "claude-ngoai",
        "claude-super-long-account-name",
        "work",
        "claude-a",
        "claude-b",
        "claude-c",
    ];
    let mut profs = serde_json::Map::new();
    for (i, name) in names.iter().take(n).enumerate() {
        let p = match i {
            1 => {
                json!({"err": "429", "status": 429, "five": 91.0, "week": 55.0, "fetched_at": cu_core::cache::now_ts() - 600.0,
                        "five_reset": "2026-11-21T23:30:00+00:00", "week_reset": "2026-11-25T08:00:00+00:00"})
            }
            3 => json!({"err": "http 500", "status": 500}),
            _ => {
                json!({"five": ((i * 37 + 12) % 100) as f64, "week": ((i * 53 + 30) % 100) as f64, "plan": "Max 5x",
                        "five_reset": "2026-11-21T23:30:00+00:00", "week_reset": "2026-11-25T08:00:00+00:00",
                        "fetched_at": cu_core::cache::now_ts() - 40.0,
                        "extra": {"pct": 42.0, "used": 21.0, "limit": 50.0, "currency": "USD"}})
            }
        };
        profs.insert((*name).into(), p);
    }
    json!({"ts": 1.0, "warn_pct": 80, "block_pct": 95, "busy": ["claude-ngoai"], "profiles": profs})
}

fn draw(
    cfg: &Config,
    cache: &Value,
    vertical: bool,
    light: bool,
    edit: bool,
    angle: f64,
    hover: i64,
) -> Img {
    let scale = 1.0;
    let items = visible_profiles(cfg, Some(cache));
    let limit = stack_limit(cfg, vertical, false, (1920, 1080));
    let (size, shown) = plan_layout(
        cfg,
        &items,
        scale,
        vertical,
        if vertical { 48.0 } else { 0.0 },
        limit,
        None,
    );
    let view = apply_plan(&items, shown);
    let busy = vec!["claude-ngoai".to_string()];
    let scene = render::Scene {
        cfg,
        view: &view,
        busy: &busy,
        scale,
        hover,
        vertical,
        bar_thick: if vertical { 48.0 } else { 0.0 },
        size,
        phase: 3,
        light,
    };
    let (cv, dims) = render::render(&scene);
    render::compose(&cv, dims.w, dims.h, scale, edit, angle).0
}

/// Flatten onto an opaque background and write an uncompressed PNG (no extra dependency).
pub fn write_png(path: &std::path::Path, img: &Img, bg: [u8; 3]) -> std::io::Result<()> {
    let (w, h) = (img.w, img.h);
    let mut raw = Vec::with_capacity((w * 3 + 1) * h);
    for y in 0..h {
        raw.push(0);
        for x in 0..w {
            let p = &img.bgra[(y * w + x) * 4..][..4];
            let inv = 255 - p[3] as u32;
            for (c, b) in [(p[2], bg[0]), (p[1], bg[1]), (p[0], bg[2])] {
                raw.push((c as u32 + b as u32 * inv / 255).min(255) as u8);
            }
        }
    }
    let mut z = vec![0x78, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    while let Some(c) = chunks.next() {
        z.push(u8::from(chunks.peek().is_none()));
        z.extend_from_slice(&(c.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        z.extend_from_slice(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &v in &raw {
        a = (a + v as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let crc = |data: &[u8]| {
        let mut c = 0xFFFF_FFFFu32;
        for &v in data {
            c ^= v as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    };
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc(&body).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    std::fs::write(path, out)
}

/// `claude-usage preview <dir>`: render sample images (all styles, both layouts and themes, edit overlay with
/// rotation, hover cards) as PNG files. Nothing is shown on screen.
pub fn preview(dir: &std::path::Path) -> std::io::Result<usize> {
    std::fs::create_dir_all(dir)?;
    let mut n = 0;
    let cache = sample_cache(4);
    let many = sample_cache(8);
    for style in STYLES {
        for (vertical, vname) in [(false, "h"), (true, "v")] {
            for (light, tname) in [(false, "dark"), (true, "light")] {
                let mut cfg = defaults();
                cfg.insert("widget_style".into(), json!(style));
                cfg.insert("widget_plate".into(), json!(true));
                let bg = if light { [230, 230, 232] } else { [32, 32, 36] };
                let img = draw(&cfg, &cache, vertical, light, false, 0.0, 1);
                write_png(&dir.join(format!("{style}-{vname}-{tname}.png")), &img, bg)?;
                n += 1;
            }
        }
    }
    let mut cfg = defaults();
    cfg.insert("widget_style".into(), json!("rings"));
    cfg.insert("widget_max_cells".into(), json!(5));
    write_png(
        &dir.join("many-plus-n.png"),
        &draw(&cfg, &many, false, false, false, 0.0, -1),
        [32, 32, 36],
    )?;
    write_png(
        &dir.join("edit-rot30.png"),
        &draw(&cfg, &cache, false, false, true, 30.0, -1),
        [32, 32, 36],
    )?;
    let cfgd = defaults();
    let items = visible_profiles(&cfgd, Some(&cache));
    write_png(
        &dir.join("tip-ok.png"),
        &render::render_tip(&items[0].0, &items[0].1, &cfgd, 1.0, false),
        [32, 32, 36],
    )?;
    write_png(
        &dir.join("tip-stale.png"),
        &render::render_tip(&items[1].0, &items[1].1, &cfgd, 1.0, true),
        [230, 230, 232],
    )?;
    write_png(
        &dir.join("tip-more.png"),
        &render::render_more_tip(&items, &cfgd, 1.0, false),
        [32, 32, 36],
    )?;
    Ok(n + 5)
}
