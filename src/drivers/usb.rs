//! Transfers to USB devices that are not HID, through nusb and WinUSB, with timeouts.
//!
//! nusb has no timeout on bulk transfers and no async runtime runs here, so the
//! calling thread waits on the transfer itself; dropping a transfer cancels it.

use anyhow::{anyhow, bail, Context as _, Result};
use nusb::transfer::{Completion, Control, ControlType, Recipient, RequestBuffer};
use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

/// Wakes the thread that waits for a transfer.
struct Unpark(std::thread::Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// The transfer's completion, or `None` if it took longer than `max`.
pub fn wait<T>(transfer: impl Future<Output = Completion<T>>, max: Duration) -> Option<Completion<T>> {
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut transfer = pin!(transfer);
    let end = Instant::now() + max;
    loop {
        if let Poll::Ready(done) = transfer.as_mut().poll(&mut cx) {
            return Some(done);
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        std::thread::park_timeout(left);
    }
}

pub fn bulk_out(usb: &nusb::Interface, endpoint: u8, data: Vec<u8>, max: Duration) -> Result<()> {
    match wait(usb.bulk_out(endpoint, data), max) {
        Some(done) => done.status.map_err(|e| anyhow!("bulk transfer failed: {e}")),
        None => bail!("the device took no data within {} ms", max.as_millis()),
    }
}

pub fn bulk_in(usb: &nusb::Interface, endpoint: u8, len: usize, max: Duration) -> Result<Vec<u8>> {
    match wait(usb.bulk_in(endpoint, RequestBuffer::new(len)), max) {
        Some(done) => done.into_result().map_err(|e| anyhow!("bulk read failed: {e}")),
        None => bail!("no reply within {} ms", max.as_millis()),
    }
}

/// A vendor request to the device with no data stage.
pub fn vendor_out(usb: &nusb::Interface, request: u8, value: u16, max: Duration) -> Result<()> {
    let control = Control { control_type: ControlType::Vendor, recipient: Recipient::Device, request, value, index: 0 };
    usb.control_out_blocking(control, &[], max).map(|_| ()).map_err(|e| anyhow!("control request {request:#04x} failed: {e}"))
}

/// Tells two devices of the same model apart, the way `Found::path` does for HID.
pub fn location(d: &nusb::DeviceInfo) -> String {
    #[cfg(windows)]
    return d.instance_id().to_string_lossy().into_owned();
    #[cfg(not(windows))]
    return format!("{}-{}", d.bus_number(), d.device_address());
}

#[cfg(windows)]
pub const OPEN_FAILED: &str = "cannot open the USB device";
/// On Linux the device files belong to root, and the udev rules give them to the user.
#[cfg(not(windows))]
pub const OPEN_FAILED: &str =
    "cannot open the USB device; install.sh's udev rules give access to it (/etc/udev/rules.d/71-camless.rules), and after installing them, plug it in again";

/// Opens the device at `location` and claims its first interface. On Windows that
/// works only with WinUSB bound to it, which the error says.
pub fn open(vid: u16, pid: u16, location: &str) -> Result<nusb::Interface> {
    let info = nusb::list_devices()?
        .find(|d| d.vendor_id() == vid && d.product_id() == pid && self::location(d) == location)
        .context("the USB device is gone")?;
    #[cfg(windows)]
    if let Some(driver) = info.driver().filter(|d| !d.eq_ignore_ascii_case("winusb")) {
        bail!("Windows has the {driver} driver on it; camless needs WinUSB there (Zadig installs it)");
    }
    let dev = info.open().context(OPEN_FAILED)?;
    dev.claim_interface(0).context("cannot claim the USB interface (does another program have it?)")
}
