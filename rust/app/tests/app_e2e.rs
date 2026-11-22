//! End-to-end test of the real background app (tray + poller + notifications), replacing `tests/e2e_app.py`.
//! The Python test called the real API with real tokens. Here the real exe talks to a fake HTTP server on
//! 127.0.0.1 through the TEST-ONLY endpoint override (`CU_API_BASE` / `CU_TOKEN_URL`, compiled only into debug
//! builds and `--features e2e`; the shipped release ignores it, see `tests/smoke_release_rust.py`). Fake tokens
//! (`test-token-not-real-*`) therefore never leave the machine. Everything runs in a sandboxed
//! LOCALAPPDATA/USERPROFILE with registry/hook writes disabled.
//!
//! ONE test function on purpose (GUI/process tests must not run concurrently). Takes ~2 minutes (real timers).
#![cfg(any(debug_assertions, feature = "e2e"))]

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW;

const DEAD_MSG: &str = "phiên đăng nhập hết hạn - chạy claude ở profile này rồi /login";

// ------------------------------------------------------------------------------------------ fake server

#[derive(Clone)]
struct Usage {
    status: u16,
    retry_after: Option<u32>,
    five: f64,
    week: f64,
    delay_ms: u64,
}

#[derive(Clone)]
enum Tok {
    Ok { access: String, refresh: String },
    InvalidGrant,
}

#[derive(Default)]
struct St {
    usage: HashMap<String, Usage>,
    tokens: HashMap<String, Tok>,
    usage_hits: HashMap<String, u32>,
    token_hits: HashMap<String, u32>,
    conns: u32,
}

struct Fake {
    base: String,
    st: Arc<Mutex<St>>,
}

fn http_reply(s: &mut TcpStream, status: u16, extra: &str, body: &str) {
    let msg = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = s.write_all(msg.as_bytes());
}

fn serve_one(mut s: TcpStream, st: Arc<Mutex<St>>) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let (head_end, len) = loop {
        let n = s.read(&mut tmp).unwrap_or(0);
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..i]).to_lowercase();
            let len = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            break (i + 4, len);
        }
    };
    while buf.len() < head_end + len {
        let n = s.read(&mut tmp).unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let body = String::from_utf8_lossy(&buf[head_end..]).to_string();
    let first = head.lines().next().unwrap_or("").to_string();
    st.lock().unwrap().conns += 1;
    if first.starts_with("GET /api/oauth/usage") {
        let token = head
            .lines()
            .find_map(|l| {
                l.to_lowercase()
                    .strip_prefix("authorization: bearer ")
                    .map(|_| l[22..].trim().to_string())
            })
            .unwrap_or_default();
        let u = {
            let mut g = st.lock().unwrap();
            *g.usage_hits.entry(token.clone()).or_default() += 1;
            g.usage.get(&token).cloned()
        };
        match u {
            None => http_reply(&mut s, 401, "", r#"{"error":"unauthorized"}"#),
            Some(u) => {
                std::thread::sleep(Duration::from_millis(u.delay_ms));
                if u.status == 200 {
                    let b = json!({
                        "five_hour": {"utilization": u.five, "resets_at": "2030-01-01T10:00:00+00:00"},
                        "seven_day": {"utilization": u.week, "resets_at": "2030-01-05T10:00:00+00:00"}
                    });
                    http_reply(&mut s, 200, "", &b.to_string());
                } else {
                    let extra = u
                        .retry_after
                        .map(|r| format!("Retry-After: {r}\r\n"))
                        .unwrap_or_default();
                    http_reply(&mut s, u.status, &extra, r#"{"error":"x"}"#);
                }
            }
        }
    } else if first.starts_with("POST /v1/oauth/token") {
        let rt = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v["refresh_token"].as_str().map(str::to_string))
            .unwrap_or_default();
        let t = {
            let mut g = st.lock().unwrap();
            *g.token_hits.entry(rt.clone()).or_default() += 1;
            g.tokens.get(&rt).cloned()
        };
        match t {
            Some(Tok::Ok { access, refresh }) => {
                let b = json!({"access_token": access, "refresh_token": refresh, "expires_in": 28800,
                               "scope": "user:inference user:profile"});
                http_reply(&mut s, 200, "", &b.to_string());
            }
            Some(Tok::InvalidGrant) | None => {
                http_reply(&mut s, 400, "", r#"{"error":"invalid_grant"}"#)
            }
        }
    } else {
        http_reply(&mut s, 404, "", "{}");
    }
}

fn fake_server() -> Fake {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let st = Arc::new(Mutex::new(St::default()));
    let st2 = st.clone();
    std::thread::spawn(move || {
        for c in l.incoming().flatten() {
            let st = st2.clone();
            std::thread::spawn(move || serve_one(c, st));
        }
    });
    Fake { base, st }
}

impl Fake {
    fn usage(&self, token: &str, u: Usage) {
        self.st.lock().unwrap().usage.insert(token.into(), u);
    }
    fn token(&self, refresh: &str, t: Tok) {
        self.st.lock().unwrap().tokens.insert(refresh.into(), t);
    }
    fn usage_hits(&self, token: &str) -> u32 {
        self.st
            .lock()
            .unwrap()
            .usage_hits
            .get(token)
            .copied()
            .unwrap_or(0)
    }
    fn token_hits(&self, rt: &str) -> u32 {
        self.st
            .lock()
            .unwrap()
            .token_hits
            .get(rt)
            .copied()
            .unwrap_or(0)
    }
}

fn ok(five: f64, week: f64, delay_ms: u64) -> Usage {
    Usage {
        status: 200,
        retry_after: None,
        five,
        week,
        delay_ms,
    }
}

// ------------------------------------------------------------------------------------------ helpers

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn now_s() -> f64 {
    now_ms() as f64 / 1000.0
}

fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    panic!("timeout waiting for: {what}");
}

fn tray_window() -> usize {
    let c: Vec<u16> = "ClaudeUsageTray".encode_utf16().chain(Some(0)).collect();
    // SAFETY: FindWindowW with a NUL-terminated class name.
    unsafe { FindWindowW(c.as_ptr(), std::ptr::null()) as usize }
}

struct Guard(Option<Child>);
impl Guard {
    fn stop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Rig {
    home: PathBuf,
    data: PathBuf,
    local: PathBuf,
    api: String,
}

impl Rig {
    fn new(root: &Path, name: &str, api: &str) -> Rig {
        let local = root.join(name).join("local");
        let home = root.join(name).join("home");
        let data = local.join("claude-usage");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&home).unwrap();
        Rig {
            home,
            data,
            local,
            api: api.into(),
        }
    }
    fn start(&self) -> Guard {
        Guard(Some(
            Command::new(env!("CARGO_BIN_EXE_claude-usage"))
                .arg("--hidden")
                .env("LOCALAPPDATA", &self.local)
                .env("USERPROFILE", &self.home)
                .env_remove("HOMEDRIVE")
                .env_remove("HOMEPATH")
                .env_remove("CLAUDE_CONFIG_DIR")
                .env_remove("HTTPS_PROXY")
                .env_remove("https_proxy")
                .env_remove("ALL_PROXY")
                .env_remove("all_proxy")
                .env("CU_NO_AUTOSTART", "1")
                .env("CU_NO_REPAIR", "1")
                .env("CU_NO_DASHBOARD", "1")
                .env("CU_API_BASE", &self.api)
                .env("CU_TOKEN_URL", format!("{}/v1/oauth/token", self.api))
                .spawn()
                .expect("spawn exe"),
        ))
    }
    fn profile(&self, name: &str, access: &str, refresh: &str, expires_at_ms: i64) -> Value {
        let dir = self.home.join(name);
        fs::create_dir_all(&dir).unwrap();
        let creds = json!({
            "claudeAiOauth": {"accessToken": access, "refreshToken": refresh, "expiresAt": expires_at_ms,
                              "scopes": ["user:inference", "user:profile"], "subscriptionType": "max"},
            "otherTopLevel": "keep-me"
        });
        fs::write(dir.join(".credentials.json"), creds.to_string()).unwrap();
        json!({"name": name, "path": dir.to_string_lossy()})
    }
    fn cfg(&self, profiles: &[Value], extra: Value) {
        let mut c = json!({"interval_sec": 90, "warn_pct": 1, "block_pct": 99, "block_enabled": false,
                           "widget_enabled": false, "autostart": false, "profiles": profiles});
        for (k, v) in extra.as_object().unwrap() {
            c[k] = v.clone();
        }
        fs::write(self.data.join("config.json"), c.to_string()).unwrap();
    }
    fn json(&self, f: &str) -> Value {
        serde_json::from_str(&fs::read_to_string(self.data.join(f)).unwrap_or_default())
            .unwrap_or(Value::Null)
    }
    fn cache(&self) -> Value {
        self.json("usage_cache.json")
    }
    fn prof(&self, name: &str) -> Value {
        self.cache()["profiles"][name].clone()
    }
    fn log(&self) -> String {
        fs::read_to_string(self.data.join("events.log")).unwrap_or_default()
    }
    fn toasts(&self) -> Vec<String> {
        self.log()
            .lines()
            .filter(|l| l.contains(" toast |"))
            .map(str::to_string)
            .collect()
    }
    fn alerts_len(&self) -> usize {
        self.json("alerts.json").as_object().map_or(0, |o| o.len())
    }
    fn has_data(&self, name: &str) -> bool {
        self.prof(name).get("five").is_some()
    }
    fn assert_no_secrets(&self) {
        for f in [
            "events.log",
            "usage_cache.json",
            "alerts.json",
            "config.json",
        ] {
            let t = fs::read_to_string(self.data.join(f)).unwrap_or_default();
            assert!(
                !t.contains("test-token-not-real"),
                "token text leaked into {f}"
            );
        }
    }
}

// ------------------------------------------------------------------------------------------ scenarios

/// A: poll cycle, busy marker, OAuth refresh with rotation, dead token, no re-notification on repeated polls,
/// 429 backoff keeping old values, manual-refresh throttling (<20 s and 429 wait).
fn scenario_cycle(root: &Path, fake: &Fake) {
    let r = Rig::new(root, "a", &fake.base);
    let far = now_ms() + 3_600_000;
    let p = [
        r.profile("acc-a", "test-token-not-real-A", "test-rt-A", far),
        r.profile("acc-b", "test-token-not-real-B", "test-rt-B", far),
        r.profile(
            "acc-c",
            "test-token-not-real-C-old",
            "test-rt-C",
            now_ms() - 60_000,
        ),
        r.profile(
            "acc-d",
            "test-token-not-real-D",
            "test-rt-D",
            now_ms() - 60_000,
        ),
    ];
    fake.usage("test-token-not-real-A", ok(97.0, 40.0, 700));
    fake.usage("test-token-not-real-B", ok(85.0, 20.0, 700));
    fake.usage("test-token-not-real-C-new", ok(50.0, 10.0, 0));
    fake.token(
        "test-rt-C",
        Tok::Ok {
            access: "test-token-not-real-C-new".into(),
            refresh: "test-rt-C2".into(),
        },
    );
    fake.token("test-rt-D", Tok::InvalidGrant);
    r.cfg(
        &p,
        json!({"notify_enabled": true, "notify_warn": true, "notify_block": true}),
    );
    fs::write(r.data.join("alerts.json"), "{}").unwrap(); // "not a first run": every high account is a new crossing

    let mut g = r.start();
    let t0 = Instant::now();
    let mut busy_samples = 0;
    wait_for("all four accounts processed", 40, || {
        if r.cache()["busy"].as_array().is_some_and(|b| !b.is_empty()) {
            busy_samples += 1;
        }
        let c = r.cache();
        ["acc-a", "acc-b", "acc-c", "acc-d"].iter().all(|n| {
            c["profiles"][n].get("five").is_some() || c["profiles"][n].get("err").is_some()
        }) && c["busy"].as_array().is_some_and(|b| b.is_empty())
    });
    println!("  cycle done after {:.1}s", t0.elapsed().as_secs_f64());
    // data / busy
    for n in ["acc-a", "acc-b", "acc-c"] {
        assert!(r.has_data(n), "{n} has data: {}", r.prof(n));
    }
    assert_eq!(r.prof("acc-a")["five"], json!(97.0));
    assert_eq!(r.prof("acc-b")["week"], json!(20.0));
    assert!(
        busy_samples >= 3,
        "busy (waiting) state visible while fetching: {busy_samples} samples"
    );
    assert_eq!(r.cache()["busy"], json!([]), "busy cleared afterwards");
    // OAuth refresh with rotation written like the CLI
    assert_eq!(fake.token_hits("test-rt-C"), 1, "refresh POST sent once");
    assert_eq!(
        fake.usage_hits("test-token-not-real-C-old"),
        0,
        "expired token never sent to the usage endpoint"
    );
    assert_eq!(
        fake.usage_hits("test-token-not-real-C-new"),
        1,
        "rotated token used for the usage call"
    );
    let cred: Value = serde_json::from_str(
        &fs::read_to_string(r.home.join("acc-c").join(".credentials.json")).unwrap(),
    )
    .unwrap();
    let o = &cred["claudeAiOauth"];
    assert_eq!(o["accessToken"], "test-token-not-real-C-new");
    assert_eq!(o["refreshToken"], "test-rt-C2");
    assert!(
        o["expiresAt"].as_i64().unwrap() > now_ms() + 20 * 3600 * 1000 / 4,
        "new expiry in the future"
    );
    assert_eq!(o["subscriptionType"], "max", "other fields kept");
    assert_eq!(
        cred["otherTopLevel"], "keep-me",
        "other top-level keys kept"
    );
    assert!(
        r.home
            .join("acc-c")
            .join(".credentials.json.claude-usage.bak")
            .exists(),
        "backup written"
    );
    // dead token
    let d = r.prof("acc-d");
    assert_eq!(d["err"], DEAD_MSG, "{d}");
    assert_eq!(d["status"], 401);
    assert_eq!(fake.token_hits("test-rt-D"), 1);
    // toasts: batched, at least one, none after repeated polls
    wait_for("toast logged", 15, || !r.toasts().is_empty());
    assert!(r.alerts_len() >= 1, "alert state persisted");

    // 2nd round through a manual refresh (>= 20 s after the first fetch): A answers 429, B still 200
    let oldest = |n: &str| now_s() - r.prof(n)["fetched_at"].as_f64().unwrap_or(0.0);
    wait_for("20 s since last fetch of every account", 40, || {
        ["acc-a", "acc-b", "acc-c"]
            .iter()
            .all(|n| oldest(n) >= 21.0)
    });
    // by now every toast of the first cycle is out (6 s batching window + 15 s minimum gap between toasts)
    let first_round = r.toasts().len();
    assert!(
        (1..=3).contains(&first_round),
        "{first_round} toast(s): {:?}",
        r.toasts()
    );
    let (hits_c_before, c_fetch_before) = (
        fake.usage_hits("test-token-not-real-C-new"),
        r.prof("acc-c")["fetched_at"].as_f64().unwrap(),
    );
    fake.usage(
        "test-token-not-real-A",
        Usage {
            status: 429,
            retry_after: Some(60),
            five: 0.0,
            week: 0.0,
            delay_ms: 0,
        },
    );
    fake.usage("test-token-not-real-B", ok(86.0, 21.0, 0));
    fs::write(r.data.join("refresh.flag"), "1").unwrap();
    wait_for("refresh recorded", 10, || r.cache()["refresh"].is_object());
    let info = r.cache()["refresh"].clone();
    let started: Vec<&str> = info["started"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        started.contains(&"acc-a") && started.contains(&"acc-b"),
        "refresh started accounts: {info}"
    );
    let skipped_d = info["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "acc-d")
        .cloned();
    assert!(
        skipped_d.is_some_and(|s| s["why"].as_str().unwrap().contains("giới hạn API")),
        "dead account waits instead of hammering: {info}"
    );
    wait_for("429 handled for A and B refetched", 25, || {
        r.prof("acc-a")["err"] == "HTTP 429"
            && r.prof("acc-b")["fetched_at"].as_f64().unwrap_or(0.0) > c_fetch_before - 30.0
            && r.prof("acc-b")["five"] == json!(86.0)
    });
    wait_for("C refetched", 15, || {
        r.prof("acc-c")["fetched_at"].as_f64().unwrap_or(0.0) > c_fetch_before
    });
    let a = r.prof("acc-a");
    assert_eq!(a["five"], json!(97.0), "429 keeps the old numbers: {a}");
    assert_eq!(a["status"], 429);
    let wait_left = a["retry_at"].as_f64().unwrap() - now_s();
    assert!(
        (50.0..=70.0).contains(&wait_left),
        "Retry-After 60 honoured: {wait_left:.0}s"
    );
    assert!(r.log().contains("fetch acc-a: HTTP 429"), "429 logged");
    assert_eq!(
        fake.token_hits("test-rt-C2"),
        0,
        "refreshed token is not refreshed again"
    );
    assert!(fake.usage_hits("test-token-not-real-C-new") > hits_c_before);
    assert_eq!(
        r.toasts().len(),
        first_round,
        "no extra toast after repeated polls: {:?}",
        r.toasts()
    );
    // immediate second refresh: everything is throttled
    fs::write(r.data.join("refresh.flag"), "1").unwrap();
    wait_for("2nd refresh recorded", 10, || {
        r.cache()["refresh"]["ts"].as_f64().unwrap_or(0.0) > info["ts"].as_f64().unwrap()
    });
    let info2 = r.cache()["refresh"].clone();
    assert_eq!(
        info2["started"],
        json!([]),
        "immediate second refresh starts nothing: {info2}"
    );
    let why = |n: &str| {
        info2["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == n)
            .map(|s| s["why"].as_str().unwrap().to_string())
            .unwrap_or_default()
    };
    assert!(why("acc-a").contains("giới hạn API"), "429 wait: {info2}");
    assert!(why("acc-b").contains("vừa cập nhật"), "<20 s: {info2}");
    r.assert_no_secrets();
    g.stop();
}

/// A2: first run (no alerts.json) with accounts already high -> silent baseline, remembered; restart stays silent.
fn scenario_first_run(root: &Path, fake: &Fake) {
    let r = Rig::new(root, "b", &fake.base);
    let far = now_ms() + 3_600_000;
    let p = [
        r.profile("hi-1", "test-token-not-real-H1", "test-rt-H1", far),
        r.profile("hi-2", "test-token-not-real-H2", "test-rt-H2", far),
    ];
    fake.usage("test-token-not-real-H1", ok(96.0, 50.0, 0));
    fake.usage("test-token-not-real-H2", ok(90.0, 60.0, 0));
    r.cfg(&p, json!({"notify_enabled": true}));
    assert!(!r.data.join("alerts.json").exists());
    let mut g = r.start();
    wait_for("both fetched", 20, || {
        r.has_data("hi-1") && r.has_data("hi-2")
    });
    std::thread::sleep(Duration::from_secs(9)); // past the 6 s batching window
    assert_eq!(
        r.toasts().len(),
        0,
        "first run does not announce what is already high: {:?}",
        r.toasts()
    );
    wait_for("state file written", 5, || r.alerts_len() >= 1);
    g.stop();
    let _ = fs::remove_file(r.data.join("daemon.pid"));
    // restart with the same state: seeded from the fresh cache, alert state loaded -> silent
    let mut g = r.start();
    wait_for("restarted", 15, || {
        tray_window() != 0 && r.data.join("daemon.pid").exists()
    });
    std::thread::sleep(Duration::from_secs(9));
    assert_eq!(r.toasts().len(), 0, "restart is silent: {:?}", r.toasts());
    r.assert_no_secrets();
    g.stop();
}

/// C: notifications OFF still records state; the test notification works while OFF; switching ON does not replay.
fn scenario_notify_toggle(root: &Path, fake: &Fake) {
    let r = Rig::new(root, "c", &fake.base);
    let far = now_ms() + 3_600_000;
    let p = [
        r.profile("n-1", "test-token-not-real-N1", "test-rt-N1", far),
        r.profile("n-2", "test-token-not-real-N2", "test-rt-N2", far),
    ];
    fake.usage("test-token-not-real-N1", ok(95.0, 50.0, 0));
    fake.usage("test-token-not-real-N2", ok(92.0, 40.0, 0));
    r.cfg(&p, json!({"notify_enabled": false}));
    fs::write(r.data.join("alerts.json"), "{}").unwrap();
    let mut g = r.start();
    wait_for("both fetched", 20, || {
        r.has_data("n-1") && r.has_data("n-2")
    });
    std::thread::sleep(Duration::from_secs(8));
    assert_eq!(r.toasts().len(), 0, "no toast while OFF: {:?}", r.toasts());
    assert!(r.alerts_len() >= 1, "state still recorded while OFF");
    fs::write(r.data.join("notify_test.flag"), "1").unwrap();
    wait_for("test toast", 10, || r.toasts().len() == 1);
    assert!(
        r.toasts()[0].contains("TẮT"),
        "test notification says it is OFF: {:?}",
        r.toasts()
    );
    r.cfg(&p, json!({"notify_enabled": true}));
    std::thread::sleep(Duration::from_secs(10));
    assert_eq!(
        r.toasts().len(),
        1,
        "switching ON does not replay old conditions: {:?}",
        r.toasts()
    );
    fs::write(r.data.join("notify_test.flag"), "1").unwrap();
    wait_for("2nd test toast", 10, || r.toasts().len() == 2);
    assert!(r.toasts()[1].contains("BẬT"), "{:?}", r.toasts());
    r.assert_no_secrets();
    g.stop();
}

#[test]
fn app_end_to_end_against_a_fake_api() {
    assert_eq!(
        tray_window(),
        0,
        "another Rust tray is running: quit it first"
    );
    let tmp = tempfile::tempdir().unwrap();
    let fake = fake_server();
    scenario_cycle(tmp.path(), &fake);
    println!("A ok");
    scenario_first_run(tmp.path(), &fake);
    println!("A2 ok");
    scenario_notify_toggle(tmp.path(), &fake);
    println!("C ok");
}
