//! Background process logic that is not UI: singleton pid file, startup clean-up, the real [`Env`]
//! (clock, randomness, HTTP fetch, token refresh, cache/alerts files, log) and one loop iteration.
//! Tray icon and toast are plugged in as callbacks so this part runs and is tested without a window.

use cu_core::cache;
use cu_core::config::{self, Config, Profile};
use cu_core::daemon::{Env, FetchResult, Poller, TickInput, Toast, TrayView};
use cu_core::log::log;
use cu_core::net::{self, Http, Oauth};
use cu_core::paths::{home_dir, Paths};
use serde_json::{Map, Value};
use std::fs;
use std::time::SystemTime;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

const STILL_ACTIVE: u32 = 259;

/// `core.daemon_running`: the pid in `daemon.pid` belongs to a live process.
pub fn daemon_running(paths: &Paths) -> bool {
    let Some(pid) = fs::read_to_string(&paths.pidfile)
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok())
    else {
        return false;
    };
    process_alive(pid)
}

pub fn process_alive(pid: u32) -> bool {
    // SAFETY: plain Win32 calls; the handle is closed before returning.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code) != 0;
        CloseHandle(h);
        ok && code == STILL_ACTIVE
    }
}

/// Start of `App.__init__`: record our pid, drop stale flag files.
pub fn startup(paths: &Paths) {
    let _ = fs::write(&paths.pidfile, std::process::id().to_string());
    for f in [&paths.show, &paths.flag, &paths.testflag] {
        let _ = fs::remove_file(f);
    }
}

/// Quit: remove our pid file (only if it is still ours).
pub fn shutdown(paths: &Paths) {
    let ours = fs::read_to_string(&paths.pidfile)
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok())
        == Some(std::process::id());
    if ours {
        let _ = fs::remove_file(&paths.pidfile);
    }
}

/// `App.load_alert_state`: `None` = no alerts.json (first run => baseline); unreadable file => empty state.
pub fn load_alert_state(paths: &Paths) -> Option<Map<String, Value>> {
    if !paths.alerts.exists() {
        return None;
    }
    Some(
        fs::read_to_string(&paths.alerts)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| match v {
                Value::Object(m) => Some(m),
                _ => None,
            })
            .unwrap_or_default(),
    )
}

pub type IconSink = Box<dyn FnMut(&TrayView)>;
pub type ToastSink = Box<dyn FnMut(&Toast) -> Result<(), String>>;

pub struct RealEnv {
    pub paths: Paths,
    pub http: Box<dyn Http>,
    pub oauth: Oauth,
    pub on_icon: IconSink,
    pub on_toast: ToastSink,
    rng: u64,
}

impl RealEnv {
    pub fn new(paths: Paths, http: Box<dyn Http>) -> Self {
        let lp = paths.clone();
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
        RealEnv {
            oauth: Oauth::new(move |m| log(&lp, m)),
            paths,
            http,
            // No UI yet: icon updates are dropped, toasts are only logged (the caller replaces both).
            on_icon: Box::new(|_| {}),
            on_toast: Box::new(|_| Ok(())),
            rng: nanos ^ (u64::from(std::process::id()) << 32) | 1,
        }
    }
}

impl Env for RealEnv {
    fn now(&self) -> f64 {
        cache::now_ts()
    }
    fn rand(&mut self) -> f64 {
        // xorshift64*: jitter only, not security relevant.
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
    fn refresh_if_expired(&mut self, p: &Profile) -> Option<FetchResult> {
        self.oauth.refresh_if_expired(self.http.as_ref(), p)
    }
    fn fetch(&mut self, p: &Profile) -> FetchResult {
        net::fetch(self.http.as_ref(), p)
    }
    fn write_cache(
        &mut self,
        cfg: &Config,
        data: &Map<String, Value>,
        busy: &[String],
        refresh: Option<Value>,
    ) {
        // Failures are ignored like the Python loop's best-effort write; the next tick tries again.
        let _ = cache::write_cache(&self.paths, cfg, data, busy, refresh);
    }
    fn refresh_icon(&mut self, view: &TrayView) {
        (self.on_icon)(view);
    }
    fn log(&mut self, msg: &str) {
        log(&self.paths, msg);
    }
    fn save_alert_state(&mut self, st: &Map<String, Value>) {
        if let Ok(t) = serde_json::to_string(&Value::Object(st.clone())) {
            let _ = fs::write(&self.paths.alerts, t);
        }
    }
    fn send_toast(&mut self, t: &Toast) -> Result<(), String> {
        (self.on_toast)(t)
    }
}

/// The poller plus the file-based inputs of `App.loop`.
pub struct Daemon {
    pub poller: Poller<RealEnv>,
    cfg_mtime: Option<SystemTime>,
    /// Set when `show.flag` was seen; the UI layer clears it after opening the dashboard.
    pub show_requested: bool,
}

fn mtime(p: &std::path::Path) -> Option<SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Remove a flag file; true if it existed.
fn take_flag(p: &std::path::Path) -> bool {
    p.exists() && {
        let _ = fs::remove_file(p);
        true
    }
}

impl Daemon {
    /// `App.__init__` + the first half of `loop()` (`seed(); publish()`). Call [`startup`] first.
    pub fn new(env: RealEnv) -> Self {
        let paths = env.paths.clone();
        let cfg = config::load_config(&paths, &home_dir());
        let mut poller = Poller::new(env, cfg, load_alert_state(&paths));
        let cache = cache::load_cache(&paths);
        poller.start(cache.as_ref());
        Daemon {
            cfg_mtime: mtime(&paths.config),
            poller,
            show_requested: false,
        }
    }

    /// One iteration of `App.loop` (call about once a second).
    pub fn step(&mut self) {
        let paths = self.poller.env.paths.clone();
        let m = mtime(&paths.config);
        let config = (m != self.cfg_mtime).then(|| {
            self.cfg_mtime = m;
            config::load_config(&paths, &home_dir())
        });
        if take_flag(&paths.show) {
            self.show_requested = true;
        }
        self.poller.tick(TickInput {
            config,
            test_toast: take_flag(&paths.testflag),
            refresh: take_flag(&paths.flag),
        });
    }
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "ClaudeUsage";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// `core.set_autostart`: write/delete `HKCU\...\Run\ClaudeUsage` (same value name, so it replaces the Python
/// build's entry). Returns false on failure. Callers gate it: see [`autostart_allowed`].
pub fn set_autostart(on: bool, exe: &std::path::Path) -> bool {
    use windows_sys::Win32::System::Registry::*;
    // SAFETY: plain registry calls with NUL-terminated UTF-16 strings; the key is closed.
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide(RUN_KEY).as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut key,
        ) != 0
        {
            return false;
        }
        let name = wide(RUN_VALUE);
        let ok = if on {
            let v = wide(&autostart_command(exe));
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                v.as_ptr() as *const u8,
                (v.len() * 2) as u32,
            ) == 0
        } else {
            let r = RegDeleteValueW(key, name.as_ptr());
            r == 0 || r == 2 // 2 = not found: already off
        };
        RegCloseKey(key);
        ok
    }
}

/// The registry is user state outside the data dir (sandboxing LOCALAPPDATA does not cover HKCU), so debug
/// builds and anything started with `CU_NO_AUTOSTART` never write it.
pub fn autostart_allowed() -> bool {
    !cfg!(debug_assertions) && std::env::var_os("CU_NO_AUTOSTART").is_none()
}

/// `core.launch_cmd("--hidden")` as stored in the autostart value: every part quoted.
pub fn autostart_command(exe: &std::path::Path) -> String {
    format!("\"{}\" \"--hidden\"", exe.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cu_core::net::HttpResponse;
    use serde_json::json;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    #[derive(Clone, Default)]
    struct Fake {
        gets: Rc<RefCell<u32>>,
        posts: Rc<RefCell<u32>>,
    }
    impl Http for Fake {
        fn get(&self, _u: &str, h: &[(&str, &str)], _t: Duration) -> Result<HttpResponse, String> {
            *self.gets.borrow_mut() += 1;
            assert!(h
                .iter()
                .any(|(k, v)| *k == "Authorization" && v.starts_with("Bearer ")));
            let body = json!({"five_hour": {"utilization": 42, "resets_at": "R5"}, "seven_day": {"utilization": 7, "resets_at": "R7"}});
            Ok(HttpResponse {
                status: 200,
                retry_after: None,
                body: body.to_string().into_bytes(),
            })
        }
        fn post_json(
            &self,
            _u: &str,
            _h: &[(&str, &str)],
            _b: &str,
            _t: Duration,
        ) -> Result<HttpResponse, String> {
            *self.posts.borrow_mut() += 1;
            let body = json!({"access_token": "fresh-fake", "refresh_token": "fresh-rt-fake", "expires_in": 3600});
            Ok(HttpResponse {
                status: 200,
                retry_after: None,
                body: body.to_string().into_bytes(),
            })
        }
    }

    fn now_ms() -> i64 {
        (cache::now_ts() * 1000.0) as i64
    }

    /// Sandbox: data dir + one profile dir with fake credentials, config written with explicit profiles.
    fn sandbox(expires_in_ms: i64) -> (tempfile::TempDir, Paths, Profile) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(tmp.path().join("data"));
        let pdir = tmp.path().join("prof");
        fs::create_dir_all(&pdir).unwrap();
        let creds = json!({"claudeAiOauth": {"accessToken": "old-fake", "refreshToken": "old-rt-fake",
            "expiresAt": now_ms() + expires_in_ms, "subscriptionType": "team"}});
        fs::write(pdir.join(".credentials.json"), creds.to_string()).unwrap();
        let p = Profile {
            name: "a".into(),
            path: pdir.to_string_lossy().into_owned(),
        };
        fs::write(
            &paths.config,
            json!({"profiles": [p.to_value()], "autostart": false}).to_string(),
        )
        .unwrap();
        (tmp, paths, p)
    }

    #[test]
    fn pid_singleton_and_startup_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(tmp.path());
        assert!(!daemon_running(&paths));
        for f in [&paths.show, &paths.flag, &paths.testflag] {
            fs::write(f, "1").unwrap();
        }
        startup(&paths);
        assert!(daemon_running(&paths), "our own pid is alive");
        assert!(!paths.show.exists() && !paths.flag.exists() && !paths.testflag.exists());
        shutdown(&paths);
        assert!(!paths.pidfile.exists());
        fs::write(&paths.pidfile, "4294967290").unwrap();
        assert!(!daemon_running(&paths), "dead pid");
        shutdown(&paths);
        assert!(
            paths.pidfile.exists(),
            "a pid file that is not ours is left alone"
        );
        fs::write(&paths.pidfile, "garbage").unwrap();
        assert!(!daemon_running(&paths));
    }

    #[test]
    fn first_step_polls_publishes_and_consumes_flags() {
        let (_t, paths, _p) = sandbox(3_600_000);
        let f = Fake::default();
        let mut d = Daemon::new(RealEnv::new(paths.clone(), Box::new(f.clone())));
        fs::write(&paths.flag, "1").unwrap();
        fs::write(&paths.show, "1").unwrap();
        d.step();
        assert_eq!(*f.gets.borrow(), 1);
        assert_eq!(*f.posts.borrow(), 0, "valid token: no refresh");
        assert!(!paths.flag.exists() && !paths.show.exists());
        assert!(d.show_requested);
        let c: Value = serde_json::from_str(&fs::read_to_string(&paths.cache).unwrap()).unwrap();
        assert_eq!(c["profiles"]["a"]["five"], json!(42));
        assert_eq!(c["profiles"]["a"]["plan"], json!("team"));
        assert!(
            c.to_string().find("old-fake").is_none(),
            "no token in the cache"
        );
    }

    #[test]
    fn expired_token_goes_through_refresh_then_fetch() {
        let (_t, paths, p) = sandbox(-60_000);
        let f = Fake::default();
        let mut d = Daemon::new(RealEnv::new(paths.clone(), Box::new(f.clone())));
        d.step();
        assert_eq!((*f.posts.borrow(), *f.gets.borrow()), (1, 1));
        let c: Value = serde_json::from_str(
            &fs::read_to_string(std::path::Path::new(&p.path).join(".credentials.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(c["claudeAiOauth"]["accessToken"], json!("fresh-fake"));
        let log = fs::read_to_string(&paths.events).unwrap();
        assert!(
            log.contains("token refreshed: a") && !log.contains("fake"),
            "{log}"
        );
    }

    #[test]
    fn config_change_is_picked_up_and_alert_state_loading() {
        let (_t, paths, p) = sandbox(3_600_000);
        assert!(load_alert_state(&paths).is_none());
        fs::write(&paths.alerts, "not json").unwrap();
        assert_eq!(load_alert_state(&paths), Some(Map::new()));
        let mut d = Daemon::new(RealEnv::new(paths.clone(), Box::new(Fake::default())));
        std::thread::sleep(Duration::from_millis(30));
        fs::write(
            &paths.config,
            json!({"profiles": [p.to_value()], "warn_pct": 55}).to_string(),
        )
        .unwrap();
        d.step();
        assert_eq!(d.poller.cfg["warn_pct"], json!(55));
    }

    #[test]
    fn autostart_command_quotes_every_part() {
        assert_eq!(
            autostart_command(std::path::Path::new(
                r"C:\Program Files\cu\claude-usage.exe"
            )),
            r#""C:\Program Files\cu\claude-usage.exe" "--hidden""#
        );
    }

    #[test]
    fn jitter_source_is_in_unit_interval() {
        let tmp = tempfile::tempdir().unwrap();
        let mut e = RealEnv::new(Paths::with_root(tmp.path()), Box::new(Fake::default()));
        let v: Vec<f64> = (0..1000).map(|_| e.rand()).collect();
        assert!(v.iter().all(|x| (0.0..1.0).contains(x)) && v.windows(2).any(|w| w[0] != w[1]));
    }
}
