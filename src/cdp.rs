//! Just enough of the Chrome DevTools Protocol to drive one headless page: a
//! WebSocket client for the browser's loopback debugging port, commands with
//! replies, and events in between.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// Screencast frames are base64 PNGs of a few hundred kB; anything far beyond that
/// is not a frame of a 640×640 screen.
const MAX_MESSAGE: usize = 32 << 20;
const CALL_TIMEOUT: Duration = Duration::from_secs(15);

pub struct Cdp {
    stream: TcpStream,
    /// Bytes read but not yet parsed into frames.
    buf: Vec<u8>,
    chunk: Vec<u8>,
    /// A text message split over continuation frames.
    partial: Vec<u8>,
    events: VecDeque<Value>,
    next_id: u64,
    mask_seed: u32,
}

impl Cdp {
    /// Connects to `ws://127.0.0.1:{port}{path}`, as written in `DevToolsActivePort`.
    pub fn connect(port: u16, path: &str) -> Result<Cdp> {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).with_context(|| format!("connecting to port {port}"))?;
        stream.set_nodelay(true)?;
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().subsec_nanos();
        let key = base64_encode(&seed.to_le_bytes().repeat(4));
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream.write_all(request.as_bytes())?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mut buf = Vec::new();
        let end = loop {
            let mut chunk = [0u8; 4096];
            let n = stream.read(&mut chunk).context("reading the WebSocket handshake")?;
            if n == 0 {
                bail!("the browser closed the connection during the handshake");
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
            if buf.len() > 16 << 10 {
                bail!("no end to the handshake reply");
            }
        };
        let head = String::from_utf8_lossy(&buf[..end]);
        if !head.starts_with("HTTP/1.1 101") {
            bail!("the browser refused the WebSocket: {}", head.lines().next().unwrap_or_default());
        }
        Ok(Cdp { stream, buf: buf[end..].to_vec(), chunk: vec![0; 256 << 10], partial: Vec::new(), events: VecDeque::new(), next_id: 1, mask_seed: seed | 1 })
    }

    /// Sends a command and returns its id without waiting for the reply.
    pub fn send(&mut self, method: &str, params: Value, session: Option<&str>) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        self.write_frame(0x1, msg.to_string().as_bytes())?;
        Ok(id)
    }

    /// Sends a command and waits for its result. Events that arrive meanwhile are
    /// kept for `next_event`.
    pub fn call(&mut self, method: &str, params: Value, session: Option<&str>) -> Result<Value> {
        let id = self.send(method, params, session)?;
        let deadline = Instant::now() + CALL_TIMEOUT;
        loop {
            let left = deadline.checked_duration_since(Instant::now()).ok_or_else(|| anyhow!("{method}: no reply"))?;
            let Some(msg) = self.read_message(left)? else { continue };
            if msg["id"].as_u64() == Some(id) {
                if let Some(e) = msg.get("error") {
                    bail!("{method}: {}", e["message"].as_str().unwrap_or("failed"));
                }
                return Ok(msg["result"].clone());
            }
            if msg.get("method").is_some() {
                self.events.push_back(msg);
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
            match self.read_message(left)? {
                Some(msg) if msg.get("method").is_some() => return Ok(Some(msg)),
                Some(_) => {}
                None if left.is_zero() => return Ok(None),
                None => {}
            }
        }
    }

    /// One whole message, or None if none arrived within `timeout`.
    fn read_message(&mut self, timeout: Duration) -> Result<Option<Value>> {
        let deadline = Instant::now() + timeout;
        loop {
            while let Some((opcode, fin, payload, used)) = parse_frame(&self.buf)? {
                self.buf.drain(..used);
                match opcode {
                    0x0 | 0x1 | 0x2 => {
                        self.partial.extend_from_slice(&payload);
                        if self.partial.len() > MAX_MESSAGE {
                            bail!("a DevTools message over {} MB", MAX_MESSAGE >> 20);
                        }
                        if fin {
                            let text = std::mem::take(&mut self.partial);
                            return Ok(Some(serde_json::from_slice(&text).context("a DevTools message that is not JSON")?));
                        }
                    }
                    0x8 => bail!("the browser closed the DevTools connection"),
                    0x9 => self.write_frame(0xA, &payload)?,
                    _ => {}
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            // A zero timeout means "block" to the socket, so wait at least a moment.
            self.stream.set_read_timeout(Some(left.max(Duration::from_millis(1))))?;
            match self.stream.read(&mut self.chunk) {
                Ok(0) => bail!("the browser closed the DevTools connection"),
                Ok(n) => self.buf.extend_from_slice(&self.chunk[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                }
                Err(e) => return Err(e).context("reading from the browser"),
            }
        }
    }

    fn write_frame(&mut self, opcode: u8, payload: &[u8]) -> Result<()> {
        // Clients must mask what they send. The mask only matters to proxies, and
        // there are none on loopback, so a cheap generator will do.
        self.mask_seed ^= self.mask_seed << 13;
        self.mask_seed ^= self.mask_seed >> 17;
        self.mask_seed ^= self.mask_seed << 5;
        let frame = encode_frame(opcode, payload, self.mask_seed.to_le_bytes());
        self.stream.write_all(&frame).context("writing to the browser")
    }
}

/// A client frame: final, masked.
fn encode_frame(opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(0x80 | opcode);
    match payload.len() {
        n if n < 126 => out.push(0x80 | n as u8),
        n if n <= 0xffff => {
            out.push(0x80 | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(0x80 | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

/// (opcode, final, payload, bytes used) of the first whole frame in `buf`.
fn parse_frame(buf: &[u8]) -> Result<Option<(u8, bool, Vec<u8>, usize)>> {
    if buf.len() < 2 {
        return Ok(None);
    }
    let (fin, opcode, masked) = (buf[0] & 0x80 != 0, buf[0] & 0x0f, buf[1] & 0x80 != 0);
    let (len, mut at) = match buf[1] & 0x7f {
        126 if buf.len() >= 4 => (u16::from_be_bytes([buf[2], buf[3]]) as u64, 4),
        127 if buf.len() >= 10 => (u64::from_be_bytes(buf[2..10].try_into().unwrap()), 10),
        126 | 127 => return Ok(None),
        n => (n as u64, 2),
    };
    if len > MAX_MESSAGE as u64 {
        bail!("a WebSocket frame of {len} bytes");
    }
    let mask = if masked {
        if buf.len() < at + 4 {
            return Ok(None);
        }
        at += 4;
        Some([buf[at - 4], buf[at - 3], buf[at - 2], buf[at - 1]])
    } else {
        None
    };
    let end = at + len as usize;
    if buf.len() < end {
        return Ok(None);
    }
    let mut payload = buf[at..end].to_vec();
    if let Some(m) = mask {
        payload.iter_mut().enumerate().for_each(|(i, b)| *b ^= m[i % 4]);
    }
    Ok(Some((opcode, fin, payload, end)))
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

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

    #[test]
    fn frames_round_trip_at_every_length_form() {
        for len in [0usize, 125, 126, 65535, 65536, 300_000] {
            let payload: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let frame = encode_frame(0x1, &payload, [1, 2, 3, 4]);
            let (op, fin, got, used) = parse_frame(&frame).unwrap().unwrap();
            assert_eq!((op, fin, used), (0x1, true, frame.len()));
            assert_eq!(got, payload);
            // Half a frame is not a frame yet.
            assert!(parse_frame(&frame[..frame.len() - 1]).unwrap().is_none() || len == 0);
        }
    }

    #[test]
    fn reads_unmasked_server_frames() {
        let mut frame = vec![0x81, 126, 0x01, 0x00];
        frame.extend(std::iter::repeat_n(b'x', 256));
        frame.extend_from_slice(&[0x89, 0]);
        let (op, _, payload, used) = parse_frame(&frame).unwrap().unwrap();
        assert_eq!((op, payload.len(), used), (1, 256, 260));
        assert_eq!(parse_frame(&frame[used..]).unwrap().unwrap().0, 0x9);
    }
}
