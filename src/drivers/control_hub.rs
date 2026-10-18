//! NZXT Control Hub (`1e71:2022`): five fan ports and five LED ports. It ships with
//! the H9 Flow RGB+ and other recent NZXT cases.
//!
//! Sources: liquidctl `control_hub.py` (PR #912) and USB captures of NZXT CAM 4.76.6.
//! Verified on hardware: fan duties and lighting.

use super::{hid, hue2, Driver, FanReading, Found, Model, Reading};
use anyhow::{bail, Context, Result};
use camless_core::lighting::Lighting;
use camless_core::status::{Device, FanChannel, LightChannel, Unit};
use hidapi::{HidApi, HidDevice};
use std::time::{Duration, Instant};

pub const MODEL: Model = Model {
    vid: 0x1e71,
    pid: 0x2022,
    interface: None,
    slug: "control-hub",
    name: "Control Hub",
    open,
};

const PORTS: usize = 5;
/// LED ports 1..=5 as channel bytes (liquidctl's mapping, matching CAM's captures).
const LED_CHANNELS: [u8; PORTS] = [0x02, 0x01, 0x04, 0x08, 0x10];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FanMode {
    None,
    Dc,
    Pwm,
    Unknown(u8),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    pub mode: FanMode,
    pub rpm: u16,
    pub duty: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub fans: Vec<Port>,
    pub noise_db: u8,
}

/// The periodic `67 02` fan report: mode at [16+i], rpm LE at [24+2i], duty at [40+i].
pub fn parse_status(msg: &[u8]) -> Option<Status> {
    if msg.len() < 57 || msg[0] != 0x67 || msg[1] != 0x02 {
        return None;
    }
    let fans = (0..PORTS)
        .map(|i| Port {
            mode: match msg[16 + i] {
                0 => FanMode::None,
                1 => FanMode::Dc,
                2 => FanMode::Pwm,
                m => FanMode::Unknown(m),
            },
            rpm: u16::from_le_bytes([msg[24 + 2 * i], msg[25 + 2 * i]]),
            duty: msg[40 + i],
        })
        .collect();
    Some(Status { fans, noise_db: msg[56] })
}

/// Sets the listed fan ports at once (`62 01 <mask> d1..d5`).
pub fn duties_report(duties: &[(usize, u8)]) -> Vec<u8> {
    let mut msg = vec![0x62, 0x01, 0x00, 0, 0, 0, 0, 0];
    for &(i, d) in duties {
        msg[2] |= 1 << i;
        msg[3 + i] = d.min(100);
    }
    msg
}

pub struct ControlHub {
    hid: HidDevice,
    firmware: Option<String>,
    accessories: Vec<hue2::ChannelAccessories>,
    last: Option<(Status, Instant)>,
}

fn open(api: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    let hid = hid::open(api, &found.path)?;
    hid::drain(&hid);
    // Fan reports every 0.5 s in both report slots, as liquidctl sets it up.
    hid::write(&hid, &[0x60, 0x02, 0x01, 0xe8, 0x01, 0x01, 0xe8, 0x01])?;
    hid::write(&hid, &[0x66, 0x01])?;
    let fw = hid::request(&hid, &[0x10, 0x02], &[0x11, 0x02])?;
    let firmware = (fw.len() > 0x13).then(|| format!("{}.{}.{}", fw[0x11], fw[0x12], fw[0x13]));
    let accessories = hue2::read_accessories(&hid, PORTS).unwrap_or_else(|_| vec![Default::default(); PORTS]);
    Ok(Box::new(ControlHub { hid, firmware, accessories, last: None }))
}

fn port(id: &str, prefix: &str) -> Option<usize> {
    id.strip_prefix(prefix).and_then(|n| n.parse::<usize>().ok()).filter(|n| (1..=PORTS).contains(n)).map(|n| n - 1)
}

impl ControlHub {
    fn status(&mut self) -> Result<Status> {
        for msg in hid::pending(&self.hid) {
            if let Some(s) = parse_status(&msg) {
                self.last = Some((s, Instant::now()));
            }
        }
        match &self.last {
            Some((s, at)) if at.elapsed() < Duration::from_secs(3) => Ok(s.clone()),
            _ => {
                let msg = hid::read_prefix(&self.hid, &[0x67, 0x02])?;
                let s = parse_status(&msg).context("malformed fan report")?;
                self.last = Some((s.clone(), Instant::now()));
                Ok(s)
            }
        }
    }
}

impl Driver for ControlHub {
    fn describe(&self) -> Device {
        Device {
            name: MODEL.name.into(),
            firmware: self.firmware.clone(),
            verified: true,
            fans: (1..=PORTS)
                .map(|n| FanChannel {
                    id: format!("fan{n}"),
                    name: format!("Fan {n}"),
                    device_curves: false,
                    ..Default::default()
                })
                .collect(),
            lights: (1..=PORTS)
                .map(|n| {
                    let a = self.accessories.get(n - 1).cloned().unwrap_or_default();
                    LightChannel { id: format!("led{n}"), name: format!("LED {n}"), leds: a.leds(), accessories: a.names() }
                })
                .collect(),
            ..Default::default()
        }
    }

    fn sensors(&self) -> Vec<(&'static str, &'static str, Unit)> {
        Vec::new()
    }

    fn poll(&mut self) -> Result<Reading> {
        let s = self.status()?;
        let fans = s
            .fans
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let connected = p.mode != FanMode::None;
                (FAN_IDS[i], FanReading { rpm: Some(p.rpm), duty: Some(p.duty), connected: Some(connected) })
            })
            .collect();
        Ok(Reading { sensors: Vec::new(), fans })
    }

    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
        let mut ports = Vec::new();
        for &(fan, duty) in duties {
            ports.push((port(fan, "fan").with_context(|| format!("no fan {fan:?}; ports are fan1 to fan5"))?, duty));
        }
        if !ports.is_empty() {
            hid::write(&self.hid, &duties_report(&ports))?;
        }
        Ok(())
    }

    fn set_lighting(&mut self, light: &str, lighting: &Lighting) -> Result<()> {
        let Some(i) = port(light, "led") else { bail!("no light {light:?}; ports are led1 to led5") };
        let leds = self.accessories.get(i).map_or(hue2::DEFAULT_LEDS, |a| a.leds());
        hue2::write(&self.hid, LED_CHANNELS[i], lighting, leds)
    }
}

const FAN_IDS: [&str; PORTS] = ["fan1", "fan2", "fan3", "fan4", "fan5"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duties_build_a_mask() {
        assert_eq!(duties_report(&[(0, 30), (2, 100), (4, 150)]), vec![0x62, 0x01, 0b10101, 30, 0, 100, 0, 100]);
        // liquidctl: [0x62, 0x01, 1 << cid, 0, 0, 0, 0, 0] with msg[cid + 3] = duty
        assert_eq!(duties_report(&[(2, 55)]), vec![0x62, 0x01, 0x04, 0, 0, 55, 0, 0]);
    }

    #[test]
    fn parses_status() {
        let mut m = vec![0u8; 64];
        m[0] = 0x67;
        m[1] = 0x02;
        m[16] = 2;
        m[17] = 1;
        m[24..26].copy_from_slice(&1200u16.to_le_bytes());
        m[26..28].copy_from_slice(&800u16.to_le_bytes());
        m[40] = 45;
        m[41] = 30;
        m[56] = 27;
        let s = parse_status(&m).unwrap();
        assert_eq!(s.fans[0], Port { mode: FanMode::Pwm, rpm: 1200, duty: 45 });
        assert_eq!(s.fans[1], Port { mode: FanMode::Dc, rpm: 800, duty: 30 });
        assert_eq!(s.fans[2].mode, FanMode::None);
        assert_eq!(s.noise_db, 27);
    }

    #[test]
    fn ports_are_one_based() {
        assert_eq!(port("fan1", "fan"), Some(0));
        assert_eq!(port("led5", "led"), Some(4));
        assert_eq!(port("fan6", "fan"), None);
        assert_eq!(port("fan0", "fan"), None);
    }
}
