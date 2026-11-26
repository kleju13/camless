//! NZXT's Hue 2 controllers besides the Control Hub, as liquidctl's `smart_device.py`
//! drives them:
//! - Smart Device V2 (`1e71:2006`, `200d`, `200f`) and RGB & Fan Controller (`2009`,
//!   `200e`, `2010`; the 3+6 channel one `2011`, `2019`, `201f`, `2020`): three fan
//!   ports and two or six LED channels. The Smart Device V2 came in NZXT's cases from
//!   mid-2019, such as the H510 Elite, H510i, H710i and H210i (liquidctl).
//! - HUE 2 (`2001`) and HUE 2 Ambient (`2002`): four and two LED channels, no fans.
//! - RGB Controller (`2012`, `2021`), from 2023, which came with the Kraken Elite 2023:
//!   three LED channels, no fans.
//!
//! The fans and accessories work as on the Control Hub: the same `67 02` fan report,
//! `62 01` duties and `21 03` accessory report, which `control_hub.rs` and `hue2.rs`
//! build and parse. The rest is older: the fan reports start after `60 03` where the
//! hub takes `66 01`, the firmware comes from `10 01`, and LED channels are bits,
//! `1 << i`. The lights take the `28 03` layout (`SmartDevice2`), except on the RGB
//! Controller and the 3+6 channel `2020`, which take `2A 04` with a fixed LED count
//! (`Nzxt2023RgbController`).
//!
//! The 3+6 channel controllers `2011`, `2019` and `201f` get no lighting: they ignore
//! `28 03`, and liquidctl disabled their lights for it (#541). Their fans work.
//!
//! The `2020` is in `SmartDevice2` with six LED channels, but one with firmware 1.7.0
//! has no output report `28` in its report descriptor, and its lights did not change
//! on `28 03` while its fans followed `62 01`. Its output reports are the RGB
//! Controller's, `10 12 20 22 24 26 2a f2 fe e2 f4 2c 1c`, plus the fans' `60 62 66`,
//! so it gets the RGB Controller's `2A 04`. No source has those bytes, but that
//! `2020`'s lights took them.
//!
//! Sources: liquidctl `smart_device.py` (`SmartDevice2`, `Nzxt2023RgbController`) with
//! #541, which logged liquidctl's writes to a `2011` and its replies, and #690, the RGB
//! Controller, run on one; the Linux `nzxt-smart2` driver; and the `camless report` of
//! a `2020` (firmware 1.7.0) and a `2021` (1.5.0) on one PC. Nexus drives none of them,
//! so they are experimental. Verified on hardware only for that `2020`'s fans and
//! lights and that `2021`'s lights, by their owner, on Reddit (camless 0.5.1-beta.1
//! and beta.2), and for a HUE 2 Ambient's lights on firmware 2.6.0 and another
//! `2020`'s, by their owner (camless #68, 0.5.1). The rest is not verified.

use super::control_hub::{self, duties_report, Status};
use super::{hid, hue2, Driver, Found, Model, Reading, NZXT_VID};
use anyhow::{bail, Context, Result};
use camless_core::lighting::Lighting;
use camless_core::status::{Device, FanChannel, LightChannel, Unit};
use hidapi::{HidApi, HidDevice};
use std::time::{Duration, Instant};

const fn model(pid: u16, slug: &'static str, name: &'static str) -> Model {
    Model { experimental: true, ..Model::hid(NZXT_VID, pid, slug, name, open) }
}

pub const SMART_DEVICE_V2: Model = model(0x2006, "smart-device-v2", "Smart Device V2");
pub const SMART_DEVICE_V2_200D: Model = model(0x200d, "smart-device-v2", "Smart Device V2");
pub const SMART_DEVICE_V2_200F: Model = model(0x200f, "smart-device-v2", "Smart Device V2");
pub const RGB_FAN_CONTROLLER: Model = model(0x2009, "rgb-fan-controller", "RGB & Fan Controller");
pub const RGB_FAN_CONTROLLER_200E: Model = model(0x200e, "rgb-fan-controller", "RGB & Fan Controller");
pub const RGB_FAN_CONTROLLER_2010: Model = model(0x2010, "rgb-fan-controller", "RGB & Fan Controller");
pub const RGB_FAN_CONTROLLER_6: Model = model(0x2011, "rgb-fan-controller-6", "RGB & Fan Controller (3+6 channels)");
pub const RGB_FAN_CONTROLLER_6_2019: Model = model(0x2019, "rgb-fan-controller-6", "RGB & Fan Controller (3+6 channels)");
pub const RGB_FAN_CONTROLLER_6_201F: Model = model(0x201f, "rgb-fan-controller-6", "RGB & Fan Controller (3+6 channels)");
pub const RGB_FAN_CONTROLLER_6_2020: Model = model(0x2020, "rgb-fan-controller-6", "RGB & Fan Controller (3+6 channels)");
pub const HUE_2: Model = model(0x2001, "hue2", "HUE 2");
pub const HUE_2_AMBIENT: Model = model(0x2002, "hue2-ambient", "HUE 2 Ambient");
pub const RGB_CONTROLLER: Model = model(0x2012, "rgb-controller", "RGB Controller");
pub const RGB_CONTROLLER_2021: Model = model(0x2021, "rgb-controller", "RGB Controller");

/// How a model's lights take colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// `28 03`, liquidctl's `SmartDevice2`.
    Hue2,
    /// `2A 04` with 8 for the LED count, liquidctl's `Nzxt2023RgbController`; also
    /// the `2020`'s.
    Rgb2023,
}

/// The fan ports and LED channels camless drives on a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Channels {
    pub fans: usize,
    pub lights: usize,
    pub layout: Layout,
}

/// liquidctl's `_MATCHES`, but for the `2020`'s layout; `None` for another id.
pub fn channels(pid: u16) -> Option<Channels> {
    let (fans, lights, layout) = match pid {
        0x2006 | 0x200d | 0x200f | 0x2009 | 0x200e | 0x2010 => (3, 2, Layout::Hue2),
        // Six on the board, but they ignore `28 03` (liquidctl#541).
        0x2011 | 0x2019 | 0x201f => (3, 0, Layout::Hue2),
        // Its descriptor has no report `28`, but the RGB Controller's `2a`.
        0x2020 => (3, 6, Layout::Rgb2023),
        0x2001 => (0, 4, Layout::Hue2),
        0x2002 => (0, 2, Layout::Hue2),
        0x2012 | 0x2021 => (0, 3, Layout::Rgb2023),
        _ => return None,
    };
    Some(Channels { fans, lights, layout })
}

/// Fan reports every 0.5 s in both slots, then fan detection: liquidctl's
/// `initialize` for a model with fans, as logged in liquidctl#541. The Linux driver
/// sends the two the other way round.
const INIT: [&[u8]; 2] = [&[0x60, 0x02, 0x01, 0xe8, 0x01, 0x01, 0xe8, 0x01], &[0x60, 0x03]];

/// What goes out before the firmware request: nothing to a model without fans, as
/// liquidctl starts fan reports only on a fan controller (its #331).
fn init(channels: Channels) -> &'static [&'static [u8]] {
    if channels.fans > 0 {
        &INIT
    } else {
        &[]
    }
}

/// `11 01`: the version at [0x11..0x13].
pub fn parse_firmware(msg: &[u8]) -> Option<String> {
    (msg.len() > 0x13 && msg[..2] == [0x11, 0x01]).then(|| format!("{}.{}.{}", msg[0x11], msg[0x12], msg[0x13]))
}

/// LED channel `i` (from 0) as liquidctl's channel byte.
fn channel_bit(i: usize) -> u8 {
    1 << i
}

pub struct SmartDevice2 {
    model: &'static Model,
    channels: Channels,
    hid: HidDevice,
    firmware: Option<String>,
    accessories: Vec<hue2::ChannelAccessories>,
    /// For `camless report`: what `21 03` held when it named no accessory, how it came
    /// after a try went unanswered, or why it has not come.
    accessory_notes: Vec<String>,
    /// Until the accessory report comes.
    asking: Option<Asking>,
    /// `describe` has changed since the daemon read it.
    changed: bool,
    last: Option<(Status, Instant)>,
}

fn open(api: &HidApi, found: &Found) -> Result<Box<dyn Driver>> {
    let channels = channels(found.model.pid).context("not a Hue 2 controller camless knows")?;
    let hid = hid::open(api, &found.path)?;
    hid::drain(&hid);
    for msg in init(channels) {
        hid::write(&hid, msg)?;
    }
    let firmware = parse_firmware(&hid::request(&hid, &[0x10, 0x01], &[0x11, 0x01])?);
    let lights = channels.lights;
    let mut dev = SmartDevice2 {
        model: found.model,
        channels,
        hid,
        firmware,
        accessories: vec![Default::default(); lights],
        accessory_notes: Vec::new(),
        asking: None,
        changed: false,
        last: None,
    };
    if lights > 0 {
        dev.ask_at_open();
    }
    Ok(Box::new(dev))
}

/// The accessory request.
const ASK: [u8; 2] = [0x20, 0x03];

/// How many times `20 03` goes out, and how long each waits for `21 03`. OpenRGB sends
/// it up to 10 times, 1 s apart.
const TRIES: u32 = 3;
const TRY_WAIT: Duration = hid::REPLY_TIMEOUT;

/// Asking for the accessory report until it comes. A Smart Device V2 `200f` with
/// firmware 1.13.0, in an H510 Elite, left `20 03` unanswered for 3 s when it opened
/// (camless #88), while a `200f` on 1.12.0, in an H210i, answered it (liquidctl #364).
/// The first `20 03` goes out right after `60 03` starts fan detection, and these
/// devices drop what comes in a burst (`hue2::send`), so `poll` asks again and takes a
/// reply that comes late. What the device sent meanwhile goes into the report's note.
#[derive(Debug)]
struct Asking {
    /// How many times `20 03` went out.
    tries: u32,
    /// When the last one went out; `None` once it went unanswered.
    sent: Option<Instant>,
    /// Reports since then, by their first two bytes, in the order they first came.
    seen: Vec<([u8; 2], u32)>,
    /// A line for each try that went unanswered.
    log: Vec<String>,
    gave_up: bool,
}

#[derive(Debug, PartialEq)]
enum Next {
    Wait,
    Ask,
    GaveUp,
}

impl Asking {
    fn new(sent: Instant) -> Self {
        Asking { tries: 1, sent: Some(sent), seen: Vec::new(), log: Vec::new(), gave_up: false }
    }

    /// The reply, if `msg` is it. Anything else is counted.
    fn see(&mut self, msg: &[u8]) -> Option<Vec<u8>> {
        if msg.starts_with(&[0x21, 0x03]) {
            return Some(msg.to_vec());
        }
        if let [a, b, ..] = *msg {
            match self.seen.iter_mut().find(|(k, _)| *k == [a, b]) {
                Some((_, n)) => *n += 1,
                None => self.seen.push(([a, b], 1)),
            }
        }
        None
    }

    /// What to do at `now`: an unanswered try is logged, and the next one is due.
    fn next(&mut self, now: Instant) -> Next {
        if self.gave_up {
            return Next::Wait;
        }
        let Some(sent) = self.sent else { return Next::Ask };
        let waited = now.duration_since(sent);
        if waited < TRY_WAIT {
            return Next::Wait;
        }
        self.log.push(format!("try {}: no reply in {} ms, saw {}", self.tries, waited.as_millis(), seen_text(&self.seen)));
        self.seen.clear();
        self.sent = None;
        if self.tries < TRIES {
            Next::Ask
        } else {
            self.gave_up = true;
            Next::GaveUp
        }
    }

    fn asked(&mut self, now: Instant) {
        self.tries += 1;
        self.sent = Some(now);
    }

    /// The note while there is no reply.
    fn note(&self) -> String {
        let log = self.log.join("; ");
        if self.gave_up {
            format!("accessory report: no reply to {} tries; {log}", self.tries)
        } else {
            format!("accessory report: asking again; {log}")
        }
    }

    /// The note for a reply at `now`, if a try went unanswered before it.
    fn came(&self, now: Instant) -> Option<String> {
        if self.log.is_empty() {
            return None;
        }
        let when = match self.sent {
            _ if self.gave_up => format!("late, after {} tries", self.tries),
            Some(t) => format!("on try {}, {} ms after it", self.tries, now.duration_since(t).as_millis()),
            None => format!("after try {}", self.tries),
        };
        Some(format!("accessory report came {when}; {}", self.log.join("; ")))
    }
}

/// `67 02 ×6, 61 03 ×1`.
fn seen_text(seen: &[([u8; 2], u32)]) -> String {
    if seen.is_empty() {
        return "nothing".into();
    }
    seen.iter().map(|([a, b], n)| format!("{a:02x} {b:02x} ×{n}")).collect::<Vec<_>>().join(", ")
}

impl SmartDevice2 {
    /// The first `20 03`, waited for here, as liquidctl does in `initialize`.
    fn ask_at_open(&mut self) {
        if let Err(e) = hid::write(&self.hid, &ASK) {
            self.accessory_notes = vec![format!("accessory report: {e:#}")];
            return;
        }
        let mut asking = Asking::new(Instant::now());
        let end = Instant::now() + TRY_WAIT;
        let mut buf = [0u8; hid::REPORT_LEN];
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match self.hid.read_timeout(&mut buf, left.as_millis().max(1) as i32) {
                Ok(n) => {
                    if let Some(reply) = asking.see(&buf[..n]) {
                        self.took(&reply, None);
                        return;
                    }
                }
                Err(e) => {
                    self.accessory_notes = vec![format!("accessory report: {e:#}")];
                    return;
                }
            }
        }
        // The next try goes out from `poll`, once fan detection is done.
        asking.next(Instant::now());
        self.accessory_notes = vec![asking.note()];
        self.asking = Some(asking);
    }

    /// Looks for the accessory report in what came, and asks again when it is due.
    fn watch(&mut self, pending: &[Vec<u8>]) {
        let Some(asking) = self.asking.as_mut() else { return };
        let now = Instant::now();
        for msg in pending {
            if let Some(reply) = asking.see(msg) {
                let came = asking.came(now);
                self.took(&reply, came);
                return;
            }
        }
        match asking.next(now) {
            Next::Wait => return,
            // A failed write goes out again at the next poll.
            Next::Ask => {
                if hid::write(&self.hid, &ASK).is_ok() {
                    asking.asked(now);
                }
            }
            Next::GaveUp => {}
        }
        let note = asking.note();
        if self.accessory_notes != [note.as_str()] {
            self.accessory_notes = vec![note];
            self.changed = true;
        }
    }

    fn took(&mut self, reply: &[u8], came: Option<String>) {
        self.accessories = hue2::parse_accessories(reply, self.channels.lights);
        self.accessory_notes = came.into_iter().collect();
        if self.accessories.iter().all(|a| a.ids.is_empty()) {
            self.accessory_notes.push(accessory_note(reply));
        }
        self.asking = None;
        self.changed = true;
    }
}

fn accessory_note(msg: &[u8]) -> String {
    let bytes: Vec<String> = msg.iter().map(|b| format!("{b:02x}")).collect();
    format!("accessory report named nothing: {}", bytes.join(" "))
}

impl Driver for SmartDevice2 {
    fn describe(&self) -> Device {
        Device {
            name: self.model.name.into(),
            firmware: self.firmware.clone(),
            verified: self.model.verified(),
            experimental: self.model.is_experimental(),
            fans: (1..=self.channels.fans)
                .map(|n| FanChannel { id: format!("fan{n}"), name: format!("Fan {n}"), ..Default::default() })
                .collect(),
            lights: self
                .accessories
                .iter()
                .enumerate()
                .map(|(i, a)| LightChannel {
                    id: format!("led{}", i + 1),
                    name: format!("LED {}", i + 1),
                    leds: a.leds(),
                    accessories: a.names(),
                    modes: match self.channels.layout {
                        Layout::Hue2 => hue2::modes_28(),
                        Layout::Rgb2023 => hue2::modes_2023(),
                    },
                })
                .collect(),
            notes: self.accessory_notes.clone(),
            ..Default::default()
        }
    }

    fn sensors(&self) -> Vec<(&'static str, &'static str, Unit)> {
        Vec::new()
    }

    fn poll(&mut self) -> Result<Reading> {
        // Without fans there is no fan report, and nothing else to read once the
        // accessories are in.
        let pending = if self.channels.fans > 0 || self.asking.is_some() { hid::pending(&self.hid) } else { Vec::new() };
        self.watch(&pending);
        if self.channels.fans == 0 {
            return Ok(Reading::default());
        }
        Ok(control_hub::reading(&control_hub::status_from(&self.hid, &pending, &mut self.last, self.channels.fans)?))
    }

    fn changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    fn set_duties(&mut self, duties: &[(&str, u8)]) -> Result<()> {
        let mut ports = Vec::new();
        for &(fan, duty) in duties {
            let port = control_hub::port(fan, "fan", self.channels.fans);
            ports.push((port.with_context(|| format!("no fan {fan:?} on the {}", self.model.name))?, duty));
        }
        if !ports.is_empty() {
            hid::write(&self.hid, &duties_report(&ports))?;
        }
        Ok(())
    }

    fn set_lighting(&mut self, light: &str, lighting: &Lighting) -> Result<()> {
        let count = self.accessories.len();
        let Some(i) = control_hub::port(light, "led", count) else {
            bail!("no light {light:?} on the {}", self.model.name)
        };
        let packet = match self.channels.layout {
            Layout::Hue2 => hue2::packet_28(channel_bit(i), lighting)?,
            Layout::Rgb2023 => hue2::packet_2023(channel_bit(i), lighting)?,
        };
        hue2::send(&self.hid, vec![packet])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::MODELS;

    /// liquidctl's `SmartDevice2._MATCHES` and `Nzxt2023RgbController._MATCHES`:
    /// (id, fan channels, LED channels, layout).
    const LIQUIDCTL: [(u16, usize, usize, Layout); 14] = [
        (0x2006, 3, 2, Layout::Hue2),
        (0x200d, 3, 2, Layout::Hue2),
        (0x200f, 3, 2, Layout::Hue2),
        (0x2001, 0, 4, Layout::Hue2),
        (0x2002, 0, 2, Layout::Hue2),
        (0x2009, 3, 2, Layout::Hue2),
        (0x200e, 3, 2, Layout::Hue2),
        (0x2010, 3, 2, Layout::Hue2),
        (0x2011, 3, 0, Layout::Hue2),
        (0x2019, 3, 0, Layout::Hue2),
        (0x201f, 3, 0, Layout::Hue2),
        (0x2020, 3, 6, Layout::Hue2),
        (0x2012, 0, 3, Layout::Rgb2023),
        (0x2021, 0, 3, Layout::Rgb2023),
    ];

    #[test]
    fn channels_follow_liquidctl() {
        for (pid, fans, lights, layout) in LIQUIDCTL {
            let layout = if pid == 0x2020 { Layout::Rgb2023 } else { layout };
            assert_eq!(channels(pid), Some(Channels { fans, lights, layout }), "{pid:04x}");
            let m = MODELS.iter().find(|m| (m.vid, m.pid) == (NZXT_VID, pid)).expect("listed in MODELS");
            assert!(m.experimental && m.vendor.is_none(), "{pid:04x}");
        }
        // The Control Hub, the H1 V2, and OpenRGB's B650E and HUE 2 Motherboard boards.
        for pid in [0x2022, 0x2015, 0x201b, 0x2005, 0x200b] {
            assert_eq!(channels(pid), None);
        }
    }

    #[test]
    fn a_reply_without_accessories_is_noted_whole() {
        let mut msg = vec![0u8; 64];
        msg[..2].copy_from_slice(&[0x21, 0x03]);
        msg[14] = 2;
        let note = accessory_note(&msg);
        assert!(note.starts_with("accessory report named nothing: 21 03 00 00"), "{note}");
        assert_eq!(note.split(": ").nth(1).unwrap().split(' ').count(), 64);
        assert!(hue2::parse_accessories(&msg, 2).iter().all(|a| a.ids.is_empty()));
    }

    #[test]
    fn accessories_are_asked_for_three_times_and_the_wait_is_noted() {
        let t0 = Instant::now();
        let s = Duration::from_secs;
        let mut a = Asking::new(t0);
        for msg in [[0x67, 0x02], [0x61, 0x03], [0x67, 0x02]] {
            assert_eq!(a.see(&msg), None);
        }
        assert_eq!(a.next(t0 + s(1)), Next::Wait);
        // Logged, and the next try waits for the caller.
        assert_eq!(a.next(t0 + s(3)), Next::Ask);
        assert_eq!(a.next(t0 + s(4)), Next::Ask);
        assert_eq!(a.note(), "accessory report: asking again; try 1: no reply in 3000 ms, saw 67 02 ×2, 61 03 ×1");
        a.asked(t0 + s(4));
        assert_eq!(a.next(t0 + s(7)), Next::Ask);
        a.asked(t0 + s(8));
        assert_eq!(a.next(t0 + s(10)), Next::Wait);
        assert_eq!(a.next(t0 + s(11)), Next::GaveUp);
        assert_eq!(a.next(t0 + s(30)), Next::Wait);
        assert_eq!(
            a.note(),
            "accessory report: no reply to 3 tries; try 1: no reply in 3000 ms, saw 67 02 ×2, 61 03 ×1; \
             try 2: no reply in 3000 ms, saw nothing; try 3: no reply in 3000 ms, saw nothing"
        );
        // A reply after that is still taken.
        let mut reply = vec![0u8; 64];
        reply[..2].copy_from_slice(&[0x21, 0x03]);
        assert_eq!(a.see(&reply), Some(reply.clone()));
        assert!(a.came(t0 + s(31)).unwrap().starts_with("accessory report came late, after 3 tries; try 1:"));
    }

    #[test]
    fn a_reply_to_the_second_try_says_so() {
        let t0 = Instant::now();
        let mut a = Asking::new(t0);
        assert_eq!(a.came(t0), None, "a first reply needs no note");
        a.next(t0 + TRY_WAIT);
        a.asked(t0 + Duration::from_secs(4));
        let came = a.came(t0 + Duration::from_millis(4140)).unwrap();
        assert_eq!(came, "accessory report came on try 2, 140 ms after it; try 1: no reply in 3000 ms, saw nothing");
    }

    #[test]
    fn init_matches_liquidctls_log() {
        // liquidctl#541: report 60 with 02 01 e8 01 01 e8 01, then 60 03, then 10 01.
        assert_eq!(INIT[0], [0x60, 0x02, 0x01, 0xe8, 0x01, 0x01, 0xe8, 0x01]);
        assert_eq!(INIT[1], [0x60, 0x03]);
        assert_eq!(init(channels(0x2006).unwrap()), INIT);
        for pid in [0x2001, 0x2002, 0x2012, 0x2021] {
            assert!(init(channels(pid).unwrap()).is_empty(), "{pid:04x}");
        }
    }

    #[test]
    fn firmware_from_a_2011() {
        // The `11 01` reply in liquidctl#541, which liquidctl read as 1.7.0; the serial
        // number in bytes 2..14 is zeroed here.
        let mut msg = vec![0u8; 64];
        msg[..2].copy_from_slice(&[0x11, 0x01]);
        msg[14..20].copy_from_slice(&[0x11, 0x20, 0x01, 0x01, 0x07, 0x00]);
        assert_eq!(parse_firmware(&msg).as_deref(), Some("1.7.0"));
        assert_eq!(parse_firmware(&msg[..0x13]), None);
        msg[1] = 0x02;
        assert_eq!(parse_firmware(&msg), None);
    }

    #[test]
    fn accessories_from_a_2011() {
        // The `21 03` reply in liquidctl#541: six channels, three F140 RGB, then three
        // F120 RGB. The serial number in bytes 2..14 is zeroed here.
        let mut msg = vec![0u8; 64];
        msg[..2].copy_from_slice(&[0x21, 0x03]);
        msg[14] = 0x06;
        for (c, id) in [0x14, 0x14, 0x14, 0x13, 0x13, 0x13].into_iter().enumerate() {
            msg[15 + 6 * c] = id;
        }
        let ch = hue2::parse_accessories(&msg, 6);
        assert_eq!(ch[0].names(), ["F140 RGB"]);
        assert_eq!(ch[5].names(), ["F120 RGB"]);
        assert!(ch.iter().all(|c| c.ids.len() == 1));
    }

    #[test]
    fn fan_report_with_three_ports() {
        // The Linux driver's `fan_status_report`: id, type 02, 14 bytes, fan types from
        // 16, rpm LE from 24, duty from 40, the duty again from 48, noise at 56.
        let mut m = vec![0u8; 64];
        m[..2].copy_from_slice(&[0x67, 0x02]);
        m[16..19].copy_from_slice(&[2, 1, 0]);
        m[24..26].copy_from_slice(&1450u16.to_le_bytes());
        m[26..28].copy_from_slice(&700u16.to_le_bytes());
        m[40..43].copy_from_slice(&[60, 40, 35]);
        m[56] = 31;
        let s = control_hub::parse_status(&m, 3).unwrap();
        assert_eq!(s.fans.len(), 3);
        assert_eq!(s.fans[0], control_hub::Port { mode: control_hub::FanMode::Pwm, rpm: 1450, duty: 60 });
        assert_eq!(s.fans[1].mode, control_hub::FanMode::Dc);
        assert_eq!(s.fans[2], control_hub::Port { mode: control_hub::FanMode::None, rpm: 0, duty: 35 });
        assert_eq!(s.noise_db, 31);
        let r = control_hub::reading(&s);
        assert_eq!(r.fans.iter().map(|f| f.0).collect::<Vec<_>>(), ["fan1", "fan2", "fan3"]);
        assert_eq!(r.fans[2].1.connected, Some(false));
        // The voltage report, type 04, is not the fan report.
        m[1] = 0x04;
        assert!(control_hub::parse_status(&m, 3).is_none());
    }

    #[test]
    fn duties_match_liquidctl_and_linux() {
        // liquidctl `_write_fixed_duty(2, 85)`: 62 01 04 00 00 55, padded with zeros;
        // Linux `set_pwm` sends the same with room for eight duties.
        let mut sent = duties_report(&[(2, 85)]);
        sent.resize(hid::REPORT_LEN, 0);
        let mut want = vec![0x62, 0x01, 0x04, 0x00, 0x00, 0x55];
        want.resize(hid::REPORT_LEN, 0);
        assert_eq!(sent, want);
        assert_eq!(control_hub::port("fan3", "fan", 3), Some(2));
        assert_eq!(control_hub::port("fan4", "fan", 3), None);
    }

    #[test]
    fn led_channels_are_bits() {
        // liquidctl: led1 is 1 << 0; led6 went out as 20 in liquidctl#541.
        assert_eq!((0..6).map(channel_bit).collect::<Vec<_>>(), [0x01, 0x02, 0x04, 0x08, 0x10, 0x20]);
    }

    #[test]
    fn the_2020_lights_take_2a_04() {
        let c = channels(0x2020).unwrap();
        assert_eq!((c.fans, c.lights, c.layout), (3, 6, Layout::Rgb2023));
        // led6 in red: channel 20 twice, fixed at speed 32 00, red as GRB, 8 LEDs.
        let p = hue2::packet_2023(channel_bit(5), &Lighting::Fixed([0xff, 0x00, 0x00])).unwrap();
        assert_eq!(&p[..10], &[0x2a, 0x04, 0x20, 0x20, 0x00, 0x32, 0x00, 0x00, 0xff, 0x00]);
        assert_eq!(&p[p.len() - 5..], &[0x00, 0x01, 0x00, 0x08, 0x03]);
    }
}
