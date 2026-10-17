//! Local HTTP API, loopback only.
//!
//! `GET /v1/monitoring` has the shape CAM passes to Web Integration pages through
//! `window.nzxt.v1.onMonitoringDataUpdate`, so a KrakenDial-style page keeps working.
//! `GET /v1/status` is everything camless knows. `POST /v1/lcd/frame` takes one
//! 640×640 frame, raw RGBA (1 638 400 bytes) or RGB565 big-endian (819 200 bytes).

use crate::daemon::{Event, Snapshot};
use crate::protocol::{rgba_to_rgb565, FanMode};
use crate::q565;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::io::Read;
use std::net::SocketAddr;
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Request, Response, Server};

const LCD_SIDE: usize = 640;
const RGBA_LEN: usize = LCD_SIDE * LCD_SIDE * 4;
const RGB565_LEN: usize = LCD_SIDE * LCD_SIDE * 2;

pub fn spawn(listen: &str, shared: Arc<Mutex<Snapshot>>, frames: SyncSender<Event>) -> Result<()> {
    let addr: SocketAddr = listen.parse().with_context(|| format!("api listen {listen:?} is not ip:port"))?;
    if !addr.ip().is_loopback() {
        bail!("api listen must be a loopback address; {listen} would expose the cooler to the network");
    }
    let server = Server::http(addr).map_err(|e| anyhow!("cannot listen on {listen}: {e}"))?;
    let hosts = [format!("127.0.0.1:{}", addr.port()), format!("localhost:{}", addr.port()), format!("[::1]:{}", addr.port())];
    std::thread::Builder::new().name("api".into()).spawn(move || {
        for req in server.incoming_requests() {
            let res = handle(req, &hosts, &shared, &frames);
            if let Err(e) = res {
                eprintln!("api: {e:#}");
            }
        }
    })?;
    Ok(())
}

fn header<'a>(req: &'a Request, name: &'static str) -> Option<&'a str> {
    req.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str())
}

fn handle(mut req: Request, hosts: &[String], shared: &Mutex<Snapshot>, frames: &SyncSender<Event>) -> Result<()> {
    // A Host check stops DNS rebinding: a web page cannot reach this server under its
    // own domain name.
    if !header(&req, "Host").is_some_and(|h| hosts.iter().any(|ok| ok.eq_ignore_ascii_case(h))) {
        return Ok(req.respond(Response::empty(421))?);
    }
    let snapshot = || shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match (req.method(), req.url()) {
        (Method::Get, "/v1/monitoring") => respond_json(req, &monitoring(&snapshot())),
        (Method::Get, "/v1/status") => respond_json(req, &status(&snapshot())),
        (Method::Post, "/v1/lcd/frame") => {
            // Browsers cannot send this content type cross-site without a preflight,
            // which this server never approves.
            if header(&req, "Content-Type") != Some("application/octet-stream") {
                return Ok(req.respond(Response::from_string("Content-Type must be application/octet-stream").with_status_code(415))?);
            }
            let mut body = Vec::with_capacity(RGBA_LEN);
            req.as_reader().take(RGBA_LEN as u64 + 1).read_to_end(&mut body)?;
            let pixels = match body.len() {
                RGBA_LEN => rgba_to_rgb565(&body),
                RGB565_LEN => body.chunks_exact(2).map(|p| u16::from_be_bytes([p[0], p[1]])).collect(),
                n => {
                    let msg = format!("{n} bytes; expected {RGBA_LEN} (RGBA) or {RGB565_LEN} (RGB565 BE)");
                    return Ok(req.respond(Response::from_string(msg).with_status_code(400))?);
                }
            };
            // Encoded here, so the cooling loop only has to send it.
            let side = LCD_SIDE as u16;
            let code = match frames.try_send(Event::Frame(q565::encode(side, side, &pixels))) {
                Ok(()) => 202,
                // The cooler is still busy with earlier frames; the client should slow down.
                Err(TrySendError::Full(_)) => 429,
                Err(TrySendError::Disconnected(_)) => 503,
            };
            Ok(req.respond(Response::empty(code))?)
        }
        _ => Ok(req.respond(Response::empty(404))?),
    }
}

fn respond_json(req: Request, v: &Value) -> Result<()> {
    let res = Response::from_string(v.to_string())
        .with_header(Header::from_bytes("Content-Type", "application/json").unwrap())
        .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap());
    Ok(req.respond(res)?)
}

/// f32 readings, rounded so they serialize as 59.1 rather than 59.099998474.
fn round(v: Option<f32>, places: i32) -> Option<f64> {
    let k = 10f64.powi(places);
    v.map(|v| (v as f64 * k).round() / k)
}

/// CAM's monitoring object: loads are 0..1, memory in MB.
pub fn monitoring(s: &Snapshot) -> Value {
    let r = &s.readings;
    let mb = |gb: Option<f32>| gb.map(|g| (g * 1024.0).round());
    let gpus = match &s.gpu_name {
        Some(name) => json!([{
            "name": name,
            "temperature": round(r.gpu_temp_c, 1),
            "load": round(r.gpu_load_pct.map(|l| l / 100.0), 3),
            "power": round(r.gpu_power_w, 1),
        }]),
        None => json!([]),
    };
    json!({
        "cpus": [{
            "name": s.cpu_name,
            "temperature": round(r.cpu_temp_c, 1),
            "load": round(r.cpu_load_pct.map(|l| l / 100.0), 3),
            "power": round(r.cpu_power_w, 1),
        }],
        "gpus": gpus,
        "kraken": s.kraken.as_ref().map(|k| json!({
            "liquidTemperature": round(Some(k.liquid_c), 1),
            "pump": { "speed": k.pump_rpm, "duty": k.pump_duty },
            "fan": { "speed": k.fan_rpm, "duty": k.fan_duty },
        })),
        "ram": { "inUse": mb(r.ram_used_gb), "totalSize": mb(r.ram_total_gb) },
    })
}

pub fn status(s: &Snapshot) -> Value {
    let r = &s.readings;
    json!({
        "cpu": s.cpu_name,
        "gpu": s.gpu_name,
        "sensors": {
            "cpu_temp_c": round(r.cpu_temp_c, 1),
            "cpu_load_pct": round(r.cpu_load_pct, 1),
            "cpu_power_w": round(r.cpu_power_w, 1),
            "gpu_temp_c": round(r.gpu_temp_c, 1),
            "gpu_load_pct": round(r.gpu_load_pct, 1),
            "gpu_power_w": round(r.gpu_power_w, 1),
            "ram_used_gb": round(r.ram_used_gb, 2),
            "ram_total_gb": round(r.ram_total_gb, 2),
        },
        "kraken": s.kraken.as_ref().map(|k| json!({
            "liquid_c": round(Some(k.liquid_c), 1),
            "pump_rpm": k.pump_rpm,
            "pump_duty": k.pump_duty,
            "fan_rpm": k.fan_rpm,
            "fan_duty": k.fan_duty,
        })),
        "hub": s.hub.as_ref().map(|h| json!({
            "noise_db": h.noise_db,
            "fans": h.fans.iter().enumerate().map(|(i, f)| json!({
                "port": i + 1,
                "connected": !matches!(f.mode, FanMode::None),
                "rpm": f.rpm,
                "duty": f.duty,
            })).collect::<Vec<_>>(),
        })),
        "targets": s.duties.iter().map(|(label, duty)| json!({ "channel": label, "duty": duty })).collect::<Vec<_>>(),
    })
}
