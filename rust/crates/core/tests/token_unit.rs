//! Unit tests for the lock keep-alive (R8), realpath normalisation, lock ordering, HTTP plumbing (timeouts,
//! error names, proxy selection / use, TLS stack start-up). Loopback only; temp directories only.

use cu_core::http::{resolve_proxy, Http, ProxyMode, RegistryProxy};
use cu_core::token::{
    acquire, lock_paths, real_path, strip_verbatim, LOCK_STALE, POST_BUDGET, POST_TIMEOUT,
};
use std::fs;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, SystemTime};

fn set_dir_age(p: &Path, age_secs: u64) {
    let f = fs::OpenOptions::new()
        .access_mode(0x100)
        .custom_flags(0x0200_0000)
        .open(p)
        .unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(age_secs))
        .unwrap();
}

fn age_of(p: &Path) -> Duration {
    fs::metadata(p)
        .unwrap()
        .modified()
        .unwrap()
        .elapsed()
        .unwrap_or_default()
}

#[test]
fn constants_respect_the_budget() {
    assert!(POST_BUDGET < Duration::from_secs(50));
    assert!(POST_TIMEOUT <= POST_BUDGET);
    assert_eq!(LOCK_STALE, 90);
}

#[test]
fn keepalive_advances_mtime_while_held_and_stops_after_drop() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a.lock"), tmp.path().join("b.lock"));
    let g = acquire(
        &[a.clone(), b.clone()],
        LOCK_STALE,
        Duration::from_millis(60),
    )
    .expect("lock");
    set_dir_age(&a, 100);
    set_dir_age(&b, 100);
    assert!(age_of(&a) > Duration::from_secs(90));
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        age_of(&a) < Duration::from_secs(5),
        "lock a was not touched"
    );
    assert!(
        age_of(&b) < Duration::from_secs(5),
        "lock b was not touched"
    );
    drop(g);
    assert!(!a.exists() && !b.exists(), "locks released on drop");
    // After the drop nothing may touch the paths any more (thread joined before the rmdir).
    fs::create_dir(&a).unwrap();
    set_dir_age(&a, 100);
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        age_of(&a) > Duration::from_secs(90),
        "keep-alive thread outlived the guard"
    );
}

#[test]
fn dropping_the_guard_right_away_does_not_wait_for_the_next_touch() {
    // Regression: a stop signalled before the keep-alive thread's first wait must not be lost (it used to
    // stall every early-error refresh by a full touch period).
    let tmp = tempfile::tempdir().unwrap();
    for i in 0..50 {
        let l = tmp.path().join(format!("{i}.lock"));
        let t = std::time::Instant::now();
        drop(acquire(std::slice::from_ref(&l), LOCK_STALE, Duration::from_secs(5)).unwrap());
        assert!(
            t.elapsed() < Duration::from_secs(2),
            "drop took {:?}",
            t.elapsed()
        );
        assert!(!l.exists());
    }
}

#[test]
fn held_lock_with_keepalive_is_never_stale_for_a_competitor() {
    let tmp = tempfile::tempdir().unwrap();
    let l = tmp.path().join("x.lock");
    let g = acquire(
        std::slice::from_ref(&l),
        LOCK_STALE,
        Duration::from_millis(100),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(1600));
    // A competitor with a 1 s stale threshold (the real one is 60 s for the CLI) must not take it over.
    assert!(acquire(std::slice::from_ref(&l), 1, Duration::from_secs(5)).is_none());
    assert!(l.exists());
    drop(g);
    // Without keep-alive (tiny threshold, old mtime) a takeover works.
    fs::create_dir(&l).unwrap();
    set_dir_age(&l, 10);
    assert!(acquire(std::slice::from_ref(&l), 1, Duration::from_secs(5)).is_some());
}

#[test]
fn acquire_releases_the_first_lock_when_the_second_is_busy_and_guard_releases_on_panic() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a.lock"), tmp.path().join("b.lock"));
    fs::create_dir(&b).unwrap();
    assert!(acquire(&[a.clone(), b.clone()], LOCK_STALE, Duration::from_secs(5)).is_none());
    assert!(
        !a.exists() && b.exists(),
        "first released, foreign lock untouched"
    );
    fs::remove_dir(&b).unwrap();
    let r = std::panic::catch_unwind(|| {
        let _g = acquire(&[a.clone(), b.clone()], LOCK_STALE, Duration::from_secs(5)).unwrap();
        panic!("boom");
    });
    assert!(r.is_err());
    assert!(!a.exists() && !b.exists(), "released during unwind");
}

#[test]
fn stale_lock_is_taken_over_and_stale_boundary() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a.lock");
    fs::create_dir(&a).unwrap();
    set_dir_age(&a, 80);
    assert!(acquire(std::slice::from_ref(&a), LOCK_STALE, Duration::from_secs(5)).is_none());
    set_dir_age(&a, 100);
    let g = acquire(std::slice::from_ref(&a), LOCK_STALE, Duration::from_secs(5));
    assert!(g.is_some());
    drop(g);
    // non-empty foreign lock dir cannot be taken over (rmdir fails) -> busy
    fs::create_dir(&a).unwrap();
    fs::write(a.join("f"), "x").unwrap();
    set_dir_age(&a, 100);
    assert!(acquire(std::slice::from_ref(&a), LOCK_STALE, Duration::from_secs(5)).is_none());
}

#[test]
fn strip_verbatim_prefixes() {
    assert_eq!(strip_verbatim(r"\\?\C:\Users\x"), r"C:\Users\x");
    assert_eq!(strip_verbatim(r"\\?\UNC\srv\share\p"), r"\\srv\share\p");
    assert_eq!(strip_verbatim(r"C:\plain"), r"C:\plain");
    assert_eq!(strip_verbatim(r"\\srv\share"), r"\\srv\share");
    assert_eq!(strip_verbatim(r"\\?\Volume{1234}\x"), r"\\?\Volume{1234}\x");
}

#[test]
fn real_path_normalises_and_falls_back() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("prof");
    fs::create_dir(&d).unwrap();
    let r = real_path(&d);
    let s = r.to_string_lossy().into_owned();
    assert!(!s.starts_with(r"\\?\"), "verbatim prefix leaked: {s}");
    assert!(r.exists());
    // legacy lock name = <realpath>.lock
    let lp = lock_paths(&d);
    assert_eq!(lp[0], d.join(".oauth_refresh.lock"));
    assert_eq!(lp[1].to_string_lossy(), format!("{s}.lock"));
    // missing path: original path, no panic
    let missing = tmp.path().join("nope").join("x");
    assert_eq!(real_path(&missing), missing);
}

#[test]
fn real_path_resolves_a_junction() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("target");
    let link = tmp.path().join("link");
    fs::create_dir(&target).unwrap();
    let ok = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&target)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("mklink /J not permitted here; junction test skipped");
        return;
    }
    let via_link = real_path(&link);
    let direct = real_path(&target);
    assert_eq!(via_link, direct, "junction must resolve to its target");
    let lp = lock_paths(&link);
    assert_eq!(lp[0], link.join(".oauth_refresh.lock"));
    assert_eq!(
        lp[1].to_string_lossy(),
        format!("{}.lock", direct.to_string_lossy())
    );
}

// ------------------------------------------------------------------------------------------------- HTTP

#[test]
fn http_error_names_timeout_refused_and_tls() {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    // accept and never answer -> timeout
    let t = std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        std::thread::sleep(Duration::from_millis(2500));
        drop(s);
        // second connection: TLS handshake against a plain-text peer
        let (s2, _) = l.accept().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        drop(s2);
    });
    let h = Http::direct();
    let e = h
        .request(
            "GET",
            &format!("http://127.0.0.1:{port}/"),
            &[],
            None,
            Duration::from_millis(800),
        )
        .unwrap_err();
    assert_eq!(e, "TimeoutError");
    // https to a non-TLS peer: exercises rustls + platform verifier start-up, must fail cleanly
    let e = h
        .request(
            "GET",
            &format!("https://127.0.0.1:{port}/"),
            &[],
            None,
            Duration::from_secs(5),
        )
        .unwrap_err();
    assert!(
        e == "URLError" || e == "OSError" || e == "TimeoutError",
        "{e}"
    );
    t.join().unwrap();
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let e = h
        .request(
            "GET",
            &format!("http://127.0.0.1:{dead}/"),
            &[],
            None,
            Duration::from_secs(5),
        )
        .unwrap_err();
    assert_eq!(e, "URLError");
    assert_eq!(
        h.request(
            "GET",
            "http://127.0.0.1:1/",
            &[("X", "a\nb")],
            None,
            Duration::from_secs(1)
        )
        .unwrap_err(),
        "ValueError"
    );
}

#[test]
fn fixed_proxy_is_actually_used_for_http() {
    // ureq tunnels through an HTTP proxy with CONNECT (this is also what https needs); play a tiny proxy.
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let t = std::thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        // TCP may deliver the request head in pieces: read up to the blank line
        let head = |s: &mut std::net::TcpStream| {
            let mut acc = Vec::new();
            let mut b = [0u8; 1024];
            while !acc.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = s.read(&mut b).unwrap();
                assert!(n > 0, "peer closed early");
                acc.extend_from_slice(&b[..n]);
            }
            String::from_utf8_lossy(&acc)
                .lines()
                .next()
                .unwrap()
                .to_string()
        };
        let connect = head(&mut s);
        s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .unwrap();
        let inner = head(&mut s);
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .unwrap();
        s.flush().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        (connect, inner)
    });
    let h = Http::new(ProxyMode::Fixed(format!("http://127.0.0.1:{port}")));
    let r = h
        .request(
            "GET",
            "http://example.invalid/api/oauth/usage",
            &[],
            None,
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(r.status, 200);
    let (connect, inner) = t.join().unwrap();
    assert!(
        connect.starts_with("CONNECT example.invalid:80 "),
        "{connect}"
    );
    assert!(inner.starts_with("GET /api/oauth/usage "), "{inner}");
}

#[test]
fn proxy_resolution() {
    let none = |_: &str| None::<String>;
    let reg = RegistryProxy {
        enabled: true,
        server: "proxy.corp:8080".into(),
        bypass: "*.corp.local;<local>;10.*".into(),
    };
    let u = "https://api.anthropic.com/api/oauth/usage";
    // nothing configured
    assert_eq!(resolve_proxy(u, &none, None), None);
    // registry, plain host:port form
    assert_eq!(
        resolve_proxy(u, &none, Some(&reg)).as_deref(),
        Some("http://proxy.corp:8080")
    );
    // registry disabled
    let off = RegistryProxy {
        enabled: false,
        ..reg.clone()
    };
    assert_eq!(resolve_proxy(u, &none, Some(&off)), None);
    // bypass list
    assert_eq!(
        resolve_proxy("https://a.corp.local/x", &none, Some(&reg)),
        None
    );
    assert_eq!(
        resolve_proxy("https://intranet/x", &none, Some(&reg)),
        None,
        "<local>"
    );
    assert_eq!(resolve_proxy("https://10.1.2.3/x", &none, Some(&reg)), None);
    // per-scheme form
    let per = RegistryProxy {
        enabled: true,
        server: "http=h1:1;https=h2:2;ftp=h3:3".into(),
        bypass: String::new(),
    };
    assert_eq!(
        resolve_proxy(u, &none, Some(&per)).as_deref(),
        Some("http://h2:2")
    );
    assert_eq!(
        resolve_proxy("http://x.example/", &none, Some(&per)).as_deref(),
        Some("http://h1:1")
    );
    let only_http = RegistryProxy {
        server: "http=h1:1".into(),
        ..per.clone()
    };
    assert_eq!(resolve_proxy(u, &none, Some(&only_http)), None);
    // environment wins over the registry; upper and lower case; NO_PROXY
    let env = |k: &str| match k {
        "HTTPS_PROXY" => Some("http://envproxy:3128".to_string()),
        "NO_PROXY" => Some("example.org, .internal".to_string()),
        _ => None,
    };
    assert_eq!(
        resolve_proxy(u, &env, Some(&reg)).as_deref(),
        Some("http://envproxy:3128")
    );
    assert_eq!(
        resolve_proxy("https://example.org/", &env, Some(&reg)),
        None
    );
    assert_eq!(resolve_proxy("https://a.internal/", &env, Some(&reg)), None);
    let envl = |k: &str| (k == "https_proxy").then(|| "envproxy:1".to_string());
    assert_eq!(
        resolve_proxy(u, &envl, None).as_deref(),
        Some("http://envproxy:1")
    );
    // loopback is never proxied
    assert_eq!(
        resolve_proxy("http://127.0.0.1:9/x", &env, Some(&reg)),
        None
    );
    assert_eq!(resolve_proxy("http://localhost/x", &env, Some(&reg)), None);
}
