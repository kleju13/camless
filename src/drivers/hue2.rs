//! Lighting in NZXT's Hue 2 family, which the Krakens since the X3 and the Control
//! Hub share: static colors (`26 04` + `26 06`), effects (`2A 04`) and the accessory
//! report (`20 03` → `21 03`). The Kraken X3 and Z3 take static colors as `2A 04`
//! too, see `packets_2a`.
//!
//! Sources: liquidctl (`kraken3.py`, `control_hub.py`, `util.py`) and captures of
//! NZXT CAM 4.76.6 driving a Kraken Elite 2024 and a Control Hub.

use super::hid;
use anyhow::Result;
use camless_core::lighting::{Effect, Lighting, Rgb};
use hidapi::HidDevice;
use std::time::Duration;

/// Up to six accessories are chained on one LED channel.
const ACCESSORIES_PER_CHANNEL: usize = 6;
/// CAM's LED count for accessories it does not tell apart, and for empty channels.
pub const DEFAULT_LEDS: u16 = 24;

/// Accessory names from liquidctl's `Hue2Accessory`, and LED counts where a CAM
/// capture showed them. Unknown ids show up in `camless status` as hex, so whoever
/// has the hardware can add them here.
fn accessory(id: u8) -> (Option<&'static str>, Option<u16>) {
    match id {
        0x01 => (Some("HUE+ LED Strip"), None),
        0x02 => (Some("AER RGB 1"), None),
        0x04 => (Some("HUE 2 LED Strip 300 mm"), None),
        0x05 => (Some("HUE 2 LED Strip 250 mm"), None),
        0x06 => (Some("HUE 2 LED Strip 200 mm"), None),
        0x07 => (Some("HUE 2 Cable Comb"), None),
        0x09 => (Some("HUE 2 Underglow 300 mm"), None),
        0x0a => (Some("HUE 2 Underglow 200 mm"), None),
        0x0b => (Some("AER RGB 2 120 mm"), None),
        0x0c => (Some("AER RGB 2 140 mm"), None),
        0x10 => (Some("Kraken X pump ring"), None),
        0x11 => (Some("Kraken X pump logo"), None),
        0x13 => (Some("F120 RGB"), None),
        0x14 => (Some("F140 RGB"), None),
        0x17 => (Some("F140 RGB Core"), None),
        // CAM: 8 LEDs, one fan.
        0x19 => (None, Some(8)),
        0x1b => (Some("F240 RGB Core"), None),
        // CAM: 24 LEDs on the Kraken Elite 2024's RGB header (three F120 RGB Core).
        0x1d => (Some("F360 RGB Core"), Some(24)),
        // CAM: 24 LEDs, the Kraken Elite 2024's pump ring.
        0x1e => (Some("Kraken Elite ring"), Some(24)),
        // CAM: 24 LEDs on a Control Hub port.
        0x1f => (None, Some(24)),
        _ => (None, None),
    }
}

/// What one LED channel has plugged in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChannelAccessories {
    pub ids: Vec<u8>,
}

impl ChannelAccessories {
    pub fn names(&self) -> Vec<String> {
        self.ids
            .iter()
            .map(|&id| accessory(id).0.map_or_else(|| format!("accessory {id:#04x}"), String::from))
            .collect()
    }

    /// What the first accessory on the channel is called, if camless knows it.
    pub fn name(&self) -> Option<&'static str> {
        self.ids.first().and_then(|&id| accessory(id).0)
    }

    /// The sum over known accessories; CAM's 24 when nothing on the channel is known.
    pub fn leds(&self) -> u16 {
        let known: Vec<u16> = self.ids.iter().filter_map(|&id| accessory(id).1).collect();
        if known.is_empty() || known.len() < self.ids.len() {
            DEFAULT_LEDS.max(known.iter().sum())
        } else {
            known.iter().sum()
        }
    }
}

/// Asks for the accessory report and returns `channels` entries.
pub fn read_accessories(dev: &HidDevice, channels: usize) -> Result<Vec<ChannelAccessories>> {
    let msg = hid::request(dev, &[0x20, 0x03], &[0x21, 0x03])?;
    Ok(parse_accessories(&msg, channels))
}

/// `21 03`: the channel count at [14], then six accessory ids per channel from [15].
pub fn parse_accessories(msg: &[u8], channels: usize) -> Vec<ChannelAccessories> {
    (0..channels)
        .map(|c| {
            let start = 15 + c * ACCESSORIES_PER_CHANNEL;
            let ids = msg.get(start..start + ACCESSORIES_PER_CHANNEL).unwrap_or_default();
            ChannelAccessories { ids: ids.iter().copied().take_while(|&id| id != 0).collect() }
        })
        .collect()
}

/// (mode byte, speed table, max colors), from liquidctl's `_COLOR_MODES`.
fn params(effect: Effect) -> (u8, usize, usize) {
    match effect {
        Effect::Fading => (0x01, 1, 8),
        Effect::SpectrumWave => (0x02, 2, 0),
        Effect::CoveringMarquee => (0x04, 2, 8),
        Effect::Pulse => (0x06, 5, 8),
        Effect::Breathing => (0x07, 6, 8),
        Effect::Candle => (0x08, 0, 1),
        Effect::RainbowFlow => (0x0b, 2, 0),
        Effect::SuperRainbow => (0x0c, 2, 0),
        Effect::RainbowPulse => (0x0d, 2, 0),
        Effect::TaiChi => (0x0e, 7, 2),
        Effect::WaterCooler => (0x0f, 6, 2),
    }
}

/// Speed bytes per table, liquidctl's `_SPEED_VALUE`.
const SPEED_VALUE: [[[u8; 2]; 5]; 8] = [
    [[0x32, 0x00]; 5],
    [[0x50, 0x00], [0x3c, 0x00], [0x28, 0x00], [0x14, 0x00], [0x0a, 0x00]],
    [[0x5e, 0x01], [0x2c, 0x01], [0xfa, 0x00], [0x96, 0x00], [0x50, 0x00]],
    [[0x40, 0x06], [0x14, 0x05], [0xe8, 0x03], [0x20, 0x03], [0x58, 0x02]],
    [[0x20, 0x03], [0xbc, 0x02], [0xf4, 0x01], [0x90, 0x01], [0x2c, 0x01]],
    [[0x19, 0x00], [0x14, 0x00], [0x0f, 0x00], [0x07, 0x00], [0x04, 0x00]],
    [[0x28, 0x00], [0x1e, 0x00], [0x14, 0x00], [0x0a, 0x00], [0x04, 0x00]],
    [[0x32, 0x00], [0x28, 0x00], [0x1e, 0x00], [0x14, 0x00], [0x0a, 0x00]],
];

fn grb(c: &Rgb) -> [u8; 3] {
    [c[1], c[0], c[2]]
}

/// One solid color, the two packets CAM sends (liquidctl#882): `26 04 <ch> 00` with
/// 24 GRB triplets, then the `26 06` apply.
pub fn static_color(channel: u8, color: &Rgb, leds: u16) -> Vec<Vec<u8>> {
    let mut colors = vec![0x26, 0x04, channel, 0x00];
    for _ in 0..24 {
        colors.extend_from_slice(&grb(color));
    }
    let apply = vec![
        0x26, 0x06, channel, 0x00, 0x01, 0x00, 0x00, leds.min(255) as u8, 0x00, 0x00, 0x80, 0x00, 0x32, 0x00, 0x00,
        0x01,
    ];
    vec![colors, apply]
}

/// An animated effect in the `2A 04` layout (liquidctl `KrakenX3._write_colors`),
/// with the channel's LED count in the footer as CAM sends it.
pub fn effect(channel: u8, effect: Effect, colors: &[Rgb], speed: usize, backward: bool, leds: u16) -> Vec<u8> {
    let (mode, table, max) = params(effect);
    let colors: Vec<Rgb> = if max == 0 { vec![[0, 0, 0]] } else { colors.iter().take(max).copied().collect() };
    let mut msg = vec![0x2a, 0x04, channel, channel, mode];
    msg.extend_from_slice(&SPEED_VALUE[table][speed.min(4)]);
    for i in 0..16 {
        msg.extend_from_slice(&colors.get(i).map(grb).unwrap_or([0, 0, 0]));
    }
    let mut count = colors.len() as u8;
    let direction = if backward { 0x02 } else { 0x00 };
    let backward_byte = if effect == Effect::CoveringMarquee { 0x04 } else { 0x00 } + direction;
    let mode_related = match effect {
        Effect::Fading | Effect::Pulse | Effect::Breathing => 0x08,
        Effect::TaiChi => 0x05,
        Effect::WaterCooler => {
            count = 1;
            0x05
        }
        _ => 0x00,
    };
    msg.extend_from_slice(&[backward_byte, count, mode_related, leds.min(255) as u8, 0x03]);
    msg
}

pub fn packets(channel: u8, lighting: &Lighting, leds: u16) -> Vec<Vec<u8>> {
    match lighting {
        Lighting::Off => static_color(channel, &[0, 0, 0], leds),
        Lighting::Fixed(c) => static_color(channel, c, leds),
        Lighting::Effect { effect: e, colors, speed, backward } => {
            vec![effect(channel, *e, colors, speed.index(), *backward, leds)]
        }
    }
}

/// One color in the `2A 04` layout, liquidctl's `fixed` mode: mode 00, its speed
/// table 0, one color slot. Its `off` is the same in black.
pub fn fixed(channel: u8, color: &Rgb, leds: u16) -> Vec<u8> {
    let mut msg = vec![0x2a, 0x04, channel, channel, 0x00];
    msg.extend_from_slice(&SPEED_VALUE[0][2]);
    msg.extend_from_slice(&grb(color));
    msg.extend_from_slice(&[0; 15 * 3]);
    msg.extend_from_slice(&[0x00, 1, 0x00, leds.min(255) as u8, 0x03]);
    msg
}

/// Lighting for the Kraken X3 and Z3, all in the `2A 04` layout as liquidctl sends
/// it; the `26 04` static colors came with the later coolers. `leds` goes where
/// liquidctl puts its per-channel `_STATIC_VALUE`.
pub fn packets_2a(channel: u8, lighting: &Lighting, leds: u16) -> Vec<Vec<u8>> {
    match lighting {
        Lighting::Off => vec![fixed(channel, &[0, 0, 0], leds)],
        Lighting::Fixed(c) => vec![fixed(channel, c, leds)],
        Lighting::Effect { effect: e, colors, speed, backward } => {
            vec![effect(channel, *e, colors, speed.index(), *backward, leds)]
        }
    }
}

pub fn write(dev: &HidDevice, channel: u8, lighting: &Lighting, leds: u16) -> Result<()> {
    send(dev, packets(channel, lighting, leds))
}

/// The devices throttle lighting: more than about four packets in half a second are
/// dropped, not queued (camera measurements in liquidctl#882). Space them out.
pub fn send(dev: &HidDevice, packets: Vec<Vec<u8>>) -> Result<()> {
    for p in packets {
        hid::write(dev, &p)?;
        std::thread::sleep(Duration::from_millis(150));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use camless_core::lighting::Speed;

    #[test]
    fn static_color_matches_cam_capture() {
        let p = static_color(0x01, &[0xaa, 0x00, 0xff], 24);
        assert_eq!(&p[0][..7], &[0x26, 0x04, 0x01, 0x00, 0x00, 0xaa, 0xff]);
        assert_eq!(p[0].len(), 4 + 24 * 3);
        assert_eq!(
            p[1],
            vec![0x26, 0x06, 0x01, 0x00, 0x01, 0x00, 0x00, 0x18, 0x00, 0x00, 0x80, 0x00, 0x32, 0x00, 0x00, 0x01]
        );
    }

    #[test]
    fn tai_chi_matches_cam_capture() {
        // CAM, hub port 5: 2a 04 10 10 0e 1e 00 32 32 de ff ff ff 00… 00 02 05 08 03
        let m = effect(0x10, Effect::TaiChi, &[[50, 50, 222], [255, 255, 255]], Speed::Normal.index(), false, 8);
        assert_eq!(&m[..7], &[0x2a, 0x04, 0x10, 0x10, 0x0e, 0x1e, 0x00]);
        assert_eq!(&m[7..13], &[50, 50, 222, 255, 255, 255]);
        assert_eq!(m.len(), 7 + 48 + 5);
        assert_eq!(&m[55..], &[0x00, 2, 0x05, 8, 0x03]);
    }

    #[test]
    fn colorless_effect_sends_one_black_slot() {
        let m = effect(0x02, Effect::SpectrumWave, &[[1, 2, 3]], Speed::Fastest.index(), true, 24);
        assert_eq!(&m[5..7], &[0x50, 0x00]);
        assert_eq!(&m[55..57], &[0x02, 1]);
    }

    #[test]
    fn fixed_in_the_2a_layout_matches_liquidctl() {
        // liquidctl KrakenX3 set_color('ring', 'fixed', [[0xff, 0x10, 0x20]]): ring 0b010,
        // mode 00, speed 32 00, one GRB slot, footer 00 01 00 <static 8> 03.
        let m = fixed(0b010, &[0xff, 0x10, 0x20], 8);
        assert_eq!(&m[..10], &[0x2a, 0x04, 0x02, 0x02, 0x00, 0x32, 0x00, 0x10, 0xff, 0x20]);
        assert!(m[10..55].iter().all(|&b| b == 0));
        assert_eq!(&m[55..], &[0x00, 1, 0x00, 8, 0x03]);
        assert_eq!(packets_2a(0b001, &Lighting::Off, 40), vec![fixed(0b001, &[0, 0, 0], 40)]);
    }

    #[test]
    fn accessories_from_cam_capture() {
        // The Control Hub's 21 03 reply in the CAM capture.
        let mut msg = vec![0u8; 64];
        msg[..2].copy_from_slice(&[0x21, 0x03]);
        msg[14] = 5;
        msg[15] = 0x1f;
        msg[21] = 0x1f;
        msg[39] = 0x19;
        let ch = parse_accessories(&msg, 5);
        assert_eq!(ch[0].ids, vec![0x1f]);
        assert_eq!(ch[2].ids, Vec::<u8>::new());
        assert_eq!(ch.iter().map(|c| c.leds()).collect::<Vec<_>>(), vec![24, 24, 24, 24, 8]);
        assert_eq!(ch[4].names(), vec!["accessory 0x19"]);
    }
}
