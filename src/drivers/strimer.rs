//! Lian Li's Strimer Plus V2 cables on their L-Connect controller (`0cf2:a200`): the
//! 24-pin ATX cable in six zones of 20 LEDs, the 8-pin GPU cable in six zones of 27
//! (four on the dual 8-pin cable). Lighting only. Another brand's device: camless
//! opens it only with `[devices] other_brands` on, and leaves it to L-Connect while
//! that runs.
//!
//! Every report is 255 bytes with id `E0` on the interrupt pipe, on the vendor
//! collection (usage page `FF72`, usage `A1`). One colour goes out as each zone's
//! LEDs in R, B, G order and a commit of the direct mode; off is the controller's own
//! off mode. A latch over every zone makes them take.
//!
//! Sources: Nexus (`src/Peripherals/Strimer`, `src/Lighting/StrimerLightingFrameWriter.cs`),
//! from OpenRGB's `LianLiStrimerLConnectController`. Nexus calls it untested, pending
//! verification. Not verified on hardware.

use super::lianli::LIAN_LI;
use super::{hid, Driver, Found, Model, Reading, Usage};
use anyhow::{bail, Context, Result};
use camless_core::lighting::{Lighting, Rgb};
use camless_core::status::{Device, LightChannel, Unit};
use hidapi::{HidApi, HidDevice};

pub const MODEL: Model =
    Model::vendor_hid(0x0cf2, 0xa200, "lianli-strimer", "Lian Li Strimer Plus V2", &LIAN_LI, Usage::Exact(0xff72, 0xa1), open);

const REPORT_LEN: usize = 255;
const REPORT_ID: u8 = 0xe0;
const ATX_ZONES: usize = 6;
const ATX_LEDS: usize = 20;
/// The triple 8-pin cable, Nexus's default; the dual one has the first four.
const GPU_ZONES: usize = 6;
const GPU_LEDS: usize = 27;
const MODE_OFF: u8 = 0x00;
const MODE_DIRECT: u8 = 0x01;
/// The defaults Nexus commits: middle speed, forward, full brightness.
const SPEED: u8 = 0x00;
const DIRECTION: u8 = 0x01;
const BRIGHTNESS: u8 = 0x00;

/// A command in a whole report.
fn report(bytes: &[u8]) -> [u8; REPORT_LEN] {
    let mut r = [0u8; REPORT_LEN];
    r[..bytes.len()].copy_from_slice(bytes);
    r
}

/// `WriteColorData`: R, B, G per LED from byte 2.
fn colors(zone: usize, color: Rgb, count: usize) -> [u8; REPORT_LEN] {
    let mut r = report(&[REPORT_ID, 0x30 | zone as u8]);
    for led in r[2..].chunks_exact_mut(3).take(count) {
        led.copy_from_slice(&[color[0], color[2], color[1]]);
    }
    r
}

fn commit(zone: usize, mode: u8) -> [u8; REPORT_LEN] {
    report(&[REPORT_ID, 0x10 | zone as u8, mode, SPEED, DIRECTION, BRIGHTNESS, 0])
}

/// `BuildApplyLatch`: a big-endian mask of the ATX zones, then the GPU ones.
fn latch() -> [u8; REPORT_LEN] {
    let mask: u16 = (1 << ATX_ZONES) - 1 | ((1 << GPU_ZONES) - 1) << ATX_ZONES;
    report(&[REPORT_ID, 0x2c, (mask >> 8) as u8, mask as u8, 0, 0, 0, 0])
}

/// The zones and LEDs per zone of a cable.
fn cable(light: &str) -> Option<(std::ops::Range<usize>, usize)> {
    match light {
        "atx" => Some((0..ATX_ZONES, ATX_LEDS)),
        "gpu" => Some((ATX_ZONES..ATX_ZONES + GPU_ZONES, GPU_LEDS)),
        _ => None,
    }
}

pub struct Strimer {
    dev: HidDevice,
}

fn open(api: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    Ok(Box::new(Strimer { dev: hid::open(api, &found.path)? }))
}

impl Strimer {
    fn send(&self, r: &[u8; REPORT_LEN]) -> Result<()> {
        self.dev.write(r).context("write to the Strimer controller failed")?;
        Ok(())
    }
}

impl Driver for Strimer {
    fn describe(&self) -> Device {
        let light = |id: &str, name: &str, leds: usize| LightChannel {
            id: id.into(),
            name: name.into(),
            leds: leds as u16,
            modes: vec!["off".into(), "fixed".into()],
            ..Default::default()
        };
        Device {
            name: MODEL.name.into(),
            verified: MODEL.verified(),
            experimental: MODEL.is_experimental(),
            lights: vec![
                light("atx", "24-pin cable", ATX_ZONES * ATX_LEDS),
                light("gpu", "8-pin GPU cable", GPU_ZONES * GPU_LEDS),
            ],
            ..Default::default()
        }
    }

    fn sensors(&self) -> Vec<(&'static str, &'static str, Unit)> {
        Vec::new()
    }

    fn poll(&mut self) -> Result<Reading> {
        Ok(Reading::default())
    }

    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
        match duties.first() {
            Some((fan, _)) => bail!("no fan {fan:?}: the Strimer has lighting only"),
            None => Ok(()),
        }
    }

    fn set_lighting(&mut self, light: &str, lighting: &Lighting) -> Result<()> {
        let (zones, leds) = cable(light).with_context(|| format!("no light {light:?} on the Strimer"))?;
        for zone in zones {
            match lighting {
                Lighting::Off => self.send(&commit(zone, MODE_OFF))?,
                Lighting::Fixed(c) => {
                    self.send(&colors(zone, *c, leds))?;
                    self.send(&commit(zone, MODE_DIRECT))?;
                }
                Lighting::Effect { .. } => bail!("the Strimer takes off or one colour"),
            }
        }
        self.send(&latch())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_follow_nexus_builders() {
        let c = colors(7, [0x10, 0x20, 0x30], 27);
        assert_eq!(&c[..5], [0xe0, 0x37, 0x10, 0x30, 0x20]);
        assert_eq!(&c[2 + 26 * 3..2 + 27 * 3], [0x10, 0x30, 0x20]);
        assert!(c[2 + 27 * 3..].iter().all(|&b| b == 0));
        assert_eq!(&commit(3, MODE_DIRECT)[..8], [0xe0, 0x13, 0x01, 0x00, 0x01, 0x00, 0x00, 0]);
        assert_eq!(&commit(11, MODE_OFF)[..3], [0xe0, 0x1b, 0x00]);
        // The triple harness: every zone, 0x0FFF.
        assert_eq!(&latch()[..8], [0xe0, 0x2c, 0x0f, 0xff, 0, 0, 0, 0]);
        assert_eq!(latch().len(), 255);
    }

    #[test]
    fn cables_are_their_zones() {
        assert_eq!(cable("atx"), Some((0..6, 20)));
        assert_eq!(cable("gpu"), Some((6..12, 27)));
        assert_eq!(cable("fan1"), None);
        // Every zone fits one report.
        assert!(2 + 3 * GPU_LEDS <= REPORT_LEN);
    }
}
