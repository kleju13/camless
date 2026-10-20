//! camless lives in the tray on Linux too, as an AppIndicator icon: KDE, Cinnamon,
//! Xfce and GNOME with its AppIndicator extension show one. Its menu opens the window;
//! AppIndicator tells the app of no clicks on the icon itself. Closing the window
//! drops it; Exit ends the app only, and the service keeps the fans on their curves.

use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// Passed by the logon entry: start with the icon only.
pub const TRAY_ONLY: &str = "--tray";

/// What a later launch sends.
const OPEN: &[u8] = b"open";

/// The socket later launches connect to, to ask this copy for its window.
pub struct Launches(Option<UnixListener>);

/// A socket in the abstract namespace: no file to clean up, and gone with the
/// process, so a crash leaves nothing that blocks the next start. Any user can bind
/// or connect to such a name, so both ends check that the other is this user.
fn address() -> Option<SocketAddr> {
    SocketAddr::from_abstract_name(format!("camless-gui-{}", me())).ok()
}

fn me() -> u32 {
    // SAFETY: getuid cannot fail.
    unsafe { libc::getuid() }
}

/// The user of the process at the other end of `s`.
fn peer(s: &impl AsRawFd) -> Option<u32> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: cred and len live through the call, and len is cred's size.
    let r = unsafe { libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) };
    (r == 0).then_some(cred.uid)
}

/// Holds the app to one copy. A later launch wakes the running one and gets `None`.
pub fn claim() -> Option<Launches> {
    // Without the socket there is no way to tell; run rather than not start.
    let Some(addr) = address() else { return Some(Launches(None)) };
    match UnixListener::bind_addr(&addr) {
        Ok(l) => Some(Launches(Some(l))),
        Err(_) => match UnixStream::connect_addr(&addr) {
            Ok(mut s) if peer(&s) == Some(me()) => {
                let _ = s.write_all(OPEN);
                None
            }
            Ok(_) => {
                eprintln!("another user holds camless's launch socket; starting without it");
                Some(Launches(None))
            }
            Err(_) => Some(Launches(None)),
        },
    }
}

/// Calls `open` each time a later launch of this user asks for the window.
pub fn watch(launches: Launches, open: impl Fn() + Send + 'static) {
    let Some(listener) = launches.0 else { return };
    let _ = std::thread::Builder::new().name("launches".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            if peer(&stream) != Some(me()) {
                continue;
            }
            // A launch writes at once; one that does not cannot hold up the next.
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let mut buf = [0u8; 8];
            let n = (&stream).take(buf.len() as u64).read(&mut buf).unwrap_or(0);
            if &buf[..n] == OPEN {
                open();
            }
        }
    });
}

pub enum Action {
    Open,
    Exit,
}

/// The icon and its menu. Events reach the main loop through the handlers that
/// tray-icon and muda call; [`Tray::menu`] tells what one asks for.
pub struct Tray {
    _icon: TrayIcon,
    show: MenuItem,
    logon: CheckMenuItem,
    exit: MenuItem,
}

impl Tray {
    /// Must run on the GTK thread, after GTK started. `None` when the desktop shows
    /// no AppIndicator icons, as GNOME without its extension: the window then is the
    /// only way in, and closing it ends the app.
    pub fn new() -> Result<Option<Tray>> {
        if !shown() {
            return Ok(None);
        }
        let show = MenuItem::new("Open camless", true, None);
        let logon = CheckMenuItem::new("Start at login", true, autostart::enabled(), None);
        let exit = MenuItem::new("Exit", true, None);
        let menu = Menu::new();
        menu.append_items(&[&show, &logon, &PredefinedMenuItem::separator(), &exit])?;
        let (rgba, size) = icon_rgba()?;
        let mut builder = TrayIconBuilder::new().with_tooltip("camless").with_icon(Icon::from_rgba(rgba, size, size)?).with_menu(Box::new(menu));
        // The icon goes to a file the panel reads; in the user's own runtime folder,
        // not in /tmp.
        if let Some(dir) = runtime_dir() {
            builder = builder.with_temp_dir_path(dir.join("camless-tray"));
        }
        // tray-icon panics when there is no AppIndicator library.
        let icon = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| builder.build()))
            .map_err(|_| anyhow::anyhow!("no AppIndicator library (libayatana-appindicator3)"))??;
        Ok(Some(Tray { _icon: icon, show, logon, exit }))
    }

    pub fn menu(&self, e: &MenuEvent) -> Option<Action> {
        if e.id == *self.show.id() {
            Some(Action::Open)
        } else if e.id == *self.exit.id() {
            Some(Action::Exit)
        } else {
            if e.id == *self.logon.id() {
                if let Err(err) = autostart::set(self.logon.is_checked()) {
                    eprintln!("start at login: {err:#}");
                }
                self.sync();
            }
            None
        }
    }

    /// Shows what the autostart folder holds, whoever changed it.
    pub fn sync(&self) {
        self.logon.set_checked(autostart::enabled());
    }
}

/// Whether a panel shows AppIndicator icons: one owns the StatusNotifierWatcher name
/// on the session bus.
fn shown() -> bool {
    use gtk::gio;
    use gtk::glib::variant::ToVariant;
    let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else { return false };
    let reply = bus.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameHasOwner",
        Some(&("org.kde.StatusNotifierWatcher",).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        1000,
        gio::Cancellable::NONE,
    );
    reply.ok().and_then(|r| r.get::<(bool,)>()).is_some_and(|(owned,)| owned)
}

/// camless.png, the icon the desktop entry names too.
pub const PNG: &[u8] = include_bytes!("../../linux/camless.png");

/// The icon as RGBA, and its size.
pub fn icon_rgba() -> Result<(Vec<u8>, u32)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(PNG));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder.read_info().context("reading camless.png")?;
    let mut buf = vec![0; reader.output_buffer_size().context("camless.png is too large")?];
    let info = reader.next_frame(&mut buf).context("reading camless.png")?;
    buf.truncate(info.buffer_size());
    Ok((buf, info.width))
}

/// `$XDG_RUNTIME_DIR`, the user's own folder for sockets and such.
pub fn runtime_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|d| d.is_absolute())
}

/// A desktop entry in `~/.config/autostart`, which every desktop that follows the
/// XDG autostart spec starts at login.
pub mod autostart {
    use super::TRAY_ONLY;
    use anyhow::{Context, Result};
    use std::path::PathBuf;

    fn file() -> PathBuf {
        camless_core::config::app_config_dir().join("autostart").join("camless.desktop")
    }

    pub fn enabled() -> bool {
        std::fs::read_to_string(file()).is_ok_and(|text| on(&text))
    }

    /// An entry the desktop runs: some turn it off with `Hidden` or GNOME's own key
    /// rather than by deleting it.
    fn on(text: &str) -> bool {
        !text.lines().any(|l| matches!(l.trim(), "Hidden=true" | "X-GNOME-Autostart-enabled=false"))
    }

    pub fn set(on: bool) -> Result<()> {
        let file = file();
        if !on {
            return match std::fs::remove_file(&file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e).with_context(|| format!("removing {}", file.display())),
                _ => Ok(()),
            };
        }
        let exe = std::env::current_exe()?;
        let exe = exe.to_string_lossy();
        // A program replaced while it runs, as install.sh does, reads as
        // "<path> (deleted)"; the new one is at the path.
        let exe = exe.strip_suffix(" (deleted)").unwrap_or(&exe);
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        std::fs::write(&file, entry(exe)).with_context(|| format!("writing {}", file.display()))
    }

    /// The entry for `exe`, quoted as the desktop entry spec asks, `%` doubled as
    /// field codes need.
    pub(super) fn entry(exe: &str) -> String {
        let quoted: String = exe
            .chars()
            .flat_map(|c| match c {
                '"' | '`' | '$' | '\\' => vec!['\\', c],
                '%' => vec!['%', '%'],
                c => vec![c],
            })
            .collect();
        format!(
            "[Desktop Entry]\nType=Application\nName=camless\nComment=Pump, fan, lighting and LCD control for NZXT devices\nExec=\"{quoted}\" {TRAY_ONLY}\nIcon=camless\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_login_entry_quotes_the_program() {
            let e = entry("/home/a b/.local/bin/camless-gui");
            assert!(e.contains("Exec=\"/home/a b/.local/bin/camless-gui\" --tray\n"), "{e}");
            assert!(entry("/x/$HOME\"").contains("Exec=\"/x/\\$HOME\\\"\" --tray"));
            assert!(entry("/x/50%").contains("Exec=\"/x/50%%\" --tray"));
            assert!(on(&e));
            assert!(!on("[Desktop Entry]\nHidden=true\n"));
            assert!(!on("[Desktop Entry]\nX-GNOME-Autostart-enabled=false\n"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_decodes_as_a_square() {
        let (rgba, size) = icon_rgba().unwrap();
        assert_eq!(size, 256);
        assert_eq!(rgba.len(), (size * size * 4) as usize);
    }

    #[test]
    fn a_launch_socket_tells_whose_the_other_end_is() {
        let (a, b) = UnixStream::pair().unwrap();
        assert_eq!(peer(&a), Some(me()));
        assert_eq!(peer(&b), Some(me()));
    }
}
