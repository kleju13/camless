//! `camless report`: what an issue about hardware needs, for a device camless does
//! not drive yet or drives wrong. It lists the NZXT devices, and Asetek's, which
//! made the older Krakens, with their USB and HID descriptors, and asks the running
//! service for firmware versions and errors.
//!
//! Read only: standard USB descriptor reads, no commands sent to the devices. Serial
//! numbers, device paths and the user's and the PC's names stay out; `scrub` takes
//! them out of error messages too.

use crate::api;
use crate::drivers::{Model, MODELS, NZXT_VID};
use crate::sensors::{reg_dword, reg_string};
use anyhow::Result;
use camless_core::status::Status;
use hidapi::HidApi;
use nusb::transfer::EndpointType;
use serde::Serialize;
use std::fmt::Write as _;

/// Asetek's vendor id, on the Krakens it made: X31, X41, X61, X40 and X60.
pub const ASETEK_VID: u16 = 0x2433;

/// What the report does to the devices. The app's dialog and the issue template say
/// the same.
pub const READ_ONLY: &str = "read only: standard USB descriptor reads, no commands sent to the devices";

#[derive(Serialize)]
pub struct Report {
    pub title: String,
    pub text: String,
}

/// Everything the report says, gathered first so that rendering it can be tested.
struct Facts {
    windows: Option<String>,
    cpu: Option<String>,
    /// The running service's status, or why there is none.
    service: std::result::Result<Status, String>,
    /// Supported devices on the bus, for when the service does not answer.
    found: Vec<(String, &'static Model)>,
    usb: std::result::Result<Vec<Usb>, String>,
    hid: std::result::Result<Vec<Hid>, String>,
    /// Serial numbers, instance ids, user and PC names, which `scrub_facts` takes out.
    secrets: Vec<String>,
}

/// One USB device model; identical devices share an entry.
struct Usb {
    vid: u16,
    pid: u16,
    count: usize,
    product: Option<String>,
    bcd: u16,
    speed: Option<nusb::Speed>,
    /// The active configuration's interfaces, or why they could not be read.
    interfaces: std::result::Result<Vec<Interface>, String>,
}

struct Interface {
    number: u8,
    alt: u8,
    class: [u8; 3],
    endpoints: Vec<Endpoint>,
}

struct Endpoint {
    address: u8,
    kind: EndpointType,
    max_packet: usize,
}

/// One HID top-level collection; Windows lists each as its own device.
struct Hid {
    vid: u16,
    pid: u16,
    product: Option<String>,
    interface: i32,
    usage_page: u16,
    usage: u16,
    descriptor: std::result::Result<Vec<u8>, String>,
}

pub fn collect(api_addr: Option<&str>) -> Report {
    let mut secrets = Vec::new();
    let usb = usb_devices(&mut secrets);
    let (found, hid) = match HidApi::new() {
        Ok(api) => {
            let found = crate::drivers::scan(&api).0.into_iter().map(|f| (f.id, f.model)).collect();
            (found, Ok(hid_collections(&api, &mut secrets)))
        }
        Err(e) => (Vec::new(), Err(e.to_string())),
    };
    for var in ["USERPROFILE", "USERNAME", "COMPUTERNAME", "USERDOMAIN"] {
        secrets.extend(std::env::var(var).ok());
    }
    let service = match api_addr {
        Some(addr) => fetch_status(addr),
        None => Err("Local API is off in the config, so the report cannot ask the service".into()),
    };
    let cpu = reg_string(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString");
    let mut facts = Facts { windows: windows_version(), cpu, service, found, usb, hid, secrets };
    scrub_facts(&mut facts);
    Report { title: title(&facts), text: render(&facts) }
}

fn windows_version() -> Option<String> {
    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let build = reg_string(KEY, "CurrentBuild")?;
    // Windows 11 still calls itself Windows 10 in ProductName.
    let name = if build.parse::<u32>().is_ok_and(|b| b >= 22000) { "Windows 11" } else { "Windows 10" };
    let release = reg_string(KEY, "DisplayVersion").map(|v| format!(" {v}")).unwrap_or_default();
    let ubr = reg_dword(KEY, "UBR").map(|u| format!(".{u}")).unwrap_or_default();
    Some(format!("{name}{release}, build {build}{ubr}"))
}

/// `GET /v1/status` from the service, the way the app asks it.
fn fetch_status(addr: &str) -> std::result::Result<Status, String> {
    let get = || -> Result<Status> { Ok(serde_json::from_str(&api::get(addr, "/v1/status")?)?) };
    get().map_err(|e| format!("not reachable at {addr} ({e:#})"))
}

fn usb_devices(secrets: &mut Vec<String>) -> std::result::Result<Vec<Usb>, String> {
    let mut list: Vec<_> = nusb::list_devices().map_err(|e| e.to_string())?.filter(|d| is_ours(d.vendor_id())).collect();
    list.sort_by_key(|d| (d.vendor_id(), d.product_id()));
    let mut out: Vec<Usb> = Vec::new();
    for d in list {
        secrets.extend(d.serial_number().map(String::from));
        // Instance ids end in the serial number or in an id Windows made up for the port.
        for id in [d.instance_id(), d.parent_instance_id()] {
            let id = id.to_string_lossy().into_owned();
            secrets.extend(id.rsplit('\\').next().map(String::from));
            secrets.push(id);
        }
        if let Some(u) = out.iter_mut().find(|u| (u.vid, u.pid) == (d.vendor_id(), d.product_id())) {
            u.count += 1;
            continue;
        }
        out.push(Usb {
            vid: d.vendor_id(),
            pid: d.product_id(),
            count: 1,
            product: d.product_string().map(String::from),
            bcd: d.device_version(),
            speed: d.speed(),
            interfaces: interfaces(&d),
        });
    }
    Ok(out)
}

/// The configuration descriptor, which Windows reads through the hub without
/// claiming the device.
fn interfaces(d: &nusb::DeviceInfo) -> std::result::Result<Vec<Interface>, String> {
    let dev = d.open().map_err(|e| e.to_string())?;
    let config = dev.active_configuration().ok().or_else(|| dev.configurations().next()).ok_or("no configuration descriptor")?;
    Ok(config
        .interface_alt_settings()
        .map(|a| Interface {
            number: a.interface_number(),
            alt: a.alternate_setting(),
            class: [a.class(), a.subclass(), a.protocol()],
            endpoints: a
                .endpoints()
                .map(|e| Endpoint { address: e.address(), kind: e.transfer_type(), max_packet: e.max_packet_size() })
                .collect(),
        })
        .collect())
}

/// Opening a HID device sends it nothing; Windows rebuilds the report descriptor
/// from what it parsed when the device was plugged in.
fn hid_collections(api: &HidApi, secrets: &mut Vec<String>) -> Vec<Hid> {
    let mut list: Vec<_> = api.device_list().filter(|d| is_ours(d.vendor_id())).collect();
    list.sort_by_key(|d| d.path().to_owned());
    let mut out: Vec<Hid> = Vec::new();
    for d in list {
        secrets.extend(d.serial_number().map(String::from));
        let key = (d.vendor_id(), d.product_id(), d.interface_number(), d.usage_page(), d.usage());
        if out.iter().any(|h| (h.vid, h.pid, h.interface, h.usage_page, h.usage) == key) {
            continue;
        }
        let descriptor = d
            .open_device(api)
            .and_then(|h| {
                let mut buf = [0u8; 4096];
                let n = h.get_report_descriptor(&mut buf)?;
                Ok(buf[..n].to_vec())
            })
            .map_err(|e| e.to_string());
        out.push(Hid {
            vid: d.vendor_id(),
            pid: d.product_id(),
            product: d.product_string().map(String::from),
            interface: d.interface_number(),
            usage_page: d.usage_page(),
            usage: d.usage(),
            descriptor,
        });
    }
    out
}

fn is_ours(vid: u16) -> bool {
    vid == NZXT_VID || vid == ASETEK_VID
}

fn model(vid: u16, pid: u16) -> Option<&'static Model> {
    MODELS.iter().find(|m| (m.vid, m.pid) == (vid, pid))
}

/// Devices without a driver, NZXT's then Asetek's: (vid, pid, product).
fn undriven(f: &Facts) -> Vec<(u16, u16, String)> {
    let usb = f.usb.as_deref().unwrap_or_default().iter().map(|u| (u.vid, u.pid, u.product.clone()));
    let hid = f.hid.as_deref().unwrap_or_default().iter().map(|h| (h.vid, h.pid, h.product.clone()));
    let mut out: Vec<(u16, u16, String)> = Vec::new();
    for (vid, pid, product) in usb.chain(hid) {
        if model(vid, pid).is_none() && !out.iter().any(|o| (o.0, o.1) == (vid, pid)) {
            out.push((vid, pid, product.unwrap_or_else(|| "?".into())));
        }
    }
    out.sort_by_key(|o| (o.0 != NZXT_VID, o.1));
    out
}

fn title(f: &Facts) -> String {
    let new = undriven(f);
    if !new.is_empty() {
        return format!("Hardware: {}", new.iter().map(|(v, p, name)| format!("{name} ({v:04x}:{p:04x})")).collect::<Vec<_>>().join(", "));
    }
    let names: Vec<String> = match &f.service {
        Ok(s) if !s.devices.is_empty() => s.devices.iter().map(|d| d.name.clone()).collect(),
        _ => f.found.iter().map(|(_, m)| m.name.to_string()).collect(),
    };
    if names.is_empty() {
        return "Hardware: no NZXT device found".into();
    }
    format!("Hardware: {}", names.join(", "))
}

fn render(f: &Facts) -> String {
    let mut s = String::new();
    let head: Vec<String> = [Some(format!("camless {}", env!("CARGO_PKG_VERSION"))), f.windows.clone(), f.cpu.clone()].into_iter().flatten().collect();
    let _ = writeln!(s, "{}", head.join(" · "));
    match &f.service {
        Ok(st) => {
            let mut parts = vec![format!("Service {}: running", st.version)];
            if let Some(sv) = &st.service {
                parts.push(if sv.elevated { "as administrator" } else { "without administrator rights" }.into());
                parts.push(match &sv.cpu_sensor_error {
                    None => "CPU temperature: yes".into(),
                    Some(e) => format!("CPU temperature: no ({e})"),
                });
            }
            let _ = writeln!(s, "{}", parts.join(", "));
        }
        Err(e) => {
            let _ = writeln!(s, "Service: {e}, so no firmware versions or device errors");
        }
    }

    let _ = writeln!(s, "\n**Devices camless drives**");
    match &f.service {
        Ok(st) if !st.devices.is_empty() => {
            for d in &st.devices {
                let slug = d.id.split('#').next().unwrap_or_default();
                let ids = MODELS.iter().find(|m| m.slug == slug).map(|m| format!(" `{:04x}:{:04x}`", m.vid, m.pid)).unwrap_or_default();
                let firmware = d.firmware.as_deref().unwrap_or("unknown");
                let tested = if d.verified { "verified" } else { "not verified yet" };
                let connected = if d.connected { "connected" } else { "not connected" };
                let _ = writeln!(s, "- {}{ids}: firmware {firmware}, {tested}, {connected}", d.name);
                if !d.fans.is_empty() {
                    let fans: Vec<String> = d
                        .fans
                        .iter()
                        .map(|c| {
                            let mut notes = Vec::new();
                            if c.pump {
                                notes.push("pump".to_string());
                            }
                            if c.min_duty > 0 {
                                notes.push(format!("min {} %", c.min_duty));
                            }
                            if c.device_curves {
                                notes.push("stores curves".into());
                            }
                            if c.connected == Some(false) {
                                notes.push("nothing plugged in".into());
                            }
                            if notes.is_empty() { c.id.clone() } else { format!("{} ({})", c.id, notes.join(", ")) }
                        })
                        .collect();
                    let _ = writeln!(s, "  - fans: {}", fans.join(", "));
                }
                if !d.lights.is_empty() {
                    let lights: Vec<String> = d
                        .lights
                        .iter()
                        .map(|l| {
                            let acc = if l.accessories.is_empty() { String::new() } else { format!(": {}", l.accessories.join(", ")) };
                            format!("{} ({} LEDs{acc})", l.id, l.leds)
                        })
                        .collect();
                    let _ = writeln!(s, "  - lights: {}", lights.join(", "));
                }
                if let Some(l) = &d.lcd {
                    let _ = writeln!(s, "  - LCD {}×{}{}", l.width, l.height, if l.round { ", round" } else { "" });
                }
                if let Some(e) = &d.error {
                    let _ = writeln!(s, "  - error: {e}");
                }
            }
        }
        _ if !f.found.is_empty() => {
            for (_, m) in &f.found {
                let _ = writeln!(s, "- {} `{:04x}:{:04x}`", m.name, m.vid, m.pid);
            }
        }
        _ => {
            let _ = writeln!(s, "None found.");
        }
    }

    let new = undriven(f);
    let _ = writeln!(s, "\n**NZXT devices without a driver**");
    let nzxt: Vec<_> = new.iter().filter(|d| d.0 == NZXT_VID).collect();
    if nzxt.is_empty() {
        let _ = writeln!(s, "None found.");
    }
    for (vid, pid, product) in nzxt {
        let _ = writeln!(s, "- {product} `{vid:04x}:{pid:04x}`");
    }
    let asetek: Vec<_> = new.iter().filter(|d| d.0 == ASETEK_VID).collect();
    if !asetek.is_empty() {
        let _ = writeln!(s, "\n**Asetek devices (Asetek made the older Krakens: X31, X41, X61, X40, X60)**");
        for (vid, pid, product) in asetek {
            let _ = writeln!(s, "- {product} `{vid:04x}:{pid:04x}`");
        }
    }

    let _ = writeln!(s, "\n**USB descriptors** ({READ_ONLY})\n```text");
    if let Err(e) = &f.usb {
        let _ = writeln!(s, "USB devices could not be listed: {e}");
    }
    if let Err(e) = &f.hid {
        let _ = writeln!(s, "HID devices could not be listed: {e}");
    }
    let usb = f.usb.as_deref().unwrap_or_default();
    let hid = f.hid.as_deref().unwrap_or_default();
    let mut keys: Vec<(u16, u16)> = usb.iter().map(|u| (u.vid, u.pid)).chain(hid.iter().map(|h| (h.vid, h.pid))).collect();
    keys.sort();
    keys.dedup();
    if keys.is_empty() {
        let _ = writeln!(s, "No NZXT or Asetek USB device.");
    }
    for (vid, pid) in keys {
        let u = usb.iter().find(|u| (u.vid, u.pid) == (vid, pid));
        let hids: Vec<&Hid> = hid.iter().filter(|h| (h.vid, h.pid) == (vid, pid)).collect();
        // The raw descriptor only for devices without a driver; the others are known.
        let raw = model(vid, pid).is_none();
        let product = u.and_then(|u| u.product.clone()).or_else(|| hids.first().and_then(|h| h.product.clone()));
        let _ = write!(s, "{vid:04x}:{pid:04x} {}", product.as_deref().unwrap_or("?"));
        let mut shown: Vec<(i32, u16, u16)> = Vec::new();
        match u {
            Some(u) => {
                let speed = match u.speed {
                    Some(nusb::Speed::Low) => ", low speed",
                    Some(nusb::Speed::Full) => ", full speed",
                    Some(nusb::Speed::High) => ", high speed",
                    Some(nusb::Speed::Super) => ", super speed",
                    Some(nusb::Speed::SuperPlus) => ", super speed+",
                    _ => "",
                };
                let count = if u.count > 1 { format!(", {} of them", u.count) } else { String::new() };
                let _ = writeln!(s, ", bcdDevice {:04x}{speed}{count}", u.bcd);
                match &u.interfaces {
                    Ok(list) => {
                        for i in list {
                            let alt = if i.alt == 0 { String::new() } else { format!(" alt {}", i.alt) };
                            let [c, sc, p] = i.class;
                            let eps: Vec<String> = i.endpoints.iter().map(endpoint).collect();
                            let eps = if eps.is_empty() { String::new() } else { format!(", {}", eps.join(", ")) };
                            let _ = writeln!(s, "  interface {}{alt}: class {c:02x}/{sc:02x}/{p:02x}{eps}", i.number);
                            if i.alt != 0 {
                                continue;
                            }
                            for h in hids.iter().filter(|h| h.interface == i32::from(i.number)) {
                                let _ = writeln!(s, "    HID usage {:04x}:{:04x}", h.usage_page, h.usage);
                                hid_lines(&mut s, h, raw, "      ");
                                shown.push((h.interface, h.usage_page, h.usage));
                            }
                        }
                    }
                    Err(e) => {
                        let _ = writeln!(s, "  descriptors: {e}");
                    }
                }
            }
            None => {
                let _ = writeln!(s);
            }
        }
        for h in hids.iter().filter(|h| !shown.contains(&(h.interface, h.usage_page, h.usage))) {
            let _ = writeln!(s, "  HID interface {}, usage {:04x}:{:04x}", h.interface, h.usage_page, h.usage);
            hid_lines(&mut s, h, raw, "    ");
        }
    }
    let _ = writeln!(s, "```");
    s
}

fn endpoint(e: &Endpoint) -> String {
    let kind = match e.kind {
        EndpointType::Control => "control",
        EndpointType::Isochronous => "isochronous",
        EndpointType::Bulk => "bulk",
        EndpointType::Interrupt => "interrupt",
    };
    let dir = if e.address & 0x80 != 0 { "in" } else { "out" };
    format!("{kind} {dir} {:02x} ({} B)", e.address, e.max_packet)
}

fn hid_lines(s: &mut String, h: &Hid, raw: bool, indent: &str) {
    match &h.descriptor {
        Ok(d) => {
            for line in report_lines(d) {
                let _ = writeln!(s, "{indent}{line}");
            }
            if raw {
                let hex: String = d.iter().map(|b| format!("{b:02x}")).collect();
                let _ = writeln!(s, "{indent}descriptor ({} B): {hex}", d.len());
            }
        }
        Err(e) => {
            let _ = writeln!(s, "{indent}descriptor: {e}");
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    In,
    Out,
    Feature,
}

/// Report ids and their lengths in bytes, without the id byte, in the order the
/// descriptor gives them. A descriptor without ids has one report per kind, id 0.
fn reports(desc: &[u8]) -> Vec<(Kind, u8, usize)> {
    let (mut id, mut size, mut count) = (0u8, 0u32, 0u32);
    let mut stack = Vec::new();
    let mut bits: Vec<(Kind, u8, u32)> = Vec::new();
    let mut i = 0;
    while i < desc.len() {
        let prefix = desc[i];
        if prefix == 0xfe {
            // A long item: its data size, its tag, then the data.
            i += 3 + usize::from(desc.get(i + 1).copied().unwrap_or(0));
            continue;
        }
        let len = [0, 1, 2, 4][usize::from(prefix & 3)];
        let Some(data) = desc.get(i + 1..i + 1 + len) else { break };
        let value = data.iter().rev().fold(0u32, |v, &b| v << 8 | u32::from(b));
        let kind = match prefix & 0xfc {
            0x80 => Some(Kind::In),
            0x90 => Some(Kind::Out),
            0xb0 => Some(Kind::Feature),
            0x74 => {
                size = value;
                None
            }
            0x94 => {
                count = value;
                None
            }
            0x84 => {
                id = value as u8;
                None
            }
            0xa4 => {
                stack.push((id, size, count));
                None
            }
            0xb4 => {
                (id, size, count) = stack.pop().unwrap_or((id, size, count));
                None
            }
            _ => None,
        };
        if let Some(k) = kind {
            match bits.iter_mut().find(|b| b.0 == k && b.1 == id) {
                Some(b) => b.2 += size * count,
                None => bits.push((k, id, size * count)),
            }
        }
        i += 1 + len;
    }
    bits.into_iter().map(|(k, id, b)| (k, id, b.div_ceil(8) as usize)).collect()
}

/// `in: 11 13 (511 B); 77 (63 B)`, one line per kind.
fn report_lines(desc: &[u8]) -> Vec<String> {
    let reports = reports(desc);
    [(Kind::In, "in"), (Kind::Out, "out"), (Kind::Feature, "feature")]
        .into_iter()
        .filter_map(|(kind, name)| {
            let mut groups: Vec<(usize, Vec<String>)> = Vec::new();
            for &(_, id, len) in reports.iter().filter(|r| r.0 == kind) {
                let id = if id == 0 { "no id".to_string() } else { format!("{id:02x}") };
                match groups.iter_mut().find(|g| g.0 == len) {
                    Some(g) => g.1.push(id),
                    None => groups.push((len, vec![id])),
                }
            }
            let groups: Vec<String> = groups.iter().map(|(len, ids)| format!("{} ({len} B)", ids.join(" "))).collect();
            (!groups.is_empty()).then(|| format!("{name}: {}", groups.join("; ")))
        })
        .collect()
}

/// Runs `scrub` over every string that came from Windows, a device or the service.
/// Descriptors and numbers are formatted here and stay as they are, so a short serial
/// number cannot garble them.
fn scrub_facts(f: &mut Facts) {
    let secrets = std::mem::take(&mut f.secrets);
    let clean = |t: &mut String| *t = scrub(t, &secrets);
    f.windows.iter_mut().chain(f.cpu.iter_mut()).for_each(clean);
    match &mut f.service {
        Ok(st) => {
            clean(&mut st.version);
            for d in &mut st.devices {
                clean(&mut d.name);
                d.firmware.iter_mut().chain(d.error.iter_mut()).for_each(clean);
                d.fans.iter_mut().for_each(|c| clean(&mut c.id));
                for l in &mut d.lights {
                    clean(&mut l.id);
                    l.accessories.iter_mut().for_each(clean);
                }
            }
            if let Some(sv) = &mut st.service {
                sv.cpu_sensor_error.iter_mut().for_each(clean);
            }
        }
        Err(e) => clean(e),
    }
    match &mut f.usb {
        Ok(list) => {
            for u in list {
                u.product.iter_mut().for_each(clean);
                if let Err(e) = &mut u.interfaces {
                    clean(e);
                }
            }
        }
        Err(e) => clean(e),
    }
    match &mut f.hid {
        Ok(list) => {
            for h in list {
                h.product.iter_mut().for_each(clean);
                if let Err(e) = &mut h.descriptor {
                    clean(e);
                }
            }
        }
        Err(e) => clean(e),
    }
}

/// Takes out what could identify the PC or its owner, wherever it got in: device
/// paths and user folders in error messages, serial numbers, user and PC names.
fn scrub(text: &str, secrets: &[String]) -> String {
    let mut secrets: Vec<&String> = secrets.iter().filter(|s| s.trim().len() >= 4).collect();
    // A user folder before the user name in it.
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut out = text.to_string();
    for s in secrets {
        out = replace_ci(&out, s, "<hidden>");
    }
    for marker in [r"\\?\", r"\\.\"] {
        out = mask(&out, marker, false, |c| c.is_whitespace() || "\"'(),:".contains(c), "<device path>");
    }
    // Windows user names cannot hold these, but can hold spaces.
    mask(&out, r"\users\", true, |c| "\\/\"':;|,<>[]=+*?\n".contains(c), "<user>")
}

/// `text` with every `needle` replaced, ignoring ASCII case.
fn replace_ci(text: &str, needle: &str, with: &str) -> String {
    let (hay, needle) = (text.to_ascii_lowercase(), needle.to_ascii_lowercase());
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (i, _) in hay.match_indices(&needle) {
        out.push_str(&text[last..i]);
        out.push_str(with);
        last = i + needle.len();
    }
    out.push_str(&text[last..]);
    out
}

/// `text` with what follows each `marker`, up to `end`, replaced; `marker` is lower case.
fn mask(text: &str, marker: &str, keep_marker: bool, end: fn(char) -> bool, with: &str) -> String {
    let hay = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    while let Some(found) = hay[last..].find(marker) {
        let start = last + found;
        let after = start + marker.len();
        out.push_str(&text[last..start]);
        if keep_marker {
            out.push_str(&text[start..after]);
        }
        out.push_str(with);
        last = text[after..].find(end).map_or(text.len(), |n| after + n);
    }
    out.push_str(&text[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use camless_core::status::{Device, FanChannel, LcdInfo, LightChannel, ServiceInfo};

    // Rebuilt by Windows from a Kraken Elite 2024 (firmware 1.2.0) and a Control Hub,
    // read with `get_report_descriptor`.
    const KRAKEN_ELITE_V2: &str = "0600ff0901a10185110901150026ff00750896ff01818285130901150026ff00750896ff01818285f50901150026ff00750896ff01818285ff0901150026ff00750896ff01818285710901150026ff00750896ff01818285750901150026ff00750896ff01818285310901150026ff00750896ff01818285330901150026ff00750896ff01818285370901150026ff00750896ff01818285390901150026ff00750896ff01818285e30901150026ff00750896ff01818285210901150026ff00750896ff01818285230901150026ff00750896ff01818285250901150026ff00750896ff01818285270901150026ff00750896ff01818285290901150026ff00750896ff018182852b0901150026ff00750896ff0181828577090a150026ff007508953f818285430902150026ff00750896ff01818285100901150026ff00750896ff01918285120901150026ff00750896ff01918285f20901150026ff00750896ff01918285f40901150026ff00750896ff01918285fe0901150026ff00750896ff01918285700901150026ff00750896ff01918285720901150026ff00750896ff01918285740901150026ff00750896ff01918285e20901150026ff00750896ff01918285300901150026ff00750896ff01918285320901150026ff00750896ff01918285360901150026ff00750896ff01918285380901150026ff00750896ff01918285200901150026ff00750896ff01918285220901150026ff00750896ff01918285240901150026ff00750896ff01918285260901150026ff00750896ff019182852a0901150026ff00750896ff01918285760909150026ff007508953f918285430902150026ff00750896ff01918285760909150026ff007508953fb182c0";
    const CONTROL_HUB: &str = "0600ff0901a10185110901150026ff00750896ff01818285130901150026ff00750896ff01818285ff0901150026ff00750896ff01818285210901150026ff00750896ff01818285230901150026ff00750896ff01818285250901150026ff00750896ff01818285270901150026ff00750896ff01818285290901150026ff00750896ff018182852b0901150026ff00750896ff01818285670901150026ff00750896ff01818285e30901150026ff00750896ff01818285f50901150026ff00750896ff01818285610901150026ff00750896ff018182852d0901150026ff00750896ff01818285100901150026ff00750896ff01918285120901150026ff00750896ff01918285200901150026ff00750896ff01918285220901150026ff00750896ff01918285240901150026ff00750896ff01918285260901150026ff00750896ff019182852a0901150026ff00750896ff01918285600901150026ff00750896ff01918285620901150026ff00750896ff01918285660901150026ff00750896ff01918285f20901150026ff00750896ff01918285fe0901150026ff00750896ff01918285e20901150026ff00750896ff01918285f40901150026ff00750896ff019182852c0901150026ff00750896ff019182c0";

    // Made up: a vendor page with 64 bytes in and out, and no Report ID items.
    const PLAIN: &str = "0600ff0901a101150026ff0075089540810295409102c0";

    fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn reads_report_ids_and_lengths() {
        assert_eq!(bytes(KRAKEN_ELITE_V2).len(), 645);
        assert_eq!(
            report_lines(&bytes(KRAKEN_ELITE_V2)),
            [
                "in: 11 13 f5 ff 71 75 31 33 37 39 e3 21 23 25 27 29 2b 43 (511 B); 77 (63 B)",
                "out: 10 12 f2 f4 fe 70 72 74 e2 30 32 36 38 20 22 24 26 2a 43 (511 B); 76 (63 B)",
                "feature: 76 (63 B)",
            ]
        );
        assert_eq!(
            report_lines(&bytes(CONTROL_HUB)),
            [
                "in: 11 13 ff 21 23 25 27 29 2b 67 e3 f5 61 2d (511 B)",
                "out: 10 12 20 22 24 26 2a 60 62 66 f2 fe e2 f4 2c (511 B)",
            ]
        );
    }

    #[test]
    fn a_descriptor_without_ids_has_one_report_per_kind() {
        // Vendor page, 64 bytes in and out, no Report ID items.
        let d = bytes(PLAIN);
        assert_eq!(report_lines(&d), ["in: no id (64 B)", "out: no id (64 B)"]);
        assert_eq!(reports(&d[..d.len() - 2]).len(), 1, "a cut descriptor gives what comes before the cut");
    }

    #[test]
    fn scrub_takes_out_serials_paths_and_names() {
        let secrets = vec![
            "A1B2C3D4E5F6A7B8C9D0E1F2".to_string(),
            r"C:\Users\Jan Kowalski".to_string(),
            "Jan Kowalski".to_string(),
            "DESKTOP-K7Q2".to_string(),
            "ab".to_string(),
        ];
        let text = r"opening \\?\hid#vid_1e71&pid_3012&mi_01#7&2f0a1b2c&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}: access denied
serial a1b2c3d4e5f6a7b8c9d0e1f2 on desktop-k7q2
reading C:\Users\Jan Kowalski\AppData\Roaming\camless\camless.toml
reading D:\users\Other Name\x.bin: missing
about the table";
        let out = scrub(text, &secrets);
        assert_eq!(
            out,
            r"opening <device path>: access denied
serial <hidden> on <hidden>
reading <hidden>\AppData\Roaming\camless\camless.toml
reading D:\users\<user>\x.bin: missing
about the table"
        );
    }

    fn facts() -> Facts {
        // An example, not a real machine: a Kraken X63 next to the hardware camless knows.
        let kraken = Device {
            id: "kraken-elite-v2".into(),
            name: "Kraken Elite (2024)".into(),
            firmware: Some("1.2.0".into()),
            verified: true,
            connected: true,
            fans: vec![
                FanChannel { id: "pump".into(), pump: true, min_duty: 20, device_curves: true, ..Default::default() },
                FanChannel { id: "fan".into(), device_curves: true, ..Default::default() },
            ],
            lights: vec![LightChannel { id: "ring".into(), leds: 24, ..Default::default() }],
            lcd: Some(LcdInfo { width: 640, height: 640, round: true }),
            error: Some(r"lost A1B2C3D4E5F6A7B8C9D0E1F2 at \\?\hid#vid_1e71&pid_3012#a1b2c3d4e5f6a7b8c9d0e1f2, C:\Users\Example\x.toml".into()),
            ..Default::default()
        };
        let status = Status {
            version: "0.3.0".into(),
            devices: vec![kraken],
            service: Some(ServiceInfo { pid: 1, elevated: true, ..Default::default() }),
            ..Default::default()
        };
        let hid = |pid, interface, hex: &str| Hid {
            vid: NZXT_VID,
            pid,
            product: None,
            interface,
            usage_page: 0xff00,
            usage: 1,
            descriptor: Ok(bytes(hex)),
        };
        let int = |number, class, eps: Vec<(u8, EndpointType, usize)>| Interface {
            number,
            alt: 0,
            class: [class, 0, 0],
            endpoints: eps.into_iter().map(|(address, kind, max_packet)| Endpoint { address, kind, max_packet }).collect(),
        };
        Facts {
            windows: Some("Windows 11 25H2, build 26200.9711".into()),
            cpu: Some("Example CPU".into()),
            service: Ok(status),
            found: Vec::new(),
            usb: Ok(vec![
                Usb {
                    vid: NZXT_VID,
                    pid: 0x2007,
                    count: 1,
                    product: Some("NZXT Kraken X".into()),
                    bcd: 0x0100,
                    speed: Some(nusb::Speed::Full),
                    interfaces: Ok(vec![int(0, 3, vec![(0x81, EndpointType::Interrupt, 64), (0x01, EndpointType::Interrupt, 64)])]),
                },
                Usb {
                    vid: NZXT_VID,
                    pid: 0x3012,
                    count: 1,
                    product: Some("NZXT Kraken Elite V2".into()),
                    bcd: 0x0102,
                    speed: Some(nusb::Speed::High),
                    interfaces: Ok(vec![
                        int(0, 0xff, vec![(0x02, EndpointType::Bulk, 512)]),
                        int(1, 3, vec![(0x81, EndpointType::Interrupt, 512), (0x01, EndpointType::Interrupt, 512)]),
                    ]),
                },
                Usb {
                    vid: ASETEK_VID,
                    pid: 0xb200,
                    count: 1,
                    product: Some("Asetek cooler".into()),
                    bcd: 0x0100,
                    speed: Some(nusb::Speed::Full),
                    interfaces: Err("no configuration descriptor".into()),
                },
            ]),
            hid: Ok(vec![hid(0x2007, 0, PLAIN), hid(0x3012, 1, KRAKEN_ELITE_V2)]),
            secrets: vec!["A1B2C3D4E5F6A7B8C9D0E1F2".into()],
        }
    }

    #[test]
    fn renders_known_and_new_devices() {
        let mut f = facts();
        scrub_facts(&mut f);
        let text = render(&f);
        assert!(!text.to_ascii_lowercase().contains("a1b2c3d4e5f6a7b8c9d0e1f2"));
        assert!(text.contains(r"  - error: lost <hidden> at <device path>, C:\Users\<user>\x.toml"));
        assert!(text.starts_with(&format!("camless {} · Windows 11 25H2, build 26200.9711 · Example CPU\n", env!("CARGO_PKG_VERSION"))));
        assert!(text.contains("Service 0.3.0: running, as administrator, CPU temperature: yes\n"));
        assert!(text.contains("- Kraken Elite (2024) `1e71:3012`: firmware 1.2.0, verified, connected\n"));
        assert!(text.contains("  - fans: pump (pump, min 20 %, stores curves), fan (stores curves)\n"));
        assert!(text.contains("  - LCD 640×640, round\n"));
        assert!(text.contains("**NZXT devices without a driver**\n- NZXT Kraken X `1e71:2007`\n"));
        assert!(text.contains("**Asetek devices (Asetek made the older Krakens: X31, X41, X61, X40, X60)**\n- Asetek cooler `2433:b200`\n"));
        assert!(text.contains(&format!("**USB descriptors** ({READ_ONLY})\n```text\n")));
        assert!(text.contains("1e71:3012 NZXT Kraken Elite V2, bcdDevice 0102, high speed\n  interface 0: class ff/00/00, bulk out 02 (512 B)\n"));
        assert!(text.contains("    HID usage ff00:0001\n      in: 11 13 f5"));
        assert!(text.contains("      in: no id (64 B)\n      out: no id (64 B)\n      descriptor (23 B): 0600ff09"));
        assert!(text.contains("2433:b200 Asetek cooler, bcdDevice 0100, full speed\n  descriptors: no configuration descriptor\n"));
        // Descriptors of supported devices are known; only new ones carry the raw bytes.
        assert_eq!(text.matches("descriptor (").count(), 1);
        assert!(text.ends_with("```\n"));
        assert_eq!(title(&f), "Hardware: NZXT Kraken X (1e71:2007), Asetek cooler (2433:b200)");
    }

    #[test]
    fn without_the_service_it_lists_what_it_found() {
        let mut f = facts();
        f.service = Err("not reachable at 127.0.0.1:30641 (nothing answers)".into());
        f.found = vec![("kraken-elite-v2".into(), &MODELS[0])];
        f.usb = Ok(Vec::new());
        f.hid = Ok(Vec::new());
        let text = render(&f);
        assert!(text.contains("Service: not reachable at 127.0.0.1:30641 (nothing answers), so no firmware versions or device errors\n"));
        assert!(text.contains("**Devices camless drives**\n- Kraken Elite (2024) `1e71:3012`\n"));
        assert!(text.contains("**NZXT devices without a driver**\nNone found.\n"));
        assert!(text.contains("No NZXT or Asetek USB device.\n"));
        assert_eq!(title(&f), "Hardware: Kraken Elite (2024)");
    }

    #[test]
    fn the_issue_template_says_what_the_report_reads() {
        let template = include_str!("../.github/ISSUE_TEMPLATE/hardware-report.yml");
        assert!(template.contains(READ_ONLY), "the template and the report must describe the same reads");
        assert!(template.contains("id: report"), "the app fills this field from the URL");
    }
}
