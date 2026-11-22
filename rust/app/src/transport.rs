//! Real HTTP transport (ureq + rustls with the Windows certificate store). Implements `cu_core::net::Http`.
//!
//! Non-2xx statuses are returned as responses (the logic in cu-core maps them); only transport failures are
//! errors, reduced to a short class name so no URL/header/body can reach a log.
//! Proxy: ureq reads the standard `HTTPS_PROXY`/`ALL_PROXY` variables; when none is set the per-user Windows
//! proxy (`HKCU\...\Internet Settings`: ProxyEnable/ProxyServer, like Python's urllib) is used. ProxyOverride
//! and PAC scripts are not interpreted.

use cu_core::net::{Http, HttpResponse};
use std::time::Duration;

pub struct Ureq;

/// Test-only endpoint override: `CU_API_BASE` replaces `https://api.anthropic.com`, `CU_TOKEN_URL` replaces the
/// token endpoint (so e2e tests can talk to a fake server on 127.0.0.1). Compiled ONLY into debug builds and builds
/// with the `e2e` feature: the shipped release has neither the code nor the variable names (checked by
/// `tests/smoke_release_rust.py`).
#[cfg(any(debug_assertions, feature = "e2e"))]
fn rewrite(url: &str) -> String {
    use cu_core::net::TOKEN_URL;
    if let (Some(rest), Some(base)) = (
        url.strip_prefix("https://api.anthropic.com"),
        std::env::var_os("CU_API_BASE"),
    ) {
        return format!("{}{rest}", base.to_string_lossy().trim_end_matches('/'));
    }
    if url == TOKEN_URL {
        if let Some(t) = std::env::var_os("CU_TOKEN_URL") {
            return t.to_string_lossy().into_owned();
        }
    }
    url.to_string()
}

#[cfg(not(any(debug_assertions, feature = "e2e")))]
fn rewrite(url: &str) -> String {
    url.to_string()
}

#[cfg(any(debug_assertions, feature = "e2e"))]
fn override_active() -> bool {
    std::env::var_os("CU_API_BASE").is_some() || std::env::var_os("CU_TOKEN_URL").is_some()
}

#[cfg(not(any(debug_assertions, feature = "e2e")))]
fn override_active() -> bool {
    false
}

/// `ProxyServer` value -> proxy URL for https: `host:port` or `http=h:p;https=h:p;...`.
fn parse_proxy(v: &str) -> Option<String> {
    let v = v.trim();
    let pick = if v.contains('=') {
        let find = |k: &str| {
            v.split(';')
                .filter_map(|p| p.split_once('='))
                .find(|(n, _)| n.trim().eq_ignore_ascii_case(k))
                .map(|(_, h)| h.trim().to_string())
        };
        find("https").or_else(|| find("http"))?
    } else {
        v.to_string()
    };
    if pick.is_empty() {
        return None;
    }
    Some(if pick.contains("://") {
        pick
    } else {
        format!("http://{pick}")
    })
}

/// Host part of an http(s) URL (no port/userinfo/path).
fn url_host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |x| x.1);
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    let auth = auth.rsplit('@').next().unwrap_or("");
    auth.split(':').next().unwrap_or("")
}

/// `ProxyOverride` semantics (WinINET): `;`-separated patterns, `*` wildcards, `<local>` = hosts without a dot.
fn bypassed(host: &str, list: &str) -> bool {
    fn glob(p: &[u8], t: &[u8]) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some((b'*', rest)) => (0..=t.len()).any(|i| glob(rest, &t[i..])),
            Some((c, rest)) => t.first() == Some(c) && glob(rest, &t[1..]),
        }
    }
    let host = host.to_ascii_lowercase();
    list.split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .any(|p| {
            if p.eq_ignore_ascii_case("<local>") {
                !host.contains('.')
            } else {
                let p = p
                    .split("://")
                    .last()
                    .unwrap_or(p)
                    .split(':')
                    .next()
                    .unwrap_or(p);
                glob(p.to_ascii_lowercase().as_bytes(), host.as_bytes())
            }
        })
}

fn registry_proxy(host: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::*;
    let key = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings"
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<u16>>();
    let rd = |name: &str, flags: u32, buf: &mut [u8]| -> Option<usize> {
        let n: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let mut len = buf.len() as u32;
        // SAFETY: read-only registry query into a caller-owned buffer.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                n.as_ptr(),
                flags,
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut _,
                &mut len,
            )
        };
        (r == 0).then_some(len as usize)
    };
    let mut en = [0u8; 4];
    rd("ProxyEnable", RRF_RT_REG_DWORD, &mut en)?;
    if u32::from_le_bytes(en) == 0 {
        return None;
    }
    let mut buf = [0u8; 2048];
    let n = rd("ProxyServer", RRF_RT_REG_SZ, &mut buf)?;
    let w: Vec<u16> = buf[..n]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|&c| c != 0)
        .collect();
    let proxy = parse_proxy(&String::from_utf16_lossy(&w))?;
    let mut ov = [0u8; 4096];
    if let Some(n) = rd("ProxyOverride", RRF_RT_REG_SZ, &mut ov) {
        let w: Vec<u16> = ov[..n]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .take_while(|&c| c != 0)
            .collect();
        if bypassed(host, &String::from_utf16_lossy(&w)) {
            return None;
        }
    }
    Some(proxy)
}

fn env_proxy_set() -> bool {
    ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
        .iter()
        .any(|k| std::env::var_os(k).is_some())
}

fn agent(timeout: Duration, url: &str) -> ureq::Agent {
    let mut b = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false);
    if !env_proxy_set() && !override_active() {
        if let Some(p) = registry_proxy(url_host(url)).and_then(|u| ureq::Proxy::new(&u).ok()) {
            b = b.proxy(Some(p));
        }
    }
    b.build().into()
}

fn class(e: &ureq::Error) -> String {
    match e {
        ureq::Error::Timeout(_) => "TimeoutError",
        ureq::Error::Io(_) | ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => {
            "URLError"
        }
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) => "SSLError",
        _ => "URLError",
    }
    .to_string()
}

fn finish(
    r: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<HttpResponse, String> {
    let mut r = r.map_err(|e| class(&e))?;
    let status = r.status().as_u16();
    let retry_after = r
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = r.body_mut().read_to_vec().map_err(|e| class(&e))?;
    Ok(HttpResponse {
        status,
        retry_after,
        body,
    })
}

impl Http for Ureq {
    fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<HttpResponse, String> {
        let url = rewrite(url);
        let url = url.as_str();
        let mut req = agent(timeout, url).get(url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        finish(req.call())
    }

    fn post_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &str,
        timeout: Duration,
    ) -> Result<HttpResponse, String> {
        let url = rewrite(url);
        let url = url.as_str();
        let mut req = agent(timeout, url).post(url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        finish(req.send(body.as_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// One-shot local server: answers with `reply`, returns the raw request it received.
    fn serve(reply: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/x", l.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = vec![0u8; 8192];
            let mut got = Vec::new();
            loop {
                let n = s.read(&mut buf).unwrap();
                got.extend_from_slice(&buf[..n]);
                let t = String::from_utf8_lossy(&got).to_string();
                if let Some(i) = t.find("\r\n\r\n") {
                    let cl = t
                        .to_lowercase()
                        .split("content-length: ")
                        .nth(1)
                        .and_then(|r| r.split("\r\n").next()?.parse::<usize>().ok())
                        .unwrap_or(0);
                    if got.len() >= i + 4 + cl {
                        break;
                    }
                }
            }
            s.write_all(reply.as_bytes()).unwrap();
            String::from_utf8_lossy(&got).to_string()
        });
        (url, h)
    }

    #[test]
    fn registry_proxy_reads_without_panicking() {
        // read-only; result depends on the machine, only the call path (NUL-terminated key) is checked
        let _ = registry_proxy("api.anthropic.com");
        let _ = agent(Duration::from_secs(1), "https://api.anthropic.com/x");
    }

    #[test]
    fn override_list_and_host() {
        assert_eq!(
            url_host("https://user@api.anthropic.com:443/a?b"),
            "api.anthropic.com"
        );
        let l = "localhost;*.corp.local;10.*;<local>";
        assert!(bypassed("localhost", l) && bypassed("a.corp.local", l) && bypassed("10.1.2.3", l));
        assert!(bypassed("intranet", l)); // <local>: no dot
        assert!(!bypassed("api.anthropic.com", l));
        assert!(bypassed("API.Anthropic.com", "*.anthropic.com"));
        assert!(!bypassed("api.anthropic.com", ""));
    }

    #[test]
    fn proxy_value_parsing() {
        assert_eq!(
            parse_proxy("p.local:8080").as_deref(),
            Some("http://p.local:8080")
        );
        assert_eq!(
            parse_proxy("http=a:1;https=b:2;ftp=c:3").as_deref(),
            Some("http://b:2")
        );
        assert_eq!(parse_proxy("ftp=c:3"), None);
        assert_eq!(parse_proxy(""), None);
    }

    #[test]
    fn get_returns_non_2xx_as_response_with_retry_after() {
        let (url, h) = serve("HTTP/1.1 429 Too Many Requests\r\nRetry-After: 77\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
        let r = Ureq
            .get(
                &url,
                &[("Authorization", "Bearer fake"), ("anthropic-beta", "b")],
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(
            (r.status, r.retry_after.as_deref(), r.body.as_slice()),
            (429, Some("77"), &b"{}"[..])
        );
        let req = h.join().unwrap().to_lowercase();
        assert!(
            req.starts_with("get /x")
                && req.contains("authorization: bearer fake")
                && req.contains("anthropic-beta: b")
        );
    }

    #[test]
    fn post_sends_json_body_and_headers() {
        let (url, h) = serve(
            "HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}",
        );
        let r = Ureq
            .post_json(
                &url,
                &[
                    ("Content-Type", "application/json"),
                    ("User-Agent", "claude-usage/t"),
                ],
                "{\"a\":1}",
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"{\"ok\":true}"[..]));
        let req = h.join().unwrap();
        assert!(
            req.starts_with("POST /x")
                && req.ends_with("{\"a\":1}")
                && req.to_lowercase().contains("user-agent: claude-usage/t")
        );
    }

    #[test]
    fn connection_refused_is_a_short_class_name() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", l.local_addr().unwrap());
        drop(l);
        let e = Ureq.get(&url, &[], Duration::from_secs(5)).err().unwrap();
        assert!(e == "URLError" && !e.contains("127.0.0.1"), "{e}");
    }

    /// Manual: `cargo test -p claude-usage --bin claude-usage -- --ignored tls`. No credentials are sent;
    /// proves TLS + the Windows certificate store (e.g. behind a TLS-inspecting proxy). Expect 401.
    #[test]
    #[ignore]
    fn tls_to_real_endpoint_without_token() {
        let r = Ureq.get(
            cu_core::net::API,
            &[("anthropic-beta", "oauth-2025-04-20")],
            Duration::from_secs(20),
        );
        let r = r.expect("TLS/transport failed");
        assert_eq!(r.status, 401, "unexpected status");
    }
}
