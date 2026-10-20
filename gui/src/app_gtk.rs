//! The tray icon and, while it is open, the window with the page in it, on Linux.
//! Both live on GTK's main loop, which wry's WebKitGTK and tray-icon's menu need.
//! The window and its page are dropped on close, as on Windows, so the tray alone
//! holds next to no memory.
//!
//! Events from other threads and from the handlers reach [`App`] through
//! [`send`], which queues them on the main loop, so no handler runs inside another.

use crate::backend::{Backend, Options};
use crate::tray::{self, Action, Launches, Tray};
use anyhow::{Context, Result};
use gtk::prelude::*;
use gtk::{gdk, gdk_pixbuf, glib};
use std::borrow::Cow;
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tray_icon::menu::MenuEvent;
use wry::http::{Request, Response};
use wry::{WebContext, WebView, WebViewBuilder, WebViewBuilderExtUnix, WebViewExtUnix};

/// The page's address; WebKitGTK serves the custom protocol as it is.
const ORIGIN: &str = "camless://localhost/";

/// How long a closing window waits for the page to save its last edits.
const SAVE_WAIT: Duration = Duration::from_secs(2);

/// The window's background until the page draws, the page's own.
const BACKGROUND: (u8, u8, u8, u8) = (0x1a, 0x1c, 0x1e, 0xff);

enum UserEvent {
    Menu(MenuEvent),
    /// Another launch asked for the window.
    Launch,
    /// The window's close button.
    CloseRequested,
    /// The page saved its edits; a closing window can go.
    Saved,
    /// The page did not answer in time.
    SaveTimeout(u64),
    /// An update is ready: close as Exit does, and the update starts after.
    Quit,
    /// The window came to the front.
    Focused,
}

struct Open {
    // Dropped in this order: the page before its window.
    webview: WebView,
    window: gtk::Window,
    /// Closing, waiting for the page to save; the number tells timeouts apart.
    closing: Option<u64>,
}

struct App {
    tray: Option<Tray>,
    open: Option<Open>,
    ui_dir: Option<PathBuf>,
    exiting: bool,
    closes: u64,
    context: WebContext,
}

thread_local! {
    /// The app, on the GTK thread, where every event is handled.
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Queues `event` for the main loop, from any thread.
fn send(event: UserEvent) {
    glib::idle_add_once(move || {
        APP.with(|app| {
            if let Some(app) = app.borrow_mut().as_mut() {
                app.handle(event);
            }
        })
    });
}

pub fn run(launches: Launches, show: bool, ui_dir: Option<PathBuf>) -> Result<()> {
    // Before any thread starts, as the environment is the process's.
    draw_on_the_cpu();
    // The window's app id: desktops match it to camless.desktop for its icon and name.
    glib::set_prgname(Some("camless"));
    gtk::init().context("starting GTK; is there a display?")?;
    MenuEvent::set_event_handler(Some(|e| send(UserEvent::Menu(e))));
    tray::watch(launches, || send(UserEvent::Launch));
    // Launching camless starts the service too, so there is one thing to run.
    crate::service::start_if_stopped();
    crate::update::on_quit(|| send(UserEvent::Quit));
    let _ = crate::update::check(crate::update::channel(&camless_core::config::default_path()));
    let tray = Tray::new().unwrap_or_else(|e| {
        eprintln!("tray: {e:#}");
        None
    });
    // Without an icon there would be no way back to a closed window.
    let show = show || tray.is_none();
    // WebKitGTK keeps its cache and storage here.
    let data = camless_core::config::xdg_dir("XDG_DATA_HOME", ".local/share").join("camless").join("webkit");
    let app = App { tray, open: None, ui_dir, exiting: false, closes: 0, context: WebContext::new(Some(data)) };
    APP.with(|a| *a.borrow_mut() = Some(app));
    if show {
        send(UserEvent::Launch);
    }
    gtk::main();
    // The page has saved; the icon goes with the app.
    APP.with(|a| a.borrow_mut().take());
    crate::update::run_ready();
    Ok(())
}

/// The page is a few panels and small canvases, which WebKit draws on the CPU for
/// about the same cost. Without GPU compositing its web process takes half the
/// memory (200 MB instead of 430 MB, measured on Xvfb), as Edge does on Windows
/// with the GPU off. It is set before GTK starts: turned off in a page that runs,
/// WebKit drew nothing more. A setting the user made stays.
fn draw_on_the_cpu() {
    const VAR: &str = "WEBKIT_DISABLE_COMPOSITING_MODE";
    if std::env::var_os(VAR).is_none() {
        std::env::set_var(VAR, "1");
    }
}

impl App {
    fn handle(&mut self, event: UserEvent) {
        match event {
            UserEvent::Menu(e) => match self.tray.as_ref().and_then(|t| t.menu(&e)) {
                Some(Action::Open) => self.show(),
                Some(Action::Exit) => self.close(true),
                None => {}
            },
            UserEvent::Launch => self.show(),
            UserEvent::CloseRequested => {
                // Without an icon, closing the window is the only way out.
                let exit = self.tray.is_none();
                self.close(exit);
            }
            UserEvent::Quit => self.close(true),
            UserEvent::Saved => {
                if self.open.as_ref().is_some_and(|o| o.closing.is_some()) {
                    self.closed();
                }
            }
            UserEvent::SaveTimeout(n) => {
                if self.open.as_ref().is_some_and(|o| o.closing == Some(n)) {
                    self.closed();
                }
            }
            UserEvent::Focused => {
                // Start at login may have changed in the page.
                if let Some(t) = &self.tray {
                    t.sync();
                }
            }
        }
    }

    fn show(&mut self) {
        if self.exiting {
            return;
        }
        if let Some(o) = &mut self.open {
            // Coming back while it was closing keeps the same page.
            o.closing = None;
            o.window.show();
            o.window.deiconify();
            o.window.present();
            return;
        }
        match self.create() {
            Ok(o) => self.open = Some(o),
            Err(e) => {
                eprintln!("window: {e:#}");
                if self.tray.is_none() {
                    gtk::main_quit();
                }
            }
        }
    }

    fn create(&mut self) -> Result<Open> {
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title("camless");
        window.set_default_size(1180, 760);
        window.set_size_request(900, 560);
        if let Ok((rgba, size)) = tray::icon_rgba() {
            let size = size as i32;
            let bytes = glib::Bytes::from_owned(rgba);
            window.set_icon(Some(&gdk_pixbuf::Pixbuf::from_bytes(&bytes, gdk_pixbuf::Colorspace::Rgb, true, 8, size, size, size * 4)));
        }
        window.connect_delete_event(|_, _| {
            send(UserEvent::CloseRequested);
            glib::Propagation::Stop
        });
        window.connect_focus_in_event(|_, _| {
            send(UserEvent::Focused);
            glib::Propagation::Proceed
        });
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        window.add(&page);
        let saved: Box<dyn Fn() + Send + Sync> = Box::new(|| send(UserEvent::Saved));
        let options = Options { config: None, ui_dir: self.ui_dir.clone(), sandbox: false };
        let backend = Arc::new(Backend::new(options, Some(saved)));
        let webview = WebViewBuilder::new_with_web_context(&mut self.context)
            .with_custom_protocol("camless".into(), move |_, req| respond(&backend, req))
            .with_url(ORIGIN)
            .with_background_color(BACKGROUND)
            // The page never leaves the app, and opens no windows of its own.
            .with_navigation_handler(|url| url.starts_with(ORIGIN))
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .build_gtk(&page)?;
        // Minimized: the page stops drawing until it is back.
        let view = webview.webview();
        window.connect_window_state_event(move |_, e| {
            view.set_visible(!e.new_window_state().contains(gdk::WindowState::ICONIFIED));
            glib::Propagation::Proceed
        });
        window.show_all();
        window.present();
        Ok(Open { webview, window, closing: None })
    }

    /// Hides the window, lets the page save, then drops it; `exit` then ends the app.
    fn close(&mut self, exit: bool) {
        self.exiting |= exit;
        let Some(o) = &mut self.open else {
            if self.exiting {
                gtk::main_quit();
            }
            return;
        };
        if o.closing.is_some() {
            return;
        }
        self.closes += 1;
        let n = self.closes;
        o.closing = Some(n);
        o.window.hide();
        let script = "(window.camlessClose ? camlessClose() : Promise.resolve()).finally(() => fetch('/api/window/closed', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' }))";
        if o.webview.evaluate_script(script).is_err() {
            self.closed();
            return;
        }
        glib::timeout_add_once(SAVE_WAIT, move || send(UserEvent::SaveTimeout(n)));
    }

    fn closed(&mut self) {
        if let Some(Open { webview, window, .. }) = self.open.take() {
            drop(webview);
            // SAFETY: nothing else holds the window; it was ours alone.
            unsafe { window.destroy() };
        }
        if self.exiting {
            // The icon goes at once, not when the process has wound down.
            self.tray = None;
            gtk::main_quit();
        }
    }
}

fn respond(backend: &Backend, req: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let content_type = req.headers().get("Content-Type").and_then(|v| v.to_str().ok());
    let target = req.uri().path_and_query().map_or("/", |p| p.as_str());
    let r = backend.handle(req.method().as_str(), target, content_type, req.body());
    let body: Cow<'static, [u8]> = Cow::Owned(r.body);
    Response::builder()
        .status(r.status)
        .header("Content-Type", r.content_type)
        .header("Cache-Control", "no-store")
        .body(body)
        .unwrap_or_else(|_| Response::new(Cow::Borrowed(&[][..])))
}
