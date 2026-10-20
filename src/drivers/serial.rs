//! USB serial ports (CDC ACM), for the hubs that take their commands over one: HYTE's
//! and iBUYPOWER's. 115200 baud, 8 data bits, no parity, one stop bit, DTR and RTS
//! on, as Nexus opens them (`Np50SerialTransport`).
//!
//! `ports()` finds them by their USB ids: on Windows the Ports device class through
//! SetupAPI and each port's `PortName`, as Nexus's `WindowsSmartHubPortDiscovery`
//! does; on Linux `/sys/class/tty`, as its `LinuxSerialDiscovery` does. Neither opens
//! a port.

use anyhow::{bail, Result};
use std::ffi::CStr;
use std::time::{Duration, Instant};

#[cfg(windows)]
const BAUD: u32 = 115_200;
/// A write that takes longer is a port that is gone.
const WRITE_TIMEOUT: Duration = Duration::from_millis(500);

/// A USB serial port on the system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Port {
    pub vid: u16,
    pub pid: u16,
    /// What `Serial::open` takes: `\\.\COM3`, `/dev/ttyACM0`.
    pub path: String,
}

/// `VID_3402&PID_0904` in a Windows hardware id, such as `USB\VID_3402&PID_0904&REV_0100`.
#[cfg_attr(not(windows), allow(dead_code))]
fn ids_in_hardware_id(id: &str) -> Option<(u16, u16)> {
    let id = id.to_ascii_uppercase();
    let hex = |key: &str| u16::from_str_radix(id.split(key).nth(1)?.get(..4)?, 16).ok();
    Some((hex("VID_")?, hex("PID_")?))
}

#[cfg(windows)]
pub use self::windows::{ports, Serial};
#[cfg(target_os = "linux")]
pub use self::linux::{ports, Serial};

#[cfg(windows)]
mod windows {
    use super::*;
    use windows_sys::Win32::Devices::Communication::{
        GetCommState, PurgeComm, SetCommState, SetCommTimeouts, COMMTIMEOUTS, DCB, NOPARITY, ONESTOPBIT,
        PURGE_RXCLEAR, PURGE_TXCLEAR,
    };
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDeviceRegistryPropertyW,
        SetupDiOpenDevRegKey, DICS_FLAG_GLOBAL, DIGCF_PRESENT, DIREG_DEV, GUID_DEVCLASS_PORTS, SPDRP_HARDWAREID,
        SP_DEVINFO_DATA,
    };
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, ReadFile, WriteFile, OPEN_EXISTING};
    use windows_sys::Win32::System::Registry::{RegCloseKey, RegQueryValueExW, KEY_READ};

    /// The serial ports Windows lists in its Ports class, with their USB ids.
    pub fn ports() -> Vec<Port> {
        let mut out = Vec::new();
        let set = unsafe { SetupDiGetClassDevsW(&GUID_DEVCLASS_PORTS, std::ptr::null(), std::ptr::null_mut(), DIGCF_PRESENT) };
        if set as isize == -1 {
            return out;
        }
        for i in 0.. {
            let mut dev: SP_DEVINFO_DATA = unsafe { std::mem::zeroed() };
            dev.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
            if unsafe { SetupDiEnumDeviceInfo(set, i, &mut dev) } == 0 {
                break;
            }
            // REG_MULTI_SZ: the first string is the most specific id.
            let mut buf = [0u16; 512];
            let ok = unsafe {
                SetupDiGetDeviceRegistryPropertyW(
                    set,
                    &dev,
                    SPDRP_HARDWAREID,
                    std::ptr::null_mut(),
                    buf.as_mut_ptr().cast(),
                    std::mem::size_of_val(&buf) as u32,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                continue;
            }
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            let Some((vid, pid)) = ids_in_hardware_id(&String::from_utf16_lossy(&buf[..end])) else { continue };
            let key = unsafe { SetupDiOpenDevRegKey(set, &dev, DICS_FLAG_GLOBAL, 0, DIREG_DEV, KEY_READ) };
            if key.is_null() || key as isize == -1 {
                continue;
            }
            let mut name = [0u16; 64];
            let mut len = std::mem::size_of_val(&name) as u32;
            let value: Vec<u16> = "PortName".encode_utf16().chain([0]).collect();
            let status = unsafe {
                RegQueryValueExW(key, value.as_ptr(), std::ptr::null(), std::ptr::null_mut(), name.as_mut_ptr().cast(), &mut len)
            };
            unsafe { RegCloseKey(key) };
            if status != 0 {
                continue;
            }
            let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
            let name = String::from_utf16_lossy(&name[..end]);
            if name.starts_with("COM") {
                out.push(Port { vid, pid, path: format!(r"\\.\{name}") });
            }
        }
        unsafe { SetupDiDestroyDeviceInfoList(set) };
        out
    }

    pub struct Serial {
        handle: HANDLE,
    }

    impl Serial {
        /// Opens the port for this program alone, as serial ports open on Windows.
        pub fn open(path: &CStr) -> Result<Serial> {
            let wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain([0]).collect();
            let handle = unsafe {
                CreateFileW(wide.as_ptr(), GENERIC_READ | GENERIC_WRITE, 0, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut())
            };
            if handle == INVALID_HANDLE_VALUE {
                let e = unsafe { GetLastError() };
                if e == ERROR_ACCESS_DENIED {
                    bail!("the port is held by another program, such as the hub's own app");
                }
                bail!("cannot open the port (Windows error {e})");
            }
            let port = Serial { handle };
            let mut dcb: DCB = unsafe { std::mem::zeroed() };
            dcb.DCBlength = std::mem::size_of::<DCB>() as u32;
            if unsafe { GetCommState(handle, &mut dcb) } == 0 {
                bail!("cannot read the port's settings (Windows error {})", unsafe { GetLastError() });
            }
            dcb.BaudRate = BAUD;
            dcb.ByteSize = 8;
            dcb.Parity = NOPARITY;
            dcb.StopBits = ONESTOPBIT;
            // fBinary, DTR_CONTROL_ENABLE, RTS_CONTROL_ENABLE; no flow control, no
            // abort on error.
            dcb._bitfield = 1 | 1 << 4 | 1 << 12;
            if unsafe { SetCommState(handle, &dcb) } == 0 {
                bail!("cannot set the port to 115200 baud (Windows error {})", unsafe { GetLastError() });
            }
            unsafe { PurgeComm(handle, PURGE_RXCLEAR | PURGE_TXCLEAR) };
            Ok(port)
        }

        fn timeouts(&self, read: Duration) -> Result<()> {
            // A read returns at once with what has arrived, or waits up to `read`
            // for the first byte.
            let t = COMMTIMEOUTS {
                ReadIntervalTimeout: u32::MAX,
                ReadTotalTimeoutMultiplier: u32::MAX,
                ReadTotalTimeoutConstant: read.as_millis().clamp(1, u32::MAX as u128 - 1) as u32,
                WriteTotalTimeoutMultiplier: 0,
                WriteTotalTimeoutConstant: WRITE_TIMEOUT.as_millis() as u32,
            };
            if unsafe { SetCommTimeouts(self.handle, &t) } == 0 {
                bail!("cannot set the port's timeouts (Windows error {})", unsafe { GetLastError() });
            }
            Ok(())
        }

        pub fn write(&mut self, data: &[u8]) -> Result<()> {
            self.timeouts(Duration::from_millis(1))?;
            let mut written = 0;
            let ok = unsafe { WriteFile(self.handle, data.as_ptr(), data.len() as u32, &mut written, std::ptr::null_mut()) };
            if ok == 0 || written as usize != data.len() {
                bail!("write to the port failed ({written} of {} bytes, Windows error {})", data.len(), unsafe {
                    GetLastError()
                });
            }
            Ok(())
        }

        /// Up to `len` bytes, whatever has come within `timeout`, or less once `done`
        /// says what came is whole.
        pub fn read_until(&mut self, len: usize, timeout: Duration, done: &dyn Fn(&[u8]) -> bool) -> Result<Vec<u8>> {
            let end = Instant::now() + timeout;
            let mut buf = vec![0u8; len];
            let mut got = 0;
            while got < len && !done(&buf[..got]) {
                let left = end.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                self.timeouts(left)?;
                let mut n = 0;
                let ok = unsafe {
                    ReadFile(self.handle, buf[got..].as_mut_ptr(), (len - got) as u32, &mut n, std::ptr::null_mut())
                };
                if ok == 0 {
                    bail!("read from the port failed (Windows error {})", unsafe { GetLastError() });
                }
                got += n as usize;
            }
            buf.truncate(got);
            Ok(buf)
        }

        /// Drops whatever the hub sent that nobody read.
        pub fn discard_input(&mut self) {
            unsafe { PurgeComm(self.handle, PURGE_RXCLEAR) };
        }
    }

    impl Drop for Serial {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.handle) };
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use anyhow::Context;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::path::{Path, PathBuf};

    /// The USB device a tty belongs to: the first directory up from its `device`
    /// link with `idVendor` in it.
    fn usb_device(tty: &Path) -> Option<PathBuf> {
        let mut dir = std::fs::canonicalize(tty.join("device")).ok()?;
        for _ in 0..6 {
            if dir.join("idVendor").is_file() {
                return Some(dir);
            }
            dir = dir.parent()?.to_path_buf();
        }
        None
    }

    fn hex(path: &Path) -> Option<u16> {
        u16::from_str_radix(std::fs::read_to_string(path).ok()?.trim(), 16).ok()
    }

    pub(super) fn ports_in(sys_tty: &Path, dev: &Path) -> Vec<Port> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(sys_tty).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("ttyACM") && !name.starts_with("ttyUSB") {
                continue;
            }
            let Some(usb) = usb_device(&entry.path()) else { continue };
            let (Some(vid), Some(pid)) = (hex(&usb.join("idVendor")), hex(&usb.join("idProduct"))) else { continue };
            out.push(Port { vid, pid, path: dev.join(&name).to_string_lossy().into_owned() });
        }
        out
    }

    /// The USB serial ports in `/sys/class/tty`, which any user can read.
    pub fn ports() -> Vec<Port> {
        ports_in(Path::new("/sys/class/tty"), Path::new("/dev"))
    }

    pub struct Serial {
        fd: OwnedFd,
    }

    fn os_error(what: &str) -> anyhow::Error {
        anyhow::anyhow!("{what}: {}", std::io::Error::last_os_error())
    }

    impl Serial {
        /// Opens the port, raw, for this program alone (`TIOCEXCL`).
        pub fn open(path: &CStr) -> Result<Serial> {
            let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC) };
            if fd < 0 {
                let e = std::io::Error::last_os_error();
                if e.raw_os_error() == Some(libc::EACCES) {
                    return Err(anyhow::anyhow!(e)).context(super::super::hid::NO_ACCESS);
                }
                if e.raw_os_error() == Some(libc::EBUSY) {
                    bail!("the port is held by another program");
                }
                bail!("cannot open the port: {e}");
            }
            let port = Serial { fd: unsafe { OwnedFd::from_raw_fd(fd) } };
            if unsafe { libc::ioctl(fd, libc::TIOCEXCL) } != 0 {
                return Err(os_error("cannot open the port for camless alone"));
            }
            let mut t: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(fd, &mut t) } != 0 {
                return Err(os_error("cannot read the port's settings"));
            }
            unsafe { libc::cfmakeraw(&mut t) };
            t.c_cflag |= libc::CLOCAL | libc::CREAD;
            t.c_cflag &= !(libc::CRTSCTS | libc::CSTOPB | libc::PARENB);
            t.c_cc[libc::VMIN] = 0;
            t.c_cc[libc::VTIME] = 0;
            if unsafe { libc::cfsetspeed(&mut t, libc::B115200) } != 0 || unsafe { libc::tcsetattr(fd, libc::TCSANOW, &t) } != 0 {
                return Err(os_error("cannot set the port to 115200 baud"));
            }
            // DTR and RTS on. A port without modem lines, such as a pty, says no,
            // which changes nothing.
            let lines: libc::c_int = libc::TIOCM_DTR | libc::TIOCM_RTS;
            unsafe { libc::ioctl(fd, libc::TIOCMBIS, &lines) };
            unsafe { libc::tcflush(fd, libc::TCIOFLUSH) };
            Ok(port)
        }

        fn wait(&self, events: libc::c_short, left: Duration) -> Result<bool> {
            let mut p = libc::pollfd { fd: self.fd.as_raw_fd(), events, revents: 0 };
            let ms = left.as_millis().clamp(1, i32::MAX as u128) as libc::c_int;
            match unsafe { libc::poll(&mut p, 1, ms) } {
                n if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => Ok(false),
                n if n < 0 => Err(os_error("the port failed")),
                0 => Ok(false),
                _ if p.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 && p.revents & events == 0 => {
                    bail!("the port is gone")
                }
                _ => Ok(true),
            }
        }

        pub fn write(&mut self, data: &[u8]) -> Result<()> {
            let end = Instant::now() + WRITE_TIMEOUT;
            let mut sent = 0;
            while sent < data.len() {
                let n = unsafe { libc::write(self.fd.as_raw_fd(), data[sent..].as_ptr().cast(), data.len() - sent) };
                if n > 0 {
                    sent += n as usize;
                    continue;
                }
                let e = std::io::Error::last_os_error();
                if n < 0 && !matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) {
                    bail!("write to the port failed: {e}");
                }
                let left = end.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    bail!("write to the port timed out ({sent} of {} bytes)", data.len());
                }
                self.wait(libc::POLLOUT, left)?;
            }
            Ok(())
        }

        /// Up to `len` bytes, whatever has come within `timeout`, or less once `done`
        /// says what came is whole.
        pub fn read_until(&mut self, len: usize, timeout: Duration, done: &dyn Fn(&[u8]) -> bool) -> Result<Vec<u8>> {
            let end = Instant::now() + timeout;
            let mut buf = vec![0u8; len];
            let mut got = 0;
            while got < len && !done(&buf[..got]) {
                let left = end.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                if !self.wait(libc::POLLIN, left)? {
                    continue;
                }
                let n = unsafe { libc::read(self.fd.as_raw_fd(), buf[got..].as_mut_ptr().cast(), len - got) };
                if n > 0 {
                    got += n as usize;
                } else if n == 0 {
                    bail!("the port is gone");
                } else {
                    let e = std::io::Error::last_os_error();
                    if !matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) {
                        bail!("read from the port failed: {e}");
                    }
                }
            }
            buf.truncate(got);
            Ok(buf)
        }

        /// Drops whatever the hub sent that nobody read.
        pub fn discard_input(&mut self) {
            unsafe { libc::tcflush(self.fd.as_raw_fd(), libc::TCIFLUSH) };
        }
    }
}

/// Sends a request and reads its reply of `len` bytes: the input left over from
/// before is dropped first, as Nexus does, so an old reply is not taken for this one.
pub fn request(port: &mut Serial, request: &[u8], len: usize, timeout: Duration) -> Result<Vec<u8>> {
    request_until(port, request, len, timeout, &|_| false)
}

/// `request` for a reply of up to `len` bytes that ends where `done` says.
pub fn request_until(
    port: &mut Serial,
    request: &[u8],
    len: usize,
    timeout: Duration,
    done: &dyn Fn(&[u8]) -> bool,
) -> Result<Vec<u8>> {
    port.discard_input();
    port.write(request)?;
    port.read_until(len, timeout, done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usb_ids_come_from_the_hardware_id() {
        assert_eq!(ids_in_hardware_id(r"USB\VID_3402&PID_0904&REV_0100"), Some((0x3402, 0x0904)));
        assert_eq!(ids_in_hardware_id(r"usb\vid_3402&pid_0901&mi_00"), Some((0x3402, 0x0901)));
        assert_eq!(ids_in_hardware_id(r"ACPI\PNP0501"), None);
        assert_eq!(ids_in_hardware_id(r"USB\VID_34"), None);
    }

    /// The ports in a made-up sysfs: a USB ACM port, a USB serial adapter one level
    /// further down, and a built-in port without USB.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_finds_usb_ports_and_their_ids_in_sysfs() {
        use std::fs;
        let root = std::env::temp_dir().join(format!("camless-tty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let usb = root.join("devices/usb1/1-2");
        let acm = usb.join("1-2:1.0/tty/ttyACM0");
        let adapter = usb.join("1-2:1.1/ttyUSB0/tty/ttyUSB0");
        let builtin = root.join("devices/platform/serial8250/tty/ttyS0");
        for d in [&acm, &adapter, &builtin] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(usb.join("idVendor"), "3402\n").unwrap();
        fs::write(usb.join("idProduct"), "0904\n").unwrap();
        let class = root.join("class/tty");
        fs::create_dir_all(&class).unwrap();
        for (name, dir) in [("ttyACM0", &acm), ("ttyUSB0", &adapter), ("ttyS0", &builtin)] {
            fs::create_dir_all(class.join(name)).unwrap();
            std::os::unix::fs::symlink(dir.parent().unwrap().parent().unwrap(), class.join(name).join("device")).unwrap();
        }
        let mut found = linux::ports_in(&class, std::path::Path::new("/dev"));
        found.sort_by(|a, b| a.path.cmp(&b.path));
        let _ = fs::remove_dir_all(&root);
        let port = |path: &str| Port { vid: 0x3402, pid: 0x0904, path: path.into() };
        assert_eq!(found, [port("/dev/ttyACM0"), port("/dev/ttyUSB0")]);
    }

    /// A pty stands in for the hub: what camless writes comes out of the other end,
    /// and what the hub sends comes back within the timeout.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_port_writes_reads_and_discards() {
        use std::io::{Read, Write};
        use std::os::fd::FromRawFd;
        let (mut master, mut slave) = (0, 0);
        assert_eq!(
            unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), std::ptr::null()) },
            0
        );
        let mut name = [0 as libc::c_char; 128];
        assert_eq!(unsafe { libc::ttyname_r(slave, name.as_mut_ptr(), name.len()) }, 0);
        let path = unsafe { CStr::from_ptr(name.as_ptr()) }.to_owned();
        let mut hub = unsafe { std::fs::File::from_raw_fd(master) };
        let mut port = Serial::open(&path).unwrap();
        unsafe { libc::close(slave) };
        // A second open while camless has it is refused: two programs on one hub
        // fight. Root may open it anyway.
        assert!(Serial::open(&path).is_err() || unsafe { libc::geteuid() } == 0);
        // Raw: no byte is translated or swallowed.
        let frame = [0xff, 0xcc, 0x01, 0x00, 0x0a, 0x0d, 0x03, 0x11, 0x13];
        port.write(&frame).unwrap();
        let mut got = [0u8; 9];
        hub.read_exact(&mut got).unwrap();
        assert_eq!(got, frame);
        hub.write_all(&[0xff, 0xcc, 0x01, 0x05]).unwrap();
        assert_eq!(port.read_until(20, Duration::from_millis(300), &|_| false).unwrap(), [0xff, 0xcc, 0x01, 0x05]);
        // Nothing more: the read gives up at its timeout.
        let start = Instant::now();
        assert!(port.read_until(4, Duration::from_millis(100), &|_| false).unwrap().is_empty());
        assert!(start.elapsed() >= Duration::from_millis(90));
        // A reply that says it is whole ends the read before its timeout.
        hub.write_all(&[0xff, 0xcc, 0x00]).unwrap();
        let start = Instant::now();
        assert_eq!(port.read_until(20, Duration::from_secs(2), &|r| r.len() >= 3).unwrap(), [0xff, 0xcc, 0x00]);
        assert!(start.elapsed() < Duration::from_secs(1));
        // An old reply is dropped before a request.
        hub.write_all(&[1, 2, 3]).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let echo = std::thread::spawn(move || {
            let mut req = [0u8; 3];
            hub.read_exact(&mut req).unwrap();
            hub.write_all(&[0xff, 0xdd, 0x02, 1, 0, 0, 2]).unwrap();
            hub
        });
        assert_eq!(request(&mut port, &[0xff, 0xdd, 0x02], 7, Duration::from_secs(2)).unwrap(), [0xff, 0xdd, 0x02, 1, 0, 0, 2]);
        drop(echo.join().unwrap());
    }
}
