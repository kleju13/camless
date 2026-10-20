//! Other brands, and their apps. While a brand's own app runs, camless leaves its
//! devices to it: two programs writing one hub fight over its fans. Nexus does the
//! same, by process name (its `ConflictAppDefinitions.cs`).

use std::collections::HashSet;
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};

/// A brand other than NZXT, and its own app.
pub struct Vendor {
    pub name: &'static str,
    pub app: &'static str,
    /// The app's executables, in lower case. Any of them running is the app running.
    pub processes: &'static [&'static str],
}

impl Vendor {
    /// The app among `running`, from `running_processes()`.
    pub fn app_runs(&self, running: &HashSet<String>) -> bool {
        self.processes.iter().any(|p| running.contains(*p))
    }

    /// Why camless leaves the device alone now: its app runs.
    pub fn busy(&self, running: &HashSet<String>) -> Option<String> {
        self.app_runs(running).then(|| format!("{} is running; camless leaves the device to it", self.app))
    }
}

/// The executable names of every process, in lower case. Empty if Windows does not
/// list them.
pub fn running_processes() -> HashSet<String> {
    let mut names = HashSet::new();
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return names;
    }
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut more = unsafe { Process32FirstW(snap, &mut entry) } != 0;
    while more {
        let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
        names.insert(String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase());
        more = unsafe { Process32NextW(snap, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snap) };
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_its_own_process_and_matches_apps_in_lower_case() {
        let running = running_processes();
        let me = std::env::current_exe().unwrap();
        let me = me.file_name().unwrap().to_string_lossy().to_lowercase();
        assert!(running.contains(&me), "{me} not among {} processes", running.len());
        let app = Vendor { name: "Test", app: "Test app", processes: &["test app.exe"] };
        assert!(app.app_runs(&HashSet::from(["test app.exe".to_string()])));
        assert!(!app.app_runs(&HashSet::from(["other.exe".to_string()])));
        assert_eq!(app.busy(&HashSet::new()), None);
    }
}
