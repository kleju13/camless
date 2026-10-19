//! A moving background a page hands over, which camless plays under the page.
//!
//! Most of what a browser costs on the LCD is redrawing and capturing the whole page
//! for every frame of a moving background. A page that knows camless can bake its
//! background into a loop of frames once, hand it over through
//! `window.nzxt.v1.camless.background`, and from then on draw only what lies over it,
//! on a transparent page. camless plays the loop and lays the page's latest frame over
//! it, so the browser repaints only when the page itself changes.

use crate::{cdp, q565, web};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::time::Duration;

/// The DevTools binding the page's messages arrive through.
pub const BINDING: &str = "__camless";
/// 20 s at 30 fps; as Q565, a loop of 640×640 frames this long takes a few tens of MB.
const MAX_FRAMES: usize = 600;
const MAX_BYTES: usize = 64 << 20;
const MAX_RATE: f64 = 8.0;

/// `window.nzxt.v1.camless`, run after CAM's `window.nzxt.v1`. Each call resolves once
/// camless has taken the message in, so a page sending frames waits for camless.
pub const BRIDGE: &str = r#"(() => {
  const send = window.__camless;
  if (typeof send !== "function") return;
  let seq = 0;
  const waiting = new Map();
  const call = (op, args) => new Promise((resolve, reject) => {
    const id = ++seq;
    waiting.set(id, { resolve, reject });
    send(JSON.stringify(Object.assign({}, args, { id, op })));
  });
  window.nzxt.v1.camless = {
    version: 1,
    background: {
      begin: (fps, frames) => call("begin", { fps, frames }),
      frame: (index, png) => call("frame", { index, png }),
      end: () => call("end"),
      clear: () => call("clear"),
      speed: (rate) => call("speed", { rate }),
    },
    _reply(id, error) {
      const w = waiting.get(id);
      if (!w) return;
      waiting.delete(id);
      if (error) w.reject(new Error(error));
      else w.resolve();
    },
  };
})();"#;

/// The answer to message `id`, as an expression for the page.
pub fn reply(id: u64, error: Option<&str>) -> String {
    format!("window.nzxt && window.nzxt.v1.camless && window.nzxt.v1.camless._reply({id}, {})", json!(error))
}

/// What a message changed for the browser, which draws the page on black while there
/// is no loop and on nothing while there is one.
#[derive(Debug, PartialEq)]
pub enum Change {
    None,
    Started,
    Stopped,
}

struct Loading {
    fps: f64,
    frames: Vec<Option<Vec<u8>>>,
    bytes: usize,
}

struct Loop {
    fps: f64,
    /// Q565 at the screen's size, deflated: about 40 % smaller, and 0.6 ms to inflate.
    frames: Vec<Vec<u8>>,
}

pub struct Backdrop {
    loading: Option<Loading>,
    playing: Option<Loop>,
    rate: f64,
    /// The page's latest frame as premultiplied RGBA, laid over the loop.
    over: Option<Vec<u8>>,
    over_changed: bool,
    /// Where in the loop, in frames, and which frame went out last.
    pos: f64,
    shown: Option<usize>,
}

impl Default for Backdrop {
    fn default() -> Self {
        Backdrop { loading: None, playing: None, rate: 1.0, over: None, over_changed: false, pos: 0.0, shown: None }
    }
}

impl Backdrop {
    pub fn active(&self) -> bool {
        self.playing.is_some()
    }

    /// Frames in the loop being played; 0 without one.
    pub fn frames(&self) -> usize {
        self.playing.as_ref().map_or(0, |l| l.frames.len())
    }

    /// One message from the page: `{ "op": "begin" | "frame" | "end" | "clear" | "speed", ... }`.
    pub fn message(&mut self, msg: &Value, w: u16, h: u16) -> Result<Change> {
        match msg["op"].as_str().unwrap_or_default() {
            "begin" => {
                let fps = msg["fps"].as_f64().filter(|f| (1.0..=60.0).contains(f)).context("fps must be 1 to 60")?;
                let n = msg["frames"]
                    .as_u64()
                    .filter(|n| (1..=MAX_FRAMES as u64).contains(n))
                    .with_context(|| format!("frames must be 1 to {MAX_FRAMES}"))?;
                self.loading = Some(Loading { fps, frames: vec![None; n as usize], bytes: 0 });
                Ok(Change::None)
            }
            "frame" => {
                let loading = self.loading.as_mut().context("a frame before begin")?;
                let i = msg["index"].as_u64().map(|i| i as usize).filter(|&i| i < loading.frames.len()).context("no frame with that index")?;
                let data = msg["png"].as_str().unwrap_or_default();
                let png = cdp::base64_decode(data.strip_prefix("data:image/png;base64,").context("the frame is not a PNG data URL")?)?;
                let mut frame = miniz_oxide::deflate::compress_to_vec(&q565::encode(w, h, &web::rgb565_from_png(&png, w, h)?), 6);
                frame.shrink_to_fit();
                loading.bytes += frame.len();
                loading.bytes -= loading.frames[i].replace(frame).map_or(0, |old| old.len());
                if loading.bytes > MAX_BYTES {
                    self.loading = None;
                    bail!("the background takes over {} MB", MAX_BYTES >> 20);
                }
                Ok(Change::None)
            }
            "end" => {
                let loading = self.loading.take().context("end before begin")?;
                let frames = loading.frames.into_iter().collect::<Option<Vec<_>>>().context("frames are missing")?;
                let started = self.playing.is_none();
                self.playing = Some(Loop { fps: loading.fps, frames });
                (self.pos, self.shown) = (0.0, None);
                Ok(if started { Change::Started } else { Change::None })
            }
            "clear" => Ok(if self.clear() { Change::Stopped } else { Change::None }),
            "speed" => {
                self.rate = msg["rate"].as_f64().filter(|r| (0.0..=MAX_RATE).contains(r)).with_context(|| format!("rate must be 0 to {MAX_RATE}"))?;
                Ok(Change::None)
            }
            op => bail!("unknown op {op:?}"),
        }
    }

    /// Back to the page drawing everything; true if a loop was playing.
    pub fn clear(&mut self) -> bool {
        self.loading = None;
        (self.over, self.shown, self.rate) = (None, None, 1.0);
        self.playing.take().is_some()
    }

    /// The page's latest frame, a PNG from the screencast.
    pub fn page_frame(&mut self, png: &[u8], w: u16, h: u16) -> Result<()> {
        self.over = Some(premultiplied_from_png(png, w, h)?);
        self.over_changed = true;
        Ok(())
    }

    /// The frame for the screen `elapsed` after the last one, or None if it would be
    /// the same as the last.
    pub fn next(&mut self, elapsed: Duration) -> Option<Vec<u8>> {
        let l = self.playing.as_ref()?;
        let n = l.frames.len();
        self.pos = (self.pos + elapsed.as_secs_f64() * l.fps * self.rate) % n as f64;
        let i = (self.pos as usize).min(n - 1);
        if self.shown == Some(i) && !self.over_changed {
            return None;
        }
        let (w, h, mut pixels) = q565::decode(&miniz_oxide::inflate::decompress_to_vec(&l.frames[i]).ok()?)?;
        if let Some(over) = &self.over {
            compose(&mut pixels, over);
        }
        (self.shown, self.over_changed) = (Some(i), false);
        Some(q565::encode(w, h, &pixels))
    }
}

/// A PNG as premultiplied RGBA of `w`×`h`; anything outside it is transparent.
/// Chromium's screencast PNGs carry premultiplied colour already, though rounding puts
/// some colours a little above their alpha: by up to 9 on a KrakenDial frame. One with
/// straight alpha has a colour far above its alpha somewhere, and is premultiplied here.
pub fn premultiplied_from_png(png_bytes: &[u8], w: u16, h: u16) -> Result<Vec<u8>> {
    const ROUNDING: u8 = 32;
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().context("reading a frame")?;
    let mut buf = vec![0; reader.output_buffer_size().context("a frame too large")?];
    let info = reader.next_frame(&mut buf).context("reading a frame")?;
    let channels = info.color_type.samples();
    let (fw, fh, w, h) = (info.width as usize, info.height as usize, w as usize, h as usize);
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..fh.min(h) {
        for x in 0..fw.min(w) {
            let px = &buf[(y * fw + x) * channels..][..channels];
            let out = match channels {
                4 => [px[0], px[1], px[2], px[3]],
                3 => [px[0], px[1], px[2], 255],
                2 => [px[0], px[0], px[0], px[1]],
                _ => [px[0], px[0], px[0], 255],
            };
            rgba[(y * w + x) * 4..][..4].copy_from_slice(&out);
        }
    }
    if rgba.chunks_exact(4).any(|p| p[0].max(p[1]).max(p[2]) > p[3].saturating_add(ROUNDING)) {
        for p in rgba.chunks_exact_mut(4) {
            let a = u16::from(p[3]);
            for c in &mut p[..3] {
                *c = ((u16::from(*c) * a + 127) / 255) as u8;
            }
        }
    }
    Ok(rgba)
}

/// Premultiplied RGBA over RGB565.
pub fn compose(under: &mut [u16], over: &[u8]) {
    for (u, o) in under.iter_mut().zip(over.chunks_exact(4)) {
        let a = u32::from(o[3]);
        if a == 0 {
            continue;
        }
        let (r, g, b) = (u32::from(*u >> 11), u32::from(*u >> 5 & 63), u32::from(*u & 31));
        let low = [r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2];
        let [r, g, b] = [0, 1, 2].map(|c| (u32::from(o[c]) + (low[c] * (255 - a) + 127) / 255).min(255));
        *u = ((r >> 3) << 11 | (g >> 2) << 5 | (b >> 3)) as u16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, color: png::ColorType, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(color);
        enc.write_header().unwrap().write_image_data(data).unwrap();
        out
    }

    fn data_url(bytes: &[u8]) -> String {
        format!("data:image/png;base64,{}", cdp::base64_encode(bytes))
    }

    /// A 2×1 loop: frame 0 red, frame 1 blue.
    fn two_frames(b: &mut Backdrop) {
        let red = png(2, 1, png::ColorType::Rgb, &[255, 0, 0, 255, 0, 0]);
        let blue = png(2, 1, png::ColorType::Rgb, &[0, 0, 255, 0, 0, 255]);
        assert_eq!(b.message(&json!({ "op": "begin", "fps": 10, "frames": 2 }), 2, 1).unwrap(), Change::None);
        b.message(&json!({ "op": "frame", "index": 1, "png": data_url(&blue) }), 2, 1).unwrap();
        b.message(&json!({ "op": "frame", "index": 0, "png": data_url(&red) }), 2, 1).unwrap();
        assert_eq!(b.message(&json!({ "op": "end" }), 2, 1).unwrap(), Change::Started);
    }

    #[test]
    fn the_bridge_uses_the_binding() {
        assert!(BRIDGE.contains(&format!("window.{BINDING}")));
        assert_eq!(reply(3, None), "window.nzxt && window.nzxt.v1.camless && window.nzxt.v1.camless._reply(3, null)");
        assert!(reply(4, Some("a \"bad\" frame")).ends_with(r#"_reply(4, "a \"bad\" frame")"#));
    }

    #[test]
    fn plays_the_loop_at_its_rate() {
        let mut b = Backdrop::default();
        two_frames(&mut b);
        assert!(b.active() && b.frames() == 2);
        let frame = |q: Option<Vec<u8>>| q565::decode(&q.unwrap()).unwrap().2;
        assert_eq!(frame(b.next(Duration::ZERO)), vec![0xf800; 2]);
        // the same frame again is not sent again
        assert_eq!(b.next(Duration::from_millis(50)), None);
        assert_eq!(frame(b.next(Duration::from_millis(60))), vec![0x001f; 2]);
        assert_eq!(frame(b.next(Duration::from_millis(100))), vec![0xf800; 2]);
        // twice as fast skips a frame each time: 10 fps × 2 × 0.1 s = 2 frames
        b.message(&json!({ "op": "speed", "rate": 2 }), 2, 1).unwrap();
        assert_eq!(b.next(Duration::from_millis(100)), None);
        b.message(&json!({ "op": "speed", "rate": 1 }), 2, 1).unwrap();
        assert_eq!(frame(b.next(Duration::from_millis(100))), vec![0x001f; 2]);
    }

    #[test]
    fn lays_the_page_over_the_loop() {
        let mut b = Backdrop::default();
        two_frames(&mut b);
        // left pixel opaque white, right one transparent
        b.page_frame(&png(2, 1, png::ColorType::Rgba, &[255, 255, 255, 255, 0, 0, 0, 0]), 2, 1).unwrap();
        let (_, _, px) = q565::decode(&b.next(Duration::ZERO).unwrap()).unwrap();
        assert_eq!(px, vec![0xffff, 0xf800]);
    }

    #[test]
    fn refuses_what_is_not_a_whole_loop() {
        let mut b = Backdrop::default();
        let msg = |b: &mut Backdrop, v: Value| b.message(&v, 2, 1).map_err(|e| e.to_string());
        assert!(msg(&mut b, json!({ "op": "frame", "index": 0, "png": "" })).unwrap_err().contains("before begin"));
        assert!(msg(&mut b, json!({ "op": "begin", "fps": 10, "frames": 0 })).is_err());
        assert!(msg(&mut b, json!({ "op": "begin", "fps": 0, "frames": 2 })).is_err());
        assert!(msg(&mut b, json!({ "op": "begin", "fps": 10, "frames": MAX_FRAMES + 1 })).is_err());
        msg(&mut b, json!({ "op": "begin", "fps": 10, "frames": 2 })).unwrap();
        assert!(msg(&mut b, json!({ "op": "frame", "index": 2, "png": "" })).unwrap_err().contains("index"));
        assert!(msg(&mut b, json!({ "op": "frame", "index": 0, "png": "data:image/jpeg;base64,AAAA" })).unwrap_err().contains("PNG"));
        assert!(msg(&mut b, json!({ "op": "end" })).unwrap_err().contains("missing"));
        assert!(!b.active());
        assert!(msg(&mut b, json!({ "op": "speed", "rate": 9 })).is_err());
        assert!(msg(&mut b, json!({ "op": "spin" })).unwrap_err().contains("unknown"));
    }

    #[test]
    fn clearing_gives_the_page_the_screen_back() {
        let mut b = Backdrop::default();
        assert_eq!(b.message(&json!({ "op": "clear" }), 2, 1).unwrap(), Change::None);
        two_frames(&mut b);
        // a new loop over a playing one changes nothing for the browser
        two_frames_again(&mut b);
        assert_eq!(b.message(&json!({ "op": "clear" }), 2, 1).unwrap(), Change::Stopped);
        assert!(!b.active() && b.next(Duration::ZERO).is_none());
    }

    fn two_frames_again(b: &mut Backdrop) {
        let red = png(2, 1, png::ColorType::Rgb, &[255, 0, 0, 255, 0, 0]);
        b.message(&json!({ "op": "begin", "fps": 10, "frames": 1 }), 2, 1).unwrap();
        b.message(&json!({ "op": "frame", "index": 0, "png": data_url(&red) }), 2, 1).unwrap();
        assert_eq!(b.message(&json!({ "op": "end" }), 2, 1).unwrap(), Change::None);
        assert_eq!(b.frames(), 1);
    }

    #[test]
    fn straight_alpha_is_premultiplied() {
        // half-transparent red: straight, then as Chromium's screencast has it
        let straight = premultiplied_from_png(&png(1, 1, png::ColorType::Rgba, &[255, 0, 0, 128]), 2, 1).unwrap();
        let premultiplied = premultiplied_from_png(&png(1, 1, png::ColorType::Rgba, &[128, 0, 0, 128]), 2, 1).unwrap();
        assert_eq!(straight, vec![128, 0, 0, 128, 0, 0, 0, 0]);
        assert_eq!(straight, premultiplied);
        // a premultiplied pixel rounded a little above its alpha is not premultiplied again
        let rounded = premultiplied_from_png(&png(1, 1, png::ColorType::Rgba, &[70, 73, 85, 84]), 1, 1).unwrap();
        assert_eq!(rounded, vec![70, 73, 85, 84]);
        // half red over blue
        let mut under = vec![0x001f];
        compose(&mut under, &straight[..4]);
        let (r, g, b) = (under[0] >> 11, under[0] >> 5 & 63, under[0] & 31);
        assert_eq!((r, g, b), (16, 0, 15));
    }
}
