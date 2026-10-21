//! Host sensors without CAM: GPU through NVML, other cards through amdgpu on Linux
//! and on Windows through what Windows keeps for Task Manager;
//! CPU temperature and power through PawnIO on Windows, the one LibreHardwareMonitor
//! and FanControl already install, and through hwmon and RAPL on Linux; CPU load and
//! memory from the system. [`os`] holds what differs.

use anyhow::{bail, Context, Result};
use camless_core::status::{Sensor, Unit};
use libloading::Library;
use serde::Serialize;
use std::ffi::c_void;

#[cfg(target_os = "linux")]
#[path = "sensors/linux.rs"]
mod os;
#[cfg(windows)]
#[path = "sensors/windows.rs"]
mod os;
#[cfg(windows)]
pub use os::{reg_dword, reg_string};
pub use os::{cpu_name, NO_CPU_TEMP};

#[derive(Clone, Debug, Default, Serialize)]
pub struct Readings {
    pub cpu_temp_c: Option<f32>,
    pub cpu_load_pct: Option<f32>,
    pub cpu_power_w: Option<f32>,
    pub gpu_temp_c: Option<f32>,
    pub gpu_load_pct: Option<f32>,
    pub gpu_power_w: Option<f32>,
    pub ram_used_pct: Option<f32>,
    pub ram_used_gb: Option<f32>,
    pub ram_total_gb: Option<f32>,
}

impl Readings {
    /// Host sensors by the ids curves use. CPU temperature is listed even without a
    /// reading, so a curve can name it before PawnIO is set up.
    pub fn list(&self, gpu: bool) -> Vec<Sensor> {
        let s = |id: &str, name: &str, unit, value| Sensor { id: id.into(), name: name.into(), unit, value };
        let mut out = vec![
            s("cpu.temp", "CPU temperature", Unit::Celsius, self.cpu_temp_c),
            s("cpu.load", "CPU load", Unit::Percent, self.cpu_load_pct),
            s("cpu.power", "CPU power", Unit::Watt, self.cpu_power_w),
        ];
        if gpu {
            out.extend([
                s("gpu.temp", "GPU temperature", Unit::Celsius, self.gpu_temp_c),
                s("gpu.load", "GPU load", Unit::Percent, self.gpu_load_pct),
                s("gpu.power", "GPU power", Unit::Watt, self.gpu_power_w),
            ]);
        }
        out.push(s("ram.load", "Memory in use", Unit::Percent, self.ram_used_pct));
        out
    }
}

/// The GPU the `gpu.*` sensors read.
enum Gpu {
    Nvidia(Nvml),
    #[cfg(target_os = "linux")]
    Amd(os::AmdGpu),
    /// Any card NVML does not read, AMD's and Intel's: temperature and load, no power.
    #[cfg(windows)]
    Wddm(os::WddmGpu),
}

impl Gpu {
    fn open() -> Result<Gpu> {
        let nvml = match Nvml::open() {
            Ok(n) => return Ok(Gpu::Nvidia(n)),
            Err(e) => e,
        };
        #[cfg(target_os = "linux")]
        return os::AmdGpu::open().map(Gpu::Amd).map_err(|amd| anyhow::anyhow!("{nvml:#}; {amd:#}"));
        #[cfg(windows)]
        return os::WddmGpu::open().map(Gpu::Wddm).map_err(|wddm| anyhow::anyhow!("{nvml:#}; {wddm:#}"));
    }

    fn name(&self) -> Option<String> {
        match self {
            Gpu::Nvidia(n) => n.name().ok(),
            #[cfg(target_os = "linux")]
            Gpu::Amd(a) => Some(a.name.clone()),
            #[cfg(windows)]
            Gpu::Wddm(w) => Some(w.name.clone()),
        }
    }

    fn read(&mut self, r: &mut Readings) {
        match self {
            Gpu::Nvidia(n) => {
                r.gpu_temp_c = n.temperature().ok();
                r.gpu_load_pct = n.utilization().ok();
                r.gpu_power_w = n.power_w().ok();
            }
            #[cfg(target_os = "linux")]
            Gpu::Amd(a) => {
                r.gpu_temp_c = a.temperature();
                r.gpu_load_pct = a.utilization();
                r.gpu_power_w = a.power_w();
            }
            #[cfg(windows)]
            Gpu::Wddm(w) => {
                r.gpu_temp_c = w.temperature();
                r.gpu_load_pct = w.utilization();
            }
        }
    }
}

pub struct Sensors {
    pub cpu_name: String,
    pub gpu_name: Option<String>,
    /// Why CPU temperature and power are off, when they are.
    pub cpu_error: Option<String>,
    /// Why there is no GPU to read, when there is none.
    pub gpu_error: Option<String>,
    gpu: Option<Gpu>,
    cpu: Option<os::Cpu>,
    times: Option<(u64, u64)>,
}

impl Sensors {
    /// `pawnio_module` is the signed `AMDFamily17.bin`, or why it could not be read;
    /// without it, or without admin rights, CPU temperature and power are absent on
    /// Windows. Linux reads hwmon, which needs neither.
    pub fn open(pawnio_module: std::result::Result<Vec<u8>, String>) -> Sensors {
        let (gpu, gpu_error) = match Gpu::open() {
            Ok(gpu) => (Some(gpu), None),
            Err(e) => {
                eprintln!("gpu sensors off: {e:#}");
                (None, Some(format!("{e:#}")))
            }
        };
        let (cpu, cpu_error) = match os::cpu(pawnio_module) {
            Ok(cpu) => (Some(cpu), None),
            Err(e) => {
                eprintln!("cpu temperature off: {e}");
                (None, Some(e))
            }
        };
        let gpu_name = gpu.as_ref().and_then(Gpu::name);
        Sensors { cpu_name: os::cpu_name().unwrap_or_else(|| "CPU".into()), gpu_name, cpu_error, gpu_error, gpu, cpu, times: None }
    }

    pub fn has_cpu_temp(&self) -> bool {
        self.cpu.is_some()
    }

    pub fn read(&mut self) -> Readings {
        let mut r = Readings::default();
        if let Some(g) = &mut self.gpu {
            g.read(&mut r);
        }
        if let Some(c) = &mut self.cpu {
            r.cpu_temp_c = c.temperature().ok();
            r.cpu_power_w = c.package_power_w();
        }
        r.cpu_load_pct = self.cpu_load();
        if let Some((total, used, pct)) = os::memory() {
            let gb = |b: u64| b as f32 / (1u64 << 30) as f32;
            r.ram_total_gb = Some(gb(total));
            r.ram_used_gb = Some(gb(used));
            r.ram_used_pct = Some(pct);
        }
        r
    }

    fn cpu_load(&mut self) -> Option<f32> {
        let (idle, busy_and_idle) = os::cpu_times()?;
        let prev = self.times.replace((idle, busy_and_idle));
        let (pi, pt) = prev?;
        let total = busy_and_idle.checked_sub(pt)?;
        (total > 0).then(|| 100.0 * (1.0 - idle.saturating_sub(pi) as f32 / total as f32))
    }
}

type NvmlDevice = *mut c_void;

struct Nvml {
    _lib: Library,
    device: NvmlDevice,
    get_temperature: unsafe extern "C" fn(NvmlDevice, u32, *mut u32) -> i32,
    get_utilization: unsafe extern "C" fn(NvmlDevice, *mut [u32; 2]) -> i32,
    get_power: unsafe extern "C" fn(NvmlDevice, *mut u32) -> i32,
    get_name: unsafe extern "C" fn(NvmlDevice, *mut u8, u32) -> i32,
}

// The device handle is an opaque pointer that NVML allows using from any thread.
unsafe impl Send for Nvml {}

fn nvml_ok(code: i32, what: &str) -> Result<()> {
    if code != 0 {
        bail!("{what} failed with NVML error {code}");
    }
    Ok(())
}

impl Nvml {
    fn open() -> Result<Nvml> {
        unsafe {
            let lib = Library::new(os::NVML).with_context(|| format!("{} not found", os::NVML))?;
            let init: unsafe extern "C" fn() -> i32 = *lib.get(b"nvmlInit_v2\0")?;
            nvml_ok(init(), "nvmlInit")?;
            let by_index: unsafe extern "C" fn(u32, *mut NvmlDevice) -> i32 =
                *lib.get(b"nvmlDeviceGetHandleByIndex_v2\0")?;
            let mut device = std::ptr::null_mut();
            nvml_ok(by_index(0, &mut device), "nvmlDeviceGetHandleByIndex")?;
            Ok(Nvml {
                get_temperature: *lib.get(b"nvmlDeviceGetTemperature\0")?,
                get_utilization: *lib.get(b"nvmlDeviceGetUtilizationRates\0")?,
                get_power: *lib.get(b"nvmlDeviceGetPowerUsage\0")?,
                get_name: *lib.get(b"nvmlDeviceGetName\0")?,
                device,
                _lib: lib,
            })
        }
    }

    fn name(&self) -> Result<String> {
        let mut buf = [0u8; 96];
        nvml_ok(unsafe { (self.get_name)(self.device, buf.as_mut_ptr(), buf.len() as u32) }, "name")?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Ok(String::from_utf8_lossy(&buf[..len]).into_owned())
    }

    fn temperature(&self) -> Result<f32> {
        let mut t = 0;
        nvml_ok(unsafe { (self.get_temperature)(self.device, 0, &mut t) }, "temperature")?;
        Ok(t as f32)
    }

    fn utilization(&self) -> Result<f32> {
        let mut u = [0u32; 2];
        nvml_ok(unsafe { (self.get_utilization)(self.device, &mut u) }, "utilization")?;
        Ok(u[0] as f32)
    }

    fn power_w(&self) -> Result<f32> {
        let mut mw = 0;
        nvml_ok(unsafe { (self.get_power)(self.device, &mut mw) }, "power")?;
        Ok(mw as f32 / 1000.0)
    }
}
