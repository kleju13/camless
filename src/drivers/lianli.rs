//! Lian Li Uni Hubs: four fan ports each, for Uni fans of the SL, AL, SL-Infinity and
//! v2 series (`0cf2:7750` and `0cf2:a100` to `a106`). Another brand's hub: camless
//! opens it only with `[devices] other_brands` on, and leaves it to L-Connect while
//! that runs (`Found::open`, `Daemon::yield_other_brands`).
//!
//! camless leaves a port as it is until a curve takes it; then it puts that port in
//! manual mode and sets its duty. When camless lets go of the hub, for L-Connect, with
//! the setting turned off or when the service stops, it hands those ports back to the
//! hub's motherboard PWM input. A port's lighting changes only when the config sets
//! it, to off or one colour, which the hub keeps showing after camless lets go.
//!
//! Every command is a 7-byte feature report with id `E0` on the hub's vendor
//! collection (usage page `FF72`, usage `A1`). The speeds and the firmware version
//! come back as input reports read on request, which hidapi's Windows backend
//! cannot do: see `winhid`, and `hidraw` on Linux. Colours go in 353-byte output
//! reports, over the interrupt pipe on the SL hubs and the control pipe on the
//! others.
//!
//! Sources: Nexus (`src/Peripherals/LianLi`, `src/Cooling/LianLiCoolingProvider.cs`,
//! `src/Lighting/LianLiLightingFrameWriter.cs`), which follows FanControl.LianLi,
//! OpenRGB and L-Connect 3. Nexus calls its Lian Li support experimental, and ran
//! the SL-Infinity and SL hubs only; the other families come from its tables. Not
//! verified on hardware.

#[cfg(target_os = "linux")]
use super::hidraw::Hid;
#[cfg(windows)]
use super::winhid::Hid;
use super::{Driver, FanReading, Found, Model, Reading, Usage, Vendor};
use anyhow::{bail, Context, Result};
use camless_core::lighting::{Lighting, Rgb};
use camless_core::status::{Device, FanChannel, LightChannel, Unit};
use hidapi::HidApi;
use std::time::{Duration, SystemTime};

pub const LIAN_LI: Vendor = Vendor {
    name: "Lian Li",
    app: "L-Connect",
    // Nexus's list, as executables.
    processes: &["l-connect 3.exe", "l-connect.exe", "lconnect3.exe", "lconnect.exe"],
};

const fn model(pid: u16, slug: &'static str, name: &'static str) -> Model {
    Model::vendor_hid(0x0cf2, pid, slug, name, &LIAN_LI, Usage::Exact(0xff72, 0xa1), open)
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
/// Ends a merged effect, one animation across every port.
const STOP_MERGE: [u8; 7] = [REPORT_ID, 0x10, 0x34, 0, 0, 0, 0];
const COLOR_REPORT_LEN: usize = 353;
/// The hub's static effect, which shows the colours as they came.
const EFFECT_STATIC: u8 = 0x01;
/// Brightness codes: full, and off.
const BRIGHTNESS_FULL: u8 = 0x00;
const BRIGHTNESS_OFF: u8 = 0x08;
/// L-Connect keeps each LED's R + G + B at most this, and scales brighter ones down.
const ENERGY_CAP: u16 = 460;
/// The SL-Infinity ignores a commit within ~5 ms of merge off.
const MERGE_OFF_SETTLE: Duration = Duration::from_millis(20);

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

/// How a hub takes its colours, from Nexus's `LianLiFanProfiles`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Leds {
    /// Byte 2 of the fan count command.
    quantity_register: u8,
    /// The fan count and the port in one byte.
    packed_quantity: bool,
    /// Two channels per port, the inner and the outer ring, or one.
    channels_per_port: usize,
    /// LEDs per fan on a port's first channel, and on its second.
    inner: usize,
    outer: usize,
    /// Colours over the interrupt pipe; the others take them over the control pipe.
    interrupt: bool,
    /// The fan count goes before every colour report, not once.
    count_per_frame: bool,
    /// Merge off when camless first lights the hub; on the SL-Infinity before
    /// every change.
    stop_merge_once: bool,
    stop_merge_always: bool,
    max_fans: usize,
    /// Byte 3 of the frame sync that makes the commits take.
    latch: u8,
}

fn leds(pid: u16) -> Option<Leds> {
    let sl_infinity = Leds {
        quantity_register: 0x60,
        packed_quantity: false,
        channels_per_port: 2,
        inner: 8,
        outer: 12,
        interrupt: false,
        count_per_frame: true,
        stop_merge_once: false,
        stop_merge_always: false,
        max_fans: 4,
        latch: 1,
    };
    let sl = Leds {
        quantity_register: 0x32,
        packed_quantity: true,
        channels_per_port: 1,
        inner: 16,
        outer: 0,
        interrupt: true,
        count_per_frame: false,
        stop_merge_once: true,
        ..sl_infinity
    };
    let sl_v2 = Leds { packed_quantity: true, channels_per_port: 1, inner: 16, outer: 0, max_fans: 6, latch: 4, ..sl_infinity };
    Some(match pid {
        0x7750 => sl_infinity,
        0xa100 | 0xa106 => sl,
        0xa101 => Leds { quantity_register: 0x40, ..sl_infinity },
        0xa102 => Leds { stop_merge_always: true, ..sl_infinity },
        0xa103 | 0xa105 => sl_v2,
        0xa104 => Leds { max_fans: 6, ..sl_infinity },
        _ => return None,
    })
}

impl Leds {
    /// The LEDs of a port's channel `k` with every fan the hub can chain there.
    fn on_channel(&self, k: usize) -> usize {
        self.max_fans * if k == 0 { self.inner } else { self.outer }
    }

    fn per_port(&self) -> usize {
        (0..self.channels_per_port).map(|k| self.on_channel(k)).sum()
    }

    /// How many fans the port chains, for the hub's animations; camless sends the
    /// most, and colours for that many.
    fn quantity(&self, port: usize) -> [u8; 7] {
        let n = self.max_fans as u8;
        if self.packed_quantity {
            [REPORT_ID, 0x10, self.quantity_register, (port as u8) << 4 | n, 0, 0, 0]
        } else {
            [REPORT_ID, 0x10, self.quantity_register, port as u8 + 1, n, 0, 0]
        }
    }

    fn frame_sync(&self) -> [u8; 7] {
        [REPORT_ID, 0x60, 0x00, self.latch, 0, 0, 0]
    }
}

/// The colour report for channel `ch`: `E0 3<ch>`, then R, B, G per LED, each under
/// the energy cap.
fn color_report(ch: usize, color: Rgb, count: usize) -> Vec<u8> {
    let [mut r, mut g, mut b] = color;
    let sum = u16::from(r) + u16::from(g) + u16::from(b);
    if sum > ENERGY_CAP {
        let scale = |c: u8| (f64::from(c) * f64::from(ENERGY_CAP) / f64::from(sum)) as u8;
        (r, g, b) = (scale(r), scale(g), scale(b));
    }
    let mut report = vec![0u8; COLOR_REPORT_LEN];
    report[0] = REPORT_ID;
    report[1] = 0x30 | ch as u8;
    for led in report[2..].chunks_exact_mut(3).take(count) {
        led.copy_from_slice(&[r, b, g]);
    }
    report
}

/// The static effect on channel `ch`, at a brightness code; default speed and
/// direction.
fn commit(ch: usize, brightness: u8) -> [u8; 7] {
    [REPORT_ID, 0x10 | ch as u8, EFFECT_STATIC, 0x00, 0x00, brightness, 0]
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

fn light_port(light: &str) -> Option<usize> {
    let n: usize = light.strip_prefix("port")?.parse().ok()?;
    (1..=PORTS).contains(&n).then(|| n - 1)
}

/// Writes go out a millisecond apart on Windows, as Nexus sends them; hidraw's
/// return after the transfer.
fn settle() {
    #[cfg(windows)]
    std::thread::sleep(Duration::from_millis(1));
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
    leds: Leds,
    /// The ports whose fan count camless sent, on the hubs that take it once.
    counted: [bool; PORTS],
    merge_stopped: bool,
}

fn open(api: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    let m = found.model;
    let profile = profile(m.pid).context("no profile for this hub")?;
    let leds = leds(m.pid).context("no lighting profile for this hub")?;
    let hid = Hid::open(api, &found.path)?;
    let mut d = LianLi {
        model: m,
        profile,
        hid,
        firmware: None,
        duties: [None; PORTS],
        manual: [false; PORTS],
        rpm: [None; PORTS],
        last_poll: SystemTime::now(),
        leds,
        counted: [false; PORTS],
        merge_stopped: false,
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

    /// One colour on every LED of a port, or off, as Nexus commits the static
    /// effect: the fan count, the colours and the commit per channel, then the
    /// frame sync.
    fn light(&mut self, port: usize, color: Rgb, brightness: u8) -> Result<()> {
        let l = self.leds;
        if l.stop_merge_always || l.stop_merge_once && !self.merge_stopped {
            self.hid.set_feature(&STOP_MERGE)?;
            self.merge_stopped = true;
            std::thread::sleep(if l.stop_merge_always { MERGE_OFF_SETTLE } else { Duration::from_millis(1) });
        }
        if !l.count_per_frame && !self.counted[port] {
            self.hid.set_feature(&l.quantity(port))?;
            self.counted[port] = true;
            settle();
        }
        for k in 0..l.channels_per_port {
            let ch = port * l.channels_per_port + k;
            if l.count_per_frame {
                self.hid.set_feature(&l.quantity(port))?;
                settle();
            }
            let report = color_report(ch, color, l.on_channel(k));
            if l.interrupt {
                self.hid.write(&report)?;
            } else {
                self.hid.set_output(&report)?;
            }
            settle();
            self.hid.set_feature(&commit(ch, brightness))?;
            settle();
        }
        self.hid.set_feature(&l.frame_sync())
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
            lights: (1..=PORTS)
                .map(|n| LightChannel {
                    id: format!("port{n}"),
                    name: format!("Port {n} lighting"),
                    leds: self.leds.per_port() as u16,
                    modes: vec!["off".into(), "fixed".into()],
                    ..Default::default()
                })
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

    fn set_lighting(&mut self, light: &str, lighting: &Lighting) -> Result<()> {
        let port = light_port(light).with_context(|| format!("no light {light:?} on this hub"))?;
        match lighting {
            Lighting::Off => self.light(port, [0, 0, 0], BRIGHTNESS_OFF),
            Lighting::Fixed(color) => self.light(port, *color, BRIGHTNESS_FULL),
            Lighting::Effect { .. } => bail!("this hub takes off or one colour"),
        }
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

    /// The Uni Hubs among Lian Li's models: not the TL controller or the Strimer's.
    fn uni_hubs() -> impl Iterator<Item = &'static Model> {
        super::super::MODELS.iter().filter(|m| m.slug.starts_with("lianli-uni-hub"))
    }

    #[test]
    fn every_model_has_a_profile() {
        assert_eq!(uni_hubs().count(), 8);
        for m in uni_hubs() {
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
        assert_eq!(["port1", "port4"].map(light_port), [Some(0), Some(3)]);
        assert_eq!(["port0", "port5", "fan1"].map(light_port), [None; 3]);
    }

    #[test]
    fn every_model_has_a_lighting_profile_from_nexus() {
        for m in uni_hubs() {
            let l = leds(m.pid).unwrap();
            // Every LED of a port fits one colour report.
            assert!((0..l.channels_per_port).all(|k| 2 + 3 * l.on_channel(k) <= COLOR_REPORT_LEN), "{:04x}", m.pid);
        }
        // SL-Infinity: two rings per fan, 8 and 12 LEDs, on channels 2p and 2p + 1.
        let inf = leds(0xa102).unwrap();
        assert_eq!((inf.on_channel(0), inf.on_channel(1), inf.per_port()), (32, 48, 80));
        // `BuildSetQuantity`: unpacked on the SL-Infinity and AL, packed on the SL and v2.
        assert_eq!(inf.quantity(2), [0xe0, 0x10, 0x60, 3, 4, 0, 0]);
        assert_eq!(leds(0xa101).unwrap().quantity(0), [0xe0, 0x10, 0x40, 1, 4, 0, 0]);
        assert_eq!(leds(0xa100).unwrap().quantity(3), [0xe0, 0x10, 0x32, 0x34, 0, 0, 0]);
        assert_eq!(leds(0xa103).unwrap().quantity(1), [0xe0, 0x10, 0x60, 0x16, 0, 0, 0]);
        // `BuildFrameSync`.
        assert_eq!(inf.frame_sync(), [0xe0, 0x60, 0, 1, 0, 0, 0]);
        assert_eq!(leds(0xa105).unwrap().frame_sync(), [0xe0, 0x60, 0, 4, 0, 0, 0]);
        assert!(leds(0xa100).unwrap().interrupt && !inf.interrupt);
        assert!(leds(0xa107).is_none());
    }

    #[test]
    fn colours_go_r_b_g_under_the_energy_cap() {
        let r = color_report(3, [0x10, 0x20, 0x30], 2);
        assert_eq!(r.len(), 353);
        assert_eq!(&r[..9], [0xe0, 0x33, 0x10, 0x30, 0x20, 0x10, 0x30, 0x20, 0]);
        assert!(r[8..].iter().all(|&b| b == 0));
        // White is 765, scaled to 460: 153 each, as `ApplyEnergyCap` truncates.
        assert_eq!(&color_report(0, [255, 255, 255], 1)[2..5], [153, 153, 153]);
        assert_eq!(&color_report(0, [255, 205, 0], 1)[2..5], [255, 0, 205]);
        // `BuildEffectCommit` with the static effect, full and off.
        assert_eq!(commit(5, BRIGHTNESS_FULL), [0xe0, 0x15, 1, 0, 0, 0, 0]);
        assert_eq!(commit(0, BRIGHTNESS_OFF), [0xe0, 0x10, 1, 0, 0, 8, 0]);
    }
}
