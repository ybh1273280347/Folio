//! Minimal HTTP/HTTPS client backed by WinHTTP on Windows and `curl` elsewhere.

#[cfg(windows)]
use std::{
    collections::{HashMap, hash_map::Entry},
    ffi::c_void,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
#[cfg(windows)]
use windows::{
    Win32::Networking::WinHttp::{
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_ACCESS_TYPE_NAMED_PROXY,
        WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_DISABLE_COOKIES, WINHTTP_FLAG_SECURE,
        WINHTTP_OPEN_REQUEST_FLAGS, WINHTTP_OPTION_DISABLE_FEATURE,
        WINHTTP_OPTION_DISABLE_GLOBAL_POOLING, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
        WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
        WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption,
        WinHttpSetTimeouts,
    },
    core::{HSTRING, PCWSTR},
};

#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

pub struct RequestOptions<'a> {
    pub https: bool,
    pub host: &'a str,
    pub port: u16,
    pub method: &'a str,
    pub path: &'a str,
    pub headers: &'a [(&'a str, &'a str)],
    pub body: &'a [u8],
    pub connect_timeout_ms: i32,
    pub receive_timeout_ms: i32,
    pub use_system_proxy: bool,
    pub use_env_proxy: bool,
    pub max_response_bytes: usize,
    pub label: &'a str,
}

#[cfg(windows)]
pub fn env_proxy() -> Option<String> {
    for key in [
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ] {
        if let Ok(val) = std::env::var(key) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                let stripped = trimmed
                    .strip_prefix("http://")
                    .or_else(|| trimmed.strip_prefix("https://"))
                    .unwrap_or(trimmed);
                return Some(stripped.trim_end_matches('/').to_owned());
            }
        }
    }
    None
}

#[cfg(windows)]
struct Handle(*mut c_void);

#[cfg(windows)]
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
#[derive(Eq, Hash, PartialEq)]
enum ProxyPolicy {
    Direct,
    System,
    Named(String),
}

#[cfg(windows)]
struct SessionHandle {
    handle: Handle,
}

#[cfg(windows)]
// SAFETY: WinHTTP handles can move across threads. The session mutex serializes
// WinHttpConnect on the same handle, and each request retains its session until it closes.
unsafe impl Send for SessionHandle {}

#[cfg(windows)]
struct CachedSession {
    handle: Arc<Mutex<SessionHandle>>,
    idle_since: Option<Instant>,
}

#[cfg(windows)]
static SESSIONS: OnceLock<Mutex<HashMap<ProxyPolicy, CachedSession>>> = OnceLock::new();

#[cfg(windows)]
fn acquire_session(opts: &RequestOptions<'_>) -> Result<Arc<Mutex<SessionHandle>>, String> {
    let proxy = if let Some(named) = opts.use_env_proxy.then(env_proxy).flatten() {
        ProxyPolicy::Named(named)
    } else if opts.use_system_proxy {
        ProxyPolicy::System
    } else {
        ProxyPolicy::Direct
    };
    let mut sessions = SESSIONS
        .get_or_init(Mutex::default)
        .lock()
        .expect("HTTP pool lock");
    let entry = match sessions.entry(proxy) {
        Entry::Occupied(entry) => entry.into_mut(),
        Entry::Vacant(entry) => {
            let (access_type, named) = match entry.key() {
                ProxyPolicy::Direct => (WINHTTP_ACCESS_TYPE_NO_PROXY, None),
                ProxyPolicy::System => (WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, None),
                ProxyPolicy::Named(proxy) => (
                    WINHTTP_ACCESS_TYPE_NAMED_PROXY,
                    Some(HSTRING::from(proxy)),
                ),
            };
            let ua = HSTRING::from("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Folio/1.0");
            let proxy = named.as_ref().map_or(PCWSTR::null(), |name| PCWSTR(name.as_ptr()));
            let raw = unsafe { WinHttpOpen(&ua, access_type, proxy, PCWSTR::null(), 0) };
            if raw.is_null() {
                return Err(crate::i18n::media::http_init_session(opts.label));
            }
            let handle = Handle(raw);
            // Keep idle connections owned by this session so reaping it also closes its pool.
            unsafe {
                WinHttpSetOption(
                    Some(handle.0),
                    WINHTTP_OPTION_DISABLE_GLOBAL_POOLING,
                    Some(&1u32.to_ne_bytes()),
                )
            }
            .map_err(|error| {
                format!("{} ({error})", crate::i18n::media::http_init_session(opts.label))
            })?;
            entry.insert(CachedSession {
                handle: Arc::new(Mutex::new(SessionHandle { handle })),
                idle_since: None,
            })
        }
    };
    entry.idle_since = None;
    Ok(entry.handle.clone())
}

#[cfg(windows)]
pub(crate) fn reap_idle_sessions() {
    let Some(sessions) = SESSIONS.get() else {
        return;
    };
    let now = Instant::now();
    // The pool is the only source of new leases. Count its own reference while holding
    // the pool lock, then close expired handles after unlocking for new requests.
    let expired: Vec<_> = sessions
        .lock()
        .expect("HTTP pool lock")
        .extract_if(|_, session| {
            if Arc::strong_count(&session.handle) > 1 {
                return false;
            }
            now.duration_since(*session.idle_since.get_or_insert(now)) >= Duration::from_secs(30)
        })
        .collect();
    drop(expired);
}

#[cfg(windows)]
pub fn send(opts: RequestOptions<'_>) -> Result<HttpResponse, String> {
    let session = acquire_session(&opts)?;
    let host_h = HSTRING::from(opts.host);
    let connect = {
        let session = session.lock().expect("HTTP session lock");
        unsafe { WinHttpConnect(session.handle.0, &host_h, opts.port, 0) }
    };
    if connect.is_null() {
        return Err(crate::i18n::media::http_connect(opts.label));
    }
    let connect = Handle(connect);

    let verb = HSTRING::from(opts.method);
    let path_h = HSTRING::from(opts.path);
    let flags = if opts.https {
        WINHTTP_FLAG_SECURE
    } else {
        WINHTTP_OPEN_REQUEST_FLAGS(0)
    };
    let request = unsafe {
        WinHttpOpenRequest(
            connect.0,
            &verb,
            &path_h,
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            flags,
        )
    };
    if request.is_null() {
        return Err(crate::i18n::media::http_open_req(opts.label));
    }
    let request = Handle(request);

    // Timeouts and mutable request options belong to the request, including when
    // health probes and long requests share the same WinHTTP session.
    unsafe {
        WinHttpSetTimeouts(
            request.0,
            opts.connect_timeout_ms,
            opts.connect_timeout_ms,
            opts.receive_timeout_ms,
            opts.receive_timeout_ms,
        )
        .and_then(|_| {
            // These API calls were stateless across requests when each had its own session.
            WinHttpSetOption(
                Some(request.0),
                WINHTTP_OPTION_DISABLE_FEATURE,
                Some(&WINHTTP_DISABLE_COOKIES.to_ne_bytes()),
            )
        })
    }
    .map_err(|error| format!("{} ({error})", crate::i18n::media::http_open_req(opts.label)))?;

    let joined_headers = opts
        .headers
        .iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join("\r\n");
    let headers_h = (!joined_headers.is_empty()).then(|| HSTRING::from(joined_headers));
    let body_ptr = (!opts.body.is_empty()).then_some(opts.body.as_ptr() as *const c_void);
    let body_len = u32::try_from(opts.body.len())
        .map_err(|_| format!("{}: request body exceeds 4 GiB", opts.label))?;

    let sent = unsafe {
        WinHttpSendRequest(
            request.0,
            headers_h.as_deref(),
            body_ptr,
            body_len,
            body_len,
            0,
        )
    };
    if sent.is_err() {
        return Err(crate::i18n::media::http_send_req(opts.label));
    }

    if unsafe { WinHttpReceiveResponse(request.0, std::ptr::null_mut()) }.is_err() {
        return Err(crate::i18n::media::http_recv_timeout(opts.label));
    }

    let mut status: u32 = 0;
    let mut status_len: u32 = std::mem::size_of::<u32>() as u32;
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut status as *mut u32).cast::<c_void>()),
            &mut status_len,
            std::ptr::null_mut(),
        )
    }
    .map_err(|error| format!("{} ({error})", crate::i18n::media::http_read_err(opts.label)))?;

    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let mut read = 0u32;
        if unsafe {
            WinHttpReadData(
                request.0,
                buf.as_mut_ptr().cast::<c_void>(),
                buf.len() as u32,
                &mut read,
            )
        }
        .is_err()
        {
            return Err(crate::i18n::media::http_read_err(opts.label));
        }
        if read == 0 {
            break;
        }
        if read as usize > opts.max_response_bytes - out.len() {
            return Err(crate::i18n::media::http_too_large(opts.label));
        }
        out.extend_from_slice(&buf[..read as usize]);
    }

    Ok(HttpResponse {
        status: status as u16,
        body: out,
    })
}

#[cfg(not(windows))]
pub fn send(opts: RequestOptions<'_>) -> Result<HttpResponse, String> {
    let scheme = if opts.https { "https" } else { "http" };
    let url = format!("{scheme}://{}:{}{}", opts.host, opts.port, opts.path);
    let max_secs = ((opts.receive_timeout_ms.max(1000) + 999) / 1000).to_string();
    let mut cmd = std::process::Command::new("curl");
    cmd.args(["-sS", "-X", opts.method, "--max-time", &max_secs, "-w", "\n%{http_code}"]);
    for (k, v) in opts.headers {
        cmd.args(["-H", &format!("{k}: {v}")]);
    }
    if !opts.body.is_empty() {
        cmd.args(["--data-binary", "@-"]);
    }
    cmd.arg(&url);
    use std::io::Write;
    let mut child = cmd
        .stdin(if opts.body.is_empty() {
            std::process::Stdio::null()
        } else {
            std::process::Stdio::piped()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| crate::i18n::media::curl_invoke_failed(opts.label, &e))?;
    if !opts.body.is_empty() {
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(opts.body).map_err(|e| e.to_string())?;
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|e| crate::i18n::media::curl_invoke_failed(opts.label, &e))?;
    if !output.status.success() {
        return Err(crate::i18n::media::curl_req_failed(opts.label));
    }
    let Some(split) = output.stdout.iter().rposition(|b| *b == b'\n') else {
        return Err(crate::i18n::media::curl_req_failed(opts.label));
    };
    let status = std::str::from_utf8(&output.stdout[split + 1..])
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .unwrap_or(0);
    let body = output.stdout[..split].to_vec();
    if body.len() > opts.max_response_bytes {
        return Err(crate::i18n::media::http_too_large(opts.label));
    }
    Ok(HttpResponse { status, body })
}
