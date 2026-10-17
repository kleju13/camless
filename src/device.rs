//! Open handles to the Kraken Elite V2 and the Control Hub.

use crate::protocol::*;
use anyhow::{bail, Context, Result};
use hidapi::{HidApi, HidDevice};
use std::time::{Duration, Instant};

/// The devices answer in a few milliseconds; status broadcasts arrive in between.
const REPLY_TIMEOUT: Duration = Duration::from_secs(3);
/// LCD commands run inside the cooling loop, so a cooler that stops answering them
/// must not hold the fans up for long. CAM sees these replies within 5 ms.
const LCD_REPLY_TIMEOUT: Duration = Duration::from_millis(500);

fn open_hid(api: &HidApi, pid: u16) -> Result<HidDevice> {
    let info = api
        .device_list()
        .find(|d| d.vendor_id() == NZXT_VID && d.product_id() == pid)
        .with_context(|| format!("device {NZXT_VID:04x}:{pid:04x} is not connected"))?;
    let dev = info.open_device(api)?;
    // An exclusive holder (NZXT CAM's service) makes the open succeed without access
    // rights, and only the first read fails. Find out now.
    let mut buf = [0u8; REPORT_LEN];
    dev.read_timeout(&mut buf, 0).with_context(|| {
        format!("{NZXT_VID:04x}:{pid:04x} is held by another program; close NZXT CAM and stop its CAM Service first")
    })?;
    Ok(dev)
}

/// The report descriptors declare 512-byte output reports.
const MAX_REPORT_LEN: usize = 512;

/// HID reports on these devices are numbered and the report ID is the first command
/// byte, so payloads go out as they are, padded to at least 64 bytes like liquidctl
/// does. A static colour (4 + 24 × 3 bytes) is longer than that, as in liquidctl.
fn write(dev: &HidDevice, payload: &[u8]) -> Result<()> {
    if payload.len() > MAX_REPORT_LEN {
        bail!("report of {} bytes does not fit", payload.len());
    }
    let mut buf = vec![0u8; payload.len().max(REPORT_LEN)];
    buf[..payload.len()].copy_from_slice(payload);
    dev.write(&buf)?;
    Ok(())
}

/// Reads until a report starting with `prefix` arrives, skipping periodic broadcasts.
fn read_prefix(dev: &HidDevice, prefix: &[u8]) -> Result<Vec<u8>> {
    read_prefix_within(dev, prefix, REPLY_TIMEOUT)
}

fn read_prefix_within(dev: &HidDevice, prefix: &[u8], max: Duration) -> Result<Vec<u8>> {
    let mut buf = [0u8; REPORT_LEN];
    let end = Instant::now() + max;
    loop {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            bail!("no reply starting with {prefix:02x?} within {} ms", max.as_millis());
        }
        let n = dev.read_timeout(&mut buf, left.as_millis().max(1) as i32)?;
        if n >= prefix.len() && buf[..prefix.len()] == *prefix {
            return Ok(buf[..n].to_vec());
        }
    }
}

fn drain(dev: &HidDevice) {
    let mut buf = [0u8; REPORT_LEN];
    while matches!(dev.read_timeout(&mut buf, 0), Ok(n) if n > 0) {}
}

#[derive(Clone, Debug)]
pub enum Lighting {
    Off,
    Fixed(Rgb),
    Effect { effect: Effect, colors: Vec<Rgb>, speed: Speed, backward: bool },
}

impl Lighting {
    fn packets(&self, channel: u8) -> Vec<Vec<u8>> {
        match self {
            Lighting::Off => static_color(channel, &[0, 0, 0]),
            Lighting::Fixed(c) => static_color(channel, c),
            Lighting::Effect { effect: e, colors, speed, backward } => {
                vec![effect(channel, *e, colors, *speed, *backward)]
            }
        }
    }
}

/// The ring throttles commands: more than about four packets in half a second are
/// dropped, not queued (camera measurements in liquidctl#882). Space them out.
fn write_lighting(dev: &HidDevice, channel: u8, lighting: &Lighting) -> Result<()> {
    for p in lighting.packets(channel) {
        write(dev, &p)?;
        std::thread::sleep(Duration::from_millis(150));
    }
    Ok(())
}

pub struct Kraken {
    hid: HidDevice,
    lcd: Option<nusb::Interface>,
    streaming: bool,
    pub firmware: Option<(u8, u8, u8)>,
    pub lcd_info: Option<LcdInfo>,
}

impl Kraken {
    pub fn open(api: &HidApi) -> Result<Kraken> {
        Ok(Kraken { hid: open_hid(api, KRAKEN_ELITE_V2_PID)?, lcd: None, streaming: false, firmware: None, lcd_info: None })
    }

    /// Needed after every power-on and resume, before the device reports status.
    pub fn initialize(&mut self) -> Result<()> {
        drain(&self.hid);
        for p in kraken_init() {
            write(&self.hid, &p)?;
        }
        write(&self.hid, &kraken_firmware_request())?;
        self.firmware = parse_kraken_firmware(&read_prefix(&self.hid, &[0x11, 0x01])?);
        write(&self.hid, &kraken_lcd_info_request())?;
        self.lcd_info = parse_kraken_lcd_info(&read_prefix(&self.hid, &[0x31, 0x01])?);
        Ok(())
    }

    pub fn status(&self) -> Result<KrakenStatus> {
        drain(&self.hid);
        write(&self.hid, &kraken_status_request())?;
        let msg = read_prefix(&self.hid, &[0x75])?;
        parse_kraken_status(&msg).context("the cooler reported an invalid liquid temperature")
    }

    /// Stores a liquid-temperature curve that the cooler then runs on its own.
    pub fn set_curve(&self, channel: KrakenChannel, table: &[u8; KRAKEN_CURVE_POINTS]) -> Result<()> {
        write(&self.hid, &kraken_curve(channel, table))
    }

    pub fn set_ring(&self, lighting: &Lighting) -> Result<()> {
        write_lighting(&self.hid, KRAKEN_RING, lighting)
    }

    pub fn set_external(&self, lighting: &Lighting) -> Result<()> {
        write_lighting(&self.hid, KRAKEN_EXTERNAL, lighting)
    }

    pub fn lcd_settings(&self, brightness: u8, quarter_turns: u8) -> Result<()> {
        write(&self.hid, &kraken_lcd_settings(brightness, quarter_turns))
    }

    /// Sends an LCD command; its reply is `[cmd + 1, sub, …]` with a result code in byte 14.
    fn command(&self, payload: &[u8]) -> Result<u8> {
        write(&self.hid, payload)?;
        let reply = read_prefix_within(&self.hid, &[payload[0] + 1, payload[1]], LCD_REPLY_TIMEOUT)?;
        Ok(reply.get(14).copied().unwrap_or(0))
    }

    pub fn is_streaming(&self) -> bool {
        self.streaming
    }

    /// The built-in liquid temperature screen, drawn by the cooler itself.
    pub fn lcd_liquid(&mut self) -> Result<()> {
        self.streaming = false;
        let _ = self.command(&kraken_lcd_transfer_cancel());
        match self.command(&kraken_lcd_liquid_mode())? {
            0x01 => Ok(()),
            code => bail!("the cooler refused the liquid screen (code {code:#04x})"),
        }
    }

    /// Sends one Q565-encoded 640×640 frame into the streaming buffer, the same
    /// sequence CAM sends for each Web Integration frame. Unlike memory buckets this
    /// writes no flash and needs no bucket bookkeeping, which is what wedged coolers
    /// in liquidctl#774.
    pub fn lcd_frame(&mut self, q565: &[u8]) -> Result<()> {
        if self.lcd.is_none() {
            self.lcd = Some(open_lcd_interface()?);
        }
        if !self.streaming {
            // A transfer left open by an earlier process makes the device refuse new ones.
            let _ = self.command(&kraken_lcd_transfer_cancel());
        }
        self.push_frame(q565)?;
        self.streaming = true;
        Ok(())
    }

    fn push_frame(&self, q565: &[u8]) -> Result<()> {
        if self.command(&kraken_lcd_stream_begin())? != 0x01 {
            let _ = self.command(&kraken_lcd_transfer_cancel());
            let code = self.command(&kraken_lcd_stream_begin())?;
            if code != 0x01 {
                bail!("the cooler refused the frame (code {code:#04x})");
            }
        }
        let iface = self.lcd.as_ref().context("LCD interface not open")?;
        // The header goes in a transfer of its own, as CAM sends it. Sharing one with
        // the pixels delayed the cooler's end reply by seconds.
        for data in [lcd_bulk_header(LCD_FORMAT_Q565, q565.len() as u32), q565.to_vec()] {
            let done = futures_lite::future::block_on(iface.bulk_out(LCD_BULK_ENDPOINT, data));
            if let Err(e) = done.status {
                let _ = self.command(&kraken_lcd_transfer_cancel());
                bail!("bulk transfer to the LCD failed: {e}");
            }
        }
        match self.command(&kraken_lcd_transfer_end())? {
            0x01 => Ok(()),
            code => {
                let _ = self.command(&kraken_lcd_transfer_cancel());
                bail!("the cooler did not accept the frame (code {code:#04x})")
            }
        }
    }
}

const LCD_BULK_ENDPOINT: u8 = 0x02;

/// Interface 0 of the cooler is bound to WinUSB and carries the LCD bulk data.
fn open_lcd_interface() -> Result<nusb::Interface> {
    let info = nusb::list_devices()?
        .find(|d| d.vendor_id() == NZXT_VID && d.product_id() == KRAKEN_ELITE_V2_PID)
        .context("the Kraken's USB device is not visible")?;
    let dev = info.open().context("cannot open the Kraken's USB device")?;
    dev.claim_interface(0).context("cannot claim the LCD interface (is CAM still running?)")
}

pub struct ControlHub {
    hid: HidDevice,
    pub firmware: Option<(u8, u8, u8)>,
    last_status: Option<(HubStatus, Instant)>,
}

impl ControlHub {
    pub fn open(api: &HidApi) -> Result<ControlHub> {
        Ok(ControlHub { hid: open_hid(api, CONTROL_HUB_PID)?, firmware: None, last_status: None })
    }

    pub fn initialize(&mut self) -> Result<()> {
        drain(&self.hid);
        for p in hub_init() {
            write(&self.hid, &p)?;
        }
        write(&self.hid, &hub_firmware_request())?;
        self.firmware = parse_hub_firmware(&read_prefix(&self.hid, &[0x11, 0x02])?);
        Ok(())
    }

    /// The hub sends its fan report twice a second after `initialize`.
    pub fn status(&mut self) -> Result<HubStatus> {
        let mut buf = [0u8; REPORT_LEN];
        while let Ok(n) = self.hid.read_timeout(&mut buf, 0) {
            if n == 0 {
                break;
            }
            if let Some(s) = parse_hub_status(&buf[..n]) {
                self.last_status = Some((s, Instant::now()));
            }
        }
        match &self.last_status {
            Some((s, at)) if at.elapsed() < Duration::from_secs(3) => Ok(s.clone()),
            _ => {
                let msg = read_prefix(&self.hid, &[0x67, 0x02])?;
                let s = parse_hub_status(&msg).context("malformed fan report")?;
                self.last_status = Some((s.clone(), Instant::now()));
                Ok(s)
            }
        }
    }

    pub fn set_duties(&self, duties: &[Option<u8>; HUB_FAN_CHANNELS]) -> Result<()> {
        write(&self.hid, &hub_fixed_duties(duties))
    }

    /// `port` is 1..=5, as printed on the hub.
    pub fn set_lighting(&self, port: usize, lighting: &Lighting) -> Result<()> {
        let channel = *HUB_LED_PORTS.get(port.wrapping_sub(1)).context("LED ports are 1 to 5")?;
        write_lighting(&self.hid, channel, lighting)
    }
}
