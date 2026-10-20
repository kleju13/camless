//! HYTE's Nexus Portal NP50 (`3402:0901`): three Nexus Link ports with chains of up
//! to 18 modules each (FP12 fans, LS10 and LS30 strips, LN60 and LN70 panels), a
//! 4-pin fan header and a 6-LED logo, on a USB serial port. Another brand's hub:
//! camless opens it only with `[devices] other_brands` on, and leaves it to HYTE
//! Nexus while that runs.
//!
//! The hub runs its fans on its own (the mode saved in it: a fixed duty, or the
//! motherboard's PWM) until a cooling mode command puts it in software mode. It stays
//! there only while it gets an info request (`FF CC 01 00`) every 5 seconds, and goes
//! back on its own after that: camless sends one with every poll, so a poll interval
//! above about 4 seconds loses it. camless puts the hub in software mode only once a
//! curve takes one of its fans. Each port's fans take their duties in one command, so
//! the FP12s the config does not set run at 50 %, and the 4-pin header at 70 %. When
//! camless lets go it puts the hub back in the mode saved in it.
//!
//! Each port's LEDs go out as a frame `FF EE 01 <port> 01 68 00` and G, R, B per LED,
//! the logo's first on port 1. The hub takes a cycle of four frames, the fourth empty,
//! and runs its own animation 5 seconds after the last, so once the config sets a
//! light camless sends the cycle with every poll, with the lights it does not set off.
//!
//! camless never sends the commands that write the hub's EEPROM: `FF CC 03` (the
//! saved mode), `05` and `07` (the start and firmware animations), `0C`, nor the
//! firmware update. It reads the saved mode (`FF CC 04 00`) to give the hub back.
//! It does not read the fans' temperature probes.
//!
//! Sources: Nexus (`src/Peripherals/Hyte/Np50`, `src/Cooling/Np50CoolingProvider.cs`,
//! `src/Lighting/Np50LightingFrameWriter.cs`), from HYTE's own service and its
//! protocol documents. Not verified on hardware.

use super::hyte::{self, HYTE};
use super::serial::{self, Serial};
use super::{static_id, Driver, FanReading, Found, Model, Reading};
use anyhow::{bail, Context, Result};
use camless_core::lighting::{Lighting, Rgb};
use camless_core::status::{Device, FanChannel, LightChannel, Unit};
use hidapi::HidApi;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const NP50: Model = Model::vendor_serial(0x3402, 0x0901, "hyte-np50", "HYTE Nexus Portal NP50", &HYTE, open);

const PORTS: usize = 3;
/// The firmware takes four port frames a cycle.
const CYCLE_PORTS: usize = 4;
const MAX_MODULES: usize = 18;
const LOGO_LEDS: usize = 6;
const MIN_FRAME: usize = 90;

const INFO_LEN: usize = 20;
const CHANNEL_LEN: usize = 240;
const FIRMWARE_LEN: usize = 7;
const DEFAULTS_LEN: usize = 17;
const INFO_TIMEOUT: Duration = Duration::from_millis(250);
const CHANNEL_TIMEOUT: Duration = Duration::from_millis(400);
const REPLY_TIMEOUT: Duration = Duration::from_millis(300);

const SOFTWARE: u8 = 0x01;
const MOTHERBOARD: u8 = 0x02;
const STATIC: u8 = 0x03;
/// The saved mode that passes the motherboard's PWM through.
const SAVED_MOTHERBOARD: u8 = 0x01;
const STATIC_DEFAULT: u8 = 50;
/// The mode command parses on the thread the LEDs stream on, so Nexus repeats it
/// at most this often while the hub reports another mode.
const MODE_RETRY: Duration = Duration::from_secs(30);
const FAN_HOLD: u8 = 50;
const LEGACY_HOLD: u8 = 70;
const FP12: u8 = 0x03;

const GET_INFO: [u8; 4] = [0xff, 0xcc, 0x01, 0x00];
const GET_FIRMWARE: [u8; 7] = [0xff, 0xdd, 0x02, 0x00, 0x00, 0x00, 0x00];
const GET_DEFAULTS: [u8; 4] = [0xff, 0xcc, 0x04, 0x00];

fn get_channel(port: usize) -> [u8; 4] {
    [0xff, 0xcc, 0x01, port as u8]
}

/// The hub's state from the info reply.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Info {
    mode: u8,
    legacy_rpm: u16,
    /// The firmware animation: effect, R, G, B, brightness. A mode command
    /// carries it back unchanged.
    animation: [u8; 5],
}

/// `ParseHubInfo`: the 4-pin tach at 9, the mode at 12, the animation from 15. A
/// reply without the animation keeps the `last` one, as Nexus does.
fn parse_info(reply: &[u8], last: [u8; 5]) -> Option<Info> {
    if reply.len() < 14 || reply[..2] != [0xff, 0xcc] {
        return None;
    }
    let animation = reply.get(15..20).and_then(|a| a.try_into().ok()).unwrap_or(last);
    Some(Info { mode: reply[12], legacy_rpm: legacy_rpm(reply[9], reply[10]), animation })
}

/// `DecodeLegacyRpm`: another scale than the Nexus Link fans'.
fn legacy_rpm(high: u8, low: u8) -> u16 {
    if (high, low) == (0, 0) {
        return 0;
    }
    (60_000.0 / ((f32::from(high) * 100.0 + f32::from(low)) / 10.0 * 4.0)) as u16
}

/// A module on a Nexus Link port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Module {
    /// Its place on the chain, from 1: its slot in the fan command.
    index: u8,
    kind: u8,
    leds: u8,
}

impl Module {
    fn name(&self) -> &'static str {
        match self.kind {
            0x01 => "LS10",
            0x02 => "LS30",
            FP12 => "FP12",
            0x06 => "LN60",
            0x07 => "LN70",
            _ => "Nexus Link module",
        }
    }
}

/// `ParseChannelInfo`: 12-byte slots until one has no module type at 3; the
/// index at 2, the LEDs at 5, the tach at 8.
/// A port's reply is whole at its first slot without a module: the hub sometimes
/// sends less than the 240 bytes, as Nexus notes.
fn chain_ends(reply: &[u8]) -> bool {
    reply.len() >= 2 && reply.chunks_exact(12).any(|slot| slot[3] == 0)
}

fn parse_channel(reply: &[u8]) -> Option<Vec<(Module, u16)>> {
    if reply.len() < 2 || reply[..2] != [0xff, 0xcc] {
        return None;
    }
    let mut out = Vec::new();
    for (i, slot) in reply.chunks_exact(12).enumerate() {
        if slot[3] == 0 {
            break;
        }
        let index = if slot[2] > 0 { slot[2] } else { i as u8 + 1 };
        out.push((Module { index, kind: slot[3], leds: slot[5] }, hyte::rpm(slot[8], slot[9])));
    }
    Some(out)
}

/// `ParseFirmwareDefaultMode`: the saved mode at 4 and its fixed duty at 5.
fn parse_defaults(reply: &[u8]) -> Option<(u8, u8)> {
    (reply.len() >= 8 && reply[..2] == [0xff, 0xcc]).then(|| (reply[4], reply[5]))
}

/// `BuildSetCoolingMode`, the 15-byte form the firmware takes: the mode, a fixed
/// duty for the static one, turbo off, and the animation as it is.
fn cooling_mode(mode: u8, fixed: u8, animation: [u8; 5]) -> [u8; 15] {
    let [effect, r, g, b, brightness] = animation;
    let brightness = if brightness == 0 { 100 } else { brightness };
    [0xff, 0xcc, 0x02, 0x00, mode, fixed.min(100), 0, 0, 0, 0x01, effect, r, g, b, brightness]
}

/// `BuildSetLegacyFanSpeed`.
fn legacy_fan(duty: u8) -> [u8; 12] {
    let mut c = [0u8; 12];
    c[..6].copy_from_slice(&[0xff, 0xcc, 0x02, 0x00, 0x00, duty.min(100)]);
    c
}

/// `BuildSetPortFanSpeeds`: all 18 slots, `<slot> 00 <duty>` and six zeros each.
fn port_fans(port: usize, duties: &[u8; MAX_MODULES]) -> [u8; 4 + MAX_MODULES * 9] {
    let mut c = [0u8; 4 + MAX_MODULES * 9];
    c[..4].copy_from_slice(&[0xff, 0xcc, 0x02, port as u8]);
    for (i, &duty) in duties.iter().enumerate() {
        c[4 + i * 9] = i as u8 + 1;
        c[4 + i * 9 + 2] = duty.min(100);
    }
    c
}

/// `BuildLightingStream`: at least 90 bytes.
fn stream(port: usize, leds: &[Rgb]) -> Vec<u8> {
    let mut f = vec![0u8; (7 + leds.len() * 3).max(MIN_FRAME)];
    f[..7].copy_from_slice(&[0xff, 0xee, 0x01, port as u8, 0x01, 0x68, 0x00]);
    for (led, c) in f[7..].chunks_exact_mut(3).zip(leds) {
        led.copy_from_slice(&[c[1], c[0], c[2]]);
    }
    f
}

pub struct Np50 {
    port: Serial,
    firmware: Option<String>,
    info: Info,
    /// The modules on each Nexus Link port, as at open.
    modules: [Vec<Module>; PORTS],
    legacy: Option<u8>,
    /// Duties set by port and slot.
    fans: [[Option<u8>; MAX_MODULES]; PORTS],
    colors: HashMap<String, Rgb>,
    /// camless put the hub in software mode.
    software: bool,
    mode_sent: Option<Instant>,
}

fn open(_: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    let mut port = Serial::open(&found.path)?;
    let info = serial::request(&mut port, &GET_INFO, INFO_LEN, INFO_TIMEOUT)?;
    let info = parse_info(&info, [0; 5]).context("no hub info reply; not an NP50, or it is busy")?;
    let firmware = hyte::parse_firmware(&serial::request(&mut port, &GET_FIRMWARE, FIRMWARE_LEN, REPLY_TIMEOUT)?);
    let mut np50 = Np50 {
        port,
        firmware,
        info,
        modules: Default::default(),
        legacy: None,
        fans: [[None; MAX_MODULES]; PORTS],
        colors: HashMap::new(),
        software: false,
        mode_sent: None,
    };
    let chains = np50.chains()?;
    np50.modules = chains.map(|c| c.into_iter().map(|(m, _)| m).collect());
    Ok(Box::new(np50))
}

fn fan_id(port: usize, m: &Module) -> String {
    format!("port{port}-fan{}", m.index)
}

fn light_id(port: usize, m: &Module) -> String {
    format!("port{port}-dev{}", m.index)
}

impl Np50 {
    fn chains(&mut self) -> Result<[Vec<(Module, u16)>; PORTS]> {
        let mut out: [Vec<(Module, u16)>; PORTS] = Default::default();
        for (p, chain) in out.iter_mut().enumerate() {
            let reply =
                serial::request_until(&mut self.port, &get_channel(p + 1), CHANNEL_LEN, CHANNEL_TIMEOUT, &chain_ends)?;
            *chain = parse_channel(&reply).with_context(|| format!("no reply for port {}", p + 1))?;
        }
        Ok(out)
    }

    fn fp12s(&self) -> impl Iterator<Item = (usize, &Module)> {
        (1..=PORTS).flat_map(|p| self.modules[p - 1].iter().filter(|m| m.kind == FP12).map(move |m| (p, m)))
    }

    /// The mode command, again when the hub reports another mode, at most every
    /// 30 seconds.
    fn assert_software(&mut self) -> Result<()> {
        let due = self.mode_sent.is_none_or(|t| t.elapsed() >= MODE_RETRY);
        if self.info.mode != SOFTWARE && due {
            self.port.write(&cooling_mode(SOFTWARE, STATIC_DEFAULT, self.info.animation))?;
            self.mode_sent = Some(Instant::now());
        }
        Ok(())
    }

    /// The 4-pin header's duty and each port's with an FP12, holding the fans the
    /// config does not set; the other slots get 0, as Nexus sends them.
    fn send_duties(&mut self) -> Result<()> {
        self.port.write(&legacy_fan(self.legacy.unwrap_or(LEGACY_HOLD)))?;
        for p in 1..=PORTS {
            let mut duties = [0u8; MAX_MODULES];
            let mut any = false;
            for m in self.modules[p - 1].iter().filter(|m| m.kind == FP12) {
                let slot = usize::from(m.index) - 1;
                if slot < MAX_MODULES {
                    duties[slot] = self.fans[p - 1][slot].unwrap_or(FAN_HOLD);
                    any = true;
                }
            }
            if any {
                self.port.write(&port_fans(p, &duties))?;
            }
        }
        Ok(())
    }

    /// The frames of one cycle: the lights the config does not set are off.
    fn cycle(&self) -> Vec<Vec<u8>> {
        let color = |id: &str| self.colors.get(id).copied().unwrap_or_default();
        (1..=CYCLE_PORTS)
            .map(|p| {
                let mut leds = Vec::new();
                if p == 1 {
                    leds.resize(LOGO_LEDS, color("logo"));
                }
                for m in self.modules.get(p - 1).into_iter().flatten() {
                    leds.resize(leds.len() + usize::from(m.leds), color(&light_id(p, m)));
                }
                stream(p, &leds)
            })
            .collect()
    }

    fn send_lighting(&mut self) -> Result<()> {
        if self.colors.is_empty() {
            return Ok(());
        }
        for frame in self.cycle() {
            self.port.write(&frame)?;
        }
        Ok(())
    }
}

impl Driver for Np50 {
    fn describe(&self) -> Device {
        let mut fans = vec![FanChannel { id: "legacy".into(), name: "4-pin fan header".into(), ..Default::default() }];
        fans.extend(self.fp12s().map(|(p, m)| FanChannel {
            id: fan_id(p, m),
            name: format!("FP12, port {p} #{}", m.index),
            ..Default::default()
        }));
        let light = |id: String, name: String, leds: u16| LightChannel {
            id,
            name,
            leds,
            modes: vec!["off".into(), "fixed".into()],
            ..Default::default()
        };
        let mut lights = vec![light("logo".into(), "Logo".into(), LOGO_LEDS as u16)];
        for p in 1..=PORTS {
            for m in self.modules[p - 1].iter().filter(|m| m.leds > 0) {
                lights.push(light(light_id(p, m), format!("{}, port {p} #{}", m.name(), m.index), m.leds.into()));
            }
        }
        Device {
            name: NP50.name.into(),
            firmware: self.firmware.clone(),
            verified: NP50.verified(),
            experimental: NP50.is_experimental(),
            fans,
            lights,
            ..Default::default()
        }
    }

    fn sensors(&self) -> Vec<(&'static str, &'static str, Unit)> {
        Vec::new()
    }

    fn poll(&mut self) -> Result<Reading> {
        // The heartbeat: without it the hub leaves software mode in 5 s.
        let reply = serial::request(&mut self.port, &GET_INFO, INFO_LEN, INFO_TIMEOUT)?;
        self.info = parse_info(&reply, self.info.animation).context("no hub info reply")?;
        let chains = self.chains()?;
        let modules: Vec<Vec<Module>> = chains.iter().map(|c| c.iter().map(|(m, _)| *m).collect()).collect();
        if modules != self.modules {
            bail!("the modules on the hub changed; camless opens it again");
        }
        if self.software {
            self.assert_software()?;
            self.send_duties()?;
        }
        self.send_lighting()?;

        let duty = |p: usize, m: &Module| self.fans[p - 1].get(usize::from(m.index) - 1).copied().flatten();
        let mut fans = vec![("legacy", FanReading { rpm: Some(self.info.legacy_rpm), duty: self.legacy, connected: None })];
        for (p, chain) in chains.iter().enumerate().map(|(i, c)| (i + 1, c)) {
            for (m, rpm) in chain.iter().filter(|(m, _)| m.kind == FP12) {
                let fan = FanReading { rpm: Some(*rpm), duty: duty(p, m), connected: None };
                fans.push((static_id(&fan_id(p, m)), fan));
            }
        }
        Ok(Reading { sensors: Vec::new(), fans })
    }

    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
        for &(id, duty) in duties {
            if id == "legacy" {
                self.legacy = Some(duty.min(100));
                continue;
            }
            let (p, m) = self
                .fp12s()
                .find(|(p, m)| fan_id(*p, m) == id)
                .map(|(p, m)| (p, *m))
                .with_context(|| format!("no fan {id:?} on the hub"))?;
            let slot = usize::from(m.index) - 1;
            if slot >= MAX_MODULES {
                bail!("fan {id:?} is past the 18 the hub addresses");
            }
            self.fans[p - 1][slot] = Some(duty.min(100));
        }
        if !self.software {
            self.software = true;
            self.mode_sent = None;
        }
        self.assert_software()?;
        self.send_duties()
    }

    fn set_lighting(&mut self, light: &str, lighting: &Lighting) -> Result<()> {
        let known = light == "logo"
            || (1..=PORTS).any(|p| self.modules[p - 1].iter().any(|m| m.leds > 0 && light_id(p, m) == light));
        if !known {
            bail!("no light {light:?} on the hub");
        }
        let color = match lighting {
            Lighting::Off => [0, 0, 0],
            Lighting::Fixed(c) => *c,
            Lighting::Effect { .. } => bail!("the NP50 takes off or one colour"),
        };
        self.colors.insert(light.to_string(), color);
        self.send_lighting()
    }

    /// Back to the mode saved in the hub, which Nexus calls firmware control: its
    /// fixed duty, or the motherboard's PWM. The LEDs go back to the hub's animation
    /// 5 seconds after the last frame.
    fn release(&mut self) -> Result<()> {
        self.colors.clear();
        self.legacy = None;
        self.fans = [[None; MAX_MODULES]; PORTS];
        if !std::mem::take(&mut self.software) {
            return Ok(());
        }
        let saved = serial::request(&mut self.port, &GET_DEFAULTS, DEFAULTS_LEN, REPLY_TIMEOUT)
            .ok()
            .and_then(|r| parse_defaults(&r));
        let (mode, fixed) = match saved {
            Some((SAVED_MOTHERBOARD, _)) | None => (MOTHERBOARD, STATIC_DEFAULT),
            Some((_, fixed)) => (STATIC, fixed),
        };
        self.port.write(&cooling_mode(mode, fixed, self.info.animation))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_follow_nexus_builders() {
        assert_eq!(
            cooling_mode(SOFTWARE, 50, [2, 10, 20, 30, 0]),
            [0xff, 0xcc, 0x02, 0x00, 0x01, 50, 0, 0, 0, 0x01, 2, 10, 20, 30, 100]
        );
        assert_eq!(cooling_mode(STATIC, 40, [1, 0, 0, 0, 80])[4..6], [0x03, 40]);
        assert_eq!(legacy_fan(70), [0xff, 0xcc, 0x02, 0x00, 0x00, 70, 0, 0, 0, 0, 0, 0]);
        let mut duties = [0u8; 18];
        duties[1] = 60;
        let c = port_fans(2, &duties);
        assert_eq!(c.len(), 166);
        assert_eq!(&c[..13], [0xff, 0xcc, 0x02, 0x02, 1, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&c[13..16], [2, 0, 60]);
        assert_eq!(&c[157..160], [18, 0, 0]);
        // An empty port's frame is the header, padded.
        let f = stream(4, &[]);
        assert_eq!((f.len(), &f[..7]), (90, &[0xff, 0xee, 0x01, 0x04, 0x01, 0x68, 0x00][..]));
        assert_eq!(&stream(1, &[[0x10, 0x20, 0x30]])[7..10], [0x20, 0x10, 0x30]);
    }

    fn slot(count: u8, kind: u8, leds: u8, rpm: [u8; 2]) -> [u8; 12] {
        [0, 0, count, kind, 1, leds, 0, 1, rpm[0], rpm[1], 0, 1]
    }

    #[test]
    fn replies_give_the_mode_and_the_modules() {
        let mut info = [0u8; 20];
        info[..4].copy_from_slice(&GET_INFO);
        // 60000 / ((1 × 100 + 50) / 10 × 4) = 1000 rpm.
        info[9..11].copy_from_slice(&[1, 50]);
        info[12] = MOTHERBOARD;
        info[15..20].copy_from_slice(&[1, 255, 0, 0, 90]);
        assert_eq!(parse_info(&info, [0; 5]), Some(Info { mode: 2, legacy_rpm: 1000, animation: [1, 255, 0, 0, 90] }));
        assert_eq!(parse_info(&info[..14], [7; 5]).map(|i| i.animation), Some([7; 5]));
        assert_eq!(parse_info(&[0xff, 0xdd, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], [0; 5]), None);

        let mut reply = Vec::new();
        reply.extend_from_slice(&slot(1, FP12, 24, [12, 50]));
        // A slot whose index byte is 0 is numbered by its place.
        reply.extend_from_slice(&slot(0, 0x02, 30, [0, 0]));
        reply.extend_from_slice(&slot(3, 0x00, 0, [0, 0]));
        reply.extend_from_slice(&slot(4, FP12, 24, [0, 0]));
        reply[..2].copy_from_slice(&[0xff, 0xcc]);
        let chain = parse_channel(&reply).unwrap();
        assert_eq!(
            chain,
            [(Module { index: 1, kind: FP12, leds: 24 }, 1200), (Module { index: 2, kind: 0x02, leds: 30 }, 0)]
        );
        // The read stops at the empty slot, not at 240 bytes.
        assert!(!chain_ends(&reply[..24]) && !chain_ends(&reply[..35]) && chain_ends(&reply[..36]));
        assert_eq!(parse_defaults(&[0xff, 0xcc, 0x04, 0x00, 0x00, 45, 0, 0]), Some((0, 45)));
    }
}
