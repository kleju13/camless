//! iBUYPOWER's MiniHub (`3402:0900`), made by HYTE: two PWM fan ports (the second
//! for a chain of up to three fans) and four ARGB ports, on a USB serial port.
//! Another brand's hub: camless opens it only with `[devices] other_brands` on, and
//! leaves it to HYTE Nexus while that runs.
//!
//! Every command starts `FF DD`. The hub passes the motherboard's fan PWM and ARGB
//! through until camless switches the fans (`FF DD 05 00`) or the LEDs
//! (`FF DD 03 00`) to software mode, which lasts until the hub loses power: camless
//! switches the fans only once a curve takes one of them, and the LEDs once the config
//! sets a light. Both fan ports take their duties in one command, from 10 to 100 %,
//! so the port the config does not set runs at 70 %. The hub keeps its duties and
//! colours with no watchdog, until it loses power. When camless lets go, or the
//! service stops, it hands both back to the motherboard; if camless crashes the hub
//! keeps the last duty.
//!
//! Each LED port takes one frame, `FF EE 03 <port> 01 68 00` and 50 LEDs in G, R, B
//! order, 100 on port 4. The tachs read right about half the time, and only while
//! frames stream: camless shows a speed only once 5 of the last 20 plausible readings
//! agree within 2 %, as Nexus does. Once the config sets a light, camless sends every
//! port's frame with every poll, the ports it does not set off: the LEDs' software
//! mode is the whole hub's.
//!
//! Sources: Nexus (`src/Peripherals/Hyte/MiniHub`, `src/Cooling/MiniHubCoolingProvider.cs`),
//! from HYTE's own service. Nexus sends 10 % for a fan port nothing set; camless
//! sends 70 %. Not verified on hardware.

use super::hyte::{self, HYTE};
use super::serial::{self, Serial};
use super::{Driver, FanReading, Found, Model, Reading, Vendor};
use anyhow::{bail, Context, Result};
use camless_core::lighting::{Lighting, Rgb};
use camless_core::status::{Device, FanChannel, LightChannel, Unit};
use hidapi::HidApi;
use std::time::Duration;

pub const IBUYPOWER: Vendor = Vendor { name: "iBUYPOWER", app: HYTE.app, processes: HYTE.processes };

pub const MINIHUB: Model =
    Model::vendor_serial(0x3402, 0x0900, "ibuypower-minihub", "iBUYPOWER MiniHub", &IBUYPOWER, open);

const FAN_PORTS: usize = 2;
const LED_PORTS: usize = 4;
const MIN_DUTY: u8 = 10;
const HOLD: u8 = 70;
const SOFTWARE: u8 = 0x00;
const MOTHERBOARD: u8 = 0x01;
const FIRMWARE_LEN: usize = 7;
const SPEEDS_LEN: usize = 9;
const REPLY_TIMEOUT: Duration = Duration::from_millis(300);

const GET_FIRMWARE: [u8; 3] = [0xff, 0xdd, 0x02];
const GET_SPEEDS: [u8; 3] = [0xff, 0xdd, 0x06];

fn rgb_mode(mode: u8) -> [u8; 4] {
    [0xff, 0xdd, 0x03, mode]
}

fn fan_mode(mode: u8) -> [u8; 4] {
    [0xff, 0xdd, 0x05, mode]
}

/// Both ports at once, from 10 %: the firmware runs a lower duty at 0.
fn set_fans(port1: u8, port2: u8) -> [u8; 7] {
    let d = |duty: u8| duty.clamp(MIN_DUTY, 100);
    [0xff, 0xdd, 0x04, 0x00, d(port1), 0x00, d(port2)]
}

fn max_leds(port: usize) -> usize {
    if port == 4 {
        100
    } else {
        50
    }
}

/// A port's frame, padded to its LEDs, every LED in one colour.
fn frame(port: usize, color: Rgb) -> Vec<u8> {
    let mut f = vec![0u8; 7 + max_leds(port) * 3];
    f[..7].copy_from_slice(&[0xff, 0xee, 0x03, port as u8, 0x01, 0x68, 0x00]);
    for led in f[7..].chunks_exact_mut(3) {
        led.copy_from_slice(&[color[1], color[0], color[2]]);
    }
    f
}

/// `FF DD 06 01 _ <period> 02 _ <period>`: 60000 / (period × 0.4) rpm, 0 for none.
fn parse_speeds(reply: &[u8]) -> Option<[u16; FAN_PORTS]> {
    if reply.len() < SPEEDS_LEN || reply[..3] != [0xff, 0xdd, 0x06] {
        return None;
    }
    let rpm = |period: u8| if period == 0 { 0 } else { (60_000.0 / (f64::from(period) * 0.4)) as u16 };
    Some([rpm(reply[5]), rpm(reply[8])])
}

/// `MiniHubTachConsensus`: a speed once enough recent readings agree, held while
/// one near it remains.
#[derive(Default)]
struct Consensus {
    window: Vec<u16>,
    next: usize,
    shown: Option<u16>,
}

impl Consensus {
    const WINDOW: usize = 20;
    const AGREEING: usize = 5;

    fn plausible(rpm: u16) -> bool {
        (200..=4000).contains(&rpm)
    }

    fn agrees(center: u16, sample: u16) -> bool {
        Self::plausible(sample) && f64::from(sample.abs_diff(center)) <= f64::from(center) * 0.02
    }

    fn add(&mut self, rpm: u16) -> Option<u16> {
        if self.window.len() < Self::WINDOW {
            self.window.push(rpm);
        } else {
            self.window[self.next] = rpm;
        }
        self.next = (self.next + 1) % Self::WINDOW;
        // The largest cluster around a plausible reading, its median.
        let best = self
            .window
            .iter()
            .filter(|&&c| Self::plausible(c))
            .map(|&c| (self.window.iter().filter(|&&s| Self::agrees(c, s)).count(), c))
            .fold((0, 0), |best, x| if x.0 > best.0 { x } else { best });
        if best.0 >= Self::AGREEING {
            let mut members: Vec<u16> = self.window.iter().copied().filter(|&s| Self::agrees(best.1, s)).collect();
            members.sort_unstable();
            self.shown = Some(members[members.len() / 2]);
        } else if self.shown.is_some_and(|held| !self.window.iter().any(|&s| Self::agrees(held, s))) {
            self.shown = None;
        }
        self.shown
    }
}

pub struct MiniHub {
    port: Serial,
    firmware: Option<String>,
    duties: [Option<u8>; FAN_PORTS],
    colors: [Option<Rgb>; LED_PORTS],
    tachs: [Consensus; FAN_PORTS],
    fans_taken: bool,
    lights_taken: bool,
}

fn open(_: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    let mut port = Serial::open(&found.path)?;
    let reply = serial::request(&mut port, &GET_FIRMWARE, FIRMWARE_LEN, REPLY_TIMEOUT)?;
    let firmware = hyte::parse_firmware(&reply).context("no firmware reply; not a MiniHub, or it is busy")?;
    Ok(Box::new(MiniHub {
        port,
        firmware: Some(firmware),
        duties: [None; FAN_PORTS],
        colors: [None; LED_PORTS],
        tachs: Default::default(),
        fans_taken: false,
        lights_taken: false,
    }))
}

impl MiniHub {
    /// Every port's frame once the config sets a light: software mode is the whole
    /// hub's, so the ports it does not set are off, not on the motherboard's ARGB.
    fn send_frames(&mut self) -> Result<()> {
        if self.colors.iter().all(Option::is_none) {
            return Ok(());
        }
        for p in 1..=LED_PORTS {
            self.port.write(&frame(p, self.colors[p - 1].unwrap_or([0, 0, 0])))?;
        }
        Ok(())
    }
}

impl Driver for MiniHub {
    fn describe(&self) -> Device {
        Device {
            name: MINIHUB.name.into(),
            firmware: self.firmware.clone(),
            verified: MINIHUB.verified(),
            experimental: MINIHUB.is_experimental(),
            fans: (1..=FAN_PORTS)
                .map(|p| FanChannel {
                    id: format!("fan{p}"),
                    name: format!("Fan port {p}"),
                    min_duty: MIN_DUTY,
                    ..Default::default()
                })
                .collect(),
            lights: (1..=LED_PORTS)
                .map(|p| LightChannel {
                    id: format!("argb{p}"),
                    name: format!("ARGB port {p}"),
                    leds: max_leds(p) as u16,
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
        let reply = serial::request(&mut self.port, &GET_SPEEDS, SPEEDS_LEN, REPLY_TIMEOUT)?;
        let rpm = parse_speeds(&reply).context("no fan speed reply")?;
        self.send_frames()?;
        const IDS: [&str; FAN_PORTS] = ["fan1", "fan2"];
        let fans = (0..FAN_PORTS)
            .map(|i| (IDS[i], FanReading { rpm: self.tachs[i].add(rpm[i]), duty: self.duties[i], connected: None }))
            .collect();
        Ok(Reading { sensors: Vec::new(), fans })
    }

    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
        for &(id, duty) in duties {
            let p = hyte::port_of(id, "fan", FAN_PORTS).with_context(|| format!("no fan {id:?} on the hub"))?;
            self.duties[p - 1] = Some(duty.clamp(MIN_DUTY, 100));
        }
        if !self.fans_taken {
            self.port.write(&fan_mode(SOFTWARE))?;
            self.fans_taken = true;
        }
        let [p1, p2] = self.duties.map(|d| d.unwrap_or(HOLD));
        self.port.write(&set_fans(p1, p2))
    }

    fn set_lighting(&mut self, light: &str, lighting: &Lighting) -> Result<()> {
        let p = hyte::port_of(light, "argb", LED_PORTS).with_context(|| format!("no light {light:?} on the hub"))?;
        let color = match lighting {
            Lighting::Off => [0, 0, 0],
            Lighting::Fixed(c) => *c,
            Lighting::Effect { .. } => bail!("the MiniHub takes off or one colour"),
        };
        if !self.lights_taken {
            self.port.write(&rgb_mode(SOFTWARE))?;
            self.lights_taken = true;
        }
        self.colors[p - 1] = Some(color);
        self.send_frames()
    }

    /// The fans and the LEDs back to the motherboard, the LEDs even if the fans fail.
    fn release(&mut self) -> Result<()> {
        self.duties = [None; FAN_PORTS];
        self.colors = [None; LED_PORTS];
        let mut result = Ok(());
        if std::mem::take(&mut self.fans_taken) {
            result = self.port.write(&fan_mode(MOTHERBOARD));
        }
        if std::mem::take(&mut self.lights_taken) {
            result = result.and(self.port.write(&rgb_mode(MOTHERBOARD)));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_follow_nexus_builders() {
        assert_eq!(fan_mode(SOFTWARE), [0xff, 0xdd, 0x05, 0x00]);
        assert_eq!(rgb_mode(MOTHERBOARD), [0xff, 0xdd, 0x03, 0x01]);
        assert_eq!(set_fans(0, 150), [0xff, 0xdd, 0x04, 0x00, 10, 0x00, 100]);
        let f = frame(4, [0x10, 0x20, 0x30]);
        assert_eq!(f.len(), 307);
        assert_eq!(&f[..10], [0xff, 0xee, 0x03, 0x04, 0x01, 0x68, 0x00, 0x20, 0x10, 0x30]);
        assert_eq!(frame(1, [0; 3]).len(), 157);
    }

    #[test]
    fn speeds_show_once_readings_agree() {
        // 60000 / (50 × 0.4) = 3000 rpm on port 1, none on port 2.
        let reply = [0xff, 0xdd, 0x06, 0x01, 0, 50, 0x02, 0, 0];
        assert_eq!(parse_speeds(&reply), Some([3000, 0]));
        assert_eq!(parse_speeds(&reply[..8]), None);

        let mut c = Consensus::default();
        // Junk and four agreeing readings: nothing yet.
        for rpm in [3000, 120, 1500, 3010, 0, 2995, 3005] {
            assert_eq!(c.add(rpm), None);
        }
        // The fifth agrees: the median of the five.
        assert_eq!(c.add(3001), Some(3001));
        // Junk holds it while readings near it remain in the window.
        for _ in 0..14 {
            assert_eq!(c.add(0), Some(3001));
        }
        // Twenty readings later none is near it.
        for _ in 0..5 {
            c.add(0);
        }
        assert_eq!(c.add(0), None);
    }
}
