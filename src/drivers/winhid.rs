//! HID reports through Windows' own calls, for what hidapi's native Windows backend
//! lacks: reading an input report on request (`HidD_GetInputReport`), and output
//! reports longer than the feature reports (hidapi cuts them to that length). The
//! Lian Li hubs answer that way after a feature report asks them to, and take their
//! colours in 353-byte output reports. Windows takes reports at the lengths the
//! device declares, so shorter ones are padded with zeros.

use anyhow::{bail, Result};
use std::ffi::CStr;
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    HidD_FreePreparsedData, HidD_GetInputReport, HidD_GetPreparsedData, HidD_SetFeature, HidD_SetOutputReport,
    HidP_GetCaps, HIDP_CAPS, HIDP_STATUS_SUCCESS,
};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, WriteFile, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};

pub struct Hid {
    handle: HANDLE,
    input_len: usize,
    output_len: usize,
    feature_len: usize,
}

fn create(path: &CStr, access: u32) -> HANDLE {
    let wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain([0]).collect();
    unsafe {
        CreateFileW(
            wide.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    }
}

/// The collection's report lengths, or why Windows does not give them.
fn read_caps(handle: HANDLE) -> std::result::Result<HIDP_CAPS, String> {
    let mut data = 0;
    if !unsafe { HidD_GetPreparsedData(handle, &mut data) } {
        return Err(format!("Windows error {}", unsafe { GetLastError() }));
    }
    let mut caps: HIDP_CAPS = unsafe { std::mem::zeroed() };
    let status = unsafe { HidP_GetCaps(data, &mut caps) };
    unsafe { HidD_FreePreparsedData(data) };
    if status != HIDP_STATUS_SUCCESS {
        return Err(format!("status {status:#x}"));
    }
    Ok(caps)
}

/// The report lengths of the collection at a hidapi path. Opened without access
/// rights, as hidapi lists devices: this sends the device nothing, and works while
/// another program holds it.
pub fn caps(path: &CStr) -> Option<super::hid::Caps> {
    let handle = create(path, 0);
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    let caps = read_caps(handle);
    unsafe { CloseHandle(handle) };
    let caps = caps.ok()?;
    Some(super::hid::Caps { input: caps.InputReportByteLength.into(), output: caps.OutputReportByteLength.into() })
}

impl Hid {
    /// Opens the collection at a hidapi path, shared, as hidapi does.
    pub fn open(_: &hidapi::HidApi, path: &CStr) -> Result<Hid> {
        let handle = create(path, GENERIC_READ | GENERIC_WRITE);
        if handle == INVALID_HANDLE_VALUE {
            bail!("cannot open the device (Windows error {})", unsafe { GetLastError() });
        }
        // Closed by `Drop` from here on, whatever fails next.
        let mut hid = Hid { handle, input_len: 0, output_len: 0, feature_len: 0 };
        let caps = read_caps(handle).map_err(|e| anyhow::anyhow!("cannot read the device's report lengths ({e})"))?;
        hid.input_len = caps.InputReportByteLength.into();
        hid.output_len = caps.OutputReportByteLength.into();
        hid.feature_len = caps.FeatureReportByteLength.into();
        Ok(hid)
    }

    /// Sends a feature report; byte 0 is the report id.
    pub fn set_feature(&self, report: &[u8]) -> Result<()> {
        let mut buf = report.to_vec();
        buf.resize(buf.len().max(self.feature_len), 0);
        if !unsafe { HidD_SetFeature(self.handle, buf.as_ptr().cast(), buf.len() as u32) } {
            bail!("feature report {:02x?} failed (Windows error {})", &report[..report.len().min(3)], unsafe {
                GetLastError()
            });
        }
        Ok(())
    }

    /// Sends an output report over the control pipe (`SET_REPORT`); byte 0 is the
    /// report id.
    pub fn set_output(&self, report: &[u8]) -> Result<()> {
        let buf = self.padded(report)?;
        if !unsafe { HidD_SetOutputReport(self.handle, buf.as_ptr().cast(), buf.len() as u32) } {
            bail!("output report {:02x?} failed (Windows error {})", &report[..report.len().min(2)], unsafe {
                GetLastError()
            });
        }
        Ok(())
    }

    /// Writes an output report to the interrupt pipe; byte 0 is the report id.
    pub fn write(&self, report: &[u8]) -> Result<()> {
        let buf = self.padded(report)?;
        let mut written = 0;
        let ok = unsafe { WriteFile(self.handle, buf.as_ptr(), buf.len() as u32, &mut written, std::ptr::null_mut()) };
        if ok == 0 || written as usize != buf.len() {
            bail!("output report {:02x?} failed (Windows error {})", &report[..report.len().min(2)], unsafe {
                GetLastError()
            });
        }
        Ok(())
    }

    /// An output report at the length the device declares.
    fn padded(&self, report: &[u8]) -> Result<Vec<u8>> {
        if report.len() > self.output_len {
            bail!("output report of {} bytes does not fit the device's {}", report.len(), self.output_len);
        }
        let mut buf = report.to_vec();
        buf.resize(self.output_len, 0);
        Ok(buf)
    }

    /// Reads input report `id` over the control pipe, at least `len` bytes with the id.
    pub fn input_report(&self, id: u8, len: usize) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; len.max(self.input_len)];
        buf[0] = id;
        if !unsafe { HidD_GetInputReport(self.handle, buf.as_mut_ptr().cast(), buf.len() as u32) } {
            bail!("input report {id:#04x} failed (Windows error {})", unsafe { GetLastError() });
        }
        Ok(buf)
    }
}

impl Drop for Hid {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle) };
    }
}
