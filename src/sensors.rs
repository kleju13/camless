//! Host sensors without CAM: GPU through NVML, CPU temperature and power through the
//! PawnIO driver that LibreHardwareMonitor and FanControl already install, CPU load
//! and memory through plain Win32 calls.

use anyhow::{bail, Context, Result};
use camless_core::status::{Sensor, Unit};
use libloading::Library;
use serde::Serialize;
use std::ffi::c_void;
use std::time::Instant;
use windows_sys::Win32::Foundation::{FILETIME, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetSystemTimes, ReleaseMutex, WaitForSingleObject,
};

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

pub struct Sensors {
    pub cpu_name: String,
    pub gpu_name: Option<String>,
    /// Why CPU temperature and power are off, when they are.
    pub cpu_error: Option<String>,
    nvml: Option<Nvml>,
    cpu: Option<AmdCpu>,
    times: Option<(u64, u64)>,
}

impl Sensors {
    /// `pawnio_module` is the signed `AMDFamily17.bin`, or why it could not be read;
    /// without it, or without admin rights, CPU temperature and power are absent.
    pub fn open(pawnio_module: std::result::Result<Vec<u8>, String>) -> Sensors {
        let nvml = Nvml::open().map_err(|e| eprintln!("gpu sensors off: {e:#}")).ok();
        let (cpu, cpu_error) = match pawnio_module.and_then(|m| AmdCpu::open(&m).map_err(|e| format!("{e:#}"))) {
            Ok(cpu) => (Some(cpu), None),
            Err(e) => {
                eprintln!("cpu temperature off: {e}");
                (None, Some(e))
            }
        };
        let gpu_name = nvml.as_ref().and_then(|n| n.name().ok());
        Sensors { cpu_name: cpu_name().unwrap_or_else(|| "CPU".into()), gpu_name, cpu_error, nvml, cpu, times: None }
    }

    pub fn has_cpu_temp(&self) -> bool {
        self.cpu.is_some()
    }

    pub fn read(&mut self) -> Readings {
        let mut r = Readings::default();
        if let Some(n) = &self.nvml {
            r.gpu_temp_c = n.temperature().ok();
            r.gpu_load_pct = n.utilization().ok();
            r.gpu_power_w = n.power_w().ok();
        }
        if let Some(c) = &mut self.cpu {
            r.cpu_temp_c = c.temperature().ok();
            r.cpu_power_w = c.package_power_w();
        }
        r.cpu_load_pct = self.cpu_load();
        let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut m) } != 0 {
            let gb = |b: u64| b as f32 / (1u64 << 30) as f32;
            r.ram_total_gb = Some(gb(m.ullTotalPhys));
            r.ram_used_gb = Some(gb(m.ullTotalPhys - m.ullAvailPhys));
            r.ram_used_pct = Some(m.dwMemoryLoad as f32);
        }
        r
    }

    fn cpu_load(&mut self) -> Option<f32> {
        let (mut idle, mut kernel, mut user) = (zero_ft(), zero_ft(), zero_ft());
        if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
            return None;
        }
        let (idle, busy_and_idle) = (ft(idle), ft(kernel) + ft(user));
        let prev = self.times.replace((idle, busy_and_idle));
        let (pi, pt) = prev?;
        let total = busy_and_idle.checked_sub(pt)?;
        (total > 0).then(|| 100.0 * (1.0 - (idle - pi) as f32 / total as f32))
    }
}

fn cpu_name() -> Option<String> {
    reg_string(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString")
}

/// A string value under HKEY_LOCAL_MACHINE.
pub fn reg_string(key: &str, value: &str) -> Option<String> {
    let mut buf = [0u16; 256];
    let mut size = std::mem::size_of_val(&buf) as u32;
    if reg_value(key, value, RRF_RT_REG_SZ, buf.as_mut_ptr().cast(), &mut size) != 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]).trim().to_string())
}

/// A DWORD value under HKEY_LOCAL_MACHINE.
pub fn reg_dword(key: &str, value: &str) -> Option<u32> {
    let mut v = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    (reg_value(key, value, RRF_RT_REG_DWORD, (&mut v as *mut u32).cast(), &mut size) == 0).then_some(v)
}

fn reg_value(key: &str, value: &str, kind: u32, out: *mut c_void, size: &mut u32) -> u32 {
    let key: Vec<u16> = key.encode_utf16().chain([0]).collect();
    let value: Vec<u16> = value.encode_utf16().chain([0]).collect();
    unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), value.as_ptr(), kind, std::ptr::null_mut(), out, size) }
}

fn zero_ft() -> FILETIME {
    FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }
}

fn ft(t: FILETIME) -> u64 {
    (t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64
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
            let lib = Library::new("nvml.dll").context("nvml.dll not found")?;
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

const SMN_THM_TCON_CUR_TMP: u64 = 0x0005_9800;
const MSR_PWR_UNIT: u64 = 0xc001_0299;
const MSR_PKG_ENERGY_STAT: u64 = 0xc001_029b;

type PawnExecute = unsafe extern "system" fn(
    HANDLE,
    *const u8,
    *const u64,
    usize,
    *mut u64,
    usize,
    *mut usize,
) -> i32;

/// AMD Zen temperature (SMN) and package energy (RAPL MSRs) through PawnIO.
struct AmdCpu {
    _lib: Library,
    handle: HANDLE,
    execute: PawnExecute,
    pci_mutex: HANDLE,
    energy_unit_j: Option<f64>,
    last_energy: Option<(u32, Instant)>,
}

unsafe impl Send for AmdCpu {}

impl AmdCpu {
    fn open(module: &[u8]) -> Result<AmdCpu> {
        unsafe {
            let lib = Library::new(r"C:\Program Files\PawnIO\PawnIOLib.dll")
                .context("PawnIO is not installed")?;
            let open: unsafe extern "system" fn(*mut HANDLE) -> i32 = *lib.get(b"pawnio_open\0")?;
            let load: unsafe extern "system" fn(HANDLE, *const u8, usize) -> i32 =
                *lib.get(b"pawnio_load\0")?;
            let execute: PawnExecute = *lib.get(b"pawnio_execute\0")?;
            let mut handle: HANDLE = std::ptr::null_mut();
            let hr = open(&mut handle);
            if hr < 0 {
                bail!("pawnio_open failed ({hr:#x}); camless needs administrator rights for this");
            }
            let hr = load(handle, module.as_ptr(), module.len());
            if hr < 0 {
                bail!("pawnio_load of AMDFamily17 failed ({hr:#x})");
            }
            // The PCI index/data pair is shared with HWiNFO, LHM and others; they all
            // serialize on this named mutex.
            let name: Vec<u16> = "Global\\Access_PCI\0".encode_utf16().collect();
            let pci_mutex = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            let mut cpu = AmdCpu {
                _lib: lib,
                handle,
                execute,
                pci_mutex,
                energy_unit_j: None,
                last_energy: None,
            };
            cpu.energy_unit_j = cpu
                .call("ioctl_read_msr", MSR_PWR_UNIT)
                .ok()
                .map(|v| 1.0 / (1u64 << ((v >> 8) & 0x1f)) as f64);
            cpu.temperature()?;
            Ok(cpu)
        }
    }

    fn call(&self, function: &str, arg: u64) -> Result<u64> {
        let name = format!("{function}\0");
        let mut out = 0u64;
        let mut written = 0usize;
        let hr = unsafe {
            (self.execute)(self.handle, name.as_ptr(), &arg, 1, &mut out, 1, &mut written)
        };
        if hr < 0 {
            bail!("{function}({arg:#x}) failed ({hr:#x})");
        }
        Ok(out)
    }

    fn temperature(&self) -> Result<f32> {
        let locked = !self.pci_mutex.is_null()
            && matches!(unsafe { WaitForSingleObject(self.pci_mutex, 20) }, WAIT_OBJECT_0 | WAIT_ABANDONED);
        let raw = self.call("ioctl_read_smn", SMN_THM_TCON_CUR_TMP);
        if locked {
            unsafe { ReleaseMutex(self.pci_mutex) };
        }
        let raw = raw? as u32;
        // Bits 31:21 are the temperature in 1/8 °C; bit 19 selects the -49 °C range.
        let offset = if raw & 0x8_0000 != 0 { -49.0 } else { 0.0 };
        Ok((raw >> 21) as f32 * 0.125 + offset)
    }

    fn package_power_w(&mut self) -> Option<f32> {
        let unit = self.energy_unit_j?;
        let now = Instant::now();
        let energy = self.call("ioctl_read_msr", MSR_PKG_ENERGY_STAT).ok()? as u32;
        let prev = self.last_energy.replace((energy, now))?;
        let dt = now.duration_since(prev.1).as_secs_f64();
        (dt > 0.05).then(|| (energy.wrapping_sub(prev.0) as f64 * unit / dt) as f32)
    }
}
