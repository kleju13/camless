//! Host sensors on Windows: CPU temperature and power through PawnIO, CPU load,
//! memory and the CPU's name through plain Win32 calls.

use anyhow::{bail, Context, Result};
use libloading::Library;
use std::ffi::c_void;
use std::time::Instant;
use windows_sys::Win32::Foundation::{FILETIME, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{CreateMutexW, GetSystemTimes, ReleaseMutex, WaitForSingleObject};

/// The library NVIDIA's driver installs.
pub const NVML: &str = "nvml.dll";

/// Without PawnIO and administrator rights there is no CPU temperature.
pub const NO_CPU_TEMP: &str = "no CPU temperature (needs PawnIO and administrator rights)";

/// CPU temperature and power, from the signed `AMDFamily17.bin`, or why there are none.
pub fn cpu(pawnio_module: std::result::Result<Vec<u8>, String>) -> std::result::Result<Cpu, String> {
    pawnio_module.and_then(|m| AmdCpu::open(&m).map_err(|e| format!("{e:#}")))
}

pub type Cpu = AmdCpu;

/// Idle time and all time, in 100 ns, since the PC started.
pub fn cpu_times() -> Option<(u64, u64)> {
    let (mut idle, mut kernel, mut user) = (zero_ft(), zero_ft(), zero_ft());
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return None;
    }
    Some((ft(idle), ft(kernel) + ft(user)))
}

/// Memory: total and in use in bytes, and the share in use in percent.
pub fn memory() -> Option<(u64, u64, f32)> {
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 {
        return None;
    }
    Some((m.ullTotalPhys, m.ullTotalPhys - m.ullAvailPhys, m.dwMemoryLoad as f32))
}

pub fn cpu_name() -> Option<String> {
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
pub struct AmdCpu {
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

    pub fn temperature(&self) -> Result<f32> {
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

    pub fn package_power_w(&mut self) -> Option<f32> {
        let unit = self.energy_unit_j?;
        let now = Instant::now();
        let energy = self.call("ioctl_read_msr", MSR_PKG_ENERGY_STAT).ok()? as u32;
        let prev = self.last_energy.replace((energy, now))?;
        let dt = now.duration_since(prev.1).as_secs_f64();
        (dt > 0.05).then(|| (energy.wrapping_sub(prev.0) as f64 * unit / dt) as f32)
    }
}
