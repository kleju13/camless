//! HTTPS GET through WinHTTP, which comes with Windows and uses its certificates and
//! proxy settings: enough for updates without a TLS stack in the app. Redirects are
//! followed, as release downloads need.

use anyhow::{anyhow, bail, Context, Result};
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Networking::WinHttp::*;

/// GitHub's API turns away requests without a User-Agent.
const AGENT: &str = concat!("camless/", env!("CARGO_PKG_VERSION"));

/// Larger answers are not a release of camless.
const MAX_BODY: usize = 64 << 20;

struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { WinHttpCloseHandle(self.0) };
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn handle(h: *mut c_void, what: &str) -> Result<Handle> {
    if h.is_null() {
        return Err(failed(what));
    }
    Ok(Handle(h))
}

/// WinHTTP's own codes have no system message, so the common ones get words here.
fn failed(what: &str) -> anyhow::Error {
    let code = unsafe { GetLastError() };
    let why = match code {
        12002 => "timed out",
        12007 => "the name did not resolve; is this PC online?",
        12029 => "could not connect",
        12175 => "the secure connection failed",
        _ => "failed",
    };
    anyhow!("{what}: {why} (WinHTTP error {code})")
}

/// The body of `url`, which must be https, if it answers 200.
pub fn get(url: &str) -> Result<Vec<u8>> {
    let rest = url.strip_prefix("https://").with_context(|| format!("{url} is not an https URL"))?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    unsafe {
        let session = handle(WinHttpOpen(wide(AGENT).as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, null(), null(), 0), "opening WinHTTP")?;
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 10_000, 30_000);
        let connect = handle(WinHttpConnect(session.0, wide(host).as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0), host)?;
        let request = handle(
            WinHttpOpenRequest(connect.0, wide("GET").as_ptr(), wide(path).as_ptr(), null(), null(), null(), WINHTTP_FLAG_SECURE),
            host,
        )?;
        if WinHttpSendRequest(request.0, null(), 0, null(), 0, 0, 0) == 0 || WinHttpReceiveResponse(request.0, null_mut()) == 0 {
            return Err(failed(host));
        }
        let (mut status, mut size) = (0u32, 4u32);
        let flags = WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER;
        if WinHttpQueryHeaders(request.0, flags, null(), &mut status as *mut u32 as *mut c_void, &mut size, null_mut()) == 0 {
            return Err(failed(host));
        }
        if status != 200 {
            bail!("{host} answered {status}");
        }
        let (mut body, mut chunk) = (Vec::new(), vec![0u8; 64 << 10]);
        loop {
            let mut read = 0u32;
            if WinHttpReadData(request.0, chunk.as_mut_ptr() as *mut c_void, chunk.len() as u32, &mut read) == 0 {
                return Err(failed(host));
            }
            if read == 0 {
                return Ok(body);
            }
            body.extend_from_slice(&chunk[..read as usize]);
            if body.len() > MAX_BODY {
                bail!("{url} is larger than {} MB", MAX_BODY >> 20);
            }
        }
    }
}
