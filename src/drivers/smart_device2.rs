//! NZXT Smart Device V2 (`1e71:2006`, `200d`, `200f`) and RGB & Fan Controller
//! (`2009`, `200e`, `2010`; the 3+6 channel one `2011`, `2019`, `201f`, `2020`): the
//! Hue 2 fan and LED controllers before the Control Hub, with three fan ports and two
//! or six LED channels. The Smart Device V2 came in NZXT's cases from mid-2019, such
//! as the H510 Elite, H510i, H710i and H210i (liquidctl).
//!
//! The fans and accessories work as on the Control Hub: the same `67 02` fan report,
//! `62 01` duties and `21 03` accessory report, which `control_hub.rs` and `hue2.rs`
//! build and parse. The rest is older: the fan reports start after `60 03` where the
//! hub takes `66 01`, the firmware comes from `10 01`, and the lights take the `28 03`
//! layout on channel bits `1 << i`.
//!
//! The 3+6 channel controllers `2011`, `2019` and `201f` get no lighting: they ignore
//! `28 03`, and liquidctl disabled their lights for it (#541). Their fans work.
//!
//! Sources: liquidctl `smart_device.py` (`SmartDevice2`), the Linux `nzxt-smart2`
//! driver, and liquidctl #541, which logged liquidctl's writes to a `2011` and its
//! replies. Nexus drives none of them, so they are experimental. Not verified on
//! hardware.

use super::control_hub::{self, duties_report, Status};
use super::{hid, hue2, Driver, Found, Model, Reading, NZXT_VID};
use anyhow::{bail, Context, Result};
use camless_core::lighting::Lighting;
use camless_core::status::{Device, FanChannel, LightChannel, Unit};
use hidapi::{HidApi, HidDevice};
use std::time::Instant;

const fn model(pid: u16, slug: &'static str, name: &'static str) -> Model {
    Model { experimental: true, ..Model::hid(NZXT_VID, pid, slug, name, open) }
}

pub const SMART_DEVICE_V2: Model = model(0x2006, "smart-device-v2", "Smart Device V2");
pub const SMART_DEVICE_V2_200D: Model = model(0x200d, "smart-device-v2", "Smart Device V2");
pub const SMART_DEVICE_V2_200F: Model = model(0x200f, "smart-device-v2", "Smart Device V2");
pub const RGB_FAN_CONTROLLER: Model = model(0x2009, "rgb-fan-controller", "RGB & Fan Controller");
pub const RGB_FAN_CONTROLLER_200E: Model = model(0x200e, "rgb-fan-controller", "RGB & Fan Controller");
pub const RGB_FAN_CONTROLLER_2010: Model = model(0x2010, "rgb-fan-controller", "RGB & Fan Controller");
pub const RGB_FAN_CONTROLLER_6: Model = model(0x2011, "rgb-fan-controller-6", "RGB & Fan Controller (3+6 channels)");
pub const RGB_FAN_CONTROLLER_6_2019: Model = model(0x2019, "rgb-fan-controller-6", "RGB & Fan Controller (3+6 channels)");
pub const RGB_FAN_CONTROLLER_6_201F: Model = model(0x201f, "rgb-fan-controller-6", "RGB & Fan Controller (3+6 channels)");
pub const RGB_FAN_CONTROLLER_6_2020: Model = model(0x2020, "rgb-fan-controller-6", "RGB & Fan Controller (3+6 channels)");

/// Every one has three fan ports (liquidctl, Linux).
pub const FANS: usize = 3;

/// The LED channels camless drives, from liquidctl's `_MATCHES`; `None` for another id.
pub fn led_channels(pid: u16) -> Option<usize> {
    match pid {
        0x2006 | 0x200d | 0x200f | 0x2009 | 0x200e | 0x2010 => Some(2),
        // Six on the board, but they ignore `28 03` (liquidctl#541).
        0x2011 | 0x2019 | 0x201f => Some(0),
        // liquidctl: "Both RGB and fan control have been reported to work".
        0x2020 => Some(6),
        _ => None,
    }
}

/// Fan reports every 0.5 s in both slots, then fan detection: liquidctl's
/// `initialize`, as logged in liquidctl#541. The Linux driver sends the two the other
/// way round.
const INIT: [&[u8]; 2] = [&[0x60, 0x02, 0x01, 0xe8, 0x01, 0x01, 0xe8, 0x01], &[0x60, 0x03]];

/// `11 01`: the version at [0x11..0x13].
pub fn parse_firmware(msg: &[u8]) -> Option<String> {
    (msg.len() > 0x13 && msg[..2] == [0x11, 0x01]).then(|| format!("{}.{}.{}", msg[0x11], msg[0x12], msg[0x13]))
}

/// LED channel `i` (from 0) as liquidctl's channel byte.
fn channel_bit(i: usize) -> u8 {
    1 << i
}

pub struct SmartDevice2 {
    model: &'static Model,
    hid: HidDevice,
    firmware: Option<String>,
    accessories: Vec<hue2::ChannelAccessories>,
    last: Option<(Status, Instant)>,
}

fn open(api: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    let lights = led_channels(found.model.pid).context("not a Smart Device V2 or RGB & Fan Controller")?;
    let hid = hid::open(api, &found.path)?;
    hid::drain(&hid);
    for msg in INIT {
        hid::write(&hid, msg)?;
    }
    let firmware = parse_firmware(&hid::request(&hid, &[0x10, 0x01], &[0x11, 0x01])?);
    let accessories = if lights == 0 {
        Vec::new()
    } else {
        hue2::read_accessories(&hid, lights).unwrap_or_else(|_| vec![Default::default(); lights])
    };
    Ok(Box::new(SmartDevice2 { model: found.model, hid, firmware, accessories, last: None }))
}

impl Driver for SmartDevice2 {
    fn describe(&self) -> Device {
        Device {
            name: self.model.name.into(),
            firmware: self.firmware.clone(),
            verified: self.model.verified(),
            experimental: self.model.is_experimental(),
            fans: (1..=FANS)
                .map(|n| FanChannel { id: format!("fan{n}"), name: format!("Fan {n}"), ..Default::default() })
                .collect(),
            lights: self
                .accessories
                .iter()
                .enumerate()
                .map(|(i, a)| LightChannel {
                    id: format!("led{}", i + 1),
                    name: format!("LED {}", i + 1),
                    leds: a.leds(),
                    accessories: a.names(),
                    modes: hue2::modes_28(),
                })
                .collect(),
            ..Default::default()
        }
    }

    fn sensors(&self) -> Vec<(&'static str, &'static str, Unit)> {
        Vec::new()
    }

    fn poll(&mut self) -> Result<Reading> {
        Ok(control_hub::reading(&control_hub::status(&self.hid, &mut self.last, FANS)?))
    }

    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
        let mut ports = Vec::new();
        for &(fan, duty) in duties {
            ports.push((control_hub::port(fan, "fan", FANS).with_context(|| format!("no fan {fan:?}; ports are fan1 to fan3"))?, duty));
        }
        if !ports.is_empty() {
            hid::write(&self.hid, &duties_report(&ports))?;
        }
        Ok(())
    }

    fn set_lighting(&mut self, light: &str, lighting: &Lighting) -> Result<()> {
        let count = self.accessories.len();
        let Some(i) = control_hub::port(light, "led", count) else {
            bail!("no light {light:?} on the {}", self.model.name)
        };
        hue2::send(&self.hid, vec![hue2::packet_28(channel_bit(i), lighting)?])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::MODELS;

    /// liquidctl's `SmartDevice2._MATCHES`, less the HUE 2 and HUE 2 Ambient, which
    /// have no fans: (id, fan channels, LED channels).
    const LIQUIDCTL: [(u16, usize, usize); 10] = [
        (0x2006, 3, 2),
        (0x200d, 3, 2),
        (0x200f, 3, 2),
        (0x2009, 3, 2),
        (0x200e, 3, 2),
        (0x2010, 3, 2),
        (0x2011, 3, 0),
        (0x2019, 3, 0),
        (0x201f, 3, 0),
        (0x2020, 3, 6),
    ];

    #[test]
    fn channels_follow_liquidctl() {
        for (pid, fans, leds) in LIQUIDCTL {
            assert_eq!((FANS, led_channels(pid)), (fans, Some(leds)), "{pid:04x}");
            let m = MODELS.iter().find(|m| (m.vid, m.pid) == (NZXT_VID, pid)).expect("listed in MODELS");
            assert!(m.experimental && m.vendor.is_none(), "{pid:04x}");
        }
        // The Control Hub, the HUE 2 and HUE 2 Ambient, and OpenRGB's B650E board.
        for pid in [0x2022, 0x2001, 0x2002, 0x201b] {
            assert_eq!(led_channels(pid), None);
        }
    }

    #[test]
    fn init_matches_liquidctls_log() {
        // liquidctl#541: report 60 with 02 01 e8 01 01 e8 01, then 60 03, then 10 01.
        assert_eq!(INIT[0], [0x60, 0x02, 0x01, 0xe8, 0x01, 0x01, 0xe8, 0x01]);
        assert_eq!(INIT[1], [0x60, 0x03]);
    }

    #[test]
    fn firmware_from_a_2011() {
        // The `11 01` reply in liquidctl#541, which liquidctl read as 1.7.0; the serial
        // number in bytes 2..14 is zeroed here.
        let mut msg = vec![0u8; 64];
        msg[..2].copy_from_slice(&[0x11, 0x01]);
        msg[14..20].copy_from_slice(&[0x11, 0x20, 0x01, 0x01, 0x07, 0x00]);
        assert_eq!(parse_firmware(&msg).as_deref(), Some("1.7.0"));
        assert_eq!(parse_firmware(&msg[..0x13]), None);
        msg[1] = 0x02;
        assert_eq!(parse_firmware(&msg), None);
    }

    #[test]
    fn accessories_from_a_2011() {
        // The `21 03` reply in liquidctl#541: six channels, three F140 RGB, then three
        // F120 RGB. The serial number in bytes 2..14 is zeroed here.
        let mut msg = vec![0u8; 64];
        msg[..2].copy_from_slice(&[0x21, 0x03]);
        msg[14] = 0x06;
        for (c, id) in [0x14, 0x14, 0x14, 0x13, 0x13, 0x13].into_iter().enumerate() {
            msg[15 + 6 * c] = id;
        }
        let ch = hue2::parse_accessories(&msg, 6);
        assert_eq!(ch[0].names(), ["F140 RGB"]);
        assert_eq!(ch[5].names(), ["F120 RGB"]);
        assert!(ch.iter().all(|c| c.ids.len() == 1));
    }

    #[test]
    fn fan_report_with_three_ports() {
        // The Linux driver's `fan_status_report`: id, type 02, 14 bytes, fan types from
        // 16, rpm LE from 24, duty from 40, the duty again from 48, noise at 56.
        let mut m = vec![0u8; 64];
        m[..2].copy_from_slice(&[0x67, 0x02]);
        m[16..19].copy_from_slice(&[2, 1, 0]);
        m[24..26].copy_from_slice(&1450u16.to_le_bytes());
        m[26..28].copy_from_slice(&700u16.to_le_bytes());
        m[40..43].copy_from_slice(&[60, 40, 35]);
        m[56] = 31;
        let s = control_hub::parse_status(&m, FANS).unwrap();
        assert_eq!(s.fans.len(), 3);
        assert_eq!(s.fans[0], control_hub::Port { mode: control_hub::FanMode::Pwm, rpm: 1450, duty: 60 });
        assert_eq!(s.fans[1].mode, control_hub::FanMode::Dc);
        assert_eq!(s.fans[2], control_hub::Port { mode: control_hub::FanMode::None, rpm: 0, duty: 35 });
        assert_eq!(s.noise_db, 31);
        let r = control_hub::reading(&s);
        assert_eq!(r.fans.iter().map(|f| f.0).collect::<Vec<_>>(), ["fan1", "fan2", "fan3"]);
        assert_eq!(r.fans[2].1.connected, Some(false));
        // The voltage report, type 04, is not the fan report.
        m[1] = 0x04;
        assert!(control_hub::parse_status(&m, FANS).is_none());
    }

    #[test]
    fn duties_match_liquidctl_and_linux() {
        // liquidctl `_write_fixed_duty(2, 85)`: 62 01 04 00 00 55, padded with zeros;
        // Linux `set_pwm` sends the same with room for eight duties.
        let mut sent = duties_report(&[(2, 85)]);
        sent.resize(hid::REPORT_LEN, 0);
        let mut want = vec![0x62, 0x01, 0x04, 0x00, 0x00, 0x55];
        want.resize(hid::REPORT_LEN, 0);
        assert_eq!(sent, want);
        assert_eq!(control_hub::port("fan3", "fan", FANS), Some(2));
        assert_eq!(control_hub::port("fan4", "fan", FANS), None);
    }

    #[test]
    fn led_channels_are_bits() {
        // liquidctl: led1 is 1 << 0; led6 went out as 20 in liquidctl#541.
        assert_eq!((0..6).map(channel_bit).collect::<Vec<_>>(), [0x01, 0x02, 0x04, 0x08, 0x10, 0x20]);
    }
}
