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

/// Opens one HID interface and makes sure it can be read. An exclusive holder (NZXT
/// CAM) lets the open succeed without access rights, and only the first read fails.
pub fn open(api: &HidApi, path: &CStr) -> Result<HidDevice> {
    let dev = api.open_path(path)?;
    let mut buf = [0u8; REPORT_LEN];
    dev.read_timeout(&mut buf, 0).context("held by another program; close NZXT CAM first")?;
    Ok(dev)
}

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
