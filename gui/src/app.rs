//! The tray icon and, while it is open, the window with the page in it. Both live on
//! the main thread's event loop; the window and its WebView2 are dropped on close,
//! so the tray alone holds next to no memory.

use crate::backend::{Backend, Options};
use crate::tray::{self, Action, Launches, Tray};
use anyhow::Result;
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tray_icon::menu::MenuEvent;
use tray_icon::TrayIconEvent;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Icon, Window, WindowId};
use wry::http::{Request, Response};
use wry::{WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows};

/// The page's address; WebView2 serves `camless://` as this.
const ORIGIN: &str = "https://camless.localhost/";

/// How long a closing window waits for the page to save its last edits.
const SAVE_WAIT: Duration = Duration::from_secs(2);

/// The window's background until the page draws, the page's own.
const BACKGROUND: (u8, u8, u8, u8) = (0x1a, 0x1c, 0x1e, 0xff);

pub enum UserEvent {
    Tray(TrayIconEvent),
    Menu(MenuEvent),
    /// Another launch asked for the window.
    Launch,
    /// The page saved its edits; a closing window can go.
    Saved,
    /// The page did not answer in time.
    SaveTimeout(u64),
}

struct Open {
    // Dropped in this order: the page before its window.
    webview: WebView,
    window: Window,
    /// Closing, waiting for the page to save; the number tells timeouts apart.
    closing: Option<u64>,
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    tray: Option<Tray>,
    open: Option<Open>,
    show_at_start: bool,
    ui_dir: Option<PathBuf>,
    exiting: bool,
    closes: u64,
    context: WebContext,
}

pub fn run(launches: Launches, show: bool, ui_dir: Option<PathBuf>) -> Result<()> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let p = proxy.clone();
    TrayIconEvent::set_event_handler(Some(move |e| {
        let _ = p.send_event(UserEvent::Tray(e));
    }));
    let p = proxy.clone();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = p.send_event(UserEvent::Menu(e));
    }));
    let p = proxy.clone();
    tray::watch(launches, move || {
        let _ = p.send_event(UserEvent::Launch);
    });
    // WebView2 keeps its profile here rather than next to the program.
    let data = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("camless").join("WebView2"));
    let mut app = App {
        proxy,
        tray: None,
        open: None,
        show_at_start: show,
        ui_dir,
        exiting: false,
        closes: 0,
        context: WebContext::new(data),
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

impl App {
    fn show(&mut self, el: &ActiveEventLoop) {
        if self.exiting {
            return;
        }
        if let Some(o) = &mut self.open {
            // Coming back while it was closing keeps the same page.
            o.closing = None;
            o.window.set_visible(true);
            o.window.set_minimized(false);
            o.window.focus_window();
            return;
        }
        match self.create(el) {
            Ok(o) => self.open = Some(o),
            Err(e) => eprintln!("window: {e:#}"),
        }
    }

    fn create(&mut self, el: &ActiveEventLoop) -> Result<Open> {
        let icon = Icon::from_rgba(tray::icon_rgba(64), 64, 64)?;
        let attrs = Window::default_attributes()
            .with_title("camless")
            .with_window_icon(Some(icon))
            .with_inner_size(LogicalSize::new(1180.0, 760.0))
            .with_min_inner_size(LogicalSize::new(900.0, 560.0))
            .with_visible(false);
        let window = el.create_window(attrs)?;
        let proxy = self.proxy.clone();
        let saved: Box<dyn Fn() + Send + Sync> = Box::new(move || {
            let _ = proxy.send_event(UserEvent::Saved);
        });
        let options = Options { config: None, ui_dir: self.ui_dir.clone(), sandbox: false };
        let backend = Arc::new(Backend::new(options, Some(saved)));
        let webview = WebViewBuilder::new_with_web_context(&mut self.context)
            .with_custom_protocol("camless".into(), move |_, req| respond(&backend, req))
            .with_url("camless://localhost/")
            .with_https_scheme(true)
            .with_background_color(BACKGROUND)
            .with_default_context_menus(false)
            .with_browser_accelerator_keys(false)
            // The page never leaves the app, and opens no windows of its own.
            .with_navigation_handler(|url| url.starts_with(ORIGIN))
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .build(&window)?;
        window.set_visible(true);
        window.focus_window();
        Ok(Open { webview, window, closing: None })
    }

    /// Hides the window, lets the page save, then drops it; `exit` then ends the app.
    fn close(&mut self, el: &ActiveEventLoop, exit: bool) {
        self.exiting |= exit;
        let Some(o) = &mut self.open else {
            if self.exiting {
                el.exit();
            }
            return;
        };
        if o.closing.is_some() {
            return;
        }
        self.closes += 1;
        let n = self.closes;
        o.closing = Some(n);
        o.window.set_visible(false);
        let script = "(window.camlessClose ? camlessClose() : Promise.resolve()).finally(() => fetch('/api/window/closed', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' }))";
        if o.webview.evaluate_script(script).is_err() {
            self.closed(el);
            return;
        }
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(SAVE_WAIT);
            let _ = proxy.send_event(UserEvent::SaveTimeout(n));
        });
    }

    fn closed(&mut self, el: &ActiveEventLoop) {
        self.open = None;
        if self.exiting {
            // The icon goes at once, not when the process has wound down.
            self.tray = None;
            el.exit();
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn new_events(&mut self, _el: &ActiveEventLoop, cause: StartCause) {
        // tray-icon needs the loop running before it makes the icon.
        if cause == StartCause::Init {
            match Tray::new() {
                Ok(t) => self.tray = Some(t),
                Err(e) => eprintln!("tray: {e:#}"),
            }
        }
    }

    fn resumed(&mut self, el: &ActiveEventLoop) {
        // Without an icon there would be no way back to a closed window.
        if std::mem::take(&mut self.show_at_start) || self.tray.is_none() {
            self.show(el);
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Tray(e) => {
                if Tray::opens(&e) {
                    self.show(el);
                }
            }
            UserEvent::Menu(e) => match self.tray.as_ref().and_then(|t| t.menu(&e)) {
                Some(Action::Open) => self.show(el),
                Some(Action::Exit) => self.close(el, true),
                None => {}
            },
            UserEvent::Launch => self.show(el),
            UserEvent::Saved => {
                if self.open.as_ref().is_some_and(|o| o.closing.is_some()) {
                    self.closed(el);
                }
            }
            UserEvent::SaveTimeout(n) => {
                if self.open.as_ref().is_some_and(|o| o.closing == Some(n)) {
                    self.closed(el);
                }
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                // Without an icon, closing the window is the only way out.
                let exit = self.tray.is_none();
                self.close(el, exit);
            }
            // Minimized: the page stops drawing until it is back.
            WindowEvent::Resized(size) => {
                if let Some(o) = &self.open {
                    let _ = o.webview.set_visible(size.width > 0 && size.height > 0);
                }
            }
            WindowEvent::Focused(true) => {
                // Start with Windows may have changed in the page.
                if let Some(t) = &self.tray {
                    t.sync();
                }
            }
            _ => {}
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
