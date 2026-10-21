//! The graphics card's fans, with `[devices] gpu` on. The card's own curve keeps every
//! fan until camless gives it a curve, and gets it back when camless lets go or
//! stops.
//!
//! On Windows, NVIDIA cards through NVML (`nvml.dll`, from the driver), the library
//! Nexus and CoolerControl use on Linux: `nvmlDeviceSetFanSpeed_v2` for a duty and
//! `nvmlDeviceSetDefaultFanSpeed_v2` to give the fan back. Setting a fan needs
//! administrator rights. On Linux, AMD cards through amdgpu's hwmon files, as the
//! motherboard's headers (`board.rs`); NVML needs root there, which the service is
//! not, and AMD's ADLX on Windows is not driven yet.
//!
//! Experimental: nobody has run camless on a graphics card's fans yet.

use super::{Bus, Driver, Found, Model, Vendor};
use anyhow::Result;
use hidapi::HidApi;

pub const MODEL: Model =
    Model { bus: Bus::Host, experimental: true, vendor: Some(&GPU_APPS), ..Model::hid(0, 0, "gpu", "GPU", open) };

/// Other programs that set the card's fans: the motherboard's (`board::FAN_APPS`)
/// and MSI Afterburner.
pub static GPU_APPS: Vendor = Vendor {
    name: "Fan control",
    app: "Another fan control program (FanControl, MSI Afterburner, LibreHardwareMonitor, CoolerControl)",
    processes: &[
        "fancontrol.exe",
        "msiafterburner.exe",
        "librehardwaremonitor.exe",
        "openhardwaremonitor.exe",
        "argusmonitor.exe",
        "speedfan.exe",
        "coolercontrold",
        "thinkfan",
    ],
};

/// Gives back the fans a camless that stopped without giving them back had set,
/// whatever the settings say now. On Linux `board::hand_back_abandoned` does, for
/// amdgpu's files.
pub fn hand_back_abandoned() {
    #[cfg(windows)]
    nvidia::hand_back_abandoned();
}

/// The graphics cards with fans camless can set.
pub fn found() -> Vec<Found> {
    #[cfg(windows)]
    return nvidia::found();
    #[cfg(target_os = "linux")]
    return super::board::os::found_where(std::path::Path::new("/sys/class/hwmon"), &MODEL, |name, bus| name == "amdgpu" && bus == "pci", |_| {
        MODEL.slug.to_string()
    });
}

fn open(_api: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    #[cfg(windows)]
    return Ok(Box::new(nvidia::Nvidia::open(found)?));
    #[cfg(target_os = "linux")]
    return Ok(Box::new(super::board::os::Board::open(found, "AMD GPU (amdgpu)".to_string())?));
}

/// Why NVML refused, by its return code (`nvml.h`).
#[cfg_attr(not(windows), allow(dead_code))]
fn nvml_error(code: i32, what: &str) -> String {
    match code {
        3 => format!("{what}: this card does not let NVML set its fans (NVML_ERROR_NOT_SUPPORTED)"),
        4 => format!("{what}: NVML needs administrator rights for this; run the service as administrator"),
        _ => format!("{what} failed with NVML error {code}"),
    }
}

#[cfg(windows)]
mod nvidia {
    use super::{nvml_error, MODEL};
    use crate::drivers::{static_id, Driver, FanReading, Found, Reading};
    use anyhow::{bail, Context, Result};
    use camless_core::lighting::Lighting;
    use camless_core::status::{Device, FanChannel, Unit};
    use libloading::Library;
    use std::collections::HashMap;
    use std::ffi::{c_void, CString};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    type Dev = *mut c_void;

    /// NVML's functions camless uses, from `nvml.h`.
    struct Api {
        _lib: Library,
        shutdown: unsafe extern "C" fn() -> i32,
        count: unsafe extern "C" fn(*mut u32) -> i32,
        by_index: unsafe extern "C" fn(u32, *mut Dev) -> i32,
        name: unsafe extern "C" fn(Dev, *mut u8, u32) -> i32,
        num_fans: unsafe extern "C" fn(Dev, *mut u32) -> i32,
        min_max: unsafe extern "C" fn(Dev, *mut u32, *mut u32) -> i32,
        speed: unsafe extern "C" fn(Dev, u32, *mut u32) -> i32,
        set_speed: unsafe extern "C" fn(Dev, u32, u32) -> i32,
        set_default: unsafe extern "C" fn(Dev, u32) -> i32,
        /// From driver R520 on; without it there is no rpm.
        rpm: Option<unsafe extern "C" fn(Dev, *mut FanSpeedInfo) -> i32>,
    }

    /// `nvmlFanSpeedInfo_t`, version 1: `NVML_STRUCT_VERSION` is its size | 1 << 24.
    #[repr(C)]
    struct FanSpeedInfo {
        version: u32,
        fan: u32,
        speed: u32,
    }

    /// Where the driver puts `nvml.dll`: System32 since 2019, NVSMI before. A full
    /// path, so no `nvml.dll` beside camless loads instead.
    fn library() -> Result<Library> {
        let system = std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join("System32").join("nvml.dll"));
        let nvsmi = std::env::var_os("ProgramFiles").map(|p| PathBuf::from(p).join("NVIDIA Corporation").join("NVSMI").join("nvml.dll"));
        for path in [system, nvsmi].into_iter().flatten().filter(|p| p.exists()) {
            if let Ok(lib) = unsafe { Library::new(&path) } {
                return Ok(lib);
            }
        }
        bail!("nvml.dll not found: no NVIDIA driver")
    }

    impl Api {
        fn open() -> Result<Api> {
            unsafe {
                let lib = library()?;
                let init: unsafe extern "C" fn() -> i32 = *lib.get(b"nvmlInit_v2\0")?;
                // Every function first, so a missing one returns before NVML starts.
                let api = Api {
                    shutdown: *lib.get(b"nvmlShutdown\0")?,
                    count: *lib.get(b"nvmlDeviceGetCount_v2\0")?,
                    by_index: *lib.get(b"nvmlDeviceGetHandleByIndex_v2\0")?,
                    name: *lib.get(b"nvmlDeviceGetName\0")?,
                    num_fans: *lib.get(b"nvmlDeviceGetNumFans\0")?,
                    min_max: *lib.get(b"nvmlDeviceGetMinMaxFanSpeed\0")?,
                    speed: *lib.get(b"nvmlDeviceGetFanSpeed_v2\0")?,
                    set_speed: *lib.get(b"nvmlDeviceSetFanSpeed_v2\0")?,
                    set_default: *lib.get(b"nvmlDeviceSetDefaultFanSpeed_v2\0")?,
                    rpm: lib.get(b"nvmlDeviceGetFanSpeedRPM\0").ok().map(|f| *f),
                    _lib: lib,
                };
                let code = init();
                if code != 0 {
                    // Dropped without nvmlShutdown: NVML did not start.
                    std::mem::forget(api);
                    bail!(nvml_error(code, "nvmlInit"));
                }
                Ok(api)
            }
        }

        fn device(&self, index: u32) -> Result<Dev> {
            let mut dev = std::ptr::null_mut();
            let code = unsafe { (self.by_index)(index, &mut dev) };
            if code != 0 {
                bail!(nvml_error(code, "nvmlDeviceGetHandleByIndex"));
            }
            Ok(dev)
        }

        fn fans(&self, dev: Dev) -> u32 {
            let mut n = 0;
            if unsafe { (self.num_fans)(dev, &mut n) } == 0 {
                n
            } else {
                0
            }
        }
    }

    /// Each start of NVML ends with its own shutdown, before the library goes.
    impl Drop for Api {
        fn drop(&mut self) {
            unsafe { (self.shutdown)() };
        }
    }

    /// The fans this process has set and not given back yet.
    static TAKEN: AtomicUsize = AtomicUsize::new(0);

    /// There while camless may have left a fan at a duty: a camless that was killed
    /// or crashed has no Drop to give the fans back, and the card keeps a duty NVML
    /// set. The next one gives every fan back to the card first.
    fn marker() -> PathBuf {
        camless_core::config::app_config_dir().join("camless").join("taken-gpu-fans")
    }

    /// Gives every fan of every card back to its curve after a camless that did not,
    /// whatever the settings say now.
    pub fn hand_back_abandoned() {
        if TAKEN.load(Ordering::SeqCst) > 0 || !marker().exists() {
            return;
        }
        let Ok(api) = Api::open() else { return };
        let mut count = 0;
        if unsafe { (api.count)(&mut count) } != 0 {
            return;
        }
        let mut all = true;
        for i in 0..count {
            let Ok(dev) = api.device(i) else { continue };
            for f in 0..api.fans(dev) {
                all &= unsafe { (api.set_default)(dev, f) } == 0;
            }
        }
        if all {
            let _ = std::fs::remove_file(marker());
        }
    }

    /// Every NVIDIA card with a fan, by NVML's index.
    pub fn found() -> Vec<Found> {
        let Ok(api) = Api::open() else { return Vec::new() };
        let mut count = 0;
        if unsafe { (api.count)(&mut count) } != 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        for i in 0..count {
            let Ok(dev) = api.device(i) else { continue };
            if api.fans(dev) == 0 {
                continue;
            }
            let id = if out.is_empty() { MODEL.slug.to_string() } else { format!("{}#{}", MODEL.slug, out.len() + 1) };
            out.push(Found { model: &MODEL, id, path: CString::new(i.to_string()).unwrap_or_default() });
        }
        out
    }

    /// Fans NVML would not let camless set, by card and fan, with why: left out when
    /// the card is opened again, rather than closing it over and over.
    static REFUSED: Mutex<Option<HashMap<(u32, u32), String>>> = Mutex::new(None);

    pub struct Nvidia {
        api: Api,
        dev: Dev,
        index: u32,
        name: String,
        /// NVML's fan numbers camless can set, with their ids.
        fans: Vec<(u32, &'static str)>,
        min: u8,
        max: u8,
        /// The fans camless set, which go back to the card's curve.
        taken: Vec<bool>,
    }

    impl Nvidia {
        pub fn open(found: &Found) -> Result<Nvidia> {
            let index: u32 = found.path.to_str().ok().and_then(|s| s.parse().ok()).context("no GPU index")?;
            let api = Api::open()?;
            let dev = api.device(index)?;
            let n = api.fans(dev);
            if n == 0 {
                bail!("the card reports no fans");
            }
            let mut buf = [0u8; 96];
            let name = if unsafe { (api.name)(dev, buf.as_mut_ptr(), buf.len() as u32) } == 0 {
                let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                String::from_utf8_lossy(&buf[..len]).into_owned()
            } else {
                "NVIDIA GPU".to_string()
            };
            let (mut min, mut max) = (0u32, 100u32);
            if unsafe { (api.min_max)(dev, &mut min, &mut max) } != 0 {
                (min, max) = (0, 100);
            }
            let refused = REFUSED.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
            let fans: Vec<(u32, &'static str)> =
                (0..n).filter(|f| !refused.contains_key(&(index, *f))).map(|f| (f, static_id(&format!("fan{}", f + 1)))).collect();
            if fans.is_empty() {
                let why = refused.get(&(index, 0)).cloned().unwrap_or_default();
                bail!("{why}");
            }
            // Fans a camless that stopped without giving them back had set.
            if marker().exists() {
                for &(f, _) in &fans {
                    unsafe { (api.set_default)(dev, f) };
                }
            }
            let taken = vec![false; fans.len()];
            let (min, max) = (min.min(100) as u8, max.clamp(1, 100) as u8);
            let min = min.min(max);
            Ok(Nvidia { api, dev, index, name, fans, min, max, taken })
        }

        fn fan(&self, id: &str) -> Option<usize> {
            self.fans.iter().position(|(_, f)| *f == id)
        }
    }

    impl Driver for Nvidia {
        fn describe(&self) -> Device {
            Device {
                name: self.name.clone(),
                verified: false,
                experimental: MODEL.is_experimental(),
                fans: self
                    .fans
                    .iter()
                    .map(|(f, id)| FanChannel { id: id.to_string(), name: format!("Fan {}", f + 1), min_duty: self.min, ..Default::default() })
                    .collect(),
                ..Default::default()
            }
        }

        fn sensors(&self) -> Vec<(&'static str, &'static str, Unit)> {
            Vec::new()
        }

        fn poll(&mut self) -> Result<Reading> {
            let mut fans = Vec::new();
            for &(f, id) in &self.fans {
                let mut speed = 0;
                let code = unsafe { (self.api.speed)(self.dev, f, &mut speed) };
                if code != 0 {
                    bail!(nvml_error(code, "nvmlDeviceGetFanSpeed_v2"));
                }
                let rpm = self.api.rpm.and_then(|get| {
                    let mut info = FanSpeedInfo { version: std::mem::size_of::<FanSpeedInfo>() as u32 | 1 << 24, fan: f, speed: 0 };
                    (unsafe { get(self.dev, &mut info) } == 0).then(|| info.speed.min(u16::MAX as u32) as u16)
                });
                fans.push((id, FanReading { rpm, duty: Some(speed.min(100) as u8), connected: None }));
            }
            Ok(Reading { sensors: Vec::new(), fans })
        }

        fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
            for &(id, duty) in duties {
                let i = self.fan(id).with_context(|| format!("no fan {id:?} on the {}", self.name))?;
                let duty = duty.clamp(self.min, self.max);
                if !self.taken[i] {
                    // Written down first, and taken before the call: one that fails
                    // can have left the fan manual already.
                    let m = marker();
                    if let Some(dir) = m.parent() {
                        std::fs::create_dir_all(dir).with_context(|| format!("writing {}", m.display()))?;
                    }
                    std::fs::write(&m, "").with_context(|| format!("writing {}", m.display()))?;
                    self.taken[i] = true;
                    TAKEN.fetch_add(1, Ordering::SeqCst);
                }
                let code = unsafe { (self.api.set_speed)(self.dev, self.fans[i].0, u32::from(duty)) };
                if code != 0 {
                    let why = nvml_error(code, "nvmlDeviceSetFanSpeed_v2");
                    // The fan, or every fan without administrator rights, is left out
                    // until the service starts again, rather than taken over and over.
                    let mut refused = REFUSED.lock().unwrap_or_else(|e| e.into_inner());
                    let refused = refused.get_or_insert_with(HashMap::new);
                    let all: Vec<u32> = if code == 4 { self.fans.iter().map(|(f, _)| *f).collect() } else { vec![self.fans[i].0] };
                    for f in all {
                        refused.insert((self.index, f), why.clone());
                    }
                    bail!(why);
                }
            }
            Ok(())
        }

        fn set_lighting(&mut self, light: &str, _: &Lighting) -> Result<()> {
            bail!("no light {light:?} on the {}", self.name)
        }

        fn release_unused(&mut self, used: &[&str]) -> Result<()> {
            let mut first = Ok(());
            for i in 0..self.taken.len() {
                if !self.taken[i] || used.contains(&self.fans[i].1) {
                    continue;
                }
                let code = unsafe { (self.api.set_default)(self.dev, self.fans[i].0) };
                if code == 0 {
                    self.taken[i] = false;
                    if TAKEN.fetch_sub(1, Ordering::SeqCst) == 1 {
                        let _ = std::fs::remove_file(marker());
                    }
                } else if first.is_ok() {
                    first = Err(anyhow::anyhow!(nvml_error(code, "nvmlDeviceSetDefaultFanSpeed_v2")));
                }
            }
            first
        }

        fn release(&mut self) -> Result<()> {
            self.release_unused(&[])
        }
    }

    /// Closing for any reason gives the fans back to the card.
    impl Drop for Nvidia {
        fn drop(&mut self) {
            let _ = self.release();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvml_errors_say_what_to_do() {
        assert!(nvml_error(4, "set").contains("administrator"));
        assert!(nvml_error(3, "set").contains("NOT_SUPPORTED"));
        assert_eq!(nvml_error(999, "set"), "set failed with NVML error 999");
    }
}
