//! A scheduled task that starts the service as administrator without a UAC prompt.
//! It is off until the user turns it on in Settings; Windows asks once then. From
//! then on the app starts the service through the task, at logon too.
//!
//! The task runs `camless-gui.exe --start-service`, which starts camless.exe next to
//! it and exits: a task that ran camless.exe itself would open a console window. Its
//! user may run, read and delete it, so the app reads it from its file, and turning
//! it off needs no administrator rights.
//!
//! A task hands administrator rights to whatever it starts, so it is only set up for
//! a camless in Program Files, where the setup puts it and only administrators write.
//! Setting it up takes a copy of the app with those rights, `camless-gui.exe
//! --set-up-task`, started through UAC: it writes the task's definition itself, in
//! its own folder, so no other program of the user's has a hand in what the task runs.

use crate::service::{self, Elevate};
use anyhow::{anyhow, bail, Context, Result};
use camless_core::config::is_inside;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Passed by the task: start the service and exit.
pub const START_SERVICE: &str = "--start-service";
/// Passed to the copy that runs as administrator: set the task up and exit.
pub const SET_UP: &str = "--set-up-task";

#[derive(Clone, Debug, Default)]
pub struct State {
    /// While it is being set up or removed; Windows may be asking the user.
    pub busy: bool,
    /// What went wrong the last time.
    pub error: Option<String>,
}

static STATE: Mutex<State> = Mutex::new(State { busy: false, error: None });

pub fn state() -> State {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Sets the task up or removes it in the background; does nothing while that runs.
pub fn spawn(on: bool) -> Result<()> {
    {
        let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if s.busy {
            bail!("Windows is still being asked");
        }
        *s = State { busy: true, error: None };
    }
    std::thread::Builder::new().name("task".into()).spawn(move || {
        let result = if on { set_up() } else { remove() };
        let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
        *s = State { busy: false, error: result.err().map(|e| format!("{e:#}")) };
    })?;
    Ok(())
}

/// Whether the task is set up and starts the camless next to this app, not another copy.
/// On a standard account it would start the service without administrator rights, so
/// there UAC asks as before, and so it does for a camless outside Program Files.
pub fn ready() -> bool {
    if !administrator() {
        return false;
    }
    let (Ok(name), Ok(exe)) = (name(), std::env::current_exe()) else { return false };
    if !in_program_files(&exe) {
        return false;
    }
    let Ok(file) = std::fs::read(system32().join("Tasks").join(name)) else { return false };
    command(&file).is_some_and(|c| c.to_lowercase() == exe.display().to_string().to_lowercase())
}

/// Starts the service through the task. Returns once Windows has the task running.
pub fn run() -> Result<()> {
    schtasks(&["/Run", "/TN", &name()?])
}

fn set_up() -> Result<()> {
    if !administrator() {
        bail!("this account is not an administrator, so the task could only start the service without administrator rights");
    }
    let exe = std::env::current_exe()?;
    if !in_program_files(&exe) {
        bail!("{NOT_INSTALLED}");
    }
    match service::run_elevated(&exe, SET_UP, true) {
        Ok(Some(0)) => {}
        Ok(_) => bail!("camless could not set the task up"),
        Err(Elevate::Declined) => bail!("Windows did not give administrator rights, so nothing changed"),
        Err(Elevate::Failed(e)) => return Err(e),
    }
    if !ready() {
        bail!("Windows set the task up, but camless cannot read it, so it keeps asking");
    }
    Ok(())
}

/// Why the task is refused for a camless outside Program Files.
const NOT_INSTALLED: &str = "this camless is not in Program Files, so other programs could replace it and get administrator rights through the task; install it with camless-setup.exe";

/// `--set-up-task`, with administrator rights: writes the task for this user and this
/// exe. The setup runs it too, after it moved camless, so the task follows.
pub fn set_up_elevated() -> Result<()> {
    if !elevated_now() {
        bail!("setting the task up needs administrator rights");
    }
    let exe = std::env::current_exe()?;
    if !in_program_files(&exe) {
        bail!("{NOT_INSTALLED}");
    }
    let xml = definition(&user_sid()?, &exe);
    // Next to the exe, which only administrators write: a file in the user's temp
    // folder could be swapped for another task before schtasks reads it.
    let file = exe.with_file_name(format!("camless-task-{}.xml", std::process::id()));
    // schtasks reads UTF-16 with a byte order mark.
    let bytes: Vec<u8> = [0xFF, 0xFE].into_iter().chain(xml.encode_utf16().flat_map(u16::to_le_bytes)).collect();
    std::fs::write(&file, bytes).context("writing the task for schtasks")?;
    let created = schtasks(&["/Create", "/TN", &name()?, "/XML", &file.display().to_string(), "/F"]);
    let _ = std::fs::remove_file(&file);
    created
}

fn remove() -> Result<()> {
    let name = name()?;
    if schtasks(&["/Delete", "/TN", &name, "/F"]).is_ok() {
        return Ok(());
    }
    match std::fs::metadata(system32().join("Tasks").join(&name)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        // A task set up some other way may let only administrators delete it.
        _ => elevated(&format!("/Delete /TN \"{name}\" /F")),
    }
}

/// `camless (<user>)`: the task runs as one user, so each has their own. The
/// uninstaller deletes it by this name.
fn name() -> Result<String> {
    use windows_sys::Win32::System::WindowsProgramming::GetUserNameW;
    let mut buf = [0u16; 257];
    let mut len = buf.len() as u32;
    if unsafe { GetUserNameW(buf.as_mut_ptr(), &mut len) } == 0 || len == 0 {
        bail!("reading the user name: {}", std::io::Error::last_os_error());
    }
    // `len` counts the terminating null.
    Ok(format!("camless ({})", String::from_utf16_lossy(&buf[..len as usize - 1])))
}

/// Whether this user is an administrator: running as one, or with the rights UAC can
/// give. A standard user who types an administrator's password at the prompt is not;
/// a task that runs as them gets only their own rights.
fn administrator() -> bool {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenElevation, TokenElevationType, TokenElevationTypeFull, TokenElevationTypeLimited, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let (mut kind, mut elevated, mut len) = (0i32, TOKEN_ELEVATION { TokenIsElevated: 0 }, 0);
        let typed = GetTokenInformation(token, TokenElevationType, &mut kind as *mut _ as *mut c_void, size_of::<i32>() as u32, &mut len) != 0;
        let read = GetTokenInformation(token, TokenElevation, &mut elevated as *mut _ as *mut c_void, size_of::<TOKEN_ELEVATION>() as u32, &mut len) != 0;
        CloseHandle(token);
        (typed && (kind == TokenElevationTypeLimited || kind == TokenElevationTypeFull)) || (read && elevated.TokenIsElevated != 0)
    }
}

/// Whether this process has administrator rights now.
fn elevated_now() -> bool {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let (mut elevated, mut len) = (TOKEN_ELEVATION { TokenIsElevated: 0 }, 0);
        let read = GetTokenInformation(token, TokenElevation, &mut elevated as *mut _ as *mut c_void, size_of::<TOKEN_ELEVATION>() as u32, &mut len) != 0;
        CloseHandle(token);
        read && elevated.TokenIsElevated != 0
    }
}

/// Whether `exe` is in Program Files, which takes administrator rights to write.
/// Windows names the folder in a part of the registry that takes them too; a variable
/// such as `%ProgramFiles%` is whatever the user's programs set it to.
fn in_program_files(exe: &Path) -> bool {
    program_files().is_some_and(|dir| is_inside(exe, &dir))
}

fn program_files() -> Option<PathBuf> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (key, value) = (wide(r"SOFTWARE\Microsoft\Windows\CurrentVersion"), wide("ProgramFilesDir"));
    let mut buf = [0u16; 520];
    let mut len = size_of_val(&buf) as u32;
    let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len) };
    if r != ERROR_SUCCESS {
        return None;
    }
    let text = String::from_utf16_lossy(&buf[..(len as usize / 2).min(buf.len())]);
    let text = text.trim_end_matches('\0');
    (!text.is_empty()).then(|| PathBuf::from(text))
}

/// The SID of the user this app runs as, such as `S-1-5-21-…-1001`.
fn user_sid() -> Result<String> {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            bail!("reading this app's user: {}", std::io::Error::last_os_error());
        }
        // TOKEN_USER is followed by the SID it points to; u64s keep it aligned.
        let mut buf = [0u64; 32];
        let mut len = 0;
        let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr() as *mut c_void, size_of_val(&buf) as u32, &mut len) != 0;
        let err = std::io::Error::last_os_error();
        CloseHandle(token);
        if !ok {
            bail!("reading this app's user: {err}");
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut text = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            bail!("reading this app's user: {}", std::io::Error::last_os_error());
        }
        let len = (0..).take_while(|&i| *text.add(i) != 0).count();
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text as *mut c_void);
        Ok(sid)
    }
}

/// The task: run by `user` when the app asks, with their full rights, at normal priority
/// and for as long as it takes. Only administrators may change it; `user` may read, run
/// and delete it.
fn definition(user: &str, exe: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>camless</Author>
    <Description>Starts the camless service as administrator without a UAC prompt when the camless app starts it. Turn it off in camless, under Settings and Service.</Description>
    <SecurityDescriptor>D:(A;;FA;;;SY)(A;;FA;;;BA)(A;;FRFXSD;;;{user})</SecurityDescriptor>
  </RegistrationInfo>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>5</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>{START_SERVICE}</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
        exe = escape(&exe.display().to_string())
    )
}

/// The program a task runs: `<Command>` in its file, which is UTF-16 XML. schtasks
/// would print it in the console's code page, which loses letters such as ł.
fn command(file: &[u8]) -> Option<String> {
    let units: Vec<u16> = file.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let xml = String::from_utf16_lossy(&units);
    let start = xml.find("<Command>")? + "<Command>".len();
    let end = start + xml[start..].find("</Command>")?;
    Some(unescape(xml[start..end].trim()).trim_matches('"').to_string())
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// Windows' System32, as Windows names it: this also runs as administrator, and a
/// variable such as `%SystemRoot%` is whatever the user's programs set it to.
fn system32() -> PathBuf {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut buf = [0u16; 260];
    let len = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if (1..buf.len()).contains(&len) {
        PathBuf::from(std::ffi::OsString::from_wide(&buf[..len]))
    } else {
        PathBuf::from(r"C:\Windows\System32")
    }
}

/// schtasks with this app's rights, without a window.
fn schtasks(args: &[&str]) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new(system32().join("schtasks.exe"))
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .output()
        .context("running schtasks")?;
    if !out.status.success() {
        bail!("schtasks {}: {}", args[0], String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// schtasks as administrator, through UAC; waits for it. Its parameters name the task
/// only, never a file.
fn elevated(params: &str) -> Result<()> {
    match service::run_elevated(&system32().join("schtasks.exe"), params, true) {
        Ok(Some(0)) => Ok(()),
        Ok(code) => Err(anyhow!("schtasks failed with code {}", code.unwrap_or(1))),
        Err(Elevate::Declined) => bail!("Windows did not give administrator rights, so nothing changed"),
        Err(Elevate::Failed(e)) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_is_read_back_from_the_file() {
        let exe = Path::new(r"C:\Users\Zażółć & 'gęślą'\AppData\Local\Programs\camless\camless-gui.exe");
        let xml = definition("S-1-5-21-1-2-3-1001", exe);
        let file: Vec<u8> = [0xFF, 0xFE].into_iter().chain(xml.encode_utf16().flat_map(u16::to_le_bytes)).collect();
        assert_eq!(command(&file).as_deref(), Some(exe.to_str().unwrap()));
        assert!(xml.contains("&amp; &apos;g"));
    }

    #[test]
    fn the_task_belongs_to_this_user() {
        assert_eq!(name().unwrap(), format!("camless ({})", std::env::var("USERNAME").unwrap()));
        assert!(user_sid().unwrap().starts_with("S-1-5-"), "{}", user_sid().unwrap());
    }

    #[test]
    fn windows_says_where_program_files_is() {
        // And System32 is not in it.
        let dir = program_files().expect("ProgramFilesDir");
        assert!(dir.is_absolute(), "{}", dir.display());
        assert!(!is_inside(&system32().join("schtasks.exe"), &dir));
        assert!(system32().is_absolute());
    }

    #[test]
    fn a_file_without_a_command_runs_nothing() {
        assert_eq!(command(&[]), None);
        assert_eq!(command(&"<Task></Task>".encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>()), None);
    }
}
