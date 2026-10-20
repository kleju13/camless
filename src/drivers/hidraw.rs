//! HID reports read on request on Linux, the `winhid` counterpart for the Lian Li
//! hubs. hidapi's hidraw backend has them (`HIDIOCGINPUT`, `HIDIOCSOUTPUT`), and
//! hidraw takes reports at the length they are, so the 7-byte commands go out
//! unpadded, as Nexus sends them on Linux (`LinuxHidDevice`).

use anyhow::{bail, Context, Result};
use hidapi::{HidApi, HidDevice};
use std::ffi::CStr;

pub struct Hid(HidDevice);

/// The report lengths of the interface at a hidraw path, from the report descriptor
/// Linux keeps in sysfs, which any user can read: this sends the device nothing.
/// One hidraw node is the whole interface, every collection of it.
pub fn caps(path: &CStr) -> Option<super::hid::Caps> {
    let node = std::path::Path::new(path.to_str().ok()?).file_name()?.to_owned();
    let sysfs = std::path::Path::new("/sys/class/hidraw").join(node).join("device/report_descriptor");
    Some(super::hid::Caps::of_descriptor(&std::fs::read(sysfs).ok()?))
}

impl Hid {
    pub fn open(api: &HidApi, path: &CStr) -> Result<Hid> {
        api.open_path(path).map(Hid).context(super::hid::NO_ACCESS)
    }

    /// Sends a feature report; byte 0 is the report id.
    pub fn set_feature(&self, report: &[u8]) -> Result<()> {
        self.0
            .send_feature_report(report)
            .with_context(|| format!("feature report {:02x?} failed", &report[..report.len().min(3)]))
    }

    /// Sends an output report over the control pipe (`SET_REPORT`); byte 0 is the
    /// report id.
    pub fn set_output(&self, report: &[u8]) -> Result<()> {
        self.0
            .send_output_report(report)
            .with_context(|| format!("output report {:02x?} failed", &report[..report.len().min(2)]))
    }

    /// Writes an output report to the interrupt pipe; byte 0 is the report id.
    pub fn write(&self, report: &[u8]) -> Result<()> {
        let n = self.0.write(report).with_context(|| format!("output report {:02x?} failed", &report[..report.len().min(2)]))?;
        if n != report.len() {
            bail!("output report {:02x?}: {n} of {} bytes went out", &report[..report.len().min(2)], report.len());
        }
        Ok(())
    }

    /// Reads input report `id` over the control pipe, `len` bytes with the id.
    pub fn input_report(&self, id: u8, len: usize) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; len];
        buf[0] = id;
        let n = self.0.get_input_report(&mut buf).with_context(|| format!("input report {id:#04x} failed"))?;
        if n == 0 {
            bail!("input report {id:#04x} came back empty");
        }
        // Zeros past what came back, as Windows hands over the declared length.
        buf[n.min(len)..].fill(0);
        Ok(buf)
    }
}
