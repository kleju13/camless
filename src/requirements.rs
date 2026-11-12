//! Software camless uses that Windows or a Linux desktop may not come with: what it
//! is for, whether something on this PC needs it, and whether it is there. The
//! service checks again every few seconds, so the app sees an install without a
//! restart, and CPU temperature starts once PawnIO and its module are in place.

use camless_core::config::{Config, LcdMode};
use camless_core::status::Requirement;
#[cfg(windows)]
use std::path::Path;

/// What camless needs from elsewhere on this PC, in the order the app lists it.
pub fn check(cfg: &Config) -> Vec<Requirement> {
    let web_lcd = cfg.lcds.iter().any(|l| l.mode == LcdMode::Web);
    let browser = browser(web_lcd, crate::web::browser_name().is_some());
    #[cfg(windows)]
    {
        let module = crate::sensors::pawnio_module_path(cfg.pawnio_module.as_ref());
        let cpu = crate::sensors::cpu_name();
        vec![
            pawnio(Path::new(crate::sensors::PAWNIO_LIB).is_file()),
            pawnio_module(
                crate::sensors::cpu_maker(),
                cpu.as_deref(),
                crate::sensors::pawnio_module_file(),
                module.is_file(),
                cfg.pawnio_module.as_deref(),
            ),
            browser,
        ]
    }
    #[cfg(not(windows))]
    vec![browser]
}

/// The driver Windows needs before a program can read the CPU's temperature.
/// LibreHardwareMonitor and FanControl install the same one.
#[cfg_attr(not(windows), allow(dead_code))]
fn pawnio(installed: bool) -> Requirement {
    Requirement {
        id: "pawnio".into(),
        name: "PawnIO".into(),
        used_for: "CPU temperature and power".into(),
        needed_because: Some("Windows lets programs read the CPU’s sensors only through a driver".into()),
        installed,
        how: "Install it with the setup from pawnio.eu. FanControl and LibreHardwareMonitor use the same driver, so with one of them it is already there.".into(),
        link: Some("pawnio".into()),
        in_app_folder: false,
    }
}

/// The signed module PawnIO runs for this maker's CPU. `configured` is the path in
/// camless.toml, when it names one.
#[cfg_attr(not(windows), allow(dead_code))]
fn pawnio_module(maker: &str, cpu: Option<&str>, file: &str, installed: bool, configured: Option<&std::path::Path>) -> Requirement {
    let found = match cpu {
        Some(name) if !name.is_empty() => format!("camless found an {maker} CPU, {name}"),
        _ => format!("camless found an {maker} CPU"),
    };
    let how = match configured {
        Some(path) => format!(
            "It is in the zip of the latest PawnIO.Modules release. camless.toml says to read it from {}.",
            path.display()
        ),
        None => format!("It is in the zip of the latest PawnIO.Modules release. Copy {file} from it into camless’s folder."),
    };
    Requirement {
        id: "pawnio-module".into(),
        name: file.into(),
        used_for: format!("CPU temperature and power on {maker} CPUs, through PawnIO"),
        needed_because: Some(found),
        installed,
        how,
        link: Some("pawnio-modules".into()),
        in_app_folder: configured.is_none(),
    }
}

/// A Chromium browser, which renders the web pages camless shows on an LCD.
fn browser(needed: bool, installed: bool) -> Requirement {
    let (name, how) = if cfg!(windows) {
        ("Microsoft Edge or Google Chrome", "Windows comes with Microsoft Edge. If it was removed, install Edge or Chrome again.")
    } else {
        ("Google Chrome, Chromium, Microsoft Edge or Brave", "Install one from your distribution’s packages or from the browser’s site.")
    };
    Requirement {
        id: "browser".into(),
        name: name.into(),
        used_for: "Web pages on the LCD".into(),
        needed_because: needed.then(|| "An LCD is set to show a web page".into()),
        installed,
        how: how.into(),
        link: None,
        in_app_folder: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn pawnio_is_needed_on_every_windows_pc() {
        assert!(pawnio(false).missing());
        assert!(!pawnio(true).missing());
    }

    #[test]
    fn the_module_says_which_cpu_needs_it() {
        let r = pawnio_module("Intel", Some("Intel(R) Core(TM) i7-14700K"), "IntelMSR.bin", false, None);
        assert_eq!(r.name, "IntelMSR.bin");
        assert!(r.missing());
        assert!(r.in_app_folder);
        assert!(r.needed_because.unwrap().contains("Intel CPU, Intel(R) Core(TM) i7-14700K"));
        assert!(r.how.contains("Copy IntelMSR.bin"));
        let r = pawnio_module("AMD", None, "AMDFamily17.bin", true, None);
        assert_eq!(r.needed_because.as_deref(), Some("camless found an AMD CPU"));
        assert!(!r.missing());
    }

    #[test]
    fn a_module_from_the_config_is_not_in_the_app_folder() {
        let r = pawnio_module("AMD", None, "AMDFamily17.bin", false, Some(Path::new(r"D:\pawn\AMDFamily17.bin")));
        assert!(!r.in_app_folder);
        assert!(r.how.contains(r"D:\pawn\AMDFamily17.bin"));
    }

    #[test]
    fn a_browser_is_needed_only_for_a_web_page() {
        assert!(!browser(false, false).missing());
        assert!(browser(false, false).needed_because.is_none());
        assert!(browser(true, false).missing());
        assert!(!browser(true, true).missing());
    }

    #[test]
    fn every_link_is_one_the_app_opens() {
        // gui/src/backend.rs `link` names the same pages.
        for r in [pawnio(false), pawnio_module("Intel", None, "IntelMSR.bin", false, None), browser(true, false)] {
            if let Some(l) = r.link.as_deref() {
                assert!(["pawnio", "pawnio-modules"].contains(&l), "{l}");
            }
        }
    }
}
