//! Corsair's iCUE LINK System Hub (`1b1c:0c3f`): a chain of up to 24 LINK fans,
//! pumps, coolers and LED devices on two ports. Another brand's hub: camless opens it
//! only with `[devices] other_brands` on, and leaves it to iCUE while that runs.
//!
//! The hub answers every command with one 512-byte report: the command at byte 2 and
//! its status at 3, 0 when it took it. Commands go out as `00 00 01 <command>` in a
//! 513-byte report on the collection with usage page `FF42`, usage `01`. Its data
//! sits behind endpoints: a read closes, opens, reads and closes one, and the reply
//! carries the endpoint's data type at bytes 4 and 5. A reply of another type is a
//! late one, and up to five more reads find the right one.
//!
//! camless puts the hub in software mode, where it runs every speed channel at the
//! duty it got last; it has no motherboard header to fall back on. So camless sends
//! every channel's duty with every poll, and holds those the config does not set at
//! 50 % for fans and 70 % for pumps. A pump does not go below 50 %, and the H100i to
//! H170i and TITAN II coolers below 70 %. When camless lets go, or the service stops,
//! it puts the hub back in hardware mode, its own curve.
//!
//! Lighting is the LEDs of every device in one stream, in chain order with an LCD
//! cap's LEDs after its cooler's: R, G, B, where a 0 leaves an LED as it was. So a
//! colour goes out with every byte at least 1, and the devices the config does not set
//! keep what they show. Each port takes only so many lit LEDs: past 238, 340 and 442
//! on one, camless dims every colour to 66 %, 33 % and 10 %.
//!
//! Every program with the hub open gets every reply, so a reply to a command camless
//! did not send is another program's (iCUE). camless listens 2.5 seconds before it
//! takes the hub, and lets it go on the first such reply, without the hand-back; it
//! takes the hub again only once a minute has passed without one.
//!
//! camless never sends `1A 01`, which maps the chain again and powers it off and on,
//! or anything to an LCD.
//!
//! Sources: Nexus (`src/Peripherals/CorsairLink`, `src/Cooling/CorsairLinkCoolingProvider.cs`,
//! `src/Lighting/CorsairLinkLightingFrameWriter.cs`), checked against OpenLinkHub and
//! OpenRGB, and run by Nexus on firmware 3.2.571 and 4.1.656. Not verified on hardware.

use super::{hid, static_id, Driver, FanReading, Found, Model, Reading, Usage, Vendor};
use anyhow::{bail, Context, Result};
use camless_core::lighting::{Lighting, Rgb};
use camless_core::status::{Device, FanChannel, LightChannel, Unit};
use hidapi::{HidApi, HidDevice};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const CORSAIR: Vendor = Vendor {
    name: "Corsair",
    app: "iCUE",
    // Nexus's list, as executables.
    processes: &[
        "icue.exe",
        "icue launcher.exe",
        "icuelauncher.exe",
        "icuedevicepluginhost.exe",
        "corsair.service.exe",
        "corsair.service.cpuldremote64.exe",
        "corsair.service.displayadapter.exe",
        "corsairdevicecontrolservice.exe",
        "cuellaccessservice.exe",
        "corsairservice.exe",
        "corsairlink4.exe",
    ],
};

pub const MODEL: Model = Model::vendor_hid(
    0x1b1c,
    0x0c3f,
    "corsair-icue-link",
    "Corsair iCUE LINK System Hub",
    &CORSAIR,
    Usage::Exact(0xff42, 0x01),
    open,
);

const REPORT_LEN: usize = 512;
const WRITE_LEN: usize = REPORT_LEN + 1;
const HEADER: usize = 3;
/// Colour bytes per report, after the header and the two command bytes.
const COLOR_CHUNK: usize = WRITE_LEN - HEADER - 2;
const MAX_CHANNELS: usize = 24;
const CHANNEL_SLOTS: usize = MAX_CHANNELS + 1;
/// Variable LED counts are capped at this, as OpenLinkHub does.
const MAX_DYNAMIC_LEDS: usize = 50;

const READ_TIMEOUT: Duration = Duration::from_millis(250);
const RESYNC_TIMEOUT: Duration = Duration::from_millis(50);
const RESYNC_TRIES: usize = 5;
const DRAIN_READS: usize = 8;
/// The hub sends nothing unasked, so a report in this time answers another program.
const LISTEN: Duration = Duration::from_millis(2500);
/// After another program was heard, the hub must stay quiet this long before
/// camless takes it: longer than iCUE's keep-alive ping.
const QUIET_AFTER_FOREIGN: Duration = Duration::from_secs(60);
/// The firmware takes no command for this long after software mode.
const SOFTWARE_MODE_SETTLE: Duration = Duration::from_millis(500);
/// Chains of QX and RX fans drop the QX lighting without this after the colour
/// endpoint opens.
const COLOR_SETTLE: Duration = Duration::from_millis(40);

const GET_FIRMWARE: &[u8] = &[0x02, 0x13];
const SOFTWARE_MODE: &[u8] = &[0x01, 0x03, 0x00, 0x02];
const HARDWARE_MODE: &[u8] = &[0x01, 0x03, 0x00, 0x01];
const OPEN: &[u8] = &[0x0d, 0x01];
const OPEN_COLOR: &[u8] = &[0x0d, 0x00];
const CLOSE: &[u8] = &[0x05, 0x01, 0x01];
const CLOSE_COLOR: &[u8] = &[0x05, 0x01, 0x00];
const WRITE: &[u8] = &[0x06, 0x01];
const WRITE_COLOR: &[u8] = &[0x06, 0x00];
const WRITE_MORE_COLOR: &[u8] = &[0x07, 0x00];
const READ: &[u8] = &[0x08, 0x01];

const DEVICES: u8 = 0x36;
const LEDS: u8 = 0x20;
const TEMPERATURES: u8 = 0x21;
const SPEEDS: u8 = 0x17;
const SET_SPEED: u8 = 0x18;
const SET_COLOR: u8 = 0x22;
const DEVICES_DATA: [u8; 2] = [0x21, 0x00];
const TEMPERATURES_DATA: [u8; 2] = [0x10, 0x00];
const SPEEDS_DATA: [u8; 2] = [0x25, 0x00];
const SET_SPEED_DATA: [u8; 2] = [0x07, 0x00];
const SET_COLOR_DATA: [u8; 2] = [0x12, 0x00];

const FAN_DEFAULT: u8 = 50;
const PUMP_DEFAULT: u8 = 70;
const PUMP_FLOOR: u8 = 50;
const AIO_FLOOR: u8 = 70;
/// The TITAN coolers: Nexus floors their curves at 31 %, under the hand floor.
const TITAN: u8 = 17;
const LCD_CAP: u8 = 6;
const PUMP_LCD: u8 = 14;
/// Type 26 is a family of unrelated products.
const FAMILY: u8 = 26;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Other,
    Fan,
    Aio,
    Pump,
    Block,
    Case,
    Adapter,
}

/// `CorsairLinkModels`: name, LEDs (0 when the hub counts them), class, a speed
/// channel, a temperature probe.
fn lookup(kind: u8, model: u8) -> Option<(&'static str, usize, Class, bool, bool)> {
    use Class::*;
    let m = match (kind, model) {
        (1, 0) => ("iCUE LINK QX RGB", 34, Fan, true, true),
        (2, 0) => ("iCUE LINK LX RGB", 18, Fan, true, false),
        (3, 0) => ("iCUE LINK RX RGB MAX", 8, Fan, true, false),
        (4, 0) => ("iCUE LINK RX MAX", 0, Fan, true, false),
        (5, 0) => ("iCUE LINK Adapter", 0, Adapter, false, false),
        (5, 1) => ("iCUE LINK 9000D Airflow", 22, Case, false, false),
        (5, 2) => ("iCUE LINK 5000T", 160, Case, false, false),
        (6, 0) => ("iCUE LINK LCD Cap", 24, Pump, false, false),
        (7, 0) => ("iCUE LINK H100i", 20, Aio, true, true),
        (7, 1) => ("iCUE LINK H115i", 20, Aio, true, true),
        (7, 2) => ("iCUE LINK H150i", 20, Aio, true, true),
        (7, 3) => ("iCUE LINK H170i", 20, Aio, true, true),
        (9, 0 | 1) => ("iCUE LINK XC7 Elite", 24, Block, false, true),
        (10, 0) => ("iCUE LINK XG3 Hybrid", 22, Block, true, false),
        (12, 0) => ("iCUE LINK XD5 Elite", 22, Pump, true, true),
        (13, 0) => ("iCUE LINK XG7 RGB", 16, Block, false, false),
        (14, 0) => ("iCUE LINK XD5 Elite LCD", 22, Pump, true, true),
        (15, 0) => ("iCUE LINK RX RGB", 8, Fan, true, false),
        (16, 0) => ("VRM Cooler Module", 0, Fan, true, false),
        (17, 0 | 4) => ("iCUE LINK TITAN 240", 20, Aio, true, true),
        (17, 1) => ("iCUE LINK TITAN 280", 20, Aio, true, true),
        (17, 2 | 5) => ("iCUE LINK TITAN 360", 20, Aio, true, true),
        (17, 3) => ("iCUE LINK TITAN 420", 20, Aio, true, true),
        (19, 0) => ("iCUE LINK RX", 0, Fan, true, false),
        (25, 0) => ("iCUE LINK XD6 Elite", 22, Pump, true, true),
        (26, 0) => ("iCUE LINK RX360 II RGB", 24, Fan, true, true),
        (26, 1) => ("iCUE LINK TITAN II", 20, Aio, true, true),
        (26, 8) => ("iCUE LINK 5\" LCD Screen Module", 32, Fan, true, true),
        (27, 0) => ("iCUE Commander Duo", 0, Adapter, true, true),
        _ if kind != FAMILY && model != 0 => return lookup(kind, 0),
        _ => return None,
    };
    Some(m)
}

/// A device on the chain.
#[derive(Clone, Debug, PartialEq)]
struct Linked {
    /// Its place on the chain, from 1: how the hub addresses it.
    channel: u8,
    kind: u8,
    model: u8,
    name: String,
    class: Class,
    leds: usize,
    speed: bool,
    temperature: bool,
}

impl Linked {
    fn new(channel: u8, kind: u8, model: u8, dynamic_leds: usize) -> Linked {
        let (name, leds, class, speed, temperature) = match lookup(kind, model) {
            Some((name, leds, class, speed, temperature)) => (name.to_string(), leds, class, speed, temperature),
            None => (format!("iCUE LINK device {kind}.{model}"), 0, Class::Other, false, false),
        };
        let leds = if leds > 0 { leds } else { dynamic_leds };
        Linked { channel, kind, model, name, class, leds, speed, temperature }
    }

    fn pump(&self) -> bool {
        matches!(self.class, Class::Aio | Class::Pump)
    }

    /// Nexus's floors for a duty set by hand, and for a curve's.
    fn floor(&self) -> u8 {
        match self.class {
            Class::Aio if self.kind != TITAN => AIO_FLOOR,
            _ if self.pump() => PUMP_FLOOR,
            _ => 0,
        }
    }

    fn id(&self) -> String {
        format!("ch{}", self.channel)
    }

    /// The hub's port: from channel 13 on firmware 2 and later, from 7 before.
    fn port(&self, firmware_major: u8) -> usize {
        usize::from(self.channel >= 13 || (firmware_major < 2 && self.channel >= 7))
    }
}

/// `ParseDevices`: the channel count at 6, then per channel an 8-byte record, type
/// at 2, model at 3, and the length of the serial number after it at 7; an empty
/// channel has length 0.
fn parse_devices(reply: &[u8]) -> Vec<(u8, u8, u8)> {
    let mut out = Vec::new();
    let Some(&channels) = reply.get(6) else { return out };
    let data = &reply[7..];
    let mut pos = 0;
    for channel in 1..=channels.min(MAX_CHANNELS as u8) {
        let Some(record) = data.get(pos..pos + 8) else { break };
        let serial_len = usize::from(record[7]);
        if serial_len > 0 {
            out.push((channel, record[2], record[3]));
        }
        pos += 8 + serial_len;
    }
    out
}

/// `ParseSpeeds` and `ParseTemperatures`: the count at 6, then per channel from 0
/// a status, 0 for a reading, and a little-endian value.
fn parse_values(reply: &[u8]) -> [Option<i16>; CHANNEL_SLOTS] {
    let mut out = [None; CHANNEL_SLOTS];
    let Some(&count) = reply.get(6) else { return out };
    for (i, slot) in out.iter_mut().enumerate().take(usize::from(count)) {
        let Some(v) = reply.get(7 + i * 3..7 + i * 3 + 3) else { break };
        *slot = (v[0] == 0).then(|| i16::from_le_bytes([v[1], v[2]]));
    }
    out
}

/// `ParseLeds`: per channel from 1, at 7 + 4 × channel, 2 for connected and the
/// LED count.
fn parse_leds(reply: &[u8]) -> [usize; CHANNEL_SLOTS] {
    let mut out = [0; CHANNEL_SLOTS];
    let Some(&channels) = reply.get(6) else { return out };
    for (i, slot) in out.iter_mut().enumerate().take(usize::from(channels) + 1).skip(1) {
        let Some(v) = reply.get(7 + i * 4..7 + i * 4 + 4) else { break };
        if u16::from_le_bytes([v[0], v[1]]) == 2 {
            *slot = usize::from(u16::from_le_bytes([v[2], v[3]])).min(MAX_DYNAMIC_LEDS);
        }
    }
    out
}

/// What an endpoint write carries: its length from the data type on, the type, and
/// the data.
fn inner(data_type: [u8; 2], data: &[u8]) -> Vec<u8> {
    let len = (data.len() + 2) as u16;
    let mut out = Vec::with_capacity(6 + data.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&data_type);
    out.extend_from_slice(data);
    out
}

/// Every speed channel's duty in one write: the count, then channel, 0, duty, 0.
fn speeds(duties: &[(u8, u8)]) -> Vec<u8> {
    let mut data = vec![duties.len().min(MAX_CHANNELS) as u8];
    for &(channel, duty) in duties.iter().take(MAX_CHANNELS) {
        data.extend_from_slice(&[channel, 0, duty.min(100), 0]);
    }
    inner(SET_SPEED_DATA, &data)
}

/// The order of the colour stream: chain order, but the first LCD cap's LEDs after
/// the first cooler's.
fn wire_order(devices: &[Linked]) -> Vec<&Linked> {
    let cap = devices.iter().find(|d| d.kind == LCD_CAP);
    let has_aio = devices.iter().any(|d| d.class == Class::Aio);
    let (Some(cap), true) = (cap, has_aio) else { return devices.iter().collect() };
    let mut out = Vec::with_capacity(devices.len());
    let mut placed = false;
    for d in devices.iter().filter(|d| !std::ptr::eq(*d, cap)) {
        out.push(d);
        if !placed && d.class == Class::Aio {
            out.push(cap);
            placed = true;
        }
    }
    out
}

/// The dimming that keeps the busier port within its power.
fn port_cap(devices: &[Linked], firmware_major: u8) -> f32 {
    let mut ports = [0usize; 2];
    for d in devices {
        ports[d.port(firmware_major)] += d.leds;
    }
    let stage = |leds: usize| -> f32 {
        match leds {
            443.. => 0.10,
            341.. => 0.33,
            239.. => 0.66,
            _ => 1.0,
        }
    };
    stage(ports[0]).min(stage(ports[1]))
}

/// The colour stream for the colours set by channel: zeros for the devices without
/// one, which keep what they show. A cooler's inner ring, LEDs 16 to 19, goes dark
/// when the chain has a pump LCD, as Nexus does.
fn color_stream(devices: &[Linked], colors: &HashMap<u8, Rgb>, firmware_major: u8) -> Vec<u8> {
    let dim = port_cap(devices, firmware_major);
    let has_lcd = devices.iter().any(|d| matches!(d.kind, LCD_CAP | PUMP_LCD));
    let mut out = Vec::new();
    for d in wire_order(devices) {
        let Some(&color) = colors.get(&d.channel) else {
            out.resize(out.len() + d.leds * 3, 0);
            continue;
        };
        let lit = color.map(|c| (f32::from(c) * dim).round() as u8);
        for led in 0..d.leds {
            let c = if d.class == Class::Aio && has_lcd && (16..20).contains(&led) { [0; 3] } else { lit };
            out.extend(c.map(|b| b.max(1)));
        }
    }
    out
}

pub struct Link {
    dev: HidDevice,
    reply: [u8; WRITE_LEN],
    firmware: String,
    firmware_major: u8,
    devices: Vec<Linked>,
    /// Duties by channel, as set, after the floors.
    duties: HashMap<u8, u8>,
    colors: HashMap<u8, Rgb>,
    /// The hub is in software mode, camless's.
    software: bool,
    /// The colour endpoint is open, after the first commands.
    primed: bool,
    /// The hub refused a colour write; its colour endpoint opens again first.
    reopen_color: bool,
    /// Another program answered on the hub.
    foreign: bool,
}

/// When another program was last heard on the hub, across opens.
static FOREIGN_HEARD: Mutex<Option<Instant>> = Mutex::new(None);

fn heard_foreign() {
    *FOREIGN_HEARD.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
}

fn open(api: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    let heard = *FOREIGN_HEARD.lock().unwrap_or_else(|e| e.into_inner());
    if heard.is_some_and(|t| t.elapsed() < QUIET_AFTER_FOREIGN) {
        bail!("another program talked to the hub (iCUE?) less than a minute ago; camless leaves it alone");
    }
    // Unread, so that the listen hears a reply already queued.
    let dev = hid::open_unread(api, &found.path)?;
    let mut buf = [0u8; WRITE_LEN];
    if dev.read_timeout(&mut buf, LISTEN.as_millis() as i32).context(hid::HELD)? > 0 {
        heard_foreign();
        bail!("another program talks to the hub (iCUE?); camless leaves it alone");
    }
    let mut link = Link {
        dev,
        reply: [0; WRITE_LEN],
        firmware: String::new(),
        firmware_major: 0,
        devices: Vec::new(),
        duties: HashMap::new(),
        colors: HashMap::new(),
        software: false,
        primed: false,
        reopen_color: false,
        foreign: false,
    };
    if let Err(e) = link.start() {
        let _ = link.hand_back();
        return Err(e);
    }
    Ok(Box::new(link))
}

impl Link {
    fn start(&mut self) -> Result<()> {
        if self.transfer(GET_FIRMWARE, &[])? >= 8 {
            let r = &self.reply;
            self.firmware = format!("{}.{}.{}", r[4], r[5], u16::from_le_bytes([r[6], r[7]]));
            self.firmware_major = r[4];
        }
        self.transfer(SOFTWARE_MODE, &[])?;
        self.software = true;
        std::thread::sleep(SOFTWARE_MODE_SETTLE);
        self.devices = self.chain()?;
        if !self.open_color()? {
            bail!("the hub refused to open its colour endpoint");
        }
        std::thread::sleep(COLOR_SETTLE);
        self.primed = true;
        // Software mode holds what the hub got last: give every channel its duty now.
        self.send_duties()
    }

    /// Writes a command and reads its reply into `reply`, without the report id:
    /// its length, 0 if none came.
    fn transfer(&mut self, cmd: &[u8], payload: &[u8]) -> Result<usize> {
        if self.foreign {
            bail!("another program answered on the hub (iCUE?); camless leaves it alone");
        }
        let mut w = [0u8; WRITE_LEN];
        w[2] = 0x01;
        w[HEADER..HEADER + cmd.len()].copy_from_slice(cmd);
        w[HEADER + cmd.len()..HEADER + cmd.len() + payload.len()].copy_from_slice(payload);
        self.dev.write(&w).context("write to the hub failed")?;
        let n = self.dev.read_timeout(&mut self.reply, READ_TIMEOUT.as_millis() as i32)?;
        if n > 0 {
            self.note_echo(cmd[0]);
        }
        Ok(n)
    }

    /// A read without a command, for a reply that came late.
    fn read_again(&mut self) -> Result<usize> {
        let n = self.dev.read_timeout(&mut self.reply, RESYNC_TIMEOUT.as_millis() as i32)?;
        if n > 0 {
            self.note_echo(0);
        }
        Ok(n)
    }

    /// Every program with the hub open gets every reply. camless sends only these
    /// commands, and the firmware request before the colour endpoint opens.
    fn note_echo(&mut self, sent: u8) {
        let echo = self.reply[2];
        let own = matches!(echo, 0x01 | 0x05 | 0x06 | 0x07 | 0x08 | 0x0d) || (echo == 0x02 && !self.primed);
        if echo != 0 && echo != sent && !own {
            self.foreign = true;
            heard_foreign();
        }
    }

    fn refused(&self, cmd: &[u8]) -> bool {
        self.reply[2] == cmd[0] && self.reply[3] != 0
    }

    /// Reads until the reply has the data type, five times at most.
    fn resync(&mut self, mut n: usize, data_type: [u8; 2]) -> Result<usize> {
        for _ in 0..RESYNC_TRIES {
            if n == 0 || self.reply[4..6] == data_type {
                break;
            }
            n = self.read_again()?;
        }
        Ok(n)
    }

    /// An endpoint's reply, if it came, of its data type when one is given.
    fn read_endpoint(&mut self, endpoint: u8, data_type: Option<[u8; 2]>) -> Result<Option<Vec<u8>>> {
        self.transfer(CLOSE, &[endpoint])?;
        self.transfer(OPEN, &[endpoint])?;
        let mut n = self.transfer(READ, &[endpoint])?;
        if let Some(t) = data_type {
            n = self.resync(n, t)?;
        }
        let ok = n > 0 && data_type.is_none_or(|t| self.reply[4..6] == t);
        let reply = ok.then(|| self.reply[..REPORT_LEN].to_vec());
        self.transfer(CLOSE, &[endpoint])?;
        Ok(reply)
    }

    /// The hub applies a write on delivery and its reply proves nothing.
    fn write_endpoint(&mut self, endpoint: u8, data_type: [u8; 2], inner: &[u8]) -> Result<()> {
        self.transfer(CLOSE, &[endpoint])?;
        self.transfer(OPEN, &[endpoint])?;
        let n = self.transfer(WRITE, inner)?;
        self.resync(n, data_type)?;
        self.transfer(CLOSE, &[endpoint])?;
        Ok(())
    }

    /// The chain, with each device's LED count; speeds and temperatures are polled.
    fn chain(&mut self) -> Result<Vec<Linked>> {
        let devices = self.read_endpoint(DEVICES, Some(DEVICES_DATA))?.context("the hub sent no device list")?;
        let leds = self.read_endpoint(LEDS, None)?.map(|r| parse_leds(&r)).unwrap_or([0; CHANNEL_SLOTS]);
        Ok(parse_devices(&devices)
            .into_iter()
            .map(|(ch, kind, model)| Linked::new(ch, kind, model, leds.get(usize::from(ch)).copied().unwrap_or(0)))
            .collect())
    }

    /// Frees the colour endpoint, which another program may have left on another
    /// resource, and opens it on colours.
    fn open_color(&mut self) -> Result<bool> {
        self.transfer(CLOSE_COLOR, &[])?;
        let n = self.transfer(OPEN_COLOR, &[SET_COLOR])?;
        Ok(n > 3 && !self.refused(OPEN_COLOR))
    }

    fn send_colors(&mut self) -> Result<()> {
        if self.colors.is_empty() {
            return Ok(());
        }
        if self.reopen_color {
            if !self.open_color()? {
                bail!("the hub refused to open its colour endpoint again");
            }
            self.reopen_color = false;
        }
        let stream = inner(SET_COLOR_DATA, &color_stream(&self.devices, &self.colors, self.firmware_major));
        for (i, chunk) in stream.chunks(COLOR_CHUNK).enumerate() {
            let cmd = if i == 0 { WRITE_COLOR } else { WRITE_MORE_COLOR };
            if self.transfer(cmd, chunk)? == 0 {
                bail!("the hub did not answer a colour write");
            }
            if self.refused(cmd) {
                self.reopen_color = true;
                bail!("the hub refused a colour write");
            }
        }
        Ok(())
    }

    /// Every speed channel's duty: the one set, or the default.
    fn send_duties(&mut self) -> Result<()> {
        let duties: Vec<(u8, u8)> = self
            .devices
            .iter()
            .filter(|d| d.speed)
            .map(|d| {
                let default = if d.pump() { PUMP_DEFAULT } else { FAN_DEFAULT };
                (d.channel, self.duties.get(&d.channel).copied().unwrap_or(default))
            })
            .collect();
        if duties.is_empty() {
            return Ok(());
        }
        self.write_endpoint(SET_SPEED, SET_SPEED_DATA, &speeds(&duties))
    }

    /// Hardware mode, unless another program took the hub, and the replies still
    /// queued, which the next open would hear as another program's.
    fn hand_back(&mut self) -> Result<()> {
        let mut result = Ok(());
        if std::mem::take(&mut self.software) && !self.foreign {
            result = self.transfer(HARDWARE_MODE, &[]).map(drop);
        }
        for _ in 0..DRAIN_READS {
            if !matches!(self.read_again(), Ok(1..)) {
                break;
            }
        }
        result
    }

    fn device(&self, id: &str) -> Option<&Linked> {
        let channel: u8 = id.strip_prefix("ch")?.parse().ok()?;
        self.devices.iter().find(|d| d.channel == channel)
    }
}

impl Driver for Link {
    fn describe(&self) -> Device {
        Device {
            name: MODEL.name.into(),
            firmware: (!self.firmware.is_empty()).then(|| self.firmware.clone()),
            verified: MODEL.verified(),
            experimental: MODEL.is_experimental(),
            fans: self
                .devices
                .iter()
                .filter(|d| d.speed)
                .map(|d| FanChannel {
                    id: d.id(),
                    name: format!("{}, channel {}", d.name, d.channel),
                    pump: d.pump(),
                    min_duty: d.floor(),
                    ..Default::default()
                })
                .collect(),
            lights: self
                .devices
                .iter()
                .filter(|d| d.leds > 0)
                .map(|d| LightChannel {
                    id: d.id(),
                    name: format!("{}, channel {}", d.name, d.channel),
                    leds: d.leds as u16,
                    modes: vec!["off".into(), "fixed".into()],
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    fn sensors(&self) -> Vec<(&'static str, &'static str, Unit)> {
        self.devices
            .iter()
            .filter(|d| d.temperature)
            .map(|d| {
                let id = static_id(&format!("ch{}-temp", d.channel));
                (id, static_id(&format!("{} probe, channel {}", d.name, d.channel)), Unit::Celsius)
            })
            .collect()
    }

    fn poll(&mut self) -> Result<Reading> {
        let chain = self.chain()?;
        let same = |a: &Linked, b: &Linked| (a.channel, a.kind, a.model) == (b.channel, b.kind, b.model);
        if chain.len() != self.devices.len() || !chain.iter().zip(&self.devices).all(|(a, b)| same(a, b)) {
            bail!("the chain changed; camless opens the hub again");
        }
        let rpm = self.read_endpoint(SPEEDS, Some(SPEEDS_DATA))?.map(|r| parse_values(&r));
        let temps = self.read_endpoint(TEMPERATURES, Some(TEMPERATURES_DATA))?.map(|r| parse_values(&r));
        // In software mode the hub holds the last duties; Nexus sends them every poll.
        self.send_duties()?;
        self.send_colors()?;

        let slot = |values: &Option<[Option<i16>; CHANNEL_SLOTS]>, ch: u8| values.and_then(|v| v.get(usize::from(ch)).copied().flatten());
        let mut reading = Reading::default();
        for d in &self.devices {
            if d.temperature {
                if let Some(t) = slot(&temps, d.channel) {
                    reading.sensors.push((static_id(&format!("ch{}-temp", d.channel)), f32::from(t) / 10.0));
                }
            }
            if d.speed {
                let fan = FanReading {
                    rpm: slot(&rpm, d.channel).map(|r| r as u16),
                    duty: self.duties.get(&d.channel).copied(),
                    connected: None,
                };
                reading.fans.push((static_id(&d.id()), fan));
            }
        }
        Ok(reading)
    }

    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
        for &(id, duty) in duties {
            let d = self.device(id).filter(|d| d.speed).with_context(|| format!("no fan or pump {id:?} on the chain"))?;
            let (channel, floor) = (d.channel, d.floor());
            self.duties.insert(channel, duty.min(100).max(floor));
        }
        self.send_duties()
    }

    fn set_lighting(&mut self, light: &str, lighting: &Lighting) -> Result<()> {
        let d = self.device(light).filter(|d| d.leds > 0).with_context(|| format!("no light {light:?} on the chain"))?;
        let channel = d.channel;
        let color = match lighting {
            Lighting::Off => [0, 0, 0],
            Lighting::Fixed(c) => *c,
            Lighting::Effect { .. } => bail!("camless sets iCUE LINK devices off or to one colour"),
        };
        self.colors.insert(channel, color);
        self.send_colors()
    }

    fn release(&mut self) -> Result<()> {
        self.duties.clear();
        self.colors.clear();
        self.hand_back()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(data_type: [u8; 2], count: u8, data: &[u8]) -> Vec<u8> {
        let mut r = vec![0u8; REPORT_LEN];
        r[2] = READ[0];
        r[4..6].copy_from_slice(&data_type);
        r[6] = count;
        r[7..7 + data.len()].copy_from_slice(data);
        r
    }

    #[test]
    fn the_device_list_skips_empty_channels() {
        // Channel 1: a QX fan with a 3-byte serial; 2: empty; 3: an H150i, no serial byte
        // past the reply is read.
        let mut data = vec![0, 0, 1, 0, 0, 0, 0, 3, b'A', b'B', b'C'];
        data.extend_from_slice(&[0; 8]);
        data.extend_from_slice(&[0, 0, 7, 2, 0, 0, 0, 2, b'X', b'Y']);
        let r = reply(DEVICES_DATA, 3, &data);
        assert_eq!(parse_devices(&r), [(1, 1, 0), (3, 7, 2)]);
        assert_eq!(parse_devices(&r[..6]), []);
    }

    #[test]
    fn the_device_list_stops_at_the_last_channel() {
        // A count past 24 would index past every per-channel table.
        let data: Vec<u8> = (0..40).flat_map(|_| [0, 0, 1, 0, 0, 0, 0, 1, b'A']).collect();
        let r = reply(DEVICES_DATA, 40, &data);
        let devices = parse_devices(&r);
        assert_eq!(devices.len(), MAX_CHANNELS);
        assert_eq!(devices.last().map(|d| d.0), Some(MAX_CHANNELS as u8));
    }

    #[test]
    fn speeds_temperatures_and_leds_come_by_channel() {
        // Slot 0 is the hub's; slot 1 reads 1200 rpm; slot 2 has no reading.
        let r = reply(SPEEDS_DATA, 3, &[0, 0, 0, 0, 0xb0, 0x04, 1, 0xff, 0xff]);
        let v = parse_values(&r);
        assert_eq!(&v[..4], [Some(0), Some(1200), None, None]);
        // -1.5 °C in tenths.
        let r = reply(TEMPERATURES_DATA, 2, &[1, 0, 0, 0, 0xf1, 0xff]);
        assert_eq!(parse_values(&r)[1], Some(-15));
        // Channel 1 connected with 80 LEDs, capped at 50; channel 2 not connected.
        let mut data = vec![0u8; 12];
        data[4..8].copy_from_slice(&[2, 0, 80, 0]);
        data[8..12].copy_from_slice(&[1, 0, 10, 0]);
        let leds = parse_leds(&reply([0, 0], 2, &data));
        assert_eq!(&leds[..3], [0, 50, 0]);
    }

    #[test]
    fn models_have_names_floors_and_their_led_counts() {
        let qx = Linked::new(1, 1, 0, 0);
        assert_eq!((qx.name.as_str(), qx.leds, qx.pump(), qx.floor()), ("iCUE LINK QX RGB", 34, false, 0));
        assert_eq!(Linked::new(2, 7, 2, 0).floor(), 70);
        // A TITAN II is an AIO too; a TITAN and a standalone pump take the hand floor.
        assert_eq!(Linked::new(2, 26, 1, 0).floor(), 70);
        assert_eq!(Linked::new(2, 17, 2, 0).floor(), 50);
        assert_eq!(Linked::new(2, 12, 0, 0).floor(), 50);
        // An adapter's LEDs come from the hub.
        assert_eq!(Linked::new(3, 5, 0, 30).leds, 30);
        // A new revision of a known type is named as its type; not so type 26.
        assert_eq!(Linked::new(4, 2, 9, 0).name, "iCUE LINK LX RGB");
        let unknown = Linked::new(5, 26, 9, 0);
        assert_eq!((unknown.name.as_str(), unknown.speed), ("iCUE LINK device 26.9", false));
        // Port 1 from channel 13, or 7 on firmware 1.
        assert_eq!((Linked::new(12, 1, 0, 0).port(3), Linked::new(13, 1, 0, 0).port(3)), (0, 1));
        assert_eq!(Linked::new(7, 1, 0, 0).port(1), 1);
    }

    #[test]
    fn duties_go_in_one_write() {
        let w = speeds(&[(1, 40), (3, 150)]);
        // Length 1 + 2 × 4 + 2 = 11.
        assert_eq!(w, [11, 0, 0, 0, 0x07, 0x00, 2, 1, 0, 40, 0, 3, 0, 100, 0]);
    }

    #[test]
    fn colours_go_in_chain_order_with_the_cap_after_its_cooler() {
        let devices = vec![Linked::new(1, 7, 2, 0), Linked::new(2, 1, 0, 0), Linked::new(3, 6, 0, 0)];
        let order: Vec<_> = wire_order(&devices).iter().map(|d| d.channel).collect();
        assert_eq!(order, [1, 3, 2]);
        let mut colors = HashMap::new();
        colors.insert(1, [0x10, 0, 0x30]);
        colors.insert(3, [0, 0, 0]);
        let s = color_stream(&devices, &colors, 3);
        assert_eq!(s.len(), (20 + 24 + 34) * 3);
        // A zero byte of a set colour goes out as 1; the cooler's inner ring goes dark.
        assert_eq!(&s[..3], [0x10, 1, 0x30]);
        assert_eq!(&s[16 * 3..20 * 3], [1; 12]);
        // The cap, off; the QX fan, not set, keeps what it shows.
        assert_eq!(&s[20 * 3..44 * 3], [1; 72]);
        assert!(s[44 * 3..].iter().all(|&b| b == 0));
        // The endpoint write: length, type, then the stream.
        let w = inner(SET_COLOR_DATA, &s);
        assert_eq!(&w[..6], [((s.len() + 2) & 0xff) as u8, ((s.len() + 2) >> 8) as u8, 0, 0, 0x12, 0x00]);
    }

    #[test]
    fn a_busy_port_dims_the_colours() {
        // 8 QX fans on port 0: 272 LEDs, past 238.
        let devices: Vec<_> = (1..=8).map(|ch| Linked::new(ch, 1, 0, 0)).collect();
        assert_eq!(port_cap(&devices, 3), 0.66);
        let colors = HashMap::from([(1, [200, 100, 0])]);
        assert_eq!(&color_stream(&devices, &colors, 3)[..3], [132, 66, 1]);
        assert_eq!(port_cap(&devices[..7], 3), 1.0);
        // On firmware 1, channel 7 on is port 1: 6 and 2 fans.
        assert_eq!(port_cap(&devices, 1), 1.0);
        // Three 5000T cases: 480 LEDs.
        let devices: Vec<_> = (1..=3).map(|ch| Linked::new(ch, 5, 2, 0)).collect();
        assert_eq!(port_cap(&devices, 3), 0.10);
    }
}
