//! Widget end-to-end checks ported from `tests/e2e_widget.py` (the real widget inside the real tray process).
//! Sandboxed LOCALAPPDATA/USERPROFILE, fake cache with fresh `fetched_at` and a huge poll interval, so the daemon
//! never polls and never rewrites the cache the test writes. No token, no network, no registry/hook writes.
//!
//! Observation channel: the TEST-ONLY seams of the widget (debug builds / `--features e2e`; NOT in the shipped
//! release): `WM_APP+21` makes the widget write `widget_state.json` (+ `widget.png`) and answer synchronously;
//! `fake_taskbar.txt` replaces the shell taskbar rectangles; `fake_fullscreen.flag` fakes a fullscreen app.
//! The mouse is simulated with window messages (the real pointer is elsewhere; `CU_WIDGET_NO_TRACK` stops Windows
//! answering a synthetic move with an immediate WM_MOUSELEAVE).
//!
//! ONE test function on purpose (GUI tests must not run concurrently).
#![cfg(any(debug_assertions, feature = "e2e"))]

use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetCursorPos, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible,
    SendMessageW, SetCursorPos, WM_APP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
};

const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_CMD: u32 = WM_APP + 20;
const WM_DUMP: u32 = WM_APP + 21;
const CMD_LOCK: usize = 5;

// ------------------------------------------------------------------------------------------ plumbing

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn find(class: &str, pid: Option<u32>) -> usize {
    let c = wide(class);
    let mut prev = std::ptr::null_mut();
    loop {
        // SAFETY: window enumeration by class with NUL-terminated name.
        let h = unsafe { FindWindowExW(std::ptr::null_mut(), prev, c.as_ptr(), std::ptr::null()) };
        if h.is_null() {
            return 0;
        }
        let mut p = 0u32;
        // SAFETY: valid window handle, valid out pointer.
        unsafe { GetWindowThreadProcessId(h, &mut p) };
        if pid.is_none_or(|x| x == p) {
            return h as usize;
        }
        prev = h;
    }
}

fn send(h: usize, msg: u32, wp: usize, lp: isize) {
    // SAFETY: cross-process SendMessage to our own windows (blocks until handled).
    unsafe { SendMessageW(h as _, msg, wp, lp) };
}

fn mouse(x: i32, y: i32) -> isize {
    ((y as isize & 0xFFFF) << 16) | (x as isize & 0xFFFF)
}

fn win_visible(h: usize) -> bool {
    h != 0 && unsafe { IsWindowVisible(h as _) != 0 }
}

fn win_rect(h: usize) -> (i32, i32, i32, i32) {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: valid window, valid out pointer.
    unsafe { GetWindowRect(h as _, &mut r) };
    (r.left, r.top, r.right, r.bottom)
}

fn sleep(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn ymd(secs: i64) -> String {
    // civil-from-days (Howard Hinnant)
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}+00:00",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn now_s() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

fn iso(off: i64) -> String {
    ymd(now_s() as i64 + off)
}

struct Guard(Child);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The test posts synthetic mouse messages, but with the widget's mouse capture the system also delivers a REAL
/// WM_MOUSEMOVE whenever the (moving/rotating) window ends up under the real pointer, which corrupts the drag.
/// Park the real pointer in a screen corner no widget geometry reaches, restore it afterwards.
struct ParkedCursor(POINT);
impl ParkedCursor {
    fn park() -> Self {
        let mut p = POINT { x: 0, y: 0 };
        // SAFETY: plain queries/moves of the pointer position.
        unsafe {
            GetCursorPos(&mut p);
            SetCursorPos(2, 2);
        }
        ParkedCursor(p)
    }
}
impl Drop for ParkedCursor {
    fn drop(&mut self) {
        // SAFETY: restores the pointer.
        unsafe { SetCursorPos(self.0.x, self.0.y) };
    }
}

fn rot(x: f64, y: f64, ang: f64) -> (f64, f64) {
    let a = ang.to_radians();
    (x * a.cos() - y * a.sin(), x * a.sin() + y * a.cos())
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

struct Rig {
    data: PathBuf,
    cfgp: PathBuf,
    names: Vec<String>,
    w: usize,
    tray: usize,
    pid: u32,
}

const NAMES: [&str; 14] = [
    "default (desktop)",
    "claude-da",
    "claude-hn",
    "claude-ngoai",
    "claude-team-05",
    "claude-team-06",
    "claude-team-07",
    "claude-team-08",
    "claude-team-09",
    "claude-team-10",
    "claude-team-11",
    "claude-team-12",
    "claude-team-13",
    "claude-team-14",
];

fn profile_data(i: usize) -> Value {
    json!({
        "five": ((i * 17 + 30) % 100) as f64, "week": ((i * 23 + 40) % 100) as f64,
        "five_reset": iso(3600 * (i as i64 % 5 + 1)), "week_reset": iso(86400 * (i as i64 % 6 + 1)),
        "fetched_at": now_s(), "plan": "team"
    })
}

impl Rig {
    fn cache(&self, mutate: impl Fn(usize, &mut Value), busy: &[&str]) {
        let mut profs = Map::new();
        for (i, n) in self.names.iter().enumerate() {
            let mut d = profile_data(i);
            if i == 0 {
                d["extra"] = json!({"used": 0.98, "limit": 25.0, "pct": 3.92, "currency": "USD"});
                d["scoped"] = json!([{"name": "Fable", "pct": 21, "resets_at": iso(86400)}]);
            }
            mutate(i, &mut d);
            profs.insert(n.clone(), d);
        }
        let c = json!({"ts": now_s(), "block_enabled": false, "block_pct": 95, "warn_pct": 80, "busy": busy, "profiles": profs});
        let tmp = self.data.join("cache.tmp");
        fs::write(&tmp, c.to_string()).unwrap();
        fs::rename(&tmp, self.data.join("usage_cache.json")).unwrap();
    }
    fn cfg(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(&self.cfgp).unwrap_or_default())
            .unwrap_or(Value::Null)
    }
    fn set(&self, kv: Value) {
        let mut c = self.cfg();
        for (k, v) in kv.as_object().unwrap() {
            c[k] = v.clone();
        }
        fs::write(&self.cfgp, c.to_string()).unwrap();
    }
    fn dump(&self) -> Value {
        send(self.w, WM_DUMP, 0, 0);
        serde_json::from_str(&fs::read_to_string(self.data.join("widget_state.json")).unwrap())
            .unwrap()
    }
    fn until(&self, what: &str, secs: u64, mut pred: impl FnMut(&Value) -> bool) -> Value {
        let t = Instant::now();
        loop {
            let d = self.dump();
            if pred(&d) {
                return d;
            }
            assert!(
                t.elapsed() < Duration::from_secs(secs),
                "timeout waiting for: {what}; last state: {d}"
            );
            sleep(100);
        }
    }
    fn cfg_until(&self, what: &str, secs: u64, mut pred: impl FnMut(&Value) -> bool) -> Value {
        let t = Instant::now();
        loop {
            let c = self.cfg();
            if pred(&c) {
                return c;
            }
            assert!(
                t.elapsed() < Duration::from_secs(secs),
                "timeout waiting for config: {what}; config: {c}"
            );
            sleep(100);
        }
    }
    fn log(&self) -> String {
        fs::read_to_string(self.data.join("events.log")).unwrap_or_default()
    }
    /// Move the pointer over the centre of cell `i` (vertical stack unless `horizontal`).
    fn hover_cell(&self, i: usize, horizontal: bool) {
        let d = self.dump();
        let fr = &d["frame"];
        let (cw, ch) = (f(&d["cell_w"]), f(&d["cell_h"]));
        let (x, y) = if horizontal {
            (f(&fr["m"]) + (i as f64 + 0.5) * cw, f(&fr["mt"]) + ch / 2.0)
        } else {
            (f(&fr["m"]) + cw / 2.0, f(&fr["mt"]) + (i as f64 + 0.5) * ch)
        };
        send(self.w, WM_MOUSEMOVE, 0, mouse(x as i32, y as i32));
    }
    fn leave(&self) {
        send(self.w, WM_MOUSELEAVE, 0, 0);
    }
    fn tip(&self) -> bool {
        win_visible(find("ClaudeUsageTip", Some(self.pid)))
    }
    /// Content centre (screen px) of the body, from a state dump (Python `content_center` after build).
    fn center(d: &Value) -> (f64, f64) {
        let (org, img, fr) = (&d["org"], &d["img"], &d["frame"]);
        let (dx, dy) = rot(
            f(&fr["m"]) + f(&fr["w"]) / 2.0 - f(&fr["wp"]) / 2.0,
            f(&fr["mt"]) + f(&fr["h"]) / 2.0 - f(&fr["hp"]) / 2.0,
            f(&d["built_angle"]),
        );
        (
            f(&org[0]) + f(&img[0]) / 2.0 + dx,
            f(&org[1]) + f(&img[1]) / 2.0 + dy,
        )
    }
    /// Screen position of a point given in frame coordinates (Python `screen_of`).
    fn screen_of(d: &Value, fx: f64, fy: f64) -> (f64, f64) {
        let (org, img, fr) = (&d["org"], &d["img"], &d["frame"]);
        let (rx, ry) = rot(
            fx - f(&fr["wp"]) / 2.0,
            fy - f(&fr["hp"]) / 2.0,
            f(&d["built_angle"]),
        );
        (
            f(&org[0]) + f(&img[0]) / 2.0 + rx,
            f(&org[1]) + f(&img[1]) / 2.0 + ry,
        )
    }
    fn point(d: &Value, kind: &str) -> (f64, f64) {
        let fr = &d["frame"];
        let (fx, fy) = match kind {
            "body" => (
                f(&fr["m"]) + f(&fr["w"]) / 2.0,
                f(&fr["mt"]) + f(&fr["h"]) / 2.0,
            ),
            k => (f(&fr[k][0]), f(&fr[k][1])),
        };
        Rig::screen_of(d, fx, fy)
    }
    fn post(&self, msg: u32, sx: f64, sy: f64) {
        let d = self.dump(); // the window moves while dragging: convert with the current origin
        let (ox, oy) = (f(&d["org"][0]), f(&d["org"][1]));
        send(
            self.w,
            msg,
            usize::from(msg == WM_LBUTTONDOWN),
            mouse((sx - ox).round() as i32, (sy - oy).round() as i32),
        );
        sleep(60);
    }
    fn drag(&self, kind: &str, dx: f64, dy: f64) -> Value {
        let d = self.dump();
        let (sx, sy) = Rig::point(&d, kind);
        self.post(WM_LBUTTONDOWN, sx, sy);
        for i in 1..=6 {
            self.post(
                WM_MOUSEMOVE,
                sx + dx * i as f64 / 6.0,
                sy + dy * i as f64 / 6.0,
            );
        }
        self.post(WM_LBUTTONUP, sx + dx, sy + dy);
        sleep(400);
        self.cfg()
    }
    fn click(&self, kind: &str) {
        let d = self.dump();
        let (sx, sy) = Rig::point(&d, kind);
        self.post(WM_LBUTTONDOWN, sx, sy);
        self.post(WM_LBUTTONUP, sx, sy);
        sleep(300);
    }
}

fn along(d: &Value) -> f64 {
    if d["vertical"] == true {
        f(&d["img"][1])
    } else {
        f(&d["img"][0])
    }
}

fn spawn(local: &Path, home: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_claude-usage"))
        .arg("--hidden")
        .env("LOCALAPPDATA", local)
        .env("USERPROFILE", home)
        .env_remove("HOMEDRIVE")
        .env_remove("HOMEPATH")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("CU_NO_AUTOSTART", "1")
        .env("CU_NO_REPAIR", "1")
        .env("CU_NO_DASHBOARD", "1")
        .env("CU_WIDGET_NO_TRACK", "1")
        .spawn()
        .expect("spawn exe")
}

// ------------------------------------------------------------------------------------------ the test

#[test]
fn widget_old_e2e_assertions() {
    assert_eq!(
        find("ClaudeUsageTray", None),
        0,
        "another Rust tray is running: quit it first"
    );
    let _cursor = ParkedCursor::park();
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let data = local.join("claude-usage");
    fs::create_dir_all(&data).unwrap();
    let names: Vec<String> = NAMES.iter().map(|s| s.to_string()).collect();
    let profiles: Vec<Value> = names
        .iter()
        .map(|n| json!({"name": n, "path": home.join(n).to_string_lossy()}))
        .collect();
    let cfgp = data.join("config.json");
    let mut rig = Rig {
        data: data.clone(),
        cfgp: cfgp.clone(),
        names,
        w: 0,
        tray: 0,
        pid: 0,
    };
    rig.cache(|_, _| {}, &[]); // fresh data: the daemon seeds from it and polls nothing for an hour
    fs::write(
        &cfgp,
        json!({"autostart": false, "notify_enabled": false, "interval_sec": 3600, "profiles": profiles,
               "widget_enabled": true, "widget_locked": true, "widget_style": "rings", "widget_theme": "dark",
               "widget_layout": "vertical", "widget_tooltip": true, "widget_anchor": "tray",
               "widget_cx": 400.0, "widget_cy": 300.0, "widget_hidden": rig.names[4..].to_vec()})
        .to_string(),
    )
    .unwrap();
    let child = spawn(&local, &home);
    rig.pid = child.id();
    let _guard = Guard(child);
    let t0 = Instant::now();
    while !win_visible(find("ClaudeUsageWidget", Some(rig.pid))) {
        assert!(
            t0.elapsed() < Duration::from_secs(20),
            "widget window did not appear"
        );
        sleep(100);
    }
    rig.w = find("ClaudeUsageWidget", Some(rig.pid));
    rig.tray = find("ClaudeUsageTray", Some(rig.pid));
    let d = rig.dump();
    let (sw, sh) = (f(&d["screen"][0]), f(&d["screen"][1]));
    let scale = f(&d["scale"]);
    println!("screen {sw}x{sh} scale {scale}");
    let wide_room = sw >= 900.0 && sh >= 600.0;
    assert!(
        wide_room,
        "the test needs a screen of at least 900x600 (got {sw}x{sh})"
    );
    let pos =
        |fx: f64, fy: f64| json!({"widget_cx": (sw * fx).round(), "widget_cy": (sh * fy).round()});

    // ---- 4 visible accounts, explicit position on the right half
    rig.set(pos(0.75, 0.5));
    let d = rig.until("widget at the configured centre", 5, |d| {
        let c = Rig::center(d);
        (c.0 - (sw * 0.75).round()).abs() <= 2.0 && (c.1 - (sh * 0.5).round()).abs() <= 2.0
    });
    assert_eq!(d["cells"].as_array().unwrap().len(), 4, "{d}");
    assert_eq!(d["visible"], true);
    assert!(f(&d["img"][0]) > 20.0 && f(&d["img"][1]) > 20.0);
    assert_eq!(d["light"], false);
    assert_eq!(d["alpha"], 255);

    // old: "changing the style in config redraws the live widget (all 5 distinct)"
    let mut hashes = std::collections::HashSet::new();
    for st in ["rings", "bars", "cards", "segments", "arc"] {
        rig.set(json!({"widget_style": st}));
        sleep(1500); // the widget polls its config once a second
        hashes.insert(rig.dump()["img_hash"].as_str().unwrap().to_string());
    }
    assert_eq!(hashes.len(), 5, "all 5 styles draw differently");

    // light / dark theme and opacity (old: "opacity setting is applied to the window alpha")
    rig.set(json!({"widget_style": "rings"}));
    sleep(1300);
    let dark = rig.dump();
    rig.set(json!({"widget_theme": "light"}));
    let light = rig.until("light theme", 5, |d| d["light"] == true);
    assert_ne!(dark["img_hash"], light["img_hash"], "light theme redraws");
    rig.set(json!({"widget_theme": "dark", "widget_opacity": 50}));
    rig.until("opacity 50", 5, |d| d["alpha"] == 127);
    rig.set(json!({"widget_opacity": 100}));
    rig.until("opacity back", 5, |d| d["alpha"] == 255);

    // old: "hiding an account shortens the live widget" / "custom name / font / plate redraw"
    let all4 = rig.dump();
    rig.set(json!({"widget_hidden": rig.names[3..].to_vec()}));
    let hid = rig.until("one account hidden", 5, |d| {
        d["cells"].as_array().unwrap().len() == 3
    });
    assert!(
        f(&hid["img"][1]) < f(&all4["img"][1]) * 0.85,
        "{} -> {}",
        all4["img"][1],
        hid["img"][1]
    );
    rig.set(json!({"widget_hidden": rig.names[4..].to_vec(), "aliases": {"default (desktop)": "Chính"}, "widget_label": "short",
                   "widget_font": 130, "widget_plate": true}));
    let styled = rig.until("alias/font/plate", 5, |d| d["img_hash"] != all4["img_hash"]);
    assert!(
        f(&styled["img"][0]) <= 60.0 * scale,
        "short labels keep the widget narrow: {}",
        styled["img"][0]
    );
    rig.set(
        json!({"aliases": {}, "widget_label": "alias", "widget_font": 100, "widget_plate": false}),
    );
    sleep(1300);

    // old: "stale / error cells and layouts render" + horizontal layout + "'hide widget' really hides it"
    rig.cache(
        |i, d| match i {
            2 => {
                d["err"] = json!("HTTP 429");
                d["retry_at"] = json!(now_s() + 100.0);
            }
            3 => *d = json!({"err": "HTTP 429", "status": 429}),
            _ => {}
        },
        &[],
    );
    rig.set(json!({"widget_layout": "horizontal"}));
    let hz = rig.until("horizontal layout with stale + error cells", 6, |d| {
        d["vertical"] == false && d["cells"].as_array().unwrap().len() == 4
    });
    assert!(
        f(&hz["img"][0]) > f(&hz["img"][1]),
        "wider than tall: {}",
        hz["img"]
    );
    rig.set(json!({"widget_enabled": false}));
    rig.until("hidden", 5, |d| d["visible"] == false);
    assert!(!win_visible(rig.w));
    rig.set(json!({"widget_enabled": true, "widget_layout": "vertical"}));
    rig.until("shown again", 5, |d| {
        d["visible"] == true && d["vertical"] == true
    });
    rig.cache(|_, _| {}, &[]);

    // old: waiting indicator (fast timer only while something is fetched; spinner moves)
    sleep(1500); // let the widget pick up the cache written above
    let idle = rig.until("idle: no fast animation timer (no wasted CPU)", 6, |d| {
        d["fast"] == false && d["visible"] == true
    });
    sleep(1200);
    let idle2 = rig.dump();
    assert_eq!(
        (&idle2["fast"], &idle2["img_hash"]),
        (&json!(false), &idle["img_hash"]),
        "idle widget is stable: {idle} -> {idle2}"
    );
    rig.cache(|_, _| {}, &["claude-da"]);
    let a = rig.until("fast animation timer while fetching", 5, |d| {
        d["fast"] == true
    });
    sleep(500);
    let b = rig.dump();
    assert_ne!(a["img_hash"], idle["img_hash"], "spinner is drawn");
    assert_ne!(
        a["img_hash"], b["img_hash"],
        "spinner moves (phases {} -> {})",
        a["phase"], b["phase"]
    );
    rig.cache(|_, _| {}, &[]);
    rig.until("fast timer stops when idle", 5, |d| d["fast"] == false);

    // old: hover maps to the right card on a card-style widget (1st, then 2nd) + hover card + delay + position
    rig.set(json!({"widget_style": "cards", "widget_layout": "vertical"}));
    rig.until("cards style", 6, |d| {
        d["cells"].as_array().unwrap().len() == 4 && f(&d["cell_h"]) > 30.0
    });
    assert!(!rig.tip(), "no card before hovering");
    rig.hover_cell(0, false);
    let h0 = rig.dump();
    assert_eq!(h0["hover"], 0, "{h0}");
    rig.hover_cell(1, false);
    assert_eq!(rig.dump()["hover"], 1, "second card");
    rig.hover_cell(0, false);
    assert_eq!(
        rig.dump()["tip_visible"],
        false,
        "card is not shown immediately (hover delay)"
    );
    sleep(150);
    assert!(!rig.tip(), "still no card 150 ms after the pointer arrived");
    let t = rig.until("card appears after the hover delay", 4, |d| {
        d["tip_visible"] == true
    });
    assert!(rig.tip());
    let wr = (
        f(&t["org"][0]),
        f(&t["org"][1]),
        f(&t["img"][0]),
        f(&t["img"][1]),
    );
    let (tp, ts) = (&t["tip_pos"], &t["tip_size"]);
    let (tl, tt, tr, tb) = (
        f(&tp[0]),
        f(&tp[1]),
        f(&tp[0]) + f(&ts[0]),
        f(&tp[1]) + f(&ts[1]),
    );
    assert!(tr <= wr.0 + 1.0 && tl >= 0.0 && tt >= 0.0 && tb <= sh, "card sits beside the widget (left, widget is on the right half) and on screen: card ({tl},{tt},{tr},{tb}) widget x {}", wr.0);
    assert_eq!(t["fast"], true, "animation timer runs while hovering");
    let g = win_rect(find("ClaudeUsageTip", Some(rig.pid)));
    assert!(
        (g.0 as f64 - tl).abs() <= 1.0 && (g.1 as f64 - tt).abs() <= 1.0,
        "tip window really placed at the reported position"
    );
    let tip0 = t["tip_hash"].clone();
    let h0h = ts[1].clone();
    rig.hover_cell(1, false);
    let t1 = rig.until("card switches to another account", 4, |d| {
        d["tip_visible"] == true && d["tip_hash"] != tip0
    });
    assert!(
        f(&t1["tip_size"][1]) < f(&h0h),
        "that account has no credit row: {} -> {}",
        h0h,
        t1["tip_size"][1]
    );
    rig.leave();
    rig.until("card disappears when the pointer leaves", 3, |d| {
        d["tip_visible"] == false
    });
    assert!(!rig.tip());
    rig.until("fast timer stops after leaving", 3, |d| d["fast"] == false);
    rig.set(json!({"widget_tooltip": false}));
    sleep(1300);
    rig.hover_cell(0, false);
    sleep(900);
    assert!(
        !rig.tip() && rig.dump()["tip_visible"] == false,
        "card can be switched off in settings"
    );
    rig.leave();
    rig.set(json!({"widget_tooltip": true}));
    sleep(1300);
    // card placement: widget on the LEFT half -> card on its right; near the bottom / top edge -> clamped on screen
    rig.set(pos(0.2, 0.5));
    rig.until("widget on the left", 5, |d| Rig::center(d).0 < sw / 2.0);
    rig.hover_cell(0, false);
    let l = rig.until("card on the left-half widget", 4, |d| {
        d["tip_visible"] == true
    });
    assert!(
        f(&l["tip_pos"][0]) >= f(&l["org"][0]) + f(&l["img"][0]) - 1.0,
        "card is on the right of a left-side widget: {l}"
    );
    rig.leave();
    for (fy, name) in [(0.97, "bottom"), (0.03, "top")] {
        rig.set(pos(0.75, fy));
        rig.until(&format!("widget near the {name}"), 5, |d| {
            (Rig::center(d).1 - (sh * fy).round()).abs() <= 3.0
        });
        rig.hover_cell(0, false);
        let e = rig.until(&format!("card near the {name} edge"), 4, |d| {
            d["tip_visible"] == true
        });
        let (y0, y1) = (
            f(&e["tip_pos"][1]),
            f(&e["tip_pos"][1]) + f(&e["tip_size"][1]),
        );
        assert!(
            y0 >= 0.0 && y1 <= sh,
            "card clamped on screen at the {name}: y {y0}..{y1} of {sh}"
        );
        rig.leave();
        rig.until("card gone", 3, |d| d["tip_visible"] == false);
    }
    rig.set(pos(0.75, 0.5));
    rig.until("back to the middle", 5, |d| {
        (Rig::center(d).1 - (sh * 0.5).round()).abs() <= 3.0
    });

    // ---- many accounts (old "14 accounts" block); style rings, sorted by usage, explicit stacking limit
    rig.set(json!({"widget_style": "rings", "widget_hidden": [], "widget_sort": "usage", "widget_max_cells": 0, "widget_max_len": 4000}));
    let big = rig.until("14 cells at natural size", 6, |d| {
        d["cells"].as_array().unwrap().len() == 14 && d["plan_size"] == json!(36.0)
    });
    assert!(!big["cells"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c == "__more__"));
    let worst = (0..14)
        .max_by(|&a, &b| {
            let s = |i: usize| f(&profile_data(i)["five"]).max(f(&profile_data(i)["week"]));
            s(a).partial_cmp(&s(b)).unwrap()
        })
        .unwrap();
    assert_eq!(
        big["cells"][0], rig.names[worst],
        "sorted by usage: the busiest account is first"
    );
    let nat = along(&big);
    let limit = (nat * 0.8).round();
    rig.set(json!({"widget_max_len": limit}));
    let shrunk = rig.until("shrinks before cutting", 6, |d| f(&d["plan_size"]) < 36.0);
    assert_eq!(
        shrunk["cells"].as_array().unwrap().len(),
        14,
        "fit by SHRINKING first (no cut needed): {shrunk}"
    );
    assert!(
        along(&shrunk) <= limit * 1.03,
        "{} px along the axis, limit {limit}",
        along(&shrunk)
    );
    rig.set(json!({"widget_max_len": 0, "widget_max_cells": 9}));
    let cap = rig.until("cap forces a cut", 6, |d| {
        d["cells"].as_array().unwrap().len() == 10
    });
    assert_eq!(
        cap["cells"][9], "__more__",
        "the rest become one '+N' cell at the end"
    );
    assert_eq!(cap["more"], json!(5), "{cap}");
    // hovering the '+N' cell lists the accounts that are not shown
    rig.hover_cell(9, false);
    let m = rig.until("card of the +N cell", 4, |d| {
        d["tip_visible"] == true && d["tip_is_more"] == true
    });
    assert!(f(&m["tip_size"][1]) > 80.0, "{}", m["tip_size"]);
    rig.leave();
    rig.until("card gone", 3, |d| d["tip_visible"] == false);
    rig.set(json!({"widget_max_cells": 4}));
    let c4 = rig.until("user cap 4", 6, |d| {
        d["cells"].as_array().unwrap().len() == 5
    });
    assert_eq!(
        (c4["cells"][4].as_str(), c4["more"].clone()),
        (Some("__more__"), json!(10)),
        "{c4}"
    );
    rig.set(json!({"widget_style": "cards", "widget_max_cells": 0, "widget_max_len": limit}));
    sleep(1800);
    let cards = rig.dump();
    assert!(
        along(&cards) <= limit * 1.03 + 2.0,
        "cards style within the budget: {} <= {limit}",
        along(&cards)
    );
    rig.set(json!({"widget_style": "rings", "widget_max_len": 0, "widget_max_cells": 0, "widget_sort": "config",
                   "widget_hidden": rig.names[4..].to_vec(), "widget_layout": "vertical"}));
    rig.until("4 cells again", 6, |d| {
        d["cells"].as_array().unwrap().len() == 4
    });

    // ---- auto placement next to the tray, taskbar orientation, no taskbar, fullscreen
    let (swi, shi) = (sw as i32, sh as i32);
    let s8 = (8.0 * scale).floor();
    rig.set(json!({"widget_cx": null, "widget_cy": null, "widget_layout": "auto", "widget_anchor": "tray"}));
    fs::write(
        data.join("fake_taskbar.txt"),
        format!(
            "0 {} {swi} {shi} {} {} {swi} {shi}",
            shi - 48,
            swi - 300,
            shi - 48
        ),
    )
    .unwrap();
    let h = rig.until("docked on a bottom taskbar", 6, |d| {
        d["vertical"] == false && Rig::center(d).1 > sh - 60.0
    });
    let fr = &h["frame"];
    let c = Rig::center(&h);
    assert!(
        (c.0 + f(&fr["w"]) / 2.0 - ((sw - 300.0) - s8)).abs() <= 1.5,
        "content ends {s8}px left of the tray: right edge {}",
        c.0 + f(&fr["w"]) / 2.0
    );
    assert!(
        (c.1 - (sh - 24.0)).abs() <= 1.5,
        "centred in the bar: {} vs {}",
        c.1,
        sh - 24.0
    );
    rig.set(json!({"widget_anchor": "left"}));
    let l = rig.until("left anchored", 6, |d| Rig::center(d).0 < sw / 2.0);
    assert!(
        (Rig::center(&l).0 - f(&l["frame"]["w"]) / 2.0 - s8).abs() <= 1.5,
        "starts {s8}px from the bar's left edge"
    );
    rig.set(json!({"widget_anchor": "tray"}));
    fs::write(
        data.join("fake_taskbar.txt"),
        format!(
            "{} 0 {swi} {shi} {} {} {swi} {shi}",
            swi - 60,
            swi - 60,
            shi - 200
        ),
    )
    .unwrap();
    let v = rig.until("docked on a vertical taskbar", 6, |d| {
        d["vertical"] == true && Rig::center(d).0 > sw - 80.0
    });
    let c = Rig::center(&v);
    assert!(
        (c.0 - (sw - 30.0)).abs() <= 1.5,
        "centred across the bar: {} vs {}",
        c.0,
        sw - 30.0
    );
    assert!(
        (c.1 + f(&v["frame"]["h"]) / 2.0 - ((sh - 200.0) - s8)).abs() <= 1.5,
        "bottom sits {s8}px above the tray"
    );
    fs::write(data.join("fake_taskbar.txt"), "").unwrap();
    rig.until("no taskbar and no saved position: hidden", 6, |d| {
        d["visible"] == false
    });
    fs::write(
        data.join("fake_taskbar.txt"),
        format!(
            "0 {} {swi} {shi} {} {} {swi} {shi}",
            shi - 48,
            swi - 300,
            shi - 48
        ),
    )
    .unwrap();
    rig.until("taskbar back: shown", 6, |d| d["visible"] == true);
    fs::write(data.join("fake_fullscreen.flag"), "1").unwrap();
    rig.until("hidden while a fullscreen app is in front", 6, |d| {
        d["visible"] == false
    });
    assert!(!win_visible(rig.w));
    fs::remove_file(data.join("fake_fullscreen.flag")).unwrap();
    rig.until("shown after leaving fullscreen", 6, |d| {
        d["visible"] == true
    });

    // ---- old: MOVE / RESIZE / ROTATE / padlock / locked click and drag
    let opens = |r: &Rig| r.log().matches("open dashboard requested").count();
    rig.set(json!({"widget_layout": "horizontal"}));
    sleep(1300);
    let base = Rig::center(&rig.dump());
    send(rig.w, WM_CMD, CMD_LOCK, 0); // unlock
    rig.cfg_until("unlocked", 3, |c| c["widget_locked"] == false);
    rig.until("edit frame", 5, |d| f(&d["frame"]["m"]) > 0.0);
    let c = rig.drag("body", -300.0, -250.0);
    assert!(
        (f(&c["widget_cx"]) - (base.0 - 300.0)).abs() < 2.0
            && (f(&c["widget_cy"]) - (base.1 - 250.0)).abs() < 2.0,
        "MOVE: content centre follows the mouse exactly: base {base:?} -> ({}, {})",
        c["widget_cx"],
        c["widget_cy"]
    );
    let (cx, cy, s0) = (f(&c["widget_cx"]), f(&c["widget_cy"]), f(&c["widget_size"]));
    let d = rig.until("moved", 5, |d| (Rig::center(d).0 - cx).abs() < 2.0);
    let (hx, hy) = Rig::point(&d, "size");
    let d0 = (hx - cx).hypot(hy - cy);
    let c = rig.drag("size", (hx - cx) / d0 * d0 * 0.5, (hy - cy) / d0 * d0 * 0.5);
    assert!(
        (f(&c["widget_size"]) - s0 * 1.5).abs() < 3.0,
        "RESIZE: size x1.5: {s0} -> {}",
        c["widget_size"]
    );
    let d = rig.dump();
    let (rx, ry) = Rig::point(&d, "rot");
    let c = rig.drag("rot", (cx + 150.0) - rx, cy - ry);
    assert_eq!(
        c["widget_angle"],
        json!(90.0),
        "ROTATE: snaps to 90 degrees"
    );
    let d = rig.until("rotated 90", 5, |d| f(&d["built_angle"]) == 90.0);
    let (rx, ry) = Rig::point(&d, "rot");
    let c = rig.drag(
        "rot",
        cx + 90.0 * 35f64.to_radians().sin() - rx,
        cy - 90.0 * 35f64.to_radians().cos() - ry,
    );
    assert!(
        (f(&c["widget_angle"]) - 35.0).abs() < 2.0
            && (f(&c["widget_cx"]) - cx).abs() < 1.0
            && (f(&c["widget_cy"]) - cy).abs() < 1.0,
        "ROTATE: free angle (~35) and centre stays put: {c}"
    );
    rig.until("edit frame rotated", 5, |d| {
        (f(&d["built_angle"]) - 35.0).abs() < 2.0
    });
    let before_opens = opens(&rig);
    rig.click("lock");
    assert_eq!(
        rig.cfg_until("padlock locks the widget", 4, |c| c["widget_locked"]
            == true)["widget_locked"],
        true
    );
    sleep(1300);
    let d = rig.until("locked frame", 5, |d| f(&d["frame"]["m"]) == 0.0);
    let (bx, by) = Rig::point(&d, "body");
    rig.post(WM_LBUTTONDOWN, bx, by);
    rig.post(WM_LBUTTONUP, bx, by);
    let t = Instant::now();
    while opens(&rig) == before_opens && t.elapsed() < Duration::from_secs(4) {
        sleep(100);
    }
    assert_eq!(
        opens(&rig),
        before_opens + 1,
        "locked: click opens the dashboard"
    );
    let before = rig.cfg();
    rig.post(WM_LBUTTONDOWN, bx, by);
    rig.post(WM_MOUSEMOVE, bx + 400.0, by + 400.0);
    rig.post(WM_LBUTTONUP, bx + 400.0, by + 400.0);
    sleep(300);
    let after = rig.cfg();
    assert!(
        before["widget_cx"] == after["widget_cx"]
            && before["widget_cy"] == after["widget_cy"]
            && opens(&rig) == before_opens + 1,
        "locked: dragging does NOT move it (and does not open the dashboard)"
    );
}
