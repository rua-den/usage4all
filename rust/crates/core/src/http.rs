//! Minimal blocking HTTP client for the two Anthropic endpoints (usage + OAuth token).
//!
//! Stack (PORT_PLAN R5): `ureq` 3 (sync) with rustls and `rustls-platform-verifier`, so certificates are
//! validated by the Windows certificate store (corporate / TLS-inspecting roots work like in Python's
//! `urllib`, which used the same store through `ssl`).
//!
//! Proxy (what Python's `urllib` did, minus PAC): environment first (`HTTPS_PROXY` / `https_proxy`,
//! `HTTP_PROXY` for http URLs, `NO_PROXY`), then the WinINet registry values of the current user
//! (`ProxyEnable`, `ProxyServer` in either `host:port` or `http=h:p;https=h:p` form, `ProxyOverride`).
//! Not supported: PAC / WPAD auto-config scripts, SOCKS, proxy authentication prompts (credentials in the proxy
//! URL are passed through). Loopback hosts are never proxied.
//!
//! Redirects are NOT followed (Python's urllib followed them, re-sending the Authorization header only to the
//! same host): neither endpoint redirects, and refusing is the safer behaviour for a bearer token. A 3xx is
//! reported like any other non-2xx status.

use std::time::Duration;

/// A completed HTTP exchange (any status).
#[derive(Clone)]
pub struct Response {
    pub status: u16,
    /// Raw `Retry-After` header value, if present.
    pub retry_after: Option<String>,
    pub body: Vec<u8>,
}

/// m9: Debug never prints the body (it may carry tokens), only its length.
impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("retry_after", &self.retry_after)
            .field("body", &format_args!("<{} bytes>", self.body.len()))
            .finish()
    }
}

impl Response {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Which proxy settings to use.
#[derive(Clone)]
pub enum ProxyMode {
    /// Never use a proxy (tests, loopback fake servers).
    Direct,
    /// Environment, then the WinINet registry (release default).
    System,
    /// Fixed proxy URL (tests of the plumbing).
    Fixed(String),
}

/// m9: Debug hides credentials embedded in a fixed proxy URL.
impl std::fmt::Debug for ProxyMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProxyMode::Direct => f.write_str("Direct"),
            ProxyMode::System => f.write_str("System"),
            ProxyMode::Fixed(u) => {
                let (scheme, rest) = u.split_once("://").unwrap_or(("", u));
                let host = rest.rsplit('@').next().unwrap_or(rest);
                write!(f, "Fixed({scheme}://{host})")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Http {
    proxy: ProxyMode,
}

/// Body cap for responses (the real ones are < 10 KB).
const MAX_BODY: u64 = 4 * 1024 * 1024;

impl Http {
    pub fn new(proxy: ProxyMode) -> Http {
        Http { proxy }
    }

    /// Release default: system proxy settings.
    pub fn system() -> Http {
        Http::new(ProxyMode::System)
    }

    /// No proxy at all.
    pub fn direct() -> Http {
        Http::new(ProxyMode::Direct)
    }

    /// One request. `Err(name)` = transport-level failure (no HTTP status), `name` is a stable error class
    /// name chosen to match what Python reported: `TimeoutError`, `URLError` (DNS, connect, TLS, proxy,
    /// protocol), `ValueError` (invalid header value), `OSError` (other I/O).
    pub fn request(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<Response, &'static str> {
        for (k, v) in headers {
            if k.is_empty() || v.bytes().any(|b| b < 0x20 && b != b'\t' || b == 0x7f) {
                return Err("ValueError");
            }
        }
        let proxy = match &self.proxy {
            ProxyMode::Direct => None,
            ProxyMode::Fixed(p) => Some(p.clone()),
            ProxyMode::System => system_proxy_for(url),
        };
        let proxy = match proxy {
            Some(p) => Some(ureq::Proxy::new(&p).map_err(|_| "URLError")?),
            None => None,
        };
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::Rustls)
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(timeout))
            .proxy(proxy)
            .tls_config(tls)
            .build();
        let agent: ureq::Agent = config.into();

        let result = match method {
            "GET" => {
                let mut r = agent.get(url);
                for (k, v) in headers {
                    r = r.header(*k, *v);
                }
                r.call()
            }
            "POST" => {
                let mut r = agent.post(url);
                for (k, v) in headers {
                    r = r.header(*k, *v);
                }
                r.send(body.unwrap_or(&[]))
            }
            _ => return Err("ValueError"),
        };
        let mut resp = result.map_err(classify)?;
        let status = resp.status().as_u16();
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_string());
        let body = resp
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_vec()
            .map_err(classify)?;
        Ok(Response {
            status,
            retry_after,
            body,
        })
    }
}

fn classify(e: ureq::Error) -> &'static str {
    use std::io::ErrorKind;
    match e {
        ureq::Error::Timeout(_) => "TimeoutError",
        ureq::Error::Io(io) => match io.kind() {
            ErrorKind::TimedOut | ErrorKind::WouldBlock => "TimeoutError",
            ErrorKind::ConnectionRefused
            | ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::NotConnected
            | ErrorKind::UnexpectedEof => "URLError",
            _ => "OSError",
        },
        ureq::Error::BadUri(_) | ureq::Error::InvalidProxyUrl => "ValueError",
        _ => "URLError",
    }
}

// ---------------------------------------------------------------------------------------- proxy selection

/// WinINet proxy values (HKCU Internet Settings).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistryProxy {
    pub enabled: bool,
    pub server: String,
    pub bypass: String,
}

fn url_scheme_host(url: &str) -> (String, String) {
    let (scheme, rest) = url.split_once("://").unwrap_or(("http", url));
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    let auth = auth.rsplit('@').next().unwrap_or(auth);
    let host = if let Some(r) = auth.strip_prefix('[') {
        r.split(']').next().unwrap_or("")
    } else {
        auth.split(':').next().unwrap_or("")
    };
    (scheme.to_ascii_lowercase(), host.to_ascii_lowercase())
}

fn is_loopback(host: &str) -> bool {
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

/// Case-insensitive glob where `*` matches any run of characters.
fn glob_match(pat: &str, text: &str) -> bool {
    let p: Vec<char> = pat.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0usize, 0usize, None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn normalize_proxy_url(p: &str) -> String {
    let p = p.trim();
    if p.contains("://") {
        p.to_string()
    } else {
        format!("http://{p}")
    }
}

/// Proxy URL for `url` given an environment lookup and the registry values. Pure (testable).
pub fn resolve_proxy(
    url: &str,
    env: &dyn Fn(&str) -> Option<String>,
    reg: Option<&RegistryProxy>,
) -> Option<String> {
    let (scheme, host) = url_scheme_host(url);
    if host.is_empty() || is_loopback(&host) {
        return None;
    }
    let env_get = |name: &str| {
        env(&name.to_ascii_uppercase())
            .or_else(|| env(&name.to_ascii_lowercase()))
            .filter(|v| !v.trim().is_empty())
    };
    let env_proxy = env_get(&format!("{scheme}_proxy"));
    if let Some(p) = env_proxy {
        if let Some(np) = env_get("no_proxy") {
            for pat in np.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                let pat = pat.trim_start_matches('.');
                if pat == "*"
                    || host == pat
                    || host.ends_with(&format!(".{pat}"))
                    || glob_match(pat, &host)
                {
                    return None;
                }
            }
        }
        return Some(normalize_proxy_url(&p));
    }
    let reg = reg.filter(|r| r.enabled && !r.server.trim().is_empty())?;
    let server = reg.server.trim();
    let chosen = if server.contains('=') {
        server.split(';').find_map(|part| {
            let (k, v) = part.split_once('=')?;
            (k.trim().eq_ignore_ascii_case(&scheme)).then(|| v.trim().to_string())
        })?
    } else {
        server.to_string()
    };
    if chosen.is_empty() {
        return None;
    }
    for pat in reg
        .bypass
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if pat.eq_ignore_ascii_case("<local>") {
            if !host.contains('.') {
                return None;
            }
        } else if glob_match(pat, &host) {
            return None;
        }
    }
    Some(normalize_proxy_url(&chosen))
}

fn system_proxy_for(url: &str) -> Option<String> {
    let env = |k: &str| std::env::var(k).ok();
    let reg = read_registry_proxy();
    resolve_proxy(url, &env, reg.as_ref())
}

#[cfg(windows)]
fn read_registry_proxy() -> Option<RegistryProxy> {
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    };
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings");
    let dword = |name: &str| -> Option<u32> {
        let n = wide(name);
        let mut v: u32 = 0;
        let mut sz: u32 = 4;
        // SAFETY: valid NUL-terminated wide strings, output buffer of `sz` bytes.
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                n.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut v as *mut u32).cast(),
                &mut sz,
            )
        };
        (rc == 0).then_some(v)
    };
    let string = |name: &str| -> Option<String> {
        let n = wide(name);
        let mut sz: u32 = 0;
        // SAFETY: size query then read into a buffer of that size.
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                n.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut sz,
            )
        };
        if rc != 0 || sz < 2 {
            return None;
        }
        let mut buf = vec![0u16; (sz as usize).div_ceil(2)];
        let mut sz2 = (buf.len() * 2) as u32;
        // SAFETY: buffer has sz2 bytes.
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                n.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut sz2,
            )
        };
        if rc != 0 {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    };
    Some(RegistryProxy {
        enabled: dword("ProxyEnable").unwrap_or(0) != 0,
        server: string("ProxyServer").unwrap_or_default(),
        bypass: string("ProxyOverride").unwrap_or_default(),
    })
}

#[cfg(not(windows))]
fn read_registry_proxy() -> Option<RegistryProxy> {
    None
}
