//! Builds a `camless.toml` from the profiles NZXT CAM is currently using.
//!
//! CAM keeps its state as JSON in `%APPDATA%\NZXT CAM\DataStorage\latest\user`.
//! The master profile picks a cooling and a lighting profile; cooling curves are in
//! `coolingController.json`, per-channel lighting in `lightingV2Store.json`, the LCD
//! in `lightingController.json`.
//!
//! CAM names devices by a unique id that starts with its model (`KrakenEliteV2…`,
//! `ControlHub…`); `DEVICES` maps those to camless device ids and channel names.

use anyhow::{Context, Result};
use crate::config::{Config, CurveSpec, Fan, Fixed, Graph, Lcd, LcdMode};
use crate::lighting::{Effect, Light, Speed};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// CAM's unique id prefix, the camless device id, and channel maps: CAM cooling
/// channel → camless fan, CAM LED channel index → camless light.
struct CamDevice {
    prefix: &'static str,
    id: &'static str,
    fan: fn(&str) -> Option<String>,
    light: fn(usize) -> Option<String>,
    lcd: bool,
}

const DEVICES: &[CamDevice] = &[
    CamDevice {
        prefix: "KrakenEliteV2",
        id: "kraken-elite-v2",
        fan: |ch| match ch {
            "cooler_pump" => Some("pump".into()),
            "cooler_fan_1" => Some("fan".into()),
            _ => None,
        },
        // CAM's channel 0 is the pump ring and 1 the RGB header, as on the device.
        light: |ch| ["ring", "rgb"].get(ch).map(|s| s.to_string()),
        lcd: true,
    },
    CamDevice {
        prefix: "ControlHub",
        id: "control-hub",
        fan: |ch| {
            let n: usize = ch.strip_prefix("fan_cooling_")?.parse().ok()?;
            (1..=5).contains(&n).then(|| format!("fan{n}"))
        },
        light: |ch| (ch < 5).then(|| format!("led{}", ch + 1)),
        lcd: false,
    },
];

fn device(uid: &str) -> Option<&'static CamDevice> {
    DEVICES.iter().find(|d| uid.starts_with(d.prefix))
}

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
    let master_profile = master["profiles"].as_array().and_then(|ps| ps.iter().find(|p| p["id"] == active_master));
    let cooling_id = master_profile
        .and_then(|p| p["coolingProfileId"].as_str())
        .or(cooling["activeProfileId"].as_str())
        .unwrap_or("DEFAULT")
        .to_string();
    let lighting_id = master_profile.and_then(|p| p["lightingProfileId"].as_str()).unwrap_or("DEFAULT").to_string();

    let mut cfg = Config::default();

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
        let Some(dev) = device(uid) else {
            notes.push(format!("{uid}: camless has no driver for this device, skipped"));
            continue;
        };
        let Some(fan) = (dev.fan)(name) else {
            notes.push(format!("{uid} {name}: unknown channel, skipped"));
            continue;
        };
        let Some(curve) = curve_from_cam(&ch["mode"]) else {
            notes.push(format!("{uid} {name}: mode {} not understood, skipped", ch["mode"]["type"]));
            continue;
        };
        let curve = add_curve(&mut cfg.curves, curve);
        let mut f = Fan::new(&format!("{}/{fan}", dev.id));
        f.name = nickname(uid, name);
        f.curve = Some(curve);
        cfg.fans.push(f);
    }
    cfg.fans.sort_by(|a, b| a.channel.cmp(&b.channel));

    if let Some(map) = lighting["ledChannelMap"].as_object() {
        let prefix = format!("{lighting_id}#");
        let mut entries: Vec<(&String, &Value)> = map.iter().filter(|(k, _)| k.starts_with(&prefix)).collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (key, v) in entries {
            let mut parts = key.split('#').skip(1);
            let (uid, channel) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
            let Some(light) = device(uid).zip(channel.parse().ok()).and_then(|(d, c)| Some(format!("{}/{}", d.id, (d.light)(c)?)))
            else {
                notes.push(format!("{uid} LED channel {channel}: no camless light for it, skipped"));
                continue;
            };
            match light_from_cam(&light, &v["selectedMode"], v["brightness"].as_f64().unwrap_or(1.0)) {
                // One light CAM stored with too few colors should not lose the rest of the import.
                Some(l) => match l.to_lighting() {
                    Ok(_) => cfg.lights.push(l),
                    Err(e) => notes.push(format!("{light}: {e:#}, skipped")),
                },
                None => notes.push(format!("{light}: effect {} not supported, skipped", v["selectedMode"]["effect"])),
            }
        }
    }

    let lcd = lighting_v1.as_ref().and_then(|l| {
        let id = l["activeProfileId"].as_str()?;
        let p = l["profiles"].as_array()?.iter().find(|p| p["id"] == id)?;
        p["channelSettings"].as_array()?.iter().find_map(|c| {
            let dev = device(c["deviceUniqueId"].as_str()?).filter(|d| d.lcd)?;
            (c["channelName"] == "LCD Display").then_some((dev, c))
        })
    });
    if let Some((dev, c)) = lcd {
        let brightness = (c["brightness"].as_f64().unwrap_or(1.0) * 100.0).round().clamp(0.0, 100.0) as u8;
        let url = ["activeUrl", "customUrl"]
            .iter()
            .filter_map(|k| c["mode"][k].as_str())
            .find(|u| u.starts_with("http://") || u.starts_with("https://"))
            .unwrap_or_default()
            .to_string();
        let name = c["mode"]["name"].as_str().unwrap_or_default();
        let mode = match name {
            "WebIntegration" if !url.is_empty() => {
                notes.push(format!("{}: CAM shows the web page {url}", dev.id));
                LcdMode::Web
            }
            _ => {
                notes.push(format!("{}: CAM mode {name:?} has no camless equivalent yet, using the liquid screen", dev.id));
                LcdMode::Liquid
            }
        };
        let url = if mode == LcdMode::Web { url } else { String::new() };
        cfg.lcds.push(Lcd { device: dev.id.into(), mode, brightness, orientation: 0, url, fps: 10 });
    }

    cfg.validate()?;
    let mut unique = Vec::new();
    for n in notes {
        if !unique.contains(&n) {
            unique.push(n);
        }
    }
    Ok(Imported { config: cfg, notes: unique })
}

/// Adds `curve` unless an identical one is there, and returns the name it goes by.
fn add_curve(curves: &mut Vec<CurveSpec>, mut curve: CurveSpec) -> String {
    let base = curve.name().to_string();
    for c in curves.iter() {
        let mut named = curve.clone();
        named.set_name(c.name().to_string());
        if named == *c {
            return c.name().to_string();
        }
    }
    let mut name = base.clone();
    let mut n = 2;
    while curves.iter().any(|c| c.name() == name) {
        name = format!("{base} {n}");
        n += 1;
    }
    curve.set_name(name.clone());
    curves.push(curve);
    name
}

fn curve_from_cam(mode: &Value) -> Option<CurveSpec> {
    let fixed = |duty: u8| CurveSpec::Fixed(Fixed { name: format!("Fixed {duty} %"), duty });
    if mode["type"].as_str()? == "Fixed" {
        return Some(fixed(mode["fixedFanPercentage"].as_u64()?.min(100) as u8));
    }
    let (name, sensor) = match mode["temperatureOption"].as_str() {
        Some("GPU") => ("GPU", "gpu.temp"),
        Some("Liquid") => ("Liquid", "kraken-elite-v2.liquid"),
        _ => ("CPU", "cpu.temp"),
    };
    let points: Vec<(f32, u8)> = mode["customThresholds"]
        .as_array()?
        .iter()
        .filter_map(|p| Some((p["temperature"].as_f64()? as f32, p["fanPercentage"].as_u64()?.min(100) as u8)))
        .collect();
    if points.is_empty() {
        return None;
    }
    // A curve that is flat everywhere is a fixed duty in disguise.
    if points.iter().all(|p| p.1 == points[0].1) {
        return Some(fixed(points[0].1));
    }
    Some(CurveSpec::Graph(Graph {
        name: name.into(),
        sensor: sensor.into(),
        points: simplify(&points),
        hysteresis: 0.0,
        response_up: 2.0,
        response_down: 10.0,
    }))
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

fn light_from_cam(channel: &str, mode: &Value, brightness: f64) -> Option<Light> {
    let scale = |c: &Value| -> Option<String> {
        let ch = |k: &str| ((c[k].as_f64().unwrap_or(0.0) * brightness).round() as u32).min(255);
        Some(format!("{:02x}{:02x}{:02x}", ch("r"), ch("g"), ch("b")))
    };
    let light = |mode: &str, colors: Vec<String>| Light {
        channel: channel.into(),
        mode: mode.into(),
        colors,
        speed: Speed::default(),
        backward: false,
    };
    let effect = mode["effect"].as_str()?;
    if effect == "Fixed" {
        let c = if mode["color"].is_object() { &mode["color"] } else { &mode["colors"][0] };
        return Some(light("fixed", vec![scale(c)?]));
    }
    if effect == "Off" {
        return Some(light("off", vec![]));
    }
    // CAM names effects in PascalCase: TaiChi is tai-chi.
    let e = Effect::parse(effect)?;
    let colors = mode["colors"].as_array().map(|cs| cs.iter().filter_map(scale).collect()).unwrap_or_default();
    let speed = mode["speed"]
        .as_str()
        .and_then(|s| Speed::ALL.into_iter().find(|x| format!("{x:?}").eq_ignore_ascii_case(s)))
        .unwrap_or_default();
    Some(Light {
        speed,
        backward: mode["direction"].as_str().is_some_and(|d| d.starts_with("Backward")),
        ..light(e.name(), colors)
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
        assert_eq!(curve_from_cam(&m), Some(CurveSpec::Fixed(Fixed { name: "Fixed 100 %".into(), duty: 100 })));
    }

    #[test]
    fn identical_curves_are_shared_and_different_ones_numbered() {
        let graph = |p: u8| {
            curve_from_cam(&json!({"type": "Custom", "temperatureOption": "CPU",
                "customThresholds": [{"temperature": 30, "fanPercentage": 20}, {"temperature": 80, "fanPercentage": p}]}))
            .unwrap()
        };
        let mut curves = Vec::new();
        assert_eq!(add_curve(&mut curves, graph(100)), "CPU");
        assert_eq!(add_curve(&mut curves, graph(100)), "CPU");
        assert_eq!(add_curve(&mut curves, graph(90)), "CPU 2");
        assert_eq!(curves.len(), 2);
    }

    #[test]
    fn channels_map_to_camless_ids() {
        let k = device("KrakenEliteV2_ABC").unwrap();
        assert_eq!(((k.fan)("cooler_pump"), (k.light)(1)), (Some("pump".into()), Some("rgb".into())));
        let h = device("ControlHub_XYZ").unwrap();
        assert_eq!(((h.fan)("fan_cooling_3"), (h.light)(4)), (Some("fan3".into()), Some("led5".into())));
        assert_eq!((h.fan)("fan_cooling_6"), None);
    }

    #[test]
    fn tai_chi_with_colors() {
        let m = json!({"colors": [{"r": 50, "g": 50, "b": 222}, {"r": 255, "g": 255, "b": 255}],
            "speed": "Faster", "direction": "Forwards", "effect": "TaiChi"});
        let l = light_from_cam("kraken-elite-v2/ring", &m, 1.0).unwrap();
        assert_eq!(l.mode, "tai-chi");
        assert_eq!(l.colors, vec!["3232de", "ffffff"]);
        assert_eq!(l.speed, Speed::Faster);
        assert!(l.to_lighting().is_ok());
    }

    #[test]
    fn fixed_uses_single_color_and_brightness() {
        let m = json!({"color": {"r": 170, "g": 0, "b": 255}, "colors": [{"r": 255, "g": 255, "b": 255}],
            "isSingleColor": true, "effect": "Fixed"});
        assert_eq!(light_from_cam("control-hub/led1", &m, 0.5).unwrap().colors, vec!["550080"]);
    }
}
