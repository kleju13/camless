//! Nollie's ARGB controllers: 1 to 32 channels of addressable LEDs, lighting only.
//! Another brand's device: camless opens it only with `[devices] other_brands` on.
//!
//! The controllers answer nothing and do not know their LED counts, so camless sends
//! each channel the most LEDs it takes, in one colour or black, and the firmware keeps
//! showing the last colours it got. camless sends the channels the config sets, and
//! leaves the others alone.
//!
//! Two transports. The 16- and 32-channel controllers take a 1025-byte report per
//! channel, `00 <channel> <flag> <count BE> G R B…`, shown on receipt. Wire channels
//! 15 and 31 carry a flag that delimits an update: they go in every frame, with no
//! LEDs when the config does not set them, and the controller gets 8 ms after them. The others take
//! 65-byte reports of 21 LEDs, numbered from the channel times a stride, and a latch
//! `00 FF` after them. Channel numbers on the wire follow a map per controller.
//!
//! The composite OS2.1 controllers have their LEDs on interface 0 or 2, usage page
//! `FF00` usage `01`. For the others no source names the collection: camless opens a
//! vendor-defined one with output reports of the controller's length. One without a
//! vendor-defined collection is not opened.
//!
//! camless never sends the standalone settings (`80`, `FE 02`), the release (`FF`,
//! `FE 01`) or the LED-count handshake (`FE 03`): the controllers may keep them.
//!
//! Sources: Nexus (`src/Peripherals/Nollie`, `src/Lighting/NollieLightingFrameWriter.cs`),
//! ported from OpenRGB's `NollieController`. Nexus has not run a wide-transport
//! controller. Not verified on hardware.

use super::{hid, Driver, Found, Model, Reading, Usage, Vendor};
use anyhow::{bail, Context, Result};
use camless_core::lighting::{Lighting, Rgb};
use camless_core::status::{Device, LightChannel, Unit};
use hidapi::{HidApi, HidDevice};
use std::time::Duration;

/// Nexus knows no app of Nollie's to leave the controllers to.
pub const NOLLIE: Vendor = Vendor { name: "Nollie", app: "Nollie's app", processes: &[] };

const WIDE_LEN: usize = 1025;
const CHUNK_LEN: usize = 65;
const LEDS_PER_CHUNK: usize = 21;
/// The channels that carry a flag on the wide transport, and the controller's time
/// after one.
const FLAG_CHANNELS: [usize; 2] = [15, 31];
const FLAG_SETTLE: Duration = Duration::from_millis(8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Transport {
    Wide,
    Chunked,
}

const IDENTITY: [u8; 32] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
];
const MAP32: [u8; 32] = [
    5, 4, 3, 2, 1, 0, 15, 14, 26, 27, 28, 29, 30, 31, 8, 9, 19, 18, 17, 16, 7, 6, 25, 24, 23, 22, 21, 20, 13, 12, 11, 10,
];
const MAP16_LEGACY: [u8; 32] = [
    19, 18, 17, 16, 24, 25, 26, 27, 20, 21, 22, 23, 31, 30, 29, 28, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
];
const MAP16_OS2: [u8; 32] = [
    3, 2, 1, 0, 8, 9, 10, 11, 4, 5, 6, 7, 15, 14, 13, 12, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
];

/// One controller, from Nexus's `NollieProtocol.Devices`.
struct Spec {
    vid: u16,
    pid: u16,
    channels: usize,
    max_leds: usize,
    map: &'static [u8; 32],
    transport: Transport,
    stride: usize,
}

const fn spec(
    vid: u16,
    pid: u16,
    channels: usize,
    max_leds: usize,
    map: &'static [u8; 32],
    transport: Transport,
    stride: usize,
) -> Spec {
    Spec { vid, pid, channels, max_leds, map, transport, stride }
}

const HIGH: u16 = 0x3061;
const LEGACY: u16 = 0x16d2;
const OS2: u16 = 0x16d5;

const SPECS: [Spec; 16] = [
    spec(HIGH, 0x4714, 32, 256, &MAP32, Transport::Wide, 25),
    spec(HIGH, 0x4716, 16, 256, &MAP16_LEGACY, Transport::Wide, 25),
    spec(LEGACY, 0x1f01, 8, 126, &IDENTITY, Transport::Chunked, 6),
    spec(LEGACY, 0x1f11, 1, 630, &IDENTITY, Transport::Chunked, 30),
    spec(LEGACY, 0x1616, 1, 42, &IDENTITY, Transport::Chunked, 2),
    spec(LEGACY, 0x1617, 8, 525, &IDENTITY, Transport::Chunked, 25),
    spec(LEGACY, 0x1618, 8, 525, &IDENTITY, Transport::Chunked, 25),
    spec(OS2, 0x4714, 32, 256, &MAP32, Transport::Wide, 25),
    spec(OS2, 0x4716, 16, 256, &MAP16_OS2, Transport::Wide, 25),
    spec(OS2, 0x1f01, 8, 126, &IDENTITY, Transport::Chunked, 6),
    spec(OS2, 0x1f11, 1, 630, &IDENTITY, Transport::Chunked, 30),
    spec(OS2, 0x2a32, 32, 256, &MAP32, Transport::Wide, 25),
    spec(OS2, 0x2a16, 16, 256, &MAP16_OS2, Transport::Wide, 25),
    spec(OS2, 0x2a08, 8, 126, &IDENTITY, Transport::Chunked, 6),
    spec(OS2, 0x2c08, 8, 126, &IDENTITY, Transport::Chunked, 6),
    spec(OS2, 0x2a01, 1, 630, &IDENTITY, Transport::Chunked, 30),
];

fn spec_of(vid: u16, pid: u16) -> Option<&'static Spec> {
    SPECS.iter().find(|s| (s.vid, s.pid) == (vid, pid))
}

const fn wide(vid: u16, pid: u16, slug: &'static str, name: &'static str) -> Model {
    Model::vendor_hid(vid, pid, slug, name, &NOLLIE, Usage::VendorOutput(WIDE_LEN), open)
}

const fn chunked(vid: u16, pid: u16, slug: &'static str, name: &'static str) -> Model {
    Model::vendor_hid(vid, pid, slug, name, &NOLLIE, Usage::VendorOutput(CHUNK_LEN), open)
}

/// An OS2.1 controller: its LEDs on one interface of several.
const fn composite(pid: u16, interface: i32, slug: &'static str, name: &'static str) -> Model {
    Model {
        interface: Some(interface),
        ..Model::vendor_hid(OS2, pid, slug, name, &NOLLIE, Usage::Exact(0xff00, 0x0001), open)
    }
}

pub const N32: Model = wide(HIGH, 0x4714, "nollie-32ch", "Nollie 32CH");
pub const N16: Model = wide(HIGH, 0x4716, "nollie-16ch", "Nollie 16CH");
pub const N8: Model = chunked(LEGACY, 0x1f01, "nollie-8ch", "Nollie 8CH");
pub const N1: Model = chunked(LEGACY, 0x1f11, "nollie-1ch", "Nollie 1CH");
pub const N28_12: Model = chunked(LEGACY, 0x1616, "nollie-28-12", "Nollie 28 12");
pub const N28_L1: Model = chunked(LEGACY, 0x1617, "nollie-28-l1", "Nollie 28 L1");
pub const N28_L2: Model = chunked(LEGACY, 0x1618, "nollie-28-l2", "Nollie 28 L2");
pub const N32_OS2: Model = wide(OS2, 0x4714, "nollie-32ch-os2", "Nollie 32CH OS2");
pub const N16_OS2: Model = wide(OS2, 0x4716, "nollie-16ch-os2", "Nollie 16CH OS2");
pub const N8_OS2: Model = chunked(OS2, 0x1f01, "nollie-8ch-os2", "Nollie 8CH OS2");
pub const N1_OS2: Model = chunked(OS2, 0x1f11, "nollie-1ch-os2", "Nollie 1CH OS2");
pub const N32_OS2_1: Model = composite(0x2a32, 0, "nollie-32ch-os2-1", "Nollie 32CH OS2.1");
pub const N16_OS2_1: Model = composite(0x2a16, 0, "nollie-16ch-os2-1", "Nollie 16CH OS2.1");
pub const N8_OS2_1: Model = composite(0x2a08, 2, "nollie-8ch-os2-1", "Nollie 8CH OS2.1");
pub const PRISM8: Model = composite(0x2c08, 2, "nollie-prism8", "Nollie Prism8");
pub const N1_OS2_1: Model = composite(0x2a01, 2, "nollie-1ch-os2-1", "Nollie 1CH OS2.1");

impl Spec {
    fn hardware(&self, channel: usize) -> usize {
        usize::from(self.map[channel])
    }

    /// GRB on the chunked controllers but the 28 series, which take RGB.
    fn grb(&self) -> bool {
        self.transport == Transport::Wide || matches!(self.pid, 0x1f01 | 0x1f11 | 0x2a08 | 0x2a01 | 0x2c08)
    }

    fn order(&self, c: Rgb) -> [u8; 3] {
        if self.grb() {
            [c[1], c[0], c[2]]
        } else {
            c
        }
    }

    /// `WriteWide`: a channel's LEDs, shown on receipt; none on a flag channel the
    /// config does not set.
    fn wide(&self, hw: usize, color: Option<Rgb>) -> Vec<u8> {
        let mut r = vec![0u8; WIDE_LEN];
        let count = if color.is_some() { self.max_leds } else { 0 };
        r[1] = hw as u8;
        r[2] = FLAG_CHANNELS.iter().position(|&f| f == hw).map_or(0, |i| i as u8 + 1);
        r[3..5].copy_from_slice(&(count as u16).to_be_bytes());
        for led in r[5..].chunks_exact_mut(3).take(count) {
            led.copy_from_slice(&self.order(color.unwrap_or_default()));
        }
        r
    }

    /// `WriteChunk`: 21 LEDs of a channel per report.
    fn chunks(&self, hw: usize, color: Rgb) -> Vec<[u8; CHUNK_LEN]> {
        let mut left = self.max_leds;
        let mut out = Vec::new();
        for chunk in 0.. {
            if left == 0 {
                break;
            }
            let n = left.min(LEDS_PER_CHUNK);
            let mut r = [0u8; CHUNK_LEN];
            r[1] = (chunk + hw * self.stride) as u8;
            for led in r[2..].chunks_exact_mut(3).take(n) {
                led.copy_from_slice(&self.order(color));
            }
            out.push(r);
            left -= n;
        }
        out
    }

    /// What to send for the colours the config sets, by channel: in hardware channel
    /// order on the wide transport, with its flag channels in every frame, empty when
    /// the config does not set them; in channel order with a latch on the chunked one.
    fn frame(&self, colors: &[Option<Rgb>]) -> Vec<Vec<u8>> {
        if colors.iter().all(Option::is_none) {
            return Vec::new();
        }
        match self.transport {
            Transport::Wide => {
                let mut sends: Vec<(usize, Option<Rgb>)> = (0..self.channels)
                    .map(|ch| (self.hardware(ch), colors[ch]))
                    .filter(|&(hw, c)| c.is_some() || FLAG_CHANNELS.contains(&hw))
                    .collect();
                sends.sort_by_key(|s| s.0);
                sends.into_iter().map(|(hw, c)| self.wide(hw, c)).collect()
            }
            Transport::Chunked => {
                let mut out: Vec<Vec<u8>> = (0..self.channels)
                    .filter_map(|ch| Some(self.chunks(self.hardware(ch), colors[ch]?)))
                    .flatten()
                    .map(|r| r.to_vec())
                    .collect();
                let mut latch = vec![0u8; CHUNK_LEN];
                latch[1] = 0xff;
                out.push(latch);
                out
            }
        }
    }
}

/// `ChannelName`: the 32-channel controllers name their last 16 by the Strimer
/// cables they take.
fn channel_name(channels: usize, ch: usize) -> String {
    match ch {
        _ if channels < 32 => format!("Channel {}", ch + 1),
        28.. => format!("Channel EXT {}", ch - 27),
        22.. => format!("Channel GPU {}", ch - 21),
        16.. => format!("Channel ATX {}", ch - 15),
        _ => format!("Channel {}", ch + 1),
    }
}

pub struct Nollie {
    model: &'static Model,
    spec: &'static Spec,
    dev: HidDevice,
    /// What the config sets each channel to: a colour, black for off.
    colors: Vec<Option<Rgb>>,
    /// Colours set since the last frame went out.
    dirty: bool,
}

fn open(api: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    let m = found.model;
    let spec = spec_of(m.vid, m.pid).context("no channel table for this controller")?;
    // The controllers send nothing, and may have no input reports to read.
    let dev = hid::open_unread(api, &found.path)?;
    Ok(Box::new(Nollie { model: m, spec, dev, colors: vec![None; spec.channels], dirty: false }))
}

impl Driver for Nollie {
    fn describe(&self) -> Device {
        Device {
            name: self.model.name.into(),
            verified: self.model.verified(),
            experimental: self.model.is_experimental(),
            lights: (0..self.spec.channels)
                .map(|ch| LightChannel {
                    id: format!("ch{}", ch + 1),
                    name: channel_name(self.spec.channels, ch),
                    leds: self.spec.max_leds as u16,
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

    /// The colours set since the last poll go out here, in one frame, so a config
    /// with every channel set sends each report once.
    fn poll(&mut self) -> Result<Reading> {
        if self.dirty {
            for report in self.spec.frame(&self.colors) {
                self.dev.write(&report).context("write to the controller failed")?;
                if self.spec.transport == Transport::Wide && FLAG_CHANNELS.contains(&usize::from(report[1])) {
                    std::thread::sleep(FLAG_SETTLE);
                }
            }
            self.dirty = false;
        }
        Ok(Reading::default())
    }

    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
        match duties.first() {
            Some((fan, _)) => bail!("no fan {fan:?}: this controller has lighting only"),
            None => Ok(()),
        }
    }

    fn set_lighting(&mut self, light: &str, lighting: &Lighting) -> Result<()> {
        let ch = light
            .strip_prefix("ch")
            .and_then(|n| n.parse::<usize>().ok())
            .filter(|n| (1..=self.spec.channels).contains(n))
            .with_context(|| format!("no light {light:?} on this controller"))?
            - 1;
        self.colors[ch] = Some(match lighting {
            Lighting::Off => [0, 0, 0],
            Lighting::Fixed(c) => *c,
            Lighting::Effect { .. } => bail!("this controller takes off or one colour"),
        });
        self.dirty = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_model_has_its_spec() {
        let nollie: Vec<_> = super::super::MODELS.iter().filter(|m| m.vendor.is_some_and(|v| v.name == "Nollie")).collect();
        assert_eq!(nollie.len(), SPECS.len());
        for m in nollie {
            let s = spec_of(m.vid, m.pid).unwrap();
            let len = if s.transport == Transport::Wide { WIDE_LEN } else { CHUNK_LEN };
            assert!(m.usage == Some(Usage::VendorOutput(len)) || m.interface.is_some());
            // Every channel maps to a different wire channel.
            let mut hw: Vec<_> = (0..s.channels).map(|ch| s.hardware(ch)).collect();
            hw.sort();
            hw.dedup();
            assert_eq!(hw.len(), s.channels);
            match s.transport {
                // A channel fits one report.
                Transport::Wide => assert!(5 + 3 * s.max_leds <= WIDE_LEN),
                // Chunk numbers stay within each channel's stride, below the latch.
                Transport::Chunked => {
                    assert!(s.max_leds.div_ceil(LEDS_PER_CHUNK) <= s.stride, "{:04x}", m.pid);
                    assert!(s.channels * s.stride <= 0xff);
                }
            }
        }
    }

    #[test]
    fn wide_frames_go_in_hardware_order_with_their_flags() {
        let s = spec_of(HIGH, 0x4714).unwrap();
        let mut colors = vec![None; 32];
        // Channel 1 is wire channel 5, channel 7 wire channel 15, a flag channel.
        colors[0] = Some([0x10, 0x20, 0x30]);
        colors[22] = Some([1, 2, 3]);
        let frame = s.frame(&colors);
        let wire: Vec<_> = frame.iter().map(|r| (r[1], r[2])).collect();
        // Wire 5, the group's flag 15 with no LEDs; channel 23 is wire 25, flag 31.
        assert_eq!(wire, [(5, 0), (15, 1), (25, 0), (31, 2)]);
        assert_eq!(&frame[0][..8], [0, 5, 0, 1, 0, 0x20, 0x10, 0x30]);
        assert_eq!(&frame[0][5 + 255 * 3..5 + 256 * 3], [0x20, 0x10, 0x30]);
        assert_eq!(frame[0].len(), 1025);
        assert_eq!(&frame[1][..5], [0, 15, 1, 0, 0]);
        assert!(frame[1][5..].iter().all(|&b| b == 0));
        // The flag channel set by the config goes once, with its LEDs.
        colors[6] = Some([9, 9, 9]);
        let wire: Vec<_> = s.frame(&colors).iter().map(|r| (r[1], r[3], r[4])).collect();
        assert_eq!(wire, [(5, 1, 0), (15, 1, 0), (25, 1, 0), (31, 0, 0)]);
        assert!(s.frame(&[None; 32]).is_empty());
        // The legacy 16-channel board has wire channels 16 to 31: only 31 flags.
        let s = spec_of(HIGH, 0x4716).unwrap();
        let mut colors = vec![None; 16];
        colors[0] = Some([1, 2, 3]);
        let wire: Vec<_> = s.frame(&colors).iter().map(|r| (r[1], r[2])).collect();
        assert_eq!(wire, [(19, 0), (31, 2)]);
    }

    #[test]
    fn chunked_frames_number_their_chunks_and_latch() {
        let s = spec_of(LEGACY, 0x1f01).unwrap();
        let mut colors = vec![None; 8];
        colors[2] = Some([0x10, 0x20, 0x30]);
        let frame = s.frame(&colors);
        // 126 LEDs in 6 chunks of 21 from 2 × 6, then the latch.
        assert_eq!(frame.iter().map(|r| r[1]).collect::<Vec<_>>(), [12, 13, 14, 15, 16, 17, 0xff]);
        assert_eq!(&frame[0][..5], [0, 12, 0x20, 0x10, 0x30]);
        assert_eq!(&frame[5][2 + 20 * 3..2 + 21 * 3], [0x20, 0x10, 0x30]);
        assert!(frame.iter().all(|r| r.len() == 65));
        // The 28 series takes RGB.
        let rgb = spec_of(LEGACY, 0x1617).unwrap();
        let mut colors = vec![None; 8];
        colors[0] = Some([0x10, 0x20, 0x30]);
        let frame = rgb.frame(&colors);
        assert_eq!(frame.len(), 25 + 1);
        assert_eq!(&frame[0][..5], [0, 0, 0x10, 0x20, 0x30]);
        // 525 = 24 × 21 + 21.
        assert_eq!(frame[24][1], 24);
    }

    #[test]
    fn channels_are_named_as_nexus_does() {
        assert_eq!(channel_name(32, 0), "Channel 1");
        assert_eq!(channel_name(32, 16), "Channel ATX 1");
        assert_eq!(channel_name(32, 22), "Channel GPU 1");
        assert_eq!(channel_name(32, 31), "Channel EXT 4");
        assert_eq!(channel_name(16, 15), "Channel 16");
    }
}
