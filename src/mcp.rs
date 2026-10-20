//! `camless mcp`: a Model Context Protocol server on stdin and stdout, so an AI
//! assistant can read the sensors, their history and the game sessions, and set fan
//! curves. It asks the running service through the local API and writes curves to
//! the config the service runs, which the service applies within an interval.
//!
//! A curve set here must speed the fans up as it gets hot: temperatures rise from
//! point to point, the duty never falls, and the hottest point, at 90 °C or below,
//! is at `failsafe_duty` or more. Smoothing stays short enough to follow a rise. The
//! service's own safeguards still apply on top. Nothing else can be changed: no
//! fan limits, no lighting, no LCD, no other setting.

use crate::api;
use crate::history::Range;
use anyhow::{bail, Context, Result};
use camless_core::config::{Config, CurveSpec, Fan, Graph, Point, Response};
use camless_core::status::{Status, Unit};
use serde_json::{json, Map, Value};
use std::io::{BufRead, Write};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Protocol versions this server speaks, oldest first.
const VERSIONS: [&str; 4] = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];
/// The hottest a curve's points may be, in °C.
const TOP_C: f32 = 90.0;
const MAX_POINTS: usize = 16;
/// The most smoothing a curve set here may have, so it still follows a rise.
const MAX_RESPONSE_UP: f64 = 10.0;
const MAX_RESPONSE_DOWN: f64 = 60.0;
const MAX_HYSTERESIS: f64 = 5.0;
/// Points each history series is cut down to.
const HISTORY_POINTS: usize = 30;

const INSTRUCTIONS: &str = "camless runs the NZXT coolers and fans on this PC. \
Call get_cooling_state before changing anything, and tell the user what you changed. \
A curve set here must rise with temperature and reach failsafe_duty by 90 °C at the latest.";

/// Serves one client until stdin closes.
pub fn serve(config: PathBuf) -> Result<()> {
    let server = Server { config };
    let mut out = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        if let Some(reply) = server.message(&line?) {
            writeln!(out, "{reply}")?;
            out.flush()?;
        }
    }
    Ok(())
}

struct Server {
    /// The config to find the service's address in.
    config: PathBuf,
}

impl Server {
    /// The reply to one line from the client, if it needs one.
    fn message(&self, line: &str) -> Option<String> {
        if line.trim().is_empty() {
            return None;
        }
        let reply = match serde_json::from_str::<Value>(line) {
            Err(e) => Some(error(Value::Null, -32700, &format!("not JSON: {e}"))),
            Ok(Value::Array(batch)) => {
                let replies: Vec<Value> = batch.iter().filter_map(|m| self.request(m)).collect();
                (!replies.is_empty()).then_some(Value::Array(replies))
            }
            Ok(m) => self.request(&m),
        };
        reply.map(|r| r.to_string())
    }

    fn request(&self, m: &Value) -> Option<Value> {
        // Notifications, such as notifications/initialized, get no reply.
        let id = m.get("id")?.clone();
        // Nor do answers: this server asks the client nothing.
        if m.get("result").is_some() || m.get("error").is_some() {
            return None;
        }
        let Some(method) = m.get("method").and_then(Value::as_str) else {
            return Some(error(id, -32600, "a request needs a method"));
        };
        let params = m.get("params").cloned().unwrap_or_else(|| json!({}));
        let result = match method {
            "initialize" => Ok(initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => self.call(&params),
            _ => Err((-32601, format!("no method {method:?}"))),
        };
        Some(match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err((code, msg)) => error(id, code, &msg),
        })
    }

    fn call(&self, params: &Value) -> std::result::Result<Value, (i64, String)> {
        let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
        let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
        let out = match name {
            "get_cooling_state" => only(&args, &[]).and_then(|_| self.cooling_state()),
            "get_sensor_history" => self.sensor_history(&args),
            "get_game_sessions" => self.game_sessions(&args),
            "set_fan_curve" => self.set_fan_curve(&args),
            "assign_curve" => self.assign_curve(&args),
            _ => return Err((-32602, format!("no tool {name:?}"))),
        };
        let (text, failed) = match out {
            Ok(v) => (serde_json::to_string_pretty(&v).unwrap_or_default(), false),
            Err(e) => (format!("{e:#}"), true),
        };
        Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": failed }))
    }

    /// The service's address and what it knows.
    fn service(&self) -> Result<(String, Status)> {
        // An invalid config still runs the service on the address it had; the default
        // one is the best guess then.
        let api = Config::load(&self.config).map(|c| c.api).unwrap_or_default();
        if !api.enabled {
            bail!("the local API is off in {}, so the service cannot be asked", self.config.display());
        }
        let body = api::get(&api.listen, "/v1/status")
            .with_context(|| format!("the camless service does not answer at {}; is it running?", api.listen))?;
        Ok((api.listen, serde_json::from_str(&body)?))
    }

    /// The config the service runs, to read or change.
    fn service_config(status: &Status) -> Result<(PathBuf, Config)> {
        let path = PathBuf::from(&status.config_path);
        if !path.is_absolute() {
            bail!("the service runs {}, a relative path, so it cannot be found from here", path.display());
        }
        let cfg = Config::load(&path).context("the config on disk does not load")?;
        Ok((path, cfg))
    }

    fn cooling_state(&self) -> Result<Value> {
        let (addr, status) = self.service()?;
        let (path, cfg) = Server::service_config(&status)?;
        Ok(cooling_state(&addr, &path, &cfg, &status))
    }

    fn sensor_history(&self, args: &Value) -> Result<Value> {
        only(args, &["ids", "minutes"])?;
        let ids = texts(args, "ids")?;
        if !(1..=8).contains(&ids.len()) {
            bail!("ids lists 1 to 8 series");
        }
        let minutes = number(args, "minutes", 10.0..=43_200.0)?.unwrap_or(60.0).round() as i64;
        let (addr, _) = self.service()?;
        let path = format!("/v1/history?span={}&ids={}", minutes * 60, ids.iter().map(|i| encode(i)).collect::<Vec<_>>().join(","));
        let range: Range = serde_json::from_str(&api::get(&addr, &path)?)?;
        Ok(summary(&range, &ids))
    }

    fn game_sessions(&self, args: &Value) -> Result<Value> {
        only(args, &["limit"])?;
        let limit = number(args, "limit", 1.0..=100.0)?.unwrap_or(10.0) as usize;
        let (addr, _) = self.service()?;
        let mut all: Value = serde_json::from_str(&api::get(&addr, "/v1/games")?)?;
        let sessions = all.get_mut("sessions").and_then(Value::as_array_mut).context("no sessions in the reply")?;
        let total = sessions.len();
        sessions.truncate(limit);
        Ok(json!({ "sessions": sessions, "total": total }))
    }

    fn set_fan_curve(&self, args: &Value) -> Result<Value> {
        let req = CurveRequest::parse(args)?;
        let (addr, status) = self.service()?;
        let (path, mut cfg) = Server::service_config(&status)?;
        let graph = set_curve(&mut cfg, &status, &req)?;
        self.save(&addr, &path, &cfg, &graph.name, json!({ "curve": graph }))
    }

    fn assign_curve(&self, args: &Value) -> Result<Value> {
        only(args, &["curve", "fans"])?;
        let curve = text(args, "curve")?.context("curve is required")?.to_string();
        let fans = texts(args, "fans")?;
        let (addr, status) = self.service()?;
        let (path, mut cfg) = Server::service_config(&status)?;
        assign(&mut cfg, &status, &curve, &fans)?;
        self.save(&addr, &path, &cfg, &curve, json!({}))
    }

    /// Writes the config, waits for the service to read it, and says how it went.
    fn save(&self, addr: &str, path: &Path, cfg: &Config, curve: &str, mut out: Value) -> Result<Value> {
        cfg.save(path)?;
        // The service looks for a changed config at the start of each interval.
        std::thread::sleep(Duration::from_millis(cfg.interval_ms.min(4_000) + 500));
        let status: Option<Status> = api::get(addr, "/v1/status").ok().and_then(|b| serde_json::from_str(&b).ok());
        let fans: Vec<&str> = cfg.fans.iter().filter(|f| f.curve.as_deref() == Some(curve)).map(|f| f.channel.as_str()).collect();
        out["saved"] = json!(path.display().to_string());
        out["profile"] = json!(cfg.profile);
        out["fans_on_curve"] = json!(fans);
        out["service"] = match status {
            Some(s) => json!({
                "config_error": s.config_error,
                "curve_duty": s.curves.iter().find(|c| c.name == curve).and_then(|c| c.duty).map(round1),
            }),
            None => json!("did not answer after the change"),
        };
        Ok(out)
    }
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked.filter(|v| VERSIONS.contains(v)).unwrap_or(VERSIONS[VERSIONS.len() - 1]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "camless", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

fn tools() -> Value {
    let read = json!({ "readOnlyHint": true, "openWorldHint": false });
    let write = json!({ "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false });
    let channels = json!({
        "type": "array", "items": { "type": "string" }, "minItems": 1,
        "description": "Fan channels from get_cooling_state, such as kraken-elite-v2/pump or control-hub/fan1.",
    });
    json!([
        {
            "name": "get_cooling_state",
            "title": "Cooling state",
            "description": "Every sensor with its value, every fan and pump channel with its speed, duty and curve, \
                every curve in the config, and failsafe_duty. Call this before changing anything.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": read,
        },
        {
            "name": "get_sensor_history",
            "title": "Sensor history",
            "description": "How sensors and fans moved over the last 10 minutes to 30 days: for each series its average, \
                lowest point, highest reading, last value, and up to 30 points with their average and highest reading. \
                Times are Unix seconds.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ids": {
                        "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": 8,
                        "description": "Sensor ids from get_cooling_state, or <fan channel>.rpm and <fan channel>.duty.",
                    },
                    "minutes": { "type": "integer", "minimum": 10, "maximum": 43200, "description": "How far back; 60 by default." },
                },
                "required": ["ids"],
                "additionalProperties": false,
            },
            "annotations": read,
        },
        {
            "name": "get_game_sessions",
            "title": "Game sessions",
            "description": "The latest game sessions, newest first: game, play time in seconds, average frame rate, \
                1 % and 0.1 % lows, and the average and highest value of each temperature while it was played. \
                Times are Unix milliseconds.",
            "inputSchema": {
                "type": "object",
                "properties": { "limit": { "type": "integer", "minimum": 1, "maximum": 100, "description": "10 by default." } },
                "additionalProperties": false,
            },
            "annotations": read,
        },
        {
            "name": "set_fan_curve",
            "title": "Set a fan curve",
            "description": "Creates or replaces a graph curve: the duty in % as a temperature sensor moves, linear between \
                points and flat beyond the ends. The fans listed then follow it; fans already on a curve of this name \
                follow the new shape too. Rules: 2 to 16 points; temperatures rise from point to point, up to 90 °C; \
                the duty never falls as the temperature rises; the last point's duty is at least failsafe_duty from \
                get_cooling_state. The service applies the change within an interval. A curve on a Kraken's liquid \
                temperature runs on the cooler itself.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "The curve's name; an existing graph curve of this name is replaced." },
                    "sensor": { "type": "string", "description": "A temperature sensor id from get_cooling_state, such as cpu.temp." },
                    "points": {
                        "type": "array", "minItems": 2, "maxItems": MAX_POINTS,
                        "items": {
                            "type": "object",
                            "properties": {
                                "temp": { "type": "number", "minimum": 0, "maximum": TOP_C },
                                "duty": { "type": "integer", "minimum": 0, "maximum": 100 },
                            },
                            "required": ["temp", "duty"],
                            "additionalProperties": false,
                        },
                    },
                    "fans": channels,
                    "response_up": { "type": "number", "minimum": 0, "maximum": MAX_RESPONSE_UP, "description": "Seconds to cover about two thirds of a rise; 2 by default." },
                    "response_down": { "type": "number", "minimum": 0, "maximum": MAX_RESPONSE_DOWN, "description": "The same for a fall; 10 by default." },
                    "hysteresis": { "type": "number", "minimum": 0, "maximum": MAX_HYSTERESIS, "description": "°C the sensor must move before the duty does; 0 by default." },
                },
                "required": ["name", "sensor", "points"],
                "additionalProperties": false,
            },
            "annotations": write,
        },
        {
            "name": "assign_curve",
            "title": "Put fans on a curve",
            "description": "Puts fans on a curve that is already in the config, of any kind, for example back on the one \
                they had before. Other fans keep theirs.",
            "inputSchema": {
                "type": "object",
                "properties": { "curve": { "type": "string", "description": "A curve name from get_cooling_state." }, "fans": channels },
                "required": ["curve", "fans"],
                "additionalProperties": false,
            },
            "annotations": write,
        },
    ])
}

fn cooling_state(addr: &str, path: &Path, cfg: &Config, status: &Status) -> Value {
    let sensors: Vec<Value> =
        status.sensors.iter().map(|s| json!({ "id": s.id, "name": s.name, "unit": s.unit.symbol(), "value": s.value.map(round1) })).collect();
    let mut fans = Vec::new();
    for d in status.devices.iter().filter(|d| d.connected) {
        for f in &d.fans {
            let channel = format!("{}/{}", d.id, f.id);
            let set = cfg.fans.iter().find(|x| x.channel == channel);
            let now = status.fans.iter().find(|x| x.channel == channel);
            let name = if f.name.is_empty() { &d.name } else { &f.name };
            fans.push(json!({
                "channel": channel,
                "name": name,
                "device": d.name,
                "pump": f.pump,
                "device_min_duty": f.min_duty,
                "rpm": f.rpm,
                "duty": now.and_then(|n| n.duty).or(f.duty),
                "curve": set.and_then(|s| s.curve.clone()),
                "curve_on_device": now.is_some_and(|n| n.on_device),
                "failsafe": now.is_some_and(|n| n.failsafe),
                "limits": set.map(|s| json!({ "offset": s.offset, "min": s.min, "max": s.max })),
            }));
        }
    }
    let curves: Vec<Value> = cfg
        .curves
        .iter()
        .map(|c| {
            let mut v = serde_json::to_value(c).unwrap_or_default();
            v["duty_now"] = json!(status.curves.iter().find(|x| x.name == c.name()).and_then(|x| x.duty).map(round1));
            v
        })
        .collect();
    json!({
        "service": {
            "address": addr,
            "elevated": status.service.as_ref().map(|s| s.elevated),
            "config": path.display().to_string(),
            "config_error": status.config_error,
        },
        "profile": cfg.profile,
        "failsafe_duty": cfg.failsafe_duty,
        "sensors": sensors,
        "fans": fans,
        "curves": curves,
    })
}

/// What `set_fan_curve` asks for.
#[derive(Debug)]
struct CurveRequest {
    name: String,
    sensor: String,
    points: Vec<Point>,
    fans: Vec<String>,
    response_up: Option<f32>,
    response_down: Option<f32>,
    hysteresis: Option<f32>,
}

impl CurveRequest {
    fn parse(args: &Value) -> Result<CurveRequest> {
        only(args, &["name", "sensor", "points", "fans", "response_up", "response_down", "hysteresis"])?;
        let name = text(args, "name")?.context("name is required")?.trim().to_string();
        if name.is_empty() || name.chars().count() > 40 {
            bail!("a curve's name is 1 to 40 characters");
        }
        let sensor = text(args, "sensor")?.context("sensor is required")?.to_string();
        let points = args.get("points").and_then(Value::as_array).context("points is a list of {temp, duty}")?;
        let points = points
            .iter()
            .map(|p| {
                only(p, &["temp", "duty"])?;
                let temp = number(p, "temp", 0.0..=f64::from(TOP_C))?.context("each point has a temp")?;
                let duty = number(p, "duty", 0.0..=100.0)?.context("each point has a duty")?;
                Ok((temp as f32, duty.round() as u8))
            })
            .collect::<Result<Vec<Point>>>()
            .context("points")?;
        let fans = if args.get("fans").is_some() { texts(args, "fans")? } else { Vec::new() };
        Ok(CurveRequest {
            name,
            sensor,
            points,
            fans,
            response_up: number(args, "response_up", 0.0..=MAX_RESPONSE_UP)?.map(|v| v as f32),
            response_down: number(args, "response_down", 0.0..=MAX_RESPONSE_DOWN)?.map(|v| v as f32),
            hysteresis: number(args, "hysteresis", 0.0..=MAX_HYSTERESIS)?.map(|v| v as f32),
        })
    }
}

/// The points speed the fans up as it gets hot, and reach `floor` by the last one.
fn check_points(points: &[Point], floor: u8) -> Result<()> {
    if !(2..=MAX_POINTS).contains(&points.len()) {
        bail!("a curve has 2 to {MAX_POINTS} points");
    }
    for w in points.windows(2) {
        let ((t0, d0), (t1, d1)) = (w[0], w[1]);
        if t1 <= t0 {
            bail!("temperatures rise from point to point: {t1} °C comes after {t0} °C");
        }
        if d1 < d0 {
            bail!("the duty may not fall as it gets hotter: {d0} % at {t0} °C, then {d1} % at {t1} °C");
        }
    }
    let (t, d) = points[points.len() - 1];
    if d < floor {
        bail!("the last point, at {t} °C, has {d} %; it needs at least {floor} %, the config's failsafe_duty, so the fans speed up when it gets hot");
    }
    Ok(())
}

/// Adds or replaces the graph curve and puts the fans asked for on it.
fn set_curve(cfg: &mut Config, status: &Status, req: &CurveRequest) -> Result<Graph> {
    let Some(sensor) = status.sensors.iter().find(|s| s.id == req.sensor) else {
        let temps: Vec<&str> = status.sensors.iter().filter(|s| s.unit == Unit::Celsius).map(|s| s.id.as_str()).collect();
        bail!("there is no sensor {:?}; temperature sensors: {}", req.sensor, temps.join(", "));
    };
    if sensor.unit != Unit::Celsius {
        bail!("{} is in {}; a curve set here follows a temperature", sensor.id, sensor.unit.symbol());
    }
    check_points(&req.points, cfg.failsafe_duty)?;
    check_fans(status, &req.fans)?;
    let i = match cfg.curves.iter().position(|c| c.name() == req.name) {
        Some(i) if matches!(cfg.curves[i], CurveSpec::Graph(_)) => i,
        Some(_) => bail!("{:?} is a curve of another kind; pick another name, or assign_curve to use it", req.name),
        None => {
            let r = Response::default();
            cfg.curves.push(CurveSpec::Graph(Graph {
                name: req.name.clone(),
                sensor: String::new(),
                points: Vec::new(),
                hysteresis: r.hysteresis,
                response_up: r.response_up,
                response_down: r.response_down,
            }));
            cfg.curves.len() - 1
        }
    };
    let CurveSpec::Graph(graph) = &mut cfg.curves[i] else { unreachable!() };
    graph.sensor = req.sensor.clone();
    graph.points = req.points.clone();
    // Smoothing not asked for stays as it was.
    graph.response_up = req.response_up.unwrap_or(graph.response_up);
    graph.response_down = req.response_down.unwrap_or(graph.response_down);
    graph.hysteresis = req.hysteresis.unwrap_or(graph.hysteresis);
    let graph = graph.clone();
    put_on(cfg, &req.name, &req.fans);
    cfg.validate()?;
    Ok(graph)
}

/// Puts fans on a curve the config already has.
fn assign(cfg: &mut Config, status: &Status, curve: &str, fans: &[String]) -> Result<()> {
    if cfg.curve(curve).is_none() {
        let names: Vec<&str> = cfg.curves.iter().map(|c| c.name()).collect();
        bail!("there is no curve {curve:?}; curves: {}", names.join(", "));
    }
    if fans.is_empty() {
        bail!("fans lists at least one channel");
    }
    check_fans(status, fans)?;
    put_on(cfg, curve, fans);
    cfg.validate()
}

/// Every channel is a fan or pump of a connected device.
fn check_fans(status: &Status, fans: &[String]) -> Result<()> {
    let known: Vec<String> =
        status.devices.iter().filter(|d| d.connected).flat_map(|d| d.fans.iter().map(move |f| format!("{}/{}", d.id, f.id))).collect();
    if let Some(bad) = fans.iter().find(|f| !known.contains(f)) {
        bail!("there is no fan channel {bad:?}; channels: {}", known.join(", "));
    }
    Ok(())
}

/// Puts each channel on `curve`, keeping its limits.
fn put_on(cfg: &mut Config, curve: &str, fans: &[String]) {
    for ch in fans {
        match cfg.fans.iter_mut().find(|f| &f.channel == ch) {
            Some(f) => f.curve = Some(curve.to_string()),
            None => cfg.fans.push(Fan { curve: Some(curve.to_string()), ..Fan::new(ch) }),
        }
    }
}

/// Each series in a few numbers and at most [`HISTORY_POINTS`] points.
fn summary(r: &Range, ids: &[String]) -> Value {
    let mut series = Map::new();
    for id in ids {
        let s = r.series.get(id).filter(|s| s.avg.iter().any(Option::is_some));
        let Some(s) = s else {
            series.insert(id.clone(), json!("no readings in this range"));
            continue;
        };
        let avgs: Vec<f32> = s.avg.iter().flatten().copied().collect();
        let highest = |xs: &[Option<f32>]| xs.iter().flatten().copied().reduce(f32::max);
        let per = s.avg.len().div_ceil(HISTORY_POINTS).max(1);
        let points: Vec<Value> = (0..s.avg.len())
            .step_by(per)
            .filter_map(|i| {
                let end = (i + per).min(s.avg.len());
                let a: Vec<f32> = s.avg[i..end].iter().flatten().copied().collect();
                (!a.is_empty()).then(|| {
                    json!({
                        "t": r.from + i as i64 * r.step,
                        "avg": round1(a.iter().sum::<f32>() / a.len() as f32),
                        "max": highest(&s.max[i..end]).map(round1),
                    })
                })
            })
            .collect();
        series.insert(
            id.clone(),
            json!({
                "avg": round1(avgs.iter().sum::<f32>() / avgs.len() as f32),
                "min": avgs.iter().copied().reduce(f32::min).map(round1),
                "max": highest(&s.max).map(round1),
                "last": avgs.last().copied().map(round1),
                "points": points,
            }),
        );
    }
    json!({ "from": r.from, "to": r.from + r.count as i64 * r.step, "series": series })
}

fn round1(v: f32) -> f64 {
    (f64::from(v) * 10.0).round() / 10.0
}

/// Ids in a query: a second device's id has a `#`.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// No arguments but `keys`, so a misspelt one is not quietly ignored.
fn only(args: &Value, keys: &[&str]) -> Result<()> {
    let Some(m) = args.as_object() else { bail!("arguments are an object") };
    if let Some(k) = m.keys().find(|k| !keys.contains(&k.as_str())) {
        bail!("there is no argument {k:?}");
    }
    Ok(())
}

fn text<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => bail!("{key} is a string"),
    }
}

fn texts(args: &Value, key: &str) -> Result<Vec<String>> {
    let list = args.get(key).and_then(Value::as_array).with_context(|| format!("{key} is a list of strings"))?;
    list.iter().map(|v| v.as_str().map(String::from).with_context(|| format!("{key} is a list of strings"))).collect()
}

fn number(args: &Value, key: &str, range: RangeInclusive<f64>) -> Result<Option<f64>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let n = v.as_f64().filter(|n| range.contains(n));
            Ok(Some(n.with_context(|| format!("{key} is a number from {} to {}", range.start(), range.end()))?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camless_core::status::{Device, FanChannel, Sensor};

    fn status() -> Status {
        let fan = |id: &str, pump: bool| FanChannel { id: id.into(), name: id.into(), pump, ..Default::default() };
        Status {
            config_path: r"C:\camless\camless.toml".into(),
            devices: vec![Device {
                id: "kraken".into(),
                name: "Kraken".into(),
                connected: true,
                fans: vec![fan("pump", true), fan("fan", false)],
                ..Default::default()
            }],
            sensors: vec![
                Sensor { id: "cpu.temp".into(), name: "CPU".into(), unit: Unit::Celsius, value: Some(45.0) },
                Sensor { id: "cpu.load".into(), name: "CPU load".into(), unit: Unit::Percent, value: Some(10.0) },
            ],
            ..Default::default()
        }
    }

    fn ask(server: &Server, msg: Value) -> Value {
        serde_json::from_str(&server.message(&msg.to_string()).expect("a reply")).unwrap()
    }

    fn server() -> Server {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        // Nothing listens on port 1, so tools that need the service fail at once. A
        // file each, so no test reads another's half written and asks the real port.
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("camless-mcp-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("camless.toml");
        std::fs::write(&config, "[api]\nlisten = \"127.0.0.1:1\"\n").unwrap();
        Server { config }
    }

    #[test]
    fn the_handshake_agrees_on_a_version() {
        let s = server();
        let r = ask(&s, json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } }));
        assert_eq!(r["id"], 1);
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(r["result"]["serverInfo"]["name"], "camless");
        // A version it does not know gets the newest one it does.
        let r = ask(&s, json!({ "jsonrpc": "2.0", "id": "a", "method": "initialize", "params": { "protocolVersion": "1999-01-01" } }));
        assert_eq!(r["result"]["protocolVersion"], VERSIONS[VERSIONS.len() - 1]);
        // Notifications get no reply.
        assert_eq!(s.message(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#), None);
        assert_eq!(ask(&s, json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" }))["result"], json!({}));
    }

    #[test]
    fn errors_follow_json_rpc() {
        let s = server();
        assert_eq!(serde_json::from_str::<Value>(&s.message("{nope").unwrap()).unwrap()["error"]["code"], -32700);
        assert_eq!(ask(&s, json!({ "jsonrpc": "2.0", "id": 3, "method": "resources/list" }))["error"]["code"], -32601);
        let r = ask(&s, json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "format_disk" } }));
        assert_eq!(r["error"]["code"], -32602);
        // A tool that fails says why, as a result.
        let r = ask(&s, json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": { "name": "get_cooling_state" } }));
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("does not answer at 127.0.0.1:1"));
        // A batch gets a list back.
        let r = ask(&s, json!([{ "jsonrpc": "2.0", "id": 6, "method": "ping" }, { "jsonrpc": "2.0", "method": "notifications/initialized" }]));
        assert_eq!(r.as_array().unwrap().len(), 1);
    }

    #[test]
    fn tools_are_listed_with_schemas() {
        let tools = tools();
        let names: Vec<&str> = tools.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["get_cooling_state", "get_sensor_history", "get_game_sessions", "set_fan_curve", "assign_curve"]);
        for t in tools.as_array().unwrap() {
            assert_eq!(t["inputSchema"]["type"], "object");
            assert!(t["description"].as_str().is_some_and(|d| !d.contains("  ")), "{}", t["name"]);
        }
    }

    #[test]
    fn curves_must_speed_up_when_hot() {
        assert!(check_points(&[(30.0, 30), (60.0, 50), (80.0, 100)], 70).is_ok());
        // A flat curve at the floor is fine.
        assert!(check_points(&[(30.0, 70), (60.0, 70)], 70).is_ok());
        let err = |p: &[Point]| check_points(p, 70).unwrap_err().to_string();
        assert!(err(&[(30.0, 30)]).contains("2 to 16"));
        assert!(err(&[(60.0, 30), (50.0, 80)]).contains("rise"));
        assert!(err(&[(30.0, 30), (30.0, 80)]).contains("rise"));
        assert!(err(&[(30.0, 60), (50.0, 40), (80.0, 100)]).contains("may not fall"));
        assert!(err(&[(30.0, 20), (85.0, 50)]).contains("at least 70 %"));
    }

    #[test]
    fn requests_are_checked_before_anything_is_read() {
        let req = |v: Value| CurveRequest::parse(&v).map_err(|e| format!("{e:#}"));
        let ok = req(json!({ "name": " Quiet ", "sensor": "cpu.temp", "points": [{ "temp": 30, "duty": 25.4 }, { "temp": 80, "duty": 100 }] })).unwrap();
        assert_eq!((ok.name.as_str(), ok.points.as_slice()), ("Quiet", &[(30.0, 25), (80.0, 100)][..]));
        assert!(req(json!({ "name": "Q", "sensor": "cpu.temp", "points": [{ "temp": 95, "duty": 100 }] })).unwrap_err().contains("temp is a number from 0 to 90"));
        assert!(req(json!({ "name": "Q", "sensor": "cpu.temp", "points": [], "response_up": 60 })).unwrap_err().contains("response_up"));
        assert!(req(json!({ "name": "Q", "sensor": "cpu.temp", "points": [], "speed": 1 })).unwrap_err().contains("no argument \"speed\""));
        assert!(req(json!({ "name": "", "sensor": "cpu.temp", "points": [] })).unwrap_err().contains("1 to 40"));
    }

    #[test]
    fn a_curve_is_added_and_the_fans_put_on_it() {
        let mut cfg = Config::parse(
            "[[curve]]\nkind = \"fixed\"\nname = \"Still\"\nduty = 40\n\n\
             [[fan]]\nchannel = \"kraken/fan\"\ncurve = \"Still\"\nmin = 25\n",
        )
        .unwrap();
        let status = status();
        let req = |name: &str, sensor: &str, fans: &[&str]| CurveRequest {
            name: name.into(),
            sensor: sensor.into(),
            points: vec![(30.0, 30), (70.0, 100)],
            fans: fans.iter().map(|f| f.to_string()).collect(),
            response_up: None,
            response_down: Some(20.0),
            hysteresis: None,
        };
        let g = set_curve(&mut cfg, &status, &req("CPU", "cpu.temp", &["kraken/fan", "kraken/pump"])).unwrap();
        assert_eq!((g.response_up, g.response_down, g.hysteresis), (2.0, 20.0, 0.0));
        assert_eq!(cfg.curves.len(), 2);
        // The fan keeps its limits; the pump gets an entry.
        assert_eq!((cfg.fans[0].curve.as_deref(), cfg.fans[0].min), (Some("CPU"), 25));
        assert_eq!((cfg.fans[1].channel.as_str(), cfg.fans[1].curve.as_deref()), ("kraken/pump", Some("CPU")));
        // Replacing it keeps what was not asked for.
        let mut again = req("CPU", "cpu.temp", &[]);
        again.response_down = None;
        again.points = vec![(40.0, 40), (75.0, 90)];
        let g = set_curve(&mut cfg, &status, &again).unwrap();
        assert_eq!((g.points.len(), g.response_down), (2, 20.0));
        assert_eq!(cfg.curves.len(), 2);
        let err = |r: CurveRequest| set_curve(&mut cfg.clone(), &status, &r).unwrap_err().to_string();
        assert!(err(req("Still", "cpu.temp", &[])).contains("another kind"));
        assert!(err(req("X", "gpu.temp", &[])).contains("temperature sensors: cpu.temp"));
        assert!(err(req("X", "cpu.load", &[])).contains("follows a temperature"));
        assert!(err(req("X", "cpu.temp", &["hub/fan1"])).contains("channels: kraken/pump, kraken/fan"));
        // Back on the old curve.
        assign(&mut cfg, &status, "Still", &["kraken/fan".into()]).unwrap();
        assert_eq!(cfg.fans[0].curve.as_deref(), Some("Still"));
        assert!(assign(&mut cfg, &status, "Gone", &["kraken/fan".into()]).unwrap_err().to_string().contains("curves: Still, CPU"));
    }

    #[test]
    fn history_is_cut_down_to_a_few_points() {
        let n = 120;
        let mut series = std::collections::BTreeMap::new();
        let avg: Vec<Option<f32>> = (0..n).map(|i| (i >= 10).then_some(40.0 + (i % 4) as f32)).collect();
        let max: Vec<Option<f32>> = avg.iter().map(|v| v.map(|v| v + 5.0)).collect();
        series.insert("cpu.temp".to_string(), crate::history::Series { avg, max });
        let r = Range { from: 1_000, step: 60, count: n, series };
        let s = summary(&r, &["cpu.temp".into(), "gpu.temp".into()]);
        let cpu = &s["series"]["cpu.temp"];
        assert_eq!((cpu["avg"].as_f64(), cpu["min"].as_f64(), cpu["max"].as_f64()), (Some(41.5), Some(40.0), Some(48.0)));
        // 4 readings a point; the first two points had no readings.
        let points = cpu["points"].as_array().unwrap();
        assert_eq!(points.len(), 28);
        assert_eq!(points[0]["t"], 1_000 + 8 * 60);
        assert_eq!(s["series"]["gpu.temp"], "no readings in this range");
        assert_eq!(s["to"], 1_000 + 120 * 60);
    }

    #[test]
    fn ids_are_encoded_for_the_query() {
        assert_eq!(encode("kraken-elite-v2#2/pump.rpm"), "kraken-elite-v2%232/pump.rpm");
    }
}
