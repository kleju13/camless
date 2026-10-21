//! Lian Li's Galahad II AIOs: the Trinity (`0416:7373`), the Trinity Performance
//! (`0416:7371`) and the Galahad II LCD, or Vision (`0416:7395`). One duty for the
//! radiator's fans and one for the pump, with their speeds, and on the LCD the liquid
//! temperature. Another brand's cooler: camless opens it only with `[devices]
//! other_brands` on, and leaves it to L-Connect while that runs.
//!
//! Every command is a 64-byte report with id `01` on the interrupt pipe, as on the
//! Uni Fan TL controller: `01 <command> 00 00 00 <payload length> <payload>`. Status
//! `81` answers with the fan's and the pump's rpm, big-endian, and on the LCD a flag
//! and the liquid temperature in degrees and tenths. `8b 00 <duty>` sets the fans and
//! `8a 00 <duty>` the pump, 0..100; the `00` is host control.
//!
//! The cooler stores no curve, so camless drives it from the host, and sends the
//! duties again with every poll, as Nexus does. Nexus floors the Trinity's pump at
//! 50 % to keep the coolant moving, and the LCD's at 20 %, the lowest duty of its
//! reference driver's rpm table; liquidctl sends the LCD any duty.
//!
//! No source has a command that hands the fans or the Trinity's pump back to the
//! cooler. When camless lets go, the LCD's pump gets the duty it ran at before
//! camless set it, estimated from its rpm at open with Nexus's table, as Nexus does,
//! and the LCD's fans 50 %, about the 1320 rpm the liquidctl guide finds they start
//! at. Nexus sends the Trinity nothing; camless leaves it at the failsafe duty, as it
//! does when the service stops. If camless crashes, the last duty stays until the
//! cooler loses power.
//!
//! The LCD's screen needs an H.264 stream, which camless does not send; liquidctl
//! finds that fans, pump and status work without it. camless sets no lighting here.
//!
//! No source names the HID collection. On the Trinity camless opens a vendor-defined
//! one with 64-byte output reports and input reports, as Nexus picks it by those
//! lengths; on the LCD one with 1024-byte output reports, the one Nexus writes frames
//! and commands to. hidapi pads a command to the report's length.
//!
//! Sources: Nexus (`src/Peripherals/Galahad2`, `src/Peripherals/LianLiCp`,
//! `src/Cooling/Galahad2CoolingProvider.cs`, `src/Peripherals/JpegPanels/LianLiAioProtocol.cs`),
//! from L-Connect and FanControl.LianLi, and for the LCD from sgtaziz/lian-li-linux;
//! liquidctl's `ga2_lcd.py`, its tests and its guide, for the LCD. Nexus calls its
//! Galahad II support experimental. Not verified on hardware.

use super::lianli::LIAN_LI;
use super::{hid, Driver, FanReading, Found, Model, Reading, Usage};
use anyhow::{Context, Result};
use camless_core::lighting::Lighting;
use camless_core::status::{Device, FanChannel, Unit};
use hidapi::{HidApi, HidDevice};
use std::time::Duration;

const fn model(pid: u16, slug: &'static str, name: &'static str, output: usize) -> Model {
    Model::vendor_hid(0x0416, pid, slug, name, &LIAN_LI, Usage::Vendor { output, input: true }, open)
}

pub const TRINITY: Model = model(0x7373, "lianli-galahad2-trinity", "Lian Li Galahad II Trinity", PACKET_LEN);
pub const PERFORMANCE: Model =
    model(0x7371, "lianli-galahad2-performance", "Lian Li Galahad II Trinity Performance", PACKET_LEN);
/// Nexus's `JpegPanelModel.GalahadIiLcd`, `ReportLength` 1024.
pub const LCD: Model = model(0x7395, "lianli-galahad2-lcd", "Lian Li Galahad II LCD", 1024);

const PACKET_LEN: usize = 64;
const REPORT_ID: u8 = 0x01;
const PAYLOAD_AT: usize = 6;
const STATUS: u8 = 0x81;
const SET_PUMP: u8 = 0x8a;
const SET_FAN: u8 = 0x8b;
const HOST: u8 = 0x00;
/// Nexus's floors: the Trinity's `PumpDutyFloor`, the LCD's `GalahadPumpDutyFloor`.
const TRINITY_PUMP_MIN: u8 = 50;
const LCD_PUMP_MIN: u8 = 20;
/// Nexus drops faster readings as implausible.
const MAX_RPM: u16 = 6000;
const OPEN_TIMEOUT: Duration = Duration::from_millis(1000);
const POLL_TIMEOUT: Duration = Duration::from_millis(200);
/// Replies to the duties can come before the status's; liquidctl drops what is queued.
const READS: usize = 4;
/// Polls without a status before its readings go, as Nexus lets the LCD's expire.
const STALE_AFTER: u32 = 3;
/// The LCD's fans as they start, about 1320 rpm (liquidctl's guide).
const LCD_FAN_START: u8 = 50;
/// Nexus's `VisionRpmToPwm`, from sgtaziz/lian-li-linux: the LCD pump's rpm at a duty.
const LCD_PUMP_RPM: [(u16, u8); 29] = [
    (800, 20), (900, 23), (1000, 25), (1100, 27), (1200, 30), (1300, 31), (1400, 34), (1500, 36),
    (1600, 39), (1700, 41), (1800, 44), (1900, 47), (2000, 49), (2100, 52), (2200, 55), (2300, 58),
    (2400, 60), (2500, 64), (2600, 67), (2700, 70), (2800, 73), (2900, 77), (3000, 80), (3100, 83),
    (3200, 87), (3300, 90), (3400, 94), (3500, 98), (3600, 100),
];

/// `GalahadPwmForRpm`: the duty of an rpm, between the table's points.
fn lcd_pump_duty(rpm: u16) -> u8 {
    let t = LCD_PUMP_RPM;
    if rpm <= t[0].0 {
        return t[0].1;
    }
    for w in t.windows(2) {
        let ((r0, p0), (r1, p1)) = (w[0], w[1]);
        if rpm <= r1 {
            let (r0, p0, r1, p1, rpm) = (f64::from(r0), f64::from(p0), f64::from(r1), f64::from(p1), f64::from(rpm));
            return (p0 + (rpm - r0) * (p1 - p0) / (r1 - r0)).round() as u8;
        }
    }
    100
}

/// `CommandPacket.Build`.
fn packet(cmd: u8, payload: &[u8]) -> [u8; PACKET_LEN] {
    let mut p = [0u8; PACKET_LEN];
    p[0] = REPORT_ID;
    p[1] = cmd;
    p[5] = payload.len() as u8;
    p[PAYLOAD_AT..PAYLOAD_AT + payload.len()].copy_from_slice(payload);
    p
}

fn set_duty(cmd: u8, duty: u8, min: u8) -> [u8; PACKET_LEN] {
    packet(cmd, &[HOST, duty.clamp(min, 100)])
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Status {
    fan: Option<u16>,
    pump: Option<u16>,
    liquid: Option<f32>,
}

/// A status reply, or None for another command's. The temperature is the LCD's
/// only: Nexus reads it when the flag after the speeds is set, and drops the 1.0 °C
/// with both speeds at 0 that the firmware reports while it starts.
fn parse_status(reply: &[u8], liquid: bool) -> Option<Status> {
    if reply.first() != Some(&REPORT_ID) || reply.get(1) != Some(&STATUS) {
        return None;
    }
    let len = usize::from(*reply.get(5)?).min(reply.len().saturating_sub(PAYLOAD_AT));
    if len < 4 {
        return None;
    }
    let d = &reply[PAYLOAD_AT..PAYLOAD_AT + len];
    let rpm = |i: usize| Some(u16::from_be_bytes([d[i], d[i + 1]])).filter(|&r| r <= MAX_RPM);
    let (fan, pump) = (rpm(0), rpm(2));
    let liquid = (liquid && len >= 7 && d[4] != 0)
        .then(|| f32::from(d[5]) + f32::from(d[6] % 10) / 10.0)
        .filter(|&t| t > 0.0 && !(t == 1.0 && d[..4] == [0; 4]));
    Some(Status { fan, pump, liquid })
}

pub struct Galahad2 {
    dev: HidDevice,
    model: &'static Model,
    pump_min: u8,
    fan: Option<u8>,
    pump: Option<u8>,
    status: Status,
    misses: u32,
    /// The LCD pump's duty before camless set it, from its rpm at open.
    pump_found: Option<u8>,
}

fn open(api: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    let lcd = found.model.pid == LCD.pid;
    let mut d = Galahad2 {
        dev: hid::open(api, &found.path)?,
        model: found.model,
        pump_min: if lcd { LCD_PUMP_MIN } else { TRINITY_PUMP_MIN },
        fan: None,
        pump: None,
        status: Status::default(),
        misses: 0,
        pump_found: None,
    };
    // Nexus's connect is one status request; camless wants it answered.
    d.status = d.request_status(OPEN_TIMEOUT)?.context("no answer to the status request")?;
    if lcd {
        d.pump_found = d.status.pump.filter(|&r| r > 0).map(lcd_pump_duty);
    }
    Ok(Box::new(d))
}

impl Galahad2 {
    fn is_lcd(&self) -> bool {
        self.model.pid == LCD.pid
    }

    /// Drops the replies nobody read, so the status gets its own.
    fn flush(&self) {
        let mut buf = [0u8; PACKET_LEN];
        for _ in 0..8 {
            if !matches!(self.dev.read_timeout(&mut buf, 1), Ok(n) if n > 0) {
                break;
            }
        }
    }

    /// The status, or None if no status reply came in time.
    fn request_status(&self, timeout: Duration) -> Result<Option<Status>> {
        self.flush();
        self.dev.write(&packet(STATUS, &[])).context("write to the cooler failed")?;
        let mut buf = [0u8; PACKET_LEN];
        for _ in 0..READS {
            let n = self.dev.read_timeout(&mut buf, timeout.as_millis() as i32)?;
            if n == 0 {
                break;
            }
            if let Some(s) = parse_status(&buf[..n], self.is_lcd()) {
                return Ok(Some(s));
            }
        }
        Ok(None)
    }

    fn send(&self, cmd: u8, duty: u8, min: u8) -> Result<()> {
        self.dev.write(&set_duty(cmd, duty, min)).context("write to the cooler failed")?;
        Ok(())
    }
}

impl Driver for Galahad2 {
    fn describe(&self) -> Device {
        Device {
            name: self.model.name.into(),
            verified: self.model.verified(),
            experimental: self.model.is_experimental(),
            fans: vec![
                FanChannel { id: "fans".into(), name: "Radiator fans".into(), ..Default::default() },
                FanChannel { id: "pump".into(), name: "Pump".into(), pump: true, min_duty: self.pump_min, ..Default::default() },
            ],
            ..Default::default()
        }
    }

    fn sensors(&self) -> Vec<(&'static str, &'static str, Unit)> {
        if self.is_lcd() {
            vec![("liquid", "Liquid", Unit::Celsius)]
        } else {
            Vec::new()
        }
    }

    fn poll(&mut self) -> Result<Reading> {
        // No answer in time is not a lost cooler, as Nexus has it; a failed write is.
        match self.request_status(POLL_TIMEOUT)? {
            Some(s) => (self.status, self.misses) = (s, 0),
            None => {
                self.misses += 1;
                // A temperature that stopped is no reading: the curve goes failsafe.
                if self.misses >= STALE_AFTER {
                    self.status = Status::default();
                }
            }
        }
        // Nexus sends the duties again with every poll.
        if let Some(d) = self.fan {
            self.send(SET_FAN, d, 0)?;
        }
        if let Some(d) = self.pump {
            self.send(SET_PUMP, d, self.pump_min)?;
        }
        let s = self.status;
        Ok(Reading {
            sensors: s.liquid.map(|t| vec![("liquid", t)]).unwrap_or_default(),
            fans: vec![
                ("fans", FanReading { rpm: s.fan, duty: self.fan, connected: None }),
                ("pump", FanReading { rpm: s.pump, duty: self.pump, connected: None }),
            ],
        })
    }

    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
        for &(id, duty) in duties {
            match id {
                "fans" => {
                    self.send(SET_FAN, duty, 0)?;
                    self.fan = Some(duty.min(100));
                }
                "pump" => {
                    self.send(SET_PUMP, duty, self.pump_min)?;
                    self.pump = Some(duty.clamp(self.pump_min, 100));
                }
                _ => anyhow::bail!("no fan {id:?} on the {}", self.model.name),
            }
        }
        Ok(())
    }

    fn set_lighting(&mut self, light: &str, _: &Lighting) -> Result<()> {
        anyhow::bail!("no light {light:?} on the {}: camless sets no lighting on it", self.model.name)
    }

    /// The LCD's fans and pump back to how they ran before camless, the pump even if
    /// the fans fail. The Trinity has nothing to go back to.
    fn release(&mut self) -> Result<()> {
        if !self.is_lcd() {
            return Ok(());
        }
        let mut result = Ok(());
        if self.fan.take().is_some() {
            result = self.send(SET_FAN, LCD_FAN_START, 0);
        }
        if let (Some(_), Some(d)) = (self.pump.take(), self.pump_found) {
            result = result.and(self.send(SET_PUMP, d, self.pump_min));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duties_are_liquidctl_and_nexus_packets() {
        // liquidctl's test_ga2_lcd_fan_speed: `01 8b 00 00 00 02 00 32`.
        let fan = set_duty(SET_FAN, 50, 0);
        assert_eq!(fan.len(), 64);
        assert_eq!(&fan[..8], [0x01, 0x8b, 0, 0, 0, 2, 0, 50]);
        assert!(fan[8..].iter().all(|&b| b == 0));
        assert_eq!(set_duty(SET_FAN, 150, 0)[7], 100);
        assert_eq!(&set_duty(SET_PUMP, 70, TRINITY_PUMP_MIN)[..8], [0x01, 0x8a, 0, 0, 0, 2, 0, 70]);
        // `EncodeSetPump` floors the Trinity's pump at 50.
        assert_eq!([0, 49, 50, 100].map(|d| set_duty(SET_PUMP, d, TRINITY_PUMP_MIN)[7]), [50, 50, 50, 100]);
        assert_eq!(&packet(STATUS, &[])[..7], [0x01, 0x81, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn status_reads_liquidctls_sample() {
        // liquidctl's test_ga2_lcd_get_status: 1440 and 2670 rpm, 37.8 °C.
        let mut reply = [0u8; 64];
        reply[..13].copy_from_slice(&[0x01, 0x81, 0, 0, 0, 0x07, 0x05, 0xa0, 0x0a, 0x6e, 0x01, 0x25, 0x08]);
        let s = parse_status(&reply, true).unwrap();
        assert_eq!((s.fan, s.pump), (Some(1440), Some(2670)));
        assert!((s.liquid.unwrap() - 37.8).abs() < 0.01);
        // The Trinity's reply: Nexus reads the speeds only.
        assert_eq!(parse_status(&reply, false).unwrap().liquid, None);
        // The flag off, or the start-up placeholder, is no reading.
        reply[10] = 0;
        assert_eq!(parse_status(&reply, true).unwrap().liquid, None);
        let mut boot = [0u8; 64];
        boot[..13].copy_from_slice(&[0x01, 0x81, 0, 0, 0, 0x07, 0, 0, 0, 0, 0x01, 0x01, 0x00]);
        assert_eq!(parse_status(&boot, true).unwrap(), Status { fan: Some(0), pump: Some(0), liquid: None });
    }

    #[test]
    fn the_lcd_pump_goes_back_to_the_duty_of_its_rpm() {
        // Nexus's table: the points, between them, and past either end.
        assert_eq!([0, 800, 2000, 2050, 2670, 3600, 5000].map(lcd_pump_duty), [20, 20, 49, 51, 69, 100, 100]);
    }

    #[test]
    fn other_replies_are_not_a_status() {
        let mut reply = [0u8; 64];
        reply[..10].copy_from_slice(&[0x01, 0x8b, 0, 0, 0, 4, 0x05, 0xa0, 0x0a, 0x6e]);
        assert_eq!(parse_status(&reply, false), None, "a reply to a duty");
        reply[1] = STATUS;
        reply[5] = 3;
        assert_eq!(parse_status(&reply, false), None, "too short");
        reply[5] = 4;
        reply[6..8].copy_from_slice(&[0xff, 0xff]);
        assert_eq!(parse_status(&reply, false).unwrap(), Status { fan: None, pump: Some(2670), liquid: None });
        // A length past the reply stops at its end.
        reply[5] = 200;
        assert_eq!(parse_status(&reply[..10], false).unwrap().pump, Some(2670));
        assert_eq!(parse_status(&[], false), None);
    }
}
