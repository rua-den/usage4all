//! File locations (core.py: DATA, CONFIG, CACHE, FLAG, PIDFILE, ALERTS, TESTFLAG, EVENTS + show.flag).

use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    pub data: PathBuf,
    pub config: PathBuf,
    pub cache: PathBuf,
    /// refresh.flag
    pub flag: PathBuf,
    /// daemon.pid
    pub pidfile: PathBuf,
    pub alerts: PathBuf,
    /// notify_test.flag
    pub testflag: PathBuf,
    /// events.log
    pub events: PathBuf,
    /// show.flag
    pub show: PathBuf,
}

/// Python `Path.home()` on Windows: USERPROFILE, else HOMEDRIVE+HOMEPATH (else HOME, else ".").
pub fn home_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    if let (Some(d), Some(p)) = (std::env::var_os("HOMEDRIVE"), std::env::var_os("HOMEPATH")) {
        let mut s = d;
        s.push(p);
        if !s.is_empty() {
            return PathBuf::from(s);
        }
    }
    if let Some(p) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    PathBuf::from(".")
}

impl Paths {
    /// Layout under an explicit data directory (tests). The directory is created best-effort.
    pub fn with_root(dir: impl Into<PathBuf>) -> Paths {
        let data: PathBuf = dir.into();
        let _ = std::fs::create_dir_all(&data);
        Paths {
            config: data.join("config.json"),
            cache: data.join("usage_cache.json"),
            flag: data.join("refresh.flag"),
            pidfile: data.join("daemon.pid"),
            alerts: data.join("alerts.json"),
            testflag: data.join("notify_test.flag"),
            events: data.join("events.log"),
            show: data.join("show.flag"),
            data,
        }
    }

    /// `%LOCALAPPDATA%\claude-usage` (fallback: `<home>\claude-usage`), created. Reads the env at call time,
    /// so tests isolate by setting `LOCALAPPDATA` exactly like the Python tests do. Like Python's
    /// `os.environ.get`, a variable that is set but empty is used as-is (relative directory).
    pub fn from_env() -> io::Result<Paths> {
        Self::from_env_with_home(&home_dir())
    }

    /// Same as [`Paths::from_env`] with an injected home (used when `LOCALAPPDATA` is unset).
    pub fn from_env_with_home(home: &Path) -> io::Result<Paths> {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.to_path_buf());
        let data = base.join("claude-usage");
        std::fs::create_dir_all(&data)?;
        Ok(Self::with_root(data))
    }
}
