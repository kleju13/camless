//! Linux has no counterpart of the Windows task that starts the service as
//! administrator: the service runs as the user, and camless's udev rules give that
//! user the devices. These keep the page's switch off.

use anyhow::{bail, Result};

pub const START_SERVICE: &str = "--start-service";

#[derive(Clone, Debug, Default)]
pub struct State {
    pub busy: bool,
    pub error: Option<String>,
}

pub fn state() -> State {
    State::default()
}

pub fn spawn(_on: bool) -> Result<()> {
    bail!("the service runs as you on Linux; it needs no administrator task")
}

pub fn ready() -> bool {
    false
}
