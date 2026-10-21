//! The NZXT Smart Device (V1, `1e71:1714`), in the H200i, H400i, H500i and H700i
//! cases, and the Grid+ V3 (`1e71:1711`): three or six fan ports, and on the Smart
//! Device one LED channel. They predate the Hue 2 family and share the Kraken X2's
//! generation of commands: `01 5c` and `01 5d` start them, each `04` report carries
//! one fan, a duty is `02 4d` and lighting is `02 4b`.
//!
//! Every setting lasts while the device has power (liquidctl's guide), so nothing here
//! writes flash. The device stores no curves: camless drives the fans from the host,
//! and a fan keeps its last duty when camless stops, as on the Control Hub.
//!
//! Sources: liquidctl `smart_device.py` (`SmartDevice`), its tests and guide, and the
//! liquidtux `nzxt-grid3` hwmon driver. Nexus leaves both devices out, so the driver
//! is experimental. Not verified on hardware.

use super::{hid, Driver, FanReading, Found, Model, Reading, NZXT_VID};
use anyhow::{bail, Context, Result};
use camless_core::lighting::{Effect, Lighting, Rgb};
use camless_core::status::{Device, FanChannel, LightChannel, Unit};
use hidapi::{HidApi, HidDevice};
use std::time::{Duration, Instant};

pub const SMART_DEVICE: Model =
    Model { experimental: true, ..Model::hid(NZXT_VID, 0x1714, "smart-device", "Smart Device", open) };
pub const GRID_V3: Model = Model { experimental: true, ..Model::hid(NZXT_VID, 0x1711, "grid-v3", "Grid+ V3", open) };

/// liquidctl pads every write to the report id and 64 bytes.
const WRITE_LEN: usize = 65;
/// One fan's speed, voltage, current and control mode, five times a second once the
/// device is started (the Linux driver).
const STATUS_ID: u8 = 0x04;
/// The Linux driver's `STATUS_VALIDITY`.
const STALE: Duration = Duration::from_secs(3);
/// The device takes 40 LEDs on its one channel: four HUE+ strips or five Aer RGB fans.
const MAX_LEDS: u16 = 40;

fn fans(model: &Model) -> usize {
    if model.pid == GRID_V3.pid {
        6
    } else {
        3
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Not detected: liquidctl's `None`. The device still measures the port.
    Unknown,
    Dc,
    Pwm,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Port {
    rpm: u16,
    mode: Mode,
    volts: f32,
    amps: f32,
}

/// One status report: the fan it is about, 0-based, and its readings.
fn parse_status(msg: &[u8]) -> Option<(usize, Port)> {
    if msg.len() < 16 || msg[0] != STATUS_ID {
        return None;
    }
    let mode = match msg[15] & 0x3 {
        1 => Mode::Dc,
        2 => Mode::Pwm,
        _ => Mode::Unknown,
    };
    let port = Port {
        rpm: u16::from_be_bytes([msg[3], msg[4]]),
        mode,
        volts: f32::from(msg[7]) + f32::from(msg[8]) / 100.0,
        amps: f32::from(msg[9]) + f32::from(msg[10]) / 100.0,
    };
    Some((usize::from(msg[15] >> 4), port))
}

/// What a status report says about the device: its firmware, CAM's short form as
/// liquidctl prints it, and on the Smart Device how many LEDs are chained.
fn parse_info(msg: &[u8]) -> Option<(String, u16)> {
    if msg.len() < 0x12 || msg[0] != STATUS_ID {
        return None;
    }
    let firmware = format!("{}.{}", msg[0xb], msg[0xe]);
    // A HUE+ strip has 10 LEDs, an Aer RGB fan 8 (liquidctl).
    let per = if msg[0x10] >> 3 == 1 { 8 } else { 10 };
    Some((firmware, u16::from(msg[0x11]) * per))
}

/// liquidctl's mode byte and color counts for the camless effects the device has.
/// (mode, fewest colors, most colors)
fn mode(effect: Effect) -> Option<(u8, usize, usize)> {
    Some(match effect {
        Effect::Fading => (0x01, 1, 8),
        Effect::SpectrumWave => (0x02, 0, 0),
        Effect::CoveringMarquee => (0x04, 1, 8),
        Effect::Pulse => (0x06, 1, 8),
        Effect::Breathing => (0x07, 1, 8),
        Effect::Candle => (0x09, 1, 1),
        _ => return None,
    })
}

fn modes() -> Vec<String> {
    let effects = Effect::ALL.into_iter().filter(|&e| mode(e).is_some()).map(|e| e.name().to_string());
    ["off", "fixed"].into_iter().map(String::from).chain(effects).collect()
}

/// Two reports per color step, `02 4b` with the first 57 bytes of 40 GRB LEDs and
/// `03` with the rest. Every step sets all 40 LEDs to its color (liquidctl).
fn lighting_reports(lighting: &Lighting) -> Result<Vec<Vec<u8>>> {
    let (mode, mut colors, speed, backward) = match lighting {
        Lighting::Off => (0x00, vec![[0, 0, 0]], 2, false),
        Lighting::Fixed(c) => (0x00, vec![*c], 2, false),
        Lighting::Effect { effect, colors, speed, backward } => {
            let Some((mode, min, max)) = self::mode(*effect) else {
                bail!("the Smart Device has no {} effect", effect.label())
            };
            let mut colors: Vec<Rgb> = colors.iter().copied().take(max).collect();
            if max == 0 {
                colors = vec![[0, 0, 0]];
            }
            while colors.len() < min {
                colors.push(colors.last().copied().unwrap_or_default());
            }
            (mode, colors, speed.index() as u8, *backward && effect.has_direction())
        }
    };
    colors.truncate(8);
    let direction = if backward { 0x10 } else { 0x00 };
    let mut out = Vec::new();
    for (step, c) in colors.iter().enumerate() {
        let leds: Vec<u8> = (0..MAX_LEDS).flat_map(|_| [c[1], c[0], c[2]]).collect();
        let mut first = vec![0x02, 0x4b, mode, direction, speed | (step as u8) << 5];
        first.extend_from_slice(&leds[..57]);
        let mut second = vec![0x03];
        second.extend_from_slice(&leds[57..]);
        out.extend([first, second]);
    }
    Ok(out)
}

fn duty_report(port: usize, duty: u8) -> Vec<u8> {
    vec![0x02, 0x4d, port as u8, 0x00, duty.min(100)]
}

pub struct SmartDevice {
    model: &'static Model,
    hid: HidDevice,
    firmware: Option<String>,
    leds: u16,
    ports: Vec<Option<(Port, Instant)>>,
    /// The duty camless last set per port; the device does not report it.
    duties: Vec<Option<u8>>,
}

fn write(dev: &HidDevice, payload: &[u8]) -> Result<()> {
    let mut buf = [0u8; WRITE_LEN];
    buf[..payload.len()].copy_from_slice(payload);
    dev.write(&buf)?;
    Ok(())
}

fn open(api: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    // The device sends nothing until it is started, so there is nothing to read yet.
    let hid = hid::open_unread(api, &found.path)?;
    // Detect the fans and the LED accessories, then start the status reports
    // (liquidctl's `initialize`, the Linux driver's `REQ_INIT_DETECT` and `_OPEN`).
    write(&hid, &[0x01, 0x5c]).context(hid::HELD)?;
    write(&hid, &[0x01, 0x5d])?;
    hid::drain(&hid);
    let msg = hid::read_prefix(&hid, &[STATUS_ID]).context("the device sends no status report")?;
    let (firmware, leds) = parse_info(&msg).context("a short status report")?;
    let n = fans(found.model);
    let mut d = SmartDevice {
        model: found.model,
        hid,
        firmware: Some(firmware),
        leds,
        ports: vec![None; n],
        duties: vec![None; n],
    };
    d.keep(&msg);
    Ok(Box::new(d))
}

impl SmartDevice {
    fn lighting(&self) -> bool {
        self.model.pid == SMART_DEVICE.pid
    }

    fn keep(&mut self, msg: &[u8]) {
        if let Some((i, p)) = parse_status(msg) {
            if let Some(slot) = self.ports.get_mut(i) {
                *slot = Some((p, Instant::now()));
            }
        }
    }

    fn fresh(&self) -> bool {
        self.ports.iter().all(|p| p.is_some_and(|(_, at)| at.elapsed() < STALE))
    }

    /// Every port's last report, reading more while one is missing or stale: the
    /// device sends one port per report, in turn.
    fn status(&mut self) -> Result<()> {
        for msg in hid::pending(&self.hid) {
            self.keep(&msg);
        }
        let end = Instant::now() + hid::REPLY_TIMEOUT;
        while !self.fresh() && Instant::now() < end {
            let msg = hid::read_prefix_within(&self.hid, &[STATUS_ID], end.saturating_duration_since(Instant::now()))?;
            self.keep(&msg);
        }
        Ok(())
    }

    fn port(&self, fan: &str) -> Option<usize> {
        fan.strip_prefix("fan").and_then(|n| n.parse::<usize>().ok()).filter(|n| (1..=self.ports.len()).contains(n)).map(|n| n - 1)
    }
}

impl Driver for SmartDevice {
    fn describe(&self) -> Device {
        let lights = if self.lighting() {
            let leds = if self.leds == 0 { MAX_LEDS } else { self.leds.min(MAX_LEDS) };
            vec![LightChannel { id: "led".into(), name: "LED".into(), leds, modes: modes(), ..Default::default() }]
        } else {
            Vec::new()
        };
        Device {
            name: self.model.name.into(),
            firmware: self.firmware.clone(),
            verified: self.model.verified(),
            experimental: self.model.is_experimental(),
            fans: (1..=self.ports.len())
                .map(|n| FanChannel { id: format!("fan{n}"), name: format!("Fan {n}"), ..Default::default() })
                .collect(),
            lights,
            ..Default::default()
        }
    }

    fn sensors(&self) -> Vec<(&'static str, &'static str, Unit)> {
        Vec::new()
    }

    fn poll(&mut self) -> Result<Reading> {
        self.status()?;
        let fans = self
            .ports
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let (p, at) = (*p)?;
                (at.elapsed() < STALE).then(|| {
                    // The device measures a port it did not detect too, so a speed means
                    // a fan is there.
                    let connected = p.mode != Mode::Unknown || p.rpm > 0;
                    (FAN_IDS[i], FanReading { rpm: Some(p.rpm), duty: self.duties[i], connected: Some(connected) })
                })
            })
            .collect();
        Ok(Reading { sensors: Vec::new(), fans })
    }

    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
        for &(fan, duty) in duties {
            let i = self.port(fan).with_context(|| format!("no fan {fan:?}; ports are fan1 to fan{}", self.ports.len()))?;
            write(&self.hid, &duty_report(i, duty))?;
            self.duties[i] = Some(duty.min(100));
        }
        Ok(())
    }

    fn set_lighting(&mut self, light: &str, lighting: &Lighting) -> Result<()> {
        if !self.lighting() || light != "led" {
            bail!("no light {light:?} on the {}", self.model.name);
        }
        lighting_reports(lighting)?.iter().try_for_each(|r| write(&self.hid, r))
    }
}

const FAN_IDS: [&str; 6] = ["fan1", "fan2", "fan3", "fan4", "fan5", "fan6"];

#[cfg(test)]
mod tests {
    use super::*;
    use camless_core::lighting::Speed;

    /// liquidctl's `SAMPLE_RESPONSES` in `tests/test_smart_device.py`.
    const SAMPLES: [&str; 3] = [
        "043e00056e00000b5b000301000007200002001e00",
        "04400005b500000b5b000201000007020002001e00",
        "044000053800000b5b000201000007120102001e00",
    ];

    fn sample(i: usize) -> Vec<u8> {
        (0..SAMPLES[i].len()).step_by(2).map(|j| u8::from_str_radix(&SAMPLES[i][j..j + 2], 16).unwrap()).collect()
    }

    #[test]
    fn status_reads_like_liquidctl() {
        let got: Vec<_> = (0..3).map(|i| parse_status(&sample(i)).unwrap()).collect();
        // liquidctl: fan 1 1461 rpm PWM, fan 2 1336 rpm PWM, fan 3 1390 rpm, no mode.
        assert_eq!(got[0].0, 2);
        assert_eq!((got[0].1.rpm, got[0].1.mode), (1390, Mode::Unknown));
        assert!((got[0].1.volts - 11.91).abs() < 1e-4 && (got[0].1.amps - 0.03).abs() < 1e-4);
        assert_eq!((got[1].0, got[1].1.rpm, got[1].1.mode), (0, 1461, Mode::Pwm));
        assert_eq!((got[2].0, got[2].1.rpm, got[2].1.mode), (1, 1336, Mode::Pwm));
        assert!((got[2].1.amps - 0.02).abs() < 1e-4);
        assert_eq!(parse_status(&[0x05; 21]), None);
        assert_eq!(parse_status(&sample(0)[..15]), None);
    }

    #[test]
    fn firmware_and_leds_read_like_liquidctl() {
        // liquidctl: firmware 1.7, two HUE+ strips, 20 LEDs.
        assert_eq!(parse_info(&sample(0)), Some(("1.7".to_string(), 20)));
        let mut aer = sample(0);
        aer[0x10] = 1 << 3;
        aer[0x11] = 5;
        assert_eq!(parse_info(&aer), Some(("1.7".to_string(), 40)));
    }

    #[test]
    fn duties_are_liquidctls() {
        // `set fan3 speed 50`.
        assert_eq!(duty_report(2, 50), [0x02, 0x4d, 0x02, 0x00, 50]);
        assert_eq!(duty_report(5, 150), [0x02, 0x4d, 0x05, 0x00, 100]);
    }

    #[test]
    fn lighting_follows_liquidctl() {
        // `set led color fixed 102030`: one step, 40 LEDs as G, R, B over two reports.
        let r = lighting_reports(&Lighting::Fixed([0x10, 0x20, 0x30])).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(&r[0][..8], &[0x02, 0x4b, 0x00, 0x00, 0x02, 0x20, 0x10, 0x30]);
        assert_eq!((r[0].len(), r[1].len()), (5 + 57, 1 + 63));
        assert_eq!(r[1][0], 0x03);
        assert_eq!(&r[1][61..], &[0x20, 0x10, 0x30]);
        // Breathing, two steps, fastest: the step in bits 5..7 of byte 4.
        let breathe = Lighting::Effect { effect: Effect::Breathing, colors: vec![[1, 2, 3], [4, 5, 6]], speed: Speed::Fastest, backward: false };
        let r = lighting_reports(&breathe).unwrap();
        assert_eq!(r.iter().step_by(2).map(|m| (m[2], m[3], m[4])).collect::<Vec<_>>(), [(0x07, 0x00, 0x04), (0x07, 0x00, 0x24)]);
        // Backward spectrum wave: 0x10 in byte 3, one step of black.
        let wave = Lighting::Effect { effect: Effect::SpectrumWave, colors: vec![[9, 9, 9]], speed: Speed::Slowest, backward: true };
        let r = lighting_reports(&wave).unwrap();
        assert_eq!(&r[0][..8], &[0x02, 0x4b, 0x02, 0x10, 0x00, 0, 0, 0]);
        // Pulse has no direction.
        let pulse = Lighting::Effect { effect: Effect::Pulse, colors: vec![[9, 9, 9]], speed: Speed::Normal, backward: true };
        assert_eq!(lighting_reports(&pulse).unwrap()[0][3], 0x00);
    }

    #[test]
    fn missing_effects_are_refused() {
        let tai = Lighting::Effect { effect: Effect::TaiChi, colors: vec![[1, 1, 1], [2, 2, 2]], speed: Speed::Normal, backward: false };
        assert!(lighting_reports(&tai).is_err());
        assert_eq!(modes(), ["off", "fixed", "fading", "spectrum-wave", "covering-marquee", "pulse", "breathing", "candle"]);
    }
}
