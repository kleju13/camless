//! Simulated devices for `camless run --demo`: a cooler with a pump, a fan, a liquid
//! sensor, two lights and an LCD, and a hub with five fans and five lights. They let
//! the GUI be developed and shown without hardware, and nothing touches USB.

use super::{Driver, FanReading, Found, Lcd, Model, Reading};
use anyhow::Result;
use camless_core::lighting::Lighting;
use camless_core::status::{Device, FanChannel, LcdInfo, LightChannel, Unit};
use hidapi::HidApi;
use std::ffi::CString;
use std::time::Instant;

const COOLER: Model = Model { vid: 0, pid: 0, interface: None, slug: "demo-cooler", name: "Demo cooler", open: open_cooler };
const HUB: Model = Model { vid: 0, pid: 1, interface: None, slug: "demo-hub", name: "Demo hub", open: open_hub };
static MODELS: [Model; 2] = [COOLER, HUB];

pub fn found() -> Vec<Found> {
    MODELS.iter().map(|m| Found { model: m, id: m.slug.to_string(), path: CString::default() }).collect()
}

/// Opens a simulated device without a HID API, which a demo never needs.
pub fn open(found: &Found) -> Result<Box<dyn Driver>> {
    Ok(if std::ptr::eq(found.model, &MODELS[1]) { hub() } else { cooler() })
}

struct Fan {
    id: &'static str,
    max_rpm: f32,
    duty: f32,
    target: f32,
    /// A curve on the liquid sensor, stored "on the device".
    table: Option<Vec<u8>>,
}

struct Demo {
    model: &'static Model,
    start: Instant,
    last: Instant,
    fans: Vec<Fan>,
    lights: Vec<&'static str>,
    lcd: bool,
    streaming: bool,
}

fn fan(id: &'static str, max_rpm: f32, duty: f32) -> Fan {
    Fan { id, max_rpm, duty, target: duty, table: None }
}

fn open_cooler(_: &HidApi, _: &Found) -> Result<Box<dyn Driver>> {
    Ok(cooler())
}

fn open_hub(_: &HidApi, _: &Found) -> Result<Box<dyn Driver>> {
    Ok(hub())
}

fn cooler() -> Box<dyn Driver> {
    Box::new(Demo {
        model: &MODELS[0],
        start: Instant::now(),
        last: Instant::now(),
        fans: vec![fan("pump", 2800.0, 60.0), fan("fan", 1800.0, 40.0)],
        lights: vec!["ring", "rgb"],
        lcd: true,
        streaming: false,
    })
}

fn hub() -> Box<dyn Driver> {
    Box::new(Demo {
        model: &MODELS[1],
        start: Instant::now(),
        last: Instant::now(),
        fans: ["fan1", "fan2", "fan3", "fan4", "fan5"].into_iter().map(|id| fan(id, 1500.0, 35.0)).collect(),
        lights: vec!["led1", "led2", "led3", "led4", "led5"],
        lcd: false,
        streaming: false,
    })
}

impl Demo {
    /// Liquid drifts between about 28 and 40 °C over a few minutes.
    fn liquid(&self) -> f32 {
        let t = self.start.elapsed().as_secs_f32();
        34.0 + 5.0 * (t / 40.0).sin() + 0.4 * (t / 3.0).sin()
    }
}

impl Driver for Demo {
    fn describe(&self) -> Device {
        Device {
            name: self.model.name.into(),
            firmware: Some("0.0.0".into()),
            verified: true,
            fans: self
                .fans
                .iter()
                .map(|f| FanChannel {
                    id: f.id.into(),
                    name: if f.id == "pump" { "Pump".into() } else { format!("Fan {}", f.id.trim_start_matches("fan")) },
                    pump: f.id == "pump",
                    min_duty: if f.id == "pump" { 20 } else { 0 },
                    device_curves: self.lcd,
                    ..Default::default()
                })
                .collect(),
            lights: self
                .lights
                .iter()
                .map(|l| LightChannel { id: (*l).into(), name: l.to_uppercase(), leds: 24, accessories: vec!["simulated".into()] })
                .collect(),
            lcd: self.lcd.then_some(LcdInfo { width: 640, height: 640, round: true }),
            ..Default::default()
        }
    }

    fn sensors(&self) -> Vec<(&'static str, &'static str, Unit)> {
        if self.lcd {
            vec![("liquid", "Liquid", Unit::Celsius)]
        } else {
            Vec::new()
        }
    }

    fn poll(&mut self) -> Result<Reading> {
        let dt = self.last.elapsed().as_secs_f32();
        self.last = Instant::now();
        let liquid = self.liquid();
        let k = (dt / 1.5).min(1.0);
        let mut fans = Vec::new();
        for (i, f) in self.fans.iter_mut().enumerate() {
            if let Some(t) = &f.table {
                let at = (liquid.round() as usize).clamp(20, 59) - 20;
                f.target = t[at] as f32;
            }
            f.duty += (f.target - f.duty) * k;
            // The hub's third port has nothing plugged in, like many real setups.
            let connected = !(self.model.slug == "demo-hub" && i == 2);
            let rpm = if connected { (f.duty / 100.0 * f.max_rpm) as u16 } else { 0 };
            fans.push((f.id, FanReading { rpm: Some(rpm), duty: Some(f.duty.round() as u8), connected: Some(connected) }));
        }
        let sensors = if self.lcd { vec![("liquid", liquid)] } else { Vec::new() };
        Ok(Reading { sensors, fans })
    }

    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
        for &(id, d) in duties {
            if let Some(f) = self.fans.iter_mut().find(|f| f.id == id) {
                f.target = d as f32;
                f.table = None;
            }
        }
        Ok(())
    }

    fn store_curve(&mut self, fan: &str, sensor: Option<&str>, duty_at: &dyn Fn(f32) -> u8) -> Result<bool> {
        if !self.lcd || !matches!(sensor, None | Some("liquid")) {
            return Ok(false);
        }
        let Some(f) = self.fans.iter_mut().find(|f| f.id == fan) else { return Ok(false) };
        f.table = Some((20..60).map(|t| duty_at(t as f32)).collect());
        Ok(true)
    }

    fn set_lighting(&mut self, _light: &str, _lighting: &Lighting) -> Result<()> {
        Ok(())
    }

    fn lcd(&mut self) -> Option<&mut dyn Lcd> {
        if self.lcd {
            Some(self)
        } else {
            None
        }
    }
}

impl Lcd for Demo {
    fn settings(&mut self, _brightness: u8, _quarter_turns: u8) -> Result<()> {
        Ok(())
    }

    fn show_builtin(&mut self) -> Result<()> {
        self.streaming = false;
        Ok(())
    }

    fn frame(&mut self, _q565: &[u8]) -> Result<()> {
        self.streaming = true;
        Ok(())
    }

    fn is_streaming(&self) -> bool {
        self.streaming
    }

    fn size(&self) -> (u16, u16) {
        (640, 640)
    }
}
