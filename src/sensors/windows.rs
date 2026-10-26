//! Host sensors on Windows: CPU temperature and power through PawnIO, CPU load,
//! memory and the CPU's name through plain Win32 calls, and a graphics card NVML
//! does not read through what Windows keeps for Task Manager, with an AMD card's
//! power through AMD's ADL.

use anyhow::{bail, Context, Result};
use libloading::Library;
use std::ffi::c_void;
use camless_core::status::{PowerSource, PowerStats};
use std::time::{Duration, Instant};
use windows_sys::Wdk::Graphics::Direct3D::{
    D3DKMTCloseAdapter, D3DKMTEnumAdapters2, D3DKMTQueryAdapterInfo, D3DKMTQueryStatistics, D3DKMT_ADAPTERINFO,
    D3DKMT_ADAPTERREGISTRYINFO, D3DKMT_ADAPTERTYPE, D3DKMT_ADAPTER_PERFDATA, D3DKMT_CLOSEADAPTER, D3DKMT_ENUMADAPTERS2,
    D3DKMT_ADAPTERADDRESS, D3DKMT_QUERYADAPTERINFO, D3DKMT_QUERYSTATISTICS, D3DKMT_QUERYSTATISTICS_ADAPTER, D3DKMT_QUERYSTATISTICS_NODE,
    D3DKMT_QUERYSTATISTICS_TYPE, D3DKMT_SEGMENTSIZEINFO, KMTQAITYPE_ADAPTERPERFDATA, KMTQAITYPE_ADAPTERREGISTRYINFO,
    KMTQAITYPE_ADAPTERADDRESS, KMTQAITYPE_ADAPTERTYPE, KMTQAITYPE_GETSEGMENTSIZE, KMTQUERYADAPTERINFOTYPE,
};
use windows_sys::Win32::Foundation::{FILETIME, HANDLE, LUID, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{CreateMutexW, GetSystemTimes, ReleaseMutex, WaitForSingleObject};

/// The library NVIDIA's driver installs.
pub const NVML: &str = "nvml.dll";

/// Without PawnIO and administrator rights there is no CPU temperature.
pub const NO_CPU_TEMP: &str = "no CPU temperature (needs PawnIO and administrator rights)";

/// CPU temperature and power, from the signed PawnIO module for this CPU, or why there
/// are none.
pub fn cpu(pawnio_module: std::result::Result<Vec<u8>, String>) -> std::result::Result<Cpu, String> {
    pawnio_module.and_then(|m| PawnCpu::open(&m, maker()).map_err(|e| format!("{e:#}")))
}

pub type Cpu = PawnCpu;

/// The signed PawnIO module for this CPU, which camless reads next to camless.exe.
/// Each refuses to load on the other maker's CPUs.
pub fn pawnio_module_file() -> &'static str {
    maker().module_file()
}

const CPU_KEY: &str = r"HARDWARE\DESCRIPTION\System\CentralProcessor\0";

#[derive(Clone, Copy)]
enum Maker {
    Amd,
    Intel,
}

impl Maker {
    fn module_file(self) -> &'static str {
        match self {
            Maker::Amd => "AMDFamily17.bin",
            Maker::Intel => "IntelMSR.bin",
        }
    }
}

/// Any CPU but Intel's is taken for AMD's, which only `AMDFamily17.bin` reads.
fn maker() -> Maker {
    match reg_string(CPU_KEY, "VendorIdentifier").as_deref() {
        Some("GenuineIntel") => Maker::Intel,
        _ => Maker::Amd,
    }
}

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
    reg_string(CPU_KEY, "ProcessorNameString")
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

// AMD Zen, through AMDFamily17.bin.
const SMN_THM_TCON_CUR_TMP: u64 = 0x0005_9800;
const MSR_PWR_UNIT: u64 = 0xc001_0299;
const MSR_PKG_ENERGY_STAT: u64 = 0xc001_029b;
// Intel, through IntelMSR.bin, which reads these and a few more.
const IA32_TEMPERATURE_TARGET: u64 = 0x1a2;
const IA32_PACKAGE_THERM_STATUS: u64 = 0x1b1;
const MSR_RAPL_POWER_UNIT: u64 = 0x606;
const MSR_PKG_ENERGY_STATUS: u64 = 0x611;

type PawnExecute = unsafe extern "system" fn(
    HANDLE,
    *const u8,
    *const u64,
    usize,
    *mut u64,
    usize,
    *mut usize,
) -> i32;

/// A signed module loaded into PawnIO.
struct Pawn {
    _lib: Library,
    handle: HANDLE,
    execute: PawnExecute,
}

impl Pawn {
    /// `file` only names the module in errors.
    fn load(module: &[u8], file: &str) -> Result<Pawn> {
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
                bail!("pawnio_load of {file} failed ({hr:#x})");
            }
            Ok(Pawn { _lib: lib, handle, execute })
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
}

/// How the CPU's temperature is read.
enum Thermal {
    /// AMD Zen's Tctl, over SMN. The PCI index/data pair is shared with HWiNFO, LHM
    /// and others; they all serialize on the named mutex.
    Amd { pci_mutex: HANDLE },
    /// Intel's package sensor, which counts down from TjMax, as LibreHardwareMonitor
    /// and Linux's coretemp read it.
    Intel { tj_max: f32 },
}

/// CPU temperature and package energy (RAPL MSRs) through PawnIO.
pub struct PawnCpu {
    pawn: Pawn,
    thermal: Thermal,
    energy_msr: u64,
    energy_unit_j: Option<f64>,
    last_energy: Option<(u32, Instant)>,
}

unsafe impl Send for PawnCpu {}

impl PawnCpu {
    fn open(module: &[u8], maker: Maker) -> Result<PawnCpu> {
        let pawn = Pawn::load(module, maker.module_file())?;
        let (unit_msr, energy_msr, thermal) = match maker {
            Maker::Amd => {
                let name: Vec<u16> = "Global\\Access_PCI\0".encode_utf16().collect();
                let pci_mutex = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
                (MSR_PWR_UNIT, MSR_PKG_ENERGY_STAT, Thermal::Amd { pci_mutex })
            }
            Maker::Intel => {
                let target = pawn.call("ioctl_read_msr", IA32_TEMPERATURE_TARGET)?;
                let tj_max = tj_max(target).context("the CPU reports no TjMax")?;
                (MSR_RAPL_POWER_UNIT, MSR_PKG_ENERGY_STATUS, Thermal::Intel { tj_max })
            }
        };
        let energy_unit_j = pawn.call("ioctl_read_msr", unit_msr).ok().map(energy_unit_j);
        let cpu = PawnCpu { pawn, thermal, energy_msr, energy_unit_j, last_energy: None };
        cpu.temperature()?;
        Ok(cpu)
    }

    pub fn temperature(&self) -> Result<f32> {
        match self.thermal {
            Thermal::Amd { pci_mutex } => {
                let locked = !pci_mutex.is_null()
                    && matches!(unsafe { WaitForSingleObject(pci_mutex, 20) }, WAIT_OBJECT_0 | WAIT_ABANDONED);
                let raw = self.pawn.call("ioctl_read_smn", SMN_THM_TCON_CUR_TMP);
                if locked {
                    unsafe { ReleaseMutex(pci_mutex) };
                }
                Ok(amd_tctl(raw? as u32))
            }
            Thermal::Intel { tj_max } => {
                let status = self.pawn.call("ioctl_read_msr", IA32_PACKAGE_THERM_STATUS)?;
                intel_package(status, tj_max).context("the CPU package's temperature reading is not valid")
            }
        }
    }

    pub fn package_power_w(&mut self) -> Option<f32> {
        let unit = self.energy_unit_j?;
        let now = Instant::now();
        let energy = self.pawn.call("ioctl_read_msr", self.energy_msr).ok()? as u32;
        let prev = self.last_energy.replace((energy, now))?;
        let dt = now.duration_since(prev.1).as_secs_f64();
        (dt > 0.05).then(|| (energy.wrapping_sub(prev.0) as f64 * unit / dt) as f32)
    }
}

/// Tctl from THM_TCON_CUR_TMP: bits 31:21 are the temperature in 1/8 °C; bit 19
/// selects the -49 °C range.
fn amd_tctl(raw: u32) -> f32 {
    let offset = if raw & 0x8_0000 != 0 { -49.0 } else { 0.0 };
    (raw >> 21) as f32 * 0.125 + offset
}

/// TjMax in °C from IA32_TEMPERATURE_TARGET's bits 23:16, without the TCC offset in
/// 29:24; None where they are 0.
fn tj_max(target: u64) -> Option<f32> {
    let t = (target >> 16) & 0xff;
    (t != 0).then_some(t as f32)
}

/// The package's temperature from IA32_PACKAGE_THERM_STATUS: bits 22:16 are how many
/// °C it is below TjMax, when bit 31 says the reading is valid.
fn intel_package(status: u64, tj_max: f32) -> Option<f32> {
    (status & 0x8000_0000 != 0).then(|| tj_max - ((status >> 16) & 0x7f) as f32)
}

/// Joules per count of the package's energy counter, from the RAPL power unit's bits
/// 12:8, the same on AMD and Intel.
fn energy_unit_j(unit: u64) -> f64 {
    1.0 / (1u64 << ((unit >> 8) & 0x1f)) as f64
}

/// `D3DKMT_ADAPTERTYPE`'s bits (`d3dkmthk.h`).
const RENDER_SUPPORTED: u32 = 1 << 0;
const SOFTWARE_DEVICE: u32 = 1 << 2;

/// A graphics card of any brand, read through D3DKMT in `gdi32.dll` as Task Manager
/// reads it: the temperature its driver reports, from WDDM 2.4 on, and the busiest
/// engine's share of the time. Windows has power only as a share of the card's
/// limit, so watts come from AMD's ADL, on an AMD card alone. Temperature and load
/// match AMD's Adrenalin on an RX 9070 XT (camless #42).
pub struct WddmGpu {
    pub name: String,
    adapter: u32,
    luid: LUID,
    nodes: u32,
    power: Option<AdlPower>,
    /// Why there is no power in watts, when there is none.
    pub power_error: Option<String>,
    /// Each engine's running time at the last reading, and when. It counts in 100 ns,
    /// though `d3dkmthk.h` says microseconds: in 100 ns it matches Task Manager's
    /// engine counters, idle and loaded, as LibreHardwareMonitor reads it.
    last: Option<(Vec<Option<i64>>, Instant)>,
}

/// What a card is picked by: its `D3DKMT_ADAPTERTYPE` bits and its own memory.
#[derive(Clone, Copy)]
struct Candidate {
    kind: u32,
    dedicated: u64,
}

/// The card that draws, not Microsoft's software renderer or a display-only adapter
/// such as a virtual display's: the one with the most memory of its own, as on
/// Linux, so a graphics card wins over the CPU's.
fn pick(candidates: &[Candidate]) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.kind & RENDER_SUPPORTED != 0 && c.kind & SOFTWARE_DEVICE == 0)
        .max_by_key(|&(i, c)| (c.dedicated, std::cmp::Reverse(i)))
        .map(|(i, _)| i)
}

/// The busiest engine's share of `wall` (in 100 ns), in percent, from two readings of
/// every engine's running time. An engine missing from either is left out.
fn busiest(before: &[Option<i64>], after: &[Option<i64>], wall: f64) -> Option<f32> {
    if wall <= 0.0 {
        return None;
    }
    before
        .iter()
        .zip(after)
        .filter_map(|(b, a)| Some((*a)? - (*b)?))
        .map(|ran| (ran.max(0) as f64 / wall * 100.0).min(100.0) as f32)
        .reduce(f32::max)
}

fn query<T>(adapter: u32, kind: KMTQUERYADAPTERINFOTYPE, out: &mut T) -> bool {
    let mut q = D3DKMT_QUERYADAPTERINFO {
        hAdapter: adapter,
        Type: kind,
        pPrivateDriverData: (out as *mut T).cast(),
        PrivateDriverDataSize: size_of::<T>() as u32,
    };
    unsafe { D3DKMTQueryAdapterInfo(&mut q) >= 0 }
}

fn statistics(luid: LUID, kind: D3DKMT_QUERYSTATISTICS_TYPE, node: u32) -> Option<D3DKMT_QUERYSTATISTICS> {
    let mut q = D3DKMT_QUERYSTATISTICS { Type: kind, AdapterLuid: luid, ..Default::default() };
    q.Anonymous.QueryNode.NodeId = node;
    // It fills in QueryResult, though its parameter is const.
    (unsafe { D3DKMTQueryStatistics(&mut q) } >= 0).then_some(q)
}

fn close(adapter: u32) {
    unsafe { D3DKMTCloseAdapter(&D3DKMT_CLOSEADAPTER { hAdapter: adapter }) };
}

fn adapter_name(adapter: u32) -> Option<String> {
    let mut info = D3DKMT_ADAPTERREGISTRYINFO::default();
    if !query(adapter, KMTQAITYPE_ADAPTERREGISTRYINFO, &mut info) {
        return None;
    }
    let s = &info.AdapterString;
    let name = String::from_utf16_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())]);
    Some(name.trim().to_string()).filter(|n| !n.is_empty())
}

impl WddmGpu {
    pub fn open() -> Result<WddmGpu> {
        // Once for how many there are, once for them.
        let mut list = D3DKMT_ENUMADAPTERS2::default();
        let status = unsafe { D3DKMTEnumAdapters2(&mut list) };
        if status < 0 || list.NumAdapters == 0 {
            bail!("Windows lists no graphics adapters ({status:#x})");
        }
        let mut adapters = vec![D3DKMT_ADAPTERINFO::default(); list.NumAdapters as usize];
        list.pAdapters = adapters.as_mut_ptr();
        let status = unsafe { D3DKMTEnumAdapters2(&mut list) };
        if status < 0 {
            bail!("D3DKMTEnumAdapters2 failed ({status:#x})");
        }
        adapters.truncate(list.NumAdapters as usize);
        let candidates: Vec<Candidate> = adapters
            .iter()
            .map(|a| {
                let mut kind = D3DKMT_ADAPTERTYPE::default();
                let mut size = D3DKMT_SEGMENTSIZEINFO::default();
                let kind = if query(a.hAdapter, KMTQAITYPE_ADAPTERTYPE, &mut kind) { unsafe { kind.Anonymous.Value } } else { 0 };
                let dedicated = if query(a.hAdapter, KMTQAITYPE_GETSEGMENTSIZE, &mut size) { size.DedicatedVideoMemorySize } else { 0 };
                Candidate { kind, dedicated }
            })
            .collect();
        let chosen = pick(&candidates);
        // Listing them opened every adapter; all but the one kept are closed again.
        for (i, a) in adapters.iter().enumerate() {
            if Some(i) != chosen {
                close(a.hAdapter);
            }
        }
        let a = adapters[chosen.context("no graphics card among the adapters Windows lists")?];
        let name = adapter_name(a.hAdapter).unwrap_or_else(|| "GPU".into());
        let mut gpu = WddmGpu { name, adapter: a.hAdapter, luid: a.AdapterLuid, nodes: 0, power: None, power_error: None, last: None };
        gpu.nodes = statistics(a.AdapterLuid, D3DKMT_QUERYSTATISTICS_ADAPTER, 0)
            .map_or(0, |s| unsafe { s.QueryResult.AdapterInformation.NodeCount });
        if gpu.nodes == 0 && gpu.temperature().is_none() {
            bail!("Windows has no readings for {}", gpu.name);
        }
        let mut address = D3DKMT_ADAPTERADDRESS::default();
        match query(gpu.adapter, KMTQAITYPE_ADAPTERADDRESS, &mut address)
            .then_some(address)
            .context("Windows gives no PCI address for the card")
            .and_then(AdlPower::open)
        {
            Ok(p) => gpu.power = Some(p),
            Err(e) => {
                eprintln!("gpu power off: {e:#}");
                gpu.power_error = Some(format!("{e:#}"));
            }
        }
        Ok(gpu)
    }

    pub fn power_w(&mut self) -> Option<f32> {
        self.power.as_mut()?.watts()
    }

    /// The ways ADL's power was read, up to the one shown, and their last minute.
    pub fn power_sources(&self) -> Vec<PowerSource> {
        self.power.as_ref().map_or_else(Vec::new, AdlPower::sources)
    }

    /// None when the driver reports none, which it does as 0.
    pub fn temperature(&self) -> Option<f32> {
        // PhysicalAdapterIndex 0: the card itself, not one linked to it.
        let mut perf = D3DKMT_ADAPTER_PERFDATA::default();
        if !query(self.adapter, KMTQAITYPE_ADAPTERPERFDATA, &mut perf) {
            return None;
        }
        (perf.Temperature > 0).then(|| perf.Temperature as f32 / 10.0)
    }

    /// The busiest engine since the last reading, as Task Manager's GPU column; None
    /// the first time.
    pub fn utilization(&mut self) -> Option<f32> {
        let now = Instant::now();
        let times: Vec<Option<i64>> = (0..self.nodes)
            .map(|n| {
                statistics(self.luid, D3DKMT_QUERYSTATISTICS_NODE, n)
                    .map(|s| unsafe { s.QueryResult.NodeInformation.GlobalInformation.RunningTime })
            })
            .collect();
        let (before, then) = self.last.replace((times.clone(), now))?;
        busiest(&before, &times, now.duration_since(then).as_secs_f64() * 1e7)
    }
}

/// `ADL_PMLOG_SENSORS` in `adl_defines.h`: what the card draws, all of it, which ADL
/// has from RDNA 3 on, and the chip's own draw, which cards before have.
const ADL_PMLOG_BOARD_POWER: usize = 73;
const ADL_PMLOG_ASIC_POWER: usize = 23;
/// How often the driver fills a log camless starts, as LibreHardwareMonitor and
/// AMD's samples ask.
const LOG_EVERY_MS: u32 = 1000;
/// A log the driver has not filled for this long reads as nothing.
const LOG_STALE: Duration = Duration::from_secs(5);
/// How far back each source's readings go in the report.
const WINDOW: Duration = Duration::from_secs(60);

/// `AdapterInfo` in `adl_structures.h`, as built for Windows.
#[repr(C)]
struct AdlAdapterInfo {
    size: i32,
    index: i32,
    udid: [u8; 256],
    bus: i32,
    device: i32,
    function: i32,
    vendor: i32,
    name: [u8; 256],
    display_name: [u8; 256],
    present: i32,
    exist: i32,
    driver_path: [u8; 256],
    driver_path_ext: [u8; 256],
    pnp: [u8; 256],
    os_display_index: i32,
}

/// `ADLPMLogDataOutput`: a `supported`, `value` pair for each of 256 sensors.
#[repr(C)]
struct AdlPmLog {
    size: i32,
    sensors: [[i32; 2]; 256],
}

/// `ADLPMLogSupportInfo`: the sensors the driver can log, ended by 0.
#[repr(C)]
struct AdlLogSupport {
    sensors: [u16; 256],
    reserved: [i32; 16],
}

/// `ADLPMLogStartInput`: the sensors to log, ended by 0, and how often.
#[repr(C)]
struct AdlLogStart {
    sensors: [u16; 256],
    sample_rate_ms: u32,
    reserved: [i32; 15],
}

/// `ADLPMLogStartOutput`: where the driver keeps the log.
#[repr(C)]
struct AdlLogStarted {
    address: *const AdlLogData,
    reserved: [i32; 14],
}

/// `ADLPMLogData`: the log the driver keeps filling, a `sensor`, `value` pair for each
/// sensor logged, ended by sensor 0.
#[repr(C)]
struct AdlLogData {
    version: u32,
    sample_rate_ms: u32,
    updated: u64,
    values: [[u32; 2]; 256],
    reserved: [u32; 256],
}

type AdlQuery = unsafe extern "C" fn(*mut c_void, i32, *mut AdlPmLog) -> i32;
type DeviceCreate = unsafe extern "C" fn(*mut c_void, i32, *mut u32) -> i32;
type DeviceDestroy = unsafe extern "C" fn(*mut c_void, u32) -> i32;
type LogSupport = unsafe extern "C" fn(*mut c_void, i32, *mut AdlLogSupport) -> i32;
type LogStart = unsafe extern "C" fn(*mut c_void, i32, *mut AdlLogStart, *mut AdlLogStarted, u32) -> i32;
type LogStop = unsafe extern "C" fn(*mut c_void, i32, u32) -> i32;

/// Some ADL calls hand back memory they get from this; none camless makes does.
extern "C" fn adl_alloc(size: i32) -> *mut c_void {
    extern "C" {
        fn malloc(size: usize) -> *mut c_void;
    }
    unsafe { malloc(size.max(0) as usize) }
}

/// Only `ADL_OK`, as AMD's samples and LibreHardwareMonitor take it from the log calls:
/// a warning may come without the device or log.
fn adl_ok(status: i32, call: &str) -> std::result::Result<(), String> {
    if status != 0 {
        return Err(format!("{call} failed ({status})"));
    }
    Ok(())
}

/// What a reading came from, for the report.
fn sensor_name(sensor: usize) -> &'static str {
    if sensor == ADL_PMLOG_BOARD_POWER {
        "board power (73)"
    } else {
        "chip power (23)"
    }
}

/// Board power, or the chip's on a card without it, in watts, as LibreHardwareMonitor
/// reads them, with the sensor it came from. None when the card has neither.
fn pmlog_watts(log: &AdlPmLog) -> Option<(usize, f32)> {
    [ADL_PMLOG_BOARD_POWER, ADL_PMLOG_ASIC_POWER]
        .into_iter()
        .find(|&i| log.sensors[i][0] != 0)
        .and_then(|i| (log.sensors[i][1] >= 0).then_some((i, log.sensors[i][1] as f32)))
}

/// The same from a log the driver keeps, whose pairs end at sensor 0.
fn log_watts(values: &[[u32; 2]]) -> Option<(usize, f32)> {
    let logged = |sensor: usize| values.iter().take_while(|v| v[0] != 0).find(|v| v[0] as usize == sensor);
    [ADL_PMLOG_BOARD_POWER, ADL_PMLOG_ASIC_POWER]
        .into_iter()
        .find_map(|i| Some((i, logged(i)?[1])))
        .and_then(|(i, w)| (w as i32 >= 0).then_some((i, w as f32)))
}

/// Whether a log's time stamp moved within `LOG_STALE`. `seen` is the last stamp and
/// when it changed.
fn fresh(seen: &mut Option<(u64, Instant)>, updated: u64, now: Instant) -> bool {
    if updated == 0 {
        return false;
    }
    match *seen {
        Some((stamp, since)) if stamp == updated => now.duration_since(since) < LOG_STALE,
        _ => {
            *seen = Some((updated, now));
            true
        }
    }
}

/// A source's readings of the last `WINDOW`, with when each was taken, for the report.
#[derive(Default)]
struct Window(std::collections::VecDeque<(Instant, f32)>);

impl Window {
    /// Drops the readings older than `WINDOW`, so a source that stopped reading
    /// empties within a minute.
    fn expire(&mut self, now: Instant) {
        while self.0.front().is_some_and(|&(at, _)| now.duration_since(at) > WINDOW) {
            self.0.pop_front();
        }
    }

    fn push(&mut self, now: Instant, watts: f32) {
        self.expire(now);
        self.0.push_back((now, watts));
    }

    fn stats(&self) -> Option<PowerStats> {
        let last = self.0.back()?.1;
        let watts = || self.0.iter().map(|&(_, w)| w);
        let n = self.0.len() as f32;
        let mean = watts().sum::<f32>() / n;
        let sd = (watts().map(|w| (w - mean).powi(2)).sum::<f32>() / n).sqrt();
        let min = watts().fold(f32::INFINITY, f32::min);
        let max = watts().fold(f32::NEG_INFINITY, f32::max);
        Some(PowerStats { last, mean, min, max, sd, readings: self.0.len() as u32 })
    }
}

/// One way ADL gives the card's power.
enum Reader {
    /// `ADL2_Adapter_PMLog_Start`: a log the driver fills once a second in memory it
    /// shares, as LibreHardwareMonitor reads RDNA 3 since its #1308. That fixed power
    /// jumping by 100 W within a second while AMD's overlay held still (its #1285).
    Log { device: u32, data: *const AdlLogData, stop: LogStop, seen: Option<(u64, Instant)> },
    /// `ADL2_New_QueryPMLogData_Get`, which ADL 18.1 deprecates: on an RX 9070 XT it
    /// jumped between 60 and 470 W each second under a steady 260 W (camless #42).
    Snapshot(AdlQuery),
}

impl Reader {
    /// Watts, the sensor they came from, and how often the driver fills the log.
    fn read(&mut self, context: *mut c_void, index: i32) -> Option<(usize, f32, Option<u32>)> {
        match self {
            Reader::Log { data, seen, .. } => {
                // A copy, as LibreHardwareMonitor takes one: the driver keeps writing
                // it, a whole number at a time, and ADL promises no alignment.
                let log = unsafe { std::ptr::read_unaligned(*data) };
                if !fresh(seen, log.updated, Instant::now()) {
                    return None;
                }
                let (sensor, watts) = log_watts(&log.values)?;
                Some((sensor, watts, Some(log.sample_rate_ms)))
            }
            Reader::Snapshot(query) => {
                let (sensor, watts) = pmlog_watts(&snapshot(*query, context, index).ok()?)?;
                Some((sensor, watts, None))
            }
        }
    }
}

struct Source {
    name: &'static str,
    reader: std::result::Result<Reader, String>,
    detail: Option<String>,
    shown: bool,
    /// Readings in a row that gave nothing.
    missed: u32,
    window: Window,
}

impl Source {
    fn new(name: &'static str, reader: std::result::Result<Reader, String>) -> Source {
        Source { name, reader, detail: None, shown: false, missed: 0, window: Window::default() }
    }
}

/// `ADL2_New_QueryPMLogData_Get`'s log, or ADL's status when it gives none.
fn snapshot(query: AdlQuery, context: *mut c_void, index: i32) -> std::result::Result<AdlPmLog, i32> {
    let mut log: AdlPmLog = unsafe { std::mem::zeroed() };
    let status = unsafe { query(context, index, &mut log) };
    if status < 0 {
        return Err(status);
    }
    Ok(log)
}

/// An AMD card's power in watts, through AMD's ADL (`atiadlxx.dll`, which comes with
/// the driver). ADL lists each card once for each display output, by its PCI bus and
/// device; the card is the one at D3DKMT's address.
///
/// The power comes from the log the driver keeps, as LibreHardwareMonitor reads it:
/// on an RX 9070 XT it followed Adrenalin's Total Board Power within a few watts
/// (camless #42). The snapshot 0.5.0-beta.6 read is read only while the log gives
/// nothing: a driver without it, or a log it stopped filling.
///
/// 0.5.0-beta.7 also read `ADL2_Overdrive8_PMLog_ShareMemory_Read`, ADL 18.1's
/// replacement for the snapshot. On that card its minute in the report matched the
/// log's to the watt, while it had the driver keep a second log, and its output has
/// no time stamp to show it frozen, so camless dropped it.
struct AdlPower {
    lib: Library,
    context: *mut c_void,
    index: i32,
    destroy: unsafe extern "C" fn(*mut c_void) -> i32,
    sources: Vec<Source>,
}

// One thread reads the sensors at a time, and the context and logs are this reader's own.
unsafe impl Send for AdlPower {}

impl AdlPower {
    fn open(address: D3DKMT_ADAPTERADDRESS) -> Result<AdlPower> {
        // A full path, so no atiadlxx.dll beside camless loads instead.
        let path = std::env::var_os("SystemRoot")
            .map(|r| std::path::PathBuf::from(r).join("System32").join("atiadlxx.dll"))
            .filter(|p| p.exists())
            .context("atiadlxx.dll not found: no AMD driver")?;
        unsafe {
            let lib = Library::new(&path).context("atiadlxx.dll does not load")?;
            let create: unsafe extern "C" fn(extern "C" fn(i32) -> *mut c_void, i32, *mut *mut c_void) -> i32 =
                *lib.get(b"ADL2_Main_Control_Create\0")?;
            let destroy: unsafe extern "C" fn(*mut c_void) -> i32 = *lib.get(b"ADL2_Main_Control_Destroy\0")?;
            let count: unsafe extern "C" fn(*mut c_void, *mut i32) -> i32 = *lib.get(b"ADL2_Adapter_NumberOfAdapters_Get\0")?;
            let info: unsafe extern "C" fn(*mut c_void, *mut AdlAdapterInfo, i32) -> i32 =
                *lib.get(b"ADL2_Adapter_AdapterInfo_Get\0")?;
            let query: Option<AdlQuery> = lib.get(b"ADL2_New_QueryPMLogData_Get\0").ok().map(|f| *f);
            let mut context = std::ptr::null_mut();
            // 1: only the adapters connected now, as LibreHardwareMonitor asks.
            let status = create(adl_alloc, 1, &mut context);
            if status < 0 {
                bail!("ADL2_Main_Control_Create failed ({status})");
            }
            // Stops the logs and destroys the context on every way out from here.
            let mut adl = AdlPower { lib, context, index: -1, destroy, sources: Vec::new() };
            let mut n = 0;
            let status = count(context, &mut n);
            if status < 0 || n <= 0 {
                bail!("ADL lists no adapters ({status})");
            }
            let mut list: Vec<AdlAdapterInfo> = (0..n).map(|_| std::mem::zeroed()).collect();
            let status = info(context, list.as_mut_ptr(), n * size_of::<AdlAdapterInfo>() as i32);
            if status < 0 {
                bail!("ADL2_Adapter_AdapterInfo_Get failed ({status})");
            }
            let at: Vec<i32> = list
                .iter()
                .filter(|a| a.bus as u32 == address.BusNumber && a.device as u32 == address.DeviceNumber)
                .map(|a| a.index)
                .collect();
            let Some(&first) = at.first() else {
                bail!("ADL does not list the card (PCI bus {}, device {})", address.BusNumber, address.DeviceNumber);
            };
            // The first of the card's entries with power in a snapshot, as 0.5.0-beta.6
            // picked it, else its first.
            let has_power = |i: i32| query.is_some_and(|q| snapshot(q, context, i).is_ok_and(|l| pmlog_watts(&l).is_some()));
            adl.index = at.iter().copied().find(|&i| has_power(i)).unwrap_or(first);
            let snapshot_reader = match query {
                None => Err("this AMD driver has no ADL2_New_QueryPMLogData_Get".to_string()),
                Some(q) => match snapshot(q, context, adl.index) {
                    Ok(log) if pmlog_watts(&log).is_some() => Ok(Reader::Snapshot(q)),
                    Ok(_) => Err("ADL has neither board nor chip power for the card".to_string()),
                    Err(status) => Err(format!("ADL2_New_QueryPMLogData_Get failed ({status})")),
                },
            };
            adl.sources = vec![
                Source::new("ADL2_Adapter_PMLog_Start", adl.start_log()),
                Source::new("ADL2_New_QueryPMLogData_Get", snapshot_reader),
            ];
            let errors: Vec<&str> = adl.sources.iter().filter_map(|s| s.reader.as_ref().err().map(String::as_str)).collect();
            if errors.len() == adl.sources.len() {
                bail!(errors.join("; "));
            }
            Ok(adl)
        }
    }

    fn symbol<T: Copy>(&self, name: &str) -> std::result::Result<T, String> {
        unsafe { self.lib.get::<T>(format!("{name}\0").as_bytes()) }
            .map(|f| *f)
            .map_err(|_| format!("this AMD driver has no {name}"))
    }

    /// The D3DKMT device a log is kept for.
    fn device(&self) -> std::result::Result<u32, String> {
        let create: DeviceCreate = self.symbol("ADL2_Device_PMLog_Device_Create")?;
        let mut device = 0;
        adl_ok(unsafe { create(self.context, self.index, &mut device) }, "ADL2_Device_PMLog_Device_Create")?;
        Ok(device)
    }

    fn drop_device(&self, device: u32) {
        if let Ok(destroy) = self.symbol::<DeviceDestroy>("ADL2_Device_PMLog_Device_Destroy") {
            unsafe { destroy(self.context, device) };
        }
    }

    /// As LibreHardwareMonitor and AMD's PMLog sample start it: every sensor the
    /// driver can log, once a second.
    fn start_log(&self) -> std::result::Result<Reader, String> {
        let support: LogSupport = self.symbol("ADL2_Adapter_PMLog_Support_Get")?;
        let start: LogStart = self.symbol("ADL2_Adapter_PMLog_Start")?;
        let stop: LogStop = self.symbol("ADL2_Adapter_PMLog_Stop")?;
        let mut supported: AdlLogSupport = unsafe { std::mem::zeroed() };
        adl_ok(unsafe { support(self.context, self.index, &mut supported) }, "ADL2_Adapter_PMLog_Support_Get")?;
        let listed: Vec<usize> = supported.sensors.iter().take_while(|&&s| s != 0).map(|&s| s as usize).collect();
        if !listed.contains(&ADL_PMLOG_BOARD_POWER) && !listed.contains(&ADL_PMLOG_ASIC_POWER) {
            return Err("the driver logs neither board nor chip power".into());
        }
        let device = self.device()?;
        let mut input = AdlLogStart { sensors: supported.sensors, sample_rate_ms: LOG_EVERY_MS, reserved: [0; 15] };
        let mut out: AdlLogStarted = unsafe { std::mem::zeroed() };
        let started = adl_ok(unsafe { start(self.context, self.index, &mut input, &mut out, device) }, "ADL2_Adapter_PMLog_Start")
            .and_then(|()| {
                if out.address.is_null() {
                    unsafe { stop(self.context, self.index, device) };
                    return Err("ADL2_Adapter_PMLog_Start gave no log".into());
                }
                Ok(())
            });
        if let Err(e) = started {
            self.drop_device(device);
            return Err(e);
        }
        Ok(Reader::Log { device, data: out.address, stop, seen: None })
    }

    fn watts(&mut self) -> Option<f32> {
        let (context, index) = (self.context, self.index);
        first_reading(&mut self.sources, Instant::now(), |r| r.read(context, index))
    }

    fn sources(&self) -> Vec<PowerSource> {
        listed(&self.sources)
            .iter()
            .map(|s| PowerSource {
                name: s.name.into(),
                detail: s.detail.clone(),
                error: s.reader.as_ref().err().cloned(),
                shown: s.shown,
                missed: s.missed,
                stats: s.window.stats(),
            })
            .collect()
    }
}

/// Reads the sources in order until one gives a reading, and gives its watts. The
/// ones after it are not read.
fn first_reading(
    sources: &mut [Source],
    now: Instant,
    mut read: impl FnMut(&mut Reader) -> Option<(usize, f32, Option<u32>)>,
) -> Option<f32> {
    let mut shown = None;
    for s in sources {
        s.window.expire(now);
        s.shown = false;
        if shown.is_some() {
            continue;
        }
        let Ok(reader) = &mut s.reader else { continue };
        let reading = read(reader);
        s.missed = if reading.is_some() { 0 } else { s.missed.saturating_add(1) };
        if let Some((sensor, watts, every)) = reading {
            s.window.push(now, watts);
            s.detail = Some(match every {
                Some(ms) => format!("{}, filled every {ms} ms", sensor_name(sensor)),
                None => sensor_name(sensor).to_string(),
            });
            s.shown = true;
            shown = Some(watts);
        }
    }
    shown
}

/// The sources up to the one GPU power shows, or all of them when it shows none:
/// each one before it says why it gave nothing.
fn listed(sources: &[Source]) -> &[Source] {
    let upto = sources.iter().position(|s| s.shown).map_or(sources.len(), |i| i + 1);
    &sources[..upto]
}

impl Drop for AdlPower {
    fn drop(&mut self) {
        for s in std::mem::take(&mut self.sources) {
            if let Ok(Reader::Log { device, stop, .. }) = s.reader {
                unsafe { stop(self.context, self.index, device) };
                self.drop_device(device);
            }
        }
        unsafe { (self.destroy)(self.context) };
    }
}

impl Drop for WddmGpu {
    fn drop(&mut self) {
        close(self.adapter);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Built from the registers' layouts in Intel's SDM and AMD's PPR, not read from a
    // CPU.
    #[test]
    fn amd_tctl_is_in_eighths_of_a_degree() {
        assert_eq!(amd_tctl(45 * 8 << 21), 45.0);
        assert_eq!(amd_tctl(94 * 8 << 21 | 0x8_0000), 45.0);
        assert_eq!(amd_tctl((45 * 8 + 4) << 21), 45.5);
    }

    #[test]
    fn intel_package_counts_down_from_tj_max() {
        // TjMax 100 °C, with a TCC offset of 10 °C that does not move it.
        let tj = tj_max(0x0a64_0000).unwrap();
        assert_eq!(tj, 100.0);
        assert_eq!(tj_max(0x0a00_0000), None);
        // 35 °C below it: 65 °C, with the reserved bit 23 left out.
        assert_eq!(intel_package(0x8823_0000, tj), Some(65.0));
        assert_eq!(intel_package(0x80a3_0000, tj), Some(65.0));
        // Bit 31 clear: not a reading.
        assert_eq!(intel_package(0x0823_0000, tj), None);
    }

    #[test]
    fn energy_unit_is_a_power_of_two() {
        // 14 on Intel's desktop CPUs, 16 on Zen.
        assert_eq!(energy_unit_j(0x000a_0e03), 1.0 / 16384.0);
        assert_eq!(energy_unit_j(0x000a_1003), 1.0 / 65536.0);
    }

    #[test]
    fn picks_the_card_with_the_most_memory_of_its_own() {
        let c = |kind, gb: u64| Candidate { kind, dedicated: gb << 30 };
        // The CPU's graphics, a graphics card, a virtual display and the software
        // renderer.
        let all = [c(0b011, 1), c(0b011, 16), c(0b010, 0), c(0b101, 0)];
        assert_eq!(pick(&all), Some(1));
        assert_eq!(pick(&[c(0b011, 0), c(0b011, 0)]), Some(0));
        assert_eq!(pick(&[c(0b010, 0), c(0b101, 0)]), None);
        assert_eq!(pick(&[]), None);
    }

    #[test]
    fn power_is_the_boards_or_else_the_chips() {
        let mut log = AdlPmLog { size: 0, sensors: [[0; 2]; 256] };
        assert_eq!(pmlog_watts(&log), None);
        log.sensors[ADL_PMLOG_ASIC_POWER] = [1, 180];
        assert_eq!(pmlog_watts(&log), Some((23, 180.0)));
        log.sensors[ADL_PMLOG_BOARD_POWER] = [1, 304];
        assert_eq!(pmlog_watts(&log), Some((73, 304.0)));
        log.sensors[ADL_PMLOG_BOARD_POWER] = [1, -1];
        assert_eq!(pmlog_watts(&log), None);
    }

    #[test]
    fn a_kept_log_is_read_up_to_sensor_0() {
        let mut values = [[0u32; 2]; 256];
        values[..4].copy_from_slice(&[[7, 2400], [23, 180], [73, 259], [17, 12]]);
        assert_eq!(log_watts(&values), Some((73, 259.0)));
        // Sensor 0 ends the list: what follows it is left over.
        values[2] = [0, 0];
        values[3] = [73, 300];
        assert_eq!(log_watts(&values), Some((23, 180.0)));
        values[1] = [23, u32::MAX];
        assert_eq!(log_watts(&values), None, "-1 is no reading");
        assert_eq!(log_watts(&[[0; 2]; 256]), None);
    }

    #[test]
    fn a_log_the_driver_stopped_filling_reads_as_nothing() {
        let t = Instant::now();
        let mut seen = None;
        assert!(!fresh(&mut seen, 0, t), "0: never filled");
        assert!(fresh(&mut seen, 100, t));
        assert!(fresh(&mut seen, 100, t + Duration::from_secs(4)));
        assert!(!fresh(&mut seen, 100, t + Duration::from_secs(5)));
        assert!(fresh(&mut seen, 101, t + Duration::from_secs(6)), "filled again");
        assert!(fresh(&mut seen, 101, t + Duration::from_secs(10)));
    }

    #[test]
    fn the_snapshot_is_read_only_while_the_log_gives_nothing() {
        unsafe extern "C" fn unused(_: *mut c_void, _: i32, _: *mut AdlPmLog) -> i32 {
            unreachable!()
        }
        let t = Instant::now();
        let mut sources = [
            Source::new("log", Ok(Reader::Snapshot(unused))),
            Source::new("snapshot", Ok(Reader::Snapshot(unused))),
        ];
        let names = |s: &[Source]| listed(s).iter().map(|s| (s.name, s.shown, s.missed)).collect::<Vec<_>>();
        // Each tick, the readings the sources give in order, and how many were read.
        let tick = |sources: &mut [Source], at: u64, readings: &[Option<f32>]| {
            let mut i = 0;
            let watts = first_reading(sources, t + Duration::from_secs(at), |_| {
                i += 1;
                readings[i - 1].map(|w| (ADL_PMLOG_BOARD_POWER, w, None))
            });
            (watts, i)
        };
        assert_eq!(tick(&mut sources, 0, &[Some(260.0)]), (Some(260.0), 1), "the log reads, the snapshot does not");
        assert_eq!(names(&sources), [("log", true, 0)]);
        assert_eq!(tick(&mut sources, 1, &[None, Some(300.0)]), (Some(300.0), 2), "a log that gives nothing falls back");
        assert_eq!(names(&sources), [("log", false, 1), ("snapshot", true, 0)]);
        assert_eq!(tick(&mut sources, 2, &[None, None]), (None, 2));
        assert_eq!(names(&sources), [("log", false, 2), ("snapshot", false, 1)]);
        assert_eq!(tick(&mut sources, 3, &[Some(261.0)]), (Some(261.0), 1), "back to the log");
        assert_eq!(names(&sources), [("log", true, 0)]);
        assert_eq!(sources[1].missed, 1, "a source not read keeps its count");

        // A driver without the log: the snapshot alone.
        sources[0].reader = Err("this AMD driver has no ADL2_Adapter_PMLog_Start".into());
        assert_eq!(tick(&mut sources, 4, &[Some(250.0)]), (Some(250.0), 1));
        assert_eq!(names(&sources), [("log", false, 0), ("snapshot", true, 0)]);
        // The snapshot's readings of the fallback stay in its minute, then expire.
        assert_eq!(sources[1].window.stats().unwrap().readings, 2);
        sources[0].reader = Ok(Reader::Snapshot(unused));
        assert_eq!(tick(&mut sources, 70, &[Some(262.0)]), (Some(262.0), 1));
        assert_eq!(sources[1].window.stats(), None);
    }

    #[test]
    fn a_window_keeps_the_last_minute() {
        let t = Instant::now();
        let at = |s: u64| t + Duration::from_secs(s);
        let mut w = Window::default();
        assert_eq!(w.stats(), None);
        // The 60 W / 470 W jumps of #42, one a second, then a minute at a steady 250 W.
        for i in 0..60 {
            w.push(at(i), if i % 2 == 0 { 60.0 } else { 470.0 });
        }
        let s = w.stats().unwrap();
        assert_eq!((s.last, s.mean, s.min, s.max, s.sd, s.readings), (470.0, 265.0, 60.0, 470.0, 205.0, 60));
        for i in 60..121 {
            w.push(at(i), 250.0);
        }
        let s = w.stats().unwrap();
        assert_eq!((s.mean, s.min, s.max, s.sd, s.readings), (250.0, 250.0, 250.0, 0.0, 61));
        // A source that stops reading empties within a minute.
        w.expire(at(180));
        assert_eq!(w.stats().unwrap().readings, 1);
        w.expire(at(181));
        assert_eq!(w.stats(), None);
    }

    #[test]
    fn adl_structs_match_the_sdk() {
        // Two ints, a path, four ints, two paths, two ints, three paths and an int.
        assert_eq!(size_of::<AdlAdapterInfo>(), 4 * 9 + 256 * 6);
        assert_eq!(size_of::<AdlPmLog>(), 4 + 256 * 8);
        // `adl_structures.h`: 256 sensors and 16 ints; the same, an unsigned long (4
        // bytes on Windows) and 15 ints; a pointer and 14 ints; two unsigned ints, an
        // unsigned long long, 256 pairs and 256 unsigned ints.
        assert_eq!(size_of::<AdlLogSupport>(), 256 * 2 + 16 * 4);
        assert_eq!(size_of::<AdlLogStart>(), 256 * 2 + 4 + 15 * 4);
        assert_eq!(size_of::<AdlLogStarted>(), 8 + 14 * 4);
        assert_eq!(size_of::<AdlLogData>(), 4 + 4 + 8 + 256 * 8 + 256 * 4);
        assert_eq!(std::mem::offset_of!(AdlLogData, values), 16);
    }

    #[test]
    fn load_is_the_busiest_engine() {
        // A second is 10 000 000 in 100 ns: one engine ran half of it, one a tenth.
        let before = [Some(0), Some(1_000), None, Some(5)];
        let after = [Some(5_000_000), Some(1_001_000), Some(9_000_000), Some(5)];
        assert_eq!(busiest(&before, &after, 10_000_000.0), Some(50.0));
        // More than the wall time, as two readings taken apart can give, is 100 %.
        assert_eq!(busiest(&[Some(0)], &[Some(12_000_000)], 10_000_000.0), Some(100.0));
        assert_eq!(busiest(&[Some(9)], &[Some(3)], 10_000_000.0), Some(0.0));
        assert_eq!(busiest(&[], &[], 10_000_000.0), None);
        assert_eq!(busiest(&[Some(0)], &[Some(1)], 0.0), None);
    }
}
