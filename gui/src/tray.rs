//! camless lives in the tray. Closing the window drops it, and with it the memory a
//! window needs; the icon opens a new one. Exit ends the app only: the service keeps
//! the fans on their curves.

use anyhow::{anyhow, Result};
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Registry::{RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ};
use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject, INFINITE};

/// Passed by the logon entry: start with the icon only.
pub const TRAY_ONLY: &str = "--tray";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// A named event that later launches set to ask this copy to open its window.
pub struct Launches(HANDLE);

/// Holds the app to one copy. A later launch wakes the running one and gets `None`.
pub fn claim() -> Option<Launches> {
    let name = wide("Local\\camless-gui-open");
    unsafe {
        let event = CreateEventW(null(), 0, 0, name.as_ptr());
        if event.is_null() {
            // Without the event there is no way to tell; run rather than not start.
            return Some(Launches(null_mut()));
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            SetEvent(event);
            return None;
        }
        Some(Launches(event))
    }
}

/// Calls `open` each time a later launch asks for the window.
pub fn watch(launches: Launches, open: impl Fn() + Send + 'static) {
    if launches.0.is_null() {
        return;
    }
    // The handle stays open for the life of the process.
    let handle = launches.0 as usize;
    let _ = std::thread::Builder::new().name("launches".into()).spawn(move || loop {
        if unsafe { WaitForSingleObject(handle as HANDLE, INFINITE) } != WAIT_OBJECT_0 {
            return;
        }
        open();
    });
}

pub enum Action {
    Open,
    Exit,
}

/// The icon and its menu. Events reach the event loop through the handlers that
/// tray-icon and muda call; [`Tray::menu`] tells what one asks for.
pub struct Tray {
    _icon: TrayIcon,
    show: MenuItem,
    logon: CheckMenuItem,
    exit: MenuItem,
}

impl Tray {
    /// Must run on the thread of the event loop, once it runs.
    pub fn new() -> Result<Tray> {
        let show = MenuItem::new("Open camless", true, None);
        let logon = CheckMenuItem::new("Start with Windows", true, autostart::enabled(), None);
        let exit = MenuItem::new("Exit", true, None);
        let menu = Menu::new();
        menu.append_items(&[&show, &logon, &PredefinedMenuItem::separator(), &exit])?;
        let icon = TrayIconBuilder::new()
            .with_tooltip("camless")
            .with_icon(Icon::from_rgba(icon_rgba(32), 32, 32)?)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()?;
        Ok(Tray { _icon: icon, show, logon, exit })
    }

    pub fn menu(&self, e: &MenuEvent) -> Option<Action> {
        if e.id == *self.show.id() {
            Some(Action::Open)
        } else if e.id == *self.exit.id() {
            Some(Action::Exit)
        } else {
            if e.id == *self.logon.id() {
                if let Err(err) = autostart::set(self.logon.is_checked()) {
                    eprintln!("start with Windows: {err:#}");
                }
                self.sync();
            }
            None
        }
    }

    /// Shows what the registry holds, whoever changed it.
    pub fn sync(&self) {
        self.logon.set_checked(autostart::enabled());
    }

    /// A left click opens the window; the right one shows the menu.
    pub fn opens(e: &TrayIconEvent) -> bool {
        matches!(e, TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. })
    }
}

/// camless's mark: a ring in the accent colour, like the Kraken's.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let c = (s - 1.0) / 2.0;
    let (outer, inner) = (s * 0.46, s * 0.27);
    let mut px = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let a = (outer - d + 0.5).clamp(0.0, 1.0) * (d - inner + 0.5).clamp(0.0, 1.0);
            px.extend_from_slice(&[0x8b, 0x5c, 0xf6, (a * 255.0).round() as u8]);
        }
    }
    px
}

/// The per-user Run entry, so camless starts in the tray at logon.
pub mod autostart {
    use super::*;

    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "camless";

    pub fn enabled() -> bool {
        let (key, value) = (wide(KEY), wide(VALUE));
        let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, null_mut(), null_mut(), null_mut()) };
        r == ERROR_SUCCESS
    }

    pub fn set(on: bool) -> Result<()> {
        let (key, value) = (wide(KEY), wide(VALUE));
        let r = if on {
            let exe = std::env::current_exe()?;
            let data = wide(&format!("\"{}\" {TRAY_ONLY}", exe.display()));
            unsafe {
                RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), REG_SZ, data.as_ptr() as *const c_void, (data.len() * 2) as u32)
            }
        } else {
            unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) }
        };
        if r != ERROR_SUCCESS {
            return Err(anyhow!("registry error {r}"));
        }
        Ok(())
    }
}
