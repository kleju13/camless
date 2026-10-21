mod api;
mod backdrop;
mod cdp;
mod daemon;
mod drivers;
mod fps;
mod games;
mod history;
mod mcp;
mod q565;
mod report;
mod sensors;
mod web;

use anyhow::{bail, Context, Result};
use camless_core::cam_import;
use camless_core::config::{self, Config, CurveSpec};
use camless_core::curve::Engine;
use drivers::{Driver, NZXT_VID};
use hidapi::HidApi;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const USAGE: &str = "camless: fan, pump, lighting and LCD control for NZXT devices without CAM

Usage:
  camless status               Open every supported device and print its channels,
                               sensors and readings (the ids the config uses)
  camless list                 Show the NZXT HID devices that are connected
  camless watch [seconds]      Listen to input reports without sending anything
                               (default 10 s; CAM must not hold the devices)
  camless sensors              Read CPU, GPU and memory sensors once
{FPS}  camless mcp [config]         An MCP server on stdin and stdout for AI assistants:
                               sensors, history, game sessions and fan curves; asks
                               the running service and writes its config
  camless report [config]      Print a hardware report for a GitHub issue: the NZXT
                               devices with their USB descriptors, and what the
                               service knows; read only, no commands are sent
                               to them
  camless import-cam [dir]     Print a camless.toml built from CAM's active profiles
  camless check [config]       Validate a config and show what each fan follows
  camless run [config]         Apply the config and keep fans on their curves
  camless lcd liquid           Put the LCD's own liquid temperature screen back
  camless lcd test [seconds]   Stream a test pattern to the LCD (default 20 s)
  camless lcd page URL         Render a web page the way the LCD shows it and save
                               the last frame as a PNG, without opening a device

{CONFIG}
Options:
  --verbose                    run: print sensors and duties every interval
  --demo                       run: simulated devices instead of USB, for trying
                               the app without hardware
  --for SECONDS                run: stop after this long, the same way as Ctrl+C
  --raw                        watch: print every report in hex
  --json                       report: the issue title and the report as JSON
  -o FILE                      import-cam: write to FILE instead of stdout;
                               lcd page: the PNG (default lcd-page.png)
  --for SECONDS                lcd page: how long to render (default 6)

{NOTES}";

#[cfg(windows)]
const PLATFORM_USAGE: [(&str, &str); 3] = [
    (
        "{FPS}",
        "  camless fps [seconds]        Print the frame rate of the program in front once a
                               second (default 10 s; needs administrator rights)
",
    ),
    (
        "{CONFIG}",
        "The config defaults to camless.toml next to camless.exe if there is one, else
%APPDATA%\\camless\\camless.toml. `run` applies changes to it while running.
",
    ),
    (
        "{NOTES}",
        "The NZXT CAM app holds the devices exclusively; quit it first (CAM Service can
keep running).
CPU temperature needs PawnIO, AMDFamily17.bin and administrator rights.
",
    ),
];

#[cfg(not(windows))]
const PLATFORM_USAGE: [(&str, &str); 3] = [
    ("{FPS}", ""),
    (
        "{CONFIG}",
        "The config defaults to camless.toml next to camless if there is one, else
~/.config/camless/camless.toml. `run` applies changes to it while running.
",
    ),
    (
        "{NOTES}",
        "camless reaches the devices through hidraw and usbfs; the udev rules that come
with it let the signed-in user do that without root. A kernel driver such as
nzxt-kraken3 may stay loaded, but nothing else should set the same fans:
CoolerControl, liquidctl or fan curves written to hwmon.
",
    ),
];

fn usage() -> String {
    PLATFORM_USAGE.iter().fold(USAGE.to_string(), |text, (key, value)| text.replace(key, value))
}

static STOP: AtomicBool = AtomicBool::new(false);
static STOPPED: AtomicBool = AtomicBool::new(false);

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let option = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1));
    let positional: Vec<&str> = {
        let mut out = Vec::new();
        let mut it = args.iter().skip(1);
        while let Some(a) = it.next() {
            match a.as_str() {
                "-o" | "--for" => {
                    it.next();
                }
                a if a.starts_with("--") => {}
                a => out.push(a),
            }
        }
        out
    };
    let config_path = || positional.first().map(PathBuf::from).unwrap_or_else(config::default_path);
    match args.first().map(String::as_str) {
        Some("status") => status(&config_path()),
        Some("list") => list(),
        Some("watch") => {
            let secs = positional.first().map(|s| s.parse()).transpose().context("watch takes seconds")?;
            watch(Duration::from_secs(secs.unwrap_or(10)), flag("--raw"))
        }
        Some("sensors") => sensors(),
        #[cfg(windows)]
        Some("fps") => {
            let secs = positional.first().map(|s| s.parse()).transpose().context("fps takes seconds")?;
            fps_watch(Duration::from_secs(secs.unwrap_or(10)))
        }
        Some("mcp") => mcp::serve(config_path()),
        Some("report") => report(&config_path(), flag("--json")),
        Some("import-cam") => {
            let dir = positional.first().map(PathBuf::from).unwrap_or_else(cam_import::default_dir);
            import_cam(&dir, option("-o").map(PathBuf::from))
        }
        Some("check") => check(&config_path()),
        Some("run") => {
            let limit = option("--for").map(|s| s.parse().map(Duration::from_secs)).transpose().context("--for takes seconds")?;
            run(config_path(), flag("--verbose"), flag("--demo"), limit)
        }
        Some("lcd") if positional.first() == Some(&"liquid") => {
            let (id, mut dev) = open_lcd()?;
            dev.lcd().context("no LCD")?.show_builtin()?;
            println!("{id}: showing its own screen");
            Ok(())
        }
        Some("lcd") if positional.first() == Some(&"page") => {
            let url = positional.get(1).context("lcd page takes a URL")?;
            let secs = option("--for").map(|s| s.parse()).transpose().context("--for takes seconds")?;
            let out = option("-o").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("lcd-page.png"));
            lcd_page(url, &out, Duration::from_secs(secs.unwrap_or(6)))
        }
        Some("lcd") if positional.first() == Some(&"test") => {
            let secs = positional.get(1).map(|s| s.parse()).transpose().context("lcd test takes seconds")?;
            lcd_test(Duration::from_secs(secs.unwrap_or(20)))
        }
        _ => {
            print!("{}", usage());
            Ok(())
        }
    }
}

#[cfg(windows)]
fn beside_exe(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

#[cfg(windows)]
fn pawnio_module(configured: Option<&PathBuf>) -> std::result::Result<Vec<u8>, String> {
    let path = configured.cloned().unwrap_or_else(|| beside_exe("AMDFamily17.bin"));
    std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))
}

/// PawnIO is Windows only; Linux reads CPU temperature from hwmon.
#[cfg(not(windows))]
fn pawnio_module(_configured: Option<&PathBuf>) -> std::result::Result<Vec<u8>, String> {
    Err("PawnIO is for Windows".into())
}

fn sensors() -> Result<()> {
    let mut s = sensors::Sensors::open(pawnio_module(None));
    s.read();
    std::thread::sleep(Duration::from_secs(1));
    let r = s.read();
    println!("cpu: {}", s.cpu_name);
    println!("gpu: {}", s.gpu_name.as_deref().unwrap_or("-"));
    for x in r.list(s.gpu_name.is_some()) {
        let value = x.value.map_or("-".into(), |v| format!("{v:.1} {}", x.unit.symbol()));
        println!("  {:<10} {value:<10} {}", x.id, x.name);
    }
    Ok(())
}

/// The config only says where the service listens and whether other brands are
/// on; a missing or broken one means the defaults.
fn report(config: &Path, json: bool) -> Result<()> {
    let cfg = Config::load(config).unwrap_or_default();
    let api = &cfg.api;
    let r = report::collect(api.enabled.then_some(api.listen.as_str()), cfg.devices.other_brands);
    if json {
        println!("{}", serde_json::to_string(&r)?);
    } else {
        print!("{}", r.text);
    }
    Ok(())
}

fn import_cam(dir: &Path, out: Option<PathBuf>) -> Result<()> {
    let imported = cam_import::import(dir)?;
    for n in &imported.notes {
        eprintln!("note: {n}");
    }
    let text = format!(
        "# Generated by `camless import-cam` from {}\n# Temperatures in °C, duties in %.\n\n{}",
        dir.display(),
        imported.config.to_toml()?
    );
    match out {
        Some(path) => {
            if path.exists() {
                bail!("{} already exists; not overwriting it", path.display());
            }
            std::fs::write(&path, text)?;
            eprintln!("wrote {}", path.display());
        }
        None => print!("{text}"),
    }
    Ok(())
}

/// Shows what `run` would do with a config, without opening any device.
fn check(path: &Path) -> Result<()> {
    let cfg = Config::load(path)?;
    println!("{}: valid", path.display());
    println!("every {} ms; fans without a reading hold {} %", cfg.interval_ms, cfg.failsafe_duty);
    for c in &cfg.curves {
        let what = match c {
            CurveSpec::Graph(g) => format!("graph on {}: {:?}", g.sensor, g.points),
            CurveSpec::Fixed(f) => format!("fixed {} %", f.duty),
            CurveSpec::Mix(m) => format!("{:?} of {}", m.function, m.curves.join(", ")).to_lowercase(),
            CurveSpec::Trigger(t) => {
                format!("trigger on {}: {} % until {}, {} % from {}", t.sensor, t.idle_duty, t.idle_at, t.load_duty, t.load_at)
            }
        };
        println!("curve {:?}: {what}", c.name());
    }
    let engine = Engine::new(&cfg);
    for f in &cfg.fans {
        let Some(curve) = &f.curve else {
            println!("fan {}: left alone", f.channel);
            continue;
        };
        let device = f.channel.split('/').next().unwrap_or_default();
        let local = engine.device_curve(&f.channel).filter(|d| {
            d.sensor.as_deref().is_none_or(|s| s.strip_prefix(device).is_some_and(|rest| rest.starts_with('.')))
        });
        let place = if local.is_some() { "the device if it can store curves, else camless" } else { "camless" };
        println!("fan {}: curve {curve:?}, run by {place}, limits {}..{} %", f.channel, f.min, f.max);
        if let Some(d) = local {
            let samples: Vec<String> = (20..=60).step_by(5).map(|t| format!("{t}°{}%", d.duty_at(t as f32))).collect();
            println!("  on the device: {}", samples.join(" "));
        }
    }
    for l in &cfg.lights {
        let name = if l.name.is_empty() { String::new() } else { format!(" ({})", l.name) };
        if l.managed() {
            println!("light {}{name}: {:?}", l.channel, l.to_lighting()?);
        } else {
            println!("light {}{name}: left as it is", l.channel);
        }
    }
    for l in &cfg.lcds {
        println!("lcd {}: {:?} at {} %, {}°", l.device, l.mode, l.brightness, l.orientation);
    }
    Ok(())
}

/// The first supported device that has an LCD.
fn open_lcd() -> Result<(String, Box<dyn Driver>)> {
    let api = HidApi::new()?;
    let (found, _) = drivers::scan(&api, false);
    let mut errors = Vec::new();
    for f in found {
        match f.open(&api) {
            Ok(mut dev) => {
                if dev.lcd().is_some() {
                    return Ok((f.id, dev));
                }
            }
            Err(e) => errors.push(format!("{}: {e:#}", f.id)),
        }
    }
    bail!("no device with an LCD could be opened{}", errors.iter().map(|e| format!("\n  {e}")).collect::<String>())
}

/// Streams red, green, blue and white quarters with a black bar moving down, five
/// frames a second, then puts the device's own screen back.
fn lcd_test(duration: Duration) -> Result<()> {
    let (id, mut dev) = open_lcd()?;
    let lcd = dev.lcd().context("no LCD")?;
    let (w, h) = lcd.size();
    let (w, h) = (usize::from(w), usize::from(h));
    println!("{id}: lcd {w}×{h}");
    catch_stop();
    let start = Instant::now();
    let (mut sent, mut slowest) = (0usize, Duration::ZERO);
    let result = loop {
        if start.elapsed() >= duration || STOP.load(Ordering::SeqCst) {
            break Ok(());
        }
        let bar = (sent * 16) % h;
        let pixels: Vec<u16> = (0..w * h)
            .map(|i| match (i % w < w / 2, i / w) {
                (_, y) if (bar..bar + 24).contains(&y) => 0x0000,
                (true, y) if y < h / 2 => 0xf800,
                (false, y) if y < h / 2 => 0x07e0,
                (true, _) => 0x001f,
                (false, _) => 0xffff,
            })
            .collect();
        let frame = q565::encode(w as u16, h as u16, &pixels);
        let t = Instant::now();
        if let Err(e) = lcd.frame(&frame) {
            break Err(e.context(format!("frame {sent}")));
        }
        slowest = slowest.max(t.elapsed());
        if sent == 0 {
            println!("first frame: {} B of Q565 in {} ms", frame.len(), t.elapsed().as_millis());
        }
        sent += 1;
        std::thread::sleep(Duration::from_millis(200));
    };
    println!("sent {sent} frames, slowest {} ms", slowest.as_millis());
    let back = lcd.show_builtin();
    STOPPED.store(true, Ordering::SeqCst);
    result.and(back)
}

/// Opens every supported device once and prints what the config can refer to;
/// other brands' only with the config's `[devices] other_brands` on, the
/// motherboard's headers with `motherboard`. It sets nothing, except that it gives
/// back the headers a camless that stopped without handing them back had taken.
fn status(config: &Path) -> Result<()> {
    let api = HidApi::new()?;
    let devices = Config::load(config).map(|c| c.devices).unwrap_or_default();
    let (mut found, unsupported) = drivers::scan(&api, devices.other_brands);
    found.extend(drivers::scan_host(&devices));
    if found.is_empty() {
        println!("no supported NZXT device found");
    }
    for f in &found {
        let mut dev = match f.open(&api) {
            Ok(d) => d,
            Err(e) => {
                println!("{}: {} — {e:#}", f.id, f.model.name);
                continue;
            }
        };
        let info = dev.describe();
        let verified = match (info.verified, info.experimental) {
            (true, _) => "",
            (false, true) => ", experimental",
            (false, false) => ", not verified on hardware",
        };
        println!("{}: {}, firmware {}{verified}", f.id, info.name, info.firmware.as_deref().unwrap_or("?"));
        let reading = dev.poll();
        if let Err(e) = &reading {
            println!("  no reading: {e:#}");
        }
        let reading = reading.unwrap_or_default();
        for (id, name, unit) in dev.sensors() {
            let v = reading.sensors.iter().find(|(x, _)| *x == id).map_or("-".into(), |(_, v)| format!("{v:.1} {}", unit.symbol()));
            println!("  sensor {}.{id:<12} {v:<10} {name}", f.id);
        }
        for c in &info.fans {
            let r = reading.fans.iter().find(|(x, _)| *x == c.id).map(|(_, r)| *r).unwrap_or_default();
            let plugged = match r.connected {
                Some(false) => " (nothing plugged in)",
                _ => "",
            };
            let rpm = r.rpm.map_or("-".into(), |v| format!("{v} rpm"));
            let duty = r.duty.map_or("-".into(), |v| format!("{v} %"));
            let stored = if c.device_curves { ", can store curves" } else { "" };
            println!("  fan    {}/{:<10} {rpm:<10} {duty:<6} {}{stored}{plugged}", f.id, c.id, c.name);
        }
        for l in &info.lights {
            let acc = if l.accessories.is_empty() { "nothing reported".into() } else { l.accessories.join(", ") };
            println!("  light  {}/{:<10} {} LEDs: {acc}", f.id, l.id, l.leds);
        }
        if let Some(l) = &info.lcd {
            println!("  lcd    {}×{}{}", l.width, l.height, if l.round { ", round" } else { "" });
        }
    }
    for u in &unsupported {
        println!("{:04x}:{:04x} {}: no driver yet (see CONTRIBUTING.md)", u.vid, u.pid, u.product);
    }
    let mut s = sensors::Sensors::open(pawnio_module(None));
    s.read();
    std::thread::sleep(Duration::from_millis(500));
    for x in s.read().list(s.gpu_name.is_some()) {
        let value = x.value.map_or("-".into(), |v| format!("{v:.1} {}", x.unit.symbol()));
        println!("sensor {:<10} {value:<10} {}", x.id, x.name);
    }
    Ok(())
}

/// Ctrl+C, closing the console, logoff and shutdown stop camless the way `--for`
/// does, so the fans are put in their safe state first.
#[cfg(windows)]
fn catch_stop() {
    unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(on_console_event), 1) };
}

#[cfg(windows)]
unsafe extern "system" fn on_console_event(kind: u32) -> windows_sys::core::BOOL {
    STOP.store(true, Ordering::SeqCst);
    // On close, logoff and shutdown Windows ends the process when this returns, so
    // wait for the fans to be put in their safe state first.
    if kind >= windows_sys::Win32::System::Console::CTRL_CLOSE_EVENT {
        let t = Instant::now();
        while !STOPPED.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(4) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    1
}

/// SIGINT (Ctrl+C), SIGTERM (systemd stopping the service, shutdown) and SIGHUP
/// stop camless the way `--for` does; it puts the fans in their safe state and exits.
#[cfg(not(windows))]
fn catch_stop() {
    extern "C" fn on_signal(_: libc::c_int) {
        STOP.store(true, Ordering::SeqCst);
    }
    let handler: extern "C" fn(libc::c_int) = on_signal;
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handler as libc::sighandler_t;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }
}

fn run(path: PathBuf, verbose: bool, demo: bool, limit: Option<Duration>) -> Result<()> {
    // The GUI edits the file at this path, from another working directory.
    let path = std::path::absolute(&path).unwrap_or(path);
    eprintln!("camless: {}", path.display());
    // A missing or broken config still starts the daemon, so the GUI can show the
    // devices and write a config; nothing is managed until there is a valid one.
    let (cfg, cfg_error) = match Config::load(&path) {
        Ok(cfg) => (cfg, None),
        Err(_) if !path.exists() => {
            eprintln!("camless: no config yet; only reading devices until {} exists", path.display());
            (Config::default(), None)
        }
        Err(e) => {
            eprintln!("camless: config not applied, nothing is managed until it is fixed: {e:#}");
            (Config::default(), Some(format!("{e:#}")))
        }
    };
    catch_stop();
    if let Some(limit) = limit {
        std::thread::spawn(move || {
            std::thread::sleep(limit);
            STOP.store(true, Ordering::SeqCst);
        });
    }
    let shared = Arc::new(Mutex::new(daemon::Snapshot::default()));
    // Simulated readings stay out of the files.
    let history_dir = path.parent().filter(|_| !demo).map(|d| d.join("history"));
    let games = games::Games::new(history_dir.as_deref(), web::is_elevated(), demo);
    let history = Arc::new(Mutex::new(history::History::new(history_dir)));
    // Two frames in flight at most; the API answers 429 beyond that.
    let (tx, rx) = mpsc::sync_channel(2);
    if cfg.api.enabled {
        api::spawn(&cfg.api.listen, shared.clone(), history.clone(), games.log(), tx.clone(), &STOP)
            .context("starting the local API")?;
        eprintln!("camless: api on http://{}", cfg.api.listen);
    }
    let sensors = sensors::Sensors::open(pawnio_module(cfg.pawnio_module.as_ref()));
    if demo {
        eprintln!("camless: demo mode, simulated devices only");
    }
    let mut d = daemon::Daemon::new(path, cfg, cfg_error, sensors, shared, history, games, tx.clone(), verbose, demo)?;
    d.run(&rx, &STOP);
    drop(tx);
    STOPPED.store(true, Ordering::SeqCst);
    Ok(())
}

/// Prints the frame rate of the program in front once a second, counted the way
/// game sessions count it.
#[cfg(windows)]
fn fps_watch(duration: Duration) -> Result<()> {
    let capture = fps::Capture::start("camless-fps-check").context("starting the ETW session")?;
    // Ctrl+C stops the session instead of leaving it running.
    catch_stop();
    eprintln!("frames of the program in front, counted 2 s late, for {} s", duration.as_secs());
    let (mut pid, mut meter) = (0, fps::Meter::default());
    let start = Instant::now();
    while start.elapsed() < duration && !STOP.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_secs(1));
        let front = games::foreground();
        if front != pid {
            (pid, meter) = (front, fps::Meter::default());
            capture.follow(pid);
        }
        meter.add(&capture.take());
        let (mut n, mut total) = (0u32, 0i64);
        meter.settle(fps::now(), |t| (n, total) = (n + 1, total + t));
        let program = games::program(pid).unwrap_or_else(|| format!("process {pid}"));
        let times = if n > 0 { format!(", frames of {:.1} ms on average", total as f64 / f64::from(n) / 1e4) } else { String::new() };
        println!("{program}: {} fps{times}", meter.fps());
    }
    STOPPED.store(true, Ordering::SeqCst);
    Ok(())
}

/// Renders `url` as the LCD would show it, with this machine's sensor readings, and
/// saves the last frame after it went through the LCD's 16-bit encoding.
fn lcd_page(url: &str, out: &Path, duration: Duration) -> Result<()> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        bail!("{url} is not an http or https URL");
    }
    let mut sensors = sensors::Sensors::open(Err("not read for a page preview".into()));
    let snapshot = daemon::Snapshot {
        readings: sensors.read(),
        cpu_name: sensors.cpu_name.clone(),
        gpu_name: sensors.gpu_name.clone(),
        ..Default::default()
    };
    let (tx, rx) = mpsc::sync_channel(4);
    let page = web::Page { url: url.into(), fps: 10, width: 640, height: 640, round: true };
    eprintln!("rendering {} at 640×640 for {} s", web::lcd_url(url), duration.as_secs());
    let renderer = web::Renderer::start("preview", page, Arc::new(Mutex::new(snapshot)), tx);
    let until = Instant::now() + duration;
    let (mut last, mut frames) = (None, 0);
    while Instant::now() < until {
        if let Ok(daemon::Event::Frame { q565, .. }) = rx.recv_timeout(Duration::from_millis(100)) {
            last = Some(q565);
            frames += 1;
        }
    }
    let state = renderer.state();
    drop(renderer);
    let Some(q565) = last else {
        bail!("no frame in {} s{}", duration.as_secs(), state.error.map(|e| format!(": {e}")).unwrap_or_default());
    };
    let (w, h, pixels) = q565::decode(&q565).context("decoding the last frame")?;
    let rgb: Vec<u8> = pixels
        .iter()
        .flat_map(|&p| {
            let (r, g, b) = ((p >> 11) as u8, (p >> 5 & 0x3f) as u8, (p & 0x1f) as u8);
            [r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2]
        })
        .collect();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(out)?), w.into(), h.into());
    enc.set_color(png::ColorType::Rgb);
    enc.write_header()?.write_image_data(&rgb)?;
    let browser = state.browser.unwrap_or_default();
    let background = match state.background {
        0 => String::new(),
        1 => " over a still background from the page".into(),
        n => format!(" over a background of {n} frames from the page"),
    };
    eprintln!("{browser}: {frames} frames, the last at {:.1} fps{background}; wrote {}", state.fps, out.display());
    Ok(())
}

fn list() -> Result<()> {
    let api = HidApi::new()?;
    for d in api.device_list().filter(|d| d.vendor_id() == NZXT_VID) {
        let driver = drivers::MODELS
            .iter()
            .find(|m| m.pid == d.product_id() && m.interface.is_none_or(|i| i == d.interface_number()))
            .map_or("no driver", |m| m.slug);
        println!(
            "{:04x}:{:04x} if={} {} [{driver}] | {}",
            d.vendor_id(),
            d.product_id(),
            d.interface_number(),
            d.product_string().unwrap_or("?"),
            d.path().to_string_lossy()
        );
    }
    Ok(())
}

/// Reads input reports only, so it can listen in on another program that drives the
/// devices with shared access. CAM opens them exclusively.
fn watch(duration: Duration, raw: bool) -> Result<()> {
    let api = HidApi::new()?;
    let mut devices = Vec::new();
    let mut seen = Vec::new();
    for d in api.device_list().filter(|d| d.vendor_id() == NZXT_VID) {
        let key = (d.product_id(), d.interface_number(), d.serial_number().map(String::from));
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        let name = format!("{:04x}:{:04x}/{}", d.vendor_id(), d.product_id(), d.interface_number());
        match api.open_path(d.path()) {
            Ok(dev) => devices.push((name, d.product_id(), dev)),
            Err(e) => eprintln!("{name}: cannot open: {e}"),
        }
    }
    if devices.is_empty() {
        bail!("no NZXT HID device could be opened");
    }
    let start = Instant::now();
    let mut buf = [0u8; 64];
    while start.elapsed() < duration && !devices.is_empty() {
        let mut failed = None;
        for (idx, (name, pid, dev)) in devices.iter().enumerate() {
            let n = match dev.read_timeout(&mut buf, 100) {
                Ok(n) => n,
                Err(e) => {
                    // Another process holds the device without sharing read access.
                    eprintln!("{name}: read failed: {e}");
                    failed = Some(idx);
                    break;
                }
            };
            if n == 0 {
                continue;
            }
            let msg = &buf[..n];
            let t = start.elapsed().as_secs_f32();
            match describe_report(*pid, msg) {
                Some(text) => println!("{t:6.2}s {name}: {text}"),
                None => println!("{t:6.2}s {name}: report {:02x} {:02x} ({n} B)", msg[0], msg.get(1).copied().unwrap_or(0)),
            }
            if raw {
                println!("         {}", hex(msg));
            }
        }
        if let Some(idx) = failed {
            devices.remove(idx);
        }
    }
    Ok(())
}

/// Decodes the status reports of supported devices.
fn describe_report(pid: u16, msg: &[u8]) -> Option<String> {
    use drivers::{control_hub, kraken_elite_v2};
    if pid == kraken_elite_v2::MODEL.pid {
        let s = kraken_elite_v2::parse_status(msg)?;
        return Some(format!(
            "liquid {:.1} °C, pump {} rpm ({} %), fan {} rpm ({} %)",
            s.liquid_c, s.pump_rpm, s.pump_duty, s.fan_rpm, s.fan_duty
        ));
    }
    if pid == control_hub::MODEL.pid {
        let s = control_hub::parse_status(msg)?;
        let fans: Vec<String> =
            s.fans.iter().enumerate().map(|(i, f)| format!("fan{} {:?} {} rpm {} %", i + 1, f.mode, f.rpm, f.duty)).collect();
        return Some(format!("{} | noise {} dB", fans.join(", "), s.noise_db));
    }
    None
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
}
