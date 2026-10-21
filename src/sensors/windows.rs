//! Host sensors on Windows: CPU temperature and power through PawnIO, CPU load,
//! memory and the CPU's name through plain Win32 calls, and a graphics card NVML
//! does not read through what Windows keeps for Task Manager, with an AMD card's
//! power through AMD's ADL.

use anyhow::{bail, Context, Result};
use libloading::Library;
use std::ffi::c_void;
use std::time::Instant;
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

    pub fn power_w(&self) -> Option<f32> {
        self.power.as_ref()?.watts()
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

type AdlQuery = unsafe extern "C" fn(*mut c_void, i32, *mut AdlPmLog) -> i32;

/// Some ADL calls hand back memory they get from this; none camless makes does.
extern "C" fn adl_alloc(size: i32) -> *mut c_void {
    extern "C" {
        fn malloc(size: usize) -> *mut c_void;
    }
    unsafe { malloc(size.max(0) as usize) }
}

/// Board power, or the chip's on a card without it, in watts, as LibreHardwareMonitor
/// reads them. None when the card has neither.
fn pmlog_watts(log: &AdlPmLog) -> Option<f32> {
    [ADL_PMLOG_BOARD_POWER, ADL_PMLOG_ASIC_POWER]
        .into_iter()
        .map(|i| log.sensors[i])
        .find(|&[supported, _]| supported != 0)
        .and_then(|[_, watts]| (watts >= 0).then_some(watts as f32))
}

/// An AMD card's power in watts, through AMD's ADL (`atiadlxx.dll`, which comes with
/// the driver): the Overdrive 8 log, `ADL2_New_QueryPMLogData_Get`, as
/// LibreHardwareMonitor reads it. ADL lists each card once for each display output,
/// by its PCI bus and device; the card is the one at D3DKMT's address.
struct AdlPower {
    _lib: Library,
    context: *mut c_void,
    index: i32,
    query: AdlQuery,
    destroy: unsafe extern "C" fn(*mut c_void) -> i32,
}

// One thread reads the sensors at a time, and the context is this reader's own.
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
            let query: AdlQuery = *lib.get(b"ADL2_New_QueryPMLogData_Get\0")
                .context("this AMD driver has no ADL2_New_QueryPMLogData_Get")?;
            let mut context = std::ptr::null_mut();
            // 1: only the adapters connected now, as LibreHardwareMonitor asks.
            let status = create(adl_alloc, 1, &mut context);
            if status < 0 {
                bail!("ADL2_Main_Control_Create failed ({status})");
            }
            // Destroys the context on every way out from here.
            let mut adl = AdlPower { _lib: lib, context, index: -1, query, destroy };
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
            let at = |a: &&AdlAdapterInfo| a.bus as u32 == address.BusNumber && a.device as u32 == address.DeviceNumber;
            let mut why = None;
            for a in list.iter().filter(at) {
                adl.index = a.index;
                match adl.log() {
                    Ok(log) if pmlog_watts(&log).is_some() => return Ok(adl),
                    Ok(_) => why = Some("ADL has neither board nor chip power for the card".to_string()),
                    Err(status) => why = Some(format!("ADL2_New_QueryPMLogData_Get failed ({status})")),
                }
            }
            match why {
                Some(why) => bail!(why),
                None => bail!("ADL does not list the card (PCI bus {}, device {})", address.BusNumber, address.DeviceNumber),
            }
        }
    }

    /// The Overdrive 8 log, or ADL's status when it gives none.
    fn log(&self) -> std::result::Result<AdlPmLog, i32> {
        let mut log: AdlPmLog = unsafe { std::mem::zeroed() };
        let status = unsafe { (self.query)(self.context, self.index, &mut log) };
        if status < 0 {
            return Err(status);
        }
        Ok(log)
    }

    fn watts(&self) -> Option<f32> {
        pmlog_watts(&self.log().ok()?)
    }
}

impl Drop for AdlPower {
    fn drop(&mut self) {
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
        assert_eq!(pmlog_watts(&log), Some(180.0));
        log.sensors[ADL_PMLOG_BOARD_POWER] = [1, 304];
        assert_eq!(pmlog_watts(&log), Some(304.0));
        log.sensors[ADL_PMLOG_BOARD_POWER] = [1, -1];
        assert_eq!(pmlog_watts(&log), None);
    }

    #[test]
    fn adl_structs_match_the_sdk() {
        // Two ints, a path, four ints, two paths, two ints, three paths and an int.
        assert_eq!(size_of::<AdlAdapterInfo>(), 4 * 9 + 256 * 6);
        assert_eq!(size_of::<AdlPmLog>(), 4 + 256 * 8);
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
