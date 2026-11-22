//! Phase 3 tests: fetch + OAuth token refresh against an in-process fake transport. Only fake tokens, only
//! temp directories (nothing real is read or written, nothing touches the network).

use cu_core::config::Profile;
use cu_core::net::{self, Http, HttpResponse, Oauth, DEAD_MSG};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

const OLD_RT: &str = "old-refresh-not-real";
const OLD_AT: &str = "old-access-not-real";
const NEW_AT: &str = "new-access-not-real";
const NEW_RT: &str = "new-refresh-not-real";

type Hdrs = Vec<(String, String)>;
type Post = (String, Hdrs, Value);

#[derive(Default)]
struct Calls {
    gets: Vec<(String, Vec<(String, String)>)>,
    posts: Vec<Post>,
}

#[derive(Clone)]
struct Fake {
    calls: Rc<RefCell<Calls>>,
    get_q: Rc<RefCell<VecDeque<Result<HttpResponse, String>>>>,
    post_q: Rc<RefCell<VecDeque<Result<HttpResponse, String>>>>,
}

fn resp(status: u16, body: Value) -> Result<HttpResponse, String> {
    Ok(HttpResponse {
        status,
        retry_after: None,
        body: body.to_string().into_bytes(),
    })
}

impl Fake {
    fn new() -> Fake {
        Fake {
            calls: Default::default(),
            get_q: Default::default(),
            post_q: Default::default(),
        }
    }
    fn on_get(&self, r: Result<HttpResponse, String>) {
        self.get_q.borrow_mut().push_back(r);
    }
    fn on_post(&self, r: Result<HttpResponse, String>) {
        self.post_q.borrow_mut().push_back(r);
    }
    fn posts(&self) -> usize {
        self.calls.borrow().posts.len()
    }
}

fn hdrs(h: &[(&str, &str)]) -> Vec<(String, String)> {
    h.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

impl Http for Fake {
    fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        _t: Duration,
    ) -> Result<HttpResponse, String> {
        self.calls
            .borrow_mut()
            .gets
            .push((url.into(), hdrs(headers)));
        self.get_q.borrow_mut().pop_front().expect("unscripted GET")
    }
    fn post_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &str,
        _t: Duration,
    ) -> Result<HttpResponse, String> {
        let b: Value = serde_json::from_str(body).unwrap();
        self.calls
            .borrow_mut()
            .posts
            .push((url.into(), hdrs(headers), b));
        self.post_q
            .borrow_mut()
            .pop_front()
            .expect("unscripted POST")
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

struct Sandbox {
    _tmp: tempfile::TempDir,
    profile: Profile,
    dir: PathBuf,
}

impl Sandbox {
    fn cred(&self) -> PathBuf {
        self.dir.join(".credentials.json")
    }
    fn read(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.cred()).unwrap()).unwrap()
    }
    fn write(&self, v: &Value) {
        fs::write(self.cred(), v.to_string()).unwrap();
    }
}

fn creds(expires_at: i64, extra: Value) -> Value {
    let mut o = json!({
        "accessToken": OLD_AT, "refreshToken": OLD_RT, "expiresAt": expires_at,
        "scopes": ["user:profile", "user:inference"], "subscriptionType": "team",
        "refreshTokenExpiresAt": now_ms() + 86_400_000_i64 * 30, "futureField": {"keep": [1, 2]},
    });
    if let Value::Object(e) = extra {
        for (k, v) in e {
            o[k] = v;
        }
    }
    json!({"claudeAiOauth": o, "otherTopLevel": "ünï"})
}

fn sandbox(c: Value) -> Sandbox {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("profile-a");
    fs::create_dir_all(&dir).unwrap();
    let sb = Sandbox {
        profile: Profile {
            name: "a".into(),
            path: dir.to_string_lossy().into_owned(),
        },
        dir,
        _tmp: tmp,
    };
    sb.write(&c);
    sb
}

fn expired() -> Value {
    creds(now_ms() - 60_000, json!({}))
}

fn token_ok() -> Result<HttpResponse, String> {
    resp(
        200,
        json!({"access_token": NEW_AT, "refresh_token": NEW_RT, "expires_in": 28800,
        "scope": "user:profile user:inference user:file_upload", "refresh_token_expires_in": 99_999_999}),
    )
}

fn oauth() -> (Oauth, Arc<Mutex<Vec<String>>>) {
    let logs = Arc::new(Mutex::new(Vec::new()));
    let l2 = logs.clone();
    (
        Oauth::new(move |m| l2.lock().unwrap().push(m.to_string())),
        logs,
    )
}

fn s(r: &net::FetchResult, k: &str) -> String {
    r.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

// ------------------------------------------------------------------------------------------ fetch

#[test]
fn fetch_ok_maps_fields_and_sends_headers() {
    let sb = sandbox(creds(now_ms() + 3_600_000, json!({})));
    let f = Fake::new();
    f.on_get(resp(200, json!({
        "five_hour": {"utilization": 23.0, "resets_at": "2026-01-01T10:00:00+00:00"},
        "seven_day": {"utilization": 0, "resets_at": null},
        "extra_usage": {"is_enabled": true, "monthly_limit": 2500, "used_credits": 98, "utilization": 3.92, "currency": "USD"},
        "limits": [{"kind": "weekly_scoped", "scope": {"model": {"display_name": "Fable"}}, "percent": 21, "resets_at": "R"}],
    })));
    let r = net::fetch(&f, &sb.profile);
    assert_eq!(r["five"], json!(23.0));
    assert_eq!(r["five_reset"], json!("2026-01-01T10:00:00+00:00"));
    assert_eq!(r["week"], json!(0));
    assert_eq!(r["week_reset"], Value::Null);
    assert_eq!(r["extra"]["limit"], json!(25.0));
    assert_eq!(r["scoped"][0]["name"], json!("Fable"));
    assert_eq!(r["plan"], json!("team"));
    assert!(!r.contains_key("err"));
    let c = f.calls.borrow();
    assert_eq!(c.gets[0].0, "https://api.anthropic.com/api/oauth/usage");
    assert!(c.gets[0]
        .1
        .contains(&("Authorization".into(), format!("Bearer {OLD_AT}"))));
    assert!(c.gets[0]
        .1
        .contains(&("anthropic-beta".into(), "oauth-2025-04-20".into())));
}

#[test]
fn fetch_errors() {
    let f = Fake::new();
    let sb = sandbox(creds(now_ms() + 3_600_000, json!({})));
    f.on_get(Ok(HttpResponse {
        status: 429,
        retry_after: Some("77".into()),
        body: b"{}".to_vec(),
    }));
    let r = net::fetch(&f, &sb.profile);
    assert_eq!(
        (s(&r, "err"), r["status"].clone(), r["retry_after"].clone()),
        ("HTTP 429".into(), json!(429), json!(77))
    );
    f.on_get(Ok(HttpResponse {
        status: 503,
        retry_after: Some("Wed, 21 Oct".into()),
        body: vec![],
    }));
    assert_eq!(
        net::fetch(&f, &sb.profile)["retry_after"],
        Value::Null,
        "Retry-After only counts as digits"
    );
    f.on_get(Err("TimeoutError".into()));
    let r = net::fetch(&f, &sb.profile);
    assert_eq!(
        (s(&r, "err"), r["status"].clone()),
        ("TimeoutError".into(), json!(0))
    );
    f.on_get(Ok(HttpResponse {
        status: 200,
        retry_after: None,
        body: b"<html>".to_vec(),
    }));
    assert_eq!(s(&net::fetch(&f, &sb.profile), "err"), "JSONDecodeError");

    let gone = Sandbox {
        profile: Profile {
            name: "x".into(),
            path: "Z:\\does\\not\\exist".into(),
        },
        ..sandbox(json!({}))
    };
    assert_eq!(
        s(&net::fetch(&f, &gone.profile), "err"),
        "không đọc được credentials"
    );
    sb.write(&creds(now_ms() - 1000, json!({})));
    let r = net::fetch(&f, &sb.profile);
    assert_eq!(
        (s(&r, "err"), r["status"].clone()),
        (
            "token hết hạn - chạy claude ở profile này 1 lần".into(),
            json!(401)
        )
    );
    assert!(f.get_q.borrow().is_empty());
}

// ------------------------------------------------------------------------------------------ refresh

#[test]
fn still_valid_or_nothing_to_refresh_with_does_not_post() {
    let f = Fake::new();
    let (mut o, _) = oauth();
    let sb = sandbox(creds(now_ms() + 3_600_000, json!({})));
    assert!(o.refresh_if_expired(&f, &sb.profile).is_none());
    sb.write(&creds(now_ms() - 1000, json!({"refreshToken": ""})));
    assert!(o.refresh_if_expired(&f, &sb.profile).is_none());
    sb.write(&creds(now_ms() - 1000, json!({"refreshToken": null})));
    assert!(o.refresh_if_expired(&f, &sb.profile).is_none());
    fs::write(sb.cred(), "{not json").unwrap();
    assert!(o.refresh_if_expired(&f, &sb.profile).is_none());
    assert_eq!(f.posts(), 0);
}

#[test]
fn expired_token_is_refreshed_and_written_like_the_cli() {
    let f = Fake::new();
    let (mut o, logs) = oauth();
    let sb = sandbox(expired());
    let before = fs::read_to_string(sb.cred()).unwrap();
    let rte_old = sb.read()["claudeAiOauth"]["refreshTokenExpiresAt"].clone();
    f.on_post(token_ok());
    let t0 = now_ms();
    assert!(o.refresh_if_expired(&f, &sb.profile).is_none());

    let (url, headers, body) = f.calls.borrow().posts[0].clone();
    assert_eq!(url, "https://platform.claude.com/v1/oauth/token");
    assert_eq!(
        body,
        json!({"grant_type": "refresh_token", "refresh_token": OLD_RT,
        "client_id": "9d1c250a-e61b-44d9-88ed-5944d1962f5e", "scope": "user:profile user:inference"})
    );
    assert!(headers
        .iter()
        .any(|(k, v)| k == "User-Agent" && v.starts_with("claude-usage/")));
    assert!(headers.contains(&("Content-Type".into(), "application/json".into())));

    let after = sb.read();
    let a = &after["claudeAiOauth"];
    assert_eq!(
        (a["accessToken"].clone(), a["refreshToken"].clone()),
        (json!(NEW_AT), json!(NEW_RT))
    );
    let exp = a["expiresAt"].as_i64().unwrap();
    assert!((t0 + 28_800_000..t0 + 28_800_000 + 5000).contains(&exp));
    assert_eq!(
        a["scopes"],
        json!(["user:profile", "user:inference", "user:file_upload"])
    );
    assert_eq!(
        a["refreshTokenExpiresAt"], rte_old,
        "never extended: min(old, new)"
    );
    assert_eq!(
        a["futureField"],
        json!({"keep": [1, 2]}),
        "unknown keys survive"
    );
    assert_eq!(a["subscriptionType"], json!("team"));
    assert_eq!(after["otherTopLevel"], json!("ünï"));
    let keys: Vec<&String> = a.as_object().unwrap().keys().collect();
    assert_eq!(
        keys[..3],
        ["accessToken", "refreshToken", "expiresAt"],
        "key order preserved"
    );
    let text = fs::read_to_string(sb.cred()).unwrap();
    assert!(
        !text.contains(": ") && !text.contains(", ") && text.contains("ünï"),
        "compact, UTF-8"
    );
    // backup = exact previous file
    let bak = fs::read_to_string(sb.dir.join(".credentials.json.claude-usage.bak")).unwrap();
    assert_eq!(bak, before);
    assert_eq!(logs.lock().unwrap().as_slice(), ["token refreshed: a"]);
    // locks released, no temp files left
    assert!(!sb.dir.join(".oauth_refresh.lock").exists());
    assert!(!net::lock_paths(&sb.dir)[1].exists());
    assert!(fs::read_dir(&sb.dir).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".tmp")));
}

#[test]
fn refresh_token_expiry_shrinks_when_server_says_sooner() {
    let f = Fake::new();
    let (mut o, _) = oauth();
    let sb = sandbox(creds(
        now_ms() - 1000,
        json!({"refreshTokenExpiresAt": now_ms() + 900_000_000_000_i64}),
    ));
    f.on_post(resp(
        200,
        json!({"access_token": NEW_AT, "expires_in": 100, "refresh_token_expires_in": 1000}),
    ));
    assert!(o.refresh_if_expired(&f, &sb.profile).is_none());
    let a = sb.read()["claudeAiOauth"].clone();
    let rte = a["refreshTokenExpiresAt"].as_i64().unwrap();
    assert!(rte < now_ms() + 1_100_000, "took the sooner server value");
    assert_eq!(
        a["refreshToken"],
        json!(OLD_RT),
        "no new refresh token in the response -> keep the old one"
    );
    assert_eq!(
        a["scopes"],
        json!(["user:profile", "user:inference"]),
        "no scope in response -> keep scopes"
    );
}

#[test]
fn rotated_refresh_token_is_used_next_time() {
    let f = Fake::new();
    let (mut o, _) = oauth();
    let sb = sandbox(expired());
    f.on_post(token_ok());
    assert!(o.refresh_if_expired(&f, &sb.profile).is_none());
    // expire again -> next POST must carry the NEW refresh token
    let mut c = sb.read();
    c["claudeAiOauth"]["expiresAt"] = json!(now_ms() - 1000);
    sb.write(&c);
    f.on_post(token_ok());
    assert!(o.refresh_if_expired(&f, &sb.profile).is_none());
    assert_eq!(f.calls.borrow().posts[1].2["refresh_token"], json!(NEW_RT));
}

#[test]
fn invalid_grant_is_dead_and_not_resent_until_file_changes() {
    let f = Fake::new();
    let (mut o, _) = oauth();
    let sb = sandbox(expired());
    f.on_post(resp(
        400,
        json!({"error": "invalid_grant", "error_description": "secret-detail"}),
    ));
    let r = o.refresh_if_expired(&f, &sb.profile).unwrap();
    assert_eq!(
        (s(&r, "err"), r["status"].clone(), r["dead"].clone()),
        (DEAD_MSG.into(), json!(401), json!(true))
    );
    for _ in 0..3 {
        let r = o.refresh_if_expired(&f, &sb.profile).unwrap();
        assert_eq!(r["dead"], json!(true));
    }
    assert_eq!(f.posts(), 1, "not sent again");
    // user logs in again: a different refresh token -> tried again
    sb.write(&creds(
        now_ms() - 1000,
        json!({"refreshToken": "relogin-refresh-not-real"}),
    ));
    f.on_post(token_ok());
    assert!(o.refresh_if_expired(&f, &sb.profile).is_none());
    assert_eq!(f.posts(), 2);
}

#[test]
fn refresh_token_already_past_its_own_expiry_is_dead_without_post() {
    let f = Fake::new();
    let (mut o, _) = oauth();
    let sb = sandbox(creds(
        now_ms() - 1000,
        json!({"refreshTokenExpiresAt": now_ms() - 5000}),
    ));
    let r = o.refresh_if_expired(&f, &sb.profile).unwrap();
    assert_eq!(
        (s(&r, "err"), r["status"].clone()),
        (DEAD_MSG.into(), json!(401))
    );
    assert_eq!(f.posts(), 0);
}

#[test]
fn invalid_scope_retries_once_without_scope() {
    let f = Fake::new();
    let (mut o, _) = oauth();
    let sb = sandbox(expired());
    f.on_post(resp(400, json!({"error": {"type": "invalid_scope"}})));
    f.on_post(token_ok());
    assert!(o.refresh_if_expired(&f, &sb.profile).is_none());
    let c = f.calls.borrow();
    assert!(c.posts[0].2.get("scope").is_some());
    assert!(c.posts[1].2.get("scope").is_none());
    // twice invalid_scope: gives up (second answer is a plain HTTP error with the code)
    drop(c);
    let sb2 = sandbox(expired());
    f.on_post(resp(400, json!({"error": "invalid_scope"})));
    f.on_post(resp(400, json!({"error": "invalid_scope"})));
    let r = o.refresh_if_expired(&f, &sb2.profile).unwrap();
    assert_eq!(
        (s(&r, "err"), r["status"].clone()),
        ("làm mới token: HTTP 400 invalid_scope".into(), json!(400))
    );
}

#[test]
fn http_errors_of_the_token_endpoint() {
    let f = Fake::new();
    let (mut o, _) = oauth();
    let sb = sandbox(expired());
    f.on_post(Ok(HttpResponse {
        status: 429,
        retry_after: Some("45".into()),
        body: b"{}".to_vec(),
    }));
    let r = o.refresh_if_expired(&f, &sb.profile).unwrap();
    assert_eq!(
        (s(&r, "err"), r["status"].clone(), r["retry_after"].clone()),
        ("làm mới token: HTTP 429".into(), json!(429), json!(45))
    );
    f.on_post(Err("TimeoutError".into()));
    let r = o.refresh_if_expired(&f, &sb.profile).unwrap();
    assert_eq!(
        (s(&r, "err"), r["status"].clone()),
        ("làm mới token: TimeoutError".into(), json!(0))
    );
    f.on_post(resp(200, json!({"access_token": "", "expires_in": 5})));
    assert_eq!(
        s(&o.refresh_if_expired(&f, &sb.profile).unwrap(), "err"),
        "làm mới token: phản hồi không hợp lệ"
    );
    f.on_post(resp(
        200,
        json!({"access_token": "x", "expires_in": "soon"}),
    ));
    assert_eq!(
        s(&o.refresh_if_expired(&f, &sb.profile).unwrap(), "err"),
        "làm mới token: phản hồi không hợp lệ"
    );
    // nothing was written on any failure
    assert_eq!(sb.read()["claudeAiOauth"]["accessToken"], json!(OLD_AT));
}

#[test]
fn secrets_never_reach_errors_or_logs() {
    let f = Fake::new();
    let (mut o, logs) = oauth();
    let sb = sandbox(expired());
    let secret = "top-secret-body-value";
    f.on_post(resp(500, json!({"error": "Bad Secret! top-secret-body-value", "access_token": secret, "refresh_token": secret})));
    let r = o.refresh_if_expired(&f, &sb.profile).unwrap();
    let all = format!("{:?}{:?}", r, logs.lock().unwrap());
    assert!(
        !all.contains(secret) && !all.contains(OLD_RT) && !all.contains(OLD_AT),
        "{all}"
    );
    assert_eq!(
        s(&r, "err"),
        "làm mới token: HTTP 500",
        "code not matching [a-z_]{{1,40}} is dropped"
    );
}

// ------------------------------------------------------------------------------------------ locks

fn old_time(dir: &Path, secs_ago: u64) {
    use std::os::windows::fs::OpenOptionsExt;
    let h = fs::OpenOptions::new()
        .write(true)
        .custom_flags(0x0200_0000)
        .open(dir)
        .unwrap();
    h.set_modified(SystemTime::now() - Duration::from_secs(secs_ago))
        .unwrap();
}

#[test]
fn lock_held_by_someone_else_returns_retry_30_and_touches_nothing() {
    let f = Fake::new();
    let (mut o, _) = oauth();
    let sb = sandbox(expired());
    let lock = sb.dir.join(".oauth_refresh.lock");
    fs::create_dir(&lock).unwrap();
    let r = o.refresh_if_expired(&f, &sb.profile).unwrap();
    assert_eq!(
        (s(&r, "err"), r["status"].clone(), r["retry_after"].clone()),
        (
            "claude đang làm mới token, thử lại sau".into(),
            json!(0),
            json!(30)
        )
    );
    assert!(lock.exists(), "someone else's lock stays");
    assert!(
        !net::lock_paths(&sb.dir)[1].exists(),
        "partial acquisition rolled back"
    );
    assert_eq!(f.posts(), 0);
    assert_eq!(sb.read()["claudeAiOauth"]["accessToken"], json!(OLD_AT));
    assert!(!sb.dir.join(".credentials.json.claude-usage.bak").exists());
}

#[test]
fn stale_lock_is_taken_over() {
    let f = Fake::new();
    let (mut o, _) = oauth();
    let sb = sandbox(expired());
    let lock = sb.dir.join(".oauth_refresh.lock");
    fs::create_dir(&lock).unwrap();
    old_time(&lock, 200);
    f.on_post(token_ok());
    assert!(o.refresh_if_expired(&f, &sb.profile).is_none());
    assert_eq!(f.posts(), 1);
    assert!(!lock.exists(), "released after use");
}

#[test]
fn fresh_lock_is_not_stale() {
    let a = tempfile::tempdir().unwrap();
    let p = a.path().join("l.lock");
    fs::create_dir(&p).unwrap();
    old_time(&p, 60);
    assert!(net::acquire(std::slice::from_ref(&p), 90.0, Duration::from_secs(5)).is_none());
    old_time(&p, 100);
    let g = net::acquire(std::slice::from_ref(&p), 90.0, Duration::from_secs(5))
        .expect("stale -> taken over");
    assert!(p.exists());
    drop(g);
    assert!(!p.exists());
}

#[test]
fn lock_is_kept_alive_while_held() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("k.lock");
    let g = net::acquire(std::slice::from_ref(&p), 90.0, Duration::from_millis(100)).unwrap();
    old_time(&p, 80);
    std::thread::sleep(Duration::from_millis(400));
    let age = SystemTime::now()
        .duration_since(fs::metadata(&p).unwrap().modified().unwrap())
        .unwrap();
    assert!(
        age < Duration::from_secs(5),
        "keeper touched the lock, age {age:?}"
    );
    drop(g);
}

#[test]
fn cli_holding_the_lock_then_finishing_means_no_post() {
    // NOTE: the re-read-under-the-lock branch itself (token rotated between our first read and the lock) cannot be
    // hit without a hook between the two; it is covered by review only. This checks the CLI-holds-lock flow.
    let f = Fake::new();
    let (mut o, _) = oauth();
    let sb = sandbox(expired());
    // Simulate by making the lock path creation a hook: hold the legacy lock, flip the file, release it from a thread.
    let legacy = net::lock_paths(&sb.dir)[1].clone();
    fs::create_dir(&legacy).unwrap();
    let cred = sb.cred();
    let t = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        let mut c: Value = serde_json::from_str(&fs::read_to_string(&cred).unwrap()).unwrap();
        c["claudeAiOauth"]["refreshToken"] = json!("cli-rotated");
        c["claudeAiOauth"]["expiresAt"] = json!(now_ms() + 3_600_000);
        fs::write(&cred, c.to_string()).unwrap();
        let _ = fs::remove_dir(legacy);
    });
    // First call sees the lock held -> retry 30 (no POST). After the CLI finished, the next call sees a valid token.
    let r = o.refresh_if_expired(&f, &sb.profile).unwrap();
    assert_eq!(r["retry_after"], json!(30));
    t.join().unwrap();
    assert!(o.refresh_if_expired(&f, &sb.profile).is_none());
    assert_eq!(f.posts(), 0);
}

#[test]
fn writes_even_if_a_reader_has_the_file_open() {
    let f = Fake::new();
    let (mut o, _) = oauth();
    let sb = sandbox(expired());
    // A reader holding the file open WITHOUT delete/rename sharing (like many Windows tools).
    let reader = fs::File::open(sb.cred()).unwrap();
    f.on_post(token_ok());
    let r = o.refresh_if_expired(&f, &sb.profile);
    drop(reader);
    assert!(r.is_none(), "{r:?}");
    assert_eq!(sb.read()["claudeAiOauth"]["accessToken"], json!(NEW_AT));
}

#[test]
fn real_path_strips_verbatim_prefix_and_survives_missing_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let r = net::real_path(tmp.path());
    assert!(!r.to_string_lossy().starts_with(r"\\?\"), "{r:?}");
    let missing = Path::new("Z:\\no\\such\\dir");
    assert_eq!(net::real_path(missing), missing);
    assert!(net::lock_paths(tmp.path())[1]
        .to_string_lossy()
        .ends_with(".lock"));
}
