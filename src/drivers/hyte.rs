//! HYTE's Smart Hub (`3402:0904`): four PWM fan headers and four ARGB ports, on a
//! USB serial port. Another brand's hub: camless opens it only with
//! `[devices] other_brands` on, and leaves it to HYTE Nexus while that runs.
//!
//! Commands start `FF CC` (control), `FF DD` (version) or `FF EE` (LEDs). A fan
//! takes its duty with `FF CC 02 <port> <duty> 01`, and only for 5 seconds: then the
//! hub's watchdog puts every fan back on the duty saved in its flash. So camless sends
//! the duties it set with every poll, and leaves the other headers to the hub, and
//! when camless stops or lets go the hub takes them all back. A port's LEDs go out as
//! one 607-byte frame, `FF EE 01 <port> 00 00 00` and 200 LEDs in G, R, B order; the
//! hub's own animation stays off on every port while frames come, and comes back 5
//! seconds after the last. So once the config sets a light, camless sends every
//! port's frame with every poll, the ports it does not set off. A poll interval above
//! about 4 seconds loses both.
//!
//! camless never sends `FF CC 07` (the animation's on/off, saved in flash) or
//! `FF CC 0C` (the settings saved in flash), nor the firmware update.
//!
//! Sources: Nexus (`src/Peripherals/Hyte/SmartHub`), from HYTE's own service. Not
//! verified on hardware.

use super::serial::{self, Serial};
use super::{Driver, FanReading, Found, Model, Reading, Vendor};
use anyhow::{bail, Context, Result};
use camless_core::lighting::{Lighting, Rgb};
use camless_core::status::{Device, FanChannel, LightChannel, Unit};
use hidapi::HidApi;
use std::time::Duration;

pub const HYTE: Vendor = Vendor {
    name: "HYTE",
    app: "HYTE Nexus",
    // Nexus's list, as executables, and HYTE Nexus 3, which is `Nexus` on Linux.
    processes: &["hyte nexus.exe", "hyte.nexus.service.exe", "nexus.exe", "nexus"],
};

pub const SMART_HUB: Model = Model::vendor_serial(0x3402, 0x0904, "hyte-smart-hub", "HYTE Smart Hub", &HYTE, open);

const PORTS: usize = 4;
/// The firmware's LEDs per port; every frame carries this many.
const MAX_LEDS: usize = 200;
const FRAME_LEN: usize = 7 + MAX_LEDS * 3;
const INFO_LEN: usize = 20;
const FIRMWARE_LEN: usize = 7;
const REPLY_TIMEOUT: Duration = Duration::from_millis(300);

const GET_INFO: [u8; 4] = [0xff, 0xcc, 0x01, 0x00];
const GET_FIRMWARE: [u8; 3] = [0xff, 0xdd, 0x02];

/// `FF CC 02 <port 1..4> <duty> <enabled>`.
fn set_fan(port: usize, duty: u8) -> [u8; 6] {
    [0xff, 0xcc, 0x02, port as u8, duty.min(100), 0x01]
}

/// A port's frame: every LED in one colour, G, R, B.
fn frame(port: usize, color: Rgb) -> Vec<u8> {
    let mut f = vec![0u8; FRAME_LEN];
    f[..4].copy_from_slice(&[0xff, 0xee, 0x01, port as u8]);
    for led in f[7..].chunks_exact_mut(3) {
        led.copy_from_slice(&[color[1], color[0], color[2]]);
    }
    f
}

/// `GetFanRPM`: the tach period in hundredths, 0 for no signal.
pub(super) fn rpm(high: u8, low: u8) -> u16 {
    if (high, low) == (0, 0) {
        return 0;
    }
    ((60_000.0 / (f32::from(high) + f32::from(low) / 100.0)) as u32 / 4).min(u32::from(u16::MAX)) as u16
}

/// `FF DD 02 a b c d`; Nexus does not check the third byte.
pub(super) fn parse_firmware(reply: &[u8]) -> Option<String> {
    match reply {
        [0xff, 0xdd, _, a, b, c, d, ..] => Some(format!("{a}.{b}.{c}.{d}")),
        _ => None,
    }
}

/// The tach of each fan header at 3 + 2 × port. The enabled flags after them are
/// set for every port whatever is plugged in.
fn parse_info(reply: &[u8]) -> Option<[u16; PORTS]> {
    if reply.len() < INFO_LEN || reply[..2] != [0xff, 0xcc] {
        return None;
    }
    Some(std::array::from_fn(|ch| rpm(reply[3 + ch * 2], reply[4 + ch * 2])))
}

pub struct SmartHub {
    port: Serial,
    firmware: Option<String>,
    duties: [Option<u8>; PORTS],
    colors: [Option<Rgb>; PORTS],
}

fn open(_: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    let mut port = Serial::open(&found.path)?;
    let info = serial::request(&mut port, &GET_INFO, INFO_LEN, REPLY_TIMEOUT)?;
    parse_info(&info).context("no hub info reply; not a Smart Hub, or it is busy")?;
    let firmware = parse_firmware(&serial::request(&mut port, &GET_FIRMWARE, FIRMWARE_LEN, REPLY_TIMEOUT)?);
    Ok(Box::new(SmartHub { port, firmware, duties: [None; PORTS], colors: [None; PORTS] }))
}

/// `fan1` → 1.
pub(super) fn port_of(id: &str, prefix: &str, ports: usize) -> Option<usize> {
    id.strip_prefix(prefix)?.parse().ok().filter(|n| (1..=ports).contains(n))
}

impl SmartHub {
    fn send(&mut self) -> Result<()> {
        for p in 1..=PORTS {
            if let Some(duty) = self.duties[p - 1] {
                self.port.write(&set_fan(p, duty))?;
            }
        }
        self.send_frames()
    }

    /// Every port's frame once the config sets a light: one frame keeps the hub's
    /// animation off on all four, so the ports it does not set are off.
    fn send_frames(&mut self) -> Result<()> {
        if self.colors.iter().all(Option::is_none) {
            return Ok(());
        }
        for p in 1..=PORTS {
            self.port.write(&frame(p, self.colors[p - 1].unwrap_or([0, 0, 0])))?;
        }
        Ok(())
    }
}

impl Driver for SmartHub {
    fn describe(&self) -> Device {
        Device {
            name: SMART_HUB.name.into(),
            firmware: self.firmware.clone(),
            verified: SMART_HUB.verified(),
            experimental: SMART_HUB.is_experimental(),
            fans: (1..=PORTS)
                .map(|p| FanChannel { id: format!("fan{p}"), name: format!("Fan port {p}"), ..Default::default() })
                .collect(),
            lights: (1..=PORTS)
                .map(|p| LightChannel {
                    id: format!("argb{p}"),
                    name: format!("ARGB port {p}"),
                    leds: MAX_LEDS as u16,
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
        let reply = serial::request(&mut self.port, &GET_INFO, INFO_LEN, REPLY_TIMEOUT)?;
        let rpm = parse_info(&reply).context("no hub info reply")?;
        // The watchdogs take the fans and the LEDs back 5 s after the last write.
        self.send()?;
        const IDS: [&str; PORTS] = ["fan1", "fan2", "fan3", "fan4"];
        let fans = (0..PORTS)
            .map(|i| (IDS[i], FanReading { rpm: Some(rpm[i]), duty: self.duties[i], connected: None }))
            .collect();
        Ok(Reading { sensors: Vec::new(), fans })
    }

    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
        for &(id, duty) in duties {
            let p = port_of(id, "fan", PORTS).with_context(|| format!("no fan {id:?} on the hub"))?;
            self.duties[p - 1] = Some(duty.min(100));
            self.port.write(&set_fan(p, duty))?;
        }
        Ok(())
    }

    fn set_lighting(&mut self, light: &str, lighting: &Lighting) -> Result<()> {
        let p = port_of(light, "argb", PORTS).with_context(|| format!("no light {light:?} on the hub"))?;
        let color = match lighting {
            Lighting::Off => [0, 0, 0],
            Lighting::Fixed(c) => *c,
            Lighting::Effect { .. } => bail!("the Smart Hub takes off or one colour"),
        };
        self.colors[p - 1] = Some(color);
        self.send_frames()
    }

    /// The hub's watchdogs take the fans and the LEDs back on their own.
    fn release(&mut self) -> Result<()> {
        self.duties = [None; PORTS];
        self.colors = [None; PORTS];
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_follow_nexus_builders() {
        assert_eq!(set_fan(1, 40), [0xff, 0xcc, 0x02, 0x01, 40, 0x01]);
        assert_eq!(set_fan(4, 150), [0xff, 0xcc, 0x02, 0x04, 100, 0x01]);
        let f = frame(3, [0x10, 0x20, 0x30]);
        assert_eq!(f.len(), 607);
        assert_eq!(&f[..10], [0xff, 0xee, 0x01, 0x03, 0, 0, 0, 0x20, 0x10, 0x30]);
        assert_eq!(&f[604..], [0x20, 0x10, 0x30]);
    }

    #[test]
    fn replies_give_the_firmware_and_each_tach() {
        assert_eq!(parse_firmware(&[0xff, 0xdd, 0x02, 1, 0, 0, 1]), Some("1.0.0.1".into()));
        assert_eq!(parse_firmware(&[0xff, 0xcc, 0x02, 1, 0, 0, 1]), None);
        assert_eq!(parse_firmware(&[0xff, 0xdd, 0x02, 1]), None);
        let mut info = [0u8; 20];
        info[..3].copy_from_slice(&[0xff, 0xcc, 0x01]);
        // 60000 / 12.5 / 4 = 1200 rpm on port 1; port 2 reads nothing.
        info[3..5].copy_from_slice(&[12, 50]);
        info[7..9].copy_from_slice(&[30, 0]);
        assert_eq!(parse_info(&info), Some([1200, 0, 500, 0]));
        assert_eq!(parse_info(&info[..19]), None);
        assert_eq!(port_of("fan4", "fan", 4), Some(4));
        assert_eq!(port_of("fan5", "fan", 4), None);
        assert_eq!(port_of("argb0", "argb", 4), None);
    }
}
