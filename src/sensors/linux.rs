//! Host sensors on Linux, read from sysfs and procfs the way Nexus reads them
//! (`LinuxSensorProvider.cs`, `AmdGpuSysfs.cs`): CPU temperature from the hwmon
//! drivers k10temp, zenpower and coretemp, package power from RAPL, load from
//! `/proc/stat`, memory from `/proc/meminfo`, and AMD GPUs from amdgpu.
//!
//! Every user can read these except RAPL's energy counter, which only root can read
//! since the PLATYPUS side channel; without it there is no CPU power.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The library NVIDIA's driver installs.
pub const NVML: &str = "libnvidia-ml.so.1";

pub const NO_CPU_TEMP: &str = "no CPU temperature (no k10temp, zenpower or coretemp sensor)";

const HWMON: &str = "/sys/class/hwmon";
const RAPL: &str = "/sys/class/powercap/intel-rapl:0";

/// The CPU drivers, and the temperature each calls the package's: AMD's Tctl is what
/// camless reads through PawnIO on Windows.
const CPU_DRIVERS: [(&str, &[&str]); 3] =
    [("k10temp", &["Tctl", "Tdie"]), ("zenpower", &["Tctl", "Tdie"]), ("coretemp", &["Package id 0"])];

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn number(path: &Path) -> Option<i64> {
    read(path)?.parse().ok()
}

pub fn cpu(_pawnio_module: std::result::Result<Vec<u8>, String>) -> std::result::Result<Cpu, String> {
    Cpu::open(Path::new(HWMON), Path::new(RAPL)).map_err(|e| format!("{e:#}"))
}

pub struct Cpu {
    temp: PathBuf,
    /// RAPL's package energy in µJ, if this user can read it, and where it wraps.
    energy: Option<(PathBuf, u64)>,
    last_energy: Option<(u64, Instant)>,
}

impl Cpu {
    fn open(hwmon: &Path, rapl: &Path) -> Result<Cpu> {
        let temp = cpu_temp_input(hwmon).context("no k10temp, zenpower or coretemp sensor in hwmon")?;
        let energy = rapl.join("energy_uj");
        let wrap = number(&rapl.join("max_energy_range_uj")).and_then(|w| u64::try_from(w).ok()).filter(|&w| w > 0);
        let energy = wrap.filter(|_| number(&energy).is_some()).map(|w| (energy, w));
        let cpu = Cpu { temp, energy, last_energy: None };
        cpu.temperature()?;
        Ok(cpu)
    }

    pub fn temperature(&self) -> Result<f32> {
        match number(&self.temp) {
            Some(milli) => Ok(milli as f32 / 1000.0),
            None => bail!("cannot read {}", self.temp.display()),
        }
    }

    pub fn package_power_w(&mut self) -> Option<f32> {
        let (path, wrap) = self.energy.as_ref()?;
        let now = Instant::now();
        let energy = u64::try_from(number(path)?).ok()?;
        let prev = self.last_energy.replace((energy, now))?;
        let dt = now.duration_since(prev.1).as_secs_f64();
        let used = if energy >= prev.0 { energy - prev.0 } else { energy + wrap - prev.0 };
        (dt > 0.05).then(|| (used as f64 / 1e6 / dt) as f32)
    }
}

/// The package temperature's `temp*_input` of the first CPU driver in hwmon: the one
/// labelled as [`CPU_DRIVERS`] says, else the first one.
fn cpu_temp_input(hwmon: &Path) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(hwmon).ok()?.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for (driver, labels) in CPU_DRIVERS {
        for dir in dirs.iter().filter(|d| read(&d.join("name")).as_deref() == Some(driver)) {
            let mut inputs: Vec<(u32, PathBuf)> = std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().into_string().ok()?;
                    let n = name.strip_prefix("temp")?.strip_suffix("_input")?.parse().ok()?;
                    Some((n, e.path()))
                })
                .collect();
            inputs.sort();
            let label = |n: u32| read(&dir.join(format!("temp{n}_label"))).unwrap_or_default();
            let labelled = labels.iter().find_map(|want| inputs.iter().find(|(n, _)| label(*n) == *want));
            if let Some((_, path)) = labelled.or(inputs.first()) {
                return Some(path.clone());
            }
        }
    }
    None
}

/// Idle time and all time, in clock ticks, from the first line of `/proc/stat`.
pub fn cpu_times() -> Option<(u64, u64)> {
    parse_stat(&std::fs::read_to_string("/proc/stat").ok()?)
}

fn parse_stat(text: &str) -> Option<(u64, u64)> {
    let line = text.lines().next()?.strip_prefix("cpu ")?;
    let v: Vec<u64> = line.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    // user nice system idle iowait irq softirq steal; guest time is in user already.
    let total: u64 = v.iter().take(8).sum();
    let idle = v.get(3)? + v.get(4).copied().unwrap_or(0);
    Some((idle, total))
}

/// Memory: total and in use in bytes, and the share in use in percent.
pub fn memory() -> Option<(u64, u64, f32)> {
    parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)
}

fn parse_meminfo(text: &str) -> Option<(u64, u64, f32)> {
    let kb = |key: &str| -> Option<u64> {
        let line = text.lines().find(|l| l.starts_with(key))?;
        line[key.len()..].trim().trim_end_matches("kB").trim().parse::<u64>().ok().map(|k| k * 1024)
    };
    let (total, available) = (kb("MemTotal:")?, kb("MemAvailable:")?);
    let used = total.saturating_sub(available);
    (total > 0).then(|| (total, used, 100.0 * used as f32 / total as f32))
}

pub fn cpu_name() -> Option<String> {
    let text = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    let line = text.lines().find(|l| l.starts_with("model name"))?;
    Some(line.split_once(':')?.1.trim().to_string())
}

/// An AMD GPU on amdgpu: the card with the most video memory, so a graphics card
/// before the CPU's own.
pub struct AmdGpu {
    pub name: String,
    device: PathBuf,
    hwmon: Option<PathBuf>,
}

impl AmdGpu {
    pub fn open() -> Result<AmdGpu> {
        Self::find(Path::new("/sys/class/drm"), Path::new("/usr/share/libdrm/amdgpu.ids")).context("no AMD GPU on amdgpu")
    }

    fn find(drm: &Path, ids: &Path) -> Option<AmdGpu> {
        let mut best: Option<(i64, PathBuf)> = None;
        for e in std::fs::read_dir(drm).ok()?.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            // cardN, not the cardN-HDMI-A-1 outputs beside it.
            if !name.strip_prefix("card").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) {
                continue;
            }
            let device = e.path().join("device");
            let driver = std::fs::read_link(device.join("driver")).ok();
            if driver.as_deref().and_then(Path::file_name).is_none_or(|d| d != "amdgpu") {
                continue;
            }
            let vram = number(&device.join("mem_info_vram_total")).unwrap_or(0);
            if best.as_ref().is_none_or(|(v, _)| vram > *v) {
                best = Some((vram, device));
            }
        }
        let (_, device) = best?;
        let hwmon = std::fs::read_dir(device.join("hwmon")).ok().and_then(|d| d.flatten().map(|e| e.path()).min());
        let id = |f: &str| read(&device.join(f)).and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok());
        let name = match (id("device"), id("revision")) {
            (Some(d), Some(r)) => std::fs::read_to_string(ids).ok().and_then(|t| marketing_name(&t, d, r)),
            _ => None,
        };
        Some(AmdGpu { name: name.unwrap_or_else(|| "AMD GPU".into()), device, hwmon })
    }

    /// The edge temperature, the one amdgpu lists first.
    pub fn temperature(&self) -> Option<f32> {
        number(&self.hwmon.as_ref()?.join("temp1_input")).filter(|&m| m > 0).map(|m| m as f32 / 1000.0)
    }

    pub fn utilization(&self) -> Option<f32> {
        number(&self.device.join("gpu_busy_percent")).map(|p| p as f32)
    }

    pub fn power_w(&self) -> Option<f32> {
        let hwmon = self.hwmon.as_ref()?;
        let uw = number(&hwmon.join("power1_average")).or_else(|| number(&hwmon.join("power1_input")))?;
        Some(uw as f32 / 1e6)
    }
}

/// The name libdrm's `amdgpu.ids` gives a device and revision: lines of
/// `DEVICE_ID, REVISION_ID, NAME` in hex.
fn marketing_name(ids: &str, device: u32, revision: u32) -> Option<String> {
    ids.lines().filter(|l| !l.starts_with('#')).find_map(|l| {
        let mut parts = l.splitn(3, ',').map(str::trim);
        let d = u32::from_str_radix(parts.next()?, 16).ok()?;
        let r = u32::from_str_radix(parts.next()?, 16).ok()?;
        ((d, r) == (device, revision)).then(|| parts.next().map(String::from)).flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn cpu_temperature_is_tctl_on_amd_and_the_package_on_intel() {
        let root = std::env::temp_dir().join(format!("camless-hwmon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // An NVMe drive first, then k10temp with Tctl second.
        write(&root.join("hwmon0/name"), "nvme\n");
        write(&root.join("hwmon0/temp1_input"), "41850\n");
        write(&root.join("hwmon1/name"), "k10temp\n");
        write(&root.join("hwmon1/temp1_input"), "52000\n");
        write(&root.join("hwmon1/temp1_label"), "Tccd1\n");
        write(&root.join("hwmon1/temp3_input"), "61250\n");
        write(&root.join("hwmon1/temp3_label"), "Tctl\n");
        let cpu = Cpu::open(&root, &root.join("no-rapl")).unwrap();
        assert_eq!(cpu.temperature().unwrap(), 61.25);
        assert!(cpu.energy.is_none());

        std::fs::remove_dir_all(root.join("hwmon1")).unwrap();
        write(&root.join("hwmon2/name"), "coretemp\n");
        write(&root.join("hwmon2/temp2_input"), "48000\n");
        write(&root.join("hwmon2/temp2_label"), "Core 0\n");
        write(&root.join("hwmon2/temp1_input"), "55000\n");
        write(&root.join("hwmon2/temp1_label"), "Package id 0\n");
        assert_eq!(Cpu::open(&root, &root).unwrap().temperature().unwrap(), 55.0);

        std::fs::remove_dir_all(root.join("hwmon2")).unwrap();
        assert!(Cpu::open(&root, &root).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn package_power_comes_from_the_energy_counter_across_its_wrap() {
        let root = std::env::temp_dir().join(format!("camless-rapl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root.join("hwmon0/name"), "k10temp\n");
        write(&root.join("hwmon0/temp1_input"), "40000\n");
        write(&root.join("rapl/energy_uj"), "999000000\n");
        write(&root.join("rapl/max_energy_range_uj"), "1000000000\n");
        let mut cpu = Cpu::open(&root, &root.join("rapl")).unwrap();
        assert_eq!(cpu.package_power_w(), None, "the first reading has nothing to compare with");
        cpu.last_energy = Some((999_000_000, Instant::now() - std::time::Duration::from_secs(1)));
        write(&root.join("rapl/energy_uj"), "44000000\n");
        let w = cpu.package_power_w().unwrap();
        assert!((44.0..46.0).contains(&w), "{w}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn load_and_memory_come_from_procfs() {
        let stat = "cpu  100 5 50 800 20 3 2 0 40 0\ncpu0 1 2 3 4 5 6 7 8 9 10\n";
        assert_eq!(parse_stat(stat), Some((820, 980)));
        let meminfo = "MemTotal:       32768000 kB\nMemFree:         1000000 kB\nMemAvailable:   24576000 kB\n";
        let (total, used, pct) = parse_meminfo(meminfo).unwrap();
        assert_eq!((total, used), (32_768_000 * 1024, 8_192_000 * 1024));
        assert_eq!(pct, 25.0);
    }

    #[test]
    fn amd_gpus_are_named_by_libdrm() {
        let ids = "# List of AMDGPU IDs\n1.0.0\n744C,\tC8,\tAMD Radeon RX 7900 XTX\n744C,\tCC,\tAMD Radeon RX 7900 XT\n";
        assert_eq!(marketing_name(ids, 0x744c, 0xcc).as_deref(), Some("AMD Radeon RX 7900 XT"));
        assert_eq!(marketing_name(ids, 0x744c, 0x01), None);

        let root = std::env::temp_dir().join(format!("camless-drm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // The CPU's own graphics on card0, a graphics card on card1.
        for (card, vram, device) in [("card0", "536870912", "0x15bf"), ("card1", "25753026560", "0x744c")] {
            let dev = root.join(card).join("device");
            write(&dev.join("mem_info_vram_total"), vram);
            write(&dev.join("device"), device);
            write(&dev.join("revision"), "0xc8");
            write(&dev.join("gpu_busy_percent"), "37\n");
            write(&dev.join("hwmon/hwmon5/temp1_input"), "64000\n");
            write(&dev.join("hwmon/hwmon5/power1_average"), "211000000\n");
            std::os::unix::fs::symlink("/sys/bus/pci/drivers/amdgpu", dev.join("driver")).unwrap();
        }
        std::fs::create_dir_all(root.join("card1-DP-1")).unwrap();
        write(&root.join("amdgpu.ids"), ids);
        let gpu = AmdGpu::find(&root, &root.join("amdgpu.ids")).unwrap();
        assert_eq!(gpu.name, "AMD Radeon RX 7900 XTX");
        assert!(gpu.device.starts_with(root.join("card1")));
        assert_eq!((gpu.temperature(), gpu.utilization(), gpu.power_w()), (Some(64.0), Some(37.0), Some(211.0)));
        let _ = std::fs::remove_dir_all(&root);
    }
}
