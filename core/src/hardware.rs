//! The hardware camless was run on and reported working: `hardware/verified.toml`
//! in the repository, built in.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::sync::OnceLock;

const FILE: &str = include_str!("../../hardware/verified.toml");
/// What a report can say was tried.
pub const CHECKS: &[&str] = &["pump", "fans", "lighting", "lcd"];

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verified {
    /// `vendor:product` in lowercase hex.
    pub usb: String,
    pub name: String,
    #[serde(default)]
    pub firmware: Vec<String>,
    pub checked: Vec<String>,
    pub source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    device: Vec<Verified>,
}

fn parse(text: &str) -> Result<Vec<Verified>> {
    let list = toml::from_str::<File>(text).context("hardware/verified.toml")?.device;
    for (i, d) in list.iter().enumerate() {
        if usb_id(&d.usb).is_none() {
            bail!("{}: usb {:?} is not vendor:product in lowercase hex", d.name, d.usb);
        }
        if list[..i].iter().any(|o| o.usb == d.usb) {
            bail!("{} is listed twice", d.usb);
        }
        if let Some(c) = d.checked.iter().find(|c| !CHECKS.contains(&c.as_str())) {
            bail!("{}: checked {c:?} is not one of {}", d.name, CHECKS.join(", "));
        }
    }
    Ok(list)
}

fn usb_id(s: &str) -> Option<(u16, u16)> {
    let (v, p) = s.split_once(':')?;
    let hex = |x: &str| (x.len() == 4 && x.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))).then(|| u16::from_str_radix(x, 16).ok()).flatten();
    Some((hex(v)?, hex(p)?))
}

/// Every verified device. The file is checked by the tests, so it parses.
pub fn list() -> &'static [Verified] {
    static LIST: OnceLock<Vec<Verified>> = OnceLock::new();
    LIST.get_or_init(|| parse(FILE).unwrap_or_default())
}

/// Whether someone ran camless on this device and reported it working.
pub fn is_verified(vid: u16, pid: u16) -> bool {
    list().iter().any(|d| usb_id(&d.usb) == Some((vid, pid)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_in_the_repository_reads() {
        let list = parse(FILE).unwrap();
        assert!(!list.is_empty());
        assert!(is_verified(0x1e71, 0x3012));
        assert!(is_verified(0x1e71, 0x300e));
        assert!(!is_verified(0x1e71, 0x2007));
    }

    #[test]
    fn mistakes_in_the_list_are_caught() {
        let entry = |usb: &str, checked: &str| format!("[[device]]\nusb = \"{usb}\"\nname = \"X\"\nchecked = [\"{checked}\"]\nsource = \"#1\"\n");
        assert!(parse(&entry("1e71:2007", "pump")).is_ok());
        assert!(parse(&entry("1E71:2007", "pump")).is_err());
        assert!(parse(&entry("1e71-2007", "pump")).is_err());
        assert!(parse(&entry("1e71:2007", "rgb")).is_err());
        assert!(parse(&(entry("1e71:2007", "pump") + &entry("1e71:2007", "fans"))).is_err());
        assert!(parse("[[device]]\nusb = \"1e71:2007\"\nname = \"X\"\nchecked = []\nsource = \"#1\"\nworks = true\n").is_err());
    }
}
