//! `camless-gui --serve 127.0.0.1:<port>`: the page and its API over loopback HTTP,
//! to open in a browser while working on the UI. It runs as a sandbox (see
//! [`crate::backend::Options::sandbox`]) and edits only the config it is given.

use crate::backend::{Backend, Reply};
use anyhow::{anyhow, bail, Context, Result};
use std::io::Read;
use std::net::SocketAddr;
use tiny_http::{Header, Request, Response, Server};

/// Requests bodies past this are refused; a config is a few kilobytes.
const MAX_BODY: u64 = 4 << 20;

pub fn run(listen: &str, backend: Backend) -> Result<()> {
    let addr: SocketAddr = listen.parse().with_context(|| format!("{listen:?} is not ip:port"))?;
    if !addr.ip().is_loopback() {
        bail!("--serve must listen on a loopback address; {listen} would let the network change the fans");
    }
    let server = Server::http(addr).map_err(|e| anyhow!("cannot listen on {listen}: {e}"))?;
    let hosts = [addr.to_string(), format!("127.0.0.1:{}", addr.port()), format!("localhost:{}", addr.port())];
    eprintln!("camless-gui: serving http://{listen}/");
    for req in server.incoming_requests() {
        if let Err(e) = handle(req, &hosts, &backend) {
            eprintln!("serve: {e:#}");
        }
    }
    Ok(())
}

fn header<'a>(req: &'a Request, name: &'static str) -> Option<&'a str> {
    req.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str())
}

fn handle(mut req: Request, hosts: &[String], backend: &Backend) -> Result<()> {
    // A Host check stops DNS rebinding: a web page cannot reach this server under its
    // own domain name.
    if !header(&req, "Host").is_some_and(|h| hosts.iter().any(|ok| ok.eq_ignore_ascii_case(h))) {
        return Ok(req.respond(Response::empty(421))?);
    }
    let mut body = Vec::new();
    req.as_reader().take(MAX_BODY + 1).read_to_end(&mut body)?;
    if body.len() as u64 > MAX_BODY {
        return Ok(req.respond(Response::empty(413))?);
    }
    let method = req.method().as_str().to_string();
    let url = req.url().to_string();
    let Reply { status, content_type, body } = backend.handle(&method, &url, header(&req, "Content-Type"), &body);
    let res = Response::from_data(body)
        .with_status_code(status)
        .with_header(Header::from_bytes("Content-Type", content_type).unwrap())
        .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap());
    Ok(req.respond(res)?)
}
