//! Starting, stopping and restarting the service from the app, so camless is one
//! thing to launch. The service needs administrator rights for CPU temperature and
//! power, so the app starts it through UAC; if that is declined, it starts it
//! without them and says so. Each action runs on its own thread: UAC can wait on
//! the user for as long as they like.

use crate::client;
use anyhow::{anyhow, bail, Context, Result};
use camless_core::config::{self, Config};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Restart,
}

impl Action {
    fn doing(self) -> &'static str {
        match self {
            Action::Start => "starting",
            Action::Stop => "stopping",
            Action::Restart => "restarting",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct State {
    /// `starting`, `stopping` or `restarting` while an action runs.
    pub busy: Option<&'static str>,
    /// What went wrong the last time, or what the user should know about it.
    pub note: Option<String>,
}

static STATE: Mutex<State> = Mutex::new(State { busy: None, note: None });

pub fn state() -> State {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Where the service that runs `config` listens, or `None` when its API is off and
/// the app cannot see it.
fn api_addr(config: &Path) -> Option<String> {
    match Config::load(config) {
        Ok(c) if !c.api.enabled => None,
        Ok(c) => Some(c.api.listen),
        Err(_) => Some(camless_core::DEFAULT_API.to_string()),
    }
}

fn running(addr: &str) -> bool {
    client::get(addr, "/v1/status").is_ok()
}

/// Runs `action` in the background for the service that runs `config`; does nothing
/// while another one runs. A restart starts it again with the same file.
pub fn spawn(action: Action, config: PathBuf) -> Result<()> {
    let addr = api_addr(&config).context("Local API is off, so the app cannot see or stop the service")?;
    {
        let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(b) = s.busy {
            bail!("the service is already {b}");
        }
        *s = State { busy: Some(action.doing()), note: None };
    }
    std::thread::Builder::new().name("service".into()).spawn(move || {
        let result = match action {
            Action::Start => start(&addr, &config),
            Action::Stop => stop(&addr),
            Action::Restart => stop(&addr).and_then(|_| start(&addr, &config)),
        };
        let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
        s.busy = None;
        match result {
            Ok(note) => s.note = note,
            Err(e) => s.note = Some(format!("{e:#}")),
        }
    })?;
    Ok(())
}

/// At launch: the service starts with the app, on the default config, unless it
/// already runs. The check can wait on a socket, so it runs on the action's thread,
/// which holds off other actions until it is done.
pub fn start_if_stopped() {
    let _ = spawn(Action::Start, config::default_path());
}

fn start(addr: &str, config: &Path) -> Result<Option<String>> {
    if running(addr) {
        return Ok(None);
    }
    let exe = client::daemon_exe().context("camless.exe is not next to camless-gui.exe")?;
    let note = match run_as_admin(&exe, config) {
        Ok(()) => None,
        Err(Elevate::Declined) => {
            client::start_daemon(config)?;
            Some("Started without administrator rights, so CPU temperature and power are off.".to_string())
        }
        Err(Elevate::Failed(e)) => return Err(e),
    };
    wait(Duration::from_secs(10), || running(addr)).map_err(|_| anyhow!("the service did not start within 10 s"))?;
    Ok(note)
}

fn stop(addr: &str) -> Result<Option<String>> {
    if !running(addr) {
        return Ok(None);
    }
    client::post_json(addr, "/v1/stop", "{}")?;
    // It puts the fans in their safe state and the LCD back before it exits.
    wait(Duration::from_secs(10), || !running(addr)).map_err(|_| anyhow!("the service did not stop within 10 s"))?;
    Ok(None)
}

fn wait(limit: Duration, done: impl Fn() -> bool) -> std::result::Result<(), ()> {
    let until = Instant::now() + limit;
    while Instant::now() < until {
        if done() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(())
}

enum Elevate {
    Declined,
    Failed(anyhow::Error),
}

/// `camless.exe run <config>` through UAC, with no console window. Returns once it
/// started.
fn run_as_admin(exe: &Path, config: &Path) -> std::result::Result<(), Elevate> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_CANCELLED};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (verb, file, params) = (wide("runas"), wide(&exe.display().to_string()), wide(&run_args(config)));
    let dir = wide(&exe.parent().map(|d| d.display().to_string()).unwrap_or_default());
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOASYNC | SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.lpDirectory = dir.as_ptr();
        info.nShow = SW_HIDE;
        if ShellExecuteExW(&mut info) == 0 {
            let err = GetLastError();
            return Err(if err == ERROR_CANCELLED {
                Elevate::Declined
            } else {
                Elevate::Failed(anyhow!("starting {} as administrator: {}", exe.display(), std::io::Error::from_raw_os_error(err as i32)))
            });
        }
        if !info.hProcess.is_null() {
            CloseHandle(info.hProcess);
        }
    }
    Ok(())
}

/// The command line for `run`. Windows paths cannot hold `"`, and the config is a
/// file, so its path never ends in a backslash that would escape the closing quote.
fn run_args(config: &Path) -> String {
    format!("run \"{}\"", config.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_args_quote_the_config() {
        let p = Path::new(r"C:\Users\A B\AppData\Roaming\camless\camless.toml");
        assert_eq!(run_args(p), r#"run "C:\Users\A B\AppData\Roaming\camless\camless.toml""#);
    }
}
