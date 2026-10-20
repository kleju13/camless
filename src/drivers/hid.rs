//! HID report I/O shared by the drivers.

use anyhow::{bail, Context, Result};
use hidapi::{HidApi, HidDevice};
use std::ffi::CStr;
use std::time::{Duration, Instant};

/// Input reports on NZXT devices are 64 bytes.
pub const REPORT_LEN: usize = 64;
/// The report descriptors declare 512-byte output reports.
const MAX_REPORT_LEN: usize = 512;
/// The devices answer in a few milliseconds; status broadcasts arrive in between.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(3);

/// Why a device does not open on Linux, where the device files belong to root: the
/// udev rules give them to the user at the desk.
#[cfg(target_os = "linux")]
pub const NO_ACCESS: &str =
    "cannot open the device; install.sh's udev rules give access to it (/etc/udev/rules.d/71-camless.rules), and after installing them, plug it in again";

/// Opens one HID interface and makes sure it can be read. An exclusive holder (NZXT
/// CAM) lets the open succeed without access rights, and only the first read fails.
pub fn open(api: &HidApi, path: &CStr) -> Result<HidDevice> {
    let dev = open_unread(api, path)?;
    let mut buf = [0u8; REPORT_LEN];
    dev.read_timeout(&mut buf, 0).context(HELD)?;
    Ok(dev)
}

/// Opens one HID interface without reading it: for a device with no input reports,
/// or a driver whose first read must see what the device sends.
pub fn open_unread(api: &HidApi, path: &CStr) -> Result<HidDevice> {
    #[cfg(windows)]
    return Ok(api.open_path(path)?);
    #[cfg(target_os = "linux")]
    return api.open_path(path).context(NO_ACCESS);
}

#[cfg(windows)]
pub const HELD: &str = "held by another program; close NZXT CAM first";
#[cfg(not(windows))]
pub const HELD: &str = "held by another program";

/// Reports are numbered and the report ID is the first command byte, so payloads go
/// out as they are, padded to at least 64 bytes like liquidctl does.
pub fn write(dev: &HidDevice, payload: &[u8]) -> Result<()> {
    if payload.len() > MAX_REPORT_LEN {
        bail!("report of {} bytes does not fit", payload.len());
    }
    let mut buf = vec![0u8; payload.len().max(REPORT_LEN)];
    buf[..payload.len()].copy_from_slice(payload);
    dev.write(&buf)?;
    Ok(())
}

/// Reads until a report starting with `prefix` arrives, skipping periodic broadcasts.
pub fn read_prefix(dev: &HidDevice, prefix: &[u8]) -> Result<Vec<u8>> {
    read_prefix_within(dev, prefix, REPLY_TIMEOUT)
}

pub fn read_prefix_within(dev: &HidDevice, prefix: &[u8], max: Duration) -> Result<Vec<u8>> {
    let mut buf = [0u8; REPORT_LEN];
    let end = Instant::now() + max;
    loop {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            bail!("no reply starting with {prefix:02x?} within {} ms", max.as_millis());
        }
        let n = dev.read_timeout(&mut buf, left.as_millis().max(1) as i32)?;
        if n >= prefix.len() && buf[..prefix.len()] == *prefix {
            return Ok(buf[..n].to_vec());
        }
    }
}

/// Sends a request and waits for its reply.
pub fn request(dev: &HidDevice, payload: &[u8], reply: &[u8]) -> Result<Vec<u8>> {
    write(dev, payload)?;
    read_prefix(dev, reply)
}

pub fn drain(dev: &HidDevice) {
    let mut buf = [0u8; REPORT_LEN];
    while matches!(dev.read_timeout(&mut buf, 0), Ok(n) if n > 0) {}
}

/// Every input report waiting now, without blocking.
pub fn pending(dev: &HidDevice) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0u8; REPORT_LEN];
    while let Ok(n) = dev.read_timeout(&mut buf, 0) {
        if n == 0 {
            break;
        }
        out.push(buf[..n].to_vec());
    }
    out
}

/// A kind of report in a HID report descriptor, which `camless report` prints and the
/// scan reads for other brands' hubs.
#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    In,
    Out,
    Feature,
}

/// Report ids and their lengths in bytes, without the id byte, in the order the
/// descriptor gives them. A descriptor without ids has one report per kind, id 0.
pub fn reports(desc: &[u8]) -> Vec<(Kind, u8, usize)> {
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

/// The report lengths of a HID collection as Windows counts them: the longest report
/// of each kind and its id byte, which Windows counts even without report ids. 0 for
/// none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Caps {
    pub input: usize,
    pub output: usize,
}

impl Caps {
    /// What Linux's sysfs report descriptor gives; Windows asks HID.dll.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn of_descriptor(desc: &[u8]) -> Caps {
        let reports = reports(desc);
        let longest = |kind| reports.iter().filter(|r| r.0 == kind).map(|r| r.2 + 1).max().unwrap_or(0);
        Caps { input: longest(Kind::In), output: longest(Kind::Out) }
    }
}
