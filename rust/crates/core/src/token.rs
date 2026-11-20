//! OAuth token refresh (core.refresh_if_expired and helpers), ported branch by branch.
//!
//! SAFETY MODEL (do not reorder anything below without reading this): the token server ROTATES refresh tokens.
//! Once a POST succeeded the old refresh token is void, so
//!  1. the file is re-read UNDER the lock (the CLI may have refreshed meanwhile -> do nothing),
//!  2. `.credentials.json.claude-usage.bak` is written BEFORE the POST,
//!  3. between a successful POST and the write of the new credentials nothing may fail: all value
//!     computations are infallible, and the writer falls back to a direct (non-atomic) write on any error,
//!  4. `refreshTokenExpiresAt` is `min(old, new)` (never extended, like the CLI),
//!  5. a refresh token the server rejected (`invalid_grant`) is remembered in RAM only (sha256) and not sent
//!     again until the file holds a different one,
//!  6. the response body never reaches a log or an error string; only an OAuth code matching `^[a-z_]{1,40}$`.
//!
//! Improvements over Python (PORT_PLAN decisions): while the locks are held a thread touches their mtime every
//! 5 s (proper-lockfile behaviour, so a CLI never sees our lock as stale), and the total POST time (including
//! the `invalid_scope` retry) is capped at 45 s (< 50 s).
//!
//! Deviations: per-attempt timeout is a *total* 30 s deadline (Python: per socket operation); request body
//! is compact JSON (Python used `json.dumps` defaults with spaces; equivalent for the server); on a rename
//! error that is not "file in use" the writer falls back to the direct write too (Python raised after the
//! POST, losing the token); floats in the credentials file are re-serialised by serde_json (shortest
//! round-trip), which can differ textually from Python's repr for exponent forms.

use crate::config::Profile;
use crate::fetch::{
    err_value, io_exc_name, now_secs, parse_body, read_json, retry_after_value, Endpoints,
    CLIENT_ID, DEAD_MSG,
};
use crate::http::Http;
use crate::log::log;
use crate::paths::Paths;
use crate::pyval::{py_num, truthy};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// The CLI (proper-lockfile) touches its lock every 5 s and treats one older than 60 s as dead.
pub const LOCK_STALE: u64 = 90;
/// Lock keep-alive period.
pub const LOCK_TOUCH: Duration = Duration::from_secs(5);
/// Per-attempt POST timeout.
pub const POST_TIMEOUT: Duration = Duration::from_secs(30);
/// Cap on the whole POST phase (both attempts); must stay below 50 s.
pub const POST_BUDGET: Duration = Duration::from_secs(45);

// ------------------------------------------------------------------------------------------- dead tokens

fn dead_map() -> &'static Mutex<HashMap<String, String>> {
    static M: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

fn fp(token: &str) -> String {
    let d = Sha256::digest(token.as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn dead_get(cred: &Path) -> Option<String> {
    dead_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&cred.to_string_lossy().into_owned())
        .cloned()
}

fn dead_set(cred: &Path, fingerprint: String) {
    dead_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(cred.to_string_lossy().into_owned(), fingerprint);
}

/// Forget every remembered dead token (tests; `_dead.clear()` in Python).
pub fn dead_clear() {
    dead_map().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

// ----------------------------------------------------------------------------------------------- locks

/// `\\?\UNC\server\share` -> `\\server\share`, `\\?\C:\x` -> `C:\x` (what Python's `realpath` returns).
pub fn strip_verbatim(s: &str) -> String {
    if let Some(r) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{r}")
    } else if let Some(r) = s.strip_prefix(r"\\?\") {
        // only drive-letter forms; other verbatim paths (e.g. \\?\Volume{..}) are left alone
        let b = r.as_bytes();
        if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            r.to_string()
        } else {
            s.to_string()
        }
    } else {
        s.to_string()
    }
}

/// `os.path.realpath`: canonicalize (resolves junctions/symlinks) with the verbatim prefix removed; on failure
/// (missing path...) the original path, as Python 3.14's non-strict realpath does not raise.
pub fn real_path(p: &Path) -> PathBuf {
    match fs::canonicalize(p) {
        Ok(c) => PathBuf::from(strip_verbatim(&c.to_string_lossy())),
        Err(_) => p.to_path_buf(),
    }
}

/// The two lock directories the claude CLI creates before refreshing a profile's token (current + legacy).
pub fn lock_paths(d: &Path) -> Vec<PathBuf> {
    let real = real_path(d);
    let mut legacy = real.into_os_string();
    legacy.push(".lock");
    vec![d.join(".oauth_refresh.lock"), PathBuf::from(legacy)]
}

fn release(got: &[PathBuf]) {
    for p in got.iter().rev() {
        let _ = fs::remove_dir(p);
    }
}

fn touch_dir(p: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        if let Ok(f) = fs::OpenOptions::new()
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(p)
        {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(f) = fs::File::open(p) {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
    }
}

/// Held lock directories. Dropping stops the keep-alive thread (joined first, so it can never touch a
/// directory after it was released) and removes the directories in reverse order.
pub struct LockGuard {
    paths: Vec<PathBuf>,
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl LockGuard {
    /// m8: false when the keep-alive thread could not be spawned (the caller logs it).
    pub fn keepalive_running(&self) -> bool {
        self.thread.is_some()
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        {
            let (m, cv) = &*self.stop;
            *m.lock().unwrap_or_else(|e| e.into_inner()) = true;
            cv.notify_all();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        release(&self.paths);
    }
}

/// mkdir-based lock compatible with the CLI. `None` = someone else holds it (a dead holder's lock, older than
/// `stale_secs`, is taken over). Creation order and the stale rule are exactly Python's `_acquire`.
pub fn acquire(paths: &[PathBuf], stale_secs: u64, touch_every: Duration) -> Option<LockGuard> {
    let mut got: Vec<PathBuf> = Vec::new();
    for p in paths {
        match fs::create_dir(p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // `time.time() - mtime > LOCK_STALE`; an unreadable or future mtime is "not stale"
                let stale = fs::metadata(p)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age.as_secs_f64() > stale_secs as f64);
                if !stale || fs::remove_dir(p).is_err() || fs::create_dir(p).is_err() {
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
    let stop = Arc::new((Mutex::new(false), Condvar::new()));
    let (stop2, paths2) = (stop.clone(), got.clone());
    let thread = thread::Builder::new()
        .name("lock-keepalive".into())
        .spawn(move || {
            let (m, cv) = &*stop2;
            let began = Instant::now();
            let mut stopped = m.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                // check BEFORE waiting: a stop signalled before the first wait must not be missed
                if *stopped {
                    return;
                }
                let (g, res) = cv
                    .wait_timeout(stopped, touch_every)
                    .unwrap_or_else(|e| e.into_inner());
                stopped = g;
                if *stopped {
                    return;
                }
                // M3: a hung holder must look dead eventually: stop touching after the POST budget + 10 s
                if res.timed_out() && began.elapsed() <= POST_BUDGET + Duration::from_secs(10) {
                    for p in &paths2 {
                        touch_dir(p);
                    }
                }
            }
        })
        .ok();
    Some(LockGuard {
        paths: got,
        stop,
        thread,
    })
}

// -------------------------------------------------------------------------------------------- the POST

/// OAuth error code from an error body: only a short identifier (`^[a-z_]{1,40}$`), never the body.
fn oauth_error(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let v: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    let err = match v.as_object().and_then(|o| o.get("error")) {
        Some(Value::Object(o)) => o.get("type").cloned(),
        other => other.cloned(),
    };
    match err {
        Some(Value::String(s))
            if (1..=40).contains(&s.len())
                && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') =>
        {
            s
        }
        _ => String::new(),
    }
}

/// POST the refresh grant. `Ok` = token response (object with access_token and numeric expires_in),
/// `Err` = `{err, status[, dead, retry_after]}`.
fn post_refresh(
    http: &Http,
    ep: &Endpoints,
    rt: &str,
    scope: Option<String>,
) -> Result<Map<String, Value>, Value> {
    let mut body = Map::new();
    body.insert("grant_type".into(), json!("refresh_token"));
    body.insert("refresh_token".into(), json!(rt));
    body.insert("client_id".into(), json!(CLIENT_ID));
    if let Some(s) = scope {
        body.insert("scope".into(), json!(s));
    }
    let start = Instant::now();
    for attempt in 0..2 {
        let remaining = POST_BUDGET.saturating_sub(start.elapsed());
        if remaining < Duration::from_secs(1) {
            return Err(err_value("làm mới token: TimeoutError", 0));
        }
        let payload = serde_json::to_vec(&Value::Object(body.clone())).unwrap_or_default();
        let ua = format!("claude-usage/{}", env!("CARGO_PKG_VERSION"));
        let resp = match http.request(
            "POST",
            &ep.token,
            &[("Content-Type", "application/json"), ("User-Agent", &ua)],
            Some(&payload),
            POST_TIMEOUT.min(remaining),
        ) {
            Ok(r) => r,
            Err(name) => return Err(err_value(&format!("làm mới token: {name}"), 0)),
        };
        if resp.is_success() {
            let d = match parse_body(&resp.body) {
                Ok(d) => d,
                Err(name) => return Err(err_value(&format!("làm mới token: {name}"), 0)),
            };
            return match d {
                Value::Object(m)
                    if m.get("access_token").is_some_and(truthy)
                        && m.get("expires_in").and_then(py_num).is_some() =>
                {
                    Ok(m)
                }
                other => {
                    // M2: the server may already have rotated the token: hand the new one to the caller to save
                    let mut e = err_value("làm mới token: phản hồi không hợp lệ", 0);
                    if let Value::Object(m) = other {
                        if let Some(nrt) = m.get("refresh_token").filter(|v| truthy(v)) {
                            e["_new_refresh_token"] = nrt.clone();
                        }
                    }
                    Err(e)
                }
            };
        }
        let code = oauth_error(&resp.body);
        if code == "invalid_scope" && body.contains_key("scope") && attempt == 0 {
            body.remove("scope"); // like the CLI: retry once with the originally granted scopes
            continue;
        }
        if code == "invalid_grant" {
            return Err(json!({"err": DEAD_MSG, "status": 401, "dead": true}));
        }
        let mut m = Map::new();
        let suffix = if code.is_empty() {
            String::new()
        } else {
            format!(" {code}")
        };
        m.insert(
            "err".into(),
            json!(format!("làm mới token: HTTP {}{}", resp.status, suffix)),
        );
        m.insert("status".into(), json!(resp.status));
        m.insert(
            "retry_after".into(),
            retry_after_value(resp.retry_after.as_deref()),
        );
        return Err(Value::Object(m));
    }
    Err(err_value("làm mới token: invalid_scope", 400))
}

// ------------------------------------------------------------------------------------------ the write

/// Atomic replace (temp file + rename, retried 50 x 20 ms while a reader has the file open), compact JSON.
/// Last resort: write the file directly. Unlike Python this also falls back when the temp write or a rename
/// fails for a reason other than "in use": after a successful POST the new token must never be dropped.
fn write_creds(cred: &Path, data: &Value) -> std::io::Result<()> {
    // m6: never write an empty file over the credentials
    let text = serde_json::to_string(data).map_err(std::io::Error::other)?;
    if text.is_empty() {
        return Err(std::io::Error::other("empty credentials text"));
    }
    let name = cred
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = cred.with_file_name(format!("{name}.{}.claude-usage.tmp", std::process::id()));
    if fs::write(&tmp, &text).is_ok() {
        for _ in 0..50 {
            match fs::rename(&tmp, cred) {
                Ok(()) => return Ok(()),
                Err(e) if crate::atomic::is_retryable(&e) => {
                    thread::sleep(Duration::from_millis(20))
                }
                Err(_) => break,
            }
        }
    }
    // Last resort: the old refresh token may already be void, never drop the new one. The tmp file stays
    // (it is the only copy) until the direct write has succeeded; retry ~5s for a sharing violation.
    let mut last = None;
    for _ in 0..250 {
        match fs::write(cred, &text) {
            Ok(()) => {
                let _ = fs::remove_file(&tmp);
                return Ok(());
            }
            Err(e) if crate::atomic::is_retryable(&e) => {
                last = Some(e);
                thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                last = Some(e);
                break;
            }
        }
    }
    // Could not write at all: keep the new token next to the credentials file as `.new`.
    let keep = cred.with_file_name(format!("{name}.claude-usage.new"));
    if !tmp.exists() || fs::rename(&tmp, &keep).is_err() {
        let _ = fs::write(&keep, &text);
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("credentials write failed")))
}

// ------------------------------------------------------------------------------------- the main entry

fn exp_secs(o: &Map<String, Value>) -> Result<f64, &'static str> {
    // `o.get("expiresAt", 0) / 1000`
    let v = o.get("expiresAt").cloned().unwrap_or(Value::from(0));
    py_num(&v).map(|n| n / 1000.0).ok_or("TypeError")
}

/// If the profile's access token has expired, get a new one with its refresh token and write it back to
/// `.credentials.json` the way the claude CLI does. `None` = nothing to do or it worked (the caller then
/// fetches); otherwise `{err, status[, dead, retry_after]}`.
pub fn refresh_if_expired(
    paths: &Paths,
    http: &Http,
    ep: &Endpoints,
    profile: &Profile,
) -> Option<Value> {
    refresh_with(paths, http, ep, profile, &RefreshOpts::default())
}

/// Knobs of [`refresh_with`]; the defaults are the production values.
pub struct RefreshOpts<'a> {
    pub stale_secs: u64,
    pub touch_every: Duration,
    /// Called right before the lock is taken (tests simulate "the CLI rewrote the file meanwhile").
    pub before_lock: Option<&'a dyn Fn()>,
}

impl Default for RefreshOpts<'_> {
    fn default() -> Self {
        RefreshOpts {
            stale_secs: LOCK_STALE,
            touch_every: LOCK_TOUCH,
            before_lock: None,
        }
    }
}

/// [`refresh_if_expired`] with lock timings / a test hook injectable.
pub fn refresh_with(
    paths: &Paths,
    http: &Http,
    ep: &Endpoints,
    profile: &Profile,
    opts: &RefreshOpts,
) -> Option<Value> {
    let cred = Path::new(&profile.path).join(".credentials.json");
    // First read, outside the lock: any problem -> None ("unreadable: fetch() reports it").
    let (rt, rte) = {
        let o = read_json(&cred).ok().and_then(|(_, v)| match v {
            Value::Object(mut r) => r.remove("claudeAiOauth"),
            _ => None,
        })?;
        let o = o.as_object()?;
        let exp = exp_secs(o).ok()?;
        let rt = match o.get("refreshToken") {
            Some(Value::String(s)) if !s.is_empty() => s.clone(),
            _ => return None,
        };
        if exp >= now_secs() {
            return None; // still valid
        }
        (rt, o.get("refreshTokenExpiresAt").cloned())
    };
    let rte_past = rte
        .as_ref()
        .and_then(py_num)
        .is_some_and(|n| n / 1000.0 < now_secs());
    if dead_get(&cred).is_some_and(|d| d == fp(&rt)) || rte_past {
        return Some(json!({"err": DEAD_MSG, "status": 401, "dead": true}));
    }
    if let Some(h) = opts.before_lock {
        h();
    }
    let guard = match acquire(
        &lock_paths(Path::new(&profile.path)),
        opts.stale_secs,
        opts.touch_every,
    ) {
        Some(g) => g,
        None => {
            return Some(
                json!({"err": "claude đang làm mới token, thử lại sau", "status": 0, "retry_after": 30}),
            )
        }
    };
    if !guard.keepalive_running() {
        log(paths, "lock keep-alive thread failed to start");
    }
    let result = locked_refresh(paths, http, ep, profile, &cred, &rt);
    drop(guard); // keep-alive stopped, locks released in reverse order (also on panic via Drop)
    match result {
        Ok(None) => None,
        Ok(Some(v)) => Some(v),
        Err(name) => Some(err_value(&format!("làm mới token: {name}"), 0)),
    }
}

fn locked_refresh(
    paths: &Paths,
    http: &Http,
    ep: &Endpoints,
    profile: &Profile,
    cred: &Path,
    rt: &str,
) -> Result<Option<Value>, &'static str> {
    // Re-read under the lock: claude may have refreshed meanwhile.
    let (old_text, mut data) = read_json(cred)?;
    let root = data.as_object_mut().ok_or("TypeError")?;
    let o = match root.get("claudeAiOauth") {
        None => return Err("KeyError"),
        Some(Value::Object(o)) => o.clone(),
        Some(_) => return Err("AttributeError"),
    };
    if o.get("refreshToken") != Some(&Value::String(rt.to_string())) || exp_secs(&o)? >= now_secs()
    {
        return Ok(None);
    }
    // Backup BEFORE the POST: once the server rotates the token, nothing may fail before the write.
    let bak = {
        let name = cred
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        cred.with_file_name(format!("{name}.claude-usage.bak"))
    };
    fs::write(&bak, &old_text).map_err(|e| io_exc_name(&e))?;
    // `" ".join(scopes)` if scopes is truthy (all of this is still before the POST)
    let scope = match o.get("scopes") {
        Some(s) if truthy(s) => Some(match s {
            Value::Array(a) => {
                let mut parts = Vec::new();
                for x in a {
                    parts.push(x.as_str().ok_or("TypeError")?.to_string());
                }
                parts.join(" ")
            }
            Value::String(st) => st.chars().map(String::from).collect::<Vec<_>>().join(" "),
            _ => return Err("TypeError"),
        }),
        _ => None,
    };
    let r = match post_refresh(http, ep, rt, scope) {
        Ok(r) => r,
        Err(mut e) => {
            if e.get("dead").is_some_and(truthy) {
                dead_set(cred, fp(rt));
            }
            if let Some(nrt) = e.as_object_mut().and_then(|m| m.remove("_new_refresh_token")) {
                // M2: keep the rotated refresh token (access token left expired), then report the error
                let mut new = o.clone();
                new.insert("refreshToken".into(), nrt);
                new.insert("expiresAt".into(), Value::from((now_secs() * 1000.0) as i64));
                if let Some(root) = data.as_object_mut() {
                    root.insert("claudeAiOauth".into(), Value::Object(new));
                }
                let _ = write_creds(cred, &data);
            }
            return Ok(Some(e));
        }
    };
    // ---- From here to the end of write_creds nothing may fail (the old refresh token is void). ----
    let now_ms = (now_secs() * 1000.0) as i64;
    let mut new = o.clone();
    new.insert("accessToken".into(), r["access_token"].clone());
    let new_rt = r
        .get("refresh_token")
        .filter(|v| truthy(v))
        .cloned()
        .unwrap_or_else(|| Value::String(rt.to_string()));
    new.insert("refreshToken".into(), new_rt);
    let exp_in = r.get("expires_in").and_then(py_num).unwrap_or(0.0);
    new.insert(
        "expiresAt".into(),
        Value::from(now_ms.saturating_add((exp_in * 1000.0) as i64)),
    );
    if let Some(Value::String(sc)) = r.get("scope") {
        let list: Vec<Value> = sc.split_whitespace().map(|s| json!(s)).collect();
        if !list.is_empty() {
            new.insert("scopes".into(), Value::Array(list));
        }
    }
    if let (Some(old), Some(rte_in)) = (
        o.get("refreshTokenExpiresAt")
            .filter(|v| py_num(v).is_some()),
        r.get("refresh_token_expires_in").and_then(py_num),
    ) {
        let cand = now_ms.saturating_add((rte_in * 1000.0) as i64);
        // min(old, cand): the candidate only wins when strictly smaller (the CLI never extends it)
        let old_n = old
            .as_i64()
            .map(|i| i as f64)
            .or_else(|| py_num(old))
            .unwrap_or(f64::MAX);
        if (cand as f64) < old_n {
            new.insert("refreshTokenExpiresAt".into(), Value::from(cand));
        }
    }
    if let Some(root) = data.as_object_mut() {
        root.insert("claudeAiOauth".into(), Value::Object(new));
    }
    match write_creds(cred, &data) {
        Ok(()) => {
            log(paths, &format!("token refreshed: {}", profile.name));
            Ok(None)
        }
        Err(e) => Err(io_exc_name(&e)),
    }
}
