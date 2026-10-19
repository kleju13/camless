//! Web pages on an LCD, shown the way CAM's Web Integration shows them. A headless
//! Microsoft Edge (or Chrome) renders the page at the screen's size with
//! `?kraken=1`, camless hands it sensor data through
//! `window.nzxt.v1.onMonitoringDataUpdate` every second, and streams what it draws.
//!
//! The browser runs as the signed-in user even when camless is elevated, with a
//! profile of its own, and in a job object, so it ends when camless does.
//!
//! A page can also hand camless its moving background (backdrop.rs) and draw only
//! what lies over it; camless then plays the background under the page's frames.

use crate::api;
use crate::backdrop::{self, Backdrop, Change};
use crate::cdp::{self, Cdp};
use crate::daemon::{Event, Snapshot};
use crate::q565;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// A page that does not change is sent again this often, so the LCD keeps it.
const KEEPALIVE: Duration = Duration::from_secs(1);
const DATA_EVERY: Duration = Duration::from_secs(1);
const RETRY_AFTER: Duration = Duration::from_secs(10);

/// What to render; a change restarts the renderer.
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub url: String,
    pub fps: u8,
    pub width: u16,
    pub height: u16,
    pub round: bool,
}

#[derive(Clone, Debug, Default)]
pub struct State {
    pub error: Option<String>,
    /// Frames per second over the last two seconds.
    pub fps: f32,
    pub browser: Option<String>,
    /// Frames of the background the page handed over; 0 while it draws its own.
    pub background: usize,
}

pub struct Renderer {
    pub page: Page,
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<State>>,
}

impl Renderer {
    pub fn start(device: &str, page: Page, shared: Arc<Mutex<Snapshot>>, events: SyncSender<Event>) -> Renderer {
        let stop = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(State::default()));
        let job = Job { device: device.to_string(), page: page.clone(), shared, events, stop: stop.clone(), state: state.clone() };
        let spawned = std::thread::Builder::new().name(format!("web {device}")).spawn(move || job.run());
        if let Err(e) = spawned {
            state.lock().unwrap_or_else(|e| e.into_inner()).error = Some(format!("starting the renderer: {e}"));
        }
        Renderer { page, stop, state }
    }

    pub fn state(&self) -> State {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Drop for Renderer {
    /// The thread closes the browser by itself. It is not waited for, so a browser
    /// that is slow to answer never holds up the fans.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

struct Job {
    device: String,
    page: Page,
    shared: Arc<Mutex<Snapshot>>,
    events: SyncSender<Event>,
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<State>>,
}

impl Job {
    fn set(&self, f: impl FnOnce(&mut State)) {
        f(&mut self.state.lock().unwrap_or_else(|e| e.into_inner()));
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn run(self) {
        while !self.stopped() {
            match self.session() {
                Ok(()) => return,
                Err(e) => self.set(|s| (s.error, s.fps) = (Some(format!("{e:#}")), 0.0)),
            }
            let until = Instant::now() + RETRY_AFTER;
            while Instant::now() < until && !self.stopped() {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    /// One browser, until the job stops (Ok) or something fails (Err).
    fn session(&self) -> Result<()> {
        let dir = profile_dir(&self.device);
        // A new renderer for the same screen waits for the old browser to close: two
        // cannot share a profile.
        let lock = profile_lock(&dir);
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        let (w, h) = (self.page.width, self.page.height);
        let browser = Browser::launch(&dir, w, h, &self.stop)?;
        self.set(|s| s.browser = Some(browser.name.clone()));
        let mut cdp = Cdp::connect(browser.port, &browser.ws_path)?;

        let targets = cdp.call("Target.getTargets", json!({}), None)?;
        let page = targets["targetInfos"].as_array().and_then(|ts| ts.iter().find(|t| t["type"] == "page")).map(|t| t["targetId"].clone());
        let target = match page {
            Some(id) => id,
            None => cdp.call("Target.createTarget", json!({ "url": "about:blank" }), None)?["targetId"].clone(),
        };
        let attached = cdp.call("Target.attachToTarget", json!({ "targetId": target, "flatten": true }), None)?;
        let session = attached["sessionId"].as_str().context("no DevTools session")?.to_string();
        let s = Some(session.as_str());
        cdp.call("Page.enable", json!({}), s)?;
        cdp.call("Emulation.setDeviceMetricsOverride", json!({ "width": w, "height": h, "deviceScaleFactor": 1, "mobile": false }), s)?;
        // Pages without a background are drawn on black, as on CAM's screen.
        cdp.call("Emulation.setDefaultBackgroundColorOverride", json!({ "color": { "r": 0, "g": 0, "b": 0, "a": 1 } }), s)?;
        // What the page sends through window.nzxt.v1.camless arrives as Runtime.bindingCalled.
        cdp.call("Runtime.enable", json!({}), s)?;
        cdp.call("Runtime.addBinding", json!({ "name": backdrop::BINDING }), s)?;
        cdp.call("Page.addScriptToEvaluateOnNewDocument", json!({ "source": shim(&self.page) }), s)?;
        let nav = cdp.call("Page.navigate", json!({ "url": lcd_url(&self.page.url) }), s)?;
        if let Some(e) = nav["errorText"].as_str() {
            bail!("{} did not load: {e}", self.page.url);
        }
        let frame = nav["frameId"].clone();
        // The screencast belongs to the document, so it starts once the page is in,
        // and again whenever the page loads anew (KrakenDial reloads on an update).
        // A page may write to camless before that; those messages, and which context
        // is the page's, wait.
        let screencast = json!({ "format": "png", "maxWidth": w, "maxHeight": h, "everyNthFrame": 1 });
        let mut early = VecDeque::new();
        let deadline = Instant::now() + Duration::from_secs(15);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match cdp.next_event(left)? {
                Some(ev) if ev["method"] == "Page.loadEventFired" => break,
                Some(ev) if ev["method"].as_str().is_some_and(|m| m.starts_with("Runtime.")) => early.push_back(ev),
                _ => {}
            }
        }
        cdp.call("Page.startScreencast", screencast.clone(), s)?;
        self.set(|st| st.error = None);

        let every = Duration::from_secs_f32(1.0 / f32::from(self.page.fps.clamp(1, 30)));
        let mut last: Option<(Vec<u8>, Instant)> = None;
        let mut ack: Option<(Value, Instant)> = None;
        let mut last_ack = Instant::now();
        let mut next_data = Instant::now();
        let mut times: VecDeque<Instant> = VecDeque::new();
        let mut back = Backdrop::default();
        // The page's latest frame, laid over a background it hands over from then on.
        let mut last_png = Vec::new();
        // Only the page itself, not a frame inside it, talks to camless.
        let mut page_context = None;
        let (mut ticked, mut next_tick) = (Instant::now(), Instant::now());
        let backdrop_alpha = |cdp: &mut Cdp, a: u8| {
            cdp.send("Emulation.setDefaultBackgroundColorOverride", json!({ "color": { "r": 0, "g": 0, "b": 0, "a": a } }), s)
        };
        loop {
            if self.stopped() {
                let _ = cdp.send("Browser.close", json!({}), None);
                return Ok(());
            }
            let now = Instant::now();
            if now >= next_data {
                let data = api::monitoring(&self.shared.lock().unwrap_or_else(|e| e.into_inner()));
                cdp.send("Runtime.evaluate", json!({ "expression": push_data(&data) }), s)?;
                next_data = now + DATA_EVERY;
            }
            // Acknowledging a frame asks for the next one, so a late ack paces the page.
            if let Some((id, due)) = ack.take() {
                if now >= due {
                    cdp.send("Page.screencastFrameAck", json!({ "sessionId": id }), s)?;
                    last_ack = now;
                } else {
                    ack = Some((id, due));
                }
            }
            // With a background, frames go out at the page's rate whether the page
            // changes or not.
            if back.active() && now >= next_tick {
                if let Some(q565) = back.next(now - ticked) {
                    if self.events.try_send(Event::Frame { device: self.device.clone(), q565: q565.clone() }).is_ok() {
                        times.push_back(now);
                    }
                    last = Some((q565, now));
                }
                ticked = now;
                next_tick += every;
                if next_tick < now {
                    next_tick = now + every;
                }
            }
            if let Some((q565, sent)) = last.as_mut() {
                if sent.elapsed() >= KEEPALIVE {
                    let _ = self.events.try_send(Event::Frame { device: self.device.clone(), q565: q565.clone() });
                    *sent = now;
                }
            }
            while times.front().is_some_and(|t| t.elapsed() > Duration::from_secs(2)) {
                times.pop_front();
            }
            self.set(|st| (st.fps, st.background) = (times.len() as f32 / 2.0, back.frames()));

            let mut wake = next_data.min(now + Duration::from_millis(250));
            if let Some((_, due)) = &ack {
                wake = wake.min(*due);
            }
            if back.active() {
                wake = wake.min(next_tick);
            }
            let ev = match early.pop_front() {
                Some(ev) => ev,
                None => match cdp.next_event(wake.saturating_duration_since(Instant::now()))? {
                    Some(ev) => ev,
                    None => continue,
                },
            };
            match ev["method"].as_str().unwrap_or_default() {
                "Page.screencastFrame" => {
                    let p = &ev["params"];
                    let png = cdp::base64_decode(p["data"].as_str().unwrap_or_default())?;
                    if back.active() {
                        back.page_frame(&png, w, h)?;
                    } else {
                        let q565 = q565_from_png(&png, w, h)?;
                        if self.stopped() {
                            continue;
                        }
                        if self.events.try_send(Event::Frame { device: self.device.clone(), q565: q565.clone() }).is_ok() {
                            times.push_back(Instant::now());
                        }
                        last = Some((q565, Instant::now()));
                    }
                    last_png = png;
                    // On a fixed beat from the last ack, so the time a frame takes to
                    // arrive does not slow the rate down.
                    ack = Some((p["sessionId"].clone(), (last_ack + every).max(Instant::now())));
                }
                "Runtime.executionContextCreated" => {
                    let c = &ev["params"]["context"];
                    if c["auxData"]["frameId"] == frame && c["auxData"]["isDefault"] == true {
                        page_context = c["id"].as_i64();
                    }
                }
                "Runtime.bindingCalled" if ev["params"]["name"] == backdrop::BINDING => {
                    let from = ev["params"]["executionContextId"].as_i64();
                    if from.is_none() || from != page_context {
                        continue;
                    }
                    let msg: Value = serde_json::from_str(ev["params"]["payload"].as_str().unwrap_or_default()).unwrap_or_default();
                    let result = back.message(&msg, w, h);
                    match result {
                        // The page stays as it was until its next frame, which then
                        // comes on nothing.
                        Ok(Change::Started) => {
                            if !last_png.is_empty() {
                                back.page_frame(&last_png, w, h)?;
                            }
                            backdrop_alpha(&mut cdp, 0)?;
                            (ticked, next_tick) = (Instant::now(), Instant::now());
                        }
                        Ok(Change::Stopped) => {
                            backdrop_alpha(&mut cdp, 1)?;
                        }
                        _ => {}
                    }
                    let error = result.err().map(|e| format!("{e:#}"));
                    let id = msg["id"].as_u64().unwrap_or_default();
                    cdp.send("Runtime.evaluate", json!({ "expression": backdrop::reply(id, error.as_deref()), "contextId": from }), s)?;
                }
                // A new document draws its own background until it hands one over again.
                "Runtime.executionContextsCleared" => {
                    (page_context, last_png) = (None, Vec::new());
                    if back.clear() {
                        backdrop_alpha(&mut cdp, 1)?;
                    }
                }
                "Page.loadEventFired" => {
                    ack = None;
                    cdp.send("Page.startScreencast", screencast.clone(), s)?;
                }
                "Inspector.targetCrashed" | "Target.targetCrashed" => bail!("the page crashed"),
                "Inspector.detached" | "Target.detachedFromTarget" => bail!("the page was closed"),
                _ => {}
            }
        }
    }
}

/// What CAM gives a page before its own scripts run, and camless's own addition.
fn shim(page: &Page) -> String {
    let v1 = json!({
        "width": page.width,
        "height": page.height,
        "shape": if page.round { "circle" } else { "square" },
        "targetFps": page.fps,
    });
    format!("window.nzxt = window.nzxt || {{}}; window.nzxt.v1 = Object.assign(window.nzxt.v1 || {{}}, {v1});\n{}", backdrop::BRIDGE)
}

/// JSON is a JavaScript expression, so the data goes in as it is.
fn push_data(data: &Value) -> String {
    format!(
        "(() => {{ const v1 = window.nzxt && window.nzxt.v1; \
         if (v1 && typeof v1.onMonitoringDataUpdate === 'function') v1.onMonitoringDataUpdate({data}); }})()"
    )
}

/// The URL with `kraken=1`, which CAM adds so a page knows it is on the cooler and
/// not in a browser tab. KrakenDial shows its settings without it.
pub fn lcd_url(url: &str) -> String {
    let (base, fragment) = match url.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (url, None),
    };
    let query = base.split_once('?').map(|(_, q)| q);
    let mut out = base.to_string();
    if !query.is_some_and(|q| q.split('&').any(|kv| kv == "kraken" || kv.starts_with("kraken="))) {
        if query.is_none() {
            out.push('?');
        } else if !out.ends_with(['?', '&']) {
            out.push('&');
        }
        out.push_str("kraken=1");
    }
    if let Some(f) = fragment {
        out.push('#');
        out.push_str(f);
    }
    out
}

/// A PNG from the browser as a Q565 frame of `w`×`h`; anything outside the
/// browser's frame is black.
pub fn q565_from_png(png_bytes: &[u8], w: u16, h: u16) -> Result<Vec<u8>> {
    Ok(q565::encode(w, h, &rgb565_from_png(png_bytes, w, h)?))
}

pub fn rgb565_from_png(png_bytes: &[u8], w: u16, h: u16) -> Result<Vec<u16>> {
    let (buf, fw, fh, channels) = decode_png(png_bytes)?;
    let (w, h) = (w as usize, h as usize);
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..fh.min(h) {
        for x in 0..fw.min(w) {
            let i = (y * fw + x) * channels;
            let px = &buf[i..i + channels];
            let (r, g, b) = if channels >= 3 { (px[0], px[1], px[2]) } else { (px[0], px[0], px[0]) };
            rgba[(y * w + x) * 4..][..3].copy_from_slice(&[r, g, b]);
        }
    }
    Ok(q565::rgb565_from_rgba(&rgba))
}

/// Wider or higher than any screen. A page hands frames over, so a PNG is checked
/// against this before anything is allocated for its pixels.
const MAX_SIDE: u32 = 4096;

/// A PNG as 8-bit samples, with its width, height and samples per pixel.
pub fn decode_png(png_bytes: &[u8]) -> Result<(Vec<u8>, usize, usize, usize)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().context("reading a frame")?;
    let (fw, fh) = (reader.info().width, reader.info().height);
    if fw > MAX_SIDE || fh > MAX_SIDE {
        bail!("a frame of {fw}×{fh}, over {MAX_SIDE} pixels a side");
    }
    let mut buf = vec![0; reader.output_buffer_size().context("a frame too large")?];
    let info = reader.next_frame(&mut buf).context("reading a frame")?;
    Ok((buf, fw as usize, fh as usize, info.color_type.samples()))
}

/// `%LOCALAPPDATA%\camless\browser\<device>`: cookies and storage of the pages on
/// that screen, apart from the user's own browser.
fn profile_dir(device: &str) -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let safe: String = device.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    base.join("camless").join("browser").join(safe)
}

fn profile_lock(dir: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    locks.entry(dir.to_path_buf()).or_default().clone()
}

fn find_browser() -> Option<(PathBuf, &'static str)> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let candidates = [
        (env("ProgramFiles(x86)"), r"Microsoft\Edge\Application\msedge.exe", "Microsoft Edge"),
        (env("ProgramFiles"), r"Microsoft\Edge\Application\msedge.exe", "Microsoft Edge"),
        (env("ProgramFiles"), r"Google\Chrome\Application\chrome.exe", "Google Chrome"),
        (env("ProgramFiles(x86)"), r"Google\Chrome\Application\chrome.exe", "Google Chrome"),
        (env("LOCALAPPDATA"), r"Google\Chrome\Application\chrome.exe", "Google Chrome"),
    ];
    candidates.into_iter().find_map(|(dir, rel, name)| Some((dir?.join(rel), name)).filter(|(p, _)| p.is_file()))
}

struct Browser {
    process: win::Process,
    port: u16,
    ws_path: String,
    name: String,
}

impl Browser {
    fn launch(dir: &Path, w: u16, h: u16, stop: &AtomicBool) -> Result<Browser> {
        let (exe, name) = find_browser().context("rendering a web page needs Microsoft Edge or Google Chrome, and neither is installed")?;
        let port_file = dir.join("DevToolsActivePort");
        let _ = std::fs::remove_file(&port_file);
        let args = [
            "--headless=new".to_string(),
            "--remote-debugging-port=0".into(),
            format!("--user-data-dir={}", dir.display()),
            format!("--window-size={w},{h}"),
            "--force-device-scale-factor=1".into(),
            "--hide-scrollbars".into(),
            "--mute-audio".into(),
            "--no-first-run".into(),
            "--no-default-browser-check".into(),
            "--disable-extensions".into(),
            "--disable-sync".into(),
            "--disable-component-update".into(),
            "--disable-background-networking".into(),
            "--disable-background-timer-throttling".into(),
            "--disable-renderer-backgrounding".into(),
            "--disable-backgrounding-occluded-windows".into(),
            "--autoplay-policy=no-user-gesture-required".into(),
            // Without these, headless Edge keeps a GPU process of about 150 MB for a
            // page CAM also draws without a GPU, and a spare renderer next to the page's.
            "--disable-gpu".into(),
            "--disable-gpu-compositing".into(),
            "--renderer-process-limit=1".into(),
            "--disable-features=SpareRendererForSitePerProcess".into(),
            "about:blank".into(),
        ];
        let process = win::spawn(&exe, &args)?;
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            // The file has the port, then the browser's WebSocket path.
            if let Ok(text) = std::fs::read_to_string(&port_file) {
                let mut lines = text.lines();
                if let (Some(Ok(port)), Some(path)) = (lines.next().map(str::parse), lines.next()) {
                    return Ok(Browser { process, port, ws_path: path.to_string(), name: name.into() });
                }
            }
            if let Some(code) = process.exit_code() {
                bail!("{name} exited at start (code {code})");
            }
            if stop.load(Ordering::SeqCst) {
                bail!("stopped");
            }
            if Instant::now() > deadline {
                bail!("{name} did not open its DevTools port");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        // A moment for a clean exit after Browser.close, so the profile is not left
        // marked as crashed; then the job ends whatever is left.
        self.process.wait(Duration::from_secs(3));
    }
}

pub use win::is_elevated;

/// Starting the browser: as the signed-in user, inside a kill-on-close job.
mod win {
    use anyhow::{bail, Result};
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE, WAIT_OBJECT_0};
    use windows_sys::Win32::Security::{
        DuplicateTokenEx, GetTokenInformation, SecurityImpersonation, TokenElevation, TokenPrimary, TOKEN_ADJUST_DEFAULT,
        TOKEN_ADJUST_SESSIONID, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, CreateProcessWithTokenW, GetCurrentProcess, GetExitCodeProcess, OpenProcess, OpenProcessToken, ResumeThread,
        TerminateProcess, WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED, PROCESS_INFORMATION,
        PROCESS_QUERY_LIMITED_INFORMATION, STARTUPINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};

    pub struct Process {
        handle: HANDLE,
        /// None if the process could not be put in a job; it is then ended by itself.
        job: Option<HANDLE>,
    }

    // Plain kernel handles, owned by this struct alone.
    unsafe impl Send for Process {}

    impl Process {
        pub fn exit_code(&self) -> Option<u32> {
            let mut code = 0;
            let ok = unsafe { GetExitCodeProcess(self.handle, &mut code) } != 0;
            (ok && code != STILL_ACTIVE as u32).then_some(code)
        }

        pub fn wait(&self, timeout: Duration) -> bool {
            unsafe { WaitForSingleObject(self.handle, timeout.as_millis() as u32) == WAIT_OBJECT_0 }
        }
    }

    impl Drop for Process {
        fn drop(&mut self) {
            unsafe {
                match self.job {
                    Some(job) => {
                        TerminateJobObject(job, 0);
                        CloseHandle(job);
                    }
                    None => {
                        TerminateProcess(self.handle, 0);
                    }
                }
                CloseHandle(self.handle);
            }
        }
    }

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }

    /// One argument as the C runtime splits a command line.
    fn quote(arg: &str) -> String {
        if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
            return arg.to_string();
        }
        let mut out = String::from('"');
        let mut slashes = 0;
        for c in arg.chars() {
            match c {
                '\\' => slashes += 1,
                '"' => {
                    out.push_str(&"\\".repeat(slashes * 2 + 1));
                    slashes = 0;
                }
                _ => {
                    out.push_str(&"\\".repeat(slashes));
                    slashes = 0;
                }
            }
            if c != '\\' {
                out.push(c);
            }
        }
        out.push_str(&"\\".repeat(slashes * 2));
        out.push('"');
        out
    }

    pub fn is_elevated() -> bool {
        unsafe {
            let mut token = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return false;
            }
            let mut e = TOKEN_ELEVATION { TokenIsElevated: 0 };
            let mut len = 0;
            let size = size_of::<TOKEN_ELEVATION>() as u32;
            let ok = GetTokenInformation(token, TokenElevation, &mut e as *mut _ as *mut c_void, size, &mut len) != 0;
            CloseHandle(token);
            ok && e.TokenIsElevated != 0
        }
    }

    /// A primary token of the desktop's user, from Explorer: an elevated camless
    /// must not hand administrator rights to a web page's browser.
    fn shell_token() -> Result<HANDLE> {
        unsafe {
            let shell = GetShellWindow();
            if shell.is_null() {
                bail!("no desktop shell to start the browser as the signed-in user");
            }
            let mut pid = 0;
            GetWindowThreadProcessId(shell, &mut pid);
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                bail!("opening the desktop shell: {}", std::io::Error::last_os_error());
            }
            let mut token = null_mut();
            let opened = OpenProcessToken(process, TOKEN_DUPLICATE, &mut token) != 0;
            CloseHandle(process);
            if !opened {
                bail!("reading the desktop user's token: {}", std::io::Error::last_os_error());
            }
            let mut primary = null_mut();
            let access = TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT | TOKEN_ADJUST_SESSIONID;
            let ok = DuplicateTokenEx(token, access, null(), SecurityImpersonation, TokenPrimary, &mut primary) != 0;
            CloseHandle(token);
            if !ok {
                bail!("copying the desktop user's token: {}", std::io::Error::last_os_error());
            }
            Ok(primary)
        }
    }

    pub fn spawn(exe: &Path, args: &[String]) -> Result<super::win::Process> {
        let mut cmdline = quote(&exe.to_string_lossy());
        for a in args {
            cmdline.push(' ');
            cmdline.push_str(&quote(a));
        }
        let app = wide(exe.as_os_str());
        let mut cmd = wide(std::ffi::OsStr::new(&cmdline));
        unsafe {
            let mut si: STARTUPINFOW = std::mem::zeroed();
            si.cb = size_of::<STARTUPINFOW>() as u32;
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let started = if is_elevated() {
                let token = shell_token()?;
                let ok = CreateProcessWithTokenW(token, 0, app.as_ptr(), cmd.as_mut_ptr(), CREATE_SUSPENDED, null(), null(), &si, &mut pi);
                CloseHandle(token);
                ok != 0
            } else {
                let flags = CREATE_SUSPENDED | CREATE_NO_WINDOW;
                CreateProcessW(app.as_ptr(), cmd.as_mut_ptr(), null(), null(), 0, flags, null(), null(), &si, &mut pi) != 0
            };
            if !started {
                bail!("starting {}: {}", exe.display(), std::io::Error::last_os_error());
            }
            // Suspended until it is in the job, so none of its child processes escape it.
            let job = CreateJobObjectW(null(), null());
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let size = size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
            let in_job = !job.is_null()
                && SetInformationJobObject(job, JobObjectExtendedLimitInformation, &limits as *const _ as *const c_void, size) != 0
                && AssignProcessToJobObject(job, pi.hProcess) != 0;
            if !in_job && !job.is_null() {
                CloseHandle(job);
            }
            ResumeThread(pi.hThread);
            CloseHandle(pi.hThread);
            Ok(Process { handle: pi.hProcess, job: in_job.then_some(job) })
        }
    }

    #[cfg(test)]
    mod tests {
        use super::quote;

        #[test]
        fn quotes_like_the_c_runtime() {
            assert_eq!(quote("--headless=new"), "--headless=new");
            assert_eq!(quote(r"--user-data-dir=C:\Users\A B\x"), r#""--user-data-dir=C:\Users\A B\x""#);
            assert_eq!(quote(r"C:\dir with space\"), r#""C:\dir with space\\""#);
            assert_eq!(quote(r#"say "hi""#), r#""say \"hi\"""#);
            assert_eq!(quote(""), r#""""#);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_kraken_like_cam() {
        assert_eq!(lcd_url("http://127.0.0.1:30640/"), "http://127.0.0.1:30640/?kraken=1");
        assert_eq!(lcd_url("http://x/lcd?theme=deep"), "http://x/lcd?theme=deep&kraken=1");
        assert_eq!(lcd_url("http://x/?kraken=1&a=b"), "http://x/?kraken=1&a=b");
        assert_eq!(lcd_url("http://x/page#top"), "http://x/page?kraken=1#top");
        assert_eq!(lcd_url("http://x/?"), "http://x/?kraken=1");
    }

    #[test]
    fn data_reaches_the_page_hook() {
        let js = push_data(&json!({ "cpus": [{ "name": "a \"quoted\" </script> cpu" }] }));
        assert!(js.contains(r#"onMonitoringDataUpdate({"cpus":[{"name":"a \"quoted\" </script> cpu"}]})"#));
        let shim = shim(&Page { url: String::new(), fps: 10, width: 640, height: 640, round: true });
        assert!(shim.contains(r#""shape":"circle""#) && shim.contains(r#""targetFps":10"#));
    }

    #[test]
    fn png_frames_become_lcd_frames() {
        // 2×1: red, then blue; the rest of a 3×2 screen is black.
        let mut png_bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png_bytes, 2, 1);
            enc.set_color(png::ColorType::Rgba);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
        }
        assert_eq!(rgb565_from_png(&png_bytes, 3, 2).unwrap(), vec![0xf800, 0x001f, 0, 0, 0, 0]);
        let q = q565_from_png(&png_bytes, 3, 2).unwrap();
        assert_eq!(q565::decode(&q).unwrap(), (3, 2, vec![0xf800, 0x001f, 0, 0, 0, 0]));
    }

    #[test]
    fn a_huge_frame_is_refused_before_it_is_decoded() {
        // a few kB that would decode to 4097×4097 pixels
        let mut png_bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png_bytes, 4097, 4097);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::One);
            enc.write_header().unwrap().write_image_data(&vec![0; 4097 * 513]).unwrap();
        }
        let err = decode_png(&png_bytes).unwrap_err().to_string();
        assert!(err.contains("4097×4097"), "{err}");
        assert!(rgb565_from_png(&png_bytes, 640, 640).is_err());
    }
}
