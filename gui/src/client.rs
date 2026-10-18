//! The daemon's loopback API. A thread polls `GET /v1/status` once a second while
//! someone holds the link, and stops when the last holder drops it.

use anyhow::{bail, Context, Result};
use camless_core::status::Status;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const POLL_EVERY: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct Link {
    /// Where the daemon listens; the config may move it.
    pub addr: String,
    pub status: Option<Status>,
    pub error: Option<String>,
}

/// One request over plain HTTP/1.0; the API is loopback-only and tiny.
pub fn get(addr: &str, path: &str) -> Result<String> {
    request(addr, "GET", path, None)
}

/// The service takes JSON only, which web pages cannot send it without asking first.
pub fn post_json(addr: &str, path: &str, body: &str) -> Result<String> {
    request(addr, "POST", path, Some(body))
}

fn request(addr: &str, method: &str, path: &str, json: Option<&str>) -> Result<String> {
    let sock: SocketAddr = addr.parse().with_context(|| format!("{addr:?} is not ip:port"))?;
    let mut s = TcpStream::connect_timeout(&sock, Duration::from_millis(300))?;
    s.set_read_timeout(Some(Duration::from_secs(2)))?;
    write!(s, "{method} {path} HTTP/1.0\r\nHost: {addr}\r\nConnection: close\r\n")?;
    match json {
        Some(b) => write!(s, "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}", b.len())?,
        None => write!(s, "\r\n")?,
    }
    let mut buf = Vec::new();
    s.read_to_end(&mut buf)?;
    let text = String::from_utf8(buf).context("reply is not UTF-8")?;
    let (head, body) = text.split_once("\r\n\r\n").context("malformed reply")?;
    let code = head.split_whitespace().nth(1).unwrap_or_default();
    if !code.starts_with('2') {
        bail!("{path} answered {code}");
    }
    Ok(body.to_string())
}

pub fn spawn(addr: String) -> Arc<Mutex<Link>> {
    let link = Arc::new(Mutex::new(Link { addr, ..Default::default() }));
    let shared = link.clone();
    std::thread::Builder::new()
        .name("status".into())
        .spawn(move || loop {
            // Whoever held the link is gone.
            if Arc::strong_count(&shared) == 1 {
                return;
            }
            let addr = shared.lock().unwrap_or_else(|e| e.into_inner()).addr.clone();
            let result = get(&addr, "/v1/status").and_then(|b| Ok(serde_json::from_str::<Status>(&b)?));
            {
                let mut l = shared.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(s) => {
                        l.status = Some(s);
                        l.error = None;
                    }
                    Err(e) => {
                        l.status = None;
                        l.error = Some(format!("{e:#}"));
                    }
                }
            }
            std::thread::sleep(POLL_EVERY);
        })
        .expect("spawning the status thread");
    link
}

/// `camless.exe` next to this program.
pub fn daemon_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.parent()?.join("camless.exe");
    exe.exists().then_some(exe)
}

/// Starts `camless run <config>` without a console window. It keeps running when
/// the app exits, so the fans stay on their curves.
pub fn start_daemon(config: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let exe = daemon_exe().context("camless.exe is not next to camless-gui.exe")?;
    std::process::Command::new(exe)
        .arg("run")
        .arg(config)
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("starting camless.exe")?;
    Ok(())
}
