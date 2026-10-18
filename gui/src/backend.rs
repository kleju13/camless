//! What the page asks of the app: the daemon's status, `camless.toml` and the
//! profiles beside it, and a few things only a desktop program can do. The window
//! reaches it through WebView2's custom protocol, `--serve` through loopback HTTP;
//! both pass every request to [`Backend::handle`].
//!
//! The page keeps the draft and sends the whole config to save; the daemon applies
//! each saved change by itself, so closing the app never stops fan control.

use crate::service::{self, Action};
use crate::{assets, client, tray, update};
use anyhow::{anyhow, Context, Result};
use camless_core::cam_import;
use camless_core::config::{self, Config, LcdMode, Setup};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Reply {
    fn json(status: u16, v: &Value) -> Reply {
        Reply { status, content_type: "application/json", body: v.to_string().into_bytes() }
    }

    fn error(status: u16, msg: impl Into<String>) -> Reply {
        Reply::json(status, &json!({ "error": msg.into() }))
    }

    pub fn empty(status: u16) -> Reply {
        Reply { status, content_type: "text/plain; charset=utf-8", body: Vec::new() }
    }
}

/// A request the page sent that the app cannot carry out, with the status to answer.
struct Refused(u16, String);

impl From<anyhow::Error> for Refused {
    fn from(e: anyhow::Error) -> Refused {
        Refused(400, format!("{e:#}"))
    }
}

type Answer = std::result::Result<Value, Refused>;

pub struct Options {
    /// The config to edit. Without one the app edits the file the service runs from.
    pub config: Option<PathBuf>,
    /// Serve the page from this folder instead of the copy built in, to work on it.
    pub ui_dir: Option<PathBuf>,
    /// For a browser during development: nothing outside the config files changes.
    /// The service is not started, Start with Windows is only pretended, and no
    /// Explorer window opens.
    pub sandbox: bool,
}

pub struct Backend {
    inner: Mutex<Inner>,
    ui_dir: Option<PathBuf>,
    sandbox: bool,
    /// Called when the page has saved its last edits before the window closes.
    on_closed: Option<Box<dyn Fn() + Send + Sync>>,
}

struct Inner {
    path: PathBuf,
    follow_service: bool,
    link: Option<Arc<Mutex<client::Link>>>,
    /// Start with Windows, in a sandbox.
    autostart: bool,
}

impl Backend {
    pub fn new(o: Options, on_closed: Option<Box<dyn Fn() + Send + Sync>>) -> Backend {
        let follow_service = o.config.is_none();
        let path = o.config.unwrap_or_else(config::default_path);
        Backend {
            inner: Mutex::new(Inner { path, follow_service, link: None, autostart: false }),
            ui_dir: o.ui_dir,
            sandbox: o.sandbox,
            on_closed,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn path(&self) -> PathBuf {
        self.lock().path.clone()
    }

    /// `path` is the request target, with or without a query.
    pub fn handle(&self, method: &str, path: &str, content_type: Option<&str>, body: &[u8]) -> Reply {
        let path = path.split(['?', '#']).next().unwrap_or("/");
        let Some(route) = path.strip_prefix("/api/") else {
            if method != "GET" {
                return Reply::empty(405);
            }
            return match assets::get(path, self.ui_dir.as_deref()) {
                Some((content_type, body)) => Reply { status: 200, content_type, body },
                None => Reply::empty(404),
            };
        };
        // Pages elsewhere can send plain text across sites without asking first, but
        // not JSON, so a page on the web cannot change the config through `--serve`.
        if method != "GET" && content_type.and_then(|c| c.split(';').next()).map(str::trim) != Some("application/json") {
            return Reply::error(415, "Content-Type must be application/json");
        }
        let answer = match (method, route) {
            ("GET", "state") => Ok(self.state()),
            ("PUT", "config") => self.put_config(body),
            ("POST", "profiles/switch") => self.switch_profile(body),
            ("POST", "profiles/save-current") => self.save_current(body),
            ("POST", "profiles/create") => self.create_profile(body),
            ("POST", "profiles/rename") => self.rename_profile(body),
            ("POST", "profiles/delete") => self.delete_profile(body),
            ("POST", "import/cam") => self.import_cam(),
            ("POST", "import/file") => self.import_file(body),
            ("POST", "service/start") => self.service(Action::Start),
            ("POST", "service/stop") => self.service(Action::Stop),
            ("POST", "service/restart") => self.service(Action::Restart),
            ("PUT", "autostart") => self.set_autostart(body),
            ("POST", "update/check") => update::check().map(|_| json!({})).map_err(Refused::from),
            ("POST", "update/install") => self.install_update(),
            ("POST", "update/notes") => self.release_notes(),
            ("POST", "open-folder") => self.open_folder(),
            ("POST", "window/closed") => {
                if let Some(f) = &self.on_closed {
                    f();
                }
                Ok(json!({}))
            }
            _ => return Reply::empty(404),
        };
        match answer {
            Ok(v) => Reply::json(200, &v),
            Err(Refused(code, msg)) => Reply::error(code, msg),
        }
    }

    fn state(&self) -> Value {
        let mut inner = self.lock();
        let (status, link_error) = match &inner.link {
            Some(link) => {
                let l = link.lock().unwrap_or_else(|e| e.into_inner());
                (l.status.clone(), l.error.clone())
            }
            None => (None, None),
        };
        // Edit the file the service actually runs from.
        if let Some(s) = status.as_ref().filter(|_| inner.follow_service) {
            let p = PathBuf::from(&s.config_path);
            if p.is_absolute() {
                inner.path = p;
            }
        }
        let path = inner.path.clone();
        let (config, load_error) = match read_config(&path) {
            Ok(c) => (Some(c), None),
            Err(e) => (None, Some(format!("{e:#}"))),
        };
        let addr = config.as_ref().map_or_else(|| camless_core::DEFAULT_API.to_string(), |c| c.api.listen.clone());
        match &inner.link {
            Some(link) => link.lock().unwrap_or_else(|e| e.into_inner()).addr = addr,
            None => inner.link = Some(client::spawn(addr)),
        }
        let sv = if self.sandbox { service::State::default() } else { service::state() };
        let autostart = if self.sandbox { inner.autostart } else { tray::autostart::enabled() };
        drop(inner);

        let profiles: Vec<Value> = config::list_profiles(&path)
            .into_iter()
            .map(|name| match config::load_profile(&path, &name) {
                Ok(s) => json!({ "name": name, "summary": summary(&s) }),
                Err(e) => json!({ "name": name, "error": format!("{e:#}") }),
            })
            .collect();
        json!({
            "app_version": env!("CARGO_PKG_VERSION"),
            "config_path": path.display().to_string(),
            "config": config,
            "load_error": load_error,
            "profiles": profiles,
            "status": status,
            "link_error": link_error,
            "can_start": !self.sandbox && client::daemon_exe().is_some(),
            "starting": matches!(sv.busy, Some("starting" | "restarting")),
            "service_busy": sv.busy,
            "service_note": sv.note,
            "autostart": autostart,
            "update": update::state(),
            "sandbox": self.sandbox,
        })
    }

    /// The config on disk, to change. A file with a mistake stays as it is: the page
    /// must not replace what someone wrote by hand.
    fn current(&self) -> std::result::Result<(PathBuf, Config), Refused> {
        let path = self.path();
        match read_config(&path) {
            Ok(c) => Ok((path, c)),
            Err(e) => Err(Refused(409, format!("{e:#}\nFix the file first; camless does not overwrite it."))),
        }
    }

    fn put_config(&self, body: &[u8]) -> Answer {
        let (path, _) = self.current()?;
        let cfg: Config = serde_json::from_slice(body).map_err(|e| Refused(400, format!("{e}")))?;
        cfg.save(&path)?;
        Ok(json!({ "config": Config::load(&path)? }))
    }

    fn switch_profile(&self, body: &[u8]) -> Answer {
        let Name { name } = parse(body)?;
        let (path, mut cfg) = self.current()?;
        if cfg.profile.is_none() && !cfg.setup().is_empty() {
            return Err(Refused(409, "the current setup is in no profile; save it as one first".into()));
        }
        cfg.set_setup(config::load_profile(&path, &name)?);
        cfg.profile = Some(name);
        cfg.save(&path)?;
        Ok(json!({}))
    }

    /// Puts the setup that is in no profile into a new one, and runs that.
    fn save_current(&self, body: &[u8]) -> Answer {
        let Name { name } = parse(body)?;
        let (path, mut cfg) = self.current()?;
        if let Some(p) = &cfg.profile {
            return Err(Refused(409, format!("the setup is already profile {p:?}")));
        }
        config::create_profile(&path, &name, &cfg.setup())?;
        cfg.profile = Some(name);
        cfg.save(&path)?;
        Ok(json!({}))
    }

    fn create_profile(&self, body: &[u8]) -> Answer {
        #[derive(Deserialize)]
        struct Create {
            name: String,
            /// Start from the running setup instead of an empty one.
            copy: bool,
        }
        let Create { name, copy } = parse(body)?;
        let (path, cfg) = self.current()?;
        let setup = if copy { cfg.setup() } else { Setup::default() };
        config::create_profile(&path, &name, &setup)?;
        Ok(json!({ "name": name }))
    }

    fn rename_profile(&self, body: &[u8]) -> Answer {
        #[derive(Deserialize)]
        struct Rename {
            from: String,
            to: String,
        }
        let Rename { from, to } = parse(body)?;
        config::check_profile_name(&to)?;
        if from == to {
            return Ok(json!({}));
        }
        let (path, mut cfg) = self.current()?;
        let (old, new) = (config::profile_path(&path, &from)?, config::profile_path(&path, &to)?);
        let case_only = config::same_profile_name(&from, &to);
        if new.exists() && !case_only {
            bail_refused(format!("there is already a profile called {to:?}"))?;
        }
        let running = cfg.profile.as_deref().is_some_and(|p| config::same_profile_name(p, &from));
        if running && !case_only {
            // The new file comes first, so the service never runs a profile that is missing.
            config::save_profile(&path, &to, &cfg.setup())?;
            cfg.profile = Some(to);
            cfg.save(&path)?;
            config::delete_profile(&path, &from)?;
        } else {
            // A rename that only changes case keeps the file, which Windows still finds
            // under the old name until the config names the new one.
            std::fs::rename(&old, &new).with_context(|| format!("renaming {}", old.display()))?;
            if running {
                cfg.profile = Some(to);
                cfg.save(&path)?;
            }
        }
        Ok(json!({}))
    }

    fn delete_profile(&self, body: &[u8]) -> Answer {
        let Name { name } = parse(body)?;
        let (path, cfg) = self.current()?;
        if cfg.profile.as_deref().is_some_and(|p| config::same_profile_name(p, &name)) {
            bail_refused(format!("{name:?} is running; switch to another profile before deleting it"))?;
        }
        config::delete_profile(&path, &name)?;
        Ok(json!({}))
    }

    /// A new profile from what NZXT CAM is set to, and what did not carry over.
    fn import_cam(&self) -> Answer {
        let (path, _) = self.current()?;
        let imported = cam_import::import(&cam_import::default_dir())?;
        let name = config::unused_profile_name(&path, "NZXT CAM");
        config::create_profile(&path, &name, &imported.config.setup())?;
        Ok(json!({ "name": name, "notes": imported.notes }))
    }

    /// A new profile from a camless.toml someone picked: its curves, fans, lights and
    /// LCDs. Nothing running changes until they switch to it.
    fn import_file(&self, body: &[u8]) -> Answer {
        #[derive(Deserialize)]
        struct File {
            name: String,
            text: String,
        }
        let File { name, text } = parse(body)?;
        let (path, _) = self.current()?;
        let file = Config::parse(&text)?;
        if file.setup().is_empty() {
            bail_refused(match &file.profile {
                Some(p) => format!("this file runs profile {p:?}; import profiles/{p}.toml instead"),
                None => "this file has no curves, fans, lights or LCDs".into(),
            })?;
        }
        let name = config::unused_profile_name(&path, &profile_name_from(&name));
        config::create_profile(&path, &name, &file.setup())?;
        Ok(json!({ "name": name }))
    }

    fn service(&self, action: Action) -> Answer {
        if self.sandbox {
            bail_refused("the sandbox does not start or stop the service")?;
        }
        // The file the service runs from, so a restart does not switch it to another.
        service::spawn(action, self.path())?;
        Ok(json!({}))
    }

    fn set_autostart(&self, body: &[u8]) -> Answer {
        #[derive(Deserialize)]
        struct On {
            on: bool,
        }
        let On { on } = parse(body)?;
        if self.sandbox {
            self.lock().autostart = on;
            return Ok(json!({ "autostart": on }));
        }
        tray::autostart::set(on)?;
        Ok(json!({ "autostart": tray::autostart::enabled() }))
    }

    fn install_update(&self) -> Answer {
        if self.sandbox {
            bail_refused("the sandbox does not install updates")?;
        }
        update::install()?;
        Ok(json!({}))
    }

    fn release_notes(&self) -> Answer {
        if self.sandbox {
            bail_refused("the sandbox does not open a browser")?;
        }
        let page = update::page().ok_or_else(|| anyhow!("no release found yet; check for updates first"))?;
        std::process::Command::new("explorer.exe").arg(page).spawn().context("opening the browser")?;
        Ok(json!({}))
    }

    fn open_folder(&self) -> Answer {
        if self.sandbox {
            bail_refused("the sandbox does not open Explorer")?;
        }
        let path = self.path();
        let dir = path.parent().filter(|d| d.exists()).ok_or_else(|| anyhow!("{} has no folder yet", path.display()))?;
        std::process::Command::new("explorer.exe").arg(dir).spawn().context("starting Explorer")?;
        Ok(json!({}))
    }
}

#[derive(Deserialize)]
struct Name {
    name: String,
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> std::result::Result<T, Refused> {
    serde_json::from_slice(body).map_err(|e| Refused(400, format!("{e}")))
}

fn bail_refused(msg: impl Into<String>) -> std::result::Result<(), Refused> {
    Err(Refused(409, msg.into()))
}

/// The config and its profile; a file that is not there yet is an empty config.
fn read_config(path: &Path) -> Result<Config> {
    match Config::load(path) {
        Ok(c) => Ok(c),
        Err(_) if !path.exists() => Ok(Config::default()),
        Err(e) => Err(e),
    }
}

/// A profile name from a file name, or "Imported" if nothing usable is left.
fn profile_name_from(file: &str) -> String {
    let stem = match file.len().checked_sub(5) {
        Some(i) if file.is_char_boundary(i) && file[i..].eq_ignore_ascii_case(".toml") => &file[..i],
        _ => file,
    };
    let kept: String = stem.chars().filter(|c| !c.is_control() && !r#"<>:"/\|?*"#.contains(*c)).take(60).collect();
    let name = kept.trim().trim_end_matches('.').trim_end().to_string();
    if config::check_profile_name(&name).is_ok() {
        name
    } else {
        "Imported".into()
    }
}

/// What a profile sets up, for the list of profiles.
fn summary(s: &Setup) -> String {
    let managed = s.fans.iter().filter(|f| f.curve.is_some()).count();
    let lights = s.lights.iter().filter(|l| l.mode != "off").count();
    let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut parts = vec![plural(managed, "fan", "fans"), plural(lights, "light", "lights")];
    for l in &s.lcds {
        parts.push(match l.mode {
            LcdMode::Liquid => "LCD: liquid".into(),
            LcdMode::Frames => "LCD: API frames".into(),
            LcdMode::Web => "LCD: web page".into(),
        });
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The demo config in a folder of its own, so the backend can save it.
    fn fixture(dir: &str) -> (Backend, PathBuf) {
        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
        let dir = std::env::temp_dir().join(dir);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("camless.toml");
        std::fs::copy(data.join("demo.toml"), &cfg).unwrap();
        let b = Backend::new(Options { config: Some(cfg.clone()), ui_dir: None, sandbox: true }, None);
        (b, cfg)
    }

    fn call(b: &Backend, method: &str, path: &str, body: Value) -> (u16, Value) {
        let r = b.handle(method, path, Some("application/json"), body.to_string().as_bytes());
        (r.status, serde_json::from_slice(&r.body).unwrap_or(Value::Null))
    }

    #[test]
    fn the_page_and_its_files_are_served() {
        let (b, _) = fixture("camless-gui-assets");
        let r = b.handle("GET", "/", None, &[]);
        assert_eq!((r.status, r.content_type), (200, "text/html; charset=utf-8"));
        assert_eq!(b.handle("GET", "/app.js?v=1", None, &[]).status, 200);
        assert_eq!(b.handle("GET", "/../Cargo.toml", None, &[]).status, 404);
    }

    #[test]
    fn a_saved_config_is_what_the_daemon_reads() {
        let (b, cfg) = fixture("camless-gui-save");
        let (code, state) = call(&b, "GET", "/api/state", Value::Null);
        assert_eq!(code, 200);
        let mut config = state["config"].clone();
        config["curve"][0]["points"] = json!([[30, 25], [50, 30], [65, 45], [75, 70], [85, 100]]);
        let (code, _) = call(&b, "PUT", "/api/config", config);
        assert_eq!(code, 200);
        let saved = Config::load(&cfg).unwrap();
        let Some(config::CurveSpec::Graph(g)) = saved.curve("GPU") else { panic!("GPU curve missing") };
        assert_eq!(g.points, vec![(30.0, 25), (50.0, 30), (65.0, 45), (75.0, 70), (85.0, 100)]);
    }

    #[test]
    fn a_config_that_does_not_validate_is_not_saved() {
        let (b, cfg) = fixture("camless-gui-invalid");
        let before = std::fs::read_to_string(&cfg).unwrap();
        let (_, state) = call(&b, "GET", "/api/state", Value::Null);
        let mut config = state["config"].clone();
        config["fan"][0]["curve"] = json!("No such curve");
        let (code, reply) = call(&b, "PUT", "/api/config", config);
        assert_eq!(code, 400);
        assert!(reply["error"].as_str().unwrap().contains("does not exist"), "{reply}");
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), before);
    }

    #[test]
    fn a_file_with_a_mistake_is_left_alone() {
        let (b, cfg) = fixture("camless-gui-broken");
        std::fs::write(&cfg, "interval_ms = \n").unwrap();
        let (_, state) = call(&b, "GET", "/api/state", Value::Null);
        assert!(state["config"].is_null() && state["load_error"].is_string());
        let (code, _) = call(&b, "PUT", "/api/config", json!({}));
        assert_eq!(code, 409);
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "interval_ms = \n");
    }

    #[test]
    fn plain_text_cannot_change_anything() {
        let (b, cfg) = fixture("camless-gui-csrf");
        let before = std::fs::read_to_string(&cfg).unwrap();
        let r = b.handle("PUT", "/api/config", Some("text/plain"), b"{}");
        assert_eq!(r.status, 415);
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), before);
    }

    #[test]
    fn switching_profiles_keeps_both_setups() {
        let (b, cfg) = fixture("camless-gui-profiles");
        let original = Config::load(&cfg).unwrap();
        assert_eq!(call(&b, "POST", "/api/profiles/switch", json!({ "name": "Mine" })).0, 409, "the setup must be saved first");
        assert_eq!(call(&b, "POST", "/api/profiles/save-current", json!({ "name": "Mine" })).0, 200);
        assert_eq!(call(&b, "POST", "/api/profiles/create", json!({ "name": "Empty", "copy": false })).0, 200);
        assert_eq!(call(&b, "POST", "/api/profiles/switch", json!({ "name": "Empty" })).0, 200);
        let running = Config::load(&cfg).unwrap();
        assert_eq!(running.profile.as_deref(), Some("Empty"));
        assert!(running.setup().is_empty());

        assert_eq!(call(&b, "POST", "/api/profiles/switch", json!({ "name": "Mine" })).0, 200);
        assert_eq!(call(&b, "POST", "/api/profiles/rename", json!({ "from": "Mine", "to": "Mine v2" })).0, 200);
        let back = Config::load(&cfg).unwrap();
        assert_eq!(back.profile.as_deref(), Some("Mine v2"));
        assert_eq!(back.setup(), original.setup());
        assert_eq!(config::list_profiles(&cfg), vec!["Empty", "Mine v2"]);
        assert_eq!(call(&b, "POST", "/api/profiles/delete", json!({ "name": "Mine v2" })).0, 409, "the running profile must stay");
        assert_eq!(call(&b, "POST", "/api/profiles/delete", json!({ "name": "MINE V2" })).0, 409, "under any case");
        assert_eq!(call(&b, "POST", "/api/profiles/delete", json!({ "name": "Empty" })).0, 200);
        let (_, state) = call(&b, "GET", "/api/state", Value::Null);
        assert_eq!(state["profiles"][0]["name"], "Mine v2");
    }

    #[test]
    fn an_imported_file_becomes_a_profile() {
        let (b, cfg) = fixture("camless-gui-import");
        let before = std::fs::read_to_string(&cfg).unwrap();
        let text = std::fs::read_to_string(&cfg).unwrap();
        let (code, reply) = call(&b, "POST", "/api/import/file", json!({ "name": "camless.toml", "text": text }));
        assert_eq!(code, 200, "{reply}");
        assert_eq!(reply["name"], "camless");
        assert_eq!(config::load_profile(&cfg, "camless").unwrap(), Config::load(&cfg).unwrap().setup());
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), before, "nothing running changes");
        let (code, _) = call(&b, "POST", "/api/import/file", json!({ "name": "x", "text": "interval_ms = 1000\n" }));
        assert_eq!(code, 409);
    }

    #[test]
    fn the_sandbox_touches_nothing_outside_the_config() {
        let (b, _) = fixture("camless-gui-sandbox");
        for action in ["start", "stop", "restart"] {
            assert_eq!(call(&b, "POST", &format!("/api/service/{action}"), json!({})).0, 409);
        }
        assert_eq!(call(&b, "GET", "/api/state", Value::Null).1["service_busy"], Value::Null);
        assert_eq!(call(&b, "POST", "/api/open-folder", json!({})).0, 409);
        assert_eq!(call(&b, "PUT", "/api/autostart", json!({ "on": true })).1["autostart"], true);
        assert_eq!(call(&b, "GET", "/api/state", Value::Null).1["autostart"], true);
    }
}
