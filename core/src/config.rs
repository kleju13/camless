//! `camless.toml` and profiles.
//!
//! Curves are named and reusable, as in FanControl: a graph or trigger on one sensor,
//! a fixed duty, or a mix of other curves. A fan channel picks a curve by name and
//! adds its own limits. Channels are `<device>/<channel>`, with device ids from
//! `camless status`, e.g. `kraken-elite-v2/pump` or `control-hub/fan1`.
//!
//! A profile is a whole setup (curves, fans, lights, LCDs) in `profiles/<name>.toml`
//! next to `camless.toml`, which then keeps only the service settings and
//! `profile = "<name>"`. Without a profile the setup is in `camless.toml` itself.

use crate::lighting::Light;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// How often sensors are read and host-driven fans are adjusted.
    #[serde(default = "default_interval_ms")]
    pub interval_ms: u64,
    /// Duty for fans whose curve has no reading (a sensor that is missing or failed),
    /// and for every host-driven fan when camless stops.
    #[serde(default = "default_failsafe_duty")]
    pub failsafe_duty: u8,
    /// The profile with the curves, fans, lights and LCDs, in `profiles/<name>.toml`.
    /// Without one they are in this file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Signed PawnIO `AMDFamily17.bin` for CPU temperature and power. Defaults to
    /// `AMDFamily17.bin` next to camless.exe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pawnio_module: Option<PathBuf>,
    /// Which releases the app offers as updates.
    #[serde(default, skip_serializing_if = "Updates::is_stable")]
    pub updates: Updates,
    #[serde(default, skip_serializing_if = "Api::is_default")]
    pub api: Api,
    #[serde(default, skip_serializing_if = "Games::is_default")]
    pub games: Games,
    #[serde(default, rename = "curve", skip_serializing_if = "Vec::is_empty")]
    pub curves: Vec<CurveSpec>,
    #[serde(default, rename = "fan", skip_serializing_if = "Vec::is_empty")]
    pub fans: Vec<Fan>,
    #[serde(default, rename = "light", skip_serializing_if = "Vec::is_empty")]
    pub lights: Vec<Light>,
    #[serde(default, rename = "lcd", skip_serializing_if = "Vec::is_empty")]
    pub lcds: Vec<Lcd>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            interval_ms: default_interval_ms(),
            failsafe_duty: default_failsafe_duty(),
            profile: None,
            pawnio_module: None,
            updates: Updates::Stable,
            api: Api::default(),
            games: Games::default(),
            curves: Vec::new(),
            fans: Vec::new(),
            lights: Vec::new(),
            lcds: Vec::new(),
        }
    }
}

/// What a profile holds: everything but the service settings.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    #[serde(default, rename = "curve", skip_serializing_if = "Vec::is_empty")]
    pub curves: Vec<CurveSpec>,
    #[serde(default, rename = "fan", skip_serializing_if = "Vec::is_empty")]
    pub fans: Vec<Fan>,
    #[serde(default, rename = "light", skip_serializing_if = "Vec::is_empty")]
    pub lights: Vec<Light>,
    #[serde(default, rename = "lcd", skip_serializing_if = "Vec::is_empty")]
    pub lcds: Vec<Lcd>,
}

impl Setup {
    pub fn is_empty(&self) -> bool {
        self.curves.is_empty() && self.fans.is_empty() && self.lights.is_empty() && self.lcds.is_empty()
    }

    pub fn validate(&self) -> Result<()> {
        validate_setup(&self.curves, &self.fans, &self.lights, &self.lcds)
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string(self)?)
    }
}

fn default_interval_ms() -> u64 {
    1000
}

fn default_failsafe_duty() -> u8 {
    70
}

/// The loopback API the GUI and LCD pages read.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Api {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "default_listen")]
    pub listen: String,
}

impl Default for Api {
    fn default() -> Api {
        Api { enabled: true, listen: default_listen() }
    }
}

impl Api {
    fn is_default(&self) -> bool {
        *self == Api::default()
    }
}

/// Which releases the app offers. A version that changes only the last number, such
/// as 0.4.1, comes out as a beta, a GitHub pre-release, until it is promoted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Updates {
    /// Releases only.
    #[default]
    Stable,
    /// Betas too.
    Beta,
}

impl Updates {
    fn is_stable(&self) -> bool {
        *self == Updates::Stable
    }
}

/// Game sessions, off until turned on: the service then follows the program in front.
/// `add` lists programs that count as games besides those from Steam, Epic Games and
/// Xbox, and `skip` programs that never do. An entry is a program's file name,
/// `javaw.exe`, or a folder, `D:\Emulators`, for everything in it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Games {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skip: Vec<String>,
}

impl Games {
    fn is_default(&self) -> bool {
        *self == Games::default()
    }
}

fn yes() -> bool {
    true
}

fn default_listen() -> String {
    crate::DEFAULT_API.into()
}

/// A point of a graph curve: (sensor value, duty %).
pub type Point = (f32, u8);

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CurveSpec {
    Graph(Graph),
    Fixed(Fixed),
    Mix(Mix),
    Trigger(Trigger),
}

impl CurveSpec {
    pub fn name(&self) -> &str {
        match self {
            CurveSpec::Graph(c) => &c.name,
            CurveSpec::Fixed(c) => &c.name,
            CurveSpec::Mix(c) => &c.name,
            CurveSpec::Trigger(c) => &c.name,
        }
    }

    pub fn set_name(&mut self, name: String) {
        match self {
            CurveSpec::Graph(c) => c.name = name,
            CurveSpec::Fixed(c) => c.name = name,
            CurveSpec::Mix(c) => c.name = name,
            CurveSpec::Trigger(c) => c.name = name,
        }
    }

    pub fn sensor(&self) -> Option<&str> {
        match self {
            CurveSpec::Graph(c) => Some(&c.sensor),
            CurveSpec::Trigger(c) => Some(&c.sensor),
            _ => None,
        }
    }
}

/// Duty from a sensor through points, linear in between and flat beyond the ends.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Graph {
    pub name: String,
    /// A sensor id from `camless status`, e.g. `cpu.temp` or `kraken-elite-v2.liquid`.
    pub sensor: String,
    pub points: Vec<Point>,
    /// The sensor must move this far from the value the duty was last set at.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub hysteresis: f32,
    /// Seconds to cover about two thirds of a rise; 0 follows at once.
    #[serde(default = "default_response_up", skip_serializing_if = "is_default_up")]
    pub response_up: f32,
    /// The same for a fall. Slower than the rise by default, so a short dip in load
    /// does not make the fans surge and settle.
    #[serde(default = "default_response_down", skip_serializing_if = "is_default_down")]
    pub response_down: f32,
}

impl Graph {
    pub fn response(&self) -> Response {
        Response { hysteresis: self.hysteresis, response_up: self.response_up, response_down: self.response_down }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fixed {
    pub name: String,
    pub duty: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MixFunction {
    Max,
    Min,
    Average,
    Sum,
}

/// Combines other curves, e.g. the higher of a CPU and a GPU curve.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Mix {
    pub name: String,
    pub curves: Vec<String>,
    pub function: MixFunction,
}

/// Two duties: `load_duty` once the sensor reaches `load_at`, back to `idle_duty`
/// only when it falls to `idle_at`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Trigger {
    pub name: String,
    pub sensor: String,
    pub idle_duty: u8,
    pub load_duty: u8,
    pub idle_at: f32,
    pub load_at: f32,
    #[serde(default = "default_response_up", skip_serializing_if = "is_default_up")]
    pub response_up: f32,
    #[serde(default = "default_response_down", skip_serializing_if = "is_default_down")]
    pub response_down: f32,
}

impl Trigger {
    pub fn response(&self) -> Response {
        Response { hysteresis: 0.0, response_up: self.response_up, response_down: self.response_down }
    }
}

/// How a curve follows its sensor. This happens on the host only: curves the cooler
/// runs by itself see the raw liquid temperature.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Response {
    pub hysteresis: f32,
    pub response_up: f32,
    pub response_down: f32,
}

impl Default for Response {
    fn default() -> Response {
        Response { hysteresis: 0.0, response_up: default_response_up(), response_down: default_response_down() }
    }
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn default_response_up() -> f32 {
    2.0
}

fn default_response_down() -> f32 {
    10.0
}

fn is_default_up(v: &f32) -> bool {
    *v == default_response_up()
}

fn is_default_down(v: &f32) -> bool {
    *v == default_response_down()
}

/// One fan or pump channel and the curve that drives it.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fan {
    pub channel: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The curve by name. Without one, camless leaves the channel alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<String>,
    /// Added to the curve's duty before the limits.
    #[serde(default, skip_serializing_if = "is_zero_i8")]
    pub offset: i8,
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub min: u8,
    #[serde(default = "hundred", skip_serializing_if = "is_hundred")]
    pub max: u8,
    /// Fastest change in % per second, up and down; 0 means no limit.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub step_up: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub step_down: f32,
}

impl Fan {
    pub fn new(channel: &str) -> Fan {
        Fan { channel: channel.into(), name: String::new(), curve: None, offset: 0, min: 0, max: 100, step_up: 0.0, step_down: 0.0 }
    }
}

fn is_zero_i8(v: &i8) -> bool {
    *v == 0
}

fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

fn hundred() -> u8 {
    100
}

fn is_hundred(v: &u8) -> bool {
    *v == 100
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LcdMode {
    /// The device's own screen (liquid temperature on a Kraken); no host work at all.
    #[default]
    Liquid,
    /// Frames posted to `POST /v1/lcd/frame`.
    Frames,
    /// A web page, rendered by camless the way CAM's Web Integration does.
    Web,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Lcd {
    pub device: String,
    #[serde(default)]
    pub mode: LcdMode,
    #[serde(default = "hundred")]
    pub brightness: u8,
    /// 0, 90, 180 or 270.
    #[serde(default)]
    pub orientation: u16,
    /// For `mode = "web"`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// For `mode = "web"`: frames per second, 1 to 30.
    #[serde(default = "default_fps")]
    pub fps: u8,
}

fn default_fps() -> u8 {
    10
}

/// `camless.toml` next to camless.exe if there is one (a portable setup), else
/// `%APPDATA%\camless\camless.toml`.
pub fn default_path() -> PathBuf {
    let beside = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("camless.toml")));
    if let Some(p) = beside.filter(|p| p.exists()) {
        return p;
    }
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
    appdata.join("camless").join("camless.toml")
}

fn split_channel(channel: &str) -> Option<(&str, &str)> {
    channel.split_once('/').filter(|(d, c)| !d.is_empty() && !c.is_empty() && !c.contains('/'))
}

impl Config {
    /// Reads `path` and, if it names one, the profile it selects.
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut cfg = Config::parse_unchecked(&text).with_context(|| format!("in {}", path.display()))?;
        cfg.validate_service().with_context(|| format!("in {}", path.display()))?;
        match cfg.profile.clone() {
            Some(name) => {
                if !cfg.setup().is_empty() {
                    bail!(
                        "{} selects profile {name:?} and also has curves, fans, lights or LCDs of its own; move them into the profile",
                        path.display()
                    );
                }
                cfg.set_setup(load_profile(path, &name)?);
            }
            None => cfg.setup().validate().with_context(|| format!("in {}", path.display()))?,
        }
        Ok(cfg)
    }

    /// One file on its own; a profile it names is not read.
    pub fn parse(text: &str) -> Result<Config> {
        let cfg = Config::parse_unchecked(text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn parse_unchecked(text: &str) -> Result<Config> {
        toml::from_str(text).map_err(|e| {
            let old = ["pump", "radiator", "hub_fan", "hub_led", "ring", "external"];
            if old.iter().any(|k| text.lines().any(|l| l.trim_start().trim_start_matches('[').starts_with(k))) {
                anyhow::anyhow!("{e}\nThis is a camless 0.1 config; `camless import-cam` writes the new format.")
            } else {
                anyhow::anyhow!("{e}")
            }
        })
    }

    /// Writes through temporary files, so the daemon never reads half a config. With
    /// a profile, the setup goes to the profile's file and `path` keeps the rest.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        match &self.profile {
            Some(name) => {
                save_profile(path, name, &self.setup())?;
                let mut service = self.clone();
                service.set_setup(Setup::default());
                write_file(path, &service.to_toml()?)
            }
            None => write_file(path, &self.to_toml()?),
        }
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string(self)?)
    }

    pub fn setup(&self) -> Setup {
        Setup { curves: self.curves.clone(), fans: self.fans.clone(), lights: self.lights.clone(), lcds: self.lcds.clone() }
    }

    pub fn set_setup(&mut self, s: Setup) {
        (self.curves, self.fans, self.lights, self.lcds) = (s.curves, s.fans, s.lights, s.lcds);
    }

    pub fn curve(&self, name: &str) -> Option<&CurveSpec> {
        self.curves.iter().find(|c| c.name() == name)
    }

    pub fn validate(&self) -> Result<()> {
        self.validate_service()?;
        validate_setup(&self.curves, &self.fans, &self.lights, &self.lcds)
    }

    fn validate_service(&self) -> Result<()> {
        if !(100..=10_000).contains(&self.interval_ms) {
            bail!("interval_ms must be 100 to 10000");
        }
        if self.failsafe_duty > 100 {
            bail!("failsafe_duty must be 0 to 100");
        }
        if let Some(name) = &self.profile {
            check_profile_name(name)?;
        }
        if self.games.add.iter().chain(&self.games.skip).any(|e| e.trim().is_empty()) {
            bail!("an entry in [games] is empty");
        }
        Ok(())
    }
}

fn validate_setup(curves: &[CurveSpec], fans: &[Fan], lights: &[Light], lcds: &[Lcd]) -> Result<()> {
    let names: HashSet<&str> = curves.iter().map(|c| c.name()).collect();
    if names.len() != curves.len() {
        bail!("two curves share a name");
    }
    for c in curves {
        check_curve(c, &names).with_context(|| format!("curve {:?}", c.name()))?;
    }
    curve_order(curves)?;
    let mut seen = HashSet::new();
    for f in fans {
        if split_channel(&f.channel).is_none() {
            bail!("fan channel {:?} is not <device>/<channel>", f.channel);
        }
        if !seen.insert(&f.channel) {
            bail!("fan {} is listed twice", f.channel);
        }
        if let Some(c) = &f.curve {
            if !names.contains(c.as_str()) {
                bail!("fan {} uses curve {c:?}, which does not exist", f.channel);
            }
        }
        if f.min > f.max || f.max > 100 {
            bail!("fan {}: limits must be 0 ≤ min ≤ max ≤ 100", f.channel);
        }
        if !(f.step_up >= 0.0 && f.step_down >= 0.0 && f.step_up.is_finite() && f.step_down.is_finite()) {
            bail!("fan {}: steps are % per second, 0 or more", f.channel);
        }
    }
    let mut seen = HashSet::new();
    for l in lights {
        if split_channel(&l.channel).is_none() {
            bail!("light channel {:?} is not <device>/<channel>", l.channel);
        }
        if !seen.insert(&l.channel) {
            bail!("light {} is listed twice", l.channel);
        }
        if l.managed() {
            l.to_lighting().with_context(|| format!("light {}", l.channel))?;
        } else if l.name.is_empty() {
            bail!("light {} has neither a mode nor a name", l.channel);
        }
    }
    let mut seen = HashSet::new();
    for l in lcds {
        if !seen.insert(&l.device) {
            bail!("lcd {} is listed twice", l.device);
        }
        if l.orientation % 90 != 0 || l.orientation >= 360 {
            bail!("lcd {}: orientation must be 0, 90, 180 or 270", l.device);
        }
        if l.brightness > 100 {
            bail!("lcd {}: brightness is 0 to 100", l.device);
        }
        if !(1..=30).contains(&l.fps) {
            bail!("lcd {}: fps must be 1 to 30", l.device);
        }
        if l.mode == LcdMode::Web && !(l.url.starts_with("http://") || l.url.starts_with("https://")) {
            bail!("lcd {}: web mode needs an http or https url", l.device);
        }
    }
    Ok(())
}

/// Replaces `path` through a temporary file. An unchanged file is left alone, so its
/// modification time only moves when something did change.
fn write_file(path: &Path, text: &str) -> Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|old| old == text) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

/// Modification times of the files a config is read from, to notice edits.
pub fn stamp(path: &Path, profile: Option<&str>) -> Vec<Option<SystemTime>> {
    let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut out = vec![mtime(path)];
    if let Some(p) = profile.and_then(|name| profile_path(path, name).ok()) {
        out.push(mtime(&p));
    }
    out
}

/// `profiles/` next to the config file.
pub fn profiles_dir(config_path: &Path) -> PathBuf {
    config_path.parent().unwrap_or(Path::new("")).join("profiles")
}

pub fn profile_path(config_path: &Path, name: &str) -> Result<PathBuf> {
    check_profile_name(name)?;
    Ok(profiles_dir(config_path).join(format!("{name}.toml")))
}

/// A profile's name is its file name, so it must be one Windows accepts and must not
/// lead out of `profiles/`.
pub fn check_profile_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("a profile needs a name");
    }
    if name.chars().any(|c| c.is_control() || r#"<>:"/\|?*"#.contains(c)) {
        bail!(r#"{name:?} cannot be a profile name: it may not contain < > : " / \ | ? *"#);
    }
    let stem = name.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
    let device = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
    if name.trim() != name || name.ends_with('.') || name.chars().count() > 60 || device {
        bail!("{name:?} cannot be a profile name: no spaces at the ends, no dot at the end, at most 60 characters");
    }
    Ok(())
}

/// Whether two names point at the same profile file: Windows ignores case, also in
/// letters such as Ł and ł.
pub fn same_profile_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// The profiles in `profiles/`, by name.
pub fn list_profiles(config_path: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(profiles_dir(config_path)) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension()? != "toml" {
                return None;
            }
            let name = p.file_stem()?.to_str()?.to_string();
            check_profile_name(&name).is_ok().then_some(name)
        })
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

pub fn load_profile(config_path: &Path, name: &str) -> Result<Setup> {
    let p = profile_path(config_path, name)?;
    let text = std::fs::read_to_string(&p).with_context(|| format!("reading profile {name:?} from {}", p.display()))?;
    let setup: Setup = toml::from_str(&text).map_err(|e| anyhow::anyhow!("{e}")).with_context(|| format!("in {}", p.display()))?;
    setup.validate().with_context(|| format!("in {}", p.display()))?;
    Ok(setup)
}

pub fn save_profile(config_path: &Path, name: &str, setup: &Setup) -> Result<()> {
    setup.validate()?;
    write_file(&profile_path(config_path, name)?, &setup.to_toml()?)
}

/// Like `save_profile`, but fails if the profile exists.
pub fn create_profile(config_path: &Path, name: &str, setup: &Setup) -> Result<()> {
    if profile_path(config_path, name)?.exists() {
        bail!("there is already a profile called {name:?}");
    }
    save_profile(config_path, name, setup)
}

pub fn delete_profile(config_path: &Path, name: &str) -> Result<()> {
    let p = profile_path(config_path, name)?;
    std::fs::remove_file(&p).with_context(|| format!("deleting {}", p.display()))
}

/// `base`, or `base 2`, `base 3`… if that profile exists.
pub fn unused_profile_name(config_path: &Path, base: &str) -> String {
    let taken = list_profiles(config_path);
    let free = |n: &str| !taken.iter().any(|t| same_profile_name(t, n));
    if free(base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base} {i}")).find(|n| free(n)).unwrap_or_default()
}

fn check_curve(c: &CurveSpec, names: &HashSet<&str>) -> Result<()> {
    if c.name().trim().is_empty() {
        bail!("a curve needs a name");
    }
    let check_response = |r: &Response| -> Result<()> {
        for v in [r.hysteresis, r.response_up, r.response_down] {
            if !(v.is_finite() && v >= 0.0) {
                bail!("hysteresis and response times are 0 or more");
            }
        }
        Ok(())
    };
    match c {
        CurveSpec::Graph(g) => {
            if g.points.is_empty() {
                bail!("needs at least one point");
            }
            if g.points.iter().any(|p| !p.0.is_finite() || p.1 > 100) {
                bail!("points are (value, duty 0 to 100)");
            }
            check_response(&g.response())?;
        }
        CurveSpec::Fixed(f) => {
            if f.duty > 100 {
                bail!("duty is 0 to 100");
            }
        }
        CurveSpec::Mix(m) => {
            if m.curves.is_empty() {
                bail!("mixes no curves");
            }
            if let Some(missing) = m.curves.iter().find(|n| !names.contains(n.as_str())) {
                bail!("mixes {missing:?}, which does not exist");
            }
        }
        CurveSpec::Trigger(t) => {
            if t.idle_duty > 100 || t.load_duty > 100 {
                bail!("duties are 0 to 100");
            }
            if !(t.idle_at.is_finite() && t.load_at.is_finite() && t.idle_at < t.load_at) {
                bail!("idle_at must be below load_at");
            }
            check_response(&t.response())?;
        }
    }
    Ok(())
}

/// Curves in an order where every mix comes after the curves it uses.
pub fn curve_order(curves: &[CurveSpec]) -> Result<Vec<usize>> {
    let index: HashMap<&str, usize> = curves.iter().enumerate().map(|(i, c)| (c.name(), i)).collect();
    // 0 unvisited, 1 in progress, 2 done
    let mut state = vec![0u8; curves.len()];
    let mut order = Vec::with_capacity(curves.len());
    fn visit(i: usize, curves: &[CurveSpec], index: &HashMap<&str, usize>, state: &mut [u8], order: &mut Vec<usize>) -> Result<()> {
        match state[i] {
            2 => return Ok(()),
            1 => bail!("curve {:?} mixes itself, directly or through other mixes", curves[i].name()),
            _ => {}
        }
        state[i] = 1;
        if let CurveSpec::Mix(m) = &curves[i] {
            for n in &m.curves {
                if let Some(&j) = index.get(n.as_str()) {
                    visit(j, curves, index, state, order)?;
                }
            }
        }
        state[i] = 2;
        order.push(i);
        Ok(())
    }
    for i in 0..curves.len() {
        visit(i, curves, &index, &mut state, &mut order)?;
    }
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const SAMPLE: &str = r#"
interval_ms = 1000
failsafe_duty = 70

[[curve]]
kind = "graph"
name = "CPU"
sensor = "cpu.temp"
points = [[30, 25], [60, 45], [80, 100]]
hysteresis = 2

[[curve]]
kind = "graph"
name = "GPU"
sensor = "gpu.temp"
points = [[45, 25], [80, 80]]

[[curve]]
kind = "mix"
name = "Case"
curves = ["CPU", "GPU"]
function = "max"

[[curve]]
kind = "fixed"
name = "Quiet"
duty = 30

[[curve]]
kind = "trigger"
name = "Boost"
sensor = "gpu.temp"
idle_duty = 30
load_duty = 70
idle_at = 55
load_at = 65

[[fan]]
channel = "kraken-elite-v2/pump"
name = "Pump"
curve = "CPU"
min = 40

[[fan]]
channel = "control-hub/fan1"
curve = "Case"
step_down = 5

[[light]]
channel = "kraken-elite-v2/ring"
mode = "tai-chi"
colors = ["3232de", "ffffff"]

[[lcd]]
device = "kraken-elite-v2"
mode = "web"
url = "http://127.0.0.1:30640/lcd"
"#;

    #[test]
    fn parses_sample() {
        let c = Config::parse(SAMPLE).unwrap();
        assert_eq!(c.curves.len(), 5);
        assert!(c.api.enabled);
        assert_eq!(c.api.listen, crate::DEFAULT_API);
        let CurveSpec::Graph(g) = c.curve("CPU").unwrap() else { panic!() };
        assert_eq!(g.hysteresis, 2.0);
        assert_eq!(g.response_down, 10.0);
        assert_eq!(c.fans[0].max, 100);
        assert_eq!(c.lcds[0].fps, 10);
    }

    #[test]
    fn round_trips_through_toml() {
        let c = Config::parse(SAMPLE).unwrap();
        let again = Config::parse(&c.to_toml().unwrap()).unwrap();
        assert_eq!(again, c);
    }

    #[test]
    fn mixes_come_after_their_inputs() {
        let c = Config::parse(SAMPLE).unwrap();
        let order = curve_order(&c.curves).unwrap();
        let pos = |name: &str| order.iter().position(|&i| c.curves[i].name() == name).unwrap();
        assert!(pos("Case") > pos("CPU") && pos("Case") > pos("GPU"));
    }

    #[test]
    fn rejects_mix_cycles_and_dangling_names() {
        let cyc = "[[curve]]\nkind = \"mix\"\nname = \"A\"\ncurves = [\"B\"]\nfunction = \"max\"\n\
                   [[curve]]\nkind = \"mix\"\nname = \"B\"\ncurves = [\"A\"]\nfunction = \"max\"\n";
        assert!(Config::parse(cyc).unwrap_err().to_string().contains("mixes itself"));
        let dangling = "[[fan]]\nchannel = \"control-hub/fan1\"\ncurve = \"Nope\"\n";
        assert!(Config::parse(dangling).is_err());
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(Config::parse("intervall_ms = 5\n").is_err());
        assert!(Config::parse("[[fan]]\nchannel = \"a/b\"\nspeed = 3\n").is_err());
        let extra = "[[curve]]\nkind = \"fixed\"\nname = \"F\"\nduty = 5\nsensor = \"cpu.temp\"\n";
        assert!(Config::parse(extra).is_err());
    }

    #[test]
    fn points_at_old_configs() {
        let err = Config::parse("[pump]\nsource = \"cpu\"\n").unwrap_err();
        assert!(format!("{err:#}").contains("import-cam"));
    }

    #[test]
    fn games_lists_round_trip_and_refuse_empty_entries() {
        let c = Config::parse("[games]\nadd = [\"javaw.exe\", 'D:\\Emulators']\nskip = [\"wallpaper64.exe\"]\n").unwrap();
        assert_eq!(c.games.add, ["javaw.exe", r"D:\Emulators"]);
        assert_eq!(Config::parse(&c.to_toml().unwrap()).unwrap(), c);
        assert!(!Config::default().to_toml().unwrap().contains("games"));
        assert!(Config::parse("[games]\nskip = [\" \"]\n").is_err());
    }

    #[test]
    fn games_are_off_until_turned_on() {
        assert!(!Config::default().games.enabled);
        let c = Config::parse("[games]\nenabled = true\n").unwrap();
        assert!(c.games.enabled);
        assert!(c.to_toml().unwrap().contains("enabled = true"));
    }

    #[test]
    fn the_update_channel_is_stable_unless_set() {
        assert_eq!(Config::default().updates, Updates::Stable);
        assert!(!Config::default().to_toml().unwrap().contains("updates"));
        let c = Config::parse("updates = \"beta\"\n[api]\nenabled = false\n").unwrap();
        assert_eq!(c.updates, Updates::Beta);
        assert_eq!(Config::parse(&c.to_toml().unwrap()).unwrap(), c);
        assert!(Config::parse("updates = \"nightly\"\n").is_err());
    }

    #[test]
    fn a_light_can_be_only_named() {
        let c = Config::parse("[[light]]\nchannel = \"control-hub/led1\"\nname = \"Front\"\n").unwrap();
        assert!(!c.lights[0].managed());
        let text = c.to_toml().unwrap();
        assert!(text.contains("Front") && !text.contains("mode"));
        assert_eq!(Config::parse(&text).unwrap(), c);
        assert!(Config::parse("[[light]]\nchannel = \"control-hub/led1\"\n").is_err());
        let named = Config::parse("[[light]]\nchannel = \"x/ring\"\nname = \"Top\"\nmode = \"fixed\"\ncolors = [\"ffffff\"]\n").unwrap();
        assert!(named.lights[0].managed());
    }

    #[test]
    fn channels_need_a_device() {
        assert!(Config::parse("[[fan]]\nchannel = \"pump\"\n").is_err());
        assert!(Config::parse("[[fan]]\nchannel = \"kraken/pump\"\n").is_ok());
    }

    fn temp_config(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("camless-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("camless.toml")
    }

    #[test]
    fn a_profile_holds_the_setup_and_the_config_the_rest() {
        let path = temp_config("split");
        let mut c = Config::parse(SAMPLE).unwrap();
        c.profile = Some("Quiet nights".into());
        c.save(&path).unwrap();

        let main = std::fs::read_to_string(&path).unwrap();
        assert!(main.contains("profile = \"Quiet nights\"") && !main.contains("[[curve]]"));
        let profile = std::fs::read_to_string(profiles_dir(&path).join("Quiet nights.toml")).unwrap();
        assert!(profile.contains("[[curve]]") && !profile.contains("interval_ms"));
        assert_eq!(Config::load(&path).unwrap(), c);
        assert_eq!(list_profiles(&path), vec!["Quiet nights"]);
        assert_eq!(unused_profile_name(&path, "quiet NIGHTS"), "quiet NIGHTS 2");
    }

    #[test]
    fn a_profile_and_its_own_setup_do_not_mix() {
        let path = temp_config("both");
        save_profile(&path, "A", &Setup::default()).unwrap();
        std::fs::write(&path, format!("profile = \"A\"\n{SAMPLE}")).unwrap();
        assert!(format!("{:#}", Config::load(&path).unwrap_err()).contains("move them into the profile"));
        std::fs::write(&path, "profile = \"Missing\"\n").unwrap();
        assert!(format!("{:#}", Config::load(&path).unwrap_err()).contains("Missing"));
    }

    #[test]
    fn profile_names_stay_in_the_profiles_folder() {
        for bad in ["", " A", "a/b", "..\\x", "..", "x.", "CON", "com1.txt", "a:b", "tab\there"] {
            assert!(check_profile_name(bad).is_err(), "{bad:?}");
        }
        for ok in ["NZXT CAM", "Quiet 2", "gaming.v2", "Cisza 🌙", "Console"] {
            assert!(check_profile_name(ok).is_ok(), "{ok:?}");
        }
        assert!(Config::parse("profile = \"../camless\"\n").is_err());
        assert!(same_profile_name("Łagodny É", "łAGODNY é"));
        assert!(!same_profile_name("Łagodny", "Lagodny"));
    }
}
