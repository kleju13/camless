//! Byte-level protocol for the NZXT Kraken 2023/2024 family and the Control Hub.
//!
//! Sources: liquidctl `kraken3.py` and `control_hub.py`, the Linux `nzxt-kraken3`
//! driver, and the firmware images embedded in NZXT CAM's `index.node`.
//! Every builder here returns the report payload without the leading report ID.

pub const NZXT_VID: u16 = 0x1e71;
pub const KRAKEN_ELITE_V2_PID: u16 = 0x3012;
pub const CONTROL_HUB_PID: u16 = 0x2022;

pub const REPORT_LEN: usize = 64;

/// The Kraken evaluates its on-device curves for liquid temperatures 20..=59 °C.
pub const KRAKEN_CURVE_FIRST_C: u8 = 20;
pub const KRAKEN_CURVE_POINTS: usize = 40;
pub const KRAKEN_PUMP_MIN_DUTY: u8 = 20;

pub const HUB_FAN_CHANNELS: usize = 5;

/// Magic that starts every bulk transfer to the LCD. The same bytes appear in the
/// device firmware images CAM ships, so the device checks for them.
pub const LCD_BULK_MAGIC: [u8; 12] = [
    0x12, 0xfa, 0x01, 0xe8, 0xab, 0xcd, 0xef, 0x98, 0x76, 0x54, 0x32, 0x10,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KrakenChannel {
    Pump,
    Fan,
}

impl KrakenChannel {
    fn id(self) -> [u8; 3] {
        match self {
            KrakenChannel::Pump => [0x01, 0x01, 0x00],
            KrakenChannel::Fan => [0x02, 0x01, 0x01],
        }
    }

    pub fn min_duty(self) -> u8 {
        match self {
            KrakenChannel::Pump => KRAKEN_PUMP_MIN_DUTY,
            KrakenChannel::Fan => 0,
        }
    }
}

pub fn kraken_init() -> Vec<Vec<u8>> {
    // 0xB8 with interval 1 asks for a status report every 0.5 s.
    vec![vec![0x70, 0x02, 0x01, 0xb8, 0x01], vec![0x70, 0x01]]
}

pub fn kraken_firmware_request() -> Vec<u8> {
    vec![0x10, 0x01]
}

pub fn kraken_status_request() -> Vec<u8> {
    vec![0x74, 0x01]
}

pub fn kraken_lcd_info_request() -> Vec<u8> {
    vec![0x30, 0x01]
}

/// One duty per liquid temperature from 20 to 59 °C. The firmware runs this curve
/// by itself, so it keeps working when no host software is running.
pub fn kraken_curve(channel: KrakenChannel, duties: &[u8; KRAKEN_CURVE_POINTS]) -> Vec<u8> {
    let mut msg = vec![0x72];
    msg.extend_from_slice(&channel.id());
    let min = channel.min_duty();
    msg.extend(duties.iter().map(|&d| d.clamp(min, 100)));
    msg
}

pub fn kraken_lcd_settings(brightness: u8, orientation_quarter_turns: u8) -> Vec<u8> {
    vec![0x30, 0x02, 0x01, brightness.min(100), 0x00, 0x00, 0x01, orientation_quarter_turns & 3]
}

/// Shows the built-in liquid temperature screen, drawn by the device itself.
pub fn kraken_lcd_liquid_mode() -> Vec<u8> {
    vec![0x38, 0x01, 0x02, 0x00]
}

/// Image format of streamed frames. cam-core's list: GIF image 0, GIF animation 1,
/// RGBA 2, …, RGB565 BE 6, RGB565 LE 7, Q565 8. The cooler accepts raw RGB565
/// transfers but shows only Q565 ones, which is also all CAM sends.
pub const LCD_FORMAT_Q565: u8 = 0x08;

/// Starts a frame transfer into the streaming buffer. The screen switches to the
/// stream by itself once a frame arrives; CAM sends no display mode command.
pub fn kraken_lcd_stream_begin() -> Vec<u8> {
    vec![0x36, 0x01, 0x00, 0x01, LCD_FORMAT_Q565]
}

pub fn kraken_lcd_transfer_end() -> Vec<u8> {
    vec![0x36, 0x02]
}

/// Cancels a transfer an earlier process left open (liquidctl#932).
pub fn kraken_lcd_transfer_cancel() -> Vec<u8> {
    vec![0x36, 0x03]
}

/// Header of a bulk transfer; `kind` is the image format, as in `kraken_lcd_stream_begin`.
pub fn lcd_bulk_header(kind: u8, len: u32) -> Vec<u8> {
    let mut h = LCD_BULK_MAGIC.to_vec();
    h.extend_from_slice(&[kind, 0, 0, 0]);
    h.extend_from_slice(&len.to_le_bytes());
    h
}

pub fn hub_init() -> Vec<Vec<u8>> {
    // Same 0.5 s report interval as the Kraken, for both fan report slots.
    vec![
        vec![0x60, 0x02, 0x01, 0xe8, 0x01, 0x01, 0xe8, 0x01],
        vec![0x66, 0x01],
    ]
}

pub fn hub_firmware_request() -> Vec<u8> {
    vec![0x10, 0x02]
}

/// Sets every fan channel at once. `None` leaves a channel out of the mask.
pub fn hub_fixed_duties(duties: &[Option<u8>; HUB_FAN_CHANNELS]) -> Vec<u8> {
    let mut mask = 0u8;
    let mut msg = vec![0x62, 0x01, 0x00, 0, 0, 0, 0, 0];
    for (i, d) in duties.iter().enumerate() {
        if let Some(d) = d {
            mask |= 1 << i;
            msg[3 + i] = (*d).min(100);
        }
    }
    msg[2] = mask;
    msg
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct KrakenStatus {
    pub liquid_c: f32,
    pub pump_rpm: u16,
    pub pump_duty: u8,
    pub fan_rpm: u16,
    pub fan_duty: u8,
}

/// Parses `75 01` (reply to a status request) and `75 02` (periodic report).
pub fn parse_kraken_status(msg: &[u8]) -> Option<KrakenStatus> {
    if msg.len() < 26 || msg[0] != 0x75 {
        return None;
    }
    if msg[15] == 0xff && msg[16] == 0xff {
        return None;
    }
    Some(KrakenStatus {
        liquid_c: msg[15] as f32 + msg[16] as f32 / 10.0,
        pump_rpm: u16::from_le_bytes([msg[17], msg[18]]),
        pump_duty: msg[19],
        fan_rpm: u16::from_le_bytes([msg[23], msg[24]]),
        fan_duty: msg[25],
    })
}

pub fn parse_kraken_firmware(msg: &[u8]) -> Option<(u8, u8, u8)> {
    (msg.len() > 19 && msg[0] == 0x11 && msg[1] == 0x01).then(|| (msg[17], msg[18], msg[19]))
}

#[derive(Clone, Debug, PartialEq)]
pub struct LcdInfo {
    pub brightness: u8,
    pub orientation_quarter_turns: u8,
}

pub fn parse_kraken_lcd_info(msg: &[u8]) -> Option<LcdInfo> {
    (msg.len() > 0x1a && msg[0] == 0x31 && msg[1] == 0x01).then(|| LcdInfo {
        brightness: msg[0x18],
        orientation_quarter_turns: msg[0x1a],
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FanMode {
    None,
    Dc,
    Pwm,
    Unknown(u8),
}

#[derive(Clone, Debug, PartialEq)]
pub struct HubFan {
    pub mode: FanMode,
    pub rpm: u16,
    pub duty: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HubStatus {
    pub fans: Vec<HubFan>,
    pub noise_db: u8,
}

/// Parses the periodic `67 02` fan report.
pub fn parse_hub_status(msg: &[u8]) -> Option<HubStatus> {
    if msg.len() < 57 || msg[0] != 0x67 || msg[1] != 0x02 {
        return None;
    }
    let fans = (0..HUB_FAN_CHANNELS)
        .map(|i| HubFan {
            mode: match msg[16 + i] {
                0 => FanMode::None,
                1 => FanMode::Dc,
                2 => FanMode::Pwm,
                m => FanMode::Unknown(m),
            },
            rpm: u16::from_le_bytes([msg[24 + 2 * i], msg[25 + 2 * i]]),
            duty: msg[40 + i],
        })
        .collect();
    Some(HubStatus { fans, noise_db: msg[56] })
}

pub fn parse_hub_firmware(msg: &[u8]) -> Option<(u8, u8, u8)> {
    (msg.len() > 0x13 && msg[0] == 0x11 && msg[1] == 0x02).then(|| (msg[0x11], msg[0x12], msg[0x13]))
}

pub type Rgb = [u8; 3];

/// LEDs on the Kraken Elite V2 ring and on a NZXT fan; CAM sends 24 colors per channel.
pub const CHANNEL_LEDS: usize = 24;

/// Lighting channel addresses on the Kraken Elite V2 (liquidctl#803, #882).
pub const KRAKEN_RING: u8 = 0b001;
pub const KRAKEN_EXTERNAL: u8 = 0b010;

/// Control Hub LED ports 1..=5 (liquidctl `control_hub.py`, after its mapping fix).
pub const HUB_LED_PORTS: [u8; 5] = [0x02, 0x01, 0x04, 0x08, 0x10];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speed {
    Slowest,
    Slower,
    Normal,
    Faster,
    Fastest,
}

impl Speed {
    pub fn parse(s: &str) -> Option<Speed> {
        Some(match s.to_ascii_lowercase().as_str() {
            "slowest" => Speed::Slowest,
            "slower" => Speed::Slower,
            "normal" => Speed::Normal,
            "faster" => Speed::Faster,
            "fastest" => Speed::Fastest,
            _ => return None,
        })
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// Animated effects in the Hue 2 `2A 04` layout that both devices accept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Fading,
    SpectrumWave,
    CoveringMarquee,
    Pulse,
    Breathing,
    Candle,
    RainbowFlow,
    SuperRainbow,
    RainbowPulse,
    TaiChi,
    WaterCooler,
}

impl Effect {
    pub fn parse(s: &str) -> Option<Effect> {
        let s: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        Some(match s.to_ascii_lowercase().as_str() {
            "fading" => Effect::Fading,
            "spectrumwave" => Effect::SpectrumWave,
            "coveringmarquee" => Effect::CoveringMarquee,
            "pulse" => Effect::Pulse,
            "breathing" => Effect::Breathing,
            "candle" => Effect::Candle,
            "rainbowflow" => Effect::RainbowFlow,
            "superrainbow" => Effect::SuperRainbow,
            "rainbowpulse" => Effect::RainbowPulse,
            "taichi" => Effect::TaiChi,
            "watercooler" => Effect::WaterCooler,
            _ => return None,
        })
    }

    /// (mode byte, speed scale, min colors, max colors), from liquidctl's `_COLOR_MODES`.
    fn params(self) -> (u8, usize, usize, usize) {
        match self {
            Effect::Fading => (0x01, 1, 1, 8),
            Effect::SpectrumWave => (0x02, 2, 0, 0),
            Effect::CoveringMarquee => (0x04, 2, 1, 8),
            Effect::Pulse => (0x06, 5, 1, 8),
            Effect::Breathing => (0x07, 6, 1, 8),
            Effect::Candle => (0x08, 0, 1, 1),
            Effect::RainbowFlow => (0x0b, 2, 0, 0),
            Effect::SuperRainbow => (0x0c, 2, 0, 0),
            Effect::RainbowPulse => (0x0d, 2, 0, 0),
            Effect::TaiChi => (0x0e, 7, 1, 2),
            Effect::WaterCooler => (0x0f, 6, 2, 2),
        }
    }

    pub fn color_range(self) -> (usize, usize) {
        let (_, _, min, max) = self.params();
        (min, max)
    }
}

/// Speed bytes per scale, liquidctl's `_SPEED_VALUE`.
const SPEED_VALUE: [[[u8; 2]; 5]; 8] = [
    [[0x32, 0x00]; 5],
    [[0x50, 0x00], [0x3c, 0x00], [0x28, 0x00], [0x14, 0x00], [0x0a, 0x00]],
    [[0x5e, 0x01], [0x2c, 0x01], [0xfa, 0x00], [0x96, 0x00], [0x50, 0x00]],
    [[0x40, 0x06], [0x14, 0x05], [0xe8, 0x03], [0x20, 0x03], [0x58, 0x02]],
    [[0x20, 0x03], [0xbc, 0x02], [0xf4, 0x01], [0x90, 0x01], [0x2c, 0x01]],
    [[0x19, 0x00], [0x14, 0x00], [0x0f, 0x00], [0x07, 0x00], [0x04, 0x00]],
    [[0x28, 0x00], [0x1e, 0x00], [0x14, 0x00], [0x0a, 0x00], [0x04, 0x00]],
    [[0x32, 0x00], [0x28, 0x00], [0x1e, 0x00], [0x14, 0x00], [0x0a, 0x00]],
];

fn grb(c: &Rgb) -> [u8; 3] {
    [c[1], c[0], c[2]]
}

/// One solid color, the two packets CAM sends (USB capture in liquidctl#882):
/// `26 04 <ch> 00` with 24 GRB triplets, then the `26 06` apply.
pub fn static_color(channel: u8, color: &Rgb) -> Vec<Vec<u8>> {
    let mut colors = vec![0x26, 0x04, channel, 0x00];
    for _ in 0..CHANNEL_LEDS {
        colors.extend_from_slice(&grb(color));
    }
    let apply = vec![
        0x26, 0x06, channel, 0x00, 0x01, 0x00, 0x00, CHANNEL_LEDS as u8, 0x00, 0x00, 0x80, 0x00,
        0x32, 0x00, 0x00, 0x01,
    ];
    vec![colors, apply]
}

/// An animated effect in the `2A 04` layout (liquidctl `KrakenX3._write_colors`),
/// with the 24-LED count CAM uses in place of the X3's per-channel constant.
pub fn effect(channel: u8, effect: Effect, colors: &[Rgb], speed: Speed, backward: bool) -> Vec<u8> {
    let (mode, scale, _min, max) = effect.params();
    let colors: Vec<Rgb> = if max == 0 {
        vec![[0, 0, 0]]
    } else {
        colors.iter().take(max).copied().collect()
    };
    let mut msg = vec![0x2a, 0x04, channel, channel, mode];
    msg.extend_from_slice(&SPEED_VALUE[scale][speed.index()]);
    for i in 0..16 {
        msg.extend_from_slice(&colors.get(i).map(grb).unwrap_or([0, 0, 0]));
    }
    let mut count = colors.len() as u8;
    let direction = if backward { 0x02 } else { 0x00 };
    let backward_byte = match effect {
        Effect::CoveringMarquee => 0x04,
        _ => 0x00,
    } + direction;
    let mode_related = match effect {
        Effect::Fading | Effect::Pulse | Effect::Breathing => 0x08,
        Effect::TaiChi => 0x05,
        Effect::WaterCooler => {
            count = 1;
            0x05
        }
        _ => 0x00,
    };
    msg.extend_from_slice(&[backward_byte, count, mode_related, CHANNEL_LEDS as u8, 0x03]);
    msg
}

/// Converts RGBA pixels to RGB565, red in the top bits.
pub fn rgba_to_rgb565(rgba: &[u8]) -> Vec<u16> {
    rgba.chunks_exact(4)
        .map(|px| {
            let (r, g, b) = (px[0] as u16, px[1] as u16, px[2] as u16);
            ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pump_curve_matches_liquidctl_layout() {
        let msg = kraken_curve(KrakenChannel::Pump, &[10; KRAKEN_CURVE_POINTS]);
        assert_eq!(&msg[..4], &[0x72, 0x01, 0x01, 0x00]);
        assert_eq!(msg.len(), 4 + KRAKEN_CURVE_POINTS);
        // The pump never goes below its minimum.
        assert!(msg[4..].iter().all(|&d| d == KRAKEN_PUMP_MIN_DUTY));
    }

    #[test]
    fn fan_curve_header() {
        let msg = kraken_curve(KrakenChannel::Fan, &[0; KRAKEN_CURVE_POINTS]);
        assert_eq!(&msg[..4], &[0x72, 0x02, 0x01, 0x01]);
        assert!(msg[4..].iter().all(|&d| d == 0));
    }

    #[test]
    fn hub_duties_build_mask() {
        let msg = hub_fixed_duties(&[Some(30), None, Some(100), None, Some(150)]);
        assert_eq!(msg, vec![0x62, 0x01, 0b10101, 30, 0, 100, 0, 100]);
    }

    #[test]
    fn single_hub_channel_matches_liquidctl() {
        // liquidctl: [0x62, 0x01, 1 << cid, 0, 0, 0, 0, 0] with msg[cid + 3] = duty
        let msg = hub_fixed_duties(&[None, None, Some(55), None, None]);
        assert_eq!(msg, vec![0x62, 0x01, 0x04, 0, 0, 55, 0, 0]);
    }

    #[test]
    fn parses_kraken_status() {
        let mut m = vec![0u8; 64];
        m[0] = 0x75;
        m[1] = 0x01;
        m[15] = 31;
        m[16] = 4;
        m[17..19].copy_from_slice(&2150u16.to_le_bytes());
        m[19] = 61;
        m[23..25].copy_from_slice(&980u16.to_le_bytes());
        m[25] = 35;
        let s = parse_kraken_status(&m).unwrap();
        assert!((s.liquid_c - 31.4).abs() < 1e-4);
        assert_eq!((s.pump_rpm, s.pump_duty, s.fan_rpm, s.fan_duty), (2150, 61, 980, 35));
    }

    #[test]
    fn rejects_faulty_temperature() {
        let mut m = vec![0u8; 64];
        m[0] = 0x75;
        m[15] = 0xff;
        m[16] = 0xff;
        assert!(parse_kraken_status(&m).is_none());
    }

    #[test]
    fn parses_hub_status() {
        let mut m = vec![0u8; 64];
        m[0] = 0x67;
        m[1] = 0x02;
        m[16] = 2;
        m[17] = 1;
        m[24..26].copy_from_slice(&1200u16.to_le_bytes());
        m[26..28].copy_from_slice(&800u16.to_le_bytes());
        m[40] = 45;
        m[41] = 30;
        m[56] = 27;
        let s = parse_hub_status(&m).unwrap();
        assert_eq!(s.fans[0], HubFan { mode: FanMode::Pwm, rpm: 1200, duty: 45 });
        assert_eq!(s.fans[1], HubFan { mode: FanMode::Dc, rpm: 800, duty: 30 });
        assert_eq!(s.fans[2].mode, FanMode::None);
        assert_eq!(s.noise_db, 27);
    }

    #[test]
    fn lcd_stream_matches_cam_capture() {
        assert_eq!(kraken_lcd_stream_begin(), vec![0x36, 0x01, 0x00, 0x01, 0x08]);
        // 12 fa 01 e8 ab cd ef 98 76 54 32 10 08 00 00 00 d9 19 00 00
        let h = lcd_bulk_header(LCD_FORMAT_Q565, 6617);
        assert_eq!(&h[..12], &LCD_BULK_MAGIC);
        assert_eq!(&h[12..], &[0x08, 0, 0, 0, 0xd9, 0x19, 0, 0]);
    }

    #[test]
    fn static_color_matches_cam_capture() {
        let p = static_color(KRAKEN_RING, &[0xaa, 0x00, 0xff]);
        assert_eq!(&p[0][..7], &[0x26, 0x04, 0x01, 0x00, 0x00, 0xaa, 0xff]);
        assert_eq!(p[0].len(), 4 + 24 * 3);
        // 26 06 <ch> 00 01 00 00 18 00 00 80 00 32 00 00 01
        assert_eq!(
            p[1],
            vec![0x26, 0x06, 0x01, 0x00, 0x01, 0x00, 0x00, 0x18, 0x00, 0x00, 0x80, 0x00, 0x32, 0x00, 0x00, 0x01]
        );
    }

    #[test]
    fn tai_chi_layout() {
        let m = effect(KRAKEN_RING, Effect::TaiChi, &[[50, 50, 222], [255, 255, 255]], Speed::Normal, false);
        assert_eq!(&m[..7], &[0x2a, 0x04, 0x01, 0x01, 0x0e, 0x1e, 0x00]);
        assert_eq!(&m[7..13], &[50, 50, 222, 255, 255, 255]);
        assert_eq!(m.len(), 7 + 48 + 5);
        assert_eq!(&m[55..], &[0x00, 2, 0x05, 24, 0x03]);
    }

    #[test]
    fn colorless_effect_sends_one_black_slot() {
        let m = effect(HUB_LED_PORTS[0], Effect::SpectrumWave, &[[1, 2, 3]], Speed::Fastest, true);
        assert_eq!(&m[5..7], &[0x50, 0x00]);
        assert_eq!(&m[55..57], &[0x02, 1]);
    }

    #[test]
    fn rgb565_matches_liquidctl() {
        // liquidctl: (r>>3)<<3 + (g>>2)>>3, ((g>>2)&7)<<5 + (b>>3), big-endian bytes
        let out = rgba_to_rgb565(&[0xff, 0x80, 0x10, 0xff]);
        let (r, g, b) = (0xffu8 >> 3, 0x80u8 >> 2, 0x10u8 >> 3);
        assert_eq!(out[0].to_be_bytes(), [(r << 3) + (g >> 3), ((g & 7) << 5) + b]);
    }
}
