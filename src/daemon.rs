//! `camless run`: applies the configuration, then keeps host-driven fans on their
//! curves, once per `interval_ms`.
//!
//! The Kraken runs liquid-temperature curves on its own, so those are stored once
//! and need no host at all. CPU- and GPU-driven channels, and every Control Hub fan,
//! need the host: the hub only takes fixed duties.

use crate::config::{Config, Source};
use crate::curve::{Curve, Smoother};
use crate::device::{ControlHub, Kraken};
use crate::protocol::*;
use crate::sensors::{Readings, Sensors};
use hidapi::HidApi;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const REOPEN_EVERY: Duration = Duration::from_secs(5);
/// Duties are sent again now and then, in case the hub reset without a USB event.
const RESEND_EVERY: Duration = Duration::from_secs(60);
/// The LCD falls back to the liquid screen when frames stop for this long.
const FRAME_STALE: Duration = Duration::from_secs(5);
/// How long frames are dropped after the LCD fails.
const LCD_BACKOFF: Duration = Duration::from_secs(10);
/// Liquid at or above this sends every fan and the pump to full speed.
const LIQUID_CRITICAL_C: f32 = 55.0;
/// Status reads that may fail in a row before the device is reopened.
const MAX_FAILURES: u32 = 3;

/// The floor under every table camless stores on the Kraken, by liquid temperature.
/// If camless stops or hangs, the cooler still speeds up on its own as liquid warms.
fn safety_floor(liquid_c: u8) -> u8 {
    match liquid_c {
        ..=45 => 0,
        55.. => 100,
        t => (t - 45) * 10,
    }
}

pub fn kraken_table(curve: &Curve) -> [u8; KRAKEN_CURVE_POINTS] {
    let mut table = curve.kraken_table();
    for (i, d) in table.iter_mut().enumerate() {
        *d = (*d).max(safety_floor(KRAKEN_CURVE_FIRST_C + i as u8));
    }
    table
}

pub enum Event {
    /// One Q565-encoded 640×640 frame for the LCD.
    Frame(Vec<u8>),
}

/// What the local API serves.
#[derive(Clone, Default)]
pub struct Snapshot {
    pub cpu_name: String,
    pub gpu_name: Option<String>,
    pub readings: Readings,
    pub kraken: Option<KrakenStatus>,
    pub hub: Option<HubStatus>,
    pub duties: Vec<(String, u8)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    Kraken(KrakenChannel),
    /// Index 0..5 of a Control Hub fan port.
    Hub(usize),
}

struct Channel {
    target: Target,
    label: String,
    source: Source,
    curve: Curve,
    duty: u8,
    sent: Option<u8>,
}

impl Channel {
    fn want(&self, cpu: Option<f32>, gpu: Option<f32>, liquid: Option<f32>, failsafe: u8) -> u8 {
        let temp = match self.source {
            Source::Fixed => Some(0.0),
            Source::Cpu => cpu,
            Source::Gpu => gpu,
            Source::Liquid => liquid,
        };
        temp.map_or(failsafe, |t| self.curve.duty_at(t))
    }
}

struct Slot<T> {
    dev: Option<T>,
    next_try: Instant,
    last_error: Option<String>,
    failures: u32,
}

impl<T> Slot<T> {
    fn new() -> Slot<T> {
        Slot { dev: None, next_try: Instant::now(), last_error: None, failures: 0 }
    }
}

pub struct Daemon {
    cfg: Config,
    api: HidApi,
    sensors: Sensors,
    kraken: Slot<Kraken>,
    hub: Slot<ControlHub>,
    wants_kraken: bool,
    wants_hub: bool,
    channels: Vec<Channel>,
    cpu_smooth: Smoother,
    gpu_smooth: Smoother,
    hub_sent: Option<([Option<u8>; HUB_FAN_CHANNELS], Instant)>,
    lcd_api: bool,
    last_frame: Option<Instant>,
    lcd_error_at: Option<Instant>,
    shared: Arc<Mutex<Snapshot>>,
    start: Instant,
    verbose: bool,
}

impl Daemon {
    pub fn new(cfg: Config, sensors: Sensors, shared: Arc<Mutex<Snapshot>>, verbose: bool) -> anyhow::Result<Daemon> {
        let mut channels = Vec::new();
        for (cooling, channel, label) in [
            (&cfg.pump, KrakenChannel::Pump, "pump"),
            (&cfg.radiator, KrakenChannel::Fan, "radiator"),
        ] {
            // Liquid and fixed curves are stored on the cooler and need no host.
            if let Some(c) = cooling.as_ref().filter(|c| matches!(c.source, Source::Cpu | Source::Gpu)) {
                channels.push(Channel {
                    target: Target::Kraken(channel),
                    label: label.into(),
                    source: c.source,
                    curve: c.curve(),
                    duty: cfg.failsafe_duty,
                    sent: None,
                });
            }
        }
        for f in &cfg.hub_fans {
            let label = if f.name.is_empty() { format!("hub fan {}", f.port) } else { format!("hub fan {} ({})", f.port, f.name) };
            channels.push(Channel {
                target: Target::Hub(f.port - 1),
                label,
                source: f.cooling.source,
                curve: f.cooling.curve(),
                duty: cfg.failsafe_duty,
                sent: None,
            });
        }
        let wants_kraken = cfg.pump.is_some()
            || cfg.radiator.is_some()
            || cfg.ring.is_some()
            || cfg.external.is_some()
            || cfg.lcd.is_some()
            || channels.iter().any(|c| c.source == Source::Liquid);
        let wants_hub = !cfg.hub_fans.is_empty() || !cfg.hub_leds.is_empty();
        let dt = Duration::from_millis(cfg.interval_ms).as_secs_f32();
        // Per-second rates: rise within a few seconds, settle over half a minute.
        let rate = |per_s: f32| 1.0 - (1.0 - per_s).powf(dt);
        let daemon = Daemon {
            lcd_api: cfg.lcd.as_ref().is_some_and(|l| l.mode == "api"),
            api: HidApi::new()?,
            kraken: Slot::new(),
            hub: Slot::new(),
            wants_kraken,
            wants_hub,
            channels,
            cpu_smooth: Smoother::new(rate(0.35), rate(0.08)),
            gpu_smooth: Smoother::new(rate(0.35), rate(0.08)),
            hub_sent: None,
            last_frame: None,
            lcd_error_at: None,
            shared,
            start: Instant::now(),
            verbose,
            sensors,
            cfg,
        };
        for c in &daemon.channels {
            let missing = match c.source {
                Source::Cpu => !daemon.sensors.has_cpu_temp(),
                Source::Gpu => daemon.sensors.gpu_name.is_none(),
                _ => false,
            };
            if missing {
                daemon.log(&format!(
                    "{}: no {:?} temperature, holding {} %",
                    c.label, c.source, daemon.cfg.failsafe_duty
                ));
            }
        }
        Ok(daemon)
    }

    fn log(&self, msg: &str) {
        eprintln!("[{:>6}s] {msg}", self.start.elapsed().as_secs());
    }

    pub fn run(&mut self, events: &Receiver<Event>, stop: &AtomicBool) {
        let interval = Duration::from_millis(self.cfg.interval_ms.max(250));
        let mut next = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            let now = Instant::now();
            if now >= next {
                self.tick();
                next = now + interval;
                continue;
            }
            match events.recv_timeout((next - now).min(Duration::from_millis(250))) {
                Ok(Event::Frame(mut frame)) => {
                    // Only the newest frame matters.
                    while let Ok(Event::Frame(newer)) = events.try_recv() {
                        frame = newer;
                    }
                    self.frame(&frame);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => std::thread::sleep(Duration::from_millis(250)),
            }
        }
        self.shutdown();
    }

    fn tick(&mut self) {
        self.ensure_kraken();
        self.ensure_hub();
        let readings = self.sensors.read();
        let kraken = self.kraken_status();
        let hub = self.hub_status();
        let cpu = readings.cpu_temp_c.map(|t| self.cpu_smooth.update(t));
        let gpu = readings.gpu_temp_c.map(|t| self.gpu_smooth.update(t));
        let liquid = kraken.as_ref().map(|s| s.liquid_c);
        let critical = liquid.is_some_and(|l| l >= LIQUID_CRITICAL_C);
        let failsafe = self.cfg.failsafe_duty;
        for c in &mut self.channels {
            c.duty = if critical { 100 } else { c.want(cpu, gpu, liquid, failsafe) };
        }
        self.drive_kraken();
        self.drive_hub(false);
        self.lcd_watchdog();
        if self.verbose {
            let duties: Vec<String> = self.channels.iter().map(|c| format!("{} {} %", c.label, c.duty)).collect();
            self.log(&format!(
                "cpu {} gpu {} liquid {} | {}",
                fmt_temp(cpu),
                fmt_temp(gpu),
                fmt_temp(liquid),
                duties.join(", ")
            ));
        }
        let mut s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        s.cpu_name = self.sensors.cpu_name.clone();
        s.gpu_name = self.sensors.gpu_name.clone();
        s.readings = readings;
        s.kraken = kraken;
        s.hub = hub;
        s.duties = self.channels.iter().map(|c| (c.label.clone(), c.duty)).collect();
    }

    fn ensure_kraken(&mut self) {
        if !self.wants_kraken || self.kraken.dev.is_some() || Instant::now() < self.kraken.next_try {
            return;
        }
        self.kraken.next_try = Instant::now() + REOPEN_EVERY;
        let _ = self.api.refresh_devices();
        match self.open_kraken() {
            Ok(k) => {
                self.log(&format!("kraken: ready, firmware {}", fmt_version(k.firmware)));
                self.kraken = Slot { dev: Some(k), ..Slot::new() };
                for c in self.channels.iter_mut().filter(|c| matches!(c.target, Target::Kraken(_))) {
                    c.sent = None;
                }
            }
            Err(e) => self.report_open_error(true, e),
        }
    }

    fn open_kraken(&self) -> anyhow::Result<Kraken> {
        let mut k = Kraken::open(&self.api)?;
        k.initialize()?;
        for (cooling, channel) in [(&self.cfg.pump, KrakenChannel::Pump), (&self.cfg.radiator, KrakenChannel::Fan)] {
            if let Some(c) = cooling.as_ref().filter(|c| matches!(c.source, Source::Liquid | Source::Fixed)) {
                k.set_curve(channel, &kraken_table(&c.curve()))?;
            }
        }
        if let Some(l) = &self.cfg.ring {
            k.set_ring(&l.to_lighting()?)?;
        }
        if let Some(l) = &self.cfg.external {
            k.set_external(&l.to_lighting()?)?;
        }
        if let Some(lcd) = &self.cfg.lcd {
            // A screen that refuses a mode is not worth losing fan control over.
            let shown = k.lcd_settings(lcd.brightness, (lcd.orientation / 90) as u8).and_then(|_| k.lcd_liquid());
            if let Err(e) = shown {
                self.log(&format!("kraken: lcd: {e:#}"));
            }
        }
        Ok(k)
    }

    fn ensure_hub(&mut self) {
        if !self.wants_hub || self.hub.dev.is_some() || Instant::now() < self.hub.next_try {
            return;
        }
        self.hub.next_try = Instant::now() + REOPEN_EVERY;
        let _ = self.api.refresh_devices();
        let opened = (|| -> anyhow::Result<ControlHub> {
            let mut h = ControlHub::open(&self.api)?;
            h.initialize()?;
            for l in &self.cfg.hub_leds {
                h.set_lighting(l.port, &l.light.to_lighting()?)?;
            }
            Ok(h)
        })();
        match opened {
            Ok(h) => {
                self.log(&format!("hub: ready, firmware {}", fmt_version(h.firmware)));
                self.hub = Slot { dev: Some(h), ..Slot::new() };
                self.hub_sent = None;
            }
            Err(e) => self.report_open_error(false, e),
        }
    }

    fn report_open_error(&mut self, kraken: bool, e: anyhow::Error) {
        let msg = format!("{e:#}");
        let (name, last) = if kraken {
            ("kraken", &mut self.kraken.last_error)
        } else {
            ("hub", &mut self.hub.last_error)
        };
        // A device held by CAM fails the same way every few seconds; say it once.
        if last.as_ref() != Some(&msg) {
            *last = Some(msg.clone());
            self.log(&format!("{name}: {msg}; retrying every {} s", REOPEN_EVERY.as_secs()));
        }
    }

    fn kraken_status(&mut self) -> Option<KrakenStatus> {
        let result = self.kraken.dev.as_ref()?.status();
        match result {
            Ok(s) => {
                self.kraken.failures = 0;
                Some(s)
            }
            Err(e) => {
                self.kraken.failures += 1;
                if self.kraken.failures >= MAX_FAILURES {
                    self.log(&format!("kraken: lost ({e:#}), reopening"));
                    self.kraken.dev = None;
                }
                None
            }
        }
    }

    fn hub_status(&mut self) -> Option<HubStatus> {
        let result = self.hub.dev.as_mut()?.status();
        match result {
            Ok(s) => {
                self.hub.failures = 0;
                Some(s)
            }
            Err(e) => {
                self.hub.failures += 1;
                if self.hub.failures >= MAX_FAILURES {
                    self.log(&format!("hub: lost ({e:#}), reopening"));
                    self.hub.dev = None;
                }
                None
            }
        }
    }

    fn drive_kraken(&mut self) {
        let Some(k) = &self.kraken.dev else { return };
        let mut failed = None;
        for c in &mut self.channels {
            let Target::Kraken(channel) = c.target else { continue };
            if c.sent == Some(c.duty) {
                continue;
            }
            match k.set_curve(channel, &kraken_table(&Curve::flat(c.duty))) {
                Ok(()) => c.sent = Some(c.duty),
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        if let Some(e) = failed {
            self.log(&format!("kraken: write failed ({e:#}), reopening"));
            self.kraken.dev = None;
        }
    }

    fn drive_hub(&mut self, force: bool) {
        let Some(h) = &self.hub.dev else { return };
        let mut duties = [None; HUB_FAN_CHANNELS];
        for c in &self.channels {
            if let Target::Hub(i) = c.target {
                duties[i] = Some(c.duty);
            }
        }
        if duties.iter().all(Option::is_none) {
            return;
        }
        let fresh = matches!(self.hub_sent, Some((d, at)) if d == duties && at.elapsed() < RESEND_EVERY);
        if fresh && !force {
            return;
        }
        match h.set_duties(&duties) {
            Ok(()) => self.hub_sent = Some((duties, Instant::now())),
            Err(e) => {
                self.log(&format!("hub: write failed ({e:#}), reopening"));
                self.hub.dev = None;
            }
        }
    }

    fn frame(&mut self, q565: &[u8]) {
        if !self.lcd_api {
            return;
        }
        self.last_frame = Some(Instant::now());
        // Frames share this thread with the fans, so a cooler that stops answering LCD
        // commands gets a rest instead of a timeout on every frame.
        if self.lcd_error_at.is_some_and(|t| t.elapsed() < LCD_BACKOFF) {
            return;
        }
        let Some(k) = self.kraken.dev.as_mut() else { return };
        if let Err(e) = k.lcd_frame(q565) {
            self.lcd_error_at = Some(Instant::now());
            self.log(&format!("kraken: lcd frame: {e:#}; dropping frames for {} s", LCD_BACKOFF.as_secs()));
        }
    }

    fn lcd_watchdog(&mut self) {
        let stale = self.last_frame.is_none_or(|t| t.elapsed() > FRAME_STALE);
        let Some(k) = self.kraken.dev.as_mut() else { return };
        if self.lcd_api && stale && k.is_streaming() {
            if let Err(e) = k.lcd_liquid() {
                self.log(&format!("kraken: lcd: {e:#}"));
            }
        }
    }

    /// Leaves the devices in a state that is safe without camless running.
    fn shutdown(&mut self) {
        let failsafe = self.cfg.failsafe_duty;
        for c in &mut self.channels {
            if c.source != Source::Fixed {
                c.duty = failsafe;
            }
        }
        self.drive_kraken();
        self.drive_hub(true);
        if let Some(k) = self.kraken.dev.as_mut() {
            if k.is_streaming() {
                let _ = k.lcd_liquid();
            }
        }
        self.log(&format!(
            "stopped; host-driven fans left at {failsafe} %, the Kraken still speeds up from 45 °C liquid"
        ));
    }
}

fn fmt_temp(t: Option<f32>) -> String {
    t.map_or("-".into(), |t| format!("{t:.1} °C"))
}

fn fmt_version(v: Option<(u8, u8, u8)>) -> String {
    v.map_or("?".into(), |(a, b, c)| format!("{a}.{b}.{c}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safety_floor_ramps_between_45_and_55() {
        assert_eq!(safety_floor(20), 0);
        assert_eq!(safety_floor(45), 0);
        assert_eq!(safety_floor(50), 50);
        assert_eq!(safety_floor(55), 100);
        let t = kraken_table(&Curve::flat(30));
        assert_eq!((t[0], t[29], t[30], t[34], t[39]), (30, 40, 50, 90, 100));
    }

    #[test]
    fn missing_temperature_uses_failsafe() {
        let c = Channel {
            target: Target::Hub(0),
            label: String::new(),
            source: Source::Cpu,
            curve: Curve::new(&[(40.0, 20), (80.0, 100)]),
            duty: 0,
            sent: None,
        };
        assert_eq!(c.want(None, Some(90.0), Some(30.0), 70), 70);
        assert_eq!(c.want(Some(60.0), None, None, 70), 60);
    }
}
