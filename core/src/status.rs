//! `GET /v1/status`: what the daemon knows, in the shape the GUI reads.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Status {
    pub version: String,
    pub config_path: String,
    /// Why the config file on disk was not applied; the previous one still runs.
    pub config_error: Option<String>,
    pub devices: Vec<Device>,
    /// NZXT devices camless has no driver for yet.
    pub unsupported: Vec<Unsupported>,
    pub sensors: Vec<Sensor>,
    pub curves: Vec<CurveValue>,
    pub fans: Vec<FanValue>,
    /// How the service itself runs; services before 0.3 leave it out.
    #[serde(default)]
    pub service: Option<ServiceInfo>,
    /// The game being played, while a session is open.
    #[serde(default)]
    pub game: Option<Game>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ServiceInfo {
    pub pid: u32,
    /// Has administrator rights, which CPU temperature and power need.
    pub elevated: bool,
    /// Why there is no CPU temperature, when there is none.
    pub cpu_sensor_error: Option<String>,
    /// Why games have no frame rates, when they have none.
    #[serde(default)]
    pub fps_error: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Game {
    pub name: String,
    /// The program's file name.
    pub exe: String,
    /// The Steam app id, for a game in a Steam library.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steam: Option<u32>,
    /// When the session started, in Unix milliseconds.
    pub start: i64,
    /// Seconds in front so far.
    pub played: u32,
    /// In front now. A game in the background keeps its session for 3 minutes.
    pub in_front: bool,
    /// Frames in one second, measured 2 s ago; none in the background or without a
    /// capture.
    pub fps: Option<f64>,
    /// The session's average frame rate and 1 % low so far.
    pub avg_fps: Option<f64>,
    pub low1: Option<f64>,
}

/// One device and what it can do. Drivers fill in everything but the readings.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Device {
    /// Stable id used in the config: the model slug, `#2` for a second one.
    pub id: String,
    pub name: String,
    pub firmware: Option<String>,
    /// Tested on real hardware by someone. Unverified drivers follow other projects'
    /// documentation or captures of CAM.
    pub verified: bool,
    pub connected: bool,
    pub error: Option<String>,
    pub fans: Vec<FanChannel>,
    pub lights: Vec<LightChannel>,
    pub lcd: Option<LcdInfo>,
    /// The web page camless renders on the LCD, and how that goes.
    #[serde(default)]
    pub web: Option<WebState>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct WebState {
    pub url: String,
    pub browser: Option<String>,
    /// Frames per second reaching the screen; 0 while the page does not change.
    pub fps: f32,
    /// Why the page is not shown; the screen is on its own display meanwhile.
    pub error: Option<String>,
    /// Frames of the background the page handed over, which camless plays under it;
    /// 0 while the page draws its own.
    #[serde(default)]
    pub background_frames: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct FanChannel {
    pub id: String,
    pub name: String,
    pub pump: bool,
    /// The device does not go below this.
    pub min_duty: u8,
    /// The device can run a curve on its own sensors (or a fixed duty) without camless.
    pub device_curves: bool,
    /// Something is plugged in, when the device can tell.
    pub connected: Option<bool>,
    pub rpm: Option<u16>,
    pub duty: Option<u8>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct LightChannel {
    pub id: String,
    pub name: String,
    pub leds: u16,
    /// What the device reports on this channel, e.g. accessory ids.
    pub accessories: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct LcdInfo {
    pub width: u16,
    pub height: u16,
    pub round: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    #[default]
    Celsius,
    Percent,
    Watt,
}

impl Unit {
    pub fn symbol(self) -> &'static str {
        match self {
            Unit::Celsius => "°C",
            Unit::Percent => "%",
            Unit::Watt => "W",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Sensor {
    /// What curves name, e.g. `cpu.temp` or `kraken-elite-v2.liquid`.
    pub id: String,
    pub name: String,
    pub unit: Unit,
    pub value: Option<f32>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CurveValue {
    pub name: String,
    /// The sensor value the curve acted on, after smoothing and hysteresis.
    pub input: Option<f32>,
    pub duty: Option<f32>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct FanValue {
    pub channel: String,
    /// What camless asks for; the device may report a little less.
    pub duty: Option<u8>,
    /// The curve runs on the device itself.
    pub on_device: bool,
    /// The curve has no reading, so the fan is at the failsafe duty.
    pub failsafe: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Unsupported {
    pub vid: u16,
    pub pid: u16,
    pub product: String,
}
