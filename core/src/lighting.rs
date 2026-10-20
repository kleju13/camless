//! Lighting as the config and the GUI describe it. Device drivers turn it into packets.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub type Rgb = [u8; 3];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Speed {
    Slowest,
    Slower,
    #[default]
    Normal,
    Faster,
    Fastest,
}

impl Speed {
    pub const ALL: [Speed; 5] = [Speed::Slowest, Speed::Slower, Speed::Normal, Speed::Faster, Speed::Fastest];

    pub fn index(self) -> usize {
        self as usize
    }
}

/// Animated effects in the NZXT Hue 2 family, which the Kraken and the Control Hub share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Fading,
    SpectrumWave,
    CoveringMarquee,
    Pulse,
    Breathing,
    Candle,
    RainbowFlow,
    SuperRainbow,
    RainbowPulse,
    TaiChi,
    WaterCooler,
}

impl Effect {
    pub const ALL: [Effect; 11] = [
        Effect::Fading,
        Effect::SpectrumWave,
        Effect::CoveringMarquee,
        Effect::Pulse,
        Effect::Breathing,
        Effect::Candle,
        Effect::RainbowFlow,
        Effect::SuperRainbow,
        Effect::RainbowPulse,
        Effect::TaiChi,
        Effect::WaterCooler,
    ];

    /// The config name.
    pub fn name(self) -> &'static str {
        match self {
            Effect::Fading => "fading",
            Effect::SpectrumWave => "spectrum-wave",
            Effect::CoveringMarquee => "covering-marquee",
            Effect::Pulse => "pulse",
            Effect::Breathing => "breathing",
            Effect::Candle => "candle",
            Effect::RainbowFlow => "rainbow-flow",
            Effect::SuperRainbow => "super-rainbow",
            Effect::RainbowPulse => "rainbow-pulse",
            Effect::TaiChi => "tai-chi",
            Effect::WaterCooler => "water-cooler",
        }
    }

    /// The name CAM shows.
    pub fn label(self) -> &'static str {
        match self {
            Effect::Fading => "Fading",
            Effect::SpectrumWave => "Spectrum Wave",
            Effect::CoveringMarquee => "Covering Marquee",
            Effect::Pulse => "Pulse",
            Effect::Breathing => "Breathing",
            Effect::Candle => "Candle",
            Effect::RainbowFlow => "Rainbow Flow",
            Effect::SuperRainbow => "Super Rainbow",
            Effect::RainbowPulse => "Rainbow Pulse",
            Effect::TaiChi => "Tai Chi",
            Effect::WaterCooler => "Water Cooler",
        }
    }

    /// Accepts the config name and CAM's PascalCase name alike.
    pub fn parse(s: &str) -> Option<Effect> {
        let key = |s: &str| s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        let want = key(s);
        Effect::ALL.into_iter().find(|e| key(e.name()) == want)
    }

    /// How many colors the effect takes, from liquidctl's `_COLOR_MODES`.
    pub fn color_range(self) -> (usize, usize) {
        match self {
            Effect::SpectrumWave | Effect::RainbowFlow | Effect::SuperRainbow | Effect::RainbowPulse => (0, 0),
            Effect::Candle => (1, 1),
            Effect::TaiChi => (1, 2),
            Effect::WaterCooler => (2, 2),
            _ => (1, 8),
        }
    }

    pub fn has_direction(self) -> bool {
        matches!(
            self,
            Effect::SpectrumWave | Effect::CoveringMarquee | Effect::RainbowFlow | Effect::SuperRainbow | Effect::TaiChi
        )
    }

    pub fn has_speed(self) -> bool {
        self != Effect::Candle
    }
}

/// One lighting channel in the config.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Light {
    /// `<device>/<light>`, e.g. `kraken-elite-v2/ring` or `control-hub/led1`.
    pub channel: String,
    /// What camless's window calls the channel; nothing else reads it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// `off`, `fixed`, or an effect name such as `tai-chi`. Left out, camless leaves
    /// the channel as it is and the entry only names it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mode: String,
    /// RRGGBB.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub colors: Vec<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub speed: Speed,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub backward: bool,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// A light setting checked and ready for a driver.
#[derive(Clone, Debug, PartialEq)]
pub enum Lighting {
    Off,
    Fixed(Rgb),
    Effect { effect: Effect, colors: Vec<Rgb>, speed: Speed, backward: bool },
}

impl Light {
    /// Whether camless sets this channel, rather than only naming it.
    pub fn managed(&self) -> bool {
        !self.mode.is_empty()
    }

    pub fn to_lighting(&self) -> Result<Lighting> {
        let colors = self.colors.iter().map(|c| parse_color(c)).collect::<Result<Vec<_>>>()?;
        Ok(match self.mode.as_str() {
            "off" => Lighting::Off,
            "fixed" => Lighting::Fixed(*colors.first().context("fixed needs one color")?),
            other => {
                let effect = Effect::parse(other).with_context(|| format!("unknown lighting mode {other:?}"))?;
                let (min, max) = effect.color_range();
                if colors.len() < min {
                    bail!("{} needs at least {min} color(s)", effect.name());
                }
                let colors = colors.into_iter().take(max).collect();
                Lighting::Effect { effect, colors, speed: self.speed, backward: self.backward }
            }
        })
    }
}

pub fn parse_color(s: &str) -> Result<Rgb> {
    let h = s.trim_start_matches('#');
    if h.len() != 6 {
        bail!("color {s:?} is not RRGGBB");
    }
    let v = u32::from_str_radix(h, 16).with_context(|| format!("color {s:?} is not hex"))?;
    Ok([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

pub fn format_color(c: Rgb) -> String {
    format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_and_cam_names() {
        assert_eq!(Effect::parse("tai-chi"), Some(Effect::TaiChi));
        assert_eq!(Effect::parse("TaiChi"), Some(Effect::TaiChi));
        assert_eq!(Effect::parse("SpectrumWave"), Some(Effect::SpectrumWave));
        assert_eq!(Effect::parse("strobe"), None);
    }

    #[test]
    fn effect_colors_are_checked() {
        let l = Light { channel: "x/ring".into(), mode: "water-cooler".into(), colors: vec!["ff0000".into()], ..Default::default() };
        assert!(l.to_lighting().is_err());
        let l = Light { colors: vec!["ff0000".into(), "00ff00".into(), "0000ff".into()], ..l };
        let Lighting::Effect { colors, .. } = l.to_lighting().unwrap() else { panic!() };
        assert_eq!(colors.len(), 2);
    }

    #[test]
    fn colors_round_trip() {
        assert_eq!(parse_color("#3232de").unwrap(), [0x32, 0x32, 0xde]);
        assert_eq!(format_color([0x32, 0x32, 0xde]), "3232de");
        assert!(parse_color("12345").is_err());
    }
}
