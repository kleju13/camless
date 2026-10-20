//! Device drivers.
//!
//! Each supported model is one module with its protocol and a `Driver`. Adding a
//! device means adding a module and listing its `Model` in `MODELS`; the daemon,
//! the config and the GUI pick it up from `describe()`. See CONTRIBUTING.md.

pub mod control_hub;
pub mod demo;
pub mod asetek;
pub mod hid;
pub mod hue2;
pub mod kraken2;
pub mod kraken3;
pub mod kraken_elite_v2;
pub mod usb;

use anyhow::Result;
use camless_core::lighting::Lighting;
use camless_core::status::{Device, Unit, Unsupported};
use hidapi::HidApi;
use std::ffi::CString;

pub const NZXT_VID: u16 = 0x1e71;

/// Every model camless can drive.
pub const MODELS: &[Model] = &[
    kraken_elite_v2::MODEL,
    kraken_elite_v2::PLUS,
    kraken3::ELITE_2023,
    kraken3::KRAKEN_2023,
    kraken3::Z3,
    kraken3::X3,
    kraken3::X3_2014,
    kraken2::X2,
    kraken2::M22,
    asetek::KRAKEN_X,
    control_hub::MODEL,
];

pub type Open = fn(&HidApi, &Found) -> Result<Box<dyn Driver>>;

pub struct Model {
    pub vid: u16,
    pub pid: u16,
    /// The HID interface to open, for devices that have several.
    pub interface: Option<i32>,
    /// The device id in the config and the API; a second one gets `#2`.
    pub slug: &'static str,
    pub name: &'static str,
    pub bus: Bus,
    /// No other program drives this model the way camless does: Nexus leaves it out,
    /// and the driver follows liquidctl alone. Experimental until someone reports it
    /// working.
    pub experimental: bool,
    pub open: Open,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bus {
    Hid,
    /// A vendor-class USB device, reached through WinUSB.
    Usb,
}

impl Model {
    /// A HID device on any interface, not experimental.
    pub const fn hid(vid: u16, pid: u16, slug: &'static str, name: &'static str, open: Open) -> Model {
        Model { vid, pid, interface: None, slug, name, bus: Bus::Hid, experimental: false, open }
    }

    /// A vendor-class USB device, not experimental.
    pub const fn usb(vid: u16, pid: u16, slug: &'static str, name: &'static str, open: Open) -> Model {
        Model { bus: Bus::Usb, ..Model::hid(vid, pid, slug, name, open) }
    }

    /// Someone ran camless on it and reported it working: `hardware/verified.toml`.
    pub fn verified(&self) -> bool {
        camless_core::hardware::is_verified(self.vid, self.pid)
    }

    /// Experimental until it is verified.
    pub fn is_experimental(&self) -> bool {
        self.experimental && !self.verified()
    }
}

/// One supported device on the bus.
pub struct Found {
    pub model: &'static Model,
    pub id: String,
    /// The HID path, or for a USB device its bus and address.
    pub path: CString,
}

impl Found {
    pub fn open(&self, api: &HidApi) -> Result<Box<dyn Driver>> {
        (self.model.open)(api, self)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FanReading {
    pub rpm: Option<u16>,
    pub duty: Option<u8>,
    /// Something is plugged in, when the device can tell.
    pub connected: Option<bool>,
}

/// One poll: sensor values and fan readings, by the driver's own ids.
#[derive(Clone, Debug, Default)]
pub struct Reading {
    pub sensors: Vec<(&'static str, f32)>,
    pub fans: Vec<(&'static str, FanReading)>,
}

pub trait Driver {
    /// Channels and what they can do; `id` and the readings are filled in by the caller.
    fn describe(&self) -> Device;

    /// Sensors this device has: (id, name, unit). They appear as `<device>.<id>`.
    fn sensors(&self) -> Vec<(&'static str, &'static str, Unit)>;

    fn poll(&mut self) -> Result<Reading>;

    /// Host-driven duties, by fan id.
    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()>;

    /// Stores a curve the device runs by itself, on one of its own sensors (`None`
    /// for a fixed duty). Returns false if this fan or sensor can't do that, and the
    /// host drives the fan instead.
    fn store_curve(&mut self, _fan: &str, _sensor: Option<&str>, _duty_at: &dyn Fn(f32) -> u8) -> Result<bool> {
        Ok(false)
    }

    fn set_lighting(&mut self, light: &str, lighting: &Lighting) -> Result<()>;

    fn lcd(&mut self) -> Option<&mut dyn Lcd> {
        None
    }
}

pub trait Lcd {
    fn settings(&mut self, brightness: u8, quarter_turns: u8) -> Result<()>;
    /// The device's own screen (liquid temperature on a Kraken).
    fn show_builtin(&mut self) -> Result<()>;
    /// One Q565-encoded frame of `size()`.
    fn frame(&mut self, q565: &[u8]) -> Result<()>;
    fn is_streaming(&self) -> bool;
    fn size(&self) -> (u16, u16);
}

/// Windows lists each top-level collection of a HID interface as its own path, so
/// one device is one serial number and interface. Without a serial, the path.
fn device_key(d: &hidapi::DeviceInfo) -> String {
    match d.serial_number().filter(|s| !s.is_empty()) {
        Some(serial) => format!("{:04x}:{:04x}:{}:{serial}", d.vendor_id(), d.product_id(), d.interface_number()),
        None => d.path().to_string_lossy().to_ascii_lowercase(),
    }
}

/// Supported devices with their ids, and NZXT devices without a driver.
pub fn scan(api: &HidApi) -> (Vec<Found>, Vec<Unsupported>) {
    let mut entries: Vec<_> = api.device_list().filter(|d| d.vendor_id() == NZXT_VID).collect();
    entries.sort_by_key(|d| d.path().to_owned());
    let (mut found, mut unsupported, mut keys) = (Vec::<Found>::new(), Vec::<Unsupported>::new(), Vec::new());
    for d in entries {
        let key = device_key(d);
        if keys.contains(&key) {
            continue;
        }
        match MODELS.iter().find(|m| m.vid == d.vendor_id() && m.pid == d.product_id()) {
            Some(m) if m.interface.is_some_and(|i| i != d.interface_number()) => continue,
            Some(m) => {
                let n = found.iter().filter(|f| std::ptr::eq(f.model, m)).count();
                let id = if n == 0 { m.slug.to_string() } else { format!("{}#{}", m.slug, n + 1) };
                found.push(Found { model: m, id, path: d.path().to_owned() });
            }
            None if unsupported.iter().any(|u| u.pid == d.product_id()) => continue,
            None => unsupported.push(Unsupported {
                vid: d.vendor_id(),
                pid: d.product_id(),
                product: d.product_string().unwrap_or("?").to_string(),
            }),
        }
        keys.push(key);
    }
    let usb: Vec<_> = MODELS.iter().filter(|m| m.bus == Bus::Usb).collect();
    if !usb.is_empty() {
        for d in nusb::list_devices().into_iter().flatten() {
            let Some(m) = usb.iter().find(|m| m.vid == d.vendor_id() && m.pid == d.product_id()) else { continue };
            let n = found.iter().filter(|f| std::ptr::eq(f.model, *m)).count();
            let id = if n == 0 { m.slug.to_string() } else { format!("{}#{}", m.slug, n + 1) };
            let path = CString::new(usb::location(&d)).unwrap_or_default();
            found.push(Found { model: m, id, path });
        }
    }
    (found, unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn experimental_models_are_the_ones_nexus_leaves_out() {
        let experimental: Vec<_> = MODELS.iter().filter(|m| m.experimental).map(|m| m.slug).collect();
        assert_eq!(experimental, ["kraken-x2", "kraken-m22", "kraken-x-asetek"]);
        assert!(MODELS.iter().filter(|m| m.bus == Bus::Usb).all(|m| m.interface.is_none()));
    }

    #[test]
    fn slugs_are_unique() {
        for (i, m) in MODELS.iter().enumerate() {
            assert!(MODELS[..i].iter().all(|o| o.slug != m.slug && (o.vid, o.pid) != (m.vid, m.pid)));
            assert!(m.slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        }
    }
}
