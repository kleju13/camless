//! Lian Li Uni Hubs: four fan ports each, for Uni fans of the SL, AL, SL-Infinity and
//! v2 series (`0cf2:7750` and `0cf2:a100` to `a106`). Another brand's hub: camless
//! opens it only with `[devices] other_brands` on, and leaves it to L-Connect while
//! that runs (`Found::open`, `Daemon::yield_other_brands`).
//!
//! camless drives the fans only. It leaves a port as it is until a curve takes it;
//! then it puts that port in manual mode and sets its duty. When camless lets go of
//! the hub, for L-Connect or with the setting turned off, it hands those ports back
//! to the hub's motherboard PWM input. The lighting stays as the hub or L-Connect
//! left it.
//!
//! Every command is a 7-byte feature report with id `E0` on the hub's vendor
//! collection (usage page `FF72`, usage `A1`). The speeds and the firmware version
//! come back as input reports read on request, which hidapi's Windows backend
//! cannot do: see `winhid`.
//!
//! Sources: Nexus (`src/Peripherals/LianLi`, `src/Cooling/LianLiCoolingProvider.cs`),
//! which follows FanControl.LianLi and L-Connect 3. Nexus calls its Lian Li support
//! experimental, and ran the SL-Infinity and SL hubs only; the other families come
//! from its tables. Not verified on hardware.

use super::winhid::Hid;
use super::{Driver, FanReading, Found, Model, Reading, Vendor};
use anyhow::{bail, Context, Result};
use camless_core::lighting::Lighting;
use camless_core::status::{Device, FanChannel, Unit};
use hidapi::HidApi;
use std::time::{Duration, SystemTime};

pub const LIAN_LI: Vendor = Vendor {
    name: "Lian Li",
    app: "L-Connect",
    // Nexus's list, as executables.
    processes: &["l-connect 3.exe", "l-connect.exe", "lconnect3.exe", "lconnect.exe"],
};

const fn model(pid: u16, slug: &'static str, name: &'static str) -> Model {
    Model {
        experimental: true,
        vendor: Some(&LIAN_LI),
        usage: Some((0xff72, 0xa1)),
        ..Model::hid(0x0cf2, pid, slug, name, open)
    }
}

pub const UNI_HUB: Model = model(0x7750, "lianli-uni-hub", "Lian Li Uni Hub");
pub const UNI_HUB_SL: Model = model(0xa100, "lianli-uni-hub-sl", "Lian Li Uni Hub SL");
pub const UNI_HUB_AL: Model = model(0xa101, "lianli-uni-hub-al", "Lian Li Uni Hub AL");
pub const UNI_HUB_SL_INFINITY: Model = model(0xa102, "lianli-uni-hub-sl-infinity", "Lian Li Uni Hub SL-Infinity");
pub const UNI_HUB_SL_V2: Model = model(0xa103, "lianli-uni-hub-sl-v2", "Lian Li Uni Hub SL v2");
pub const UNI_HUB_AL_V2: Model = model(0xa104, "lianli-uni-hub-al-v2", "Lian Li Uni Hub AL v2");
/// A second product id of the SL v2 hub.
pub const UNI_HUB_SL_V2_A105: Model = model(0xa105, "lianli-uni-hub-sl-v2", "Lian Li Uni Hub SL v2");
pub const UNI_HUB_SL_REDRAGON: Model = model(0xa106, "lianli-uni-hub-sl-redragon", "Lian Li Uni Hub SL (Redragon)");

const REPORT_ID: u8 = 0xe0;
const INPUT_LEN: usize = 65;
const PORTS: usize = 4;
/// The firmware drops a duty that comes sooner than this after manual mode.
const SETTLE: Duration = Duration::from_millis(200);
/// Higher readings are not a speed; Nexus drops them.
const MAX_RPM: u16 = 6000;
/// A longer gap between polls is the PC waking from sleep, which takes the hub out
/// of manual mode. Polls come at least every 10 s.
const ASLEEP: Duration = Duration::from_secs(30);
const RPM_PRIMER: [u8; 7] = [REPORT_ID, 0x50, 0x00, 0, 0, 0, 0];
const FIRMWARE_PRIMER: [u8; 7] = [REPORT_ID, 0x50, 0x01, 0, 0, 0, 0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Sl,
    Al,
    SlInfinity,
    SlV2,
    AlV2,
}

/// What differs between the hubs, from Nexus's `LianLiFanProfiles`.
#[derive(Clone, Copy, Debug)]
struct Profile {
    family: Family,
    /// Duty 0 goes out as 1 and 1 to 9 as 10, below which these fans stall.
    floored: bool,
    /// Byte 2 of the mode commands.
    register: u8,
    /// Where the speeds start in the input report.
    rpm_offset: usize,
}

fn profile(pid: u16) -> Option<Profile> {
    let p = |family, floored, register, rpm_offset| Profile { family, floored, register, rpm_offset };
    Some(match pid {
        0x7750 | 0xa100 | 0xa106 => p(Family::Sl, false, 0x31, 1),
        0xa101 => p(Family::Al, false, 0x42, 1),
        0xa102 => p(Family::SlInfinity, true, 0x62, 1),
        0xa103 | 0xa105 => p(Family::SlV2, true, 0x62, 2),
        0xa104 => p(Family::AlV2, true, 0x62, 2),
        _ => return None,
    })
}

/// Takes the port off the motherboard's PWM. Entering it again resets the fan to
/// its default speed.
fn manual_mode(p: &Profile, port: usize) -> [u8; 7] {
    [REPORT_ID, 0x10, p.register, 0x10 << port, 0, 0, 0]
}

/// Hands the port back to the motherboard's PWM.
fn release_mode(p: &Profile, port: usize) -> [u8; 7] {
    [REPORT_ID, 0x10, p.register, 0x11 << port, 0, 0, 0]
}

fn speed(p: &Profile, port: usize, duty: u8) -> [u8; 7] {
    let duty = match (p.floored, duty) {
        (true, 0) => 1,
        (true, d) => d.clamp(10, 100),
        (false, d) => d.min(100),
    };
    [REPORT_ID, 0x20 | port as u8, 0x00, duty, 0, 0, 0]
}

/// Big-endian speeds from `offset`, one per port.
fn parse_rpm(msg: &[u8], offset: usize) -> [Option<u16>; PORTS] {
    std::array::from_fn(|port| {
        let i = offset + 2 * port;
        let rpm = u16::from_be_bytes([*msg.get(i)?, *msg.get(i + 1)?]);
        (rpm <= MAX_RPM).then_some(rpm)
    })
}

/// The reply to the firmware primer: `E0 50` again at [1], the family id at [4] and
/// the version at [5], which each family encodes its own way. A version outside its
/// family's encoding shows as hex.
fn firmware_text(msg: &[u8], family: Family) -> Option<String> {
    if msg.len() < 6 || msg[1] != REPORT_ID || msg[2] != 0x50 {
        return None;
    }
    let fine = msg[5];
    Some(match firmware_number(fine, family, msg[4]) {
        Some(n) => format!("{}.{}", n / 10, n % 10),
        None => format!("0x{fine:02X}"),
    })
}

/// Major × 10 + minor. SL-Infinity leaves 0 to 3 and 15 to 31 unused and reads 13
/// and 14 one minor step up; AL reads two steps up, with 1.0 below 8; SL v2 family
/// `C7` reads 0 as 0.5 and leaves 1 to 5 unused.
fn firmware_number(fine: u8, family: Family, family_id: u8) -> Option<u8> {
    let (hi, mut lo) = (fine >> 4, fine & 0x0f);
    match family {
        Family::SlInfinity => {
            if fine <= 3 || (15..=31).contains(&fine) || (fine >= 32 && lo > 9) {
                return None;
            }
            if fine == 13 || fine == 14 {
                lo += 1;
            }
            Some(hi * 10 + lo)
        }
        Family::Al if lo > 9 => None,
        Family::Al => Some(if fine < 8 { 10 } else { hi * 10 + lo + 2 }),
        Family::SlV2 if family_id == 0xc7 && fine <= 5 => (fine == 0).then_some(5),
        _ => (lo <= 9).then_some(hi * 10 + lo),
    }
}

fn port_of(fan: &str) -> Option<usize> {
    let n: usize = fan.strip_prefix("fan")?.parse().ok()?;
    (1..=PORTS).contains(&n).then(|| n - 1)
}

pub struct LianLi {
    model: &'static Model,
    profile: Profile,
    hid: Hid,
    firmware: Option<String>,
    /// The duty of each port camless drives.
    duties: [Option<u8>; PORTS],
    /// The ports in manual mode now.
    manual: [bool; PORTS],
    rpm: [Option<u16>; PORTS],
    /// Wall-clock time goes on while the PC sleeps.
    last_poll: SystemTime,
}

fn open(_: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    let m = found.model;
    let profile = profile(m.pid).context("no profile for this hub")?;
    let hid = Hid::open(&found.path)?;
    let mut d = LianLi {
        model: m,
        profile,
        hid,
        firmware: None,
        duties: [None; PORTS],
        manual: [false; PORTS],
        rpm: [None; PORTS],
        last_poll: SystemTime::now(),
    };
    // The hub can answer this empty for a while after it attaches.
    for _ in 0..3 {
        d.hid.set_feature(&FIRMWARE_PRIMER)?;
        d.firmware = firmware_text(&d.hid.input_report(REPORT_ID, INPUT_LEN)?, profile.family);
        if d.firmware.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    d.rpm = d.read_rpm()?;
    Ok(Box::new(d))
}

impl LianLi {
    fn read_rpm(&self) -> Result<[Option<u16>; PORTS]> {
        self.hid.set_feature(&RPM_PRIMER)?;
        Ok(parse_rpm(&self.hid.input_report(REPORT_ID, INPUT_LEN)?, self.profile.rpm_offset))
    }

    /// The duty only, on a port in manual mode; manual mode first on one that is not.
    fn send(&mut self, port: usize, duty: u8) -> Result<()> {
        if !self.manual[port] {
            self.hid.set_feature(&manual_mode(&self.profile, port))?;
            self.manual[port] = true;
            std::thread::sleep(SETTLE);
        }
        self.hid.set_feature(&speed(&self.profile, port, duty))
    }
}

impl Driver for LianLi {
    fn describe(&self) -> Device {
        Device {
            name: self.model.name.into(),
            firmware: self.firmware.clone(),
            verified: self.model.verified(),
            experimental: self.model.is_experimental(),
            fans: (1..=PORTS)
                .map(|n| FanChannel { id: format!("fan{n}"), name: format!("Port {n}"), ..Default::default() })
                .collect(),
            ..Default::default()
        }
    }

    fn sensors(&self) -> Vec<(&'static str, &'static str, Unit)> {
        Vec::new()
    }

    fn poll(&mut self) -> Result<Reading> {
        let now = SystemTime::now();
        if now.duration_since(self.last_poll).map_or(true, |gap| gap > ASLEEP) {
            self.manual = [false; PORTS];
        }
        self.last_poll = now;
        for (port, rpm) in self.read_rpm()?.into_iter().enumerate() {
            self.rpm[port] = rpm.or(self.rpm[port]);
        }
        // Nexus sends the duties again with every poll; after sleep that takes
        // manual mode again too.
        for port in 0..PORTS {
            if let Some(duty) = self.duties[port] {
                self.send(port, duty)?;
            }
        }
        let fans = ["fan1", "fan2", "fan3", "fan4"]
            .into_iter()
            .zip(0..PORTS)
            .map(|(id, port)| (id, FanReading { rpm: self.rpm[port], duty: self.duties[port], connected: None }))
            .collect();
        Ok(Reading { sensors: Vec::new(), fans })
    }

    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
        for &(fan, duty) in duties {
            let port = port_of(fan).with_context(|| format!("no fan {fan:?} on this hub"))?;
            self.send(port, duty)?;
            self.duties[port] = Some(duty.min(100));
        }
        Ok(())
    }

    fn set_lighting(&mut self, light: &str, _: &Lighting) -> Result<()> {
        bail!("no light {light:?}: camless leaves this hub's lighting as it is")
    }

    fn release(&mut self) -> Result<()> {
        for port in 0..PORTS {
            if self.duties[port].take().is_some() || self.manual[port] {
                self.manual[port] = false;
                self.hid.set_feature(&release_mode(&self.profile, port))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SL_INFINITY: Profile = Profile { family: Family::SlInfinity, floored: true, register: 0x62, rpm_offset: 1 };
    const SL: Profile = Profile { family: Family::Sl, floored: false, register: 0x31, rpm_offset: 1 };

    #[test]
    fn every_model_has_a_profile() {
        for m in super::super::MODELS.iter().filter(|m| m.vendor.is_some_and(|v| v.name == "Lian Li")) {
            assert!(profile(m.pid).is_some(), "{:04x}", m.pid);
        }
        assert!(profile(0xa107).is_none());
    }

    #[test]
    fn mode_and_speed_reports_are_nexus_builders() {
        assert_eq!(manual_mode(&SL_INFINITY, 0), [0xe0, 0x10, 0x62, 0x10, 0, 0, 0]);
        assert_eq!(manual_mode(&SL_INFINITY, 3), [0xe0, 0x10, 0x62, 0x80, 0, 0, 0]);
        assert_eq!(release_mode(&SL, 1), [0xe0, 0x10, 0x31, 0x22, 0, 0, 0]);
        assert_eq!(release_mode(&SL, 3), [0xe0, 0x10, 0x31, 0x88, 0, 0, 0]);
        assert_eq!(speed(&SL_INFINITY, 2, 55), [0xe0, 0x22, 0, 55, 0, 0, 0]);
        // `DutyByte`: the floored families stall below 10.
        let byte = |p, d| speed(p, 0, d)[3];
        assert_eq!([0, 1, 9, 10, 100, 120].map(|d| byte(&SL_INFINITY, d)), [1, 10, 10, 10, 100, 100]);
        assert_eq!([0, 1, 9, 100, 120].map(|d| byte(&SL, d)), [0, 1, 9, 100, 100]);
    }

    #[test]
    fn speeds_are_big_endian_from_the_offset() {
        let mut msg = [0u8; INPUT_LEN];
        msg[0] = REPORT_ID;
        msg[1..9].copy_from_slice(&[0x03, 0xe8, 0x00, 0x00, 0x17, 0x71, 0x05, 0xdc]);
        assert_eq!(parse_rpm(&msg, 1), [Some(1000), Some(0), None, Some(1500)]);
        // The v2 hubs start a byte later: e8 00 is no speed.
        assert_eq!(parse_rpm(&msg, 2)[0], None);
        assert_eq!(parse_rpm(&msg[..4], 1), [Some(1000), None, None, None]);
    }

    #[test]
    fn firmware_reads_per_family() {
        let reply = |family_id, fine| [REPORT_ID, REPORT_ID, 0x50, 1, family_id, fine];
        let text = |family, family_id, fine| firmware_text(&reply(family_id, fine), family).unwrap();
        // SL-Infinity reads 13 as 1.4, the firmware Nexus ran.
        assert_eq!(text(Family::SlInfinity, 0, 13), "1.4");
        assert_eq!(text(Family::SlInfinity, 0, 0x21), "2.1");
        assert_eq!(text(Family::SlInfinity, 0, 20), "0x14");
        assert_eq!(text(Family::Al, 0, 5), "1.0");
        assert_eq!(text(Family::Al, 0, 0x13), "1.5");
        assert_eq!(text(Family::SlV2, 0xc7, 0), "0.5");
        assert_eq!(text(Family::SlV2, 0xc7, 3), "0x03");
        assert_eq!(text(Family::Sl, 0, 0x12), "1.2");
        // The empty answer of a hub that just attached.
        assert_eq!(firmware_text(&[REPORT_ID, 0, 0, 0, 0, 0], Family::Sl), None);
    }

    #[test]
    fn fans_are_the_four_ports() {
        assert_eq!(["fan1", "fan4"].map(port_of), [Some(0), Some(3)]);
        assert_eq!(["fan0", "fan5", "pump", "fan"].map(port_of), [None; 4]);
    }
}
