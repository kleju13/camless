//! Updates from the GitHub releases that release.yml publishes. The app asks for the
//! latest one once at launch, and again from Settings. When it is newer, the page
//! offers it. On the beta channel (`updates = "beta"` in camless.toml) betas count
//! too, the GitHub pre-releases, and the newest version of all is offered. Installing downloads camless-setup.exe, checks it against the release's
//! SHA-256 and quits the app, which starts the setup on its way out, once the page
//! has saved and the tray icon is gone. The setup stops the service, replaces both
//! exes and opens the app again, which starts the service.

use crate::http;
use anyhow::{bail, Context, Result};
use camless_core::config::{Config, Updates};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

const LATEST: &str = "https://api.github.com/repos/kleju13/camless/releases/latest";
/// The newest releases and betas, for the beta channel.
const RELEASES: &str = "https://api.github.com/repos/kleju13/camless/releases?per_page=30";
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
    /// The latest release is a beta.
    pub beta: bool,
    /// What went wrong the last time.
    pub error: Option<String>,
    #[serde(skip)]
    release: Option<Release>,
    /// The channel the last check was asked for.
    #[serde(skip)]
    wanted: Updates,
}

#[derive(Clone, Debug)]
struct Release {
    version: String,
    beta: bool,
    page: String,
    setup: String,
    sum: String,
}

/// A release as GitHub lists it.
#[derive(Deserialize)]
struct Found {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

impl Found {
    fn version(&self) -> &str {
        self.tag_name.trim_start_matches('v')
    }
}

/// The channel set in the config at `path`; stable when it does not load.
pub fn channel(path: &Path) -> Updates {
    Config::load(path).map(|c| c.updates).unwrap_or_default()
}

static STATE: Mutex<State> =
    Mutex::new(State { busy: None, latest: None, available: false, beta: false, error: None, release: None, wanted: Updates::Stable });
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

/// Asks GitHub for the latest release on `channel`, in the background. A check that
/// is running already starts again if it was for the other channel, so a release from
/// the channel left behind is never offered.
pub fn check(channel: Updates) -> Result<()> {
    {
        let mut s = lock();
        s.wanted = channel;
        if s.busy == Some("checking") {
            return Ok(());
        }
        if let Some(b) = s.busy {
            bail!("camless is already {b} an update");
        }
        s.busy = Some("checking");
        (s.error, s.latest, s.available, s.beta, s.release) = (None, None, false, false, None);
    }
    std::thread::Builder::new().name("update".into()).spawn(move || {
        let mut channel = channel;
        loop {
            let found = latest(channel);
            let mut s = lock();
            if s.wanted != channel {
                channel = s.wanted;
                continue;
            }
            s.busy = None;
            match found {
                Ok(r) => {
                    s.available = newer(&r.version, env!("CARGO_PKG_VERSION"));
                    s.latest = Some(r.version.clone());
                    s.beta = r.beta;
                    s.release = Some(r);
                }
                Err(e) => s.error = Some(format!("{e:#}")),
            }
            return;
        }
    })?;
    Ok(())
}

fn latest(channel: Updates) -> Result<Release> {
    let found = match channel {
        Updates::Stable => {
            let body = http::get(LATEST).context("asking GitHub for the latest release")?;
            serde_json::from_slice::<Found>(&body).context("reading GitHub's answer")?
        }
        Updates::Beta => {
            let body = http::get(RELEASES).context("asking GitHub for the latest releases")?;
            let all: Vec<Found> = serde_json::from_slice(&body).context("reading GitHub's answer")?;
            newest(all).context("GitHub lists no release with a setup")?
        }
    };
    let url = |name: &str| {
        let asset = found.assets.iter().find(|a| a.name == name);
        asset.map(|a| a.browser_download_url.clone()).with_context(|| format!("release {} has no {name}", found.tag_name))
    };
    let (setup, sum) = (url(SETUP)?, url(&format!("{SETUP}.sha256"))?);
    Ok(Release { version: found.version().to_string(), beta: found.prerelease, page: found.html_url.clone(), setup, sum })
}

/// The newest version among the releases and betas that have a setup and its
/// checksum; GitHub lists a release while its files are still uploading.
fn newest(all: Vec<Found>) -> Option<Found> {
    let has = |f: &Found, name: &str| f.assets.iter().any(|a| a.name == name);
    let ready = all.into_iter().filter(|f| !f.draft && has(f, SETUP) && has(f, &format!("{SETUP}.sha256")));
    ready.reduce(|best, f| if newer(f.version(), best.version()) { f } else { best })
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

/// Whether version `a` is newer than `b`: `x.y.z` as numbers, then a release after
/// its own pre-releases (`0.3.0` after `0.3.0-rc1`), and pre-releases by name.
fn newer(a: &str, b: &str) -> bool {
    fn parse(v: &str) -> ([u64; 3], Option<&str>) {
        let v = v.split('+').next().unwrap_or_default();
        let (core, pre) = v.split_once('-').map_or((v, None), |(c, p)| (c, Some(p)));
        let mut n = [0u64; 3];
        for (slot, part) in n.iter_mut().zip(core.split('.')) {
            *slot = part.parse().unwrap_or(0);
        }
        (n, pre)
    }
    let ((na, pa), (nb, pb)) = (parse(a), parse(b));
    if na != nb {
        return na > nb;
    }
    match (pa, pb) {
        (None, Some(_)) => true,
        (Some(x), Some(y)) => x > y,
        _ => false,
    }
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
        assert!(newer("0.3.0", "0.3.0-rc1"));
        assert!(newer("0.3.0-rc2", "0.3.0-rc1"));
        assert!(newer("0.3.0-rc1", "0.2.9"));
        assert!(!newer("0.2.0+build5", "0.2.0"));
    }

    #[test]
    fn the_beta_channel_takes_the_newest_version_with_a_setup() {
        let list = |json: &str| serde_json::from_str::<Vec<Found>>(json).unwrap();
        let setup = r#"[{"name":"camless-setup.exe","browser_download_url":"https://x/s"},{"name":"camless-setup.exe.sha256","browser_download_url":"https://x/h"}]"#;
        let all = list(&format!(
            r#"[{{"tag_name":"v0.5.0","html_url":"https://github.com/r/5","prerelease":true,"assets":[]}},
               {{"tag_name":"v0.4.5","html_url":"https://github.com/r/45","prerelease":true,"assets":[{{"name":"camless-setup.exe","browser_download_url":"https://x/s"}}]}},
               {{"tag_name":"v0.4.1","html_url":"https://github.com/r/41","prerelease":true,"assets":{setup}}},
               {{"tag_name":"v0.4.10","html_url":"https://github.com/r/410","draft":true,"assets":{setup}}},
               {{"tag_name":"v0.4.0","html_url":"https://github.com/r/40","assets":{setup}}}]"#
        ));
        // 0.5.0 has no setup yet, 0.4.5 no checksum, and 0.4.10 is a draft.
        let f = newest(all).unwrap();
        assert_eq!((f.version(), f.prerelease), ("0.4.1", true));
        // A release newer than every beta wins.
        let all = list(&format!(
            r#"[{{"tag_name":"v0.5.0","html_url":"https://github.com/r/5","assets":{setup}}},
               {{"tag_name":"v0.4.2","html_url":"https://github.com/r/42","prerelease":true,"assets":{setup}}}]"#
        ));
        assert_eq!(newest(all).unwrap().version(), "0.5.0");
        assert!(newest(Vec::new()).is_none());
    }

    #[test]
    fn sha256_matches_the_standard_vector() {
        assert_eq!(hex(&sha256(b"abc").unwrap()), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
