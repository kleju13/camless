//! NZXT Kraken Elite 2024 (`1e71:3012`, "KrakenEliteV2" in CAM): pump, the fan
//! header for the radiator fans, the pump ring, the RGB header and the 640×640 LCD.
//! The Kraken Plus 2024 (`1e71:3014`) runs on the same driver, with its screen size
//! and lighting channels as the cooler reports them.
//!
//! Sources: liquidctl `kraken3.py` and PRs #882, #927, #932, the Linux
//! `nzxt-kraken3` driver, USB captures of NZXT CAM 4.76.6 on firmware 1.2.0, and for
//! the Plus, Nexus (`KrakenModel.cs`, `KrakenProtocol.cs`, `KrakenHub.cs`).
//! Verified on hardware, the Elite only: pump and fan curves, lighting, LCD streaming.

use super::{hid, hue2, usb, Driver, FanReading, Found, Lcd, Model, Reading};
use anyhow::{anyhow, bail, Context as _, Result};
use camless_core::lighting::Lighting;
use camless_core::status::{Device, FanChannel, LcdInfo, LightChannel, Unit};
use hidapi::{HidApi, HidDevice};
use std::time::Duration;

pub const MODEL: Model = Model { interface: Some(1), ..Model::hid(0x1e71, 0x3012, "kraken-elite-v2", "Kraken Elite (2024)", open) };

/// liquidctl's name and 240×240 screen; Nexus lists it as a second Elite 2024 with the
/// 640×640 one. The driver asks the cooler instead of picking one.
pub const PLUS: Model = Model { interface: Some(1), ..Model::hid(0x1e71, 0x3014, "kraken-plus-2024", "Kraken Plus (2024)", open) };

/// The cooler evaluates its stored curves for liquid temperatures 20..=59 °C.
const CURVE_FIRST_C: u8 = 20;
const CURVE_POINTS: usize = 40;
pub const PUMP_MIN_DUTY: u8 = 20;
const RING: u8 = 0b01;
const RGB_HEADER: u8 = 0b10;
const LCD_SIDE: u16 = 640;
/// liquidctl's size for the Plus, if it does not say.
const PLUS_LCD_SIDE: u16 = 240;

/// The floor under every table camless stores, by liquid temperature: if camless
/// stops or hangs, the cooler still speeds up on its own as the liquid warms.
pub fn safety_floor(liquid_c: u8) -> u8 {
    match liquid_c {
        ..=45 => 0,
        55.. => 100,
        t => (t - 45) * 10,
    }
}

/// The 40 duties the cooler stores, with the pump minimum and the safety floor.
pub fn table(pump: bool, duty_at: &dyn Fn(f32) -> u8) -> [u8; CURVE_POINTS] {
    let min = if pump { PUMP_MIN_DUTY } else { 0 };
    std::array::from_fn(|i| {
        let t = CURVE_FIRST_C + i as u8;
        duty_at(t as f32).max(safety_floor(t)).clamp(min, 100)
    })
}

/// The channel bytes after `72`, which liquidctl and the Linux driver send the
/// coolers since the Kraken 2023 (the X3 and Z3 take `01 00 00` and `02 00 00`).
pub const PUMP_CHANNEL: [u8; 3] = [0x01, 0x01, 0x00];
pub const FAN_CHANNEL: [u8; 3] = [0x02, 0x01, 0x01];

pub fn curve_report(channel: &[u8; 3], table: &[u8; CURVE_POINTS]) -> Vec<u8> {
    let mut msg = vec![0x72];
    msg.extend_from_slice(channel);
    msg.extend_from_slice(table);
    msg
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub liquid_c: f32,
    pub pump_rpm: u16,
    pub pump_duty: u8,
    pub fan_rpm: u16,
    pub fan_duty: u8,
}

/// `75 01` (reply to `74 01`) and `75 02` (periodic). `ff ff` liquid means a fault.
pub fn parse_status(msg: &[u8]) -> Option<Status> {
    if msg.len() < 26 || msg[0] != 0x75 || (msg[15] == 0xff && msg[16] == 0xff) {
        return None;
    }
    Some(Status {
        liquid_c: msg[15] as f32 + msg[16] as f32 / 10.0,
        pump_rpm: u16::from_le_bytes([msg[17], msg[18]]),
        pump_duty: msg[19],
        fan_rpm: u16::from_le_bytes([msg[23], msg[24]]),
        fan_duty: msg[25],
    })
}

/// LCD frames are Q565 (format 8), the only format the screen shows; see `q565`.
const LCD_FORMAT_Q565: u8 = 0x08;
/// Starts every bulk transfer; the same bytes are in the firmware images CAM ships.
const LCD_BULK_MAGIC: [u8; 12] = [0x12, 0xfa, 0x01, 0xe8, 0xab, 0xcd, 0xef, 0x98, 0x76, 0x54, 0x32, 0x10];
pub const LCD_BULK_ENDPOINT: u8 = 0x02;
/// LCD commands run inside the cooling loop, so a cooler that stops answering them
/// must not hold the fans up for long. CAM sees these replies within 5 ms.
pub const LCD_REPLY_TIMEOUT: Duration = Duration::from_millis(500);

/// One bulk transfer of a frame takes about 40 ms. The frames go out from the cooling
/// loop, so a cooler that never takes one must not hold the fans.
const LCD_BULK_TIMEOUT: Duration = Duration::from_secs(1);

/// A bulk write to the LCD that gives up after `LCD_BULK_TIMEOUT`.
pub fn lcd_bulk_out(usb: &nusb::Interface, data: Vec<u8>) -> Result<()> {
    match usb::wait(usb.bulk_out(LCD_BULK_ENDPOINT, data), LCD_BULK_TIMEOUT) {
        Some(done) => done.status.map_err(|e| anyhow!("bulk transfer to the LCD failed: {e}")),
        None => bail!("the LCD took no data within {} ms", LCD_BULK_TIMEOUT.as_millis()),
    }
}

/// The 20 bytes ahead of an image: the magic, the image format, the length.
pub fn lcd_bulk_header(format: u8, len: u32) -> Vec<u8> {
    let mut h = LCD_BULK_MAGIC.to_vec();
    h.extend_from_slice(&[format, 0, 0, 0]);
    h.extend_from_slice(&len.to_le_bytes());
    h
}

/// The screen size in the `31 01` reply to `30 01`: width and height, u16 LE, at
/// [20] and [22] (Nexus `DecodeLcdInfo`, from its Elite 2024).
pub fn parse_lcd_size(msg: &[u8]) -> Option<(u16, u16)> {
    if msg.len() < 24 || msg[..2] != [0x31, 0x01] {
        return None;
    }
    let (w, h) = (u16::from_le_bytes([msg[20], msg[21]]), u16::from_le_bytes([msg[22], msg[23]]));
    ((1..=1024).contains(&w) && (1..=1024).contains(&h)).then_some((w, h))
}

struct Light {
    id: &'static str,
    name: &'static str,
    channel: u8,
    leds: u16,
    accessories: Vec<String>,
}

pub struct KrakenEliteV2 {
    model: &'static Model,
    hid: HidDevice,
    usb: Option<nusb::Interface>,
    streaming: bool,
    firmware: Option<String>,
    lights: Vec<Light>,
    lcd_size: (u16, u16),
}

fn open(api: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    let hid = hid::open(api, &found.path)?;
    hid::drain(&hid);
    // A status report every 0.5 s (0xB8 with interval 1).
    hid::write(&hid, &[0x70, 0x02, 0x01, 0xb8, 0x01])?;
    hid::write(&hid, &[0x70, 0x01])?;
    let fw = hid::request(&hid, &[0x10, 0x01], &[0x11, 0x01])?;
    let firmware = (fw.len() > 19).then(|| format!("{}.{}.{}", fw[17], fw[18], fw[19]));
    let plus = found.model.pid == PLUS.pid;
    let (channels, lcd_size) = if plus {
        // The Plus says how many LED channels it has, and how big its screen is. The
        // channel table can come back empty right after reporting starts, so Nexus asks
        // up to three times.
        let mut acc = Vec::new();
        for _ in 0..3 {
            if let Ok(m) = hid::request(&hid, &[0x20, 0x03], &[0x21, 0x03]) {
                acc = hue2::parse_accessories(&m, usize::from(m.get(14).copied().unwrap_or(0)).min(2));
                if acc.iter().any(|a| !a.ids.is_empty()) {
                    break;
                }
            }
        }
        let size = hid::request(&hid, &[0x30, 0x01], &[0x31, 0x01]).ok().as_deref().and_then(parse_lcd_size);
        (acc, size.unwrap_or((PLUS_LCD_SIDE, PLUS_LCD_SIDE)))
    } else {
        (hue2::read_accessories(&hid, 2).unwrap_or_default(), (LCD_SIDE, LCD_SIDE))
    };
    // The Elite always has both; the Plus only what it reports, named after what is on
    // each channel, as Nexus names them.
    let count = if plus { channels.len() } else { 2 };
    let lights = [("ring", "Pump ring", RING), ("rgb", "RGB header", RGB_HEADER)]
        .into_iter()
        .take(count)
        .enumerate()
        .map(|(c, (id, name, channel))| Light {
            id,
            name: channels.get(c).and_then(|a| a.name()).filter(|_| plus).unwrap_or(name),
            channel,
            leds: channels.get(c).map_or(hue2::DEFAULT_LEDS, |a| a.leds()),
            accessories: channels.get(c).map(|a| a.names()).unwrap_or_default(),
        })
        .collect();
    Ok(Box::new(KrakenEliteV2 { model: found.model, hid, usb: None, streaming: false, firmware, lights, lcd_size }))
}

impl KrakenEliteV2 {
    /// An LCD command; its reply is `[cmd + 1, sub, …]` with a result code in byte 14.
    fn command(&self, payload: &[u8]) -> Result<u8> {
        hid::write(&self.hid, payload)?;
        let reply = hid::read_prefix_within(&self.hid, &[payload[0] + 1, payload[1]], LCD_REPLY_TIMEOUT)?;
        Ok(reply.get(14).copied().unwrap_or(0))
    }

    fn push_frame(&self, q565: &[u8]) -> Result<()> {
        let begin = [0x36, 0x01, 0x00, 0x01, LCD_FORMAT_Q565];
        if self.command(&begin)? != 0x01 {
            let _ = self.command(&[0x36, 0x03]);
            let code = self.command(&begin)?;
            if code != 0x01 {
                bail!("the cooler refused the frame (code {code:#04x})");
            }
        }
        let usb = self.usb.as_ref().context("LCD interface not open")?;
        // The header goes in a transfer of its own, as CAM sends it. Sharing one with
        // the pixels delayed the cooler's end reply by seconds.
        for data in [lcd_bulk_header(LCD_FORMAT_Q565, q565.len() as u32), q565.to_vec()] {
            if let Err(e) = lcd_bulk_out(usb, data) {
                let _ = self.command(&[0x36, 0x03]);
                return Err(e);
            }
        }
        match self.command(&[0x36, 0x02])? {
            0x01 => Ok(()),
            code => {
                let _ = self.command(&[0x36, 0x03]);
                bail!("the cooler did not accept the frame (code {code:#04x})")
            }
        }
    }
}

/// Interface 0 is bound to WinUSB and carries the LCD's bulk data.
pub fn open_lcd_interface(model: &Model) -> Result<nusb::Interface> {
    let info = nusb::list_devices()?
        .find(|d| d.vendor_id() == model.vid && d.product_id() == model.pid)
        .context("the Kraken's USB device is not visible")?;
    let dev = info.open().context(super::usb::OPEN_FAILED)?;
    dev.claim_interface(0).context("cannot claim the LCD interface (is CAM still running?)")
}

impl Driver for KrakenEliteV2 {
    fn describe(&self) -> Device {
        let fan = |id: &str, name: &str, pump: bool| FanChannel {
            id: id.into(),
            name: name.into(),
            pump,
            min_duty: if pump { PUMP_MIN_DUTY } else { 0 },
            device_curves: true,
            ..Default::default()
        };
        Device {
            name: self.model.name.into(),
            firmware: self.firmware.clone(),
            verified: self.model.verified(),
            fans: vec![fan("pump", "Pump", true), fan("fan", "Radiator fans", false)],
            lights: self
                .lights
                .iter()
                .map(|l| LightChannel { id: l.id.into(), name: l.name.into(), leds: l.leds, accessories: l.accessories.clone(), ..Default::default() })
                .collect(),
            lcd: Some(LcdInfo { width: self.lcd_size.0, height: self.lcd_size.1, round: true, frames: true }),
            ..Default::default()
        }
    }

    fn sensors(&self) -> Vec<(&'static str, &'static str, Unit)> {
        vec![("liquid", "Liquid", Unit::Celsius)]
    }

    fn poll(&mut self) -> Result<Reading> {
        hid::drain(&self.hid);
        let msg = hid::request(&self.hid, &[0x74, 0x01], &[0x75])?;
        let s = parse_status(&msg).context("the cooler reported an invalid liquid temperature")?;
        let fan = |rpm, duty| FanReading { rpm: Some(rpm), duty: Some(duty), connected: None };
        Ok(Reading {
            sensors: vec![("liquid", s.liquid_c)],
            fans: vec![("pump", fan(s.pump_rpm, s.pump_duty)), ("fan", fan(s.fan_rpm, s.fan_duty))],
        })
    }

    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
        for &(fan, duty) in duties {
            let pump = fan == "pump";
            hid::write(&self.hid, &curve_report(if pump { &PUMP_CHANNEL } else { &FAN_CHANNEL }, &table(pump, &|_| duty)))?;
        }
        Ok(())
    }

    fn store_curve(&mut self, fan: &str, sensor: Option<&str>, duty_at: &dyn Fn(f32) -> u8) -> Result<bool> {
        if !matches!(sensor, None | Some("liquid")) || !matches!(fan, "pump" | "fan") {
            return Ok(false);
        }
        let pump = fan == "pump";
        hid::write(&self.hid, &curve_report(if pump { &PUMP_CHANNEL } else { &FAN_CHANNEL }, &table(pump, duty_at)))?;
        Ok(true)
    }

    fn set_lighting(&mut self, light: &str, lighting: &Lighting) -> Result<()> {
        let Some(l) = self.lights.iter().find(|l| l.id == light) else {
            let ids: Vec<&str> = self.lights.iter().map(|l| l.id).collect();
            bail!("no light {light:?}; this cooler has {}", if ids.is_empty() { "none".into() } else { ids.join(" and ") })
        };
        hue2::write(&self.hid, l.channel, lighting, l.leds)
    }

    fn lcd(&mut self) -> Option<&mut dyn Lcd> {
        Some(self)
    }
}

impl Lcd for KrakenEliteV2 {
    fn settings(&mut self, brightness: u8, quarter_turns: u8) -> Result<()> {
        hid::write(&self.hid, &[0x30, 0x02, 0x01, brightness.min(100), 0x00, 0x00, 0x01, quarter_turns & 3])
    }

    fn show_builtin(&mut self) -> Result<()> {
        self.streaming = false;
        let _ = self.command(&[0x36, 0x03]);
        match self.command(&[0x38, 0x01, 0x02, 0x00])? {
            0x01 => Ok(()),
            code => bail!("the cooler refused the liquid screen (code {code:#04x})"),
        }
    }

    /// One frame into the streaming buffer, the sequence CAM sends for each Web
    /// Integration frame. The screen switches to the stream when a frame arrives.
    /// Unlike memory buckets this writes no flash and needs no bucket bookkeeping,
    /// which is what wedged coolers in liquidctl#774.
    fn frame(&mut self, q565: &[u8]) -> Result<()> {
        if self.usb.is_none() {
            self.usb = Some(open_lcd_interface(self.model)?);
        }
        if !self.streaming {
            // A transfer left open by an earlier process makes the device refuse new ones.
            let _ = self.command(&[0x36, 0x03]);
        }
        self.push_frame(q565)?;
        self.streaming = true;
        Ok(())
    }

    fn is_streaming(&self) -> bool {
        self.streaming
    }

    fn size(&self) -> (u16, u16) {
        self.lcd_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safety_floor_ramps_between_45_and_55() {
        assert_eq!((safety_floor(20), safety_floor(45), safety_floor(50), safety_floor(55)), (0, 0, 50, 100));
        let t = table(false, &|_| 30);
        assert_eq!((t[0], t[29], t[30], t[34], t[39]), (30, 40, 50, 90, 100));
    }

    #[test]
    fn pump_table_keeps_its_minimum() {
        let msg = curve_report(&PUMP_CHANNEL, &table(true, &|_| 10));
        assert_eq!(&msg[..4], &[0x72, 0x01, 0x01, 0x00]);
        assert_eq!(msg.len(), 4 + CURVE_POINTS);
        assert!(msg[4..30].iter().all(|&d| d == PUMP_MIN_DUTY));
    }

    #[test]
    fn fan_table_header() {
        assert_eq!(&curve_report(&FAN_CHANNEL, &table(false, &|_| 0))[..4], &[0x72, 0x02, 0x01, 0x01]);
    }

    #[test]
    fn parses_status() {
        let mut m = vec![0u8; 64];
        m[0] = 0x75;
        m[1] = 0x01;
        m[15] = 31;
        m[16] = 4;
        m[17..19].copy_from_slice(&2150u16.to_le_bytes());
        m[19] = 61;
        m[23..25].copy_from_slice(&980u16.to_le_bytes());
        m[25] = 35;
        let s = parse_status(&m).unwrap();
        assert!((s.liquid_c - 31.4).abs() < 1e-4);
        assert_eq!((s.pump_rpm, s.pump_duty, s.fan_rpm, s.fan_duty), (2150, 61, 980, 35));
        m[15] = 0xff;
        m[16] = 0xff;
        assert!(parse_status(&m).is_none());
    }

    #[test]
    fn lcd_header_matches_cam_capture() {
        // 12 fa 01 e8 ab cd ef 98 76 54 32 10 08 00 00 00 d9 19 00 00
        let h = lcd_bulk_header(LCD_FORMAT_Q565, 6617);
        assert_eq!(&h[..12], &LCD_BULK_MAGIC);
        assert_eq!(&h[12..], &[0x08, 0, 0, 0, 0xd9, 0x19, 0, 0]);
    }

    #[test]
    fn lcd_size_from_the_info_reply() {
        let mut m = vec![0u8; 64];
        m[..2].copy_from_slice(&[0x31, 0x01]);
        m[20..24].copy_from_slice(&[0xf0, 0x00, 0xf0, 0x00]);
        assert_eq!(parse_lcd_size(&m), Some((240, 240)));
        m[20..24].copy_from_slice(&[0x80, 0x02, 0x80, 0x02]);
        assert_eq!(parse_lcd_size(&m), Some((640, 640)));
        m[20..24].fill(0);
        assert_eq!(parse_lcd_size(&m), None);
        m[0] = 0x75;
        assert_eq!(parse_lcd_size(&m), None);
    }
}
