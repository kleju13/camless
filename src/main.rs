mod api;
mod cam_import;
mod config;
mod curve;
mod daemon;
mod device;
mod protocol;
mod q565;
mod sensors;

use anyhow::{bail, Context, Result};
use hidapi::{HidApi, HidDevice};
use protocol::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const USAGE: &str = "camless: pump, fan, lighting and LCD control for NZXT devices without CAM

Usage:
  camless list                 Show the NZXT HID devices that are connected
  camless watch [seconds]      Listen to status reports without sending anything
                               (default 10 s; CAM must not hold the devices)
  camless sensors              Read CPU, GPU and memory sensors once
  camless import-cam [dir]     Print a camless.toml built from CAM's active profiles
  camless check [config]       Validate a config and show what run would send
  camless status               Initialize both devices and print their state
  camless run [config]         Apply the config and keep fans on their curves
                               (default: camless.toml next to camless.exe)
  camless lcd liquid           Put the Kraken's own liquid temperature screen back
  camless lcd test [seconds]   Stream a test pattern to the LCD (default 20 s)

Options:
  --verbose                    run: print temperatures and duties every interval
  --for SECONDS                run: stop after this long, the same way as Ctrl+C
  -o FILE                      import-cam: write to FILE instead of stdout

NZXT CAM and its CAM Service hold both devices exclusively; stop them first.
CPU temperature needs PawnIO, AMDFamily17.bin and administrator rights.
";

static STOP: AtomicBool = AtomicBool::new(false);
static STOPPED: AtomicBool = AtomicBool::new(false);

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
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
    match args.first().map(String::as_str) {
        Some("list") => list(),
        Some("watch") => {
            let secs = positional.first().map(|s| s.parse()).transpose()?.unwrap_or(10);
            watch(Duration::from_secs(secs), flag("--raw"))
        }
        Some("sensors") => sensors(),
        Some("import-cam") => {
            let dir = positional.first().map(PathBuf::from).unwrap_or_else(cam_import::default_dir);
            let out = args.iter().position(|a| a == "-o").and_then(|i| args.get(i + 1)).map(PathBuf::from);
            import_cam(&dir, out)
        }
        Some("status") => status(),
        Some("check") => check(&positional.first().map(PathBuf::from).unwrap_or_else(|| beside_exe("camless.toml"))),
        Some("run") => {
            let path = positional.first().map(PathBuf::from).unwrap_or_else(|| beside_exe("camless.toml"));
            let seconds = args.iter().position(|a| a == "--for").and_then(|i| args.get(i + 1));
            let limit = seconds.map(|s| s.parse().map(Duration::from_secs)).transpose().context("--for takes seconds")?;
            run(path, flag("--verbose"), limit)
        }
        Some("lcd") if positional.first() == Some(&"liquid") => {
            let api = HidApi::new()?;
            let mut k = device::Kraken::open(&api)?;
            k.initialize()?;
            k.lcd_liquid()
        }
        Some("lcd") if positional.first() == Some(&"test") => {
            let secs = positional.get(1).map(|s| s.parse()).transpose().context("lcd test takes seconds")?;
            lcd_test(Duration::from_secs(secs.unwrap_or(20)))
        }
        _ => {
            print!("{USAGE}");
            Ok(())
        }
    }
}

fn beside_exe(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

fn pawnio_module(configured: Option<&PathBuf>) -> Option<Vec<u8>> {
    let path = configured.cloned().unwrap_or_else(|| beside_exe("AMDFamily17.bin"));
    match std::fs::read(&path) {
        Ok(m) => Some(m),
        Err(e) => {
            eprintln!("cpu temperature off: {}: {e}", path.display());
            None
        }
    }
}

fn sensors() -> Result<()> {
    let mut s = sensors::Sensors::open(pawnio_module(None).as_deref());
    s.read();
    std::thread::sleep(Duration::from_secs(1));
    let r = s.read();
    println!("cpu: {}", s.cpu_name);
    println!("gpu: {}", s.gpu_name.as_deref().unwrap_or("-"));
    println!("{}", serde_json::to_string_pretty(&r)?);
    Ok(())
}

fn import_cam(dir: &std::path::Path, out: Option<PathBuf>) -> Result<()> {
    let imported = cam_import::import(dir)?;
    for n in &imported.notes {
        eprintln!("note: {n}");
    }
    let text = format!(
        "# Generated by `camless import-cam` from {}
# Temperatures in °C, duties in %. Sources: liquid, cpu, gpu, fixed.

{}",
        dir.display(),
        toml::to_string(&imported.config)?
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
fn check(path: &std::path::Path) -> Result<()> {
    let cfg = config::Config::load(path)?;
    let source = |c: &config::Cooling| match (c.source, c.duty) {
        (config::Source::Fixed, Some(d)) => format!("fixed {d} %"),
        (s, _) => format!("{s:?} curve {:?}", c.curve),
    };
    for (name, c) in [("pump", &cfg.pump), ("radiator", &cfg.radiator)] {
        let Some(c) = c else { continue };
        let on_device = matches!(c.source, config::Source::Liquid | config::Source::Fixed);
        println!("{name}: {} -> {}", source(c), if on_device { "stored on the Kraken" } else { "driven by camless" });
        if on_device {
            println!("  table 20..59 °C: {:?}", daemon::kraken_table(&c.curve()));
        }
    }
    for f in &cfg.hub_fans {
        println!("hub fan {} {}: {} -> driven by camless", f.port, f.name, source(&f.cooling));
    }
    for (name, l) in [("ring", &cfg.ring), ("external", &cfg.external)] {
        if let Some(l) = l {
            println!("{name}: {:?}", l.to_lighting()?);
        }
    }
    for l in &cfg.hub_leds {
        println!("hub led {}: {:?}", l.port, l.light.to_lighting()?);
    }
    if let Some(l) = &cfg.lcd {
        println!("lcd: {} at {} % brightness, {}°", l.mode, l.brightness, l.orientation);
    }
    println!("failsafe: {} %, every {} ms", cfg.failsafe_duty, cfg.interval_ms);
    Ok(())
}

/// Streams red, green, blue and white quarters with a black bar moving down, five
/// frames a second, then puts the liquid screen back.
fn lcd_test(duration: Duration) -> Result<()> {
    const SIDE: usize = 640;
    let api = HidApi::new()?;
    let mut k = device::Kraken::open(&api)?;
    k.initialize()?;
    println!("kraken: firmware {:?}, lcd {:?}", k.firmware, k.lcd_info);
    unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(on_console_event), 1) };
    let start = Instant::now();
    let (mut sent, mut slowest) = (0usize, Duration::ZERO);
    let result = loop {
        if start.elapsed() >= duration || STOP.load(Ordering::SeqCst) {
            break Ok(());
        }
        let bar = (sent * 16) % SIDE;
        let pixels: Vec<u16> = (0..SIDE * SIDE)
            .map(|i| match (i % SIDE < SIDE / 2, i / SIDE) {
                (_, y) if (bar..bar + 24).contains(&y) => 0x0000,
                (true, y) if y < SIDE / 2 => 0xf800,
                (false, y) if y < SIDE / 2 => 0x07e0,
                (true, _) => 0x001f,
                (false, _) => 0xffff,
            })
            .collect();
        let frame = q565::encode(SIDE as u16, SIDE as u16, &pixels);
        let t = Instant::now();
        if let Err(e) = k.lcd_frame(&frame) {
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
    let back = k.lcd_liquid();
    STOPPED.store(true, Ordering::SeqCst);
    result.and(back)
}

fn status() -> Result<()> {
    let api = HidApi::new()?;
    match device::Kraken::open(&api).and_then(|mut k| {
        k.initialize()?;
        Ok((k.status()?, k.firmware, k.lcd_info.clone()))
    }) {
        Ok((s, fw, lcd)) => {
            println!("kraken: firmware {fw:?}, lcd {lcd:?}");
            println!(
                "kraken: liquid {:.1} °C, pump {} rpm ({} %), fan {} rpm ({} %)",
                s.liquid_c, s.pump_rpm, s.pump_duty, s.fan_rpm, s.fan_duty
            );
        }
        Err(e) => eprintln!("kraken: {e:#}"),
    }
    match device::ControlHub::open(&api).and_then(|mut h| {
        h.initialize()?;
        Ok((h.status()?, h.firmware))
    }) {
        Ok((s, fw)) => {
            println!("hub: firmware {fw:?}, noise {} dB", s.noise_db);
            for (i, f) in s.fans.iter().enumerate() {
                println!("hub: fan {} {:?} {} rpm {} %", i + 1, f.mode, f.rpm, f.duty);
            }
        }
        Err(e) => eprintln!("hub: {e:#}"),
    }
    Ok(())
}

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

fn run(path: PathBuf, verbose: bool, limit: Option<Duration>) -> Result<()> {
    let cfg = config::Config::load(&path)?;
    eprintln!("camless: {}", path.display());
    unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(on_console_event), 1) };
    if let Some(limit) = limit {
        std::thread::spawn(move || {
            std::thread::sleep(limit);
            STOP.store(true, Ordering::SeqCst);
        });
    }
    let shared = Arc::new(Mutex::new(daemon::Snapshot::default()));
    // Two frames in flight at most; the API answers 429 beyond that.
    let (tx, rx) = mpsc::sync_channel(2);
    if let Some(a) = &cfg.api {
        api::spawn(&a.listen, shared.clone(), tx.clone()).context("starting the local API")?;
        eprintln!("camless: api on http://{}", a.listen);
    }
    let sensors = sensors::Sensors::open(pawnio_module(cfg.pawnio_module.as_ref()).as_deref());
    let mut d = daemon::Daemon::new(cfg, sensors, shared, verbose)?;
    d.run(&rx, &STOP);
    drop(tx);
    STOPPED.store(true, Ordering::SeqCst);
    Ok(())
}

fn list() -> Result<()> {
    let api = HidApi::new()?;
    for d in api.device_list().filter(|d| d.vendor_id() == NZXT_VID) {
        println!(
            "{:04x}:{:04x} if={} {} | {}",
            d.vendor_id(),
            d.product_id(),
            d.interface_number(),
            d.product_string().unwrap_or("?"),
            d.path().to_string_lossy()
        );
    }
    Ok(())
}

fn open(api: &HidApi, pid: u16) -> Result<HidDevice> {
    let info = api
        .device_list()
        .find(|d| d.vendor_id() == NZXT_VID && d.product_id() == pid)
        .with_context(|| format!("no device {NZXT_VID:04x}:{pid:04x}"))?;
    Ok(info.open_device(api)?)
}

/// Reads input reports only. Windows gives every open handle its own copy of each
/// input report, so this sees the replies to CAM's own polling without touching it.
fn watch(duration: Duration, raw: bool) -> Result<()> {
    let api = HidApi::new()?;
    let mut devices = Vec::new();
    for (name, pid) in [("kraken", KRAKEN_ELITE_V2_PID), ("hub", CONTROL_HUB_PID)] {
        match open(&api, pid) {
            Ok(dev) => devices.push((name, dev)),
            Err(e) => eprintln!("{name}: cannot open: {e}"),
        }
    }
    if devices.is_empty() {
        bail!("neither the Kraken Elite V2 nor the Control Hub could be opened");
    }
    let start = Instant::now();
    let mut buf = [0u8; REPORT_LEN];
    while start.elapsed() < duration && !devices.is_empty() {
        let mut failed = None;
        for (idx, (name, dev)) in devices.iter().enumerate() {
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
            if let Some(s) = parse_kraken_status(msg) {
                println!(
                    "{t:6.2}s {name}: liquid {:.1} °C, pump {} rpm ({} %), fan {} rpm ({} %)",
                    s.liquid_c, s.pump_rpm, s.pump_duty, s.fan_rpm, s.fan_duty
                );
            } else if let Some(s) = parse_hub_status(msg) {
                let fans: Vec<String> = s
                    .fans
                    .iter()
                    .enumerate()
                    .map(|(i, f)| format!("fan{} {:?} {} rpm {} %", i + 1, f.mode, f.rpm, f.duty))
                    .collect();
                println!("{t:6.2}s {name}: {} | noise {} dB", fans.join(", "), s.noise_db);
            } else {
                println!("{t:6.2}s {name}: report {:02x} {:02x} ({n} B)", msg[0], msg[1]);
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

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
}
