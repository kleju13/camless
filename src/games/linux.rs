//! Processes on Linux, for game sessions. There is no window in front to ask about
//! on Wayland, so a game counts as in front while it runs.
//!
//! A program is where its executable is, or for a Windows game under Proton or Wine,
//! the Windows path it was started with: Proton starts a Steam game as
//! `Z:\home\…\steamapps\common\<game>\game.exe`, and Wine's `Z:` drive is `/`.

/// A process, told apart from a later one with the same id by when it started.
pub struct Process {
    pid: u32,
    started: Option<u64>,
}

impl Process {
    pub fn open(pid: u32) -> Option<Process> {
        let started = started(pid);
        (pid != 0 && started.is_some()).then_some(Process { pid, started })
    }

    pub fn exited(&self) -> bool {
        started(self.pid) != self.started
    }

    pub fn path(&self) -> Option<String> {
        program_path(self.pid)
    }
}

/// Field 22 of `/proc/<pid>/stat`, the start time in clock ticks after boot. The name
/// before it is in parentheses and can hold spaces and parentheses itself.
fn started(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()
}

/// The program process `pid` runs: its Windows path for a Wine program, else its
/// executable. Other users' executables cannot be read, and are no one's game here.
fn program_path(pid: u32) -> Option<String> {
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let argv0 = cmdline.split(|&b| b == 0).next().map(String::from_utf8_lossy).unwrap_or_default();
    if let Some(path) = wine_path(&argv0) {
        return Some(path);
    }
    std::fs::read_link(format!("/proc/{pid}/exe")).ok().map(|p| p.to_string_lossy().into_owned())
}

/// A Windows path on Wine's `Z:` drive as the Linux path it is; any other Windows
/// path as it is, so `[games] add` can still name its file.
pub fn wine_path(argv0: &str) -> Option<String> {
    let b = argv0.as_bytes();
    if b.len() < 3 || !b[0].is_ascii_alphabetic() || b[1] != b':' || b[2] != b'\\' {
        return None;
    }
    if b[0].eq_ignore_ascii_case(&b'z') {
        return Some(argv0[2..].replace('\\', "/"));
    }
    Some(argv0.to_string())
}

/// Every process with the program it runs, in process id order.
pub fn processes() -> Vec<(u32, String)> {
    let mut pids: Vec<u32> = std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse().ok())
        .collect();
    pids.sort_unstable();
    let me = std::process::id();
    pids.into_iter().filter(|&p| p != me).filter_map(|p| Some((p, program_path(p)?))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proton_games_are_found_by_their_windows_path() {
        assert_eq!(
            wine_path(r"Z:\home\a\.local\share\Steam\steamapps\common\Cyberpunk 2077\bin\x64\Cyberpunk2077.exe").as_deref(),
            Some("/home/a/.local/share/Steam/steamapps/common/Cyberpunk 2077/bin/x64/Cyberpunk2077.exe")
        );
        assert_eq!(wine_path(r"C:\windows\system32\services.exe").as_deref(), Some(r"C:\windows\system32\services.exe"));
        assert_eq!(wine_path("/usr/bin/bash"), None);
        assert_eq!(wine_path("Z:"), None);
    }

    #[test]
    fn this_process_has_a_start_time_and_an_executable() {
        let me = Process::open(std::process::id()).unwrap();
        assert!(!me.exited());
        let exe = std::env::current_exe().unwrap();
        assert_eq!(me.path().as_deref(), Some(exe.to_string_lossy().as_ref()));
        assert!(Process::open(0).is_none());
        assert!(!processes().is_empty());
    }
}
