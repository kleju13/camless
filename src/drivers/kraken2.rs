//! The Kraken X2 (X42, X52, X62, X72; `1e71:170e`) and the Kraken M22 (`1e71:1715`),
//! the Krakens before the X3. They predate the command set of every later Kraken: the
//! cooler sends a status report by itself, a curve is 36 `02 4d` points and lighting
//! is `02 4c`. The M22 has the lighting only.
//!
//! Settings stay while the cooler has power (liquidctl's guide), so nothing here
//! writes flash. Stored curves have the same safety floor as the later Krakens.
//!
//! Sources: liquidctl `kraken2.py` and its guide, and the Linux `nzxt-kraken2` driver
//! for the status report. Nexus leaves these coolers out, so the driver is
//! experimental. Not verified on hardware.

use super::kraken_elite_v2::safety_floor;
use super::{hid, Driver, FanReading, Found, Model, Reading, NZXT_VID};
use anyhow::{bail, Context, Result};
use camless_core::lighting::{Effect, Lighting, Rgb};
use camless_core::status::{Device, FanChannel, LightChannel, Unit};
use hidapi::{HidApi, HidDevice};
use std::time::Duration;

pub const X2: Model = Model { experimental: true, ..Model::hid(NZXT_VID, 0x170e, "kraken-x2", "Kraken X2", open) };
pub const M22: Model = Model { experimental: true, ..Model::hid(NZXT_VID, 0x1715, "kraken-m22", "Kraken M22", open) };

/// liquidctl pads every write to the report id and 64 bytes.
const WRITE_LEN: usize = 65;
/// The cooler sends it twice a second (the Linux driver).
const STATUS_ID: u8 = 0x04;
const FAN_BASE: u8 = 0x80;
const PUMP_BASE: u8 = 0xc0;
/// The lowest duties in liquidctl's guide.
const FAN_MIN_DUTY: u8 = 25;
const PUMP_MIN_DUTY: u8 = 50;
/// liquidctl's profiles start below firmware 4, which its guide names.
const PROFILES_FROM_FIRMWARE: u8 = 3;
/// liquidctl sets both channels on initialization, so they share one set of
/// temperatures, which the firmware needs.
const RESET_FAN: &[(u8, u8)] = &[(20, 25), (30, 50), (50, 90), (60, 100)];
const RESET_PUMP: &[(u8, u8)] = &[(20, 50), (30, 60), (40, 90), (50, 100)];

/// The liquid temperatures of a stored curve, liquidctl's: every degree to 49, every
/// second to 58, and 60. Both channels use these.
fn curve_temps() -> impl Iterator<Item = u8> {
    (20..50).chain((50..60).step_by(2)).chain([60])
}

/// The 36 points of a curve, with the safety floor, which reaches 100 % before 60 °C.
fn curve_reports(base: u8, min: u8, duty_at: &dyn Fn(f32) -> u8) -> Vec<Vec<u8>> {
    curve_temps()
        .enumerate()
        .map(|(i, t)| vec![0x02, 0x4d, base + i as u8, t, duty_at(t as f32).max(safety_floor(t)).clamp(min, 100)])
        .collect()
}

/// liquidctl's `interpolate_profile` on a sorted profile.
fn interpolate(points: &[(u8, u8)], t: u8) -> u8 {
    let (mut lo, mut hi) = (points[0], points[points.len() - 1]);
    for &p in points {
        if p.0 <= t {
            lo = p;
        }
        if p.0 >= t {
            hi = p;
            break;
        }
    }
    if lo.0 == hi.0 {
        return lo.1;
    }
    let f = f32::from(t - lo.0) / f32::from(hi.0 - lo.0);
    (f32::from(lo.1) + f * (f32::from(hi.1) - f32::from(lo.1))).round() as u8
}

#[derive(Clone, Debug, PartialEq)]
struct Status {
    liquid_c: f32,
    fan_rpm: u16,
    pump_rpm: u16,
    firmware: [u8; 4],
}

fn parse_status(msg: &[u8]) -> Option<Status> {
    if msg.len() < 0x0f || msg[0] != STATUS_ID {
        return None;
    }
    Some(Status {
        liquid_c: f32::from(msg[1]) + f32::from(msg[2]) / 10.0,
        fan_rpm: u16::from_be_bytes([msg[3], msg[4]]),
        pump_rpm: u16::from_be_bytes([msg[5], msg[6]]),
        firmware: [msg[0xb], msg[0xc], msg[0xd], msg[0xe]],
    })
}

/// CAM's short form from firmware 5 on, as liquidctl prints it.
fn firmware_text(fw: [u8; 4]) -> String {
    if fw[0] >= 5 {
        format!("{}.{}", fw[0], fw[3])
    } else {
        format!("{}.{}.{}", fw[0], fw[2], fw[3])
    }
}

struct Light {
    id: &'static str,
    name: &'static str,
    channel: u8,
    leds: u16,
    ring: bool,
}

/// One LED behind the logo and eight in the ring, liquidctl's `logo` and `ring`.
const LIGHTS: [Light; 2] = [
    Light { id: "logo", name: "Pump logo", channel: 0x01, leds: 1, ring: false },
    Light { id: "ring", name: "Pump ring", channel: 0x02, leds: 8, ring: true },
];

/// liquidctl's mode byte and color counts for each camless effect the cooler has.
/// (mode, fewest colors, most colors, ring only, has a direction)
fn mode(effect: Effect) -> Option<(u8, usize, usize, bool, bool)> {
    Some(match effect {
        Effect::Fading => (0x01, 2, 8, false, false),
        Effect::SpectrumWave => (0x02, 0, 0, false, true),
        Effect::CoveringMarquee => (0x04, 1, 8, true, true),
        Effect::Breathing => (0x06, 1, 8, false, false),
        Effect::Pulse => (0x07, 1, 8, false, false),
        Effect::TaiChi => (0x08, 2, 2, true, false),
        Effect::WaterCooler => (0x09, 0, 0, true, false),
        _ => return None,
    })
}

fn modes(ring: bool) -> Vec<String> {
    let effects = Effect::ALL.into_iter().filter(|&e| mode(e).is_some_and(|m| ring || !m.3)).map(|e| e.name().to_string());
    ["off", "fixed"].into_iter().map(String::from).chain(effects).collect()
}

/// One report per color step. Every step sets all nine LEDs to its color; the logo
/// takes green, red, blue (liquidctl).
fn lighting_reports(light: &Light, lighting: &Lighting) -> Result<Vec<Vec<u8>>> {
    let (mode, mut colors, speed, backward) = match lighting {
        Lighting::Off => (0x00, vec![[0, 0, 0]], 2, false),
        Lighting::Fixed(c) => (0x00, vec![*c], 2, false),
        Lighting::Effect { effect, colors, speed, backward } => {
            let Some((mode, min, max, ring_only, direction)) = self::mode(*effect) else {
                bail!("the Kraken X2 has no {} effect", effect.label())
            };
            if ring_only && !light.ring {
                bail!("{} runs on the ring only", effect.label());
            }
            let mut colors: Vec<Rgb> = colors.iter().copied().take(max).collect();
            if max == 0 {
                colors = vec![[0, 0, 0]];
            }
            // camless starts these from one color; the cooler takes the one it has twice.
            while colors.len() < min {
                colors.push(colors.last().copied().unwrap_or_default());
            }
            (mode, colors, speed.index() as u8, *backward && direction)
        }
    };
    colors.truncate(8);
    let byte2 = (if backward { 0x10 } else { 0 }) | light.channel;
    Ok(colors
        .iter()
        .enumerate()
        .map(|(step, c)| {
            let mut r = vec![0x02, 0x4c, byte2, mode, speed | (step as u8) << 5, c[1], c[0], c[2]];
            for _ in 0..8 {
                r.extend_from_slice(c);
            }
            r
        })
        .collect())
}

pub struct Kraken2 {
    model: &'static Model,
    hid: HidDevice,
    firmware: Option<[u8; 4]>,
    /// The duty `set_duties` last stored per channel (fan, pump): on firmware with
    /// profiles a duty is 36 writes, so one that did not change is not sent again.
    sent: [Option<u8>; 2],
}

fn write(dev: &HidDevice, payload: &[u8]) -> Result<()> {
    let mut buf = [0u8; WRITE_LEN];
    buf[..payload.len()].copy_from_slice(payload);
    dev.write(&buf)?;
    Ok(())
}

fn open(api: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    let hid = hid::open(api, &found.path)?;
    let cooling = found.model.pid == X2.pid;
    // liquidctl reads the firmware from a status report on the M22 too.
    let status = hid::read_prefix_within(&hid, &[STATUS_ID], Duration::from_secs(2)).map(|m| parse_status(&m));
    let firmware = match status {
        Ok(Some(s)) => Some(s.firmware),
        _ if cooling => bail!("the cooler sends no status report"),
        _ => None,
    };
    let d = Kraken2 { model: found.model, hid, firmware, sent: [None; 2] };
    if d.profiles() {
        for (base, min, reset) in [(FAN_BASE, FAN_MIN_DUTY, RESET_FAN), (PUMP_BASE, PUMP_MIN_DUTY, RESET_PUMP)] {
            d.send(&curve_reports(base, min, &|t| interpolate(reset, t as u8)))?;
        }
    }
    Ok(Box::new(d))
}

impl Kraken2 {
    fn cooling(&self) -> bool {
        self.model.pid == X2.pid
    }

    /// Curves the cooler runs on its liquid temperature.
    fn profiles(&self) -> bool {
        self.cooling() && self.firmware.is_some_and(|f| f[0] >= PROFILES_FROM_FIRMWARE)
    }

    fn send(&self, reports: &[Vec<u8>]) -> Result<()> {
        reports.iter().try_for_each(|r| write(&self.hid, r))
    }

    /// (index in `sent`, base, minimum duty)
    fn channel(&self, fan: &str) -> Option<(usize, u8, u8)> {
        match fan {
            "fan" if self.cooling() => Some((0, FAN_BASE, FAN_MIN_DUTY)),
            "pump" if self.cooling() => Some((1, PUMP_BASE, PUMP_MIN_DUTY)),
            _ => None,
        }
    }
}

impl Driver for Kraken2 {
    fn describe(&self) -> Device {
        let fan = |id: &str, name: &str, pump: bool| FanChannel {
            id: id.into(),
            name: name.into(),
            pump,
            min_duty: if pump { PUMP_MIN_DUTY } else { FAN_MIN_DUTY },
            device_curves: self.profiles(),
            ..Default::default()
        };
        Device {
            name: self.model.name.into(),
            firmware: self.firmware.map(firmware_text),
            verified: self.model.verified(),
            experimental: self.model.is_experimental(),
            fans: if self.cooling() { vec![fan("pump", "Pump", true), fan("fan", "Radiator fans", false)] } else { Vec::new() },
            lights: LIGHTS
                .iter()
                .map(|l| LightChannel { id: l.id.into(), name: l.name.into(), leds: l.leds, modes: modes(l.ring), ..Default::default() })
                .collect(),
            ..Default::default()
        }
    }

    fn sensors(&self) -> Vec<(&'static str, &'static str, Unit)> {
        if self.cooling() {
            vec![("liquid", "Liquid", Unit::Celsius)]
        } else {
            Vec::new()
        }
    }

    fn poll(&mut self) -> Result<Reading> {
        if !self.cooling() {
            return Ok(Reading::default());
        }
        // The newest report waiting, or the next one.
        let msg = match hid::pending(&self.hid).into_iter().rev().find(|m| m.first() == Some(&STATUS_ID)) {
            Some(m) => m,
            None => hid::read_prefix(&self.hid, &[STATUS_ID])?,
        };
        let s = parse_status(&msg).context("a short status report")?;
        let fan = |rpm| FanReading { rpm: Some(rpm), duty: None, connected: None };
        Ok(Reading { sensors: vec![("liquid", s.liquid_c)], fans: vec![("pump", fan(s.pump_rpm)), ("fan", fan(s.fan_rpm))] })
    }

    /// With profiles, a flat curve, as liquidctl sets a fixed duty: the floor still
    /// speeds the cooler up if camless stops. Without, liquidctl's instant duty.
    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
        for &(fan, duty) in duties {
            let (i, base, min) = self.channel(fan).with_context(|| format!("no fan {fan:?} on this cooler"))?;
            if self.sent[i] == Some(duty) {
                continue;
            }
            if self.profiles() {
                self.send(&curve_reports(base, min, &|_| duty))?;
            } else {
                write(&self.hid, &[0x02, 0x4d, base & 0x70, 0x00, duty.clamp(min, 100)])?;
            }
            self.sent[i] = Some(duty);
        }
        Ok(())
    }

    fn store_curve(&mut self, fan: &str, sensor: Option<&str>, duty_at: &dyn Fn(f32) -> u8) -> Result<bool> {
        let Some((i, base, min)) = self.channel(fan) else { return Ok(false) };
        if !self.profiles() || !matches!(sensor, None | Some("liquid")) {
            return Ok(false);
        }
        self.send(&curve_reports(base, min, duty_at))?;
        self.sent[i] = None;
        Ok(true)
    }

    fn set_lighting(&mut self, light: &str, lighting: &Lighting) -> Result<()> {
        let l = LIGHTS.iter().find(|l| l.id == light).with_context(|| format!("no light {light:?} on this cooler"))?;
        self.send(&lighting_reports(l, lighting)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camless_core::lighting::Speed;

    #[test]
    fn curves_are_liquidctls_36_points_under_the_floor() {
        let temps: Vec<u8> = curve_temps().collect();
        assert_eq!(temps.len(), 36);
        assert_eq!((temps[0], temps[29], temps[30], temps[34], temps[35]), (20, 49, 50, 58, 60));
        let r = curve_reports(FAN_BASE, FAN_MIN_DUTY, &|_| 0);
        assert_eq!(r[0], [0x02, 0x4d, 0x80, 20, 25]);
        assert_eq!(r[26], [0x02, 0x4d, 0x80 + 26, 46, 25]);
        assert_eq!(r[31], [0x02, 0x4d, 0x80 + 31, 52, 70]);
        assert_eq!(r[35], [0x02, 0x4d, 0x80 + 35, 60, 100]);
        let p = curve_reports(PUMP_BASE, PUMP_MIN_DUTY, &|_| 80);
        assert_eq!(p[0], [0x02, 0x4d, 0xc0, 20, 80]);
        assert_eq!(p[35], [0x02, 0x4d, 0xc0 + 35, 60, 100]);
    }

    #[test]
    fn reset_profiles_interpolate_like_liquidctl() {
        assert_eq!(interpolate(RESET_FAN, 20), 25);
        assert_eq!(interpolate(RESET_FAN, 25), 38);
        assert_eq!(interpolate(RESET_FAN, 40), 70);
        assert_eq!(interpolate(RESET_PUMP, 35), 75);
        assert_eq!(interpolate(RESET_PUMP, 58), 100);
        assert_eq!(interpolate(RESET_PUMP, 10), 50);
    }

    #[test]
    fn status_reads_like_liquidctl_and_linux() {
        let mut msg = [0u8; 64];
        msg[..7].copy_from_slice(&[0x04, 29, 9, 0x03, 0x55, 0x07, 0x9c]);
        msg[0xb..0xf].copy_from_slice(&[6, 0, 0, 2]);
        let s = parse_status(&msg).unwrap();
        assert_eq!((s.liquid_c, s.fan_rpm, s.pump_rpm), (29.9, 853, 1948));
        assert_eq!(firmware_text(s.firmware), "6.2");
        assert_eq!(firmware_text([4, 0, 0, 3]), "4.0.3");
        msg[0] = 0x05;
        assert_eq!(parse_status(&msg), None);
        assert_eq!(parse_status(&[0x04, 1, 2]), None);
    }

    #[test]
    fn lighting_follows_liquidctl() {
        let (logo, ring) = (&LIGHTS[0], &LIGHTS[1]);
        // `set logo color fixed 102030`: one step, the logo as G, R, B.
        let r = lighting_reports(logo, &Lighting::Fixed([0x10, 0x20, 0x30])).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(&r[0][..8], &[0x02, 0x4c, 0x01, 0x00, 0x02, 0x20, 0x10, 0x30]);
        assert_eq!(&r[0][8..11], &[0x10, 0x20, 0x30]);
        assert_eq!(r[0].len(), 32);
        // Fading on the ring, two steps, fastest: step in bits 5..7.
        let fade = Lighting::Effect { effect: Effect::Fading, colors: vec![[1, 2, 3], [4, 5, 6]], speed: Speed::Fastest, backward: false };
        let r = lighting_reports(ring, &fade).unwrap();
        assert_eq!(r.iter().map(|m| (m[2], m[3], m[4])).collect::<Vec<_>>(), [(0x02, 0x01, 0x04), (0x02, 0x01, 0x24)]);
        // Backward spectrum wave: 0x10 in the channel byte, one step of black.
        let wave = Lighting::Effect { effect: Effect::SpectrumWave, colors: vec![], speed: Speed::Slowest, backward: true };
        let r = lighting_reports(ring, &wave).unwrap();
        assert_eq!(&r[0][..8], &[0x02, 0x4c, 0x12, 0x02, 0x00, 0, 0, 0]);
        // Fading needs two colors; one is taken twice.
        let one = Lighting::Effect { effect: Effect::Fading, colors: vec![[9, 9, 9]], speed: Speed::Normal, backward: false };
        assert_eq!(lighting_reports(logo, &one).unwrap().len(), 2);
    }

    #[test]
    fn ring_only_and_missing_effects_are_refused() {
        let tai = Lighting::Effect { effect: Effect::TaiChi, colors: vec![[1, 1, 1], [2, 2, 2]], speed: Speed::Normal, backward: false };
        assert!(lighting_reports(&LIGHTS[0], &tai).is_err());
        assert_eq!(lighting_reports(&LIGHTS[1], &tai).unwrap()[0][3], 0x08);
        let candle = Lighting::Effect { effect: Effect::Candle, colors: vec![[1, 1, 1]], speed: Speed::Normal, backward: false };
        assert!(lighting_reports(&LIGHTS[1], &candle).is_err());
        assert_eq!(modes(false), ["off", "fixed", "fading", "spectrum-wave", "pulse", "breathing"]);
        assert!(modes(true).contains(&"tai-chi".to_string()) && !modes(true).contains(&"candle".to_string()));
    }
}
