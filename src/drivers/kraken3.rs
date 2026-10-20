//! The Krakens before the Elite 2024, on the same command set: the Kraken X3 (X53,
//! X63, X73), the Kraken Z3 (Z53, Z63, Z73), the Kraken 2023 and the Kraken Elite
//! 2023. Curves are stored in the cooler as on the Elite 2024, under the same safety
//! floor; the fan header, lighting and the screen differ by model.
//!
//! Sources: liquidctl `kraken3.py`, the Linux `nzxt-kraken3` driver, and Nexus
//! (`KrakenModel.cs`, `KrakenProtocol.cs`, `KrakenHub.cs`). docs/protocol.md says
//! where they disagree and which one this follows.
//! Not verified on hardware.

use super::kraken_elite_v2::{self as elite, Status};
use super::{hid, hue2, Driver, FanReading, Found, Lcd, Model, Reading, NZXT_VID};
use crate::q565;
use anyhow::{bail, Context, Result};
use camless_core::lighting::Lighting;
use camless_core::status::{Device, FanChannel, LcdInfo, LightChannel, Unit};
use hidapi::{HidApi, HidDevice};
use std::time::{Duration, Instant};

pub const X3: Model = Model::hid(NZXT_VID, 0x2007, "kraken-x3", "Kraken X3", open);
/// A later X3; liquidctl and the Linux driver treat it as the same cooler.
pub const X3_2014: Model = Model::hid(NZXT_VID, 0x2014, "kraken-x3-2014", "Kraken X3", open);
pub const Z3: Model = Model::hid(NZXT_VID, 0x3008, "kraken-z3", "Kraken Z3", open);
pub const KRAKEN_2023: Model = Model::hid(NZXT_VID, 0x300e, "kraken-2023", "Kraken (2023)", open);
pub const ELITE_2023: Model = Model::hid(NZXT_VID, 0x300c, "kraken-elite-2023", "Kraken Elite (2023)", open);

/// How a picture reaches the screen without the memory buckets, which are flash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stream {
    /// Only through the buckets, so camless leaves the cooler on its own display.
    None,
    /// Raw RGB565, high byte first, image format 6, on firmware 2 (liquidctl).
    Rgb565,
    /// Raw BGR, a byte per channel, image format 9, from firmware 2 on (Nexus).
    Bgr888,
}

struct Light {
    id: &'static str,
    name: &'static str,
    channel: u8,
    /// What liquidctl sends where CAM sends the LED count (`_STATIC_VALUE`).
    leds: u16,
}

struct Kind {
    /// The bytes after `72` for the pump, and for the fan header if there is one.
    pump: [u8; 3],
    fan: Option<[u8; 3]>,
    /// Answers `74 01`. The X3 has no such request and only reports every 0.5 s.
    asks: bool,
    lights: &'static [Light],
    /// The screen's side, and the stream it takes on firmware 2.
    lcd: Option<(u16, Stream)>,
}

static X3_KIND: Kind = Kind {
    pump: [0x01, 0x00, 0x00],
    fan: None,
    asks: false,
    lights: &[
        Light { id: "rgb", name: "RGB header", channel: 0b001, leds: 40 },
        Light { id: "ring", name: "Pump ring", channel: 0b010, leds: 8 },
        Light { id: "logo", name: "Pump logo", channel: 0b100, leds: 1 },
    ],
    lcd: None,
};

static Z3_KIND: Kind = Kind {
    pump: [0x01, 0x00, 0x00],
    fan: Some([0x02, 0x00, 0x00]),
    asks: true,
    lights: &[Light { id: "rgb", name: "RGB header", channel: 0b001, leds: 40 }],
    lcd: Some((320, Stream::None)),
};

// Nexus sends the 2023 curves with `01 00 00` and `02 00 00` below firmware 2.1.1.
// liquidctl and the Linux driver send these on any firmware; camless follows them.
static KRAKEN_2023_KIND: Kind = Kind {
    pump: elite::PUMP_CHANNEL,
    fan: Some(elite::FAN_CHANNEL),
    asks: true,
    lights: &[],
    lcd: Some((240, Stream::Rgb565)),
};

static ELITE_2023_KIND: Kind = Kind {
    pump: elite::PUMP_CHANNEL,
    fan: Some(elite::FAN_CHANNEL),
    asks: true,
    lights: &[],
    lcd: Some((640, Stream::Bgr888)),
};

fn kind(pid: u16) -> &'static Kind {
    match pid {
        0x3008 => &Z3_KIND,
        0x300e => &KRAKEN_2023_KIND,
        0x300c => &ELITE_2023_KIND,
        // 0x2007 and 0x2014: `open` is reached only through the models above.
        _ => &X3_KIND,
    }
}

/// The image formats in the `36 01` start and the bulk header.
const FORMAT_RGB565: u8 = 0x06;
const FORMAT_BGR888: u8 = 0x09;
/// liquidctl reads a reply to these without looking at it, and Nexus waits up to 150 ms
/// without needing one, so a cooler that does not send them must not slow every frame
/// by much.
const STREAM_REPLY_TIMEOUT: Duration = Duration::from_millis(200);

/// What came back for an LCD command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reply {
    Done,
    /// Report `ff` naming the command in bytes 14 and 15, Nexus's NAK.
    Refused,
    Silent,
}

/// Whether report `r` answers `payload`, and how; `None` for any other report.
fn answer(payload: &[u8], r: &[u8]) -> Option<Reply> {
    if r.len() > 1 && r[0] == payload[0] + 1 && r[1] == payload[1] {
        Some(Reply::Done)
    } else if r.len() > 15 && r[0] == 0xff && r[14] == payload[0] && r[15] == payload[1] {
        Some(Reply::Refused)
    } else {
        None
    }
}

/// The stream a cooler takes on its firmware, as far as its source goes: liquidctl
/// streams to the Kraken 2023 on firmware 2 only, Nexus to the Elite 2023 from 2 on.
fn stream_on(kind: &Kind, firmware: Option<(u8, u8, u8)>) -> Stream {
    let major = firmware.map(|f| f.0);
    match kind.lcd {
        Some((_, Stream::Rgb565)) if major == Some(2) => Stream::Rgb565,
        Some((_, Stream::Bgr888)) if major.is_some_and(|m| m >= 2) => Stream::Bgr888,
        _ => Stream::None,
    }
}

/// A Q565 frame as the raw pixels these screens take, turned `turns` quarter turns
/// clockwise first. liquidctl and Nexus both turn pictures themselves: the screen's
/// own setting turns only its built-in display.
fn raw_frame(q565: &[u8], side: u16, turns: u8, stream: Stream) -> Result<Vec<u8>> {
    let (w, h, px) = q565::decode(q565).context("not a Q565 frame")?;
    if (w, h) != (side, side) {
        bail!("a {w}×{h} frame for a {side}×{side} screen");
    }
    let n = usize::from(side);
    let mut out = Vec::with_capacity(n * n * if stream == Stream::Bgr888 { 3 } else { 2 });
    for y in 0..n {
        for x in 0..n {
            let (sx, sy) = match turns & 3 {
                1 => (y, n - 1 - x),
                2 => (n - 1 - x, n - 1 - y),
                3 => (n - 1 - y, x),
                _ => (x, y),
            };
            let p = px[sy * n + sx];
            match stream {
                Stream::Bgr888 => {
                    let (r, g, b) = ((p >> 11) as u8, ((p >> 5) & 63) as u8, (p & 31) as u8);
                    out.extend_from_slice(&[b << 3 | b >> 2, g << 2 | g >> 4, r << 3 | r >> 2]);
                }
                _ => out.extend_from_slice(&p.to_be_bytes()),
            }
        }
    }
    Ok(out)
}

pub struct Kraken3 {
    model: &'static Model,
    kind: &'static Kind,
    hid: HidDevice,
    usb: Option<nusb::Interface>,
    firmware: Option<(u8, u8, u8)>,
    accessories: Vec<hue2::ChannelAccessories>,
    turns: u8,
    streaming: bool,
}

fn open(api: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    let kind = kind(found.model.pid);
    let hid = hid::open(api, &found.path)?;
    hid::drain(&hid);
    // A status report every 0.5 s, as liquidctl and the Linux driver set it up.
    hid::write(&hid, &[0x70, 0x02, 0x01, 0xb8, 0x01])?;
    hid::write(&hid, &[0x70, 0x01])?;
    let fw = hid::request(&hid, &[0x10, 0x01], &[0x11, 0x01])?;
    let firmware = (fw.len() > 19).then(|| (fw[17], fw[18], fw[19]));
    let accessories = if kind.lights.is_empty() {
        Vec::new()
    } else {
        hue2::read_accessories(&hid, kind.lights.len()).unwrap_or_default()
    };
    Ok(Box::new(Kraken3 { model: found.model, kind, hid, usb: None, firmware, accessories, turns: 0, streaming: false }))
}

impl Kraken3 {
    fn stream(&self) -> Stream {
        stream_on(self.kind, self.firmware)
    }

    fn status(&mut self) -> Result<Status> {
        let msg = if self.kind.asks {
            hid::drain(&self.hid);
            hid::request(&self.hid, &[0x74, 0x01], &[0x75])?
        } else {
            // The newest report waiting, or the next one.
            match hid::pending(&self.hid).into_iter().rev().find(|m| m.first() == Some(&0x75)) {
                Some(m) => m,
                None => hid::read_prefix(&self.hid, &[0x75])?,
            }
        };
        elite::parse_status(&msg).context("the cooler reported an invalid liquid temperature")
    }

    fn curve(&self, fan: &str, duty_at: &dyn Fn(f32) -> u8) -> Option<Vec<u8>> {
        let (channel, pump) = match fan {
            "pump" => (self.kind.pump, true),
            "fan" => (self.kind.fan?, false),
            _ => return None,
        };
        Some(elite::curve_report(&channel, &elite::table(pump, duty_at)))
    }

    /// Sends an LCD command and waits up to `wait` for its reply. Any `[cmd + 1, sub]`
    /// reply counts: the Elite 2023 on firmware 2 is reported to answer with code `05`
    /// where the Elite 2024 sends `01` (Nexus), and liquidctl checks no codes.
    fn command(&self, payload: &[u8], wait: Duration) -> Result<Reply> {
        // A late reply to an earlier command must not answer this one.
        hid::drain(&self.hid);
        hid::write(&self.hid, payload)?;
        let mut buf = [0u8; hid::REPORT_LEN];
        let end = Instant::now() + wait;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(Reply::Silent);
            }
            let n = self.hid.read_timeout(&mut buf, left.as_millis().max(1) as i32)?;
            if let Some(reply) = answer(payload, &buf[..n]) {
                return Ok(reply);
            }
        }
    }

    /// A command that fails only on a refusal: neither source needs the reply.
    fn expect(&self, payload: &[u8], what: &str) -> Result<()> {
        if self.command(payload, STREAM_REPLY_TIMEOUT)? == Reply::Refused {
            let _ = self.command(&[0x36, 0x03], STREAM_REPLY_TIMEOUT);
            bail!("the cooler refused {what}");
        }
        Ok(())
    }

    /// Start, the header and the pixels in transfers of their own, end.
    fn send_frame(&mut self, format: u8, data: &[u8]) -> Result<()> {
        if self.usb.is_none() {
            self.usb = Some(elite::open_lcd_interface(self.model)?);
        }
        self.expect(&[0x36, 0x01, 0x00, 0x01, format], "the frame")?;
        let usb = self.usb.as_ref().context("LCD interface not open")?;
        for data in [elite::lcd_bulk_header(format, data.len() as u32), data.to_vec()] {
            if let Err(e) = elite::lcd_bulk_out(usb, data) {
                let _ = self.command(&[0x36, 0x03], STREAM_REPLY_TIMEOUT);
                return Err(e);
            }
        }
        self.expect(&[0x36, 0x02], "the end of the frame")
    }
}

impl Driver for Kraken3 {
    fn describe(&self) -> Device {
        let fan = |id: &str, name: &str, pump: bool| FanChannel {
            id: id.into(),
            name: name.into(),
            pump,
            min_duty: if pump { elite::PUMP_MIN_DUTY } else { 0 },
            device_curves: true,
            ..Default::default()
        };
        let mut fans = vec![fan("pump", "Pump", true)];
        if self.kind.fan.is_some() {
            fans.push(fan("fan", "Radiator fans", false));
        }
        let lights = self
            .kind
            .lights
            .iter()
            .enumerate()
            .map(|(c, l)| {
                let a = self.accessories.get(c).cloned().unwrap_or_default();
                // The pump's own LEDs are fixed; the header has whatever is plugged in.
                let leds = if l.channel == 0b001 { a.leds() } else { l.leds };
                LightChannel { id: l.id.into(), name: l.name.into(), leds, accessories: a.names(), ..Default::default() }
            })
            .collect();
        Device {
            name: self.model.name.into(),
            firmware: self.firmware.map(|(a, b, c)| format!("{a}.{b}.{c}")),
            verified: self.model.verified(),
            fans,
            lights,
            lcd: self.kind.lcd.map(|(side, _)| LcdInfo { width: side, height: side, round: true, frames: self.stream() != Stream::None }),
            ..Default::default()
        }
    }

    fn sensors(&self) -> Vec<(&'static str, &'static str, Unit)> {
        vec![("liquid", "Liquid", Unit::Celsius)]
    }

    fn poll(&mut self) -> Result<Reading> {
        let s = self.status()?;
        let fan = |rpm, duty| FanReading { rpm: Some(rpm), duty: Some(duty), connected: None };
        let mut fans = vec![("pump", fan(s.pump_rpm, s.pump_duty))];
        if self.kind.fan.is_some() {
            fans.push(("fan", fan(s.fan_rpm, s.fan_duty)));
        }
        Ok(Reading { sensors: vec![("liquid", s.liquid_c)], fans })
    }

    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
        for &(fan, duty) in duties {
            let msg = self.curve(fan, &|_| duty).with_context(|| format!("no fan {fan:?} on this cooler"))?;
            hid::write(&self.hid, &msg)?;
        }
        Ok(())
    }

    fn store_curve(&mut self, fan: &str, sensor: Option<&str>, duty_at: &dyn Fn(f32) -> u8) -> Result<bool> {
        if !matches!(sensor, None | Some("liquid")) {
            return Ok(false);
        }
        match self.curve(fan, duty_at) {
            Some(msg) => hid::write(&self.hid, &msg).map(|_| true),
            None => Ok(false),
        }
    }

    fn set_lighting(&mut self, light: &str, lighting: &Lighting) -> Result<()> {
        let Some(l) = self.kind.lights.iter().find(|l| l.id == light) else {
            bail!("no light {light:?} on this cooler")
        };
        hue2::send(&self.hid, hue2::packets_2a(l.channel, lighting, l.leds))
    }

    fn lcd(&mut self) -> Option<&mut dyn Lcd> {
        if self.kind.lcd.is_some() {
            Some(self)
        } else {
            None
        }
    }
}

impl Lcd for Kraken3 {
    fn settings(&mut self, brightness: u8, quarter_turns: u8) -> Result<()> {
        self.turns = quarter_turns & 3;
        hid::write(&self.hid, &[0x30, 0x02, 0x01, brightness.min(100), 0x00, 0x00, 0x01, quarter_turns & 3])
    }

    /// `38 01 02 00`, the liquid screen in liquidctl's `set_screen`.
    fn show_builtin(&mut self) -> Result<()> {
        if self.streaming {
            let _ = self.command(&[0x36, 0x03], STREAM_REPLY_TIMEOUT);
        }
        self.streaming = false;
        if self.command(&[0x38, 0x01, 0x02, 0x00], elite::LCD_REPLY_TIMEOUT)? == Reply::Refused {
            bail!("the cooler refused the liquid screen");
        }
        Ok(())
    }

    /// One frame on the bucketless path. Nexus clears the memory buckets before
    /// streaming to the Elite 2023; camless does not touch them, as they are flash.
    fn frame(&mut self, q565: &[u8]) -> Result<()> {
        let side = self.kind.lcd.map_or(0, |(s, _)| s);
        let stream = self.stream();
        let format = match stream {
            Stream::None => bail!("this cooler takes pictures only into its flash memory, which camless does not write"),
            Stream::Rgb565 => FORMAT_RGB565,
            Stream::Bgr888 => FORMAT_BGR888,
        };
        let data = raw_frame(q565, side, self.turns, stream)?;
        let mut times = 1;
        if !self.streaming {
            // A transfer left open by an earlier process makes the device refuse new ones.
            let _ = self.command(&[0x36, 0x03], STREAM_REPLY_TIMEOUT);
            // liquidctl sends the first picture to the Kraken 2023 twice.
            if stream == Stream::Rgb565 {
                times = 2;
            }
        }
        for _ in 0..times {
            self.send_frame(format, &data)?;
        }
        self.streaming = true;
        Ok(())
    }

    fn is_streaming(&self) -> bool {
        self.streaming
    }

    fn size(&self) -> (u16, u16) {
        let side = self.kind.lcd.map_or(0, |(s, _)| s);
        (side, side)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_headers_follow_liquidctl_and_linux() {
        let report = |k: &Kind, pump: bool| {
            let channel = if pump { k.pump } else { k.fan.unwrap() };
            elite::curve_report(&channel, &elite::table(pump, &|_| 50))[..4].to_vec()
        };
        assert_eq!(report(&X3_KIND, true), [0x72, 0x01, 0x00, 0x00]);
        assert!(X3_KIND.fan.is_none());
        assert_eq!(report(&Z3_KIND, true), [0x72, 0x01, 0x00, 0x00]);
        assert_eq!(report(&Z3_KIND, false), [0x72, 0x02, 0x00, 0x00]);
        for k in [&KRAKEN_2023_KIND, &ELITE_2023_KIND] {
            assert_eq!(report(k, true), [0x72, 0x01, 0x01, 0x00]);
            assert_eq!(report(k, false), [0x72, 0x02, 0x01, 0x01]);
        }
    }

    #[test]
    fn stored_tables_keep_the_floor_and_the_pump_minimum() {
        let t = elite::table(true, &|_| 0);
        assert_eq!((t[0], t[25], t[30], t[35]), (elite::PUMP_MIN_DUTY, elite::PUMP_MIN_DUTY, 50, 100));
    }

    #[test]
    fn every_model_maps_to_its_kind() {
        assert!(std::ptr::eq(kind(X3.pid), &X3_KIND) && std::ptr::eq(kind(X3_2014.pid), &X3_KIND));
        assert!(std::ptr::eq(kind(Z3.pid), &Z3_KIND));
        assert!(std::ptr::eq(kind(KRAKEN_2023.pid), &KRAKEN_2023_KIND));
        assert!(std::ptr::eq(kind(ELITE_2023.pid), &ELITE_2023_KIND));
    }

    #[test]
    fn x3_lighting_uses_liquidctl_channels_and_static_values() {
        // liquidctl _COLOR_CHANNELS_KRAKENX and _STATIC_VALUE.
        let l: Vec<_> = X3_KIND.lights.iter().map(|l| (l.id, l.channel, l.leds)).collect();
        assert_eq!(l, [("rgb", 0b001, 40), ("ring", 0b010, 8), ("logo", 0b100, 1)]);
        let p = hue2::packets_2a(0b100, &Lighting::Fixed([1, 2, 3]), 1);
        assert_eq!(&p[0][..10], &[0x2a, 0x04, 0x04, 0x04, 0x00, 0x32, 0x00, 2, 1, 3]);
        assert_eq!(&p[0][55..], &[0x00, 1, 0x00, 1, 0x03]);
    }

    #[test]
    fn raw_frames_turn_and_pack_like_the_sources() {
        // 2×2: red, green / blue, white.
        let px = [0xf800, 0x07e0, 0x001f, 0xffff];
        let q = q565::encode(2, 2, &px);
        // liquidctl's RGB565: high byte first.
        assert_eq!(raw_frame(&q, 2, 0, Stream::Rgb565).unwrap(), [0xf8, 0x00, 0x07, 0xe0, 0x00, 0x1f, 0xff, 0xff]);
        // A quarter turn clockwise: blue, red / white, green.
        assert_eq!(raw_frame(&q, 2, 1, Stream::Rgb565).unwrap(), [0x00, 0x1f, 0xf8, 0x00, 0xff, 0xff, 0x07, 0xe0]);
        // Nexus's BGR: blue, green, red, each widened to 8 bits.
        let bgr = raw_frame(&q, 2, 0, Stream::Bgr888).unwrap();
        assert_eq!(&bgr[..6], &[0, 0, 0xff, 0, 0xff, 0]);
        assert_eq!(&bgr[9..], &[0xff, 0xff, 0xff]);
        assert!(raw_frame(&q, 4, 0, Stream::Rgb565).is_err());
    }

    #[test]
    fn any_reply_answers_and_only_a_matching_nak_refuses() {
        let start = [0x36, 0x01, 0x00, 0x01, FORMAT_BGR888];
        let mut reply = [0u8; 64];
        reply[..2].copy_from_slice(&[0x37, 0x01]);
        for code in [0x01, 0x05, 0x00] {
            reply[14] = code;
            assert_eq!(answer(&start, &reply), Some(Reply::Done));
        }
        let mut nak = [0u8; 64];
        nak[0] = 0xff;
        nak[14..16].copy_from_slice(&[0x36, 0x01]);
        assert_eq!(answer(&start, &nak), Some(Reply::Refused));
        // A NAK raised by another command, and the periodic status, answer nothing.
        nak[14..16].copy_from_slice(&[0x2a, 0x04]);
        assert_eq!(answer(&start, &nak), None);
        assert_eq!(answer(&start, &[0x75, 0x02, 0, 0]), None);
        assert_eq!(answer(&[0x36, 0x02], &[0x37, 0x01]), None);
    }

    #[test]
    fn streams_only_on_the_firmware_the_sources_name() {
        let fw = |major| Some((major, 0, 0));
        assert_eq!(stream_on(&KRAKEN_2023_KIND, fw(2)), Stream::Rgb565);
        for f in [fw(1), fw(3), None] {
            assert_eq!(stream_on(&KRAKEN_2023_KIND, f), Stream::None);
        }
        assert_eq!(stream_on(&ELITE_2023_KIND, fw(2)), Stream::Bgr888);
        assert_eq!(stream_on(&ELITE_2023_KIND, fw(3)), Stream::Bgr888);
        assert_eq!(stream_on(&ELITE_2023_KIND, fw(1)), Stream::None);
        assert_eq!(stream_on(&ELITE_2023_KIND, None), Stream::None);
        assert_eq!(stream_on(&Z3_KIND, fw(2)), Stream::None);
        assert_eq!(stream_on(&X3_KIND, fw(2)), Stream::None);
    }

    #[test]
    fn stream_headers() {
        assert_eq!(&elite::lcd_bulk_header(FORMAT_RGB565, 240 * 240 * 2)[12..], &[0x06, 0, 0, 0, 0x00, 0xc2, 0x01, 0x00]);
        assert_eq!(&elite::lcd_bulk_header(FORMAT_BGR888, 640 * 640 * 3)[12..], &[0x09, 0, 0, 0, 0x00, 0xc0, 0x12, 0x00]);
    }
}
