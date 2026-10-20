//! A hardware report for a GitHub issue. `camless report --json` gathers it in
//! the background, as asking the service and the devices takes a few seconds. The
//! page shows all of it; Open on GitHub opens a new issue with the report filled in.
//! Nothing is sent from here: the issue exists once the person submits it on GitHub.

use crate::client;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const NEW_ISSUE: &str = "https://github.com/kleju13/camless/issues/new?template=hardware-report.yml";
const WORKS_ISSUE: &str = "https://github.com/kleju13/camless/issues/new?template=hardware-works.yml";
/// GitHub sends someone who is not signed in to its login page with the whole URL in
/// `return_to`, which fails from about 7000 characters; longer URLs get 414.
const MAX_URL: usize = 6000;
/// The form's report field, when the report did not fit in the URL.
const PASTE_HERE: &str = "camless copied the report to your clipboard. Paste it here (Ctrl+V) in place of this line.";

/// What the issue is about: a device camless does not drive or drives wrong, or one
/// that is not verified yet and works.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Problem,
    Works,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct State {
    pub busy: bool,
    pub title: Option<String>,
    pub text: Option<String>,
    /// The report fits in the issue's URL; otherwise the page copies it first.
    pub fits: bool,
    pub error: Option<String>,
}

static STATE: Mutex<State> = Mutex::new(State { busy: false, title: None, text: None, fits: false, error: None });

fn lock() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn state() -> State {
    lock().clone()
}

/// Gathers a report for the service that runs `config`, in the background.
pub fn start(config: &Path) -> Result<()> {
    let exe = client::daemon_exe().context(client::NO_DAEMON)?;
    {
        let mut s = lock();
        if s.busy {
            return Ok(());
        }
        *s = State { busy: true, ..Default::default() };
    }
    let config = config.to_path_buf();
    let spawned = std::thread::Builder::new().name("report".into()).spawn(move || {
        let result = run(&exe, &config);
        *lock() = match result {
            // A Works issue has the shorter URL, so this holds for both.
            Ok(r) => State { fits: issue_url(Kind::Problem, &r.title, &r.text).1, title: Some(r.title), text: Some(r.text), ..Default::default() },
            Err(e) => State { error: Some(format!("{e:#}")), ..Default::default() },
        };
    });
    if let Err(e) = spawned {
        *lock() = State::default();
        return Err(e.into());
    }
    Ok(())
}

#[derive(Deserialize)]
struct Report {
    title: String,
    text: String,
}

/// `camless report` gives up on the service after a few seconds by itself, but a
/// device that does not answer its descriptor reads could hold it up for good. On the
/// maintainer's PC it takes under half a second.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Where to run the report by hand when it takes too long here.
#[cfg(windows)]
const BY_HAND: &str = r#"Run it in PowerShell instead, where it can take as long as it needs: & "$env:LOCALAPPDATA\Programs\camless\camless.exe" report"#;
#[cfg(not(windows))]
const BY_HAND: &str = "Run it in a terminal instead, where it can take as long as it needs: camless report";

fn run(exe: &Path, config: &Path) -> Result<Report> {
    let mut command = Command::new(exe);
    command.arg("report").arg(config).arg("--json");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let what = format!("{} report", client::DAEMON);
    let Some(out) = output_within(&mut command, TIMEOUT).with_context(|| format!("running {what}"))? else {
        bail!("The report took more than {} seconds, so camless stopped it. {BY_HAND}", TIMEOUT.as_secs());
    };
    if !out.status.success() {
        bail!("{what} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    serde_json::from_slice(&out.stdout).with_context(|| format!("reading what {what} printed"))
}

/// `Command::output`, but the child is killed after `limit`, and then this gives `None`.
/// Threads read its output, so a full pipe cannot hold it up. It is not waited for after
/// the kill: a process stuck in a driver call can outlive TerminateProcess, and Windows
/// needs no wait to clean up.
fn output_within(command: &mut Command, limit: Duration) -> std::io::Result<Option<Output>> {
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let (stdout, stderr) = (drain(child.stdout.take()), drain(child.stderr.take()));
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait()? {
            let join = |t: JoinHandle<Vec<u8>>| t.join().unwrap_or_default();
            return Ok(Some(Output { status, stdout: join(stdout), stderr: join(stderr) }));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            // Linux keeps a killed child until it is waited for.
            #[cfg(not(windows))]
            thread::spawn(move || child.wait());
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    })
}

/// Opens the new issue in the default browser with the last report. Returns whether
/// the report is in the URL; if not, the page has put it on the clipboard.
pub fn open_issue(kind: Kind) -> Result<bool> {
    let s = state();
    let (Some(title), Some(text)) = (s.title, s.text) else {
        bail!("there is no report yet");
    };
    let (url, filled) = issue_url(kind, &title, &text);
    open_url(&url)?;
    Ok(filled)
}

/// The new-issue URL with the title and the report filled in, and whether the report
/// fit; if not, the report field asks for the clipboard.
fn issue_url(kind: Kind, title: &str, text: &str) -> (String, bool) {
    let (base, title) = match kind {
        Kind::Problem => (NEW_ISSUE, title.to_string()),
        Kind::Works => (WORKS_ISSUE, format!("Works: {}", title.strip_prefix("Hardware: ").unwrap_or(title))),
    };
    let url = |report: &str| format!("{base}&title={}&report={}", encode(&title), encode(report));
    let full = url(text);
    if full.len() <= MAX_URL {
        (full, true)
    } else {
        (url(PASTE_HERE), false)
    }
}

/// Percent-encodes a query value, with spaces as `+`.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => char::from(b).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// ShellExecuteExW hands the browser the whole URL. `explorer.exe <url>`, which
/// opens the release notes, drops URLs with a query and cuts others at 260 characters.
#[cfg(windows)]
pub fn open_url(url: &str) -> Result<()> {
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (verb, file) = (wide("open"), wide(url));
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.nShow = SW_SHOWNORMAL;
        if ShellExecuteExW(&mut info) == 0 {
            bail!("opening the browser: {}", std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// The desktop's browser for a URL, or its file manager for a folder, through
/// xdg-open, which every desktop has.
#[cfg(not(windows))]
pub fn open_url(target: &str) -> Result<()> {
    let mut child = Command::new("xdg-open")
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("running xdg-open")?;
    // It hands the target over and exits; reaped so no zombie stays.
    thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_query_value() {
        assert_eq!(encode("Hardware: Kraken X (1e71:2007)"), "Hardware%3A+Kraken+X+%281e71%3A2007%29");
        assert_eq!(encode("a\n**b** `c`|×"), "a%0A%2A%2Ab%2A%2A+%60c%60%7C%C3%97");
    }

    #[test]
    fn a_long_report_goes_through_the_clipboard() {
        let (url, filled) = issue_url(Kind::Problem, "Hardware: Control Hub", "camless 0.3.0\n**Devices camless drives**");
        assert!(filled);
        assert_eq!(
            url,
            "https://github.com/kleju13/camless/issues/new?template=hardware-report.yml&title=Hardware%3A+Control+Hub&report=camless+0.3.0%0A%2A%2ADevices+camless+drives%2A%2A"
        );
        let (url, filled) = issue_url(Kind::Problem, "Hardware: Control Hub", &"0600ff".repeat(1000));
        assert!(!filled);
        assert!(url.ends_with(&format!("&report={}", encode(PASTE_HERE))));
        assert!(url.len() <= MAX_URL);
    }

    #[test]
    fn a_works_report_has_its_own_form_and_title() {
        let (url, filled) = issue_url(Kind::Works, "Hardware: Kraken X3", "camless 0.5.0");
        assert!(filled);
        assert_eq!(url, "https://github.com/kleju13/camless/issues/new?template=hardware-works.yml&title=Works%3A+Kraken+X3&report=camless+0.5.0");
        let template = include_str!("../../.github/ISSUE_TEMPLATE/hardware-works.yml");
        assert!(template.contains("id: report"), "the app fills this field from the URL");
    }

    #[test]
    fn a_report_that_hangs_is_stopped() {
        // Both would take about 30 s: ping waits a second between pings.
        let start = Instant::now();
        #[cfg(windows)]
        let mut hangs = Command::new("ping");
        #[cfg(windows)]
        hangs.args(["-n", "31", "127.0.0.1"]);
        #[cfg(not(windows))]
        let mut hangs = Command::new("sleep");
        #[cfg(not(windows))]
        hangs.arg("30");
        let out = output_within(&mut hangs, Duration::from_millis(500)).unwrap();
        assert!(out.is_none());
        assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
    }

    #[test]
    fn a_long_output_does_not_fill_the_pipe() {
        // 4 MB. Read only after the child exits, this much holds `type` up until the kill.
        let file = std::env::temp_dir().join(format!("camless-report-pipe-{}.txt", std::process::id()));
        std::fs::write(&file, "0123456789abcdef".repeat(1 << 18)).unwrap();
        #[cfg(windows)]
        let out = output_within(Command::new("cmd").arg("/C").arg("type").arg(&file), TIMEOUT);
        #[cfg(not(windows))]
        let out = output_within(Command::new("cat").arg(&file), TIMEOUT);
        let _ = std::fs::remove_file(&file);
        let out = out.unwrap().expect("cmd finished");
        assert!(out.status.success());
        assert_eq!(out.stdout.len(), 16 << 18);
    }

    #[test]
    fn the_dialog_the_form_and_the_url_agree() {
        let form = include_str!("../../.github/ISSUE_TEMPLATE/hardware-report.yml");
        let page = include_str!("../ui/app.js");
        // The same words as `READ_ONLY` in the camless crate.
        let read_only = "read only: standard USB descriptor reads, no commands sent to the devices";
        assert!(form.contains(read_only) && page.contains(read_only));
        assert!(NEW_ISSUE.ends_with("template=hardware-report.yml") && form.contains("id: report"));
        assert!(form.contains("Settings → Devices → **Create report**") && page.contains("'Create report'"));
    }
}
