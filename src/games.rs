//! Game sessions: which game is being played, its frame rate, and a record of each
//! session.
//!
//! A program is a game when it runs from a Steam library (`steamapps\common\<game>`),
//! an Xbox games folder (`XboxGames\<game>`) or an Epic Games install, or when
//! `[games] add` lists it; `[games] skip` takes one out. A session starts when a game
//! comes to the front. It ends when the game exits, when another game comes to the
//! front, or after [`AWAY`] in the background. Only time in front counts: play time,
//! frames and temperatures.
//!
//! Sessions shorter than [`MIN_PLAYED`] are not kept: a launcher, a crash at start.
//! The others go to `history/sessions.jsonl`, one JSON object a line, and stay.
//!
//! Frame rates come from [`crate::fps`], which needs administrator rights. Without
//! them a session still has its play time and temperatures.

use crate::fps::{self, Capture, Meter, SECOND};
use crate::history;
use camless_core::config;
use camless_core::status::{Game, Sensor, Unit};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

/// A game this long in the background ends its session.
const AWAY: Duration = Duration::from_secs(180);
/// Seconds of play a session needs to be kept.
const MIN_PLAYED: f64 = 60.0;
/// How long a failed capture waits before another try.
const RETRY: Duration = Duration::from_secs(60);
/// Epic's list of installs is read again after this long.
const EPIC_EVERY: Duration = Duration::from_secs(60);
/// Buckets per tenfold frame time: 10^(1/230) is 1.01, so a bucket is 1 % wide.
const PER_DECADE: f64 = 230.0;
/// The frame time the first bucket starts at, 0.1 ms: 10 000 fps.
const SHORTEST: i64 = 1_000;

/// One finished session, a line of `sessions.jsonl`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct Session {
    pub game: String,
    /// The program's file name.
    pub exe: String,
    /// Unix milliseconds: when the game came to the front, and when it was last there.
    pub start: i64,
    pub end: i64,
    /// Seconds in front.
    pub played: u32,
    /// Frames counted in front; 0 without a capture.
    pub frames: u64,
    /// The average frame rate.
    pub fps: Option<f64>,
    /// The 1 % and 0.1 % lows: 1 % (0.1 %) of frames took at least as long as one
    /// frame at this rate.
    pub low1: Option<f64>,
    pub low01: Option<f64>,
    /// Each temperature while in front, by sensor id.
    #[serde(default)]
    pub temps: BTreeMap<String, Stat>,
    /// The Steam app id, for a game in a Steam library; the app finds its cover by it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steam: Option<u32>,
}

/// A game, as its store knows it.
#[derive(Clone, Debug, PartialEq)]
struct Title {
    name: String,
    /// The Steam app id, for a game in a Steam library.
    steam: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct Stat {
    pub avg: f64,
    pub max: f64,
}

/// Frame times in buckets 1 % wide, from 0.1 ms to [`fps::MAX_FRAME`].
struct Frames {
    buckets: Vec<u32>,
    count: u64,
    /// The sum of the frame times, in 100 ns.
    total: i64,
}

fn bucket(t: i64) -> usize {
    ((t.max(SHORTEST) as f64 / SHORTEST as f64).log10() * PER_DECADE) as usize
}

impl Frames {
    fn new() -> Frames {
        Frames { buckets: vec![0; bucket(fps::MAX_FRAME) + 1], count: 0, total: 0 }
    }

    fn add(&mut self, t: i64) {
        let i = bucket(t).min(self.buckets.len() - 1);
        self.buckets[i] = self.buckets[i].saturating_add(1);
        self.count += 1;
        self.total += t;
    }

    fn fps(&self) -> Option<f64> {
        (self.total > 0).then(|| round1(self.count as f64 * SECOND as f64 / self.total as f64))
    }

    /// The rate of the frame time that a `share` of frames took at least as long as:
    /// 0.01 for the 1 % low.
    fn low(&self, share: f64) -> Option<f64> {
        let want = ((self.count as f64 * share).ceil() as u64).max(1);
        let mut seen = 0;
        for (i, &n) in self.buckets.iter().enumerate().rev() {
            seen += u64::from(n);
            if seen >= want {
                // The middle of the bucket.
                let t = SHORTEST as f64 * 10f64.powf((i as f64 + 0.5) / PER_DECADE);
                return Some(round1(SECOND as f64 / t));
            }
        }
        None
    }
}

/// One decimal, so 59.1 is written as 59.1 and not as 59.099998474.
fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

#[derive(Default)]
struct Acc {
    sum: f64,
    n: u32,
    max: f32,
}

impl Acc {
    fn add(&mut self, v: f32) {
        self.max = if self.n == 0 { v } else { self.max.max(v) };
        self.sum += f64::from(v);
        self.n += 1;
    }
}

/// Adds each temperature to a session's.
fn add_temps(temps: &mut BTreeMap<String, Acc>, sensors: &[Sensor]) {
    for s in sensors.iter().filter(|s| s.unit == Unit::Celsius) {
        if let Some(v) = s.value {
            temps.entry(s.id.clone()).or_default().add(v);
        }
    }
}

/// A process handle, for telling when the game exits.
struct Process(HANDLE);

impl Process {
    fn open(pid: u32) -> Option<Process> {
        let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        (!h.is_null()).then_some(Process(h))
    }

    fn exited(&self) -> bool {
        unsafe { WaitForSingleObject(self.0, 0) == WAIT_OBJECT_0 }
    }

    fn path(&self) -> Option<String> {
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = unsafe { QueryFullProcessImageNameW(self.0, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) } != 0;
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// The process that owns the window in front, 0 for none, as on a locked screen.
pub fn foreground() -> u32 {
    let w = unsafe { GetForegroundWindow() };
    if w.is_null() {
        return 0;
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(w, &mut pid) };
    pid
}

/// The file name of process `pid`'s program.
pub fn program(pid: u32) -> Option<String> {
    let path = Process::open(pid)?.path()?;
    Some(file_name(&path).to_string())
}

struct Open {
    pid: u32,
    /// None when Windows would not open the process; then only [`AWAY`] ends it.
    process: Option<Process>,
    game: String,
    exe: String,
    steam: Option<u32>,
    start: i64,
    last_seen: i64,
    played: f64,
    away: Option<Instant>,
    meter: Meter,
    frames: Frames,
    /// The last second's frames, while in front with a capture.
    fps: Option<u32>,
    temps: BTreeMap<String, Acc>,
}

impl Open {
    fn session(&self) -> Session {
        let temps = self
            .temps
            .iter()
            .map(|(id, a)| (id.clone(), Stat { avg: round1(a.sum / f64::from(a.n)), max: round1(a.max.into()) }))
            .collect();
        Session {
            game: self.game.clone(),
            exe: self.exe.clone(),
            start: self.start,
            end: self.last_seen,
            played: self.played as u32,
            frames: self.frames.count,
            fps: self.frames.fps(),
            low1: self.frames.low(0.01),
            low01: self.frames.low(0.001),
            temps,
            steam: self.steam,
        }
    }
}

pub struct Games {
    /// Where sessions go; none in demo mode.
    file: Option<PathBuf>,
    /// Every session kept, oldest first; the API reads it.
    log: Arc<Mutex<Vec<Session>>>,
    demo: Option<Demo>,
    capture: Option<Capture>,
    /// Why there is no capture.
    error: Option<String>,
    /// When to try a capture again; none for never.
    retry: Option<Instant>,
    /// Epic Games installs: (folder in lower case, name).
    epic: Vec<(String, String)>,
    epic_read: Option<Instant>,
    lists: config::Games,
    /// The process in front, and the game it is, with its file name.
    front: (u32, Option<(Title, String)>),
    /// The game each program is, by path, so each is looked up once. Not by process
    /// id: Windows soon gives a closed game's id to another program.
    known: HashMap<String, Option<Title>>,
    open: Option<Open>,
    notes: Vec<String>,
}

impl Games {
    /// Sessions are kept in `dir`, or only in memory without one. Without
    /// administrator rights there are no frame rates.
    pub fn new(dir: Option<&Path>, elevated: bool, demo: bool) -> Games {
        let file = dir.map(|d| d.join("sessions.jsonl"));
        let log = file.as_deref().map(load).unwrap_or_default();
        let (error, retry) = match (demo, elevated) {
            (true, _) | (_, true) => (None, Some(Instant::now())),
            _ => (Some("needs administrator rights".to_string()), None),
        };
        Games {
            file,
            log: Arc::new(Mutex::new(if demo { Demo::sessions() } else { log })),
            demo: demo.then(Demo::default),
            capture: None,
            error,
            retry,
            epic: Vec::new(),
            epic_read: None,
            lists: config::Games::default(),
            front: (u32::MAX, None),
            known: HashMap::new(),
            open: None,
            notes: Vec::new(),
        }
    }

    pub fn log(&self) -> Arc<Mutex<Vec<Session>>> {
        self.log.clone()
    }

    /// What to say in the service's log since the last call.
    pub fn notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    pub fn error(&self) -> Option<String> {
        self.error.clone()
    }

    /// Once a tick, `dt` seconds after the one before. Returns the frame rate of the
    /// game in front, for the history.
    pub fn tick(&mut self, dt: f32, sensors: &[Sensor], lists: &config::Games) -> Option<f32> {
        if let Some(demo) = &mut self.demo {
            return demo.tick(dt, sensors, &mut self.open);
        }
        if *lists != self.lists {
            self.lists = lists.clone();
            self.known.clear();
            self.front = (u32::MAX, None);
        }
        let pid = foreground();
        if self.front.0 != pid {
            let path = Process::open(pid).and_then(|p| p.path());
            let game = path.and_then(|path| Some((self.game_at(&path)?, file_name(&path).to_string())));
            self.front = (pid, game);
        }
        let new_game = self.front.1.clone().filter(|_| self.open.as_ref().is_none_or(|o| o.pid != pid));
        if let Some((title, exe)) = new_game {
            let process = Process::open(pid);
            // A game that just closed can be in front a moment longer.
            if !process.as_ref().is_some_and(Process::exited) {
                self.close();
                let now = unix_ms();
                self.notes.push(format!("game: {} ({exe})", title.name));
                self.open = Some(Open {
                    pid,
                    process,
                    game: title.name,
                    exe,
                    steam: title.steam,
                    start: now,
                    last_seen: now,
                    played: 0.0,
                    away: None,
                    meter: Meter::default(),
                    frames: Frames::new(),
                    fps: None,
                    temps: BTreeMap::new(),
                });
            }
        }
        let Some(o) = &mut self.open else {
            // No ETW session while nothing is played.
            self.capture = None;
            return None;
        };
        let in_front = o.pid == pid;
        let away = !in_front && o.away.get_or_insert_with(Instant::now).elapsed() >= AWAY;
        let exited = o.process.as_ref().is_some_and(Process::exited);
        if away || exited {
            self.close();
            self.capture = None;
            if exited {
                // Looked up again, in case another program gets its process id.
                self.front = (u32::MAX, None);
            }
            return None;
        }
        if in_front {
            o.away = None;
            o.last_seen = unix_ms();
            o.played += f64::from(dt);
            add_temps(&mut o.temps, sensors);
        }
        self.keep_capture();
        let (Some(c), Some(o)) = (&self.capture, &mut self.open) else { return None };
        c.follow(o.pid);
        o.meter.add(&c.take());
        let frames = &mut o.frames;
        o.meter.settle(fps::now(), |t| {
            if in_front {
                frames.add(t)
            }
        });
        // Nothing is counted for the first couple of seconds; that is not 0 fps.
        o.fps = (in_front && o.meter.started()).then(|| o.meter.fps());
        o.fps.map(|f| f as f32)
    }

    /// Starts the capture, and starts it again after it failed or stopped.
    fn keep_capture(&mut self) {
        if self.capture.as_ref().is_some_and(|c| !c.alive()) {
            self.capture = None;
            self.error = Some("the ETW session stopped".into());
            self.notes.push("fps: the ETW session stopped; trying again in a minute".into());
            self.retry = Some(Instant::now() + RETRY);
        }
        if self.capture.is_some() || self.retry.is_none_or(|t| Instant::now() < t) {
            return;
        }
        match Capture::start(fps::SESSION) {
            Ok(c) => {
                self.capture = Some(c);
                self.error = None;
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if self.error.as_ref() != Some(&msg) {
                    self.notes.push(format!("fps: {msg}; trying again every minute"));
                }
                self.error = Some(msg);
                self.retry = Some(Instant::now() + RETRY);
            }
        }
    }

    /// The game that the program at `path` is, if it is one.
    fn game_at(&mut self, path: &str) -> Option<Title> {
        if let Some(game) = self.known.get(path) {
            return game.clone();
        }
        let game = self.identify(path);
        if self.known.len() >= 256 {
            self.known.clear();
        }
        self.known.insert(path.to_string(), game.clone());
        game
    }

    fn identify(&mut self, path: &str) -> Option<Title> {
        if listed(&self.lists.skip, path) {
            return None;
        }
        if self.epic_read.is_none_or(|t| t.elapsed() >= EPIC_EVERY) {
            self.epic = epic_installs();
            self.epic_read = Some(Instant::now());
        }
        store_game(path, &self.epic)
            .or_else(|| listed(&self.lists.add, path).then(|| Title { name: stem(path).to_string(), steam: None }))
    }

    /// The session being played, for the status.
    pub fn now(&self) -> Option<Game> {
        let o = self.open.as_ref()?;
        Some(Game {
            name: o.game.clone(),
            exe: o.exe.clone(),
            steam: o.steam,
            start: o.start,
            played: o.played as u32,
            in_front: o.away.is_none(),
            fps: o.fps.map(f64::from),
            avg_fps: o.frames.fps(),
            low1: o.frames.low(0.01),
        })
    }

    /// Ends the open session, keeping it if it was long enough.
    fn close(&mut self) {
        let Some(o) = self.open.take() else { return };
        let s = o.session();
        if o.played < MIN_PLAYED {
            self.notes.push(format!("game: {} ended after {} s, too short to keep", s.game, s.played));
            return;
        }
        let rate = s.fps.map(|f| format!(", {f} fps, 1 % low {}", s.low1.unwrap_or_default())).unwrap_or_default();
        self.notes.push(format!("game: {} ended, played {} min{rate}", s.game, s.played / 60));
        if let Some(f) = &self.file {
            if let Err(e) = append(f, &s) {
                self.notes.push(format!("game: not saved to {}: {e}", f.display()));
            }
        }
        self.log.lock().unwrap_or_else(|e| e.into_inner()).push(s);
    }

    /// When the service stops: the open session is kept as it is.
    pub fn finish(&mut self) {
        self.close();
        self.capture = None;
    }
}

fn unix_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn load(file: &Path) -> Vec<Session> {
    let Ok(text) = std::fs::read_to_string(file) else { return Vec::new() };
    // A line a crash cut short does not parse and is skipped.
    text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn append(file: &Path, s: &Session) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // One write, so a crash leaves at most one short line.
    let mut line = serde_json::to_string(s).map_err(std::io::Error::other)?;
    line.push('\n');
    history::append_lines(file, &line)
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

fn stem(path: &str) -> &str {
    let name = file_name(path);
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

/// Lower case with backslashes and no trailing one, for comparing paths.
fn normalize(path: &str) -> String {
    path.trim().replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase()
}

fn in_folder(path: &str, folder: &str) -> bool {
    path.strip_prefix(folder).is_some_and(|rest| rest.starts_with('\\'))
}

/// A `[games]` list names the program at `path`: by file name, or a folder it is in.
fn listed(list: &[String], path: &str) -> bool {
    let path = normalize(path);
    let exe = file_name(&path);
    list.iter().map(|e| normalize(e)).any(|e| if e.contains('\\') { in_folder(&path, &e) } else { exe == e })
}

/// The game a program is, by where it runs: a Steam library, an Xbox games folder, or
/// an Epic Games install, given as (folder in lower case, name).
fn store_game(path: &str, epic: &[(String, String)]) -> Option<Title> {
    let lower = path.to_ascii_lowercase().replace('/', "\\");
    if let Some((i, folder)) = folder_after(path, &lower, r"\steamapps\common\") {
        let app = steam_app(Path::new(&path[..i]), folder);
        let steam = app.as_ref().and_then(|a| a.appid);
        let name = app.and_then(|a| a.name).unwrap_or_else(|| folder.to_string());
        return Some(Title { name, steam });
    }
    if let Some((_, folder)) = folder_after(path, &lower, r"\xboxgames\") {
        return Some(Title { name: folder.to_string(), steam: None });
    }
    epic.iter().find(|(folder, _)| in_folder(&lower, folder)).map(|(_, name)| Title { name: name.clone(), steam: None })
}

/// Where `marker` is in `path`, found in its lower case `lower`, and the folder after
/// it, if a file is in that folder. ASCII lower case keeps every byte where it was,
/// so offsets in `lower` fit `path`.
fn folder_after<'a>(path: &'a str, lower: &str, marker: &str) -> Option<(usize, &'a str)> {
    let i = lower.find(marker)?;
    let (folder, _) = path[i + marker.len()..].split_once(['\\', '/'])?;
    (!folder.is_empty()).then_some((i, folder))
}

/// What a Steam app manifest says about a game.
#[derive(Debug, Default, PartialEq)]
struct Manifest {
    appid: Option<u32>,
    name: Option<String>,
    installdir: Option<String>,
}

/// The manifest in the Steam library at `library` for the game in
/// `steamapps\common\<folder>`. A manifest without an `appid` has it in its file name.
fn steam_app(library: &Path, folder: &str) -> Option<Manifest> {
    for entry in std::fs::read_dir(library.join("steamapps")).ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(id) = name.strip_prefix("appmanifest_").and_then(|n| n.strip_suffix(".acf")) else { continue };
        let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
        let m = manifest(&text);
        if m.installdir.as_deref().is_some_and(|d| d.eq_ignore_ascii_case(folder)) {
            return Some(Manifest { appid: m.appid.or_else(|| id.parse().ok()), ..m });
        }
    }
    None
}

/// `appid`, `name` and `installdir` from a Steam app manifest.
fn manifest(text: &str) -> Manifest {
    let mut m = Manifest::default();
    for line in text.lines() {
        let q = quoted(line);
        match (q.first().map(|k| k.to_ascii_lowercase()).as_deref(), q.get(1)) {
            (Some("appid"), Some(v)) if m.appid.is_none() => m.appid = v.parse().ok(),
            (Some("name"), Some(v)) if m.name.is_none() => m.name = Some(v.clone()),
            (Some("installdir"), Some(v)) if m.installdir.is_none() => m.installdir = Some(v.clone()),
            _ => {}
        }
    }
    m
}

/// The quoted strings on a line of a Steam `.acf` file, with `\"` and `\\` undone.
fn quoted(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut s = String::new();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => s.extend(chars.next()),
                c => s.push(c),
            }
        }
        out.push(s);
    }
    out
}

/// Epic Games installs from the launcher's manifests: (folder in lower case, name).
fn epic_installs() -> Vec<(String, String)> {
    let data = std::env::var_os("ProgramData").map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from);
    let Ok(dir) = std::fs::read_dir(data.join(r"Epic\EpicGamesLauncher\Data\Manifests")) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in dir.flatten() {
        if entry.path().extension().is_none_or(|e| e != "item") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
        if let Some(install) = epic_install(&text) {
            out.push(install);
        }
    }
    out
}

fn epic_install(item: &str) -> Option<(String, String)> {
    let v: serde_json::Value = serde_json::from_str(item).ok()?;
    let folder = normalize(v["InstallLocation"].as_str()?);
    let name = v["DisplayName"].as_str()?.to_string();
    (!folder.is_empty() && !name.is_empty()).then_some((folder, name))
}

/// `camless run --demo`: a game that is always in front, and a few past sessions.
#[derive(Default)]
struct Demo {
    t: f64,
    seed: u32,
}

impl Demo {
    fn sessions() -> Vec<Session> {
        let day = 86_400_000;
        let base = unix_ms() / day * day - 3 * day;
        let s = |game: &str, exe: &str, start: i64, min: u32, fps: f64, low1: f64, low01: f64, cpu: f64, gpu: f64| Session {
            game: game.into(),
            exe: exe.into(),
            start,
            end: start + i64::from(min) * 60_000,
            played: min * 60,
            frames: (fps * f64::from(min * 60)) as u64,
            fps: Some(fps),
            low1: Some(low1),
            low01: Some(low01),
            temps: [
                ("cpu.temp".to_string(), Stat { avg: cpu - 6.0, max: cpu }),
                ("gpu.temp".to_string(), Stat { avg: gpu - 5.0, max: gpu }),
                ("demo-cooler.liquid".to_string(), Stat { avg: 33.5, max: 35.1 }),
            ]
            .into(),
            steam: None,
        };
        vec![
            s("Demo racer", "racer.exe", base + 19 * 3_600_000, 94, 118.4, 86.2, 71.0, 74.0, 68.0),
            s("Demo shooter", "shooter.exe", base + day + 20 * 3_600_000, 47, 287.9, 171.3, 120.8, 81.0, 72.0),
            s("Demo racer", "racer.exe", base + 2 * day + 18 * 3_600_000, 136, 121.0, 90.5, 77.9, 75.0, 69.0),
        ]
    }

    fn tick(&mut self, dt: f32, sensors: &[Sensor], open: &mut Option<Open>) -> Option<f32> {
        self.t += f64::from(dt);
        if self.seed == 0 {
            self.seed = 2_463_534_242;
        }
        let o = open.get_or_insert_with(|| Open {
            pid: 0,
            process: None,
            game: "Demo game".into(),
            exe: "demo.exe".into(),
            steam: None,
            start: unix_ms(),
            last_seen: unix_ms(),
            played: 0.0,
            away: None,
            meter: Meter::default(),
            frames: Frames::new(),
            fps: None,
            temps: BTreeMap::new(),
        });
        o.played += f64::from(dt);
        o.last_seen = unix_ms();
        add_temps(&mut o.temps, sensors);
        let rate = 120.0 + 25.0 * (self.t / 20.0).sin();
        let n = (rate * f64::from(dt)) as u32;
        for _ in 0..n {
            // xorshift: frame times scattered around the rate, with a hitch now and then.
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 17;
            self.seed ^= self.seed << 5;
            let r = f64::from(self.seed % 1000) / 1000.0;
            let spread = if self.seed % 200 == 0 { 2.5 } else { 0.85 + 0.3 * r };
            o.frames.add((SECOND as f64 / rate * spread) as i64);
        }
        o.fps = Some(rate.round() as u32);
        o.fps.map(|f| f as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const MS: i64 = SECOND / 1000;

    #[test]
    fn lows_are_the_slow_end_of_the_frame_times() {
        let mut f = Frames::new();
        // 990 frames at 10 ms, 9 at 20 ms and one at 50 ms.
        (0..990).for_each(|_| f.add(10 * MS));
        (0..9).for_each(|_| f.add(20 * MS));
        f.add(50 * MS);
        let near = |got: Option<f64>, want: f64| assert!((got.unwrap() - want).abs() <= want * 0.01, "{got:?} vs {want}");
        near(f.fps(), 1000.0 / 10.13);
        near(f.low(0.01), 50.0);
        near(f.low(0.001), 20.0);
        assert_eq!(Frames::new().fps(), None);
        assert_eq!(Frames::new().low(0.01), None);
    }

    #[test]
    fn stores_tell_the_game_by_its_folder() {
        let epic = [(normalize(r"D:\Epic Games\Fortnite"), "Fortnite".to_string())];
        let name = |path| store_game(path, &epic).map(|t| t.name);
        assert_eq!(name(r"D:\Gry\SteamLibrary\steamapps\common\No Such Game\bin\game.exe").as_deref(), Some("No Such Game"));
        assert_eq!(name(r"C:\XboxGames\Forza Horizon 5\Content\ForzaHorizon5.exe").as_deref(), Some("Forza Horizon 5"));
        assert_eq!(name(r"d:\epic games\fortnite\FortniteGame\Binaries\Win64\x.exe").as_deref(), Some("Fortnite"));
        assert_eq!(store_game(r"D:\Epic Games\FortniteOther\x.exe", &epic), None);
        assert_eq!(store_game(r"C:\Program Files (x86)\Steam\steam.exe", &epic), None);
        assert_eq!(store_game(r"C:\Windows\explorer.exe", &epic), None);
    }

    #[test]
    fn steam_names_and_ids_come_from_the_app_manifest() {
        let lib = std::env::temp_dir().join(format!("camless-steam-{}", std::process::id()));
        std::fs::create_dir_all(lib.join("steamapps")).unwrap();
        let acf = "\"AppState\"\n{\n\t\"appid\"\t\t\"1091500\"\n\t\"name\"\t\t\"A \\\"Quoted\\\" Game\"\n\t\"installdir\"\t\t\"AQG\"\n}\n";
        std::fs::write(lib.join(r"steamapps\appmanifest_1091500.acf"), acf).unwrap();
        // Without an appid line, the file name has it.
        std::fs::write(lib.join(r"steamapps\appmanifest_730.acf"), "\"AppState\"\n{\n\t\"installdir\"\t\t\"Other\"\n}\n").unwrap();
        let game = |dir: &str| store_game(&lib.join(format!(r"steamapps\common\{dir}\Game.exe")).to_string_lossy(), &[]);
        assert_eq!(game("aqg"), Some(Title { name: "A \"Quoted\" Game".into(), steam: Some(1_091_500) }));
        assert_eq!(game("Other"), Some(Title { name: "Other".into(), steam: Some(730) }));
        assert_eq!(game("Gone"), Some(Title { name: "Gone".into(), steam: None }));
        std::fs::remove_dir_all(&lib).unwrap();
    }

    #[test]
    fn epic_manifests_give_folder_and_name() {
        let item = r#"{"FormatVersion": 0, "DisplayName": "Alan Wake 2", "InstallLocation": "E:/Epic/AlanWake2/", "LaunchExecutable": "AlanWake2.exe"}"#;
        assert_eq!(epic_install(item), Some((r"e:\epic\alanwake2".to_string(), "Alan Wake 2".to_string())));
        assert_eq!(epic_install("{}"), None);
    }

    #[test]
    fn lists_match_a_file_name_or_a_folder() {
        let list = ["javaw.exe".to_string(), r"D:\Emulators\".to_string()];
        assert!(listed(&list, r"C:\Program Files\Java\bin\JAVAW.EXE"));
        assert!(listed(&list, r"d:\emulators\dolphin\Dolphin.exe"));
        assert!(!listed(&list, r"D:\EmulatorsOld\x.exe"));
        assert!(!listed(&list, r"C:\java.exe"));
    }

    #[test]
    fn short_sessions_are_dropped_and_long_ones_kept() {
        let dir = std::env::temp_dir().join(format!("camless-games-keep-{}", std::process::id()));
        let open = |played| Open {
            pid: 1,
            process: None,
            game: "Game".into(),
            exe: "game.exe".into(),
            steam: Some(1),
            start: 1,
            last_seen: 2,
            played,
            away: None,
            meter: Meter::default(),
            frames: Frames::new(),
            fps: None,
            temps: BTreeMap::new(),
        };
        let mut g = Games::new(Some(&dir), true, false);
        g.open = Some(open(MIN_PLAYED - 1.0));
        g.close();
        assert!(g.log.lock().unwrap().is_empty());
        let mut o = open(MIN_PLAYED + 1.0);
        let sensor = |id: &str, unit, v| Sensor { id: id.into(), unit, value: Some(v), ..Default::default() };
        add_temps(&mut o.temps, &[sensor("cpu.temp", Unit::Celsius, 60.0), sensor("cpu.load", Unit::Percent, 90.0)]);
        add_temps(&mut o.temps, &[sensor("cpu.temp", Unit::Celsius, 70.04)]);
        g.open = Some(o);
        g.close();
        let kept = g.log.lock().unwrap().clone();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].temps, [("cpu.temp".to_string(), Stat { avg: 65.0, max: 70.0 })].into());
        // No capture, no frame rates.
        assert_eq!((kept[0].frames, kept[0].fps, kept[0].low1), (0, None, None));
        // The next start of the service reads them back.
        assert_eq!(*Games::new(Some(&dir), true, false).log.lock().unwrap(), kept);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sessions_are_a_line_each_and_a_torn_line_is_skipped() {
        let dir = std::env::temp_dir().join(format!("camless-games-{}", std::process::id()));
        let file = dir.join("sessions.jsonl");
        let s = Demo::sessions();
        for x in &s {
            append(&file, x).unwrap();
        }
        let mut f = std::fs::OpenOptions::new().append(true).open(&file).unwrap();
        f.write_all(br#"{"game":"cut sh"#).unwrap();
        assert_eq!(load(&file), s);
        // The next session starts a line of its own.
        append(&file, &s[0]).unwrap();
        assert_eq!(load(&file), [&s[..], &s[..1]].concat());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
