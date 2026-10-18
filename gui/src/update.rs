//! Updates from the GitHub releases that release.yml publishes. The app asks for the
//! latest one once at launch, and again from Settings. When it is newer, the page
//! offers it. Installing downloads camless-setup.exe, checks it against the release's
//! SHA-256 and quits the app, which starts the setup on its way out, once the page
//! has saved and the tray icon is gone. The setup stops the service, replaces both
//! exes and opens the app again, which starts the service.

use crate::http;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

const LATEST: &str = "https://api.github.com/repos/kleju13/camless/releases/latest";
const SETUP: &str = "camless-setup.exe";
/// Shows its progress and asks nothing; errors still show.
const SETUP_ARGS: [&str; 2] = ["/SILENT", "/NORESTART"];

#[derive(Clone, Debug, Default, Serialize)]
pub struct State {
    /// `checking`, `downloading` or `installing` while that runs.
    pub busy: Option<&'static str>,
    /// The latest release's version, once a check got it.
    pub latest: Option<String>,
    /// The latest release is newer than this app.
    pub available: bool,
    /// What went wrong the last time.
    pub error: Option<String>,
    #[serde(skip)]
    release: Option<Release>,
}

#[derive(Clone, Debug)]
struct Release {
    version: String,
    page: String,
    setup: String,
    sum: String,
}

static STATE: Mutex<State> = Mutex::new(State { busy: None, latest: None, available: false, error: None, release: None });
/// How the app quits; the window's event loop sets it, and `--serve` has none.
static QUIT: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
/// The checked setup, for `run_ready` once the app has quit.
static READY: Mutex<Option<PathBuf>> = Mutex::new(None);

fn lock() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn state() -> State {
    lock().clone()
}

pub fn on_quit(quit: impl Fn() + Send + Sync + 'static) {
    let _ = QUIT.set(Box::new(quit));
}

fn begin(busy: &'static str) -> Result<()> {
    let mut s = lock();
    if let Some(b) = s.busy {
        bail!("camless is already {b} an update");
    }
    s.busy = Some(busy);
    s.error = None;
    Ok(())
}

fn fail(e: anyhow::Error) {
    let mut s = lock();
    s.busy = None;
    s.error = Some(format!("{e:#}"));
}

/// Asks GitHub for the latest release, in the background.
pub fn check() -> Result<()> {
    begin("checking")?;
    std::thread::Builder::new().name("update".into()).spawn(|| match latest() {
        Ok(r) => {
            let mut s = lock();
            s.busy = None;
            s.available = newer(&r.version, env!("CARGO_PKG_VERSION"));
            s.latest = Some(r.version.clone());
            s.release = Some(r);
        }
        Err(e) => fail(e),
    })?;
    Ok(())
}

fn latest() -> Result<Release> {
    #[derive(Deserialize)]
    struct Found {
        tag_name: String,
        html_url: String,
        assets: Vec<Asset>,
    }
    #[derive(Deserialize)]
    struct Asset {
        name: String,
        browser_download_url: String,
    }
    let body = http::get(LATEST).context("asking GitHub for the latest release")?;
    let found: Found = serde_json::from_slice(&body).context("reading GitHub's answer")?;
    let url = |name: &str| {
        let asset = found.assets.iter().find(|a| a.name == name);
        asset.map(|a| a.browser_download_url.clone()).with_context(|| format!("release {} has no {name}", found.tag_name))
    };
    let (setup, sum) = (url(SETUP)?, url(&format!("{SETUP}.sha256"))?);
    Ok(Release { version: found.tag_name.trim_start_matches('v').to_string(), page: found.html_url, setup, sum })
}

/// Downloads and checks the newer release's setup in the background, then quits the app.
pub fn install() -> Result<()> {
    let quit = QUIT.get().context("only the app's own window installs updates")?;
    let release = {
        let s = lock();
        s.release.clone().filter(|_| s.available).context("there is no newer release to install; check again")?
    };
    begin("downloading")?;
    std::thread::Builder::new().name("update".into()).spawn(move || match download(&release) {
        Ok(path) => {
            *READY.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
            lock().busy = Some("installing");
            quit();
        }
        Err(e) => fail(e),
    })?;
    Ok(())
}

fn download(r: &Release) -> Result<PathBuf> {
    let setup = http::get(&r.setup).with_context(|| format!("downloading {SETUP}"))?;
    let sum = http::get(&r.sum).with_context(|| format!("downloading {SETUP}.sha256"))?;
    let want = String::from_utf8_lossy(&sum).split_whitespace().next().unwrap_or_default().to_string();
    let have = hex(&sha256(&setup)?);
    if !have.eq_ignore_ascii_case(&want) {
        bail!("the downloaded {SETUP} does not match its checksum ({have}, expected {want})");
    }
    let dir = std::env::temp_dir().join("camless-update");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(SETUP);
    std::fs::write(&path, &setup).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Starts the setup that `install` got ready, after the app's event loop has ended.
pub fn run_ready() {
    let Some(path) = READY.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
    if let Err(e) = std::process::Command::new(&path).args(SETUP_ARGS).spawn() {
        // The window is gone, so this is the only way to say so.
        message(&format!("camless could not start the update.\n\n{}: {e}\n\nRun it by hand, or open camless again.", path.display()));
    }
}

/// The release's page on GitHub, for its notes.
pub fn page() -> Option<String> {
    lock().release.as_ref().map(|r| r.page.clone()).filter(|p| p.starts_with("https://github.com/"))
}

fn message(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    unsafe { MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide("camless").as_ptr(), MB_OK | MB_ICONERROR) };
}

/// Whether version `a` is newer than `b`, comparing `x.y.z` as numbers.
fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| v.split(['.', '-', '+']).take(3).map(|p| p.parse().unwrap_or(0)).collect::<Vec<u64>>();
    parts(a) > parts(b)
}

fn sha256(data: &[u8]) -> Result<[u8; 32]> {
    use windows_sys::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE};
    let mut out = [0u8; 32];
    let status = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, std::ptr::null(), 0, data.as_ptr(), data.len() as u32, out.as_mut_ptr(), 32) };
    if status != 0 {
        bail!("SHA-256 failed (NTSTATUS {status:#x})");
    }
    Ok(out)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert!(newer("0.2.10", "0.2.9"));
        assert!(newer("0.3.0", "0.2.0"));
        assert!(newer("1.0.0", "0.99.99"));
        assert!(!newer("0.2.0", "0.2.0"));
        assert!(!newer("0.1.9", "0.2.0"));
        assert!(!newer("0.3.0-rc1", "0.3.0"));
    }

    #[test]
    fn sha256_matches_the_standard_vector() {
        assert_eq!(hex(&sha256(b"abc").unwrap()), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
