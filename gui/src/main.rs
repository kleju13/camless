//! camless-gui: the desktop app. It lives in the tray; its window shows the daemon's
//! status over the loopback API and edits `camless.toml`. The daemon applies every
//! saved change by itself, so closing the app never stops fan control.
//!
//! `camless-gui --serve 127.0.0.1:<port> --config <camless.toml> [--ui-dir <dir>]`
//! serves the same page to a browser instead, as a sandbox, to work on the UI.
//! `camless-gui --start-service` starts the service and exits; see [`task`].
//!
//! On Linux the window is GTK's and the page WebKitGTK's ([`app`]), the tray icon an
//! AppIndicator ([`tray`]), and the service a systemd user service ([`service`]).

#![windows_subsystem = "windows"]

// What differs on Linux has a file of its own, under the same module name.
#[cfg_attr(target_os = "linux", path = "app_gtk.rs")]
mod app;
mod art;
mod assets;
mod backend;
mod client;
#[cfg_attr(target_os = "linux", path = "http_curl.rs")]
mod http;
mod report;
mod serve;
mod service;
#[cfg_attr(target_os = "linux", path = "task_none.rs")]
mod task;
#[cfg_attr(target_os = "linux", path = "tray_linux.rs")]
mod tray;
mod update;

use anyhow::Context;
use backend::{Backend, Options};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let ui_dir = value("--ui-dir").map(PathBuf::from);
    // The scheduled task runs this as administrator: the service inherits that.
    if args.iter().any(|a| a == task::START_SERVICE) {
        return client::start_daemon(&camless_core::config::default_path());
    }
    if let Some(listen) = value("--serve") {
        let config = value("--config")
            .map(PathBuf::from)
            .context("--serve needs --config <camless.toml>; it does not edit the one the service runs")?;
        return serve::run(&listen, Backend::new(Options { config: Some(config), ui_dir, sandbox: true }, None));
    }
    // A second launch asks the running copy to open its window.
    let Some(launches) = tray::claim() else { return Ok(()) };
    app::run(launches, !args.iter().any(|a| a == tray::TRAY_ONLY), ui_dir)
}
