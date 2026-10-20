//! What the camless daemon and the GUI share: the config file, the curve engine and
//! the status the daemon serves. Nothing here touches hardware.

pub mod cam_import;
pub mod config;
pub mod curve;
pub mod hardware;
pub mod lighting;
pub mod status;

/// Port of the daemon's local API. KrakenDial uses 30640.
pub const DEFAULT_API: &str = "127.0.0.1:30641";
