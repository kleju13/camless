//! HTTPS GET through curl on Linux, which desktops come with, with the system's
//! certificates and proxy settings: the counterpart of WinHTTP on Windows, and no
//! TLS stack in the app. Redirects are followed, as release downloads need, to
//! https only.

use anyhow::{bail, Context, Result};
use std::process::{Command, Stdio};

/// GitHub's API turns away requests without a User-Agent.
const AGENT: &str = concat!("camless/", env!("CARGO_PKG_VERSION"));

/// Larger answers are not a release of camless.
const MAX_BODY: usize = 64 << 20;

/// The body of `url`, which must be https, if it answers 200.
pub fn get(url: &str) -> Result<Vec<u8>> {
    let rest = url.strip_prefix("https://").with_context(|| format!("{url} is not an https URL"))?;
    let host = rest.split('/').next().unwrap_or(rest);
    let out = Command::new("curl")
        .args(["--silent", "--show-error", "--fail", "--location", "--proto", "=https", "--proto-redir", "=https"])
        .args(["--connect-timeout", "10", "--max-time", "300", "--max-filesize", &MAX_BODY.to_string()])
        .args(["--user-agent", AGENT, "--", url])
        .stdin(Stdio::null())
        .output()
        .context("running curl, which camless uses for updates; is it installed?")?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        bail!("{host}: {}", why.trim().trim_start_matches("curl: "));
    }
    if out.stdout.len() > MAX_BODY {
        bail!("{url} is larger than {} MB", MAX_BODY >> 20);
    }
    Ok(out.stdout)
}
