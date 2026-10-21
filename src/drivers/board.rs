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
    return Ok(Box::new(os::Board::open(found, os::board_name(found)?)?));
    #[cfg(not(target_os = "linux"))]
    {
        let _ = found;
        anyhow::bail!("camless sets motherboard fan headers on Linux only, so far")
    }
}

/// hwmon's fan files, as the kernel's drivers for the motherboard's chips give them.
#[cfg(target_os = "linux")]
pub mod os {
    use super::{duty, raw, MODEL};
    use crate::drivers::{Driver, FanReading, Found, Model, Reading};
    use anyhow::{bail, Context, Result};
    use camless_core::lighting::Lighting;
    use camless_core::status::{Device, FanChannel, Unit};
    use std::collections::{BTreeMap, HashSet};
    use std::ffi::CString;
    use std::fs;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    /// The `name` of the hwmon drivers for Super I/O chips, by prefix: Nuvoton and
    /// Winbond (`nct6775`, `nct6798`, `w83627ehf`), ITE (`it87`, `it8688`) and Fintek
    /// (`f71882fg`, `f71869`, `f81866a`). Not the GPU's or a USB device's hwmon, which
    /// have their own drivers in camless or none.
    pub const CHIPS: [&str; 5] = ["nct6", "w83", "it8", "f71", "f8"];

    pub fn is_super_io(name: &str) -> bool {
        CHIPS.iter().any(|c| name.starts_with(c))
    }

    pub fn read(path: &Path) -> Option<String> {
        fs::read_to_string(path).ok().map(|s| s.trim().to_string())
    }

    /// The folder with a hwmon device's files: its own, or its device's for a driver
    /// on hwmon's old interface, as `f71882fg` was.
    fn files(dir: &Path) -> PathBuf {
        if dir.join("name").exists() {
            dir.to_path_buf()
        } else {
            dir.join("device")
        }
    }

    /// The Super I/O chips with at least one header camless can set, by chip:
    /// `motherboard-nct6798`, and `motherboard-nct6798#2` for a second of the same.
    pub fn found(root: &Path) -> Vec<Found> {
        // A chip on the platform bus: what the Super I/O drivers register.
        found_where(root, &MODEL, |name, bus| is_super_io(name) && bus == "platform", |chip| format!("{}-{chip}", MODEL.slug))
    }

    /// The hwmon devices whose driver `name` and bus `keep` takes, with at least one
    /// header, as devices of `model` with the `id` of their name. Two of the same are
    /// in the order of their devices, which does not change between boots as `hwmonN`
    /// can.
    pub fn found_where(
        root: &Path,
        model: &'static Model,
        keep: impl Fn(&str, &str) -> bool,
        id: impl Fn(&str) -> String,
    ) -> Vec<Found> {
        let bus = |dir: &Path| {
            fs::read_link(dir.join("device").join("subsystem"))
                .ok()
                .and_then(|s| s.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_default()
        };
        let mut chips: Vec<(PathBuf, String, PathBuf)> = fs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter_map(|dir| {
                let files = files(&dir);
                let name = read(&files.join("name"))?;
                let device = fs::canonicalize(dir.join("device")).unwrap_or_else(|_| dir.clone());
                (keep(&name, &bus(&dir)) && !headers(&files).is_empty()).then_some((device, name, files))
            })
            .collect();
        chips.sort();
        let mut out: Vec<Found> = Vec::new();
        for (_, name, files) in chips {
            let base = id(&name);
            let same = out.iter().filter(|f| f.id == base || f.id.starts_with(&format!("{base}#"))).count();
            let id = if same == 0 { base } else { format!("{base}#{}", same + 1) };
            let Ok(path) = CString::new(files.as_os_str().as_bytes()) else { continue };
            out.push(Found { model, id, path });
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

    /// Headers whose chip kept `pwmN_enable` off manual mode when camless asked, by
    /// that file: left out when the chip is opened again, so one header the chip
    /// keeps does not close the others over and over.
    static REFUSED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

    fn refused(enable: &Path) -> bool {
        REFUSED.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|s| s.contains(enable))
    }

    /// The headers camless has taken, with the mode and duty they had, in a file: a
    /// service that was killed or crashed gives them back to that when it starts
    /// again, instead of taking its own manual mode for what they had. For this boot
    /// only, as the BIOS sets the chip again when the computer starts.
    static TAKEN: Mutex<()> = Mutex::new(());

    fn taken_file() -> PathBuf {
        camless_core::config::xdg_dir("XDG_STATE_HOME", ".local/state").join("camless").join("taken-fans")
    }

    fn boot() -> String {
        read(Path::new("/proc/sys/kernel/random/boot_id")).unwrap_or_default()
    }

    type Before = (String, String);

    /// The file's lines: the boot, then a header's `pwmN_enable`, its mode and its
    /// duty, by tabs.
    fn load(file: &Path) -> BTreeMap<PathBuf, Before> {
        let Ok(text) = fs::read_to_string(file) else { return BTreeMap::new() };
        let mut lines = text.lines();
        if lines.next() != Some(boot().as_str()) {
            return BTreeMap::new();
        }
        lines
            .filter_map(|l| {
                let mut p = l.split('\t');
                Some((PathBuf::from(p.next()?), (p.next()?.to_string(), p.next()?.to_string())))
            })
            .collect()
    }

    fn saved(file: &Path) -> BTreeMap<PathBuf, Before> {
        let _lock = TAKEN.lock().unwrap_or_else(|e| e.into_inner());
        load(file)
    }

    fn store(file: &Path, change: impl FnOnce(&mut BTreeMap<PathBuf, Before>)) -> Result<()> {
        let _lock = TAKEN.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = load(file);
        change(&mut all);
        let what = || format!("writing {}", file.display());
        if all.is_empty() {
            return match fs::remove_file(file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e).with_context(what),
                _ => Ok(()),
            };
        }
        let mut text = boot() + "\n";
        for (enable, (mode, pwm)) in &all {
            text += &format!("{}\t{mode}\t{pwm}\n", enable.display());
        }
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir).with_context(what)?;
        }
        let new = file.with_extension("new");
        fs::write(&new, text).and_then(|()| fs::rename(&new, file)).with_context(what)
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
        before: Option<Before>,
    }

    /// The headers of one hwmon chip.
    pub struct Board {
        model: &'static Model,
        name: String,
        headers: Vec<Header>,
        taken: PathBuf,
    }

    /// The hwmon folder of a device found here.
    pub fn dir(found: &Found) -> PathBuf {
        PathBuf::from(std::ffi::OsStr::from_bytes(found.path.as_bytes()))
    }

    /// What the motherboard is called, with its chip: "ASUSTeK ROG STRIX B550-F (nct6798)".
    pub fn board_name(found: &Found) -> Result<String> {
        let chip = read(&dir(found).join("name")).context("the chip is gone")?;
        let dmi = |f: &str| read(&Path::new("/sys/class/dmi/id").join(f)).filter(|s| !s.is_empty());
        let board = match (dmi("board_vendor"), dmi("board_name")) {
            (Some(v), Some(n)) => format!("{v} {n}"),
            (None, Some(n)) => n,
            _ => MODEL.name.to_string(),
        };
        Ok(format!("{board} ({chip})"))
    }

    fn write(path: &Path, value: &str) -> Result<()> {
        fs::write(path, value).with_context(|| format!("writing {value} to {}", path.display()))
    }

    /// Opening for writing writes nothing; it fails without the udev rule.
    fn writable(path: &Path) -> bool {
        fs::OpenOptions::new().write(true).open(path).is_ok()
    }

    /// Hands one header back to the BIOS: the mode it had, its duty too if that was
    /// manual. A chip that refuses the mode gets automatic (2), then full speed (0),
    /// as Nexus falls back. The header stays camless's until that works.
    fn hand_back(h: &mut Header, taken: &Path) -> Result<()> {
        let Some((enable, pwm)) = h.before.clone() else { return Ok(()) };
        if enable == "1" {
            let _ = write(&h.pwm, &pwm);
        }
        write(&h.enable, &enable).or_else(|_| write(&h.enable, "2")).or_else(|_| write(&h.enable, "0"))?;
        h.before = None;
        store(taken, |all| {
            all.remove(&h.enable);
        })
    }

    impl Board {
        pub fn open(found: &Found, name: String) -> Result<Board> {
            Board::open_at(found, name, taken_file())
        }

        fn open_at(found: &Found, name: String, taken: PathBuf) -> Result<Board> {
            let dir = dir(found);
            let mut headers = Vec::new();
            let mut locked = None;
            for n in self::headers(&dir) {
                let (pwm, enable) = (dir.join(format!("pwm{n}")), dir.join(format!("pwm{n}_enable")));
                if refused(&enable) {
                    continue;
                }
                // The udev rule gives pwm1..pwm9; a header past them stays the BIOS's.
                if !writable(&enable) || !writable(&pwm) {
                    locked.get_or_insert(enable);
                    continue;
                }
                let label = read(&dir.join(format!("fan{n}_label")));
                headers.push(Header {
                    n,
                    id: crate::drivers::static_id(&format!("fan{n}")),
                    name: label.unwrap_or_else(|| format!("Fan {n}")),
                    pwm,
                    enable,
                    rpm: dir.join(format!("fan{n}_input")),
                    before: None,
                });
            }
            if headers.is_empty() {
                match locked {
                    Some(f) => bail!(
                        "no write access to {}; install.sh's udev rules give it (/etc/udev/rules.d/72-camless-fans.rules), and apply after a restart",
                        f.display()
                    ),
                    None => bail!("the {name} has no header camless can set"),
                }
            }
            let mut board = Board { model: found.model, name, headers, taken };
            // Headers a service that stopped without handing them back had taken.
            let saved = saved(&board.taken);
            for h in &mut board.headers {
                h.before = saved.get(&h.enable).cloned();
            }
            board.release()?;
            Ok(board)
        }
    }

    impl Driver for Board {
        fn describe(&self) -> Device {
            Device {
                name: self.name.clone(),
                verified: false,
                experimental: self.model.is_experimental(),
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
                let Some(h) = self.headers.iter_mut().find(|h| h.id == fan) else { bail!("no fan {fan:?} on the {}", self.name) };
                if h.before.is_none() {
                    let before = (read(&h.enable).context("reading the header's mode")?, read(&h.pwm).unwrap_or_default());
                    // Written down first: a header camless could not give back after a
                    // crash would keep its last duty however hot it gets.
                    store(&self.taken, |all| {
                        all.insert(h.enable.clone(), before.clone());
                    })?;
                    h.before = Some(before);
                    write(&h.enable, "1")?;
                    // A chip can refuse manual mode on a header, or the driver put it back.
                    let now = read(&h.enable).unwrap_or_default();
                    if now != "1" {
                        REFUSED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashSet::new).insert(h.enable.clone());
                        let _ = hand_back(h, &self.taken);
                        bail!("the chip keeps pwm{}_enable at {now}, not 1: it does not let camless set this header, which is left out from now on", h.n);
                    }
                }
                write(&h.pwm, &raw(d).to_string())?;
            }
            Ok(())
        }

        fn set_lighting(&mut self, light: &str, _: &Lighting) -> Result<()> {
            bail!("no light {light:?} on the {}", self.name)
        }

        fn release_unused(&mut self, used: &[&str]) -> Result<()> {
            let mut first = Ok(());
            for h in self.headers.iter_mut().filter(|h| !used.contains(&h.id)) {
                let r = hand_back(h, &self.taken);
                if first.is_ok() {
                    first = r;
                }
            }
            first
        }

        fn release(&mut self) -> Result<()> {
            self.release_unused(&[])
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
            headers_in(&dir, pwms);
            dir
        }

        fn headers_in(dir: &Path, pwms: &[u8]) {
            for n in pwms {
                fs::write(dir.join(format!("pwm{n}")), "128\n").unwrap();
                fs::write(dir.join(format!("pwm{n}_enable")), "5\n").unwrap();
                fs::write(dir.join(format!("fan{n}_input")), "900\n").unwrap();
            }
        }

        fn tmp(name: &str) -> PathBuf {
            let root = std::env::temp_dir().join(format!("camless-board-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            root
        }

        fn found_at(dir: &Path) -> Found {
            Found { model: &MODEL, id: "motherboard-nct6798".into(), path: CString::new(dir.to_str().unwrap()).unwrap() }
        }

        #[test]
        fn finds_super_io_chips_on_the_platform_bus_only() {
            let root = tmp("scan");
            let a = chip(&root, "hwmon3", "nct6798", "nct6775.656", "platform", &[1, 2, 7]);
            chip(&root, "hwmon1", "amdgpu", "0000:03:00.0", "pci", &[1]);
            chip(&root, "hwmon2", "nzxtkraken3", "0003:1E71:2007.0004", "hid", &[1, 2]);
            chip(&root, "hwmon4", "k10temp", "0000:00:18.3", "pci", &[]);
            let b = chip(&root, "hwmon0", "it8792", "it87.2592", "platform", &[1]);
            let c = chip(&root, "hwmon6", "it8792", "it87.2560", "platform", &[2]);
            chip(&root, "hwmon5", "it8689", "it87.2656", "platform", &[]);
            // On hwmon's old interface the files are the device's.
            let dev = root.join("devices").join("f71882fg.2576");
            let old = chip(&root, "hwmon7", "f71882fg", "f71882fg.2576", "platform", &[]);
            fs::remove_file(old.join("name")).unwrap();
            fs::write(dev.join("name"), "f71882fg\n").unwrap();
            headers_in(&dev, &[1, 3]);
            let found = found(&root.join("class"));
            let got: Vec<_> = found.iter().map(|f| (f.id.as_str(), PathBuf::from(f.path.to_str().unwrap()))).collect();
            // By chip, and two of the same by their devices, whatever hwmonN says.
            assert_eq!(
                got,
                [
                    ("motherboard-f71882fg", old.join("device")),
                    ("motherboard-it8792", c),
                    ("motherboard-it8792#2", b),
                    ("motherboard-nct6798", a.clone()),
                ]
            );
            assert_eq!(headers(&a), [1, 2, 7]);
            fs::remove_dir_all(&root).unwrap();
        }

        #[test]
        fn takes_a_header_only_when_given_a_duty_and_gives_back_what_it_had() {
            let root = tmp("drive");
            let dir = chip(&root, "hwmon2", "nct6798", "nct6775.656", "platform", &[1, 2]);
            fs::write(dir.join("fan2_label"), "CPU_FAN\n").unwrap();
            let taken = root.join("taken-fans");
            let mut b = Board::open_at(&found_at(&dir), "Motherboard (nct6798)".into(), taken.clone()).unwrap();
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
            assert!(!taken.exists());
            // A header the user had set by hand gets its duty back too.
            fs::write(dir.join("pwm1_enable"), "1\n").unwrap();
            fs::write(dir.join("pwm1"), "200\n").unwrap();
            b.set_duties(&[("fan1", 100)]).unwrap();
            assert_eq!(read(&dir.join("pwm1")).unwrap(), "255");
            drop(b);
            assert_eq!((read(&dir.join("pwm1_enable")).unwrap(), read(&dir.join("pwm1")).unwrap()), ("1".into(), "200".into()));
            fs::remove_dir_all(&root).unwrap();
        }

        #[test]
        fn a_header_without_a_curve_goes_back_to_the_bios() {
            let root = tmp("unused");
            let dir = chip(&root, "hwmon2", "nct6798", "nct6775.656", "platform", &[1, 2]);
            let mut b = Board::open_at(&found_at(&dir), "Motherboard".into(), root.join("taken-fans")).unwrap();
            b.set_duties(&[("fan1", 60), ("fan2", 60)]).unwrap();
            b.release_unused(&["fan2"]).unwrap();
            assert_eq!(read(&dir.join("pwm1_enable")).unwrap(), "5");
            assert_eq!(read(&dir.join("pwm2_enable")).unwrap(), "1");
            // A hand back that fails keeps the header camless's, to try again.
            fs::remove_file(dir.join("pwm2_enable")).unwrap();
            fs::create_dir(dir.join("pwm2_enable")).unwrap();
            assert!(b.release().is_err());
            assert!(b.headers[1].before.is_some());
            fs::remove_dir(dir.join("pwm2_enable")).unwrap();
            fs::write(dir.join("pwm2_enable"), "1\n").unwrap();
            b.release().unwrap();
            assert_eq!(read(&dir.join("pwm2_enable")).unwrap(), "5");
            fs::remove_dir_all(&root).unwrap();
        }

        #[test]
        fn a_service_that_crashed_gives_back_what_the_headers_had_when_it_starts() {
            let root = tmp("crash");
            let dir = chip(&root, "hwmon2", "nct6798", "nct6775.656", "platform", &[1, 2]);
            let taken = root.join("state").join("taken-fans");
            let mut b = Board::open_at(&found_at(&dir), "Motherboard".into(), taken.clone()).unwrap();
            b.set_duties(&[("fan2", 30)]).unwrap();
            // Killed: no Drop, the header stays manual.
            std::mem::forget(b);
            assert_eq!(read(&dir.join("pwm2_enable")).unwrap(), "1");
            assert!(fs::read_to_string(&taken).unwrap().contains("pwm2_enable\t5\t128"));
            let b = Board::open_at(&found_at(&dir), "Motherboard".into(), taken.clone()).unwrap();
            assert_eq!(read(&dir.join("pwm2_enable")).unwrap(), "5");
            assert!(!taken.exists());
            drop(b);
            // From another boot, the file says nothing: the BIOS set the chip again.
            fs::create_dir_all(taken.parent().unwrap()).unwrap();
            fs::write(&taken, format!("another boot\n{}\t2\t0\n", dir.join("pwm1_enable").display())).unwrap();
            let _b = Board::open_at(&found_at(&dir), "Motherboard".into(), taken.clone()).unwrap();
            assert_eq!(read(&dir.join("pwm1_enable")).unwrap(), "5");
            fs::remove_dir_all(&root).unwrap();
        }

        #[test]
        fn a_header_the_chip_keeps_is_left_out() {
            let root = tmp("refused");
            let dir = chip(&root, "hwmon2", "nct6798", "nct6775.656", "platform", &[1, 2]);
            REFUSED.lock().unwrap().get_or_insert_with(HashSet::new).insert(dir.join("pwm1_enable"));
            let b = Board::open_at(&found_at(&dir), "Motherboard".into(), root.join("taken-fans")).unwrap();
            assert_eq!(b.describe().fans.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["fan2"]);
            drop(b);
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
        let names = line.split(r#"ATTRS{name}==""#).nth(1).unwrap().split('"').next().unwrap();
        assert_eq!(names, "nct6*|w83*|it8*|f71*|f8*");
        #[cfg(target_os = "linux")]
        assert_eq!(names.split('|').map(|n| n.trim_end_matches('*')).collect::<Vec<_>>(), os::CHIPS);
        let files: Vec<&str> = line.split_whitespace().filter(|w| w.starts_with("/sys%p/")).collect();
        assert_eq!(files, ["/sys%p/pwm[1-9]", "/sys%p/pwm[1-9]_enable", "/sys%p/device/pwm[1-9]", "/sys%p/device/pwm[1-9]_enable;"]);
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
