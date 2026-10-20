//! Lian Li's Uni Fan TL controller (`0416:7372`): up to four ports of daisy-chained TL
//! fans, each fan with its own speed and lighting. Another brand's hub: camless opens
//! it only with `[devices] other_brands` on, and leaves it to L-Connect while that
//! runs.
//!
//! Every command is a 64-byte report with id `01` on the interrupt pipe, and the
//! controller answers each one: `01 <command> 00 00 00 <payload length> <payload>`.
//! The handshake `A1` lists the fans with their speeds. camless takes a fan off the
//! motherboard's sync when a curve first takes it, and hands it back when camless
//! lets go. Lighting is off or one colour per port, in the controller's static mode.
//!
//! No source names the HID collection. camless opens a vendor-defined one with 64-byte
//! output reports and input reports, as Nexus picks the interface by those lengths.
//!
//! Sources: Nexus (`src/Peripherals/LianLiTl`, `src/Peripherals/LianLiCp`,
//! `src/Lighting/TlLightingFrameWriter.cs`), from L-Connect and FanControl.LianLi.
//! Nexus calls its TL support untested, pending verification. Not verified on
//! hardware.

use super::lianli::LIAN_LI;
use super::{hid, static_id, Driver, FanReading, Found, Model, Reading, Usage};
use anyhow::{bail, Context, Result};
use camless_core::lighting::{Lighting, Rgb};
use camless_core::status::{Device, FanChannel, LightChannel, Unit};
use hidapi::{HidApi, HidDevice};
use std::time::Duration;

pub const MODEL: Model = Model::vendor_hid(
    0x0416,
    0x7372,
    "lianli-uni-fan-tl",
    "Lian Li Uni Fan TL controller",
    &LIAN_LI,
    Usage::Vendor { output: PACKET_LEN, input: true },
    open,
);

const PACKET_LEN: usize = 64;
const REPORT_ID: u8 = 0x01;
const PAYLOAD_AT: usize = 6;
const HANDSHAKE: u8 = 0xa1;
const SET_SPEED: u8 = 0xaa;
const SYNC: u8 = 0xb1;
const SET_LIGHT: u8 = 0xa3;
const PORTS: u8 = 4;
/// The firmware takes duties from 12; 0 idles the fan at 1.
const MIN_DUTY: u8 = 12;
const IDLE: u8 = 1;
const MAX_RPM: u16 = 6000;
const DISCOVER_TIMEOUT: Duration = Duration::from_millis(1000);
const POLL_TIMEOUT: Duration = Duration::from_millis(200);
const LIGHT_TIMEOUT: Duration = Duration::from_millis(50);
/// Static mode, and its brightness and speed at their defaults: 4 is full.
const MODE_OFF: u8 = 0;
const MODE_STATIC: u8 = 3;
const BRIGHTNESS: u8 = 4;
const SPEED: u8 = 2;

/// `CommandPacket.Build`.
fn packet(cmd: u8, payload: &[u8]) -> [u8; PACKET_LEN] {
    let mut p = [0u8; PACKET_LEN];
    p[0] = REPORT_ID;
    p[1] = cmd;
    p[5] = payload.len() as u8;
    p[PAYLOAD_AT..PAYLOAD_AT + payload.len()].copy_from_slice(payload);
    p
}

fn address(port: u8, fan: u8) -> u8 {
    (port & 0x0f) << 4 | fan & 0x0f
}

fn set_speed(port: u8, fan: u8, duty: u8) -> [u8; PACKET_LEN] {
    let pwm = if duty == 0 { IDLE } else { duty.clamp(MIN_DUTY, 100) };
    packet(SET_SPEED, &[address(port, fan), pwm])
}

/// With `sync` the fan follows the motherboard's PWM; without, the controller.
fn sync(port: u8, fan: u8, on: bool) -> [u8; PACKET_LEN] {
    packet(SYNC, &[address(port, fan) | if on { 0x80 } else { 0 }])
}

/// `EncodeSetFanLight`: off, or one colour in static mode.
fn set_light(port: u8, fan: u8, color: Option<Rgb>) -> [u8; PACKET_LEN] {
    let mut p = [0u8; 20];
    p[0] = (port & 0x0f) << 4;
    p[1] = address(port, fan);
    match color {
        Some(c) => {
            (p[2], p[3], p[4]) = (MODE_STATIC, BRIGHTNESS, SPEED);
            p[5..8].copy_from_slice(&c);
            p[19] = 1;
        }
        None => {
            p[2] = MODE_OFF;
            p[18] = 1;
        }
    }
    packet(SET_LIGHT, &p)
}

/// The fans a handshake reply lists: (port, fan, rpm), the ones detected only. A
/// late reply to another command lists none.
fn parse_handshake(reply: &[u8]) -> Vec<(u8, u8, u16)> {
    if reply.get(1) != Some(&HANDSHAKE) {
        return Vec::new();
    }
    let count = usize::from(*reply.get(5).unwrap_or(&0)) / 3;
    (0..count)
        .filter_map(|i| {
            let r = reply.get(PAYLOAD_AT + 3 * i..PAYLOAD_AT + 3 * i + 3)?;
            (r[0] & 0x80 != 0).then(|| ((r[0] >> 4) & 0x03, r[0] & 0x0f, u16::from_be_bytes([r[1], r[2]])))
        })
        .collect()
}

struct Fan {
    port: u8,
    fan: u8,
    id: &'static str,
    duty: Option<u8>,
    /// Off the motherboard's sync, which camless does when a curve first takes it.
    taken: bool,
    rpm: Option<u16>,
}

pub struct LianLiTl {
    dev: HidDevice,
    fans: Vec<Fan>,
}

fn open(api: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    let mut d = LianLiTl { dev: hid::open(api, &found.path)?, fans: Vec::new() };
    let mut listed = parse_handshake(&d.request(&packet(HANDSHAKE, &[]), DISCOVER_TIMEOUT)?.context("no answer to the handshake")?);
    listed.sort();
    listed.dedup_by_key(|f| (f.0, f.1));
    d.fans = listed
        .into_iter()
        .map(|(port, fan, rpm)| Fan {
            port,
            fan,
            id: static_id(&format!("port{}-fan{}", port + 1, fan + 1)),
            duty: None,
            taken: false,
            rpm: (rpm <= MAX_RPM).then_some(rpm),
        })
        .collect();
    Ok(Box::new(d))
}

impl LianLiTl {
    /// Drops the replies nobody read, so each command gets its own.
    fn flush(&self) {
        let mut buf = [0u8; PACKET_LEN];
        for _ in 0..8 {
            if !matches!(self.dev.read_timeout(&mut buf, 1), Ok(n) if n > 0) {
                break;
            }
        }
    }

    /// Writes a command and reads its reply; `None` if none came in time.
    fn request(&self, cmd: &[u8; PACKET_LEN], timeout: Duration) -> Result<Option<Vec<u8>>> {
        self.flush();
        self.dev.write(cmd).context("write to the controller failed")?;
        let mut buf = [0u8; PACKET_LEN];
        let n = self.dev.read_timeout(&mut buf, timeout.as_millis() as i32)?;
        Ok((n > 0).then(|| buf[..n].to_vec()))
    }

    fn send(&mut self, i: usize, duty: u8) -> Result<()> {
        let (port, fan) = (self.fans[i].port, self.fans[i].fan);
        if !self.fans[i].taken {
            self.request(&sync(port, fan, false), LIGHT_TIMEOUT)?;
            self.fans[i].taken = true;
        }
        self.request(&set_speed(port, fan, duty), LIGHT_TIMEOUT)?;
        Ok(())
    }
}

impl Driver for LianLiTl {
    fn describe(&self) -> Device {
        let mut ports: Vec<u8> = self.fans.iter().map(|f| f.port).collect();
        ports.dedup();
        Device {
            name: MODEL.name.into(),
            verified: MODEL.verified(),
            experimental: MODEL.is_experimental(),
            fans: self
                .fans
                .iter()
                .map(|f| FanChannel {
                    id: f.id.into(),
                    name: format!("Port {}, fan {}", f.port + 1, f.fan + 1),
                    min_duty: 0,
                    ..Default::default()
                })
                .collect(),
            lights: ports
                .into_iter()
                .map(|p| LightChannel {
                    id: format!("port{}", p + 1),
                    name: format!("Port {} lighting", p + 1),
                    accessories: self.fans.iter().filter(|f| f.port == p).map(|f| format!("TL fan {}", f.fan + 1)).collect(),
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
        // No answer in time is not a lost controller, as Nexus has it; a failed
        // write is.
        if let Some(reply) = self.request(&packet(HANDSHAKE, &[]), POLL_TIMEOUT)? {
            for (port, fan, rpm) in parse_handshake(&reply) {
                if let Some(f) = self.fans.iter_mut().find(|f| (f.port, f.fan) == (port, fan)) {
                    if rpm <= MAX_RPM {
                        f.rpm = Some(rpm);
                    }
                }
            }
        }
        // Nexus sends the duties again with every poll.
        for i in 0..self.fans.len() {
            if let Some(duty) = self.fans[i].duty {
                self.send(i, duty)?;
            }
        }
        let fans = self.fans.iter().map(|f| (f.id, FanReading { rpm: f.rpm, duty: f.duty, connected: Some(true) })).collect();
        Ok(Reading { sensors: Vec::new(), fans })
    }

    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
        for &(id, duty) in duties {
            let i = self.fans.iter().position(|f| f.id == id).with_context(|| format!("no fan {id:?} on this controller"))?;
            self.send(i, duty)?;
            self.fans[i].duty = Some(duty.min(100));
        }
        Ok(())
    }

    fn set_lighting(&mut self, light: &str, lighting: &Lighting) -> Result<()> {
        let port = light
            .strip_prefix("port")
            .and_then(|n| n.parse::<u8>().ok())
            .filter(|n| (1..=PORTS).contains(n) && self.fans.iter().any(|f| f.port == n - 1))
            .with_context(|| format!("no light {light:?} on this controller"))?
            - 1;
        let color = match lighting {
            Lighting::Off => None,
            Lighting::Fixed(c) => Some(*c),
            Lighting::Effect { .. } => bail!("this controller takes off or one colour"),
        };
        for f in self.fans.iter().filter(|f| f.port == port) {
            self.request(&set_light(f.port, f.fan, color), LIGHT_TIMEOUT)?;
        }
        Ok(())
    }

    /// Every fan camless took back on the motherboard's sync, the others even if one
    /// fails.
    fn release(&mut self) -> Result<()> {
        let mut result = Ok(());
        for i in 0..self.fans.len() {
            self.fans[i].duty = None;
            if std::mem::take(&mut self.fans[i].taken) {
                let (port, fan) = (self.fans[i].port, self.fans[i].fan);
                if let Err(e) = self.request(&sync(port, fan, true), LIGHT_TIMEOUT) {
                    result = result.and(Err(e));
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets_are_nexus_command_packets() {
        let hs = packet(HANDSHAKE, &[]);
        assert_eq!(&hs[..7], [0x01, 0xa1, 0, 0, 0, 0, 0]);
        assert_eq!(hs.len(), 64);
        assert_eq!(&set_speed(2, 3, 55)[..8], [0x01, 0xaa, 0, 0, 0, 2, 0x23, 55]);
        // `EncodeSetFanSpeed`: 0 idles at 1, the rest clamps to 12..100.
        assert_eq!([0, 1, 11, 12, 100, 120].map(|d| set_speed(0, 0, d)[7]), [1, 12, 12, 12, 100, 100]);
        assert_eq!(&sync(1, 0, false)[..7], [0x01, 0xb1, 0, 0, 0, 1, 0x10]);
        assert_eq!(&sync(1, 0, true)[..7], [0x01, 0xb1, 0, 0, 0, 1, 0x90]);
    }

    #[test]
    fn lights_are_static_or_off() {
        let on = set_light(1, 2, Some([0x11, 0x22, 0x33]));
        assert_eq!(&on[..6], [0x01, 0xa3, 0, 0, 0, 20]);
        let p = &on[PAYLOAD_AT..PAYLOAD_AT + 20];
        assert_eq!(&p[..8], [0x10, 0x12, 3, 4, 2, 0x11, 0x22, 0x33]);
        assert_eq!(&p[8..], [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        let off = set_light(3, 0, None);
        let p = &off[PAYLOAD_AT..PAYLOAD_AT + 20];
        assert_eq!(&p[..5], [0x30, 0x30, 0, 0, 0]);
        assert_eq!((p[18], p[19]), (1, 0));
    }

    #[test]
    fn handshake_lists_the_detected_fans() {
        let mut reply = [0u8; 64];
        reply[..6].copy_from_slice(&[0x01, 0xa1, 0, 0, 0, 9]);
        // Port 0 fan 0 at 1200, port 2 fan 1 at 900, and an undetected slot.
        reply[6..15].copy_from_slice(&[0x80, 0x04, 0xb0, 0xa1, 0x03, 0x84, 0x10, 0x01, 0x00]);
        assert_eq!(parse_handshake(&reply), [(0, 0, 1200), (2, 1, 900)]);
        // A count past the reply stops at its end.
        reply[5] = 255;
        assert_eq!(parse_handshake(&reply[..12]), [(0, 0, 1200), (2, 1, 900)]);
        assert!(parse_handshake(&[]).is_empty());
        reply[1] = 0xaa;
        assert!(parse_handshake(&reply).is_empty(), "a late reply to a duty");
    }
}
