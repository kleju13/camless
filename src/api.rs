//! Local HTTP API, loopback only.
//!
//! `GET /v1/status` is everything the daemon knows, as `camless_core::status::Status`;
//! the GUI reads it. `GET /v1/monitoring` has the shape CAM passes to Web Integration
//! pages through `window.nzxt.v1.onMonitoringDataUpdate`, so a KrakenDial-style page
//! keeps working. `POST /v1/lcd/frame` takes one frame for the LCD in frames mode, raw
//! RGBA or RGB565 big-endian at the screen's size (640×640 on a Kraken Elite).

use crate::daemon::{Event, Snapshot};
use crate::q565;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::io::Read;
use std::net::SocketAddr;
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Request, Response, Server};

pub fn spawn(listen: &str, shared: Arc<Mutex<Snapshot>>, frames: SyncSender<Event>) -> Result<()> {
    let addr: SocketAddr = listen.parse().with_context(|| format!("api listen {listen:?} is not ip:port"))?;
    if !addr.ip().is_loopback() {
        bail!("api listen must be a loopback address; {listen} would expose the devices to the network");
    }
    let server = Server::http(addr).map_err(|e| anyhow!("cannot listen on {listen}: {e}"))?;
    let hosts = [format!("127.0.0.1:{}", addr.port()), format!("localhost:{}", addr.port()), format!("[::1]:{}", addr.port())];
    std::thread::Builder::new().name("api".into()).spawn(move || {
        for req in server.incoming_requests() {
            if let Err(e) = handle(req, &hosts, &shared, &frames) {
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
        (Method::Get, "/v1/status") => respond_json(req, &serde_json::to_value(&snapshot().status)?),
        (Method::Post, "/v1/lcd/frame") => {
            // Browsers cannot send this content type cross-site without a preflight,
            // which this server never approves.
            if header(&req, "Content-Type") != Some("application/octet-stream") {
                return Ok(req.respond(Response::from_string("Content-Type must be application/octet-stream").with_status_code(415))?);
            }
            let Some((device, width, height)) = snapshot().frame_target else {
                return Ok(req.respond(Response::from_string("no connected LCD is in frames mode").with_status_code(409))?);
            };
            let pixels = usize::from(width) * usize::from(height);
            let mut body = Vec::with_capacity(pixels * 4);
            req.as_reader().take(pixels as u64 * 4 + 1).read_to_end(&mut body)?;
            let pixels = match body.len() {
                n if n == pixels * 4 => q565::rgb565_from_rgba(&body),
                n if n == pixels * 2 => body.chunks_exact(2).map(|p| u16::from_be_bytes([p[0], p[1]])).collect(),
                n => {
                    let msg = format!("{n} bytes; a {width}×{height} frame is {} (RGBA) or {} (RGB565 BE)", pixels * 4, pixels * 2);
                    return Ok(req.respond(Response::from_string(msg).with_status_code(400))?);
                }
            };
            // Encoded here, so the cooling loop only has to send it.
            let code = match frames.try_send(Event::Frame { device, q565: q565::encode(width, height, &pixels) }) {
                Ok(()) => 202,
                // The device is still busy with earlier frames; the client should slow down.
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

/// CAM's monitoring object: loads are 0..1, memory in MB. `kraken` is the first
/// device with a liquid sensor, its pump and its first fan.
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
    let kraken = s.status.devices.iter().filter(|d| d.connected).find_map(|d| {
        let liquid = s.status.sensors.iter().find(|x| x.id == format!("{}.liquid", d.id))?;
        let channel = |pump: bool| {
            d.fans.iter().find(|f| f.pump == pump).map(|f| json!({ "speed": f.rpm, "duty": f.duty }))
        };
        Some(json!({
            "liquidTemperature": round(liquid.value, 1),
            "pump": channel(true),
            "fan": channel(false),
        }))
    });
    json!({
        "cpus": [{
            "name": s.cpu_name,
            "temperature": round(r.cpu_temp_c, 1),
            "load": round(r.cpu_load_pct.map(|l| l / 100.0), 3),
            "power": round(r.cpu_power_w, 1),
        }],
        "gpus": gpus,
        "kraken": kraken,
        "ram": { "inUse": mb(r.ram_used_gb), "totalSize": mb(r.ram_total_gb) },
    })
}
