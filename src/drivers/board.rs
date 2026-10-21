//! The motherboard's fan headers, with `[devices] motherboard` on: a CPU fan, case
//! fans, or a pump that takes its speed from a header, as most AIOs without software
//! do. The BIOS drives every header until camless gives it a curve, and gets it back
//! when camless lets go or stops.
//!
//! On Linux the kernel's Super I/O drivers (`nct6775`, `it87`, `f71882fg`,
//! `w83627ehf`) do the chip's work, and camless writes their hwmon files, as Nexus's
//! `LinuxFanControlProvider` and lm-sensors' `fancontrol` do: `pwmN_enable` 1 for a
//! duty of its own, `pwmN` 0..255, `fanN_input` in rpm. Back to the BIOS is the
//! `pwmN_enable` the header had before. Windows needs the chip's registers, through
//! PawnIO, which camless does not drive yet.
//!
//! Experimental: nobody has run camless on a motherboard yet.

use super::{Bus, Driver, Found, Model, Vendor};
use anyhow::Result;
use hidapi::HidApi;

pub const MODEL: Model =
    Model { bus: Bus::Host, experimental: true, vendor: Some(&FAN_APPS), ..Model::hid(0, 0, "motherboard", "Motherboard", open) };

/// Other programs that set the headers. While one runs, camless leaves them to it, as
/// it leaves another brand's hub to that brand's app.
pub static FAN_APPS: Vendor = Vendor {
    name: "Fan control",
    app: "Another fan control program (FanControl, LibreHardwareMonitor, CoolerControl)",
    processes: &[
        "fancontrol.exe",
        "librehardwaremonitor.exe",
        "openhardwaremonitor.exe",
        "argusmonitor.exe",
        "speedfan.exe",
        "coolercontrold",
        "thinkfan",
    ],
};

/// A duty as the chip's 0..255.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn raw(duty: u8) -> u8 {
    ((u32::from(duty.min(100)) * 255 + 50) / 100) as u8
}

/// The chip's 0..255 as a duty.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn duty(raw: u8) -> u8 {
    ((u32::from(raw) * 100 + 127) / 255) as u8
}

/// The motherboard's chips with fan headers camless can set.
pub fn found() -> Vec<Found> {
    #[cfg(target_os = "linux")]
    return os::found(std::path::Path::new("/sys/class/hwmon"));
    #[cfg(not(target_os = "linux"))]
    return Vec::new();
}

fn open(_api: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    #[cfg(target_os = "linux")]
    return Ok(Box::new(os::Board::open(found)?));
    #[cfg(not(target_os = "linux"))]
    {
        let _ = found;
        anyhow::bail!("camless sets motherboard fan headers on Linux only, so far")
    }
}

#[cfg(target_os = "linux")]
mod os {
    use super::{duty, raw, MODEL};
    use crate::drivers::{Driver, FanReading, Found, Reading};
    use anyhow::{bail, Context, Result};
    use camless_core::lighting::Lighting;
    use camless_core::status::{Device, FanChannel, Unit};
    use std::ffi::CString;
    use std::fs;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    /// The `name` of the hwmon drivers for Super I/O chips, by prefix: Nuvoton and
    /// Winbond (`nct6775`, `nct6798`, `w83627ehf`), ITE (`it87`, `it8688`) and Fintek
    /// (`f71882fg`, `f71869`, `f81866a`). Not the GPU's or a USB device's hwmon, which
    /// have their own drivers in camless or none.
    pub const CHIPS: [&str; 5] = ["nct6", "w83", "it8", "f71", "f8"];

    pub fn is_super_io(name: &str) -> bool {
        CHIPS.iter().any(|c| name.starts_with(c))
    }

    fn read(path: &Path) -> Option<String> {
        fs::read_to_string(path).ok().map(|s| s.trim().to_string())
    }

    /// The hwmon folders of Super I/O chips with at least one header camless can set,
    /// in the order of the chips' own devices, which does not change between boots
    /// as `hwmonN` can.
    pub fn found(root: &Path) -> Vec<Found> {
        let mut chips: Vec<(PathBuf, PathBuf)> = fs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|dir| read(&dir.join("name")).is_some_and(|n| is_super_io(&n)))
            // A chip on the platform bus: what the Super I/O drivers register.
            .filter(|dir| {
                fs::read_link(dir.join("device").join("subsystem"))
                    .is_ok_and(|s| s.file_name().is_some_and(|n| n == "platform"))
            })
            .filter(|dir| !headers(dir).is_empty())
            .map(|dir| (fs::canonicalize(dir.join("device")).unwrap_or_else(|_| dir.clone()), dir))
            .collect();
        chips.sort();
        let mut out = Vec::new();
        for (i, (_, dir)) in chips.into_iter().enumerate() {
            let id = if i == 0 { MODEL.slug.to_string() } else { format!("{}#{}", MODEL.slug, i + 1) };
            let Ok(path) = CString::new(dir.as_os_str().as_bytes()) else { continue };
            out.push(Found { model: &MODEL, id, path });
        }
        out
    }

    /// The headers of one chip: `pwmN` with its `pwmN_enable`, as Nexus counts them.
    fn headers(dir: &Path) -> Vec<u8> {
        let mut n: Vec<u8> = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_prefix("pwm")?.parse::<u8>().ok())
            .filter(|n| dir.join(format!("pwm{n}_enable")).exists())
            .collect();
        n.sort();
        n
    }

    struct Header {
        n: u8,
        id: &'static str,
        name: String,
        pwm: PathBuf,
        enable: PathBuf,
        rpm: PathBuf,
        /// `pwmN_enable` and `pwmN` before camless took the header; None while the
        /// BIOS has it.
        before: Option<(String, String)>,
    }

    pub struct Board {
        name: String,
        headers: Vec<Header>,
    }

    fn write(path: &Path, value: &str) -> Result<()> {
        fs::write(path, value).with_context(|| format!("writing {value} to {}", path.display()))
    }

    impl Board {
        pub fn open(found: &Found) -> Result<Board> {
            let dir = PathBuf::from(std::ffi::OsStr::from_bytes(found.path.as_bytes()));
            let chip = read(&dir.join("name")).context("the chip is gone")?;
            let mut headers = Vec::new();
            for n in self::headers(&dir) {
                let label = read(&dir.join(format!("fan{n}_label")));
                headers.push(Header {
                    n,
                    id: crate::drivers::static_id(&format!("fan{n}")),
                    name: label.unwrap_or_else(|| format!("Fan {n}")),
                    pwm: dir.join(format!("pwm{n}")),
                    enable: dir.join(format!("pwm{n}_enable")),
                    rpm: dir.join(format!("fan{n}_input")),
                    before: None,
                });
            }
            // Opening for writing writes nothing; it fails without the udev rule.
            if let Some(h) = headers.first() {
                fs::OpenOptions::new().write(true).open(&h.enable).with_context(|| {
                    format!("no write access to {}; install.sh's udev rules give it (/etc/udev/rules.d/72-camless-fans.rules), and apply after a restart", h.enable.display())
                })?;
            }
            let dmi = |f: &str| read(&Path::new("/sys/class/dmi/id").join(f)).filter(|s| !s.is_empty());
            let board = match (dmi("board_vendor"), dmi("board_name")) {
                (Some(v), Some(n)) => format!("{v} {n}"),
                (None, Some(n)) => n,
                _ => MODEL.name.to_string(),
            };
            Ok(Board { name: format!("{board} ({chip})"), headers })
        }

        /// Hands one header back to the BIOS: the mode it had, its duty too if that
        /// was manual. A chip that refuses the mode gets automatic (2), then full
        /// speed (0), as Nexus falls back.
        fn hand_back(h: &mut Header) -> Result<()> {
            let Some((enable, pwm)) = h.before.take() else { return Ok(()) };
            if enable == "1" {
                let _ = write(&h.pwm, &pwm);
            }
            write(&h.enable, &enable).or_else(|_| write(&h.enable, "2")).or_else(|_| write(&h.enable, "0"))
        }
    }

    impl Driver for Board {
        fn describe(&self) -> Device {
            Device {
                name: self.name.clone(),
                verified: false,
                experimental: MODEL.is_experimental(),
                fans: self
                    .headers
                    .iter()
                    .map(|h| FanChannel { id: h.id.into(), name: h.name.clone(), ..Default::default() })
                    .collect(),
                ..Default::default()
            }
        }

        fn sensors(&self) -> Vec<(&'static str, &'static str, Unit)> {
            Vec::new()
        }

        fn poll(&mut self) -> Result<Reading> {
            let mut fans = Vec::new();
            for h in &self.headers {
                let raw = read(&h.pwm).with_context(|| format!("reading {}", h.pwm.display()))?;
                let rpm = read(&h.rpm).and_then(|s| s.parse::<u16>().ok());
                let d = raw.parse::<u8>().ok().map(duty);
                fans.push((h.id, FanReading { rpm, duty: d, connected: None }));
            }
            Ok(Reading { sensors: Vec::new(), fans })
        }

        fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
            for &(fan, d) in duties {
                let Some(h) = self.headers.iter_mut().find(|h| h.id == fan) else { bail!("no fan {fan:?} on this motherboard") };
                if h.before.is_none() {
                    let enable = read(&h.enable).context("reading the header's mode")?;
                    let pwm = read(&h.pwm).unwrap_or_default();
                    h.before = Some((enable, pwm));
                    write(&h.enable, "1")?;
                    // A chip can refuse manual mode on a header, or the driver put it back.
                    let now = read(&h.enable).unwrap_or_default();
                    if now != "1" {
                        Board::hand_back(h)?;
                        bail!("the chip keeps pwm{}_enable at {now}, not 1: it does not let camless set this header", h.n);
                    }
                }
                write(&h.pwm, &raw(d).to_string())?;
            }
            Ok(())
        }

        fn set_lighting(&mut self, light: &str, _: &Lighting) -> Result<()> {
            bail!("no light {light:?} on the motherboard's fan headers")
        }

        fn release(&mut self) -> Result<()> {
            let mut first = Ok(());
            for h in &mut self.headers {
                let r = Board::hand_back(h);
                if first.is_ok() {
                    first = r;
                }
            }
            first
        }
    }

    /// Closing for any reason, a lost device or a crash of the service's thread
    /// included, gives the headers back.
    impl Drop for Board {
        fn drop(&mut self) {
            let _ = self.release();
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::symlink;

        /// A hwmon folder like the kernel's: name, the device's subsystem, headers.
        fn chip(root: &Path, hwmon: &str, name: &str, device: &str, subsystem: &str, pwms: &[u8]) -> PathBuf {
            let dev = root.join("devices").join(device);
            fs::create_dir_all(&dev).unwrap();
            let bus = root.join("bus").join(subsystem);
            fs::create_dir_all(&bus).unwrap();
            let _ = symlink(&bus, dev.join("subsystem"));
            let dir = root.join("class").join(hwmon);
            fs::create_dir_all(&dir).unwrap();
            symlink(&dev, dir.join("device")).unwrap();
            fs::write(dir.join("name"), format!("{name}\n")).unwrap();
            for n in pwms {
                fs::write(dir.join(format!("pwm{n}")), "128\n").unwrap();
                fs::write(dir.join(format!("pwm{n}_enable")), "5\n").unwrap();
                fs::write(dir.join(format!("fan{n}_input")), "900\n").unwrap();
            }
            dir
        }

        fn tmp(name: &str) -> PathBuf {
            let root = std::env::temp_dir().join(format!("camless-board-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            root
        }

        #[test]
        fn finds_super_io_chips_on_the_platform_bus_only() {
            let root = tmp("scan");
            let a = chip(&root, "hwmon3", "nct6798", "nct6775.656", "platform", &[1, 2, 7]);
            chip(&root, "hwmon1", "amdgpu", "0000:03:00.0", "pci", &[1]);
            chip(&root, "hwmon2", "nzxtkraken3", "0003:1E71:2007.0004", "hid", &[1, 2]);
            chip(&root, "hwmon4", "k10temp", "0000:00:18.3", "pci", &[]);
            let b = chip(&root, "hwmon0", "it8792", "it87.2592", "platform", &[1]);
            chip(&root, "hwmon5", "it8689", "it87.2656", "platform", &[]);
            let found = found(&root.join("class"));
            let got: Vec<_> = found.iter().map(|f| (f.id.clone(), PathBuf::from(f.path.to_str().unwrap()))).collect();
            // By the chips' devices: it87.2592 before nct6775.656, whatever hwmonN says.
            assert_eq!(got, [("motherboard".to_string(), b), ("motherboard#2".to_string(), a.clone())]);
            assert_eq!(headers(&a), [1, 2, 7]);
            fs::remove_dir_all(&root).unwrap();
        }

        #[test]
        fn takes_a_header_only_when_given_a_duty_and_gives_back_what_it_had() {
            let root = tmp("drive");
            let dir = chip(&root, "hwmon2", "nct6798", "nct6775.656", "platform", &[1, 2]);
            fs::write(dir.join("fan2_label"), "CPU_FAN\n").unwrap();
            let found = Found { model: &MODEL, id: "motherboard".into(), path: CString::new(dir.to_str().unwrap()).unwrap() };
            let mut b = Board::open(&found).unwrap();
            let names: Vec<_> = b.describe().fans.iter().map(|f| (f.id.clone(), f.name.clone())).collect();
            assert_eq!(names, [("fan1".to_string(), "Fan 1".to_string()), ("fan2".to_string(), "CPU_FAN".to_string())]);
            let r = b.poll().unwrap();
            assert_eq!((r.fans[0].1.rpm, r.fans[0].1.duty), (Some(900), Some(50)));
            b.set_duties(&[("fan2", 40)]).unwrap();
            assert_eq!(read(&dir.join("pwm2_enable")).unwrap(), "1");
            assert_eq!(read(&dir.join("pwm2")).unwrap(), "102");
            // fan1 has no curve: the BIOS keeps it.
            assert_eq!(read(&dir.join("pwm1_enable")).unwrap(), "5");
            assert!(b.set_duties(&[("fan3", 40)]).is_err());
            b.release().unwrap();
            assert_eq!(read(&dir.join("pwm2_enable")).unwrap(), "5");
            // A header the user had set by hand gets its duty back too.
            fs::write(dir.join("pwm1_enable"), "1\n").unwrap();
            fs::write(dir.join("pwm1"), "200\n").unwrap();
            b.set_duties(&[("fan1", 100)]).unwrap();
            assert_eq!(read(&dir.join("pwm1")).unwrap(), "255");
            drop(b);
            assert_eq!((read(&dir.join("pwm1_enable")).unwrap(), read(&dir.join("pwm1")).unwrap()), ("1".into(), "200".into()));
            fs::remove_dir_all(&root).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// linux/72-camless-fans.rules hands over a header's duty and mode, on the chips
    /// camless drives, and nothing else.
    #[test]
    fn the_fan_rule_names_the_chips_and_two_files_per_header() {
        let rules = include_str!("../../linux/72-camless-fans.rules");
        let lines: Vec<&str> = rules.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
        assert_eq!(lines.len(), 1);
        let line = lines[0];
        assert!(line.contains(r#"SUBSYSTEM=="hwmon""#));
        let names = line.split(r#"ATTR{name}==""#).nth(1).unwrap().split('"').next().unwrap();
        assert_eq!(names, "nct6*|w83*|it8*|f71*|f8*");
        #[cfg(target_os = "linux")]
        assert_eq!(names.split('|').map(|n| n.trim_end_matches('*')).collect::<Vec<_>>(), os::CHIPS);
        let files: Vec<&str> = line.split_whitespace().filter(|w| w.starts_with("/sys%p/")).collect();
        assert_eq!(files, ["/sys%p/pwm[1-9]", "/sys%p/pwm[1-9]_enable;"]);
        assert!(line.contains("chown @USER@ $$f"));
    }

    #[test]
    fn duties_scale_to_the_chips_range_and_back() {
        assert_eq!((raw(0), raw(40), raw(50), raw(100), raw(150)), (0, 102, 128, 255, 255));
        assert_eq!((duty(0), duty(102), duty(128), duty(255)), (0, 40, 50, 100));
        for d in 0..=100 {
            assert_eq!(duty(raw(d)), d);
        }
    }
}
