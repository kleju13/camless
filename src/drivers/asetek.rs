//! The first Kraken X coolers: the X31, X41 and X61, and the X40 and X60 before them.
//! Asetek 690LC pumps on USB id `2433:b200`, which EVGA's CLC coolers share. They are
//! not HID but a Silicon Labs USBXpress bridge with two bulk endpoints, so on Windows
//! the cooler needs WinUSB bound to it (Zadig), which also keeps CAM from it.
//!
//! These "legacy" 690LC take a fixed fan and pump duty and answer every command with
//! the liquid temperature and speeds. They store no curves: camless drives both from
//! the host, and sends the duties with every poll as liquidctl does with every status
//! read. Command `21` would save the settings to flash; camless never sends it.
//!
//! Sources: liquidctl `asetek.py` (`Legacy690Lc`) and its guide. Nexus leaves these
//! coolers out, so the driver is experimental. Not verified on hardware.

use super::{usb, Driver, FanReading, Found, Model, Reading};
use anyhow::{bail, Context, Result};
use camless_core::lighting::{Effect, Lighting, Rgb, Speed};
use camless_core::status::{Device, FanChannel, LightChannel, Unit};
use hidapi::HidApi;
use std::time::Duration;

pub const KRAKEN_X: Model =
    Model { experimental: true, ..Model::usb(0x2433, 0xb200, "kraken-x-asetek", "Kraken X31/X41/X61", open) };

const READ_ENDPOINT: u8 = 0x82;
const WRITE_ENDPOINT: u8 = 0x02;
const READ_LEN: usize = 32;
const TIMEOUT: Duration = Duration::from_secs(1);
/// The USBXpress vendor request and its values, from the USBXpress SDK by way of
/// liquidctl.
const USBXPRESS_REQUEST: u8 = 0x02;
const FLUSH_BUFFERS: u16 = 0x01;
const CLEAR_TO_SEND: u16 = 0x02;
const CMD_CONFIGURE: u8 = 0x10;
const CMD_FAN: u8 = 0x12;
const CMD_PUMP: u8 = 0x13;
const PUMP_MIN_DUTY: u8 = 50;
/// The cooler's own warning: the logo turns this color from this liquid temperature,
/// liquidctl's defaults.
const ALERT_C: u8 = 45;
const ALERT_COLOR: Rgb = [255, 0, 0];

/// The `10` command: three colors, the alert threshold, two intervals in seconds, and
/// on/fading/blinking/alert flags. It sets the lighting; liquidctl sends it to
/// initialize the cooler too.
fn configure(lighting: &Lighting) -> Vec<u8> {
    let black = [0, 0, 0];
    let (c1, c2, interval, fading) = match lighting {
        Lighting::Off => (black, black, 0, false),
        Lighting::Fixed(c) => (*c, black, 0, false),
        Lighting::Effect { colors, speed, .. } => {
            let c1 = colors.first().copied().unwrap_or(black);
            (c1, colors.get(1).copied().unwrap_or(c1), seconds_per_color(*speed), true)
        }
    };
    let mut msg = vec![CMD_CONFIGURE];
    for c in [c1, c2, ALERT_COLOR] {
        msg.extend_from_slice(&c);
    }
    // Lights on, so the alert still shows; liquidctl's blackout would hide it.
    msg.extend_from_slice(&[ALERT_C, interval, 0, 1, fading as u8, 0, 1, 0x00, 0x01]);
    msg
}

/// liquidctl's default on these is 5 s per color.
fn seconds_per_color(speed: Speed) -> u8 {
    [10, 7, 5, 3, 1][speed.index()]
}

#[derive(Clone, Debug, PartialEq)]
struct Status {
    liquid_c: f32,
    fan_rpm: u16,
    pump_rpm: u16,
    firmware: String,
}

fn parse_status(msg: &[u8]) -> Option<Status> {
    if msg.len() < 0x1b {
        return None;
    }
    Some(Status {
        liquid_c: f32::from(msg[10]) + f32::from(msg[14]) / 10.0,
        fan_rpm: u16::from_be_bytes([msg[0], msg[1]]),
        pump_rpm: u16::from_be_bytes([msg[8], msg[9]]),
        firmware: format!("{}.{}.{}.{}", msg[0x17], msg[0x18], msg[0x19], msg[0x1a]),
    })
}

pub struct Asetek {
    model: &'static Model,
    usb: nusb::Interface,
    /// Pump and fan. liquidctl starts both at 100 % until it is told otherwise.
    duties: [u8; 2],
    firmware: Option<String>,
}

fn open(_: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    let m = found.model;
    let usb = usb::open(m.vid, m.pid, found.path.to_str().unwrap_or_default())?;
    usb::vendor_out(&usb, USBXPRESS_REQUEST, CLEAR_TO_SEND, TIMEOUT)?;
    let mut d = Asetek { model: m, usb, duties: [100, 100], firmware: None };
    d.transaction(&[configure(&Lighting::Off)])?;
    d.firmware = Some(d.speeds()?.firmware);
    Ok(Box::new(d))
}

impl Asetek {
    /// Flush, the writes, and the reply that ends every transaction.
    fn transaction(&self, writes: &[Vec<u8>]) -> Result<Vec<u8>> {
        usb::vendor_out(&self.usb, USBXPRESS_REQUEST, FLUSH_BUFFERS, TIMEOUT)?;
        for w in writes {
            usb::bulk_out(&self.usb, WRITE_ENDPOINT, w.clone(), TIMEOUT)?;
        }
        usb::bulk_in(&self.usb, READ_ENDPOINT, READ_LEN, TIMEOUT)
    }

    /// Both duties, as liquidctl sends them for every status.
    fn speeds(&self) -> Result<Status> {
        let msg = self.transaction(&[vec![CMD_PUMP, self.duties[0]], vec![CMD_FAN, self.duties[1]]])?;
        parse_status(&msg).context("a short reply from the cooler")
    }
}

impl Driver for Asetek {
    fn describe(&self) -> Device {
        let fan = |id: &str, name: &str, pump: bool| FanChannel {
            id: id.into(),
            name: name.into(),
            pump,
            min_duty: if pump { PUMP_MIN_DUTY } else { 0 },
            device_curves: false,
            ..Default::default()
        };
        Device {
            name: self.model.name.into(),
            firmware: self.firmware.clone(),
            verified: self.model.verified(),
            experimental: self.model.is_experimental(),
            fans: vec![fan("pump", "Pump", true), fan("fan", "Radiator fans", false)],
            lights: vec![LightChannel {
                id: "logo".into(),
                name: "Pump logo".into(),
                leds: 1,
                modes: vec!["off".into(), "fixed".into(), Effect::Fading.name().into()],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn sensors(&self) -> Vec<(&'static str, &'static str, Unit)> {
        vec![("liquid", "Liquid", Unit::Celsius)]
    }

    fn poll(&mut self) -> Result<Reading> {
        let s = self.speeds()?;
        let fan = |rpm, duty| FanReading { rpm: Some(rpm), duty: Some(duty), connected: None };
        Ok(Reading {
            sensors: vec![("liquid", s.liquid_c)],
            fans: vec![("pump", fan(s.pump_rpm, self.duties[0])), ("fan", fan(s.fan_rpm, self.duties[1]))],
        })
    }

    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
        for &(fan, duty) in duties {
            match fan {
                "pump" => self.duties[0] = duty.clamp(PUMP_MIN_DUTY, 100),
                "fan" => self.duties[1] = duty.min(100),
                _ => bail!("no fan {fan:?} on this cooler"),
            }
        }
        self.speeds().map(|_| ())
    }

    fn set_lighting(&mut self, light: &str, lighting: &Lighting) -> Result<()> {
        if light != "logo" {
            bail!("no light {light:?} on this cooler");
        }
        if let Lighting::Effect { effect, .. } = lighting {
            if *effect != Effect::Fading {
                bail!("this cooler has no {} effect, only fading", effect.label());
            }
        }
        self.transaction(&[configure(lighting)]).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_is_liquidctls_19_bytes() {
        // `initialize`: black, black, red alert at 45 °C, on, alert enabled.
        let init = configure(&Lighting::Off);
        assert_eq!(init, [0x10, 0, 0, 0, 0, 0, 0, 255, 0, 0, 45, 0, 0, 1, 0, 0, 1, 0, 1]);
        // `set logo color fixed 102030`.
        let fixed = configure(&Lighting::Fixed([0x10, 0x20, 0x30]));
        assert_eq!(&fixed[1..4], &[0x10, 0x20, 0x30]);
        assert_eq!(&fixed[10..], &init[10..]);
        // `set logo color fading 010203 040506`: liquidctl's legacy default of 5 s.
        let fade = Lighting::Effect { effect: Effect::Fading, colors: vec![[1, 2, 3], [4, 5, 6]], speed: Speed::Normal, backward: false };
        let f = configure(&fade);
        assert_eq!(&f[1..7], &[1, 2, 3, 4, 5, 6]);
        assert_eq!((f[11], f[14]), (5, 1));
    }

    #[test]
    fn status_reads_like_liquidctl() {
        let mut msg = [0u8; READ_LEN];
        msg[..2].copy_from_slice(&[0x01, 0xe0]);
        msg[8..11].copy_from_slice(&[0x07, 0x62, 28]);
        msg[14] = 7;
        msg[0x17..0x1b].copy_from_slice(&[2, 10, 0, 0]);
        let s = parse_status(&msg).unwrap();
        assert_eq!((s.liquid_c, s.fan_rpm, s.pump_rpm), (28.7, 480, 1890));
        assert_eq!(s.firmware, "2.10.0.0");
        assert_eq!(parse_status(&msg[..20]), None);
    }
}
