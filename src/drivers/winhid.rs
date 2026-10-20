//! HID reports through Windows' own calls, for what hidapi's native Windows backend
//! lacks: reading an input report on request (`HidD_GetInputReport`). The Lian Li
//! hubs answer that way after a feature report asks them to. Windows takes reports
//! at the lengths the device declares, so shorter ones are padded with zeros.

use anyhow::{bail, Result};
use std::ffi::CStr;
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    HidD_FreePreparsedData, HidD_GetInputReport, HidD_GetPreparsedData, HidD_SetFeature, HidP_GetCaps, HIDP_CAPS,
    HIDP_STATUS_SUCCESS,
};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};

pub struct Hid {
    handle: HANDLE,
    input_len: usize,
    feature_len: usize,
}

impl Hid {
    /// Opens the collection at a hidapi path, shared, as hidapi does.
    pub fn open(_: &hidapi::HidApi, path: &CStr) -> Result<Hid> {
        let wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain([0]).collect();
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            bail!("cannot open the device (Windows error {})", unsafe { GetLastError() });
        }
        // Closed by `Drop` from here on, whatever fails next.
        let mut hid = Hid { handle, input_len: 0, feature_len: 0 };
        let mut data = 0;
        if !unsafe { HidD_GetPreparsedData(handle, &mut data) } {
            bail!("cannot read the device's report lengths (Windows error {})", unsafe { GetLastError() });
        }
        let mut caps: HIDP_CAPS = unsafe { std::mem::zeroed() };
        let status = unsafe { HidP_GetCaps(data, &mut caps) };
        unsafe { HidD_FreePreparsedData(data) };
        if status != HIDP_STATUS_SUCCESS {
            bail!("cannot read the device's report lengths (status {status:#x})");
        }
        hid.input_len = caps.InputReportByteLength.into();
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
