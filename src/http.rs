//! Minimal HTTP/HTTPS client backed by WinHTTP on Windows and `curl` elsewhere.

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
pub fn send(opts: RequestOptions<'_>) -> Result<HttpResponse, String> {
    use std::ffi::c_void;
    use windows::{
        Win32::Networking::WinHttp::{
            WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
            WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
            WinHttpSetTimeouts, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            WINHTTP_ACCESS_TYPE_NAMED_PROXY, WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_FLAG_SECURE,
            WINHTTP_OPEN_REQUEST_FLAGS, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
        },
        core::{HSTRING, PCWSTR},
    };

    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = WinHttpCloseHandle(self.0);
                }
            }
        }
    }

    let ua = HSTRING::from("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Folio/1.0");
    let proxy = if opts.use_env_proxy { env_proxy() } else { None };
    let proxy_h = proxy.as_deref().map(HSTRING::from);
    let session = unsafe {
        if let Some(ref p) = proxy_h {
            WinHttpOpen(&ua, WINHTTP_ACCESS_TYPE_NAMED_PROXY, p, PCWSTR::null(), 0)
        } else if opts.use_system_proxy {
            WinHttpOpen(
                &ua,
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            )
        } else {
            WinHttpOpen(
                &ua,
                WINHTTP_ACCESS_TYPE_NO_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            )
        }
    };
    if session.is_null() {
        return Err(crate::i18n::media::http_init_session(opts.label));
    }
    let session = Handle(session);

    unsafe {
        let _ = WinHttpSetTimeouts(
            session.0,
            opts.connect_timeout_ms,
            opts.connect_timeout_ms,
            opts.receive_timeout_ms,
            opts.receive_timeout_ms,
        );
    }

    let host_h = HSTRING::from(opts.host);
    let connect = unsafe { WinHttpConnect(session.0, &host_h, opts.port, 0) };
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
    let _ = unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut status as *mut u32).cast::<c_void>()),
            &mut status_len,
            std::ptr::null_mut(),
        )
    };

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
        out.extend_from_slice(&buf[..read as usize]);
        if out.len() > opts.max_response_bytes {
            return Err(crate::i18n::media::http_too_large(opts.label));
        }
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
