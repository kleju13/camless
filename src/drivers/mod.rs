//! Device drivers.
//!
//! Each supported model is one module with its protocol and a `Driver`. Adding a
//! device means adding a module and listing its `Model` in `MODELS`; the daemon,
//! the config and the GUI pick it up from `describe()`. See CONTRIBUTING.md.
//!
//! Other brands' fan and lighting hubs are listed here too, with their `Vendor`.
//! camless opens them only with `[devices] other_brands` on, and only on their
//! vendor-defined HID collection or their serial port: never a keyboard, mouse or
//! other input device.

pub mod control_hub;
pub mod corsair_link;
pub mod demo;
pub mod asetek;
pub mod hid;
#[cfg(target_os = "linux")]
pub mod hidraw;
pub mod hue2;
pub mod hyte;
pub mod hyte_np50;
pub mod ibuypower_minihub;
pub mod kraken2;
pub mod kraken3;
pub mod kraken_elite_v2;
pub mod lianli;
pub mod lianli_galahad2;
pub mod lianli_tl;
pub mod nollie;
pub mod serial;
pub mod strimer;
pub mod umbra;
pub mod usb;
pub mod vendor;
#[cfg(windows)]
pub mod winhid;

use anyhow::Result;
use camless_core::lighting::Lighting;
use camless_core::status::{Device, Unit, Unsupported};
use hidapi::HidApi;
use std::ffi::CString;
pub use vendor::Vendor;

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
    lianli::UNI_HUB,
    lianli::UNI_HUB_SL,
    lianli::UNI_HUB_AL,
    lianli::UNI_HUB_SL_INFINITY,
    lianli::UNI_HUB_SL_V2,
    lianli::UNI_HUB_AL_V2,
    lianli::UNI_HUB_SL_V2_A105,
    lianli::UNI_HUB_SL_REDRAGON,
    lianli_tl::MODEL,
    lianli_galahad2::TRINITY,
    lianli_galahad2::PERFORMANCE,
    lianli_galahad2::LCD,
    strimer::MODEL,
    umbra::MODEL,
    corsair_link::MODEL,
    nollie::N32,
    nollie::N16,
    nollie::N8,
    nollie::N1,
    nollie::N28_12,
    nollie::N28_L1,
    nollie::N28_L2,
    nollie::N32_OS2,
    nollie::N16_OS2,
    nollie::N8_OS2,
    nollie::N1_OS2,
    nollie::N32_OS2_1,
    nollie::N16_OS2_1,
    nollie::N8_OS2_1,
    nollie::PRISM8,
    nollie::N1_OS2_1,
    hyte::SMART_HUB,
    hyte_np50::NP50,
    ibuypower_minihub::MINIHUB,
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
    /// No other program drives this model the way camless does: Nexus leaves it out
    /// and the driver follows liquidctl alone, or Nexus calls its own support
    /// experimental. Experimental until someone reports it working.
    pub experimental: bool,
    /// Another brand's device, opened only with `[devices] other_brands` on.
    pub vendor: Option<&'static Vendor>,
    /// The HID collection to open. Another brand's HID model needs one on a
    /// vendor-defined page, so no input device of that brand matches.
    pub usage: Option<Usage>,
    pub open: Open,
}

/// Which HID collection of a device a driver opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Usage {
    /// This (usage page, usage).
    Exact(u16, u16),
    /// Any collection on a vendor-defined page with output reports of at least
    /// `output` bytes, as Windows counts them (`Caps`), and with `input`, input
    /// reports too: what the driver writes and reads. For devices whose usage no
    /// source names.
    Vendor { output: usize, input: bool },
    /// Any collection on a vendor-defined page whose output reports are exactly this
    /// long, as Windows counts them: for a device camless only writes, whose usage no
    /// source names.
    VendorOutput(usize),
}

impl Usage {
    /// Whether the collection `usage` with its report lengths, read only if needed,
    /// is this one. Never one off the vendor-defined pages.
    fn matches(self, usage: (u16, u16), caps: &dyn Fn() -> Option<hid::Caps>) -> bool {
        usage.0 >= VENDOR_USAGE_PAGES
            && match self {
                Usage::Exact(page, id) => usage == (page, id),
                Usage::Vendor { output, input } => {
                    caps().is_some_and(|c| c.output >= output && (!input || c.input > 0))
                }
                Usage::VendorOutput(output) => caps().is_some_and(|c| c.output == output),
            }
    }
}

/// HID usage pages from here up are vendor-defined: not a keyboard, mouse or game
/// controller.
const VENDOR_USAGE_PAGES: u16 = 0xff00;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bus {
    Hid,
    /// A vendor-class USB device, reached through WinUSB.
    Usb,
    /// A USB serial port (CDC ACM): COM on Windows, `/dev/ttyACM*` on Linux.
    Serial,
}

impl Model {
    /// A HID device on any interface, not experimental.
    pub const fn hid(vid: u16, pid: u16, slug: &'static str, name: &'static str, open: Open) -> Model {
        Model { vid, pid, interface: None, slug, name, bus: Bus::Hid, experimental: false, vendor: None, usage: None, open }
    }

    /// A vendor-class USB device, not experimental.
    pub const fn usb(vid: u16, pid: u16, slug: &'static str, name: &'static str, open: Open) -> Model {
        Model { bus: Bus::Usb, ..Model::hid(vid, pid, slug, name, open) }
    }

    /// Another brand's HID hub on the collection `usage`: experimental.
    pub const fn vendor_hid(
        vid: u16,
        pid: u16,
        slug: &'static str,
        name: &'static str,
        vendor: &'static Vendor,
        usage: Usage,
        open: Open,
    ) -> Model {
        Model { experimental: true, vendor: Some(vendor), usage: Some(usage), ..Model::hid(vid, pid, slug, name, open) }
    }

    /// Another brand's hub on a USB serial port: experimental.
    pub const fn vendor_serial(
        vid: u16,
        pid: u16,
        slug: &'static str,
        name: &'static str,
        vendor: &'static Vendor,
        open: Open,
    ) -> Model {
        Model { bus: Bus::Serial, experimental: true, vendor: Some(vendor), ..Model::hid(vid, pid, slug, name, open) }
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
    /// The HID path, for a USB device its bus and address, for a serial port its
    /// device (`\\.\COM3`, `/dev/ttyACM0`).
    pub path: CString,
}

impl Found {
    /// Opens the device, unless it is another brand's and that brand's app runs.
    pub fn open(&self, api: &HidApi) -> Result<Box<dyn Driver>> {
        if let Some(why) = self.model.vendor.and_then(|v| v.busy(&vendor::running_processes())) {
            anyhow::bail!(why);
        }
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

    /// Hands the fans camless drives back to the device's own control, such as a
    /// hub's motherboard PWM input, before camless closes it for another app or for
    /// good. Most devices keep their last duty instead.
    fn release(&mut self) -> Result<()> {
        Ok(())
    }

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

/// A channel id made at run time, such as a fan found on a chain, for `Reading`,
/// which takes `&'static str`. Each distinct id is kept once, for good.
pub fn static_id(id: &str) -> &'static str {
    static IDS: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    let mut ids = IDS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(known) = ids.iter().find(|known| **known == id) {
        return known;
    }
    let new: &'static str = Box::leak(id.to_owned().into_boxed_str());
    ids.push(new);
    new
}

/// Windows lists each top-level collection of a HID interface as its own path, so
/// one device is one serial number and interface. Without a serial, the path.
fn device_key(d: &hidapi::DeviceInfo) -> String {
    match d.serial_number().filter(|s| !s.is_empty()) {
        Some(serial) => format!("{:04x}:{:04x}:{}:{serial}", d.vendor_id(), d.product_id(), d.interface_number()),
        None => d.path().to_string_lossy().to_ascii_lowercase(),
    }
}

/// The model of a HID collection. Another brand's model matches only with
/// `other_brands` on, and only its own collection on a vendor-defined usage page;
/// `caps` reads the collection's report lengths, for the models that need them.
fn hid_model(
    vid: u16,
    pid: u16,
    usage: (u16, u16),
    other_brands: bool,
    caps: &dyn Fn() -> Option<hid::Caps>,
) -> Option<&'static Model> {
    MODELS.iter().find(|m| {
        m.bus == Bus::Hid
            && (m.vid, m.pid) == (vid, pid)
            && (m.vendor.is_none() || other_brands && m.usage.is_some_and(|u| u.matches(usage, caps)))
    })
}

/// The report lengths of the collection at a hidapi path.
fn caps(path: &std::ffi::CStr) -> Option<hid::Caps> {
    #[cfg(windows)]
    return winhid::caps(path);
    #[cfg(target_os = "linux")]
    return hidraw::caps(path);
}

/// The id of the next device of a model: its slug, then `#2`, `#3`. Models that
/// share a slug count together.
fn next_id(found: &[Found], m: &Model) -> String {
    let n = found.iter().filter(|f| f.model.slug == m.slug).count();
    if n == 0 {
        m.slug.to_string()
    } else {
        format!("{}#{}", m.slug, n + 1)
    }
}

/// Supported devices with their ids, and NZXT devices without a driver. Other
/// brands' devices are looked for only with `other_brands` on.
pub fn scan(api: &HidApi, other_brands: bool) -> (Vec<Found>, Vec<Unsupported>) {
    let mut entries: Vec<_> = api
        .device_list()
        .map(|d| {
            let usage = (d.usage_page(), d.usage());
            (d, hid_model(d.vendor_id(), d.product_id(), usage, other_brands, &|| caps(d.path())))
        })
        .filter(|(d, m)| d.vendor_id() == NZXT_VID || m.is_some())
        .collect();
    entries.sort_by_key(|(d, _)| d.path().to_owned());
    let (mut found, mut unsupported, mut keys) = (Vec::<Found>::new(), Vec::<Unsupported>::new(), Vec::new());
    for (d, model) in entries {
        let key = device_key(d);
        if keys.contains(&key) {
            continue;
        }
        match model {
            Some(m) if m.interface.is_some_and(|i| i != d.interface_number()) => continue,
            Some(m) => {
                let id = next_id(&found, m);
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
    let usb: Vec<_> = MODELS.iter().filter(|m| m.bus == Bus::Usb && (m.vendor.is_none() || other_brands)).collect();
    if !usb.is_empty() {
        for d in nusb::list_devices().into_iter().flatten() {
            let Some(m) = usb.iter().find(|m| m.vid == d.vendor_id() && m.pid == d.product_id()) else { continue };
            let id = next_id(&found, m);
            let path = CString::new(usb::location(&d)).unwrap_or_default();
            found.push(Found { model: m, id, path });
        }
    }
    let serial: Vec<_> = MODELS.iter().filter(|m| m.bus == Bus::Serial && other_brands).collect();
    if !serial.is_empty() {
        let mut ports = serial::ports();
        ports.sort_by(|a, b| a.path.cmp(&b.path));
        for p in ports {
            let Some(m) = serial.iter().find(|m| (m.vid, m.pid) == (p.vid, p.pid)) else { continue };
            let id = next_id(&found, m);
            let Ok(path) = CString::new(p.path) else { continue };
            found.push(Found { model: m, id, path });
        }
    }
    (found, unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn experimental_models_are_the_ones_nexus_leaves_out_or_calls_experimental() {
        let nzxt: Vec<_> = MODELS.iter().filter(|m| m.experimental && m.vendor.is_none()).map(|m| m.slug).collect();
        assert_eq!(nzxt, ["kraken-x2", "kraken-m22", "kraken-x-asetek"]);
        // Nexus badges its Lian Li support experimental.
        assert!(MODELS.iter().filter(|m| m.vendor.is_some()).all(|m| m.experimental));
        assert!(MODELS.iter().filter(|m| m.bus == Bus::Usb).all(|m| m.interface.is_none()));
    }

    /// linux/71-camless.rules gives the user every model's hidraw nodes and USB
    /// device, and nothing else: no keyboard or mouse of the same brands.
    #[test]
    fn the_udev_rules_name_every_model_and_nothing_else() {
        use std::collections::BTreeSet;
        let (mut hidraw, mut usb, mut tty) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        let rules = include_str!("../../linux/71-camless.rules");
        for line in rules.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            assert!(line.ends_with(r#"TAG+="uaccess""#), "{line}");
            let value = |key: &str| {
                let rest = line.split(&format!("{key}==\"")).nth(1).unwrap_or_else(|| panic!("no {key} in {line}"));
                rest.split('"').next().unwrap()
            };
            let (set, attr) = match value("SUBSYSTEM") {
                "hidraw" => (&mut hidraw, "ATTRS"),
                "usb" => (&mut usb, "ATTR"),
                // ModemManager would probe a serial hub with modem commands.
                "tty" if line.contains(r#"ENV{ID_MM_DEVICE_IGNORE}="1""#) => (&mut tty, "ATTRS"),
                other => panic!("subsystem {other} in {line}"),
            };
            let vid = u16::from_str_radix(value(&format!("{attr}{{idVendor}}")), 16).unwrap();
            for pid in value(&format!("{attr}{{idProduct}}")).split('|') {
                set.insert((vid, u16::from_str_radix(pid, 16).unwrap()));
            }
        }
        let ids = |bus: Option<Bus>| -> BTreeSet<_> { MODELS.iter().filter(|m| bus.is_none_or(|b| m.bus == b)).map(|m| (m.vid, m.pid)).collect() };
        assert_eq!(hidraw, ids(Some(Bus::Hid)));
        assert_eq!(tty, ids(Some(Bus::Serial)));
        assert_eq!(usb, ids(None));
    }

    #[test]
    fn slugs_are_unique() {
        for (i, m) in MODELS.iter().enumerate() {
            assert!(MODELS[..i].iter().all(|o| (o.vid, o.pid) != (m.vid, m.pid)));
            // Two ids of one product, as Lian Li's SL v2 hub has, share the slug.
            assert!(MODELS[..i].iter().all(|o| o.slug != m.slug || o.name == m.name), "{}", m.slug);
            assert!(m.slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        }
    }

    #[test]
    fn other_brands_open_only_their_vendor_collection() {
        let none = || None;
        for m in MODELS.iter().filter(|m| m.vendor.is_some()) {
            let caps = match m.usage {
                Some(Usage::VendorOutput(output)) => hid::Caps { input: 0, output },
                _ => hid::Caps { input: 1025, output: 1025 },
            };
            let fits = move || Some(caps);
            assert_ne!(m.vid, NZXT_VID);
            if m.bus == Bus::Serial {
                assert!(m.usage.is_none() && hid_model(m.vid, m.pid, (0xff00, 1), true, &fits).is_none());
                continue;
            }
            assert_eq!(m.bus, Bus::Hid, "{}", m.slug);
            let usage = match m.usage.expect("another brand's HID model names its collection") {
                Usage::Exact(page, usage) => (page, usage),
                Usage::Vendor { .. } | Usage::VendorOutput(_) => (0xff00, 1),
            };
            assert!(usage.0 >= VENDOR_USAGE_PAGES);
            assert!(hid_model(m.vid, m.pid, usage, true, &fits).is_some());
            assert!(hid_model(m.vid, m.pid, usage, false, &fits).is_none());
            // The same device's keyboard, mouse or consumer-control collections.
            for input in [(0x01, 0x06), (0x01, 0x02), (0x0c, 0x01), (0xfeff, 1)] {
                assert!(hid_model(m.vid, m.pid, input, true, &fits).is_none());
            }
            match m.usage {
                Some(Usage::Exact(page, usage)) => {
                    assert!(hid_model(m.vid, m.pid, (page, usage + 1), true, &fits).is_none())
                }
                // Without its report lengths, or with too short ones, it is not the one.
                _ => {
                    assert!(hid_model(m.vid, m.pid, usage, true, &none).is_none());
                    let short = || Some(hid::Caps { input: 0, output: 8 });
                    assert!(hid_model(m.vid, m.pid, usage, true, &short).is_none());
                }
            }
        }
        // NZXT's own devices need neither the setting nor a usage.
        assert!(hid_model(NZXT_VID, 0x2022, (0x01, 0x06), false, &none).is_some());
    }
}
