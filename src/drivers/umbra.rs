//! Asiahorse's Umbra V2 fan and ARGB hub (`1a86:fe04`): ten ARGB ports and fan
//! headers that all run at one duty, with no speed readings. Another brand's hub:
//! camless opens it only with `[devices] other_brands` on, and leaves it to Sync_light
//! while that runs.
//!
//! Every frame is 64 bytes, `52 42 <length> 00 <command> <arguments> <sum>`, written
//! after a zero report id. The hub answers requests with a frame of the same command.
//! The fans follow the motherboard's PWM header until a curve takes them: then the
//! hub runs them at camless's duty, sent again with every poll, and when camless lets
//! go they follow the header again. Lighting is off or one colour on every port, in
//! the hub's static effect; the hub holds it.
//!
//! No source names the HID collection. camless opens a vendor-defined one with 64-byte
//! output reports and input reports, as Nexus picks the interface by those lengths.
//!
//! camless never sends the commands that store settings or change the firmware's
//! state: `02` (a port's LED count, kept in flash), `0B`, `0C`, `F9`, `FA`, `FC`, `FF`.
//!
//! Sources: Nexus (`src/Peripherals/AsiahorseUmbra`, `src/Cooling/AsiahorseUmbraCoolingProvider.cs`),
//! from the vendor app and USB captures of it. Not verified on hardware.

use super::{hid, Driver, FanReading, Found, Model, Reading, Usage, Vendor};
use anyhow::{bail, Context, Result};
use camless_core::lighting::{Lighting, Rgb};
use camless_core::status::{Device, FanChannel, LightChannel, Unit};
use hidapi::{HidApi, HidDevice};
use std::time::Duration;

pub const ASIAHORSE: Vendor = Vendor { name: "Asiahorse", app: "Sync_light", processes: &["sync_light.exe"] };

pub const MODEL: Model = Model::vendor_hid(
    0x1a86,
    0xfe04,
    "asiahorse-umbra-v2",
    "Asiahorse Umbra V2",
    &ASIAHORSE,
    Usage::Vendor { output: FRAME_LEN, input: true },
    open,
);

const FRAME_LEN: usize = 64;
const INFO: u8 = 0x00;
const EFFECT: u8 = 0x03;
const LIGHTS_OFF: u8 = 0x06;
const FAN_SYNC: u8 = 0x08;
const FAN_DUTY: u8 = 0xfb;
const AUTHORITY: u8 = 0xfd;
/// The model code the vendor app pairs with this product id.
const MODEL_CODE: u32 = 5;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(1000);
const REQUEST_TIMEOUT: Duration = Duration::from_millis(300);
/// A request skips late replies to earlier ones, up to this many.
const MAX_REPLY_READS: usize = 8;
const EFFECT_BLOCK_LEN: usize = 52;
/// The static effect, at Nexus's default speed (level 3, sent as 6 - 3),
/// forward, and full brightness (level 5 of 51 each).
const STATIC: u8 = 0;
const SPEED: u8 = 3;
const FORWARD: u8 = 1;
const BRIGHTNESS: u8 = 255;

/// `UmbraProtocol.Frame`: the length counts the header, the arguments and the sum,
/// which adds up every byte before it.
fn frame(cmd: u8, args: &[u8]) -> [u8; FRAME_LEN] {
    let len = 5 + args.len() + 1;
    let mut f = [0u8; FRAME_LEN];
    f[..5].copy_from_slice(&[0x52, 0x42, len as u8, 0x00, cmd]);
    f[5..5 + args.len()].copy_from_slice(args);
    f[len - 1] = f[..len - 1].iter().fold(0u8, |s, &b| s.wrapping_add(b));
    f
}

/// The fans follow the motherboard's PWM header, or the hub.
fn fan_sync(follow_motherboard: bool) -> [u8; FRAME_LEN] {
    frame(FAN_SYNC, &[if follow_motherboard { 0x00 } else { 0x01 }])
}

fn fan_duty(duty: u8) -> [u8; FRAME_LEN] {
    frame(FAN_DUTY, &[duty.min(100)])
}

/// The hub drives the LEDs, rather than passing its ARGB input header through.
fn authority() -> [u8; FRAME_LEN] {
    frame(AUTHORITY, &[0x01])
}

/// The static effect on every port at once (the sync group `FF FF`), colour 1 in
/// all four slots, as Nexus sends it.
fn static_effect(color: Rgb) -> [u8; FRAME_LEN] {
    let mut args = [0u8; 2 + EFFECT_BLOCK_LEN];
    args[..6].copy_from_slice(&[0xff, 0xff, STATIC, SPEED, FORWARD, BRIGHTNESS]);
    for slot in args[6..18].chunks_exact_mut(3) {
        slot.copy_from_slice(&color);
    }
    frame(EFFECT, &args)
}

/// A reply to `cmd`, with the zero report id Windows leaves in front taken off.
fn reply_to(cmd: u8, reply: &[u8]) -> Option<&[u8]> {
    let r = match reply {
        [0x00, 0x52, 0x42, ..] => &reply[1..],
        _ => reply,
    };
    (r.len() >= 5 && r[..2] == [0x52, 0x42] && r[4] == cmd).then_some(r)
}

/// `DecodeInfo`: the model code at 5 and the firmware at 17.
fn parse_info(reply: &[u8]) -> Option<(u32, String)> {
    if reply.len() < 25 {
        return None;
    }
    let model = u32::from_be_bytes(reply[5..9].try_into().ok()?);
    Some((model, format!("{}.{}.{}", reply[17], reply[18], reply[19])))
}

pub struct Umbra {
    dev: HidDevice,
    firmware: String,
    duty: Option<u8>,
    /// The fans on the hub's duty rather than the header's.
    taken: bool,
}

fn open(api: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    let mut d = Umbra { dev: hid::open(api, &found.path)?, firmware: String::new(), duty: None, taken: false };
    let reply = d.request(INFO, &frame(INFO, &[]), CONNECT_TIMEOUT)?.context("no info reply; not an Umbra V2, or it is busy")?;
    let (model, firmware) = parse_info(&reply).context("the hub's info reply is too short")?;
    if model != MODEL_CODE {
        bail!("this hub calls itself model {model}, not the Umbra V2 (5); camless leaves it alone");
    }
    d.firmware = firmware;
    Ok(Box::new(d))
}

impl Umbra {
    fn send(&self, f: &[u8; FRAME_LEN]) -> Result<()> {
        let mut report = [0u8; FRAME_LEN + 1];
        report[1..].copy_from_slice(f);
        self.dev.write(&report).context("write to the hub failed")?;
        Ok(())
    }

    /// The reply to `cmd`, or `None` if none came in time: a slow hub, not a lost
    /// one, as Nexus has it. A failed write or read is an error.
    fn request(&self, cmd: u8, f: &[u8; FRAME_LEN], timeout: Duration) -> Result<Option<Vec<u8>>> {
        self.send(f)?;
        let mut buf = [0u8; FRAME_LEN + 1];
        for _ in 0..MAX_REPLY_READS {
            let n = self.dev.read_timeout(&mut buf, timeout.as_millis() as i32)?;
            if n == 0 {
                break;
            }
            if let Some(r) = reply_to(cmd, &buf[..n]) {
                return Ok(Some(r.to_vec()));
            }
        }
        Ok(None)
    }

    fn drive(&mut self, duty: u8) -> Result<()> {
        if !self.taken {
            self.send(&fan_sync(false))?;
            self.taken = true;
        }
        self.send(&fan_duty(duty))
    }
}

impl Driver for Umbra {
    fn describe(&self) -> Device {
        Device {
            name: MODEL.name.into(),
            firmware: Some(self.firmware.clone()),
            verified: MODEL.verified(),
            experimental: MODEL.is_experimental(),
            fans: vec![FanChannel { id: "fans".into(), name: "Fans".into(), ..Default::default() }],
            lights: vec![LightChannel {
                id: "argb".into(),
                name: "ARGB ports".into(),
                modes: vec!["off".into(), "fixed".into()],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn sensors(&self) -> Vec<(&'static str, &'static str, Unit)> {
        Vec::new()
    }

    fn poll(&mut self) -> Result<Reading> {
        // The info request is the poll, as in Nexus; its reply carries nothing camless
        // shows. Nexus sends the duty again with every one.
        self.request(INFO, &frame(INFO, &[]), REQUEST_TIMEOUT)?;
        if let Some(duty) = self.duty {
            self.drive(duty)?;
        }
        let fans = vec![("fans", FanReading { rpm: None, duty: self.duty, connected: None })];
        Ok(Reading { sensors: Vec::new(), fans })
    }

    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
        for &(fan, duty) in duties {
            if fan != "fans" {
                bail!("no fan {fan:?} on this hub: its fans run at one duty, \"fans\"");
            }
            self.drive(duty)?;
            self.duty = Some(duty.min(100));
        }
        Ok(())
    }

    fn set_lighting(&mut self, light: &str, lighting: &Lighting) -> Result<()> {
        if light != "argb" {
            bail!("no light {light:?} on this hub");
        }
        let effect = match lighting {
            Lighting::Off => frame(LIGHTS_OFF, &[]),
            Lighting::Fixed(c) => static_effect(*c),
            Lighting::Effect { .. } => bail!("this hub takes off or one colour"),
        };
        self.send(&authority())?;
        self.send(&effect)
    }

    fn release(&mut self) -> Result<()> {
        self.duty = None;
        if std::mem::take(&mut self.taken) {
            self.send(&fan_sync(true))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_carry_their_length_and_sum() {
        // Info: 52 42 06 00 00, sum 0x52 + 0x42 + 0x06 = 0x9a.
        assert_eq!(&frame(INFO, &[])[..7], [0x52, 0x42, 0x06, 0x00, 0x00, 0x9a, 0]);
        assert_eq!(&fan_sync(false)[..7], [0x52, 0x42, 0x07, 0x00, 0x08, 0x01, 0xa4]);
        assert_eq!(&fan_sync(true)[..7], [0x52, 0x42, 0x07, 0x00, 0x08, 0x00, 0xa3]);
        assert_eq!(&fan_duty(150)[..7], [0x52, 0x42, 0x07, 0x00, 0xfb, 100, 0xfa]);
        assert_eq!(&authority()[..7], [0x52, 0x42, 0x07, 0x00, 0xfd, 0x01, 0x99]);
        assert_eq!(&frame(LIGHTS_OFF, &[])[..6], [0x52, 0x42, 0x06, 0x00, 0x06, 0xa0]);
    }

    #[test]
    fn static_effect_fills_every_slot() {
        let f = static_effect([0x10, 0x20, 0x30]);
        // 5 + 2 + 52 + 1 bytes.
        assert_eq!(f[2], 60);
        assert_eq!(&f[4..11], [0x03, 0xff, 0xff, 0x00, 0x03, 0x01, 0xff]);
        assert_eq!(&f[11..23], [0x10, 0x20, 0x30].repeat(4).as_slice());
        assert!(f[23..59].iter().all(|&b| b == 0));
        assert_eq!(f[59], f[..59].iter().fold(0u8, |s, &b| s.wrapping_add(b)));
    }

    #[test]
    fn replies_match_their_command_and_give_the_model() {
        let mut reply = frame(INFO, &[0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 0, 0, 0, 1, 1]);
        reply[2] = 26;
        assert!(reply_to(INFO, &reply).is_some());
        assert!(reply_to(FAN_DUTY, &reply).is_none());
        // Windows hands the zero report id over in front.
        let mut windows = vec![0u8];
        windows.extend_from_slice(&reply);
        let r = reply_to(INFO, &windows).unwrap();
        assert_eq!(parse_info(r), Some((5, "1.2.3".to_string())));
        assert_eq!(parse_info(&reply[..20]), None);
        assert!(reply_to(INFO, &[0x52, 0x41, 0, 0, 0]).is_none());
    }
}
