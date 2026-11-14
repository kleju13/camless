//! Just enough of the Chrome DevTools Protocol to drive one headless page over the
//! browser's own pipes (`--remote-debugging-pipe`): commands with replies, and
//! events in between. Each message is JSON that ends in a NUL byte.
//!
//! The pipes, and not a debugging port on 127.0.0.1: any user of the computer, and
//! any program on it, can connect to such a port and then drive a browser that runs
//! as the signed-in user. Nobody else can reach a pipe.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};

/// Screencast frames are base64 PNGs of a few hundred kB; anything far beyond that
/// is not a frame of a 640×640 screen.
const MAX_MESSAGE: usize = 32 << 20;
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// How much of the browser's output is read at once.
const CHUNK: usize = 256 << 10;

pub struct Cdp {
    /// The pipe the browser reads commands from.
    to: File,
    from: Replies,
    /// Bytes read but not yet split into messages.
    buf: Vec<u8>,
    events: VecDeque<Value>,
    next_id: u64,
}

impl Cdp {
    /// Talks to a browser started with `--remote-debugging-pipe` over its two pipes:
    /// `to` is the one it reads, `from` the one it writes.
    pub fn pipe(to: File, from: File) -> Result<Cdp> {
        Ok(Cdp { to, from: Replies::new(from, CHUNK)?, buf: Vec::new(), events: VecDeque::new(), next_id: 1 })
    }

    /// Sends a command and returns its id without waiting for the reply.
    pub fn send(&mut self, method: &str, params: Value, session: Option<&str>) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        self.to.write_all(&[msg.to_string().as_bytes(), b"\0"].concat()).context("writing to the browser")?;
        Ok(id)
    }

    /// Sends a command and waits for its result. Events that arrive meanwhile are
    /// kept for `next_event`.
    pub fn call(&mut self, method: &str, params: Value, session: Option<&str>) -> Result<Value> {
        let id = self.send(method, params, session)?;
        let reply = self.reply(id, CALL_TIMEOUT)?.ok_or_else(|| anyhow!("{method}: no reply"))?;
        if let Some(e) = reply.get("error") {
            bail!("{method}: {}", e["message"].as_str().unwrap_or("failed"));
        }
        Ok(reply["result"].clone())
    }

    /// The reply to command `id`, or None if it did not come within `timeout`.
    /// Events that arrive meanwhile are kept for `next_event`.
    pub fn reply(&mut self, id: u64, timeout: Duration) -> Result<Option<Value>> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match read_message(&mut self.from, &mut self.buf, left)? {
                Some(msg) if msg["id"].as_u64() == Some(id) => return Ok(Some(msg)),
                Some(msg) if msg.get("method").is_some() => self.events.push_back(msg),
                Some(_) => {}
                None if left.is_zero() => return Ok(None),
                None => {}
            }
        }
    }

    /// The next event, waiting at most `timeout`. Replies to `send` are dropped.
    pub fn next_event(&mut self, timeout: Duration) -> Result<Option<Value>> {
        if let Some(e) = self.events.pop_front() {
            return Ok(Some(e));
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match read_message(&mut self.from, &mut self.buf, left)? {
                Some(msg) if msg.get("method").is_some() => return Ok(Some(msg)),
                Some(_) => {}
                None if left.is_zero() => return Ok(None),
                None => {}
            }
        }
    }
}

/// One whole message from the browser, or None if none arrived within `timeout`.
fn read_message(from: &mut Replies, buf: &mut Vec<u8>, timeout: Duration) -> Result<Option<Value>> {
    let deadline = Instant::now() + timeout;
    // A frame arrives over many reads: each looks for the end in what it added.
    let mut searched = 0;
    loop {
        if let Some(i) = buf[searched..].iter().position(|&b| b == 0) {
            let end = searched + i;
            let msg: Vec<u8> = buf.drain(..=end).take(end).collect();
            return Ok(Some(serde_json::from_slice(&msg).context("a DevTools message that is not JSON")?));
        }
        searched = buf.len();
        if buf.len() > MAX_MESSAGE {
            bail!("a DevTools message over {} MB", MAX_MESSAGE >> 20);
        }
        if !from.more(buf, deadline.saturating_duration_since(Instant::now()))? {
            return Ok(None);
        }
    }
}

const CLOSED: &str = "the browser closed the DevTools connection";

/// The pipe the browser writes to, read with a time limit.
#[cfg(unix)]
struct Replies {
    from: File,
    chunk: Vec<u8>,
}

#[cfg(unix)]
impl Replies {
    fn new(from: File, chunk: usize) -> Result<Replies> {
        Ok(Replies { from, chunk: vec![0; chunk] })
    }

    /// Waits up to `timeout` for the browser to write, and adds what it wrote to
    /// `buf`. False if the time passed first.
    fn more(&mut self, buf: &mut Vec<u8>, timeout: Duration) -> Result<bool> {
        use std::os::fd::AsRawFd;
        let mut fd = libc::pollfd { fd: self.from.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        // SAFETY: one pollfd that lives through the call.
        let ready = unsafe { libc::poll(&mut fd, 1, ms) };
        if ready < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == ErrorKind::Interrupted {
                return Ok(true);
            }
            return Err(e).context("waiting for the browser");
        }
        if ready == 0 {
            return Ok(false);
        }
        match self.from.read(&mut self.chunk) {
            Ok(0) => bail!(CLOSED),
            Ok(n) => buf.extend_from_slice(&self.chunk[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e).context("reading from the browser"),
        }
        Ok(true)
    }
}

/// The pipe the browser writes to, read with a time limit. A read of an anonymous
/// pipe on Windows cannot be given one, so a thread reads and hands on what it got.
/// It ends with the browser, which closes the pipe.
#[cfg(windows)]
struct Replies(std::sync::mpsc::Receiver<std::io::Result<Vec<u8>>>);

#[cfg(windows)]
impl Replies {
    fn new(mut from: File, chunk: usize) -> Result<Replies> {
        // A few chunks ahead at most: the browser waits with the rest, as it would
        // on a pipe nobody reads.
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let read = move || {
            let mut chunk = vec![0; chunk];
            loop {
                let got = match from.read(&mut chunk) {
                    Ok(0) => return,
                    Ok(n) => Ok(chunk[..n].to_vec()),
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(e) => Err(e),
                };
                let failed = got.is_err();
                if tx.send(got).is_err() || failed {
                    return;
                }
            }
        };
        std::thread::Builder::new().name("browser-pipe".into()).spawn(read).context("starting the thread that reads from the browser")?;
        Ok(Replies(rx))
    }

    /// Waits up to `timeout` for the browser to write, and adds what it wrote to
    /// `buf`. False if the time passed first.
    fn more(&mut self, buf: &mut Vec<u8>, timeout: Duration) -> Result<bool> {
        use std::sync::mpsc::RecvTimeoutError;
        match self.0.recv_timeout(timeout) {
            Ok(Ok(bytes)) => buf.extend_from_slice(&bytes),
            Ok(Err(e)) => return Err(e).context("reading from the browser"),
            Err(RecvTimeoutError::Timeout) => return Ok(false),
            Err(RecvTimeoutError::Disconnected) => bail!(CLOSED),
        }
        Ok(true)
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// What the tests make their data URLs with; camless itself only decodes.
#[cfg(test)]
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(B64[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn base64_decode(text: &str) -> Result<Vec<u8>> {
    let mut map = [255u8; 256];
    for (i, &c) in B64.iter().enumerate() {
        map[c as usize] = i as u8;
    }
    let text = text.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for &c in text {
        let v = map[c as usize];
        if v == 255 {
            bail!("not base64");
        }
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for data in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar", &[0, 255, 128, 7, 9]] {
            assert_eq!(base64_decode(&base64_encode(data)).unwrap(), data);
        }
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert!(base64_decode("a$b").is_err());
    }

    /// A pipe as (read end, write end).
    #[cfg(unix)]
    fn pipe() -> (File, File) {
        use std::os::fd::FromRawFd;
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
        unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) }
    }

    #[cfg(windows)]
    fn pipe() -> (File, File) {
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::System::Pipes::CreatePipe;
        let (mut read, mut write) = (std::ptr::null_mut(), std::ptr::null_mut());
        assert_ne!(unsafe { CreatePipe(&mut read, &mut write, std::ptr::null(), 0) }, 0);
        unsafe { (File::from_raw_handle(read), File::from_raw_handle(write)) }
    }

    #[test]
    fn piped_messages_split_over_reads_come_out_whole_and_in_order() {
        let (from, mut to) = pipe();
        let mut from = Replies::new(from, 7).unwrap();
        let mut buf = Vec::new();
        to.write_all(b"{\"id\":1,\"a\":\"long enough for many reads\"}\0{\"id\":2}\0{\"id\"").unwrap();
        let first = read_message(&mut from, &mut buf, Duration::from_secs(5)).unwrap().unwrap();
        assert_eq!(first["id"], 1);
        assert_eq!(read_message(&mut from, &mut buf, Duration::from_secs(5)).unwrap().unwrap()["id"], 2);
        assert_eq!(read_message(&mut from, &mut buf, Duration::from_millis(50)).unwrap(), None);
        to.write_all(b":3}\0").unwrap();
        assert_eq!(read_message(&mut from, &mut buf, Duration::from_secs(5)).unwrap().unwrap()["id"], 3);
        assert!(buf.is_empty());
    }

    #[test]
    fn a_closed_pipe_is_an_error_and_not_a_wait() {
        let (from, to) = pipe();
        let mut from = Replies::new(from, 64).unwrap();
        drop(to);
        let e = read_message(&mut from, &mut Vec::new(), Duration::from_secs(5)).unwrap_err();
        assert_eq!(e.to_string(), CLOSED);
    }
}
