//! Where the service keeps what it writes: the history, the game sessions and the
//! marker for the GPU fans it has taken.
//!
//! That is the config's folder, except for the installed service on Windows while it
//! runs as administrator: there it is `data` in camless's own folder in Program
//! Files, which only administrators write. In a folder of the user's, any program
//! they run could put a link in place of `history`, and the service would write,
//! with administrator rights, wherever the link leads.

use crate::{games, history};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// A file larger than this is not the service's own.
const LARGEST: u64 = 64 << 20;

/// Settles the folder for the service that runs the config at `config`. Without a
/// call, [`dir`] goes by the default config.
pub fn init(config: &Path) -> &'static Path {
    DIR.get_or_init(|| resolve(config))
}

pub fn dir() -> &'static Path {
    init(&camless_core::config::default_path())
}

fn resolve(config: &Path) -> PathBuf {
    let beside = config.parent().map(Path::to_path_buf).unwrap_or_default();
    own().map_or(beside, Path::to_path_buf)
}

/// The installed service's folder for its files, while it runs as administrator.
pub fn own() -> Option<&'static Path> {
    static OWN: OnceLock<Option<PathBuf>> = OnceLock::new();
    OWN.get_or_init(|| own_folder().map(|folder| folder.join("data"))).as_deref()
}

/// The folder camless.exe is in, when that is in Program Files and the service has
/// administrator rights. Windows names Program Files in a part of the registry that
/// takes those rights to change; `%ProgramFiles%` is whatever the user's programs set.
#[cfg(windows)]
fn own_folder() -> Option<PathBuf> {
    if !crate::web::is_elevated() {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let program_files = crate::sensors::reg_string(r"SOFTWARE\Microsoft\Windows\CurrentVersion", "ProgramFilesDir")?;
    camless_core::config::is_inside(&exe, Path::new(&program_files)).then(|| exe.parent().map(Path::to_path_buf))?
}

#[cfg(not(windows))]
fn own_folder() -> Option<PathBuf> {
    None
}

/// Brings the history kept beside the config at `config` to [`dir`], once: what the
/// service wrote before it had a folder of its own, or while it ran without
/// administrator rights. The old files stay where they are.
pub fn bring_history_over(config: &Path) {
    let Some(beside) = config.parent() else { return };
    if beside != dir() {
        bring_over(&beside.join("history"), &dir().join("history"));
    }
}

/// Only the service's own files, and of each only the lines that are its own. `from`
/// is a folder of the user's, and this may run as administrator: a link there could
/// name a file the user may not read, and a copy of it would land where they may.
fn bring_over(from: &Path, to: &Path) {
    if to.exists() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(from) else { return };
    // Filled beside `to` and renamed at the end: a camless that stops halfway leaves
    // no `to`, and the next one starts again.
    let part = to.with_extension("part");
    let _ = std::fs::remove_dir_all(&part);
    if let Err(e) = std::fs::create_dir_all(&part) {
        eprintln!("history: {} not made, so the old history stays in {}: {e}", part.display(), from.display());
        return;
    }
    let mut brought = 0;
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        let clean: fn(&str) -> String = match name {
            games::FILE => games::clean,
            _ if history::is_day_file(name) => history::clean,
            _ => continue,
        };
        if !e.metadata().is_ok_and(|m| m.file_type().is_file() && m.len() <= LARGEST) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
        if std::fs::write(part.join(name), clean(&text)).is_ok() {
            brought += 1;
        }
    }
    if let Err(e) = std::fs::rename(&part, to) {
        eprintln!("history: {} not made, so the old history stays in {}: {e}", to.display(), from.display());
    } else if brought > 0 {
        eprintln!("history: {brought} files brought over from {} to {}", from.display(), to.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_services_own_lines_come_over() {
        let root = std::env::temp_dir().join(format!("camless-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (from, to) = (root.join("old").join("history"), root.join("new").join("history"));
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join("2026-10-20.csv"), "#t,cpu.temp\n1760918400,40,41.5\nroot:x:0:0:secret\n1760918460,,\n1760918520,4").unwrap();
        let session = r#"{"game":"Half-Life","exe":"hl.exe","start":1,"end":2,"played":1,"frames":0}"#;
        std::fs::write(from.join(games::FILE), format!("{session}\nnot json\n")).unwrap();
        std::fs::write(from.join("notes.txt"), "mine").unwrap();
        std::fs::create_dir_all(from.join("2026-10-21.csv")).unwrap();
        // What a camless that stopped halfway left.
        let part = root.join("new").join("history.part");
        std::fs::create_dir_all(&part).unwrap();
        std::fs::write(part.join("2026-10-19.csv"), "#t\n").unwrap();

        bring_over(&from, &to);
        assert!(!part.exists() && !to.join("2026-10-19.csv").exists());
        assert_eq!(std::fs::read_to_string(to.join("2026-10-20.csv")).unwrap(), "#t,cpu.temp\n1760918400,40,41.5\n1760918460,,\n1760918520,4\n");
        let sessions = std::fs::read_to_string(to.join(games::FILE)).unwrap();
        assert_eq!(sessions.lines().count(), 1);
        assert!(sessions.contains("Half-Life") && !sessions.contains("not json"), "{sessions}");
        assert!(!to.join("notes.txt").exists() && !to.join("2026-10-21.csv").exists());
        // The old files stay, and a second run leaves what the service has written since.
        assert!(from.join("2026-10-20.csv").exists());
        std::fs::write(to.join("2026-10-20.csv"), "#t\n").unwrap();
        bring_over(&from, &to);
        assert_eq!(std::fs::read_to_string(to.join("2026-10-20.csv")).unwrap(), "#t\n");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_folder_is_the_configs_without_an_install() {
        // The tests' own exe is not in Program Files, whatever rights it runs with.
        assert_eq!(resolve(Path::new("/etc/camless/camless.toml")), Path::new("/etc/camless"));
    }
}
