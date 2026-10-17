//! `camless.toml`.

use crate::curve::Curve;
use crate::device::Lighting;
use crate::protocol::{Effect, Rgb, Speed};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// How often sensors are read and host-controlled fans are adjusted.
    #[serde(default = "default_interval_ms")]
    pub interval_ms: u64,
    /// Signed PawnIO `AMDFamily17.bin`, for CPU temperature and power. Defaults to
    /// `AMDFamily17.bin` next to camless.exe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pawnio_module: Option<PathBuf>,
    /// Duty for Control Hub fans whose temperature source is unavailable.
    #[serde(default = "default_failsafe_duty")]
    pub failsafe_duty: u8,
    /// Local HTTP API for KrakenDial; omit to turn it off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<ApiConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pump: Option<Cooling>,
    /// The fan header on the Kraken (the radiator fans).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radiator: Option<Cooling>,
    #[serde(default, rename = "hub_fan")]
    pub hub_fans: Vec<HubFan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ring: Option<Light>,
    /// The RGB header on the Kraken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<Light>,
    #[serde(default, rename = "hub_led")]
    pub hub_leds: Vec<HubLed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lcd: Option<Lcd>,
}

fn default_interval_ms() -> u64 {
    1000
}

fn default_failsafe_duty() -> u8 {
    70
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApiConfig {
    #[serde(default = "default_listen")]
    pub listen: String,
}

fn default_listen() -> String {
    "127.0.0.1:30641".into()
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Kraken liquid temperature; on the Kraken itself this curve runs on the device.
    Liquid,
    Cpu,
    Gpu,
    Fixed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Cooling {
    pub source: Source,
    /// [[°C, duty %], ...]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curve: Vec<(f32, u8)>,
    /// For `source = "fixed"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duty: Option<u8>,
}

impl Cooling {
    pub fn curve(&self) -> Curve {
        match (self.source, self.duty) {
            (Source::Fixed, Some(d)) => Curve::flat(d),
            _ => Curve::new(&self.curve),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HubFan {
    /// 1..=5, as printed on the hub.
    pub port: usize,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(flatten)]
    pub cooling: Cooling,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Light {
    /// "off", "fixed" or an effect: fading, spectrum-wave, covering-marquee, pulse,
    /// breathing, candle, rainbow-flow, super-rainbow, rainbow-pulse, tai-chi, water-cooler.
    pub mode: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub colors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub backward: bool,
}

impl Light {
    pub fn to_lighting(&self) -> Result<Lighting> {
        let colors = self.colors.iter().map(|c| parse_color(c)).collect::<Result<Vec<_>>>()?;
        Ok(match self.mode.to_ascii_lowercase().as_str() {
            "off" => Lighting::Off,
            "fixed" => Lighting::Fixed(*colors.first().context("fixed needs one color")?),
            other => {
                let effect = Effect::parse(other).with_context(|| format!("unknown lighting mode {other:?}"))?;
                let (min, _) = effect.color_range();
                if colors.len() < min {
                    bail!("{other} needs at least {min} color(s)");
                }
                let speed = match &self.speed {
                    Some(s) => Speed::parse(s).with_context(|| format!("unknown speed {s:?}"))?,
                    None => Speed::Normal,
                };
                Lighting::Effect { effect, colors, speed, backward: self.backward }
            }
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HubLed {
    pub port: usize,
    #[serde(flatten)]
    pub light: Light,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Lcd {
    /// "liquid": the cooler's own temperature screen, no host work at all.
    /// "api": frames come from `POST /v1/lcd/frame` (KrakenDial).
    pub mode: String,
    #[serde(default = "default_brightness")]
    pub brightness: u8,
    /// 0, 90, 180 or 270.
    #[serde(default)]
    pub orientation: u16,
}

fn default_brightness() -> u8 {
    100
}

fn check_cooling(name: &str, c: &Cooling) -> Result<()> {
    if c.source == Source::Fixed && c.duty.is_none() {
        bail!("{name}: source \"fixed\" needs duty");
    }
    if c.source != Source::Fixed && c.curve.is_empty() {
        bail!("{name}: needs a curve");
    }
    if c.duty.is_some_and(|d| d > 100) || c.curve.iter().any(|p| p.1 > 100 || !p.0.is_finite()) {
        bail!("{name}: duties are 0 to 100 %");
    }
    Ok(())
}

pub fn parse_color(s: &str) -> Result<Rgb> {
    let h = s.trim_start_matches('#');
    if h.len() != 6 {
        bail!("color {s:?} is not RRGGBB");
    }
    let v = u32::from_str_radix(h, 16).with_context(|| format!("color {s:?} is not hex"))?;
    Ok([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let cfg: Config = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<()> {
        for (i, f) in self.hub_fans.iter().enumerate() {
            if !(1..=5).contains(&f.port) {
                bail!("hub_fan port {} is not 1 to 5", f.port);
            }
            if self.hub_fans[..i].iter().any(|g| g.port == f.port) {
                bail!("hub_fan port {} is listed twice", f.port);
            }
            check_cooling(&format!("hub_fan {}", f.port), &f.cooling)?;
        }
        for l in &self.hub_leds {
            if !(1..=5).contains(&l.port) {
                bail!("hub_led port {} is not 1 to 5", l.port);
            }
            l.light.to_lighting()?;
        }
        for (name, c) in [("pump", &self.pump), ("radiator", &self.radiator)] {
            if let Some(c) = c {
                check_cooling(name, c)?;
            }
        }
        if !(0..=100).contains(&self.failsafe_duty) {
            bail!("failsafe_duty must be 0 to 100");
        }
        for l in [&self.ring, &self.external].into_iter().flatten() {
            l.to_lighting()?;
        }
        if let Some(lcd) = &self.lcd {
            if !matches!(lcd.mode.as_str(), "liquid" | "api") {
                bail!("lcd mode must be \"liquid\" or \"api\"");
            }
            if lcd.orientation % 90 != 0 || lcd.orientation >= 360 {
                bail!("lcd orientation must be 0, 90, 180 or 270");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
interval_ms = 1000
failsafe_duty = 70

[api]

[pump]
source = "cpu"
curve = [[20, 61], [65, 61], [80, 82]]

[radiator]
source = "liquid"
curve = [[30, 20], [55, 100]]

[[hub_fan]]
port = 1
name = "dol"
source = "gpu"
curve = [[45, 25], [80, 80]]

[[hub_fan]]
port = 4
source = "fixed"
duty = 100

[ring]
mode = "tai-chi"
colors = ["3232de", "ffffff"]

[[hub_led]]
port = 3
mode = "fixed"
colors = ["aa00ff"]

[lcd]
mode = "api"
"#;

    #[test]
    fn parses_sample() {
        let c: Config = toml::from_str(SAMPLE).unwrap();
        c.validate().unwrap();
        assert_eq!(c.api.unwrap().listen, "127.0.0.1:30641");
        assert_eq!(c.hub_fans[1].cooling.curve().duty_at(10.0), 100);
        assert_eq!(c.pump.unwrap().curve().duty_at(72.5), 72);
        assert!(matches!(c.ring.unwrap().to_lighting().unwrap(), Lighting::Effect { effect: Effect::TaiChi, .. }));
    }

    #[test]
    fn round_trips_through_toml() {
        let c: Config = toml::from_str(SAMPLE).unwrap();
        let again: Config = toml::from_str(&toml::to_string(&c).unwrap()).unwrap();
        again.validate().unwrap();
        assert_eq!(again.hub_fans.len(), 2);
        assert_eq!(again.hub_fans[0].name, "dol");
        assert_eq!(again.hub_fans[1].cooling.duty, Some(100));
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(toml::from_str::<Config>("[[hub_fan]]\nport = 1\nsource = \"fixed\"\nduty = 50\nspeed = 3\n").is_err());
        assert!(toml::from_str::<Config>("intervall_ms = 5\n").is_err());
    }

    #[test]
    fn rejects_bad_port() {
        let c: Config = toml::from_str("[[hub_fan]]\nport = 6\nsource = \"fixed\"\nduty = 50\n").unwrap();
        assert!(c.validate().is_err());
    }
}
