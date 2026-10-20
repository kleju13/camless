//! Starting, stopping and restarting the service from the app, so camless is one
//! thing to launch. The service needs administrator rights for CPU temperature and
//! power, so the app starts it through UAC, or through the task that needs none once
//! the user turned it on; if UAC is declined, it starts it without them and says so.
//! Each action runs on its own thread: UAC can wait on the user for as long as they
//! like.
//!
//! On Linux the service runs as the user and needs no rights of its own: camless's
//! udev rules give that user the devices. The app starts it as the systemd user
//! service that linux/install.sh sets up, or without it when there is none.

use crate::client;
#[cfg(windows)]
use crate::task;
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
    #[cfg(not(windows))]
    let note = {
        client::daemon_exe().context(client::NO_DAEMON)?;
        // The user service runs the default config.
        if config == config::default_path() && systemd::installed() {
            systemd::start()?;
        } else {
            client::start_daemon(config)?;
        }
        None
    };
    #[cfg(windows)]
    let exe = client::daemon_exe().context(client::NO_DAEMON)?;
    // The task runs the default config. Without it, or if it is gone, UAC asks.
    #[cfg(windows)]
    let note = if config == config::default_path() && task::ready() && task::run().is_ok() {
        None
    } else {
        match run_elevated(&exe, &run_args(config), false) {
            Ok(_) => None,
            Err(Elevate::Declined) => {
                client::start_daemon(config)?;
                Some("Started without administrator rights, so CPU temperature and power are off.".to_string())
            }
            Err(Elevate::Failed(e)) => return Err(e),
        }
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

/// The systemd user service in `~/.config/systemd/user`, which runs `camless run`
/// on the default config.
#[cfg(not(windows))]
mod systemd {
    use anyhow::{bail, Context, Result};
    use std::process::{Command, Stdio};

    const UNIT: &str = "camless.service";

    pub fn installed() -> bool {
        let out = Command::new("systemctl").args(["--user", "show", "--property=LoadState", "--value", UNIT]).stderr(Stdio::null()).output();
        out.is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "loaded")
    }

    pub fn start() -> Result<()> {
        let out = Command::new("systemctl").args(["--user", "start", UNIT]).stdin(Stdio::null()).output().context("running systemctl")?;
        if !out.status.success() {
            bail!("systemctl --user start {UNIT}: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    }
}

#[cfg(windows)]
pub enum Elevate {
    Declined,
    Failed(anyhow::Error),
}

/// `exe` with `params` through UAC, with no console window. Returns once it started,
/// or with its exit code once it ended if `wait`.
#[cfg(windows)]
pub fn run_elevated(exe: &Path, params: &str, wait: bool) -> std::result::Result<Option<u32>, Elevate> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_CANCELLED};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (verb, file, params) = (wide("runas"), wide(&exe.display().to_string()), wide(params));
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
        let mut code = None;
        if !info.hProcess.is_null() {
            let mut c = 0;
            if wait && WaitForSingleObject(info.hProcess, INFINITE) == 0 && GetExitCodeProcess(info.hProcess, &mut c) != 0 {
                code = Some(c);
            }
            CloseHandle(info.hProcess);
        }
        Ok(code)
    }
}

/// The command line for `run`. Windows paths cannot hold `"`, and the config is a
/// file, so its path never ends in a backslash that would escape the closing quote.
#[cfg(windows)]
fn run_args(config: &Path) -> String {
    format!("run \"{}\"", config.display())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn run_args_quote_the_config() {
        let p = Path::new(r"C:\Users\A B\AppData\Roaming\camless\camless.toml");
        assert_eq!(run_args(p), r#"run "C:\Users\A B\AppData\Roaming\camless\camless.toml""#);
    }
}
