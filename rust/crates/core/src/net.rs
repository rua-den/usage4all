//! Usage fetch + OAuth token refresh: ports of `core.fetch`, `core.refresh_if_expired`, `_acquire/_release`,
//! `_post_refresh`, `_write_creds` (see PORT_PLAN Phase 3).
//!
//! Network access goes through the [`Http`] trait so tests use an in-process fake: there is NO way to point the
//! real client at another endpoint (the URLs are constants). Secrets (access/refresh tokens, response bodies)
//! never reach a log line or an error string: errors carry only fixed text, HTTP status codes and a short
//! `[a-z_]{1,40}` OAuth error code.

use crate::atomic::atomic_write;
use crate::cache::now_ts;
use crate::config::Profile;
use crate::parse::{parse_extra, parse_scoped};
use crate::pyval::{py_num, truthy};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

pub const API: &str = "https://api.anthropic.com/api/oauth/usage";
pub const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
/// Public client id of the claude CLI.
pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub const DEAD_MSG: &str = "phiên đăng nhập hết hạn - chạy claude ở profile này rồi /login";
/// The CLI (proper-lockfile) touches its lock every 5 s and treats one older than 60 s as dead.
pub const LOCK_STALE: f64 = 90.0;
const LOCK_TOUCH: Duration = Duration::from_secs(5);
const USAGE_TIMEOUT: Duration = Duration::from_secs(20);
/// Two POST attempts (invalid_scope retry) must stay below the CLI's 60 s stale threshold.
const POST_TIMEOUT: Duration = Duration::from_secs(25);

pub type FetchResult = Map<String, Value>;

/// A completed HTTP exchange. Non-2xx statuses are `Ok` with their status; only transport failures are `Err`.
pub struct HttpResponse {
    pub status: u16,
    /// Raw `Retry-After` header value.
    pub retry_after: Option<String>,
    pub body: Vec<u8>,
}

/// Transport. `Err(name)` = could not complete the exchange; `name` is a short error class like Python's
/// `type(e).__name__` (`"TimeoutError"`, `"URLError"`...). It must never contain URL, headers or body.
pub trait Http {
    fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<HttpResponse, String>;
    fn post_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &str,
        timeout: Duration,
    ) -> Result<HttpResponse, String>;
}

fn err(msg: impl Into<String>, status: i64) -> FetchResult {
    let mut m = Map::new();
    m.insert("err".into(), Value::from(msg.into()));
    m.insert("status".into(), Value::from(status));
    m
}

fn err_ra(msg: String, status: u16, retry_after: Option<&str>) -> FetchResult {
    let mut m = err(msg, status as i64);
    m.insert(
        "retry_after".into(),
        retry_after
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse::<i64>().ok())
            .map_or(Value::Null, Value::from),
    );
    m
}

fn cred_path(p: &Profile) -> PathBuf {
    Path::new(&p.path).join(".credentials.json")
}

/// `json.loads(cred.read_text())`.
fn read_creds(cred: &Path) -> Option<(String, Value)> {
    let text = fs::read_to_string(cred).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    Some((text, v))
}

/// `o.get("expiresAt", 0) / 1000`: absent -> 0 (expired); a non-number makes Python raise -> `None`.
fn expires_at_secs(o: &Map<String, Value>) -> Option<f64> {
    match o.get("expiresAt") {
        None => Some(0.0),
        Some(v) => py_num(v).filter(|_| v.is_number()).map(|x| x / 1000.0),
    }
}

/// `core.fetch`: usage numbers, or `{err, status[, retry_after]}`.
/// Deviation: a non-numeric `expiresAt` or a missing `accessToken` (Python raises) report as unreadable credentials.
pub fn fetch(http: &dyn Http, profile: &Profile) -> FetchResult {
    let unreadable = || err("không đọc được credentials", 0);
    let Some((_, v)) = read_creds(&cred_path(profile)) else {
        return unreadable();
    };
    let Some(o) = v.get("claudeAiOauth").and_then(Value::as_object) else {
        return unreadable();
    };
    let Some(exp) = expires_at_secs(o) else {
        return unreadable();
    };
    if exp < now_ts() {
        return err("token hết hạn - chạy claude ở profile này 1 lần", 401);
    }
    let Some(token) = o.get("accessToken").and_then(Value::as_str) else {
        return unreadable();
    };
    let bearer = format!("Bearer {token}");
    let headers = [
        ("Authorization", bearer.as_str()),
        ("anthropic-beta", "oauth-2025-04-20"),
    ];
    let resp = match http.get(API, &headers, USAGE_TIMEOUT) {
        Ok(r) => r,
        Err(name) => return err(name, 0),
    };
    if !(200..300).contains(&resp.status) {
        return err_ra(
            format!("HTTP {}", resp.status),
            resp.status,
            resp.retry_after.as_deref(),
        );
    }
    let Ok(d) = serde_json::from_slice::<Value>(&resp.body) else {
        return err("JSONDecodeError", 0);
    };
    let Some(d) = d.as_object() else {
        return err("AttributeError", 0);
    };
    let blk = |k: &str| {
        d.get(k)
            .filter(|v| truthy(v))
            .and_then(Value::as_object)
            .cloned()
    };
    let (f, s) = (
        blk("five_hour").unwrap_or_default(),
        blk("seven_day").unwrap_or_default(),
    );
    let util = |m: &Map<String, Value>| {
        m.get("utilization")
            .filter(|v| truthy(v))
            .cloned()
            .unwrap_or_else(|| Value::from(0))
    };
    let at = |m: &Map<String, Value>| m.get("resets_at").cloned().unwrap_or(Value::Null);
    let mut out = Map::new();
    out.insert("five".into(), util(&f));
    out.insert("five_reset".into(), at(&f));
    out.insert("week".into(), util(&s));
    out.insert("week_reset".into(), at(&s));
    out.insert(
        "extra".into(),
        parse_extra(d.get("extra_usage").unwrap_or(&Value::Null)).unwrap_or(Value::Null),
    );
    out.insert(
        "scoped".into(),
        Value::Array(parse_scoped(d.get("limits").unwrap_or(&Value::Null))),
    );
    out.insert(
        "plan".into(),
        o.get("subscriptionType").cloned().unwrap_or(Value::Null),
    );
    out
}

// ---------------------------------------------------------------------------------------------- locks

/// Python `os.path.realpath` flavour: `canonicalize` with the `\\?\` prefixes stripped; on failure the path as is.
pub fn real_path(p: &Path) -> PathBuf {
    match fs::canonicalize(p) {
        Ok(c) => {
            let s = c.to_string_lossy();
            if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
                PathBuf::from(format!(r"\\{rest}"))
            } else if let Some(rest) = s.strip_prefix(r"\\?\") {
                PathBuf::from(rest)
            } else {
                c
            }
        }
        Err(_) => p.to_path_buf(),
    }
}

/// The two lock directories the CLI creates before refreshing a profile's token (current + legacy).
pub fn lock_paths(profile_dir: &Path) -> [PathBuf; 2] {
    let mut legacy = real_path(profile_dir).into_os_string();
    legacy.push(".lock");
    [
        profile_dir.join(".oauth_refresh.lock"),
        PathBuf::from(legacy),
    ]
}

/// Held lock directories. While alive, a thread touches their mtime every 5 s like proper-lockfile so a
/// slow POST cannot make the CLI think the lock is dead. Released in reverse order on drop.
pub struct LockGuard {
    paths: Vec<PathBuf>,
    stop: Arc<AtomicBool>,
    keeper: Option<JoinHandle<()>>,
}

fn touch_dir(p: &Path) {
    // Directories need FILE_FLAG_BACKUP_SEMANTICS to be opened on Windows.
    if let Ok(f) = OpenOptions::new()
        .write(true)
        .custom_flags(0x0200_0000)
        .open(p)
    {
        let _ = f.set_modified(SystemTime::now());
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.keeper.take() {
            let _ = h.join();
        }
        for p in self.paths.iter().rev() {
            let _ = fs::remove_dir(p);
        }
    }
}

fn release(got: &[PathBuf]) {
    for p in got.iter().rev() {
        let _ = fs::remove_dir(p);
    }
}

/// mkdir-based lock compatible with the CLI. `None` = someone else holds it (a dead holder's lock, older
/// than `stale_secs`, is taken over).
pub fn acquire(paths: &[PathBuf], stale_secs: f64, touch_every: Duration) -> Option<LockGuard> {
    let mut got: Vec<PathBuf> = Vec::new();
    for p in paths {
        match fs::create_dir(p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale = fs::metadata(p)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| SystemTime::now().duration_since(t).ok())
                    .is_some_and(|age| age.as_secs_f64() > stale_secs);
                if !stale || fs::remove_dir(p).and_then(|_| fs::create_dir(p)).is_err() {
                    release(&got);
                    return None;
                }
            }
            Err(_) => {
                release(&got);
                return None;
            }
        }
        got.push(p.clone());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let (s2, held) = (stop.clone(), got.clone());
    let keeper = thread::spawn(move || {
        let mut waited = Duration::ZERO;
        let tick = Duration::from_millis(50).min(touch_every);
        while !s2.load(Ordering::SeqCst) {
            thread::sleep(tick);
            waited += tick;
            if waited >= touch_every {
                waited = Duration::ZERO;
                held.iter().for_each(|p| touch_dir(p));
            }
        }
    });
    Some(LockGuard {
        paths: got,
        stop,
        keeper: Some(keeper),
    })
}

// ---------------------------------------------------------------------------------------------- refresh

fn fp(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// OAuth error code of an error body: only a short `[a-z_]{1,40}` identifier, never the body itself.
fn oauth_error(body: &[u8]) -> String {
    let code = serde_json::from_slice::<Value>(body).ok().and_then(|v| {
        let e = v.get("error")?.clone();
        let e = match e {
            Value::Object(m) => m.get("type").cloned().unwrap_or(Value::Null),
            other => other,
        };
        e.as_str().map(str::to_owned)
    });
    match code {
        Some(c)
            if (1..=40).contains(&c.len())
                && c.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') =>
        {
            c
        }
        _ => String::new(),
    }
}

enum Post {
    Tokens(Map<String, Value>),
    Fail { res: FetchResult, dead: bool },
}

fn fail(res: FetchResult) -> Post {
    Post::Fail { res, dead: false }
}

/// POST the refresh grant (same body as the CLI), retrying once without `scope` on `invalid_scope`.
fn post_refresh(http: &dyn Http, rt: &str, scopes: Option<&str>) -> Post {
    let mut body =
        json!({"grant_type": "refresh_token", "refresh_token": rt, "client_id": CLIENT_ID});
    if let Some(s) = scopes {
        body["scope"] = Value::from(s);
    }
    let ua = format!("claude-usage/{}", env!("CARGO_PKG_VERSION"));
    let headers = [
        ("Content-Type", "application/json"),
        ("User-Agent", ua.as_str()),
    ];
    for attempt in 0..2 {
        let text = body.to_string();
        let r = match http.post_json(TOKEN_URL, &headers, &text, POST_TIMEOUT) {
            Ok(r) => r,
            Err(name) => return fail(err(format!("làm mới token: {name}"), 0)),
        };
        if (200..300).contains(&r.status) {
            let Ok(d) = serde_json::from_slice::<Value>(&r.body) else {
                return fail(err("làm mới token: JSONDecodeError", 0));
            };
            let valid = d.as_object().is_some_and(|m| {
                m.get("access_token").is_some_and(truthy)
                    && m.get("expires_in").is_some_and(Value::is_number)
            });
            return match d {
                Value::Object(m) if valid => Post::Tokens(m),
                _ => fail(err("làm mới token: phản hồi không hợp lệ", 0)),
            };
        }
        let code = oauth_error(&r.body);
        if code == "invalid_scope" && body.get("scope").is_some() && attempt == 0 {
            body.as_object_mut().map(|m| m.remove("scope"));
            continue;
        }
        if code == "invalid_grant" {
            let mut res = err(DEAD_MSG, 401);
            res.insert("dead".into(), Value::Bool(true));
            return Post::Fail { res, dead: true };
        }
        let msg = if code.is_empty() {
            format!("làm mới token: HTTP {}", r.status)
        } else {
            format!("làm mới token: HTTP {} {code}", r.status)
        };
        return fail(err_ra(msg, r.status, r.retry_after.as_deref()));
    }
    fail(err("làm mới token: invalid_scope", 400))
}

/// Atomic replace of `.credentials.json` (temp file + rename retried while a reader has it open), compact JSON.
fn write_creds(cred: &Path, data: &Value) -> std::io::Result<()> {
    let text = serde_json::to_string(data).map_err(std::io::Error::other)?;
    let name = cred.file_name().unwrap_or_default().to_string_lossy();
    let tmp = cred.with_file_name(format!("{name}.{}.claude-usage.tmp", std::process::id()));
    atomic_write(cred, &tmp, &text, 50, Duration::from_millis(20))
}

/// Refreshes a profile's access token once it has expired, the way the CLI does. Holds the only in-memory
/// state: fingerprints of refresh tokens the server rejected.
pub struct Oauth {
    dead: HashMap<String, String>,
    log: Box<dyn Fn(&str) + Send>,
    stale_secs: f64,
    touch_every: Duration,
}

impl Oauth {
    pub fn new(log: impl Fn(&str) + Send + 'static) -> Self {
        Oauth {
            dead: HashMap::new(),
            log: Box::new(log),
            stale_secs: LOCK_STALE,
            touch_every: LOCK_TOUCH,
        }
    }

    /// `core.refresh_if_expired`: `None` = nothing to do or it worked (then [`fetch`] reads the file), else
    /// `{err, status[, dead, retry_after]}`. `dead`: the server rejected the refresh token, so it is not sent
    /// again until the file holds a different one.
    pub fn refresh_if_expired(
        &mut self,
        http: &dyn Http,
        profile: &Profile,
    ) -> Option<FetchResult> {
        let cred = cred_path(profile);
        let (_, v) = read_creds(&cred)?;
        let o = v.get("claudeAiOauth")?.as_object()?;
        let rt = o
            .get("refreshToken")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())?;
        if expires_at_secs(o)? >= now_ts() {
            return None;
        }
        let key = cred.to_string_lossy().into_owned();
        let rte_gone = o
            .get("refreshTokenExpiresAt")
            .filter(|v| v.is_number())
            .and_then(py_num)
            .is_some_and(|x| x / 1000.0 < now_ts());
        if self.dead.get(&key).is_some_and(|f| *f == fp(rt)) || rte_gone {
            let mut res = err(DEAD_MSG, 401);
            res.insert("dead".into(), Value::Bool(true));
            return Some(res);
        }
        let rt = rt.to_owned();
        let Some(_guard) = acquire(
            &lock_paths(Path::new(&profile.path)),
            self.stale_secs,
            self.touch_every,
        ) else {
            let mut res = err("claude đang làm mới token, thử lại sau", 0);
            res.insert("retry_after".into(), Value::from(30));
            return Some(res);
        };
        // Re-read under the lock: claude may have refreshed meanwhile.
        let Some((old_text, mut data)) = read_creds(&cred) else {
            return Some(err("làm mới token: KeyError", 0));
        };
        let Some(o) = data
            .get("claudeAiOauth")
            .and_then(Value::as_object)
            .cloned()
        else {
            return Some(err("làm mới token: KeyError", 0));
        };
        if o.get("refreshToken").and_then(Value::as_str) != Some(rt.as_str())
            || expires_at_secs(&o).is_none_or(|e| e >= now_ts())
        {
            return None;
        }
        // Backup BEFORE the POST: once the server rotates the token nothing may fail before the write.
        let bak = cred.with_file_name(format!(
            "{}.claude-usage.bak",
            cred.file_name().unwrap_or_default().to_string_lossy()
        ));
        if let Err(e) = fs::write(&bak, &old_text) {
            return Some(err(format!("làm mới token: {}", io_name(&e)), 0));
        }
        let scopes = match o.get("scopes").filter(|v| truthy(v)) {
            None => None,
            Some(Value::Array(a)) => {
                let parts: Option<Vec<&str>> = a.iter().map(Value::as_str).collect();
                match parts {
                    Some(p) => Some(p.join(" ")),
                    None => return Some(err("làm mới token: TypeError", 0)),
                }
            }
            Some(_) => return Some(err("làm mới token: TypeError", 0)),
        };
        let r = match post_refresh(http, &rt, scopes.as_deref()) {
            Post::Tokens(r) => r,
            Post::Fail { res, dead } => {
                if dead {
                    self.dead.insert(key, fp(&rt));
                }
                return Some(res);
            }
        };
        let now_ms = (now_ts() * 1000.0) as i64;
        let mut new = o.clone();
        new.insert("accessToken".into(), r["access_token"].clone());
        let next_rt = r.get("refresh_token").filter(|v| truthy(v)).cloned();
        new.insert(
            "refreshToken".into(),
            next_rt.unwrap_or_else(|| Value::from(rt.as_str())),
        );
        let expires_in = r.get("expires_in").and_then(py_num).unwrap_or(0.0);
        new.insert(
            "expiresAt".into(),
            Value::from(now_ms + (expires_in * 1000.0) as i64),
        );
        if let Some(sc) = r.get("scope").and_then(Value::as_str) {
            let list: Vec<Value> = sc.split_whitespace().map(Value::from).collect();
            if !list.is_empty() {
                new.insert("scopes".into(), Value::Array(list));
            }
        }
        // The CLI never extends the refresh token's lifetime on refresh.
        let old_rte = o.get("refreshTokenExpiresAt").filter(|v| v.is_number());
        let rte_in = r.get("refresh_token_expires_in").filter(|v| v.is_number());
        if let (Some(old), Some(inn)) = (old_rte, rte_in) {
            let cand = now_ms + (inn.as_f64().unwrap_or(0.0) * 1000.0) as i64;
            let keep_old = py_num(old).is_some_and(|x| x <= cand as f64);
            new.insert(
                "refreshTokenExpiresAt".into(),
                if keep_old {
                    old.clone()
                } else {
                    Value::from(cand)
                },
            );
        }
        data["claudeAiOauth"] = Value::Object(new);
        if let Err(e) = write_creds(&cred, &data) {
            return Some(err(format!("làm mới token: {}", io_name(&e)), 0));
        }
        (self.log)(&format!("token refreshed: {}", profile.name));
        None
    }
}

/// Short, secret-free class name of an I/O error (like Python's exception type name).
fn io_name(e: &std::io::Error) -> &'static str {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => "PermissionError",
        std::io::ErrorKind::NotFound => "FileNotFoundError",
        _ => "OSError",
    }
}
