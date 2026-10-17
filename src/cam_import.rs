//! Builds a `camless.toml` from the profiles NZXT CAM is currently using.
//!
//! CAM keeps its state as JSON in `%APPDATA%\NZXT CAM\DataStorage\latest\user`.
//! The master profile picks a cooling and a lighting profile; cooling curves are in
//! `coolingController.json`, per-channel lighting in `lightingV2Store.json`.

use crate::config::{ApiConfig, Config, Cooling, HubFan, HubLed, Lcd, Light, Source};
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub fn default_dir() -> PathBuf {
    let appdata = std::env::var_os("APPDATA").unwrap_or_default();
    PathBuf::from(appdata).join(r"NZXT CAM\DataStorage\latest\user")
}

fn read(dir: &Path, name: &str) -> Result<Value> {
    let p = dir.join(name);
    let text = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
    Ok(serde_json::from_str(&text)?)
}

pub struct Imported {
    pub config: Config,
    pub notes: Vec<String>,
}

pub fn import(dir: &Path) -> Result<Imported> {
    let master = read(dir, "masterProfileController.json")?;
    let cooling = read(dir, "coolingController.json")?;
    let lighting = read(dir, "lightingV2Store.json")?;
    let lighting_v1 = read(dir, "lightingController.json").ok();
    let mut notes = Vec::new();

    let active_master = master["activeProfileId"].as_str().unwrap_or_default();
    let master_profile = master["profiles"]
        .as_array()
        .and_then(|ps| ps.iter().find(|p| p["id"] == active_master));
    let cooling_id = master_profile
        .and_then(|p| p["coolingProfileId"].as_str())
        .or(cooling["activeProfileId"].as_str())
        .unwrap_or("DEFAULT")
        .to_string();
    let lighting_id = master_profile
        .and_then(|p| p["lightingProfileId"].as_str())
        .unwrap_or("DEFAULT")
        .to_string();

    let mut cfg = Config {
        interval_ms: 1000,
        failsafe_duty: 70,
        api: Some(ApiConfig { listen: "127.0.0.1:30641".into() }),
        ..Default::default()
    };

    let nicknames = cooling["channelSettings"].as_array().cloned().unwrap_or_default();
    let nickname = |uid: &str, ch: &str| -> String {
        nicknames
            .iter()
            .find(|c| c["deviceIdentifier"]["uniqueId"] == uid && c["channelName"] == ch)
            .and_then(|c| c["nickname"].as_str())
            .unwrap_or_default()
            .to_string()
    };

    let profile = cooling["profiles"]
        .as_array()
        .and_then(|ps| ps.iter().find(|p| p["id"] == cooling_id.as_str()))
        .with_context(|| format!("cooling profile {cooling_id} not found"))?;
    for ch in profile["channelSettings"].as_array().into_iter().flatten() {
        let uid = ch["deviceIdentifier"]["uniqueId"].as_str().unwrap_or_default();
        let name = ch["channelName"].as_str().unwrap_or_default();
        let Some(c) = cooling_from_cam(&ch["mode"]) else {
            notes.push(format!("{uid} {name}: mode {} not understood, skipped", ch["mode"]["type"]));
            continue;
        };
        if uid.starts_with("KrakenEliteV2") {
            match name {
                "cooler_pump" => cfg.pump = Some(c),
                "cooler_fan_1" => cfg.radiator = Some(c),
                _ => notes.push(format!("Kraken channel {name} skipped")),
            }
        } else if uid.starts_with("ControlHub") {
            if let Some(port) = name.strip_prefix("fan_cooling_").and_then(|n| n.parse().ok()) {
                cfg.hub_fans.push(HubFan { port, name: nickname(uid, name), cooling: c });
            }
        }
    }
    cfg.hub_fans.sort_by_key(|f| f.port);

    if let Some(map) = lighting["ledChannelMap"].as_object() {
        let prefix = format!("{lighting_id}#");
        let mut entries: Vec<(&String, &Value)> = map.iter().filter(|(k, _)| k.starts_with(&prefix)).collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (key, v) in entries {
            let mut parts = key.split('#').skip(1);
            let (uid, channel) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
            let channel: usize = channel.parse().unwrap_or(0);
            let Some(light) = light_from_cam(&v["selectedMode"], v["brightness"].as_f64().unwrap_or(1.0)) else {
                notes.push(format!("{uid} channel {channel}: effect {} not supported, skipped", v["selectedMode"]["effect"]));
                continue;
            };
            if uid.starts_with("KrakenEliteV2") {
                // CAM's channel 0 is the pump ring and 1 the RGB header, as on the device.
                match channel {
                    0 => cfg.ring = Some(light),
                    1 => cfg.external = Some(light),
                    _ => {}
                }
            } else if uid.starts_with("ControlHub") {
                cfg.hub_leds.push(HubLed { port: channel + 1, light });
            }
        }
    }

    let lcd_mode = lighting_v1
        .as_ref()
        .and_then(|l| {
            let id = l["activeProfileId"].as_str()?;
            let p = l["profiles"].as_array()?.iter().find(|p| p["id"] == id)?;
            p["channelSettings"].as_array()?.iter().find(|c| {
                c["deviceUniqueId"].as_str().is_some_and(|u| u.starts_with("KrakenEliteV2"))
                    && c["channelName"] == "LCD Display"
            })
        })
        .map(|c| {
            let brightness = (c["brightness"].as_f64().unwrap_or(1.0) * 100.0).round().clamp(0.0, 100.0) as u8;
            let url = ["activeUrl", "customUrl"]
                .iter()
                .filter_map(|k| c["mode"][k].as_str())
                .find(|u| !u.is_empty())
                .unwrap_or("?")
                .to_string();
            (c["mode"]["name"].as_str().unwrap_or_default().to_string(), url, brightness)
        });
    cfg.lcd = Some(match lcd_mode {
        Some((name, url, brightness)) if name == "WebIntegration" => {
            notes.push(format!("LCD: CAM shows the web page {url}; camless takes its frames on POST /v1/lcd/frame"));
            Lcd { mode: "api".into(), brightness, orientation: 0 }
        }
        other => {
            if let Some((name, _, _)) = &other {
                notes.push(format!("LCD: CAM mode {name} has no camless equivalent yet, using the liquid screen"));
            }
            Lcd { mode: "liquid".into(), brightness: other.map_or(100, |o| o.2), orientation: 0 }
        }
    });

    cfg.validate()?;
    Ok(Imported { config: cfg, notes })
}

fn cooling_from_cam(mode: &Value) -> Option<Cooling> {
    let source = match mode["temperatureOption"].as_str() {
        Some("CPU") => Source::Cpu,
        Some("GPU") => Source::Gpu,
        Some("Liquid") => Source::Liquid,
        _ => Source::Cpu,
    };
    match mode["type"].as_str()? {
        "Fixed" => Some(Cooling {
            source: Source::Fixed,
            curve: vec![],
            duty: Some(mode["fixedFanPercentage"].as_u64()?.min(100) as u8),
        }),
        _ => {
            let curve: Vec<(f32, u8)> = mode["customThresholds"]
                .as_array()?
                .iter()
                .filter_map(|p| Some((p["temperature"].as_f64()? as f32, p["fanPercentage"].as_u64()?.min(100) as u8)))
                .collect();
            if curve.is_empty() {
                return None;
            }
            // A curve that is flat everywhere is a fixed duty in disguise.
            if curve.iter().all(|p| p.1 == curve[0].1) {
                return Some(Cooling { source: Source::Fixed, curve: vec![], duty: Some(curve[0].1) });
            }
            Some(Cooling { source, curve: simplify(&curve), duty: None })
        }
    }
}

/// Drops points that lie on the line between their neighbours.
fn simplify(curve: &[(f32, u8)]) -> Vec<(f32, u8)> {
    let mut out = vec![curve[0]];
    for i in 1..curve.len() {
        let last = i + 1 == curve.len();
        if last || {
            let (a, b, c) = (out[out.len() - 1], curve[i], curve[i + 1]);
            let expect = a.1 as f32 + (b.0 - a.0) / (c.0 - a.0) * (c.1 as f32 - a.1 as f32);
            (expect - b.1 as f32).abs() > 0.5
        } {
            out.push(curve[i]);
        }
    }
    out
}

fn light_from_cam(mode: &Value, brightness: f64) -> Option<Light> {
    let scale = |c: &Value| -> Option<String> {
        let ch = |k: &str| ((c[k].as_f64().unwrap_or(0.0) * brightness).round() as u32).min(255);
        Some(format!("{:02x}{:02x}{:02x}", ch("r"), ch("g"), ch("b")))
    };
    let effect = mode["effect"].as_str()?;
    if effect == "Fixed" {
        let c = if mode["color"].is_object() { &mode["color"] } else { &mode["colors"][0] };
        return Some(Light { mode: "fixed".into(), colors: vec![scale(c)?], speed: None, backward: false });
    }
    if effect == "Off" {
        return Some(Light { mode: "off".into(), colors: vec![], speed: None, backward: false });
    }
    // CAM names effects in PascalCase: TaiChi -> tai-chi.
    let mut name = String::new();
    for (i, ch) in effect.chars().enumerate() {
        if ch.is_ascii_uppercase() && i > 0 {
            name.push('-');
        }
        name.push(ch.to_ascii_lowercase());
    }
    crate::protocol::Effect::parse(&name)?;
    let colors = mode["colors"].as_array().map(|cs| cs.iter().filter_map(scale).collect()).unwrap_or_default();
    Some(Light {
        mode: name,
        colors,
        speed: mode["speed"].as_str().map(|s| s.to_ascii_lowercase()),
        backward: mode["direction"].as_str().is_some_and(|d| d.starts_with("Backward")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn simplify_keeps_corners_only() {
        let c = [(20.0, 25), (25.0, 25), (30.0, 25), (35.0, 30), (40.0, 35), (45.0, 35)];
        assert_eq!(simplify(&c), vec![(20.0, 25), (30.0, 25), (40.0, 35), (45.0, 35)]);
    }

    #[test]
    fn flat_custom_curve_becomes_fixed() {
        let m = json!({"type": "Custom", "temperatureOption": "CPU",
            "customThresholds": [{"temperature": 20, "fanPercentage": 100}, {"temperature": 100, "fanPercentage": 100}]});
        let c = cooling_from_cam(&m).unwrap();
        assert_eq!((c.source, c.duty), (Source::Fixed, Some(100)));
    }

    #[test]
    fn tai_chi_with_colors() {
        let m = json!({"colors": [{"r": 50, "g": 50, "b": 222}, {"r": 255, "g": 255, "b": 255}],
            "speed": "Normal", "direction": "Forwards", "effect": "TaiChi"});
        let l = light_from_cam(&m, 1.0).unwrap();
        assert_eq!(l.mode, "tai-chi");
        assert_eq!(l.colors, vec!["3232de", "ffffff"]);
        assert_eq!(l.speed.as_deref(), Some("normal"));
    }

    #[test]
    fn fixed_uses_single_color_and_brightness() {
        let m = json!({"color": {"r": 170, "g": 0, "b": 255}, "colors": [{"r": 255, "g": 255, "b": 255}],
            "isSingleColor": true, "effect": "Fixed"});
        assert_eq!(light_from_cam(&m, 0.5).unwrap().colors, vec!["550080"]);
    }
}
