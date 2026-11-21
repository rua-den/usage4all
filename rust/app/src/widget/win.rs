//! Widget window: a per-pixel-alpha layered window next to the tray (or free-floating), its hover card, mouse
//! handling (drag / resize / rotate / lock), right-click menu, timers. Own thread with its own message loop,
//! like `Widget(threading.Thread)` in `widget.py`. Clicks that belong to the tray process (open dashboard,
//! refresh, quit) are posted to the tray window as `WM_COMMAND`.

use super::canvas::Img;
use super::render::{self, Geo, Scene};
use cu_core::cache::load_cache;
use cu_core::config::{self, Config};
use cu_core::log::log;
use cu_core::paths::{home_dir, Paths};
use cu_core::widget::*;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::{null, null_mut};
use std::time::{Instant, SystemTime};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows_sys::Win32::UI::Shell::SHQueryUserNotificationState;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WM_MOUSELEAVE: u32 = 0x02A3;
pub const CLASS: &str = "ClaudeUsageWidget";
pub const TIP_CLASS: &str = "ClaudeUsageTip";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Is the taskbar / system theme light? (`SystemUsesLightTheme`)
pub fn light_taskbar() -> bool {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let val = wide("SystemUsesLightTheme");
    let mut data: u32 = 0;
    let mut len: u32 = 4;
    // SAFETY: valid NUL-terminated names and a 4-byte output buffer.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            val.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            &mut data as *mut u32 as *mut _,
            &mut len,
        )
    };
    r == 0 && data == 1
}

pub fn is_light(cfg: &Config) -> bool {
    match cfg
        .get("widget_theme")
        .and_then(Value::as_str)
        .unwrap_or("auto")
    {
        "auto" => light_taskbar(),
        t => t == "light",
    }
}

/// Paint a premultiplied BGRA image into a layered window at (x, y).
fn blit(hwnd: HWND, img: &Img, x: i32, y: i32, alpha: u8) {
    // SAFETY: standard UpdateLayeredWindow sequence; every GDI object created here is released here.
    unsafe {
        let screen = GetDC(null_mut());
        let mem = CreateCompatibleDC(screen);
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: img.w as i32,
                biHeight: -(img.h as i32),
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
        let bmp = CreateDIBSection(screen, &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if !bmp.is_null() && !bits.is_null() {
            std::ptr::copy_nonoverlapping(img.bgra.as_ptr(), bits as *mut u8, img.bgra.len());
            let old = SelectObject(mem, bmp);
            let pt = POINT { x, y };
            let sz = SIZE {
                cx: img.w as i32,
                cy: img.h as i32,
            };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: 0,
                BlendFlags: 0,
                SourceConstantAlpha: alpha,
                AlphaFormat: 1,
            };
            UpdateLayeredWindow(hwnd, screen, &pt, &sz, mem, &src, 0, &blend, 2);
            SelectObject(mem, old);
            DeleteObject(bmp);
        }
        DeleteDC(mem);
        ReleaseDC(null_mut(), screen);
    }
}

#[derive(Clone, Copy)]
struct Taskbar {
    r: RECT,
    tray: Option<RECT>,
    vertical: bool,
}

#[derive(Default, Clone, Copy)]
struct Live {
    cx: Option<f64>,
    cy: Option<f64>,
    size: Option<f64>,
    angle: Option<f64>,
}

struct Drag {
    kind: &'static str,
    sx: i32,
    sy: i32,
    cx0: f64,
    cy0: f64,
    s0: f64,
    moved: bool,
    d0: f64,
}

struct St {
    size: f64,
    angle: f64,
    cx: Option<f64>,
    cy: Option<f64>,
    edit: bool,
}

struct Widget {
    paths: Paths,
    tray: isize,
    hwnd: HWND,
    tip: HWND,
    cfg: Option<Config>,
    cfg_m: Option<SystemTime>,
    sig: Option<String>,
    hover: i64,
    visible: bool,
    live: Option<Live>,
    drag: Option<Drag>,
    last_hit: Option<&'static str>,
    down_hit: Option<&'static str>,
    geo: Option<Geo>,
    img: Option<Img>,
    cell_w: f64,
    cell_h: f64,
    vertical: bool,
    built_angle: f64,
    org: (i32, i32),
    pos: Option<(i32, i32)>,
    scale: f64,
    phase: i64,
    fast: bool,
    tip_visible: bool,
    tip_key: Option<String>,
    tip_img: Option<Img>,
    tip_pos: Option<(i32, i32)>,
    hover_t: Instant,
    mouse_pt: (i32, i32),
    plan: (f64, usize),
    plan_key: Option<String>,
    view: Items,
    view_hidden: Items,
}

thread_local! {
    static STATE: RefCell<Option<Widget>> = const { RefCell::new(None) };
}

fn screen() -> (i64, i64) {
    // SAFETY: plain metric query.
    unsafe {
        (
            GetSystemMetrics(SM_CXSCREEN) as i64,
            GetSystemMetrics(SM_CYSCREEN) as i64,
        )
    }
}

fn lo(lp: LPARAM) -> i32 {
    (lp & 0xFFFF) as i16 as i32
}
fn hi(lp: LPARAM) -> i32 {
    ((lp >> 16) & 0xFFFF) as i16 as i32
}

impl Widget {
    fn cfg(&self) -> &Config {
        self.cfg.as_ref().expect("config loaded")
    }

    fn st(&self) -> St {
        let c = self.cfg();
        let l = self.live.unwrap_or_default();
        let num = |k: &str| c.get(k).and_then(cu_core::pyval::py_num);
        St {
            size: l
                .size
                .unwrap_or_else(|| config::get_f64(c, "widget_size", 36.0)),
            angle: l
                .angle
                .unwrap_or_else(|| num("widget_angle").unwrap_or(0.0)),
            cx: l.cx.or(num("widget_cx")),
            cy: l.cy.or(num("widget_cy")),
            edit: !flag(c, "widget_locked", true),
        }
    }

    fn load_cfg(&self) -> Config {
        config::load_config(&self.paths, &home_dir())
    }

    fn save_cfg(&mut self, cfg: Config) {
        let _ = config::save_config(&self.paths, &cfg);
        self.cfg = Some(cfg);
        self.cfg_m = std::fs::metadata(&self.paths.config)
            .and_then(|m| m.modified())
            .ok();
    }

    fn save_state(&mut self, kv: &[(&str, Value)]) {
        let mut cfg = self.load_cfg();
        for (k, v) in kv {
            cfg.insert(format!("widget_{k}"), v.clone());
        }
        self.save_cfg(cfg);
    }

    fn taskbar(&self) -> Option<Taskbar> {
        #[cfg(any(debug_assertions, feature = "e2e"))]
        if let Some(t) = self.fake_taskbar() {
            return t;
        }
        // SAFETY: window lookups and rect queries on shell windows; all out-params are valid.
        unsafe {
            let tb = FindWindowW(wide("Shell_TrayWnd").as_ptr(), null());
            if tb.is_null() || IsWindowVisible(tb) == 0 {
                return None;
            }
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(tb, &mut r);
            let tray = FindWindowExW(tb, null_mut(), wide("TrayNotifyWnd").as_ptr(), null());
            let mut tr: RECT = std::mem::zeroed();
            let ok = !tray.is_null()
                && GetWindowRect(tray, &mut tr) != 0
                && tr.right > tr.left
                && tr.bottom > tr.top;
            Some(Taskbar {
                r,
                tray: ok.then_some(tr),
                vertical: (r.right - r.left) < (r.bottom - r.top),
            })
        }
    }

    /// Content centre when auto-anchored next to the tray.
    fn auto_center(&self, tb: &Taskbar, w: f64, h: f64) -> (f64, f64) {
        let c = self.cfg();
        let s = self.scale;
        let anchor_tray = c.get("widget_anchor").and_then(Value::as_str) == Some("tray");
        let off = |k: &str| c.get(k).and_then(cu_core::pyval::py_num).unwrap_or(0.0);
        let r = tb.r;
        let (x, y);
        if tb.vertical {
            x = r.left as f64 + (((r.right - r.left) as f64 - w) / 2.0).floor();
            y = if anchor_tray {
                tb.tray
                    .map_or(r.bottom as f64 - (220.0 * s).floor(), |t| t.top as f64)
                    - h
                    - (8.0 * s).floor()
            } else {
                r.top as f64 + (8.0 * s).floor()
            };
        } else {
            y = r.top as f64 + (((r.bottom - r.top) as f64 - h) / 2.0).floor();
            x = if anchor_tray {
                tb.tray
                    .map_or(r.right as f64 - (260.0 * s).floor(), |t| t.left as f64)
                    - w
                    - (8.0 * s).floor()
            } else {
                r.left as f64 + (8.0 * s).floor()
            };
        }
        (
            x + off("widget_offset_x") + w / 2.0,
            y + off("widget_offset_y") + h / 2.0,
        )
    }

    fn fullscreen_busy(&self) -> bool {
        #[cfg(any(debug_assertions, feature = "e2e"))]
        if self.paths.data.join("fake_fullscreen.flag").exists() {
            return true;
        }
        if std::env::var_os("CU_WIDGET_IGNORE_FULLSCREEN").is_some() {
            return false;
        }
        let mut st: i32 = 0;
        // SAFETY: valid out-pointer.
        let hr = unsafe { SHQueryUserNotificationState(&mut st) };
        hr == 0 && matches!(st, 2..=4)
    }

    fn win_topleft(&self, ccx: f64, ccy: f64) -> (i32, i32) {
        let (g, img) = (self.geo.as_ref().unwrap(), self.img.as_ref().unwrap());
        let (dx, dy) = rot(
            g.m + g.w / 2.0 - g.wp / 2.0,
            g.mt + g.h / 2.0 - g.hp / 2.0,
            self.built_angle,
        );
        (
            (ccx - dx - img.w as f64 / 2.0).round() as i32,
            (ccy - dy - img.h as f64 / 2.0).round() as i32,
        )
    }

    fn present(&mut self, x: i32, y: i32) {
        let alpha = opacity_alpha(self.cfg());
        blit(self.hwnd, self.img.as_ref().unwrap(), x, y, alpha);
        self.org = (x, y);
        self.pos = Some((x, y));
    }

    fn hide_tip(&mut self) {
        if self.tip_visible {
            // SAFETY: our own window.
            unsafe { ShowWindow(self.tip, SW_HIDE) };
        }
        self.tip_visible = false;
        self.tip_key = None;
        self.tip_pos = None;
    }

    fn update_tip(&mut self, cache: &Option<Value>) {
        let c = self.cfg().clone();
        let on = flag(&c, "widget_tooltip", true);
        let n = self.view.len() as i64;
        if !(on
            && self.drag.is_none()
            && self.visible
            && self.hover >= 0
            && self.hover < n
            && self.hover_t.elapsed().as_secs_f64() >= 0.3)
        {
            return self.hide_tip();
        }
        let (name, p) = self.view[self.hover as usize].clone();
        let ts = cache
            .as_ref()
            .and_then(|c| c.get("ts"))
            .map(|v| v.to_string())
            .unwrap_or_default();
        let now = cu_core::cache::now_ts();
        let key = format!(
            "{name}|{ts}|{}|{}|{:?}|{:?}|{:?}|{:?}",
            (now / 5.0) as i64,
            self.scale,
            c.get("widget_theme"),
            c.get("widget_font"),
            c.get("warn_pct"),
            c.get("block_pct")
        );
        if self.tip_key.as_deref() != Some(&key) {
            let light = is_light(&c);
            self.tip_img = Some(if name == MORE {
                render::render_more_tip(&self.view_hidden, &c, self.scale, light)
            } else {
                render::render_tip(&name, &p, &c, self.scale, light)
            });
            self.tip_key = Some(key);
            self.tip_pos = None;
        }
        let (wimg, scr) = (self.img.as_ref().unwrap(), screen());
        let t = self.tip_img.as_ref().unwrap();
        let pos = tip_position(
            (
                self.org.0 as f64,
                self.org.1 as f64,
                wimg.w as f64,
                wimg.h as f64,
            ),
            self.mouse_pt.1 as f64,
            (t.w as f64, t.h as f64),
            (scr.0 as f64, scr.1 as f64),
            8.0,
        );
        let pos = (pos.0 as i32, pos.1 as i32);
        if Some(pos) != self.tip_pos || !self.tip_visible {
            blit(self.tip, t, pos.0, pos.1, 255);
            self.tip_pos = Some(pos);
        }
        // SAFETY: our own window; topmost without activating.
        unsafe {
            SetWindowPos(
                self.tip,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
        };
        self.tip_visible = true;
    }

    fn build(&mut self, cache: &Option<Value>, tb: &Option<Taskbar>) {
        let s = self.st();
        let c = self.cfg().clone();
        let layout = c
            .get("widget_layout")
            .and_then(Value::as_str)
            .unwrap_or("auto");
        let vertical = if layout == "auto" {
            tb.is_none_or(|t| t.vertical)
        } else {
            layout == "vertical"
        };
        let thick = match tb {
            Some(t) if t.vertical && vertical => (t.r.right - t.r.left) as f64,
            _ => 0.0,
        };
        let items = visible_profiles(&c, cache.as_ref());
        let limit = stack_limit(&c, vertical, s.cx.is_none(), screen());
        let names: Vec<&str> = items.iter().map(|i| i.0.as_str()).collect();
        let aliases = cu_core::alias::aliases(
            &items.iter().map(|i| i.0.clone()).collect::<Vec<_>>(),
            &c,
            label_max("rings"),
        );
        let key = format!(
            "{names:?}|{aliases:?}|{:.1}|{}|{vertical}|{thick}|{limit}|{}",
            s.size,
            self.scale,
            [
                "widget_style",
                "widget_font",
                "widget_label",
                "widget_reset",
                "widget_show_label",
                "widget_show_5h",
                "widget_show_7d",
                "widget_show_reset5",
                "widget_show_reset7",
                "widget_max_cells"
            ]
            .iter()
            .map(|k| format!("{:?}", c.get(*k)))
            .collect::<Vec<_>>()
            .join(",")
        );
        if self.plan_key.as_deref() != Some(&key) {
            let mut c2 = c.clone();
            c2.insert("widget_size".into(), json!(s.size));
            self.plan = plan_layout(&c2, &items, self.scale, vertical, thick, limit, None);
            self.plan_key = Some(key);
        }
        let (size, shown) = self.plan;
        let view = apply_plan(&items, shown);
        self.view_hidden = items.iter().skip(shown).cloned().collect();
        self.view = view;
        let busy: Vec<String> = match cache.as_ref().and_then(|c| c.get("busy")) {
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            _ => vec![],
        };
        let light = is_light(&c);
        let scene = Scene {
            cfg: &c,
            view: &self.view,
            busy: &busy,
            scale: self.scale,
            hover: if s.edit { -1 } else { self.hover },
            vertical,
            bar_thick: thick,
            size,
            phase: self.phase,
            light,
        };
        let (cv, dims) = render::render(&scene);
        let (img, geo) = render::compose(&cv, dims.w, dims.h, self.scale, s.edit, s.angle);
        self.cell_w = dims.cw;
        self.cell_h = dims.ch;
        self.img = Some(img);
        self.geo = Some(geo);
        self.vertical = vertical;
        self.built_angle = s.angle;
    }

    fn content_center(&self, tb: &Option<Taskbar>) -> Option<(f64, f64)> {
        let s = self.st();
        if let (Some(x), Some(y)) = (s.cx, s.cy) {
            return Some((x, y));
        }
        let g = self.geo.as_ref()?;
        tb.as_ref().map(|t| self.auto_center(t, g.w, g.h))
    }

    fn set_fast(&mut self, on: bool) {
        // SAFETY: timers of our own window.
        unsafe {
            if on && !self.fast {
                SetTimer(self.hwnd, 2, 110, None);
                self.fast = true;
            } else if !on && self.fast {
                KillTimer(self.hwnd, 2);
                self.fast = false;
            }
        }
    }

    fn hide(&mut self) {
        self.hide_tip();
        if self.visible {
            // SAFETY: our own window.
            unsafe { ShowWindow(self.hwnd, SW_HIDE) };
            self.visible = false;
        }
    }

    fn tick(&mut self, force: bool) {
        let m = std::fs::metadata(&self.paths.config)
            .and_then(|m| m.modified())
            .ok();
        if (m != self.cfg_m || self.cfg.is_none()) && self.drag.is_none() {
            self.cfg = Some(self.load_cfg());
            self.cfg_m = m;
            self.sig = None;
        }
        let c = self.cfg().clone();
        // SAFETY: plain query.
        self.scale = unsafe { GetDpiForSystem() } as f64 / 96.0;
        if !flag(&c, "widget_enabled", true) || self.fullscreen_busy() {
            return self.hide();
        }
        let cache = load_cache(&self.paths);
        let tb = self.taskbar();
        let s = self.st();
        if tb.is_none() && (s.cx.is_none() || s.cy.is_none()) {
            return self.hide();
        }
        let busy = cache
            .as_ref()
            .and_then(|c| c.get("busy"))
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty());
        let now = cu_core::cache::now_ts();
        self.phase = if busy { ((now * 8.0) as i64) % 12 } else { 0 };
        self.set_fast(busy || self.hover >= 0);
        let keys = [
            "widget_reset",
            "warn_pct",
            "block_pct",
            "widget_layout",
            "widget_style",
            "widget_label",
            "widget_font",
            "widget_opacity",
            "widget_theme",
            "widget_plate",
            "widget_show_label",
            "widget_show_5h",
            "widget_show_7d",
            "widget_show_reset5",
            "widget_show_reset7",
            "widget_sort",
            "widget_max_cells",
            "widget_max_len",
            "widget_hidden",
            "aliases",
            "widget_anchor",
        ];
        let sig = format!(
            "{:?}|{}|{}|{busy}|{}|{}|{:?}|{}|{}|{}|{}|{}",
            cache.as_ref().and_then(|c| c.get("ts")),
            (now / 20.0) as i64,
            self.phase,
            self.hover,
            self.scale,
            tb.map(|t| (t.vertical, t.r.right - t.r.left, t.r.bottom - t.r.top)),
            s.size,
            s.angle,
            s.edit,
            keys.iter()
                .map(|k| format!("{:?}", c.get(*k)))
                .collect::<Vec<_>>()
                .join(","),
            light_taskbar()
        );
        let redrawn = force || self.sig.as_deref() != Some(&sig) || self.img.is_none();
        if redrawn {
            self.build(&cache, &tb);
            self.sig = Some(sig);
        }
        let Some(cc) = self.content_center(&tb) else {
            return self.hide();
        };
        let pos = self.win_topleft(cc.0, cc.1);
        if redrawn || Some(pos) != self.pos || !self.visible {
            self.present(pos.0, pos.1);
        }
        // SAFETY: our own window; topmost without activating, shown.
        unsafe {
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
        };
        self.visible = true;
        self.update_tip(&cache);
    }

    // ---- hit testing ----
    fn to_frame(&self, x: i32, y: i32) -> (f64, f64) {
        let (g, img) = (self.geo.as_ref().unwrap(), self.img.as_ref().unwrap());
        let (ux, uy) = rot(
            x as f64 - img.w as f64 / 2.0,
            y as f64 - img.h as f64 / 2.0,
            -self.built_angle,
        );
        (ux + g.wp / 2.0, uy + g.hp / 2.0)
    }

    fn inside_body(&self, px: f64, py: f64) -> bool {
        let g = self.geo.as_ref().unwrap();
        g.m <= px && px <= g.m + g.w && g.mt <= py && py <= g.mt + g.h
    }

    fn hit(&self, x: i32, y: i32) -> Option<&'static str> {
        let (g, _) = (self.geo.as_ref()?, self.img.as_ref()?);
        let c = self.cfg.as_ref()?;
        let (px, py) = self.to_frame(x, y);
        if !flag(c, "widget_locked", true) {
            for (name, (hx, hy), r) in [
                ("lock", g.lock, 12.0),
                ("rot", g.rot, 11.0),
                ("size", g.size, 11.0),
            ] {
                if (px - hx).powi(2) + (py - hy).powi(2) <= (r * self.scale).powi(2) {
                    return Some(name);
                }
            }
        }
        self.inside_body(px, py).then_some("body")
    }

    fn mouse_screen(&self, lp: LPARAM) -> (i32, i32) {
        (self.org.0 + lo(lp), self.org.1 + hi(lp))
    }

    fn begin_drag(&mut self, kind: &'static str, sx: i32, sy: i32) {
        let s = self.st();
        let tb = self.taskbar();
        let cc = self.content_center(&tb).unwrap_or((sx as f64, sy as f64));
        self.drag = Some(Drag {
            kind,
            sx,
            sy,
            cx0: cc.0,
            cy0: cc.1,
            s0: s.size,
            moved: false,
            d0: (sx as f64 - cc.0).hypot(sy as f64 - cc.1).max(1.0),
        });
        self.live = Some(Live {
            cx: Some(cc.0),
            cy: Some(cc.1),
            ..Default::default()
        });
        // SAFETY: capture for our own window.
        unsafe { SetCapture(self.hwnd) };
    }

    fn do_drag(&mut self, sx: i32, sy: i32) {
        let Some(d) = self.drag.as_mut() else { return };
        if (sx - d.sx).abs() + (sy - d.sy).abs() > 2 {
            d.moved = true;
        }
        if !d.moved {
            return;
        }
        let (kind, cx0, cy0, s0, d0, ox, oy) = (d.kind, d.cx0, d.cy0, d.s0, d.d0, d.sx, d.sy);
        let live = self.live.get_or_insert_with(Live::default);
        match kind {
            "body" => {
                live.cx = Some(cx0 + (sx - ox) as f64);
                live.cy = Some(cy0 + (sy - oy) as f64);
                self.tick(false);
            }
            "size" => {
                live.size =
                    Some((s0 * (sx as f64 - cx0).hypot(sy as f64 - cy0) / d0).clamp(20.0, 90.0));
                self.tick(true);
            }
            "rot" => {
                let mut ang = (sx as f64 - cx0).atan2(-(sy as f64 - cy0)).to_degrees();
                for k in [-180.0, -90.0, 0.0, 90.0, 180.0] {
                    if (ang - k).abs() < 5.0 {
                        ang = k;
                    }
                }
                live.angle = Some(if ang <= -180.0 {
                    180.0
                } else {
                    (ang * 10.0).round() / 10.0
                });
                self.tick(true);
            }
            _ => {}
        }
    }

    fn end_drag(&mut self) {
        let d = self.drag.take();
        let live = self.live.take();
        // SAFETY: releases our own capture.
        unsafe { ReleaseCapture() };
        if let (Some(d), Some(l)) = (d, live) {
            if d.moved {
                let mut kv: Vec<(&str, Value)> = vec![];
                if let Some(v) = l.cx {
                    kv.push(("cx", json!(v)));
                }
                if let Some(v) = l.cy {
                    kv.push(("cy", json!(v)));
                }
                if let Some(v) = l.size {
                    kv.push(("size", json!(v)));
                }
                if let Some(v) = l.angle {
                    kv.push(("angle", json!(v)));
                }
                self.save_state(&kv);
            }
        }
        self.sig = None;
    }

    fn post_tray(&self, id: usize) {
        // SAFETY: posting to the tray window of this process.
        unsafe { PostMessageW(self.tray as HWND, WM_COMMAND, id, 0) };
    }

    fn menu(&mut self, hwnd: HWND) {
        let locked = flag(self.cfg(), "widget_locked", true);
        let style = self
            .cfg()
            .get("widget_style")
            .and_then(Value::as_str)
            .unwrap_or("rings")
            .to_string();
        let style_name = STYLE_NAMES
            .iter()
            .find(|(k, _)| *k == style)
            .map_or("?", |e| e.1);
        let items: Vec<(usize, String)> = vec![
            (1, "Open dashboard".into()),
            (2, "Refresh now".into()),
            (
                5,
                if locked {
                    "Unlock to move / resize / rotate"
                } else {
                    "Lock widget"
                }
                .into(),
            ),
            (6, "Reset position, size and angle".into()),
            (7, "Switch layout (vertical / horizontal)".into()),
            (8, format!("Next style  (now: {style_name})")),
            (3, "Hide widget".into()),
            (4, "Quit".into()),
        ];
        // SAFETY: popup menu owned by this call; destroyed before returning.
        let cmd = unsafe {
            let m = CreatePopupMenu();
            for (id, t) in &items {
                AppendMenuW(m, 0, *id, wide(t).as_ptr());
            }
            let mut pt = POINT { x: 0, y: 0 };
            GetCursorPos(&mut pt);
            SetForegroundWindow(hwnd);
            let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_NONOTIFY, pt.x, pt.y, 0, hwnd, null());
            DestroyMenu(m);
            cmd as usize
        };
        self.run_command(cmd);
    }

    /// Menu command (1 open, 2 refresh, 3 hide, 4 quit, 5 lock toggle, 6 reset, 7 layout, 8 next style).
    fn run_command(&mut self, cmd: usize) {
        let mut cfg = self.load_cfg();
        let locked = flag(&cfg, "widget_locked", true);
        match cmd {
            1 => return self.post_tray(1),
            2 => return self.post_tray(2),
            4 => return self.post_tray(6),
            3 => {
                cfg.insert("widget_enabled".into(), json!(false));
            }
            5 => {
                cfg.insert("widget_locked".into(), json!(!locked));
            }
            6 => {
                cfg.insert("widget_cx".into(), Value::Null);
                cfg.insert("widget_cy".into(), Value::Null);
                cfg.insert("widget_angle".into(), json!(0.0));
                cfg.insert("widget_size".into(), json!(36));
                cfg.insert("widget_layout".into(), json!("auto"));
            }
            7 => {
                let l = if self.vertical {
                    "horizontal"
                } else {
                    "vertical"
                };
                cfg.insert("widget_layout".into(), json!(l));
            }
            8 => {
                let cur = cfg
                    .get("widget_style")
                    .and_then(Value::as_str)
                    .unwrap_or("rings");
                let i = STYLES.iter().position(|s| *s == cur).unwrap_or(0);
                cfg.insert("widget_style".into(), json!(STYLES[(i + 1) % STYLES.len()]));
            }
            _ => return,
        }
        self.save_cfg(cfg);
        self.sig = None;
    }

    fn handle(&mut self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        if msg == WM_TIMER {
            if self.drag.is_none() {
                self.tick(false);
            }
            return Some(0);
        }
        #[cfg(any(debug_assertions, feature = "e2e"))]
        if msg == WM_APP + 21 {
            self.dump();
            return Some(0);
        }
        if msg == WM_APP + 20 {
            // test/automation hook: run a menu command without a modal menu
            self.run_command(wp);
            self.tick(true);
            return Some(0);
        }
        let (x, y) = (lo(lp), hi(lp));
        let edit = self
            .cfg
            .as_ref()
            .is_some_and(|c| !flag(c, "widget_locked", true));
        match msg {
            WM_LBUTTONDOWN => {
                self.down_hit = self.hit(x, y);
                if edit {
                    if let Some(h @ ("body" | "rot" | "size")) = self.down_hit {
                        let (sx, sy) = self.mouse_screen(lp);
                        self.begin_drag(h, sx, sy);
                    }
                }
                Some(0)
            }
            WM_MOUSEMOVE => {
                if self.drag.is_some() {
                    let (sx, sy) = self.mouse_screen(lp);
                    self.do_drag(sx, sy);
                    return Some(0);
                }
                self.last_hit = self.hit(x, y);
                if !edit && self.geo.is_some() {
                    let (px, py) = self.to_frame(x, y);
                    let g = self.geo.unwrap();
                    let inside = self.inside_body(px, py);
                    let i = if !inside {
                        -1
                    } else if self.vertical {
                        ((py - g.mt) / self.cell_h.max(1.0)) as i64
                    } else {
                        ((px - g.m) / self.cell_w.max(1.0)) as i64
                    };
                    self.mouse_pt = self.mouse_screen(lp);
                    if i != self.hover {
                        self.hover = i;
                        self.hover_t = Instant::now();
                        self.tick(false);
                    }
                }
                let mut t = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                // tests send synthetic moves with the real pointer elsewhere: Windows would answer with an
                // immediate WM_MOUSELEAVE, so they switch the tracking off
                if std::env::var_os("CU_WIDGET_NO_TRACK").is_none() {
                    // SAFETY: valid TRACKMOUSEEVENT for our window.
                    unsafe { TrackMouseEvent(&mut t) };
                }
                Some(0)
            }
            WM_MOUSELEAVE => {
                if self.hover != -1 {
                    self.hover = -1;
                    self.tick(false);
                }
                Some(0)
            }
            WM_LBUTTONUP => {
                if self.drag.is_some() {
                    self.end_drag();
                    self.tick(true);
                    return Some(0);
                }
                let h = self.hit(x, y);
                if edit && h == Some("lock") && self.down_hit == Some("lock") {
                    self.save_state(&[("locked", json!(true))]);
                    self.tick(true);
                } else if !edit && h == Some("body") {
                    self.post_tray(1);
                }
                Some(0)
            }
            WM_CAPTURECHANGED => {
                if self.drag.is_some() {
                    self.drag = None;
                    self.live = None;
                    self.sig = None;
                }
                Some(0)
            }
            WM_SETCURSOR => {
                let id: u16 = match if edit { self.last_hit } else { None } {
                    Some("body") => 32646,
                    Some("size") => 32642,
                    _ => 32649,
                };
                // SAFETY: stock cursor.
                unsafe { SetCursor(LoadCursorW(null_mut(), id as usize as *const u16)) };
                Some(1)
            }
            WM_RBUTTONUP => {
                self.menu(hwnd);
                Some(0)
            }
            _ => None,
        }
    }
}

/// Test-only seams (debug builds or `--features e2e`; NOT in the shipped release):
/// * `WM_APP+21` writes `widget_state.json` (+ `widget.png`, `widget_tip.png`) into the data dir, synchronously;
/// * `fake_taskbar.txt` (`l t r b [tl tt tr tb]`) replaces the shell taskbar/tray rectangles;
/// * `fake_fullscreen.flag` makes the widget behave as if a fullscreen app were in front.
#[cfg(any(debug_assertions, feature = "e2e"))]
impl Widget {
    fn fake_taskbar(&self) -> Option<Option<Taskbar>> {
        let t = std::fs::read_to_string(self.paths.data.join("fake_taskbar.txt")).ok()?;
        let v: Vec<i32> = t
            .split_whitespace()
            .filter_map(|x| x.parse().ok())
            .collect();
        if v.len() == 4 || v.len() == 8 {
            let r = RECT {
                left: v[0],
                top: v[1],
                right: v[2],
                bottom: v[3],
            };
            let tray = (v.len() == 8).then(|| RECT {
                left: v[4],
                top: v[5],
                right: v[6],
                bottom: v[7],
            });
            return Some(Some(Taskbar {
                r,
                tray,
                vertical: (r.right - r.left) < (r.bottom - r.top),
            }));
        }
        Some(None) // empty / malformed file = "no taskbar"
    }

    fn dump(&self) {
        fn fnv(b: &[u8]) -> String {
            let mut h = 0xcbf2_9ce4_8422_2325u64;
            for &x in b {
                h = (h ^ x as u64).wrapping_mul(0x0100_0000_01b3);
            }
            format!("{h:016x}")
        }
        let cfg = self.cfg.clone().unwrap_or_default();
        let img = self.img.as_ref();
        let tip = self.tip_img.as_ref().filter(|_| self.tip_visible);
        let g = self.geo.as_ref();
        let v = json!({
            "visible": self.visible,
            "org": [self.org.0, self.org.1],
            "img": img.map(|i| json!([i.w, i.h])),
            "img_hash": img.map(|i| fnv(&i.bgra)),
            "vertical": self.vertical,
            "cells": self.view.iter().map(|c| c.0.clone()).collect::<Vec<_>>(),
            "more": self.view.iter().find(|c| c.0 == MORE).and_then(|c| c.1.get("more").cloned()),
            "hidden_cells": self.view_hidden.len(),
            "plan_size": self.plan.0,
            "plan_shown": self.plan.1,
            "cell_w": self.cell_w,
            "cell_h": self.cell_h,
            "frame": g.map(|g| json!({"m": g.m, "mt": g.mt, "w": g.w, "h": g.h, "wp": g.wp, "hp": g.hp, "lock": [g.lock.0, g.lock.1], "rot": [g.rot.0, g.rot.1], "size": [g.size.0, g.size.1]})),
            "built_angle": self.built_angle,
            "fast": self.fast,
            "phase": self.phase,
            "hover": self.hover,
            "tip_visible": self.tip_visible,
            "tip_pos": self.tip_pos.map(|p| json!([p.0, p.1])),
            "tip_size": tip.map(|i| json!([i.w, i.h])),
            "tip_hash": tip.map(|i| fnv(&i.bgra)),
            "tip_is_more": self.tip_key.as_deref().is_some_and(|k| k.starts_with(MORE)),
            "alpha": opacity_alpha(&cfg),
            "light": is_light(&cfg),
            "scale": self.scale,
            "screen": [screen().0, screen().1],
            "limit": stack_limit(&cfg, self.vertical, self.st().cx.is_none(), screen()),
        });
        let _ = std::fs::write(self.paths.data.join("widget_state.json"), v.to_string());
        if let Some(i) = img {
            let _ = super::write_png(&self.paths.data.join("widget.png"), i, [32, 32, 36]);
        }
        if let Some(i) = tip {
            let _ = super::write_png(&self.paths.data.join("widget_tip.png"), i, [32, 32, 36]);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let r = STATE.with(|s| match s.try_borrow_mut() {
        Ok(mut g) => match g.as_mut() {
            Some(w) => {
                catch_unwind(AssertUnwindSafe(|| w.handle(hwnd, msg, wp, lp))).unwrap_or(None)
            }
            None => None,
        },
        Err(_) => None,
    });
    match r {
        Some(v) => v,
        // SAFETY: default processing for everything we do not handle.
        None => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Start the widget thread. `tray_hwnd` receives the commands the widget cannot handle itself.
pub fn spawn(paths: Paths, tray_hwnd: isize) {
    std::thread::Builder::new()
        .name("widget".into())
        .spawn(move || {
            let p2 = paths.clone();
            if catch_unwind(AssertUnwindSafe(|| run(paths, tray_hwnd))).is_err() {
                log(&p2, "widget thread crashed");
            }
        })
        .ok();
}

fn run(paths: Paths, tray: isize) {
    // SAFETY: Win32 window setup and the message loop of this dedicated thread.
    unsafe {
        let hinst = GetModuleHandleW(null());
        let (cls, tcls) = (wide(CLASS), wide(TIP_CLASS));
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = hinst;
        wc.lpszClassName = cls.as_ptr();
        RegisterClassW(&wc);
        let mut tc: WNDCLASSW = std::mem::zeroed();
        tc.lpfnWndProc = Some(DefWindowProcW);
        tc.hInstance = hinst;
        tc.lpszClassName = tcls.as_ptr();
        RegisterClassW(&tc);
        // layered | topmost | toolwindow (no taskbar button) | noactivate
        let ex = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        let hwnd = CreateWindowExW(
            ex,
            cls.as_ptr(),
            wide("claude-usage-widget").as_ptr(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        // same flags + WS_EX_TRANSPARENT: the card never steals the mouse from the widget
        let tip = CreateWindowExW(
            ex | WS_EX_TRANSPARENT,
            tcls.as_ptr(),
            wide("claude-usage-tip").as_ptr(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        if hwnd.is_null() || tip.is_null() {
            log(&paths, "widget: cannot create windows");
            return;
        }
        let w = Widget {
            paths,
            tray,
            hwnd,
            tip,
            cfg: None,
            cfg_m: None,
            sig: None,
            hover: -1,
            visible: false,
            live: None,
            drag: None,
            last_hit: None,
            down_hit: None,
            geo: None,
            img: None,
            cell_w: 1.0,
            cell_h: 1.0,
            vertical: true,
            built_angle: 0.0,
            org: (0, 0),
            pos: None,
            scale: 1.0,
            phase: 0,
            fast: false,
            tip_visible: false,
            tip_key: None,
            tip_img: None,
            tip_pos: None,
            hover_t: Instant::now(),
            mouse_pt: (0, 0),
            plan: (36.0, 0),
            plan_key: None,
            view: vec![],
            view_hidden: vec![],
        };
        STATE.with(|s| *s.borrow_mut() = Some(w));
        SetTimer(hwnd, 1, 1000, None);
        // first frame right away instead of after one second
        STATE.with(|s| {
            if let Ok(mut g) = s.try_borrow_mut() {
                if let Some(w) = g.as_mut() {
                    let _ = catch_unwind(AssertUnwindSafe(|| w.tick(false)));
                }
            }
        });
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
