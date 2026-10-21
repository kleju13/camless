//! Corsair's Commander Pro (`1b1c:0c10`) and the Obsidian 1000D's built-in one
//! (`1b1c:1d00`): six fan headers, four temperature probes and two LED channels.
//! Another brand's hub: camless opens it only with `[devices] other_brands` on, and
//! leaves it to iCUE while that runs.
//!
//! Every command is a 64-byte output report without a report id, `<command> <args>`,
//! and the hub answers each with a 16-byte input report, a status byte first. A reply
//! does not say which command it answers, so camless drops what is queued before each
//! command and reads one reply after it, as liquidctl does. After a reply that did not
//! come in time, it waits for it once more before the next command.
//!
//! `10` gives which probes are connected, `20` what each header has (0 nothing, 1 a DC
//! fan, 2 a PWM fan), `11 <probe>` a probe's temperature in hundredths of a degree,
//! `21 <fan>` a fan's rpm, both big-endian after the status, and `23 <fan> <duty>`
//! sets a fan, 0..100.
//!
//! camless drives the fans from the host: it never stores a curve on the hub (`25`),
//! as it would run on the hub's own probes in rpm. It sends a duty only to a header
//! with a fan, as liquidctl does. No source has a command that hands the fans back to
//! the hub's own settings, so camless leaves them at the failsafe duty when it lets go
//! or the service stops, and at the last one if camless crashes. No source says
//! whether the hub keeps a duty after it loses power. Which headers have a fan and
//! which probes are connected is read when camless opens the hub.
//!
//! camless never sends `28`, which sets a header's fan type and is kept across power
//! cycles, or anything for the LEDs.
//!
//! On Linux the kernel's `corsair-cpro` driver binds the hub and sends it commands of
//! its own whenever something reads its sensors. Replies carry no command, so two
//! programs on the hub mix them up: a probe could read as a fan's rpm. camless leaves
//! the hub alone while a kernel driver has it, which it tells as liquidctl does, by
//! the hwmon device under the hidraw node.
//!
//! No source names the HID collection. camless opens a vendor-defined one with
//! 64-byte output reports and input reports. hidapi takes the zero report id off the
//! replies on Windows too (its `windows_native` read).
//!
//! Sources: liquidctl's `commander_pro.py`, its tests and its guide. Not verified on
//! hardware.

use super::corsair_link::CORSAIR;
use super::{Driver, FanReading, Found, Model, Reading, Usage};
use anyhow::{bail, Context, Result};
use camless_core::lighting::Lighting;
use camless_core::status::{Device, FanChannel, Unit};
use hidapi::{HidApi, HidDevice};
use std::time::Duration;

const fn model(pid: u16, slug: &'static str, name: &'static str) -> Model {
    Model::vendor_hid(0x1b1c, pid, slug, name, &CORSAIR, Usage::Vendor { output: REPORT_LEN, input: true }, open)
}

pub const COMMANDER_PRO: Model = model(0x0c10, "corsair-commander-pro", "Corsair Commander Pro");
pub const OBSIDIAN_1000D: Model = model(0x1d00, "corsair-obsidian-1000d", "Corsair Obsidian 1000D");

const REPORT_LEN: usize = 64;
const REPLY_LEN: usize = 16;
const GET_PROBES: u8 = 0x10;
const GET_TEMP: u8 = 0x11;
const GET_FAN_MODES: u8 = 0x20;
const GET_RPM: u8 = 0x21;
const SET_DUTY: u8 = 0x23;
const FANS: usize = 6;
const PROBES: usize = 4;
const FAN_IDS: [&str; FANS] = ["fan1", "fan2", "fan3", "fan4", "fan5", "fan6"];
const PROBE_IDS: [&str; PROBES] = ["temp1", "temp2", "temp3", "temp4"];
const PROBE_NAMES: [&str; PROBES] = ["Probe 1", "Probe 2", "Probe 3", "Probe 4"];
/// liquidctl waits 5 s; a hub that takes over 1 s has its reply dropped as late.
const TIMEOUT: Duration = Duration::from_millis(1000);
/// camless's own bounds, past which a reply is something else's: faster is not a fan,
/// colder is not a probe in a PC.
const MAX_RPM: u16 = 10_000;
const MIN_TEMP: u16 = 100;

/// A command, after the zero report id hidapi takes for a device without ids.
fn command(cmd: u8, args: &[u8]) -> [u8; REPORT_LEN + 1] {
    let mut r = [0u8; REPORT_LEN + 1];
    r[1] = cmd;
    r[2..2 + args.len()].copy_from_slice(args);
    r
}

/// The data of a reply, after its status byte, or None if the hub refused.
fn data(reply: &[u8]) -> Option<&[u8]> {
    match reply {
        [0, rest @ ..] if rest.len() >= 2 => Some(rest),
        _ => None,
    }
}

fn be16(reply: &[u8]) -> Option<u16> {
    data(reply).map(|d| u16::from_be_bytes([d[0], d[1]]))
}

/// The probes `10` says are connected.
fn probes(reply: &[u8]) -> Option<[bool; PROBES]> {
    let d = data(reply).filter(|d| d.len() >= PROBES)?;
    Some(std::array::from_fn(|i| d[i] == 1))
}

/// The headers `20` says have a fan, DC or PWM.
fn fans(reply: &[u8]) -> Option<[bool; FANS]> {
    let d = data(reply).filter(|d| d.len() >= FANS)?;
    Some(std::array::from_fn(|i| matches!(d[i], 1 | 2)))
}

/// Whether a kernel driver with a hwmon device has the hidraw node at `path`, the way
/// liquidctl tells it (`HwmonDevice.from_hidraw`).
#[cfg(target_os = "linux")]
fn kernel_driver(path: &std::ffi::CStr) -> bool {
    let node = path.to_str().ok().and_then(|p| std::path::Path::new(p).file_name().map(|n| n.to_owned()));
    node.is_some_and(|n| std::path::Path::new("/sys/class/hidraw").join(n).join("device/hwmon").exists())
}

pub struct CommanderPro {
    dev: HidDevice,
    model: &'static Model,
    probes: [bool; PROBES],
    fans: [bool; FANS],
    duties: [Option<u8>; FANS],
    /// A reply did not come in time, and may still.
    late: bool,
}

fn open(api: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    #[cfg(target_os = "linux")]
    if kernel_driver(&found.path) {
        bail!(
            "the kernel's corsair-cpro driver has it, and its commands would mix with camless's; \
             unload it (modprobe -r corsair-cpro) and blacklist it for camless to drive the hub"
        );
    }
    let mut d = CommanderPro {
        dev: super::hid::open(api, &found.path)?,
        model: found.model,
        probes: [false; PROBES],
        fans: [false; FANS],
        duties: [None; FANS],
        late: false,
    };
    d.probes = probes(&d.request(GET_PROBES, &[])?).context("the hub did not say which probes it has")?;
    d.fans = fans(&d.request(GET_FAN_MODES, &[])?).context("the hub did not say which headers have a fan")?;
    Ok(Box::new(d))
}

impl CommanderPro {
    /// One command and its reply, with what was queued dropped first, and a late
    /// reply waited for and dropped.
    fn request(&mut self, cmd: u8, args: &[u8]) -> Result<Vec<u8>> {
        let mut buf = [0u8; REPORT_LEN];
        if self.late {
            self.dev.read_timeout(&mut buf, TIMEOUT.as_millis() as i32)?;
            self.late = false;
        }
        super::hid::drain(&self.dev);
        let c = command(cmd, args);
        let n = self.dev.write(&c).context("write to the hub failed")?;
        if n < c.len() {
            bail!("{cmd:#04x}: {n} of {} bytes went out", c.len());
        }
        let n = self.dev.read_timeout(&mut buf, TIMEOUT.as_millis() as i32)?;
        if n == 0 {
            self.late = true;
            bail!("no reply to {cmd:#04x} within {} ms", TIMEOUT.as_millis());
        }
        Ok(buf[..n.min(REPLY_LEN)].to_vec())
    }
}

impl Driver for CommanderPro {
    fn describe(&self) -> Device {
        Device {
            name: self.model.name.into(),
            verified: self.model.verified(),
            experimental: self.model.is_experimental(),
            fans: (0..FANS)
                .map(|i| FanChannel {
                    id: FAN_IDS[i].into(),
                    name: format!("Fan {}", i + 1),
                    connected: Some(self.fans[i]),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    fn sensors(&self) -> Vec<(&'static str, &'static str, Unit)> {
        (0..PROBES).filter(|&i| self.probes[i]).map(|i| (PROBE_IDS[i], PROBE_NAMES[i], Unit::Celsius)).collect()
    }

    fn poll(&mut self) -> Result<Reading> {
        let mut r = Reading::default();
        let probes = self.probes;
        for i in (0..PROBES).filter(|&i| probes[i]) {
            // A refused reply is a probe that went, not a lost hub.
            if let Some(t) = be16(&self.request(GET_TEMP, &[i as u8])?).filter(|&t| t >= MIN_TEMP) {
                r.sensors.push((PROBE_IDS[i], f32::from(t) / 100.0));
            }
        }
        for i in 0..FANS {
            let rpm = if self.fans[i] { be16(&self.request(GET_RPM, &[i as u8])?).filter(|&n| n <= MAX_RPM) } else { None };
            r.fans.push((FAN_IDS[i], FanReading { rpm, duty: self.duties[i], connected: Some(self.fans[i]) }));
        }
        Ok(r)
    }

    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
        for &(id, duty) in duties {
            let i = FAN_IDS.iter().position(|f| *f == id).with_context(|| format!("no fan {id:?} on the {}", self.model.name))?;
            if !self.fans[i] {
                continue;
            }
            let duty = duty.min(100);
            if data(&self.request(SET_DUTY, &[i as u8, duty])?).is_none() {
                bail!("the hub refused fan {} at {duty} %", i + 1);
            }
            self.duties[i] = Some(duty);
        }
        Ok(())
    }

    fn set_lighting(&mut self, light: &str, _: &Lighting) -> Result<()> {
        bail!("no light {light:?} on the {}: camless sets no lighting on it", self.model.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(hex: &str) -> Vec<u8> {
        (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn commands_are_liquidctls() {
        // test_set_fixed_speed_valid: fan2 at 50 goes out as `23 01 32`.
        let c = command(SET_DUTY, &[1, 50]);
        assert_eq!(c.len(), 65);
        assert_eq!(&c[..4], [0x00, 0x23, 0x01, 0x32]);
        assert!(c[4..].iter().all(|&b| b == 0));
        assert_eq!(&command(GET_TEMP, &[1])[..3], [0x00, 0x11, 0x01]);
    }

    #[test]
    fn replies_read_as_liquidctls_samples() {
        // test_initialize_commander_pro: probes 1, 2 and 4; DC, DC, PWM, then nothing.
        assert_eq!(probes(&reply("00010100010000000000000000000000")), Some([true, true, false, true]));
        assert_eq!(fans(&reply("00010102000000000000000000000000")), Some([true, true, true, false, false, false]));
        // test_get_status_commander_pro: 26.91 °C and 940 rpm.
        assert_eq!(be16(&reply("000a8300000000000000000000000000")), Some(2691));
        assert_eq!(be16(&reply("0003ac00000000000000000000000000")), Some(940));
    }

    #[test]
    fn refused_or_short_replies_are_no_reading() {
        assert_eq!(be16(&reply("110a83")), None);
        assert_eq!(be16(&reply("000a")), None);
        assert_eq!(probes(&reply("000101")), None);
        assert_eq!(fans(&[]), None);
    }
}
