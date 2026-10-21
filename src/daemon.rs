//! `camless run`: finds NZXT devices, applies the config, and keeps fans on their
//! curves once per `interval_ms`.
//!
//! A curve a device can run by itself (a fixed duty, or a graph on the device's own
//! sensor, like a Kraken's liquid temperature) is stored on it and needs no host.
//! The curve engine drives everything else from host and device sensors.
//!
//! The config file is watched: a valid change applies within one interval, an
//! invalid one is reported in the status and the previous config keeps running.

use crate::drivers::{self, Driver, Found, Reading};
use crate::games::Games;
use crate::history::History;
use crate::sensors::{Readings, Sensors};
use crate::web;
use camless_core::config::{self, Config, Lcd, LcdMode};
use camless_core::curve::{DeviceCurve, Duty, Engine};
use camless_core::lighting::Light;
use camless_core::status::{CurveValue, Device, FanValue, Sensor, ServiceInfo, Status, Unit, Unsupported, WebState};
use hidapi::HidApi;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const RESCAN_EVERY: Duration = Duration::from_secs(5);
/// Duties are sent again now and then, in case a device reset without a USB event.
const RESEND_EVERY: Duration = Duration::from_secs(60);
/// An LCD in frames or web mode goes back to its own screen when frames stop for
/// this long.
const FRAME_STALE: Duration = Duration::from_secs(5);
/// How long frames are dropped after the LCD fails.
const LCD_BACKOFF: Duration = Duration::from_secs(10);
/// Liquid at or above this sends every managed fan and pump to full speed.
const LIQUID_CRITICAL_C: f32 = 55.0;
/// Polls that may fail in a row before a device is reopened.
const MAX_FAILURES: u32 = 3;

pub enum Event {
    /// One Q565-encoded frame for the LCD of `device`, which must be in frames or web
    /// mode.
    Frame { device: String, q565: Vec<u8> },
}

/// What the API serves.
#[derive(Clone, Default)]
pub struct Snapshot {
    pub status: Status,
    pub readings: Readings,
    pub cpu_name: String,
    pub gpu_name: Option<String>,
    /// The connected LCD in frames mode: (device id, width, height).
    pub frame_target: Option<(String, u16, u16)>,
}

struct Slot {
    found: Found,
    dev: Option<Box<dyn Driver>>,
    /// The last `describe()`, with readings filled in.
    info: Device,
    sensors: Vec<(&'static str, &'static str, Unit)>,
    reading: Option<Reading>,
    next_try: Instant,
    last_error: Option<String>,
    failures: u32,
    applied: bool,
    /// Fans running a curve stored on the device.
    on_device: HashSet<String>,
    /// What the device was last given, so a config change only resends what it
    /// changes: resent lighting restarts effects and takes 150 ms per packet.
    lights_sent: HashMap<String, Light>,
    lcd_sent: Option<Lcd>,
    curves_sent: HashMap<String, DeviceCurve>,
    sent: HashMap<String, (u8, Instant)>,
    last_frame: Option<Instant>,
    lcd_error_at: Option<Instant>,
}

impl Slot {
    fn new(found: Found) -> Slot {
        let info = Device { id: found.id.clone(), name: found.model.name.into(), ..Default::default() };
        Slot {
            found,
            dev: None,
            info,
            sensors: Vec::new(),
            reading: None,
            next_try: Instant::now(),
            last_error: None,
            failures: 0,
            applied: false,
            on_device: HashSet::new(),
            lights_sent: HashMap::new(),
            lcd_sent: None,
            curves_sent: HashMap::new(),
            sent: HashMap::new(),
            last_frame: None,
            lcd_error_at: None,
        }
    }

    fn id(&self) -> &str {
        &self.found.id
    }
}

/// The channel part of `<device>/<channel>` if it is on device `id`. A free function,
/// so it borrows only the id while the driver is borrowed mutably.
fn local<'a>(id: &str, channel: &'a str) -> Option<&'a str> {
    channel.split_once('/').filter(|(d, _)| *d == id).map(|(_, c)| c)
}

/// The duties of a device's host-driven fans among `duties`.
fn host_batch<'a>(s: &Slot, duties: &'a [(String, Duty)]) -> Vec<(&'a str, u8)> {
    duties
        .iter()
        .filter_map(|(c, d)| Some((local(&s.found.id, c)?, d.value()?)))
        .filter(|(f, _)| !s.on_device.contains(*f) && s.info.fans.iter().any(|x| x.id == *f))
        .collect()
}

pub struct Daemon {
    cfg: Config,
    cfg_path: PathBuf,
    cfg_stamp: Vec<Option<SystemTime>>,
    cfg_error: Option<String>,
    engine: Engine,
    /// None in a demo, which touches no HID device.
    api: Option<HidApi>,
    sensors: Sensors,
    /// Runs with administrator rights; read once, it cannot change.
    elevated: bool,
    slots: Vec<Slot>,
    unsupported: Vec<Unsupported>,
    next_scan: Instant,
    last_tick: Option<Instant>,
    shared: Arc<Mutex<Snapshot>>,
    start: Instant,
    verbose: bool,
    /// Curves already reported as missing their sensor, so a reload does not repeat it.
    warned: HashSet<String>,
    /// Where renderers send their frames: the same queue as the API's.
    frames: SyncSender<Event>,
    /// Web pages being rendered, by device.
    pages: HashMap<String, Rendering>,
    /// Sensors and fan speeds over time, for the API.
    history: Arc<Mutex<History>>,
    /// The game in front and its frame rate.
    games: Games,
}

struct Rendering {
    renderer: web::Renderer,
    logged: Option<String>,
    shown: bool,
}

impl Daemon {
    /// `cfg_error` says why the file at `cfg_path` is not what `cfg` came from.
    pub fn new(
        cfg_path: PathBuf,
        cfg: Config,
        cfg_error: Option<String>,
        sensors: Sensors,
        shared: Arc<Mutex<Snapshot>>,
        history: Arc<Mutex<History>>,
        games: Games,
        frames: SyncSender<Event>,
        verbose: bool,
        demo: bool,
    ) -> anyhow::Result<Daemon> {
        let daemon = Daemon {
            engine: Engine::new(&cfg),
            cfg_stamp: config::stamp(&cfg_path, cfg.profile.as_deref()),
            cfg_path,
            cfg,
            cfg_error,
            api: if demo { None } else { Some(HidApi::new()?) },
            sensors,
            elevated: web::is_elevated(),
            slots: Vec::new(),
            unsupported: Vec::new(),
            next_scan: Instant::now(),
            last_tick: None,
            shared,
            start: Instant::now(),
            verbose,
            warned: HashSet::new(),
            frames,
            pages: HashMap::new(),
            history,
            games,
        };
        let mut daemon = daemon;
        daemon.warn_missing_sensors();
        Ok(daemon)
    }

    fn log(&self, msg: &str) {
        eprintln!("[{:>6}s] {msg}", self.start.elapsed().as_secs());
    }

    fn warn_missing_sensors(&mut self) {
        let host = self.sensors_now(&Readings::default());
        let mut warned = HashSet::new();
        for c in &self.cfg.curves {
            let Some(sensor) = c.sensor() else { continue };
            let is_host = host.iter().any(|s| s.id == sensor);
            let why = if sensor == "cpu.temp" && !self.sensors.has_cpu_temp() {
                crate::sensors::NO_CPU_TEMP
            } else if sensor.starts_with("gpu.") && !is_host {
                if cfg!(windows) { "no NVIDIA GPU sensors" } else { "no NVIDIA or AMD GPU sensors" }
            } else {
                continue;
            };
            let key = format!("{} on {sensor}", c.name());
            if !self.warned.contains(&key) {
                self.log(&format!("curve {:?}: {why}; its fans hold {} %", c.name(), self.cfg.failsafe_duty));
            }
            warned.insert(key);
        }
        self.warned = warned;
    }

    pub fn run(&mut self, events: &Receiver<Event>, stop: &AtomicBool) {
        let mut next = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            let now = Instant::now();
            if now >= next {
                self.tick();
                next = now + Duration::from_millis(self.cfg.interval_ms.max(250));
                continue;
            }
            match events.recv_timeout((next - now).min(Duration::from_millis(250))) {
                Ok(Event::Frame { mut device, mut q565 }) => {
                    // Only the newest frame matters.
                    while let Ok(Event::Frame { device: d, q565: q }) = events.try_recv() {
                        (device, q565) = (d, q);
                    }
                    self.frame(&device, &q565);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => std::thread::sleep(Duration::from_millis(250)),
            }
        }
        self.shutdown();
    }

    fn tick(&mut self) {
        let now = Instant::now();
        let interval = Duration::from_millis(self.cfg.interval_ms).as_secs_f32();
        let dt = self.last_tick.map_or(interval, |t| now.duration_since(t).as_secs_f32()).min(5.0);
        self.last_tick = Some(now);
        self.reload();
        self.scan();
        self.open_devices();
        for i in 0..self.slots.len() {
            self.poll(i);
            if self.slots[i].dev.is_some() && !self.slots[i].applied {
                self.apply(i);
            }
        }
        let readings = self.sensors.read();
        let sensors = self.sensors_now(&readings);
        let values: HashMap<&str, f32> = sensors.iter().filter_map(|s| Some((s.id.as_str(), s.value?))).collect();
        let critical = self.slots.iter().any(|s| {
            s.reading.as_ref().is_some_and(|r| r.sensors.iter().any(|&(id, v)| id == "liquid" && v >= LIQUID_CRITICAL_C))
        });
        self.engine.step(&|id| values.get(id).copied(), dt, critical);
        self.drive(false);
        self.render_pages();
        self.lcd_watchdog();
        if self.verbose {
            let shown: Vec<String> = sensors.iter().filter_map(|s| Some(format!("{} {:.1}", s.id, s.value?))).collect();
            let duties: Vec<String> =
                self.engine.duties().iter().filter_map(|(c, d)| Some(format!("{c} {} %", d.value()?))).collect();
            self.log(&format!("{} | {}", shown.join(", "), duties.join(", ")));
        }
        let fps = if self.cfg.games.enabled {
            self.games.tick(dt, &sensors, &self.cfg.games)
        } else {
            // Off, or just turned off: an open session ends, and the ETW session with it.
            self.games.finish();
            None
        };
        for n in self.games.notes() {
            self.log(&n);
        }
        self.record(&sensors, fps);
        self.publish(readings, sensors);
    }

    /// Every sensor, the speed and duty of every fan on a device that answers, and the
    /// frame rate of the game in front.
    fn record(&self, sensors: &[Sensor], fps: Option<f32>) {
        let mut values: Vec<(String, f32)> = sensors.iter().filter_map(|s| Some((s.id.clone(), s.value?))).collect();
        values.extend(fps.map(|f| ("fps".to_string(), f)));
        for s in self.slots.iter().filter(|s| s.dev.is_some() && s.reading.is_some()) {
            for f in &s.info.fans {
                if let Some(rpm) = f.rpm {
                    values.push((format!("{}/{}.rpm", s.id(), f.id), rpm.into()));
                }
                if let Some(duty) = f.duty {
                    values.push((format!("{}/{}.duty", s.id(), f.id), duty.into()));
                }
            }
        }
        let ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        self.history.lock().unwrap_or_else(|e| e.into_inner()).record(ms, &values);
    }

    fn reload(&mut self) {
        let now = config::stamp(&self.cfg_path, self.cfg.profile.as_deref());
        if now == self.cfg_stamp {
            return;
        }
        self.cfg_stamp = now;
        match Config::load(&self.cfg_path) {
            Ok(cfg) if cfg == self.cfg => self.cfg_error = None,
            Ok(cfg) => {
                match &cfg.profile {
                    Some(p) if cfg.profile != self.cfg.profile => {
                        self.log(&format!("profile {p:?}, applying it"));
                        // From now on the new profile's file is the one to watch.
                        self.cfg_stamp = config::stamp(&self.cfg_path, Some(p));
                    }
                    _ => self.log("config changed, applying it"),
                }
                self.engine = Engine::new(&cfg);
                if cfg.devices != self.cfg.devices {
                    // Other brands' devices come and go with the setting at once.
                    self.next_scan = Instant::now();
                }
                self.cfg = cfg;
                self.cfg_error = None;
                for s in &mut self.slots {
                    s.applied = false;
                }
                self.warn_missing_sensors();
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if self.cfg_error.as_ref() != Some(&msg) {
                    self.log(&format!("config not applied, the previous one keeps running: {msg}"));
                }
                self.cfg_error = Some(msg);
            }
        }
    }

    fn scan(&mut self) {
        if Instant::now() < self.next_scan {
            return;
        }
        self.next_scan = Instant::now() + RESCAN_EVERY;
        let (found, unsupported) = match &mut self.api {
            None => (drivers::demo::found(), Vec::new()),
            Some(api) => {
                let _ = api.refresh_devices();
                drivers::scan(api, self.cfg.devices.other_brands)
            }
        };
        for f in found {
            match self.slots.iter_mut().find(|s| s.found.id == f.id) {
                Some(s) if s.dev.is_none() => s.found = f,
                Some(_) => {}
                None => {
                    self.log(&format!("{}: found {}", f.id, f.model.name));
                    self.slots.push(Slot::new(f));
                }
            }
        }
        for u in &unsupported {
            if !self.unsupported.iter().any(|o| o.pid == u.pid) {
                self.log(&format!("{:04x}:{:04x} {}: no driver yet", u.vid, u.pid, u.product));
            }
        }
        self.unsupported = unsupported;
        self.yield_other_brands();
    }

    /// Lets go of other brands' devices while their brand's app runs, which would
    /// fight camless over them, and for good with `other_brands` off. They go back to
    /// how they run without camless. `Found::open` keeps them closed while the app
    /// runs.
    fn yield_other_brands(&mut self) {
        if !self.slots.iter().any(|s| s.found.model.vendor.is_some()) {
            return;
        }
        let on = self.cfg.devices.other_brands;
        let running = if on { drivers::vendor::running_processes() } else { HashSet::new() };
        let duties = self.safe_duties();
        let mut notes = Vec::new();
        for s in &mut self.slots {
            let Some(vendor) = s.found.model.vendor else { continue };
            let why = if on { vendor.busy(&running) } else { Some("other brands are off".to_string()) };
            let batch = host_batch(s, &duties);
            let (Some(why), Some(dev)) = (why, s.dev.as_mut()) else { continue };
            // A hub with nothing to hand back to keeps these, as when the service stops.
            if !batch.is_empty() {
                let _ = dev.set_duties(&batch);
            }
            let released = match dev.release() {
                Ok(()) => "handed back".to_string(),
                Err(e) => format!("not handed back ({e:#})"),
            };
            notes.push(format!("{}: {why}; {released}", s.found.id));
            s.dev = None;
            s.reading = None;
            s.last_error = Some(why);
            s.next_try = Instant::now() + RESCAN_EVERY;
        }
        if !on {
            self.slots.retain(|s| s.found.model.vendor.is_none());
        }
        for n in notes {
            self.log(&n);
        }
    }

    fn open_devices(&mut self) {
        let now = Instant::now();
        for i in 0..self.slots.len() {
            let s = &mut self.slots[i];
            if s.dev.is_some() || now < s.next_try {
                continue;
            }
            s.next_try = now + RESCAN_EVERY;
            let opened = match &self.api {
                Some(api) => s.found.open(api),
                None => drivers::demo::open(&s.found),
            };
            match opened {
                Ok(dev) => {
                    let mut info = dev.describe();
                    info.id = s.found.id.clone();
                    s.sensors = dev.sensors();
                    s.info = info;
                    s.dev = Some(dev);
                    s.applied = false;
                    s.failures = 0;
                    s.last_error = None;
                    s.sent.clear();
                    s.lights_sent.clear();
                    s.lcd_sent = None;
                    s.curves_sent.clear();
                    let fw = s.info.firmware.clone().unwrap_or_else(|| "?".into());
                    let id = s.found.id.clone();
                    self.log(&format!("{id}: ready, firmware {fw}"));
                }
                Err(e) => {
                    let msg = format!("{e:#}");
                    // A device held by CAM fails the same way every few seconds; say it once.
                    if s.last_error.as_ref() != Some(&msg) {
                        s.last_error = Some(msg.clone());
                        let id = s.found.id.clone();
                        self.log(&format!("{id}: {msg}; retrying every {} s", RESCAN_EVERY.as_secs()));
                    }
                }
            }
        }
    }

    fn poll(&mut self, i: usize) {
        let s = &mut self.slots[i];
        let Some(dev) = s.dev.as_mut() else { return };
        match dev.poll() {
            Ok(r) => {
                s.failures = 0;
                for f in &mut s.info.fans {
                    let got = r.fans.iter().find(|(id, _)| *id == f.id).map(|(_, x)| *x).unwrap_or_default();
                    (f.rpm, f.duty, f.connected) = (got.rpm, got.duty, got.connected);
                }
                s.reading = Some(r);
            }
            Err(e) => {
                s.failures += 1;
                if s.failures >= MAX_FAILURES {
                    s.dev = None;
                    s.reading = None;
                    s.last_error = Some(format!("{e:#}"));
                    let id = s.found.id.clone();
                    self.log(&format!("{id}: lost ({e:#}), reopening"));
                }
            }
        }
    }

    /// Lighting, the LCD and stored curves for one device, as far as they differ from
    /// what it was last given. Config mistakes are logged and skipped; I/O errors
    /// show up in the next poll.
    fn apply(&mut self, i: usize) {
        let mut notes = Vec::new();
        let s = &mut self.slots[i];
        let id = s.found.id.clone();
        let Some(dev) = s.dev.as_mut() else { return };
        for l in self.cfg.lights.iter().filter(|l| l.managed()) {
            let Some(light) = local(&s.found.id, &l.channel) else { continue };
            // A new name alone changes nothing on the device.
            let sent = Light { name: String::new(), ..l.clone() };
            if s.lights_sent.get(light) == Some(&sent) {
                continue;
            }
            if !s.info.lights.iter().any(|x| x.id == light) {
                notes.push(format!("light {}: this device has no {light:?}", l.channel));
                continue;
            }
            match l.to_lighting().and_then(|lighting| dev.set_lighting(light, &lighting)) {
                Ok(()) => {
                    s.lights_sent.insert(light.to_string(), sent);
                }
                Err(e) => notes.push(format!("light {}: {e:#}", l.channel)),
            }
        }
        let lcd_config = self.cfg.lcds.iter().find(|c| c.device == id);
        let frames = s.info.lcd.as_ref().is_some_and(|l| l.frames);
        if let Some(c) = lcd_config.filter(|c| s.lcd_sent.as_ref() != Some(*c)) {
            match dev.lcd() {
                None => notes.push(format!("lcd {id}: this device has no screen")),
                Some(lcd) => {
                    let shown = lcd.settings(c.brightness, (c.orientation / 90) as u8).and_then(|_| match c.mode {
                        LcdMode::Liquid => lcd.show_builtin(),
                        LcdMode::Frames | LcdMode::Web if !frames => {
                            notes.push(format!(
                                "lcd {id}: this cooler takes pictures only into its flash memory, which camless does not write, so it keeps its own display"
                            ));
                            lcd.show_builtin()
                        }
                        // The stream takes over when the first frame arrives.
                        LcdMode::Frames | LcdMode::Web => Ok(()),
                    });
                    match shown {
                        Ok(()) => s.lcd_sent = Some(c.clone()),
                        // A screen that refuses a mode is not worth losing fan control over.
                        Err(e) => notes.push(format!("lcd {id}: {e:#}")),
                    }
                }
            }
        }
        if lcd_config.is_none() {
            s.lcd_sent = None;
            // Without an entry the watchdog no longer looks at this screen, so a stream
            // that stopped would leave its last frame up for good.
            if let Some(lcd) = dev.lcd().filter(|l| l.is_streaming()) {
                if let Err(e) = lcd.show_builtin() {
                    notes.push(format!("lcd {id}: {e:#}"));
                }
            }
        }
        let before = std::mem::take(&mut s.on_device);
        let mut stored = HashMap::new();
        for f in &self.cfg.fans {
            let Some(fan) = local(&s.found.id, &f.channel) else { continue };
            if !s.info.fans.iter().any(|x| x.id == fan) {
                notes.push(format!("fan {}: this device has no {fan:?}", f.channel));
                continue;
            }
            let Some(curve) = self.engine.device_curve(&f.channel) else { continue };
            // The device can only use its own sensors.
            let local = match &curve.sensor {
                None => Some(None),
                Some(sensor) => sensor.strip_prefix(&format!("{id}.")).map(Some),
            };
            let Some(local) = local else { continue };
            if s.curves_sent.get(fan) == Some(&curve) && before.contains(fan) {
                s.on_device.insert(fan.to_string());
                stored.insert(fan.to_string(), curve);
                continue;
            }
            match dev.store_curve(fan, local, &|v| curve.duty_at(v)) {
                Ok(true) => {
                    s.on_device.insert(fan.to_string());
                    stored.insert(fan.to_string(), curve);
                }
                Ok(false) => {}
                Err(e) => notes.push(format!("fan {}: storing its curve: {e:#}", f.channel)),
            }
        }
        s.curves_sent = stored;
        // Fans that left the device are driven from the host from now on.
        s.sent.clear();
        s.applied = true;
        if s.on_device != before {
            let mut on_device: Vec<String> = s.on_device.iter().map(|f| format!("{id}/{f}")).collect();
            on_device.sort();
            if on_device.is_empty() {
                notes.push(format!("{id}: no curves run on the device"));
            } else {
                notes.push(format!("running on the device: {}", on_device.join(", ")));
            }
        }
        for n in notes {
            self.log(&n);
        }
    }

    fn drive(&mut self, force: bool) {
        let duties = self.engine.duties();
        for i in 0..self.slots.len() {
            let s = &mut self.slots[i];
            let Some(dev) = s.dev.as_mut() else { continue };
            let mut batch: Vec<(String, u8)> = Vec::new();
            for (channel, duty) in &duties {
                let Some(fan) = local(&s.found.id, channel) else { continue };
                let Some(d) = duty.value() else { continue };
                if s.on_device.contains(fan) || !s.info.fans.iter().any(|x| x.id == fan) {
                    continue;
                }
                let fresh = s.sent.get(fan).is_some_and(|(sent, at)| *sent == d && at.elapsed() < RESEND_EVERY);
                if force || !fresh {
                    batch.push((fan.to_string(), d));
                }
            }
            if batch.is_empty() {
                continue;
            }
            let refs: Vec<(&str, u8)> = batch.iter().map(|(f, d)| (f.as_str(), *d)).collect();
            match dev.set_duties(&refs) {
                Ok(()) => {
                    for (f, d) in batch {
                        s.sent.insert(f, (d, Instant::now()));
                    }
                }
                Err(e) => {
                    s.dev = None;
                    s.last_error = Some(format!("{e:#}"));
                    let id = s.found.id.clone();
                    self.log(&format!("{id}: write failed ({e:#}), reopening"));
                }
            }
        }
    }

    fn frame(&mut self, device: &str, q565: &[u8]) {
        if !self.cfg.lcds.iter().any(|c| c.device == device && matches!(c.mode, LcdMode::Frames | LcdMode::Web)) {
            return;
        }
        let Some(s) = self.slots.iter_mut().find(|s| s.found.id == device) else { return };
        s.last_frame = Some(Instant::now());
        // Frames share this thread with the fans, so a device that stops answering
        // LCD commands gets a rest instead of a timeout on every frame.
        if s.lcd_error_at.is_some_and(|t| t.elapsed() < LCD_BACKOFF) {
            return;
        }
        let Some(lcd) = s.dev.as_mut().and_then(|d| d.lcd()) else { return };
        if let Err(e) = lcd.frame(q565) {
            s.lcd_error_at = Some(Instant::now());
            let id = s.found.id.clone();
            self.log(&format!("{id}: lcd frame: {e:#}; dropping frames for {} s", LCD_BACKOFF.as_secs()));
        }
    }

    fn lcd_watchdog(&mut self) {
        let mut errors = Vec::new();
        for c in self.cfg.lcds.iter().filter(|c| matches!(c.mode, LcdMode::Frames | LcdMode::Web)) {
            let Some(s) = self.slots.iter_mut().find(|s| s.found.id == c.device) else { continue };
            let stale = s.last_frame.is_none_or(|t| t.elapsed() > FRAME_STALE);
            let Some(lcd) = s.dev.as_mut().and_then(|d| d.lcd()) else { continue };
            if stale && lcd.is_streaming() {
                if let Err(e) = lcd.show_builtin() {
                    errors.push(format!("{}: lcd: {e:#}", c.device));
                }
            }
        }
        for e in errors {
            self.log(&e);
        }
    }

    /// A renderer for every connected LCD in web mode, restarted when its page changes.
    fn render_pages(&mut self) {
        let mut want = HashMap::new();
        for c in self.cfg.lcds.iter().filter(|c| c.mode == LcdMode::Web) {
            let Some(s) = self.slots.iter().find(|s| s.found.id == c.device && s.dev.is_some()) else { continue };
            let Some(lcd) = s.info.lcd.as_ref().filter(|l| l.frames) else { continue };
            let page = web::Page { url: c.url.clone(), fps: c.fps, width: lcd.width, height: lcd.height, round: lcd.round };
            want.insert(c.device.clone(), page);
        }
        self.pages.retain(|device, r| want.get(device) == Some(&r.renderer.page));
        let mut notes = Vec::new();
        for (device, page) in want {
            if !self.pages.contains_key(&device) {
                notes.push(format!("{device}: opening {} for the LCD", page.url));
                let renderer = web::Renderer::start(&device, page, self.shared.clone(), self.frames.clone());
                self.pages.insert(device, Rendering { renderer, logged: None, shown: false });
            }
        }
        for (device, r) in &mut self.pages {
            let state = r.renderer.state();
            if state.error != r.logged {
                if let Some(e) = &state.error {
                    notes.push(format!("{device}: web page: {e}; trying again every 10 s"));
                }
                r.logged = state.error.clone();
            }
            if state.fps > 0.0 && !r.shown {
                r.shown = true;
                let browser = state.browser.unwrap_or_default();
                notes.push(format!("{device}: showing {} ({browser}, up to {} fps)", r.renderer.page.url, r.renderer.page.fps));
            }
        }
        for n in notes {
            self.log(&n);
        }
    }

    /// Host sensors, then every device sensor as `<device>.<sensor>`.
    fn sensors_now(&self, readings: &Readings) -> Vec<Sensor> {
        let mut out = readings.list(self.sensors.gpu_name.is_some());
        for s in &self.slots {
            for &(id, name, unit) in &s.sensors {
                let value = s.reading.as_ref().and_then(|r| r.sensors.iter().find(|(x, _)| *x == id).map(|(_, v)| *v));
                out.push(Sensor { id: format!("{}.{id}", s.id()), name: format!("{name} ({})", s.info.name), unit, value });
            }
        }
        out
    }

    fn publish(&self, readings: Readings, sensors: Vec<Sensor>) {
        let devices = self
            .slots
            .iter()
            .map(|s| {
                let mut d = s.info.clone();
                d.connected = s.dev.is_some();
                d.error = if s.dev.is_some() { None } else { s.last_error.clone() };
                d.web = self.pages.get(s.id()).map(|r| {
                    let state = r.renderer.state();
                    WebState {
                        url: r.renderer.page.url.clone(),
                        browser: state.browser,
                        fps: state.fps,
                        error: state.error,
                        background_frames: state.background as u32,
                    }
                });
                d
            })
            .collect();
        let fans = self
            .engine
            .duties()
            .into_iter()
            .map(|(channel, duty)| FanValue {
                on_device: self.slots.iter().any(|s| local(&s.found.id, channel).is_some_and(|f| s.on_device.contains(f))),
                failsafe: matches!(duty, Duty::Failsafe(_)),
                duty: duty.value(),
                channel: channel.to_string(),
            })
            .collect();
        let curves = self
            .cfg
            .curves
            .iter()
            .map(|c| CurveValue {
                name: c.name().to_string(),
                input: self.engine.curve_input(c.name()),
                duty: self.engine.curve_value(c.name()),
            })
            .collect();
        let status = Status {
            version: env!("CARGO_PKG_VERSION").into(),
            config_path: self.cfg_path.display().to_string(),
            config_error: self.cfg_error.clone(),
            devices,
            unsupported: self.unsupported.clone(),
            sensors,
            curves,
            fans,
            service: Some(ServiceInfo {
                pid: std::process::id(),
                elevated: self.elevated,
                cpu_sensor_error: self.sensors.cpu_error.clone(),
                fps_error: if self.cfg.games.enabled { self.games.error() } else { None },
            }),
            game: self.games.now(),
        };
        let frame_target = self.cfg.lcds.iter().filter(|c| c.mode == LcdMode::Frames).find_map(|c| {
            let s = self.slots.iter().find(|s| s.found.id == c.device && s.dev.is_some())?;
            let lcd = s.info.lcd.as_ref().filter(|l| l.frames)?;
            Some((c.device.clone(), lcd.width, lcd.height))
        });
        let mut snap = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        *snap = Snapshot {
            status,
            readings,
            cpu_name: self.sensors.cpu_name.clone(),
            gpu_name: self.sensors.gpu_name.clone(),
            frame_target,
        };
    }

    /// What every channel is left at without camless: the failsafe duty, or a fixed
    /// curve's own.
    fn safe_duties(&self) -> Vec<(String, Duty)> {
        let failsafe = self.cfg.failsafe_duty;
        let fixed: HashMap<String, u8> = self
            .engine
            .duties()
            .iter()
            .filter_map(|(c, d)| {
                let curve = self.engine.device_curve(c)?;
                Some((c.to_string(), curve.sensor.is_none().then_some(d.value()?)?))
            })
            .collect();
        self.engine
            .duties()
            .into_iter()
            .map(|(c, d)| (c.to_string(), if d == Duty::Unmanaged { d } else { Duty::Set(*fixed.get(c).unwrap_or(&failsafe)) }))
            .collect()
    }

    /// Leaves the devices in a state that is safe without camless running: host-driven
    /// fans at the failsafe duty (fixed ones at theirs), LCDs on their own screen.
    fn shutdown(&mut self) {
        // Browsers close first, so no frame comes after the LCD's own screen.
        self.pages.clear();
        self.games.finish();
        for n in self.games.notes() {
            self.log(&n);
        }
        self.history.lock().unwrap_or_else(|e| e.into_inner()).finish();
        let failsafe = self.cfg.failsafe_duty;
        let duties = self.safe_duties();
        let (mut set, mut handed) = (0, 0);
        for s in &mut self.slots {
            let batch = host_batch(s, &duties);
            let Some(dev) = s.dev.as_mut() else { continue };
            let sent = if !batch.is_empty() && dev.set_duties(&batch).is_ok() { batch.len() } else { 0 };
            // Another brand's hub goes back to its own control or the motherboard's,
            // as when camless lets go of it; the failsafe duties stay if that fails.
            if s.found.model.vendor.is_some() && dev.release().is_ok() {
                handed += 1;
            } else {
                set += sent;
            }
            if let Some(lcd) = dev.lcd() {
                if lcd.is_streaming() {
                    let _ = lcd.show_builtin();
                }
            }
        }
        let mut note = "stopped".to_string();
        if set > 0 {
            note += &format!("; {set} host-driven fans left at {failsafe} % or their fixed duty");
        }
        if handed > 0 {
            note += &format!("; {handed} other brands' hubs handed back");
        }
        self.log(&note);
    }
}
