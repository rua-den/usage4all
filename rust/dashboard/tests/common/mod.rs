//! Shared helpers of the dashboard UI tests (WebView2 process driven by the `--e2e` JS hook).
//!
//! Multi-stage scripts: the page script calls `stage(name, data)` (JS prelude below) which writes `<out>.<name>`
//! and then waits until the test creates `<out>.<name>.go`. Between the two the test may inspect and edit the
//! sandbox (cache, config, settings.json, flag files) exactly like the Python tests did from their driver thread.
#![allow(dead_code)]

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn window() -> HWND {
    // SAFETY: NUL-terminated class name.
    unsafe { FindWindowW(wide("ClaudeUsageDashboard").as_ptr(), std::ptr::null()) }
}

pub struct Guard(pub Child);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timeout waiting for: {what}");
}

pub fn spawn(local: &Path, home: &Path, script: Option<(&Path, &Path)>) -> Guard {
    let mut c = Command::new(env!("CARGO_BIN_EXE_claude-usage-dashboard"));
    if let Some((s, o)) = script {
        c.arg("--e2e").arg(s).arg(o);
    }
    c.env("LOCALAPPDATA", local)
        .env("USERPROFILE", home)
        .env_remove("HOMEDRIVE")
        .env_remove("HOMEPATH")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("CU_NO_AUTOSTART", "1")
        .env("CU_NO_REPAIR", "1")
        .env("CU_NO_DASHBOARD", "1");
    Guard(c.spawn().expect("spawn dashboard"))
}

pub const PRELUDE: &str = r#"
const sleep = ms => new Promise(r => setTimeout(r, ms));
const $ = id => document.getElementById(id);
const R = [];
const check = (n, ok, d) => { R.push([n, !!ok, d === undefined ? '' : String(d)]); };
const cfgNow = async () => (await api.state()).cfg;
const waitFor = async (f, ms) => { const t = performance.now(); while (performance.now() - t < ms) { if (f()) return true; await sleep(50); } return false; };
const stage = async (name, data) => {
  await api.e2e_stage(name, { R: R.splice(0), data: data === undefined ? null : data });
  while (!(await api.e2e_go(name))) await sleep(60);
};
const setv = (id, v, t = 'input') => { const e = $(id); e.value = v; e.dispatchEvent(new Event(t, { bubbles: true })); };
const tick = (el, v) => { el.checked = v; el.dispatchEvent(new Event('change', { bubbles: true })); };
"#;

/// One dashboard process running a (multi-stage) script. Checks reported by the page accumulate in `checks`.
pub struct Run {
    pub guard: Guard,
    out: PathBuf,
    started: Instant,
    pub checks: Vec<(String, bool, String)>,
}

impl Run {
    pub fn start(local: &Path, home: &Path, dir: &Path, name: &str, js: &str, consts: &str) -> Run {
        let s = dir.join(format!("{name}.js"));
        let o = dir.join(format!("{name}.out"));
        fs::write(&s, format!("{PRELUDE}\n{consts}\n{js}")).unwrap();
        let _ = fs::remove_file(&o);
        let started = Instant::now();
        let guard = spawn(local, home, Some((&s, &o)));
        Run {
            guard,
            out: o,
            started,
            checks: vec![],
        }
    }

    fn file(&self, suffix: &str) -> PathBuf {
        let mut p = self.out.clone().into_os_string();
        p.push(suffix);
        PathBuf::from(p)
    }

    fn take(&mut self, v: &Value) {
        for c in v["R"].as_array().into_iter().flatten() {
            self.checks.push((
                c[0].as_str().unwrap_or("?").to_string(),
                c[1].as_bool().unwrap_or(false),
                c[2].as_str().unwrap_or("").to_string(),
            ));
        }
    }

    /// Wait until the page reaches stage `name`; returns the `data` it attached (checks are collected).
    pub fn stage(&mut self, name: &str) -> Value {
        let f = self.file(&format!(".{name}"));
        wait_for(&format!("stage {name}"), 60, || f.exists());
        std::thread::sleep(Duration::from_millis(60));
        let v: Value = serde_json::from_str(&fs::read_to_string(&f).unwrap()).unwrap();
        self.take(&v);
        v["data"].clone()
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Release the page from stage `name`.
    pub fn go(&self, name: &str) {
        fs::write(self.file(&format!(".{name}.go")), "1").unwrap();
    }

    /// Record a check done on the Rust side (files, window state).
    pub fn rust_check(&mut self, name: &str, ok: bool, detail: impl Into<String>) {
        self.checks.push((name.to_string(), ok, detail.into()));
    }

    /// The final value the script returned (written by the `__e2e` message), when it ends by itself.
    pub fn finish(&mut self) -> Value {
        let o = self.out.clone();
        wait_for("script result", 60, || o.exists());
        std::thread::sleep(Duration::from_millis(100));
        let v: Value = serde_json::from_str(&fs::read_to_string(&o).unwrap()).unwrap();
        assert!(v.get("error").is_none() || v["error"].is_null(), "script error: {v}");
        self.take(&v);
        v
    }

    pub fn exited(&mut self, secs: u64) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(secs) {
            if self.guard.0.try_wait().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }
}

/// Print the table of checks and fail with the list of failed ones.
pub fn report(title: &str, checks: &[(String, bool, String)]) {
    println!("== {title}");
    for (n, ok, d) in checks {
        println!(
            "  {} {n}{}",
            if *ok { "ok  " } else { "FAIL" },
            if d.is_empty() { String::new() } else { format!("  [{d}]") }
        );
    }
    let bad: Vec<_> = checks.iter().filter(|c| !c.1).map(|c| format!("{} [{}]", c.0, c.2)).collect();
    println!("{}/{} checks passed", checks.len() - bad.len(), checks.len());
    assert!(bad.is_empty(), "failed checks:\n{}", bad.join("\n"));
}

pub fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

pub fn read_json(p: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(p).unwrap_or_default()).unwrap_or(Value::Null)
}

/// A sandbox: `local` (LOCALAPPDATA), `home` (USERPROFILE: empty fake home), data dir, profile dirs.
pub struct Sandbox {
    pub root: tempfile::TempDir,
    pub local: PathBuf,
    pub home: PathBuf,
    pub data: PathBuf,
    pub profiles: Vec<(String, PathBuf)>,
}

impl Sandbox {
    pub fn new(names: &[String]) -> Sandbox {
        let root = tempfile::tempdir().unwrap();
        let local = root.path().join("local");
        let home = root.path().join("home");
        let data = local.join("claude-usage");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&data).unwrap();
        let mut profiles = vec![];
        for (i, n) in names.iter().enumerate() {
            let d = root.path().join(format!("prof{i}"));
            fs::create_dir_all(&d).unwrap();
            profiles.push((n.clone(), d));
        }
        let list: Vec<Value> = profiles
            .iter()
            .map(|(n, d)| json!({"name": n, "path": d.to_string_lossy()}))
            .collect();
        fs::write(
            data.join("config.json"),
            json!({"profiles": list, "autostart": false}).to_string(),
        )
        .unwrap();
        Sandbox {
            root,
            local,
            home,
            data,
            profiles,
        }
    }

    pub fn cfg(&self) -> Value {
        read_json(&self.data.join("config.json"))
    }

    /// `core.write_cache` equivalent (fresh timestamps). `extra` patches per-profile entries by name.
    pub fn write_cache(
        &self,
        entry: impl Fn(usize, &str, f64) -> Value,
        busy: &[&str],
        refresh: Option<Value>,
    ) {
        let now = now_secs();
        let mut profiles = serde_json::Map::new();
        for (i, (n, d)) in self.profiles.iter().enumerate() {
            let mut e = entry(i, n, now);
            e["path"] = json!(d.to_string_lossy());
            profiles.insert(n.clone(), e);
        }
        let v = json!({"ts": now, "block_enabled": true, "block_pct": 95, "warn_pct": 80, "busy": busy,
            "refresh": refresh, "profiles": profiles});
        let tmp = self.data.join("usage_cache.json.tmp");
        fs::write(&tmp, v.to_string()).unwrap();
        fs::rename(&tmp, self.data.join("usage_cache.json")).unwrap();
    }
}
