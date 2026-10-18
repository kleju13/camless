//! Q565, the compressed RGB565 format the Kraken LCD takes for streamed frames.
//!
//! Q565 is QOI adapted to RGB565 (reference implementation: the `q565` crate,
//! seritools/q565). CAM sends every frame this way with image format 0x08; the
//! device accepts raw RGB565 transfers but never shows them.
//!
//! Stream: `q565`, u16 LE width, u16 LE height, ops, then 0xFF. Ops, by tag:
//! - `00iiiiii` the colour array entry `i`;
//! - `01rrggbb` the previous pixel plus a -2..1 difference per channel;
//! - `100ggggg rrrrbbbb` the previous pixel plus a -16..15 green difference, with
//!   red and blue given as -8..7 offsets from that green difference;
//! - `101gggrr bbiiiiii` colour array entry `i` plus a small difference;
//! - `11nnnnnn` the previous pixel repeated n + 1 times (1..62);
//! - `0xFE lo hi` a raw RGB565 pixel.
//! The colour array has 64 entries; a pixel's slot is the sum of its two bytes
//! mod 64. Pixels from the one-byte ops are not added to it.

const OP_DIFF: u8 = 0b0100_0000;
const OP_LUMA: u8 = 0b1000_0000;
const OP_DIFF_INDEXED: u8 = 0b1010_0000;
const OP_RUN: u8 = 0b1100_0000;
const OP_RGB565: u8 = 0xfe;
const OP_END: u8 = 0xff;
const MAX_RUN: usize = 62;

fn slot(pixel: u16) -> usize {
    let [a, b] = pixel.to_le_bytes();
    usize::from(a.wrapping_add(b) & 63)
}

fn split(pixel: u16) -> [u8; 3] {
    [(pixel >> 11) as u8, ((pixel >> 5) & 63) as u8, (pixel & 31) as u8]
}

/// Signed difference of two `bits`-wide channel values, wrapping like the device does.
fn diff(a: u8, b: u8, bits: u32) -> i8 {
    (a.wrapping_sub(b) as i8) << (8 - bits) >> (8 - bits)
}

/// Encodes `pixels` (row-major RGB565) the same way as the reference encoder.
pub fn encode(width: u16, height: u16, pixels: &[u16]) -> Vec<u8> {
    assert_eq!(pixels.len(), usize::from(width) * usize::from(height), "pixel count does not match the size");
    let mut out = Vec::with_capacity(pixels.len() / 2);
    out.extend_from_slice(b"q565");
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());

    let mut prev = 0u16;
    let mut arr = [0u16; 64];
    let mut i = 0;
    while i < pixels.len() {
        let pixel = pixels[i];
        if pixel == prev {
            let run = pixels[i..].iter().take_while(|&&p| p == prev).count();
            i += run;
            for _ in 0..run / MAX_RUN {
                out.push(OP_RUN | (MAX_RUN as u8 - 1));
            }
            if run % MAX_RUN > 0 {
                out.push(OP_RUN | (run % MAX_RUN) as u8 - 1);
            }
            continue;
        }
        i += 1;

        let [r, g, b] = split(pixel);
        let [pr, pg, pb] = split(prev);
        prev = pixel;
        let s = slot(pixel);
        if arr[s] == pixel {
            out.push(s as u8);
            continue;
        }

        let (dr, dg, db) = (diff(r, pr, 5), diff(g, pg, 6), diff(b, pb, 5));
        if (-2..=1).contains(&dr) && (-2..=1).contains(&dg) && (-2..=1).contains(&db) {
            out.push(OP_DIFF | ((dr + 2) as u8) << 4 | ((dg + 2) as u8) << 2 | (db + 2) as u8);
            continue;
        }
        let (drg, dbg) = (dr - dg, db - dg);
        if (-8..=7).contains(&drg) && (-16..=15).contains(&dg) && (-8..=7).contains(&dbg) {
            out.extend_from_slice(&[OP_LUMA | (dg + 16) as u8, ((drg + 8) as u8) << 4 | (dbg + 8) as u8]);
        } else if let Some(op) = arr.iter().enumerate().find_map(|(j, &entry)| {
            let [ar, ag, ab] = split(entry);
            let (dr, dg, db) = (diff(r, ar, 5), diff(g, ag, 6), diff(b, ab, 5));
            ((-2..=1).contains(&dr) && (-4..=3).contains(&dg) && (-2..=1).contains(&db))
                .then(|| [OP_DIFF_INDEXED | ((dg + 4) as u8) << 2 | (dr + 2) as u8, ((db + 2) as u8) << 6 | j as u8])
        }) {
            out.extend_from_slice(&op);
        } else {
            let [lo, hi] = pixel.to_le_bytes();
            out.extend_from_slice(&[OP_RGB565, lo, hi]);
        }
        arr[s] = pixel;
    }
    out.push(OP_END);
    out
}

/// RGBA bytes to RGB565, dropping the low bits of each channel.
pub fn rgb565_from_rgba(rgba: &[u8]) -> Vec<u16> {
    rgba.chunks_exact(4)
        .map(|px| {
            let (r, g, b) = (px[0] as u16, px[1] as u16, px[2] as u16);
            ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)
        })
        .collect()
}

/// The inverse of `encode`, for checking what a screen will show.
pub fn decode(data: &[u8]) -> Option<(u16, u16, Vec<u16>)> {
    fn add(pixel: u16, dr: i8, dg: i8, db: i8) -> u16 {
        let [r, g, b] = split(pixel);
        let (r, g, b) = (r.wrapping_add_signed(dr) & 31, g.wrapping_add_signed(dg) & 63, b.wrapping_add_signed(db) & 31);
        u16::from(r) << 11 | u16::from(g) << 5 | u16::from(b)
    }

    if data.get(..4)? != b"q565" {
        return None;
    }
    let width = u16::from_le_bytes([data[4], data[5]]);
    let height = u16::from_le_bytes([data[6], data[7]]);
    let mut bytes = data[8..].iter().copied();
    let mut out = Vec::new();
    let (mut prev, mut arr) = (0u16, [0u16; 64]);
    loop {
        let op = bytes.next()?;
        let pixel = match op >> 6 {
            0b00 => {
                prev = arr[usize::from(op)];
                out.push(prev);
                continue;
            }
            0b01 => {
                prev = add(prev, (op >> 4 & 3) as i8 - 2, (op >> 2 & 3) as i8 - 2, (op & 3) as i8 - 2);
                out.push(prev);
                continue;
            }
            0b10 => {
                let next = bytes.next()?;
                if op & 0x20 == 0 {
                    let dg = (op & 31) as i8 - 16;
                    add(prev, (next >> 4) as i8 - 8 + dg, dg, (next & 15) as i8 - 8 + dg)
                } else {
                    add(arr[usize::from(next & 63)], (op & 3) as i8 - 2, (op >> 2 & 7) as i8 - 4, (next >> 6) as i8 - 2)
                }
            }
            _ if op == OP_END => break,
            _ if op == OP_RGB565 => u16::from_le_bytes([bytes.next()?, bytes.next()?]),
            _ => {
                out.extend(std::iter::repeat_n(prev, usize::from(op & 63) + 1));
                continue;
            }
        };
        arr[slot(pixel)] = pixel;
        prev = pixel;
        out.push(pixel);
    }
    (bytes.next().is_none() && out.len() == usize::from(width) * usize::from(height)).then_some((width, height, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame CAM sent to the cooler (its white loading screen), captured over USB.
    const CAM_FRAME: &[u8] = include_bytes!("../tests/data/cam_frame_white.q565");

    #[test]
    fn decodes_and_reproduces_a_cam_frame() {
        let (w, h, pixels) = decode(CAM_FRAME).expect("CAM frame decodes");
        assert_eq!((w, h), (640, 640));
        assert_eq!(encode(w, h, &pixels), CAM_FRAME);
    }

    #[test]
    fn round_trips_every_op() {
        // Runs, small and large differences, array hits and raw pixels.
        let mut pixels = vec![0u16; 200];
        let mut x = 0x1234_5678u32;
        for (i, p) in pixels.iter_mut().enumerate() {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *p = match i % 5 {
                0 => x as u16,
                1 => 0xffff,
                2 => 0xffff,
                3 => 0xf7de,
                _ => (x as u16) & 0x0841,
            };
        }
        pixels.extend(std::iter::repeat_n(0x07e0, 130));
        pixels.extend([0x07e0 ^ 0x0821, 0xf800]);
        let len = pixels.len() as u16;
        assert_eq!(decode(&encode(len, 1, &pixels)), Some((len, 1, pixels)));
    }
}
