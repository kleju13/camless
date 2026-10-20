//! Frames per second of one program, from the presents the graphics kernel reports.
//!
//! A real-time ETW session listens to one event of Microsoft-Windows-DxgKrnl,
//! Present_Info (184, keyword Present), and reads only its header: which process
//! presented a frame, and when. Nexus counts frames the same way, and PresentMon reads
//! the same provider. Starting the session needs administrator rights; nothing goes
//! to disk.
//!
//! ETW hands events over in batches, one buffer per CPU, up to a second late and not
//! always in order. A [`Meter`] sorts the presents and counts them once they are
//! [`SETTLE`] old.
//!
//! On Linux nothing counts them, so game sessions there have no frame rates.

use std::collections::VecDeque;

/// ETW timestamps count 100 ns since 1601, like a FILETIME.
pub const SECOND: i64 = 10_000_000;
/// How old a present must be before it is counted.
pub const SETTLE: i64 = 2 * SECOND;
/// A longer gap between two presents is a pause, not a frame.
pub const MAX_FRAME: i64 = SECOND;
/// The service's ETW session; `camless fps` uses another, so the two can run at once.
pub const SESSION: &str = "camless-fps";

#[cfg(windows)]
mod etw;
#[cfg(windows)]
pub use etw::{now, Capture};

/// Frame rates come from ETW, which Linux has nothing like for every game. There
/// a capture never starts, and sessions keep their play time and temperatures.
#[cfg(not(windows))]
pub struct Capture;

#[cfg(not(windows))]
impl Capture {
    pub fn start(_session: &str) -> anyhow::Result<Capture> {
        anyhow::bail!("frame rates are counted on Windows only")
    }

    pub fn follow(&self, _pid: u32) {}

    pub fn take(&self) -> Vec<i64> {
        Vec::new()
    }

    pub fn alive(&self) -> bool {
        false
    }
}

/// Now, in 100 ns since 1601 like a FILETIME, the unit of ETW timestamps.
#[cfg(not(windows))]
pub fn now() -> i64 {
    const UNIX_FROM_1601: i64 = 11_644_473_600 * SECOND;
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    UNIX_FROM_1601 + (since.as_nanos() / 100) as i64
}

/// Frame times from present timestamps that arrive late and out of order.
#[derive(Default)]
pub struct Meter {
    pending: Vec<i64>,
    /// The newest present counted.
    last: Option<i64>,
    /// Counted presents of the last second.
    recent: VecDeque<i64>,
}

impl Meter {
    pub fn add(&mut self, presents: &[i64]) {
        self.pending.extend_from_slice(presents);
    }

    /// Counts the presents that are [`SETTLE`] old at `now`, in order, and hands each
    /// frame time to `frame`: the time since the present before, if that was at most
    /// [`MAX_FRAME`] earlier. A present older than one already counted came too late
    /// and is dropped.
    pub fn settle(&mut self, now: i64, mut frame: impl FnMut(i64)) {
        let cut = now - SETTLE;
        self.pending.sort_unstable();
        let n = self.pending.partition_point(|&t| t <= cut);
        for t in self.pending.drain(..n) {
            match self.last {
                Some(last) if t <= last => continue,
                Some(last) if t - last <= MAX_FRAME => frame(t - last),
                _ => {}
            }
            self.last = Some(t);
            self.recent.push_back(t);
        }
        while self.recent.front().is_some_and(|&t| t <= cut - SECOND) {
            self.recent.pop_front();
        }
    }

    /// A present has been counted.
    pub fn started(&self) -> bool {
        self.last.is_some()
    }

    /// Frames in the second before the last [`Meter::settle`] cut.
    pub fn fps(&self) -> u32 {
        self.recent.len() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: i64 = SECOND / 1000;

    #[test]
    fn presents_count_once_settled_and_in_order() {
        let mut m = Meter::default();
        let t0 = 1_000 * SECOND;
        // 100 fps for a second, handed over as two batches, the later one first.
        let presents: Vec<i64> = (0..100).map(|i| t0 + i * 10 * MS).collect();
        m.add(&presents[50..]);
        m.add(&presents[..50]);
        let mut frames = Vec::new();
        m.settle(t0 + SETTLE / 2, |f| frames.push(f));
        assert!(frames.is_empty(), "nothing is old enough yet");
        // The second up to 995 ms holds all 100.
        let cut = t0 + 995 * MS;
        m.settle(cut + SETTLE, |f| frames.push(f));
        assert_eq!(frames, vec![10 * MS; 99]);
        assert_eq!(m.fps(), 100);
        // A present that comes after later ones were counted is dropped.
        m.add(&[t0 + 5 * MS]);
        m.settle(cut + SETTLE, |f| frames.push(f));
        assert_eq!(frames.len(), 99);
    }

    #[test]
    fn a_pause_is_not_a_frame() {
        let mut m = Meter::default();
        let t0 = 1_000 * SECOND;
        m.add(&[t0, t0 + 16 * MS, t0 + 16 * MS + 3 * SECOND, t0 + 32 * MS + 3 * SECOND]);
        let mut frames = Vec::new();
        m.settle(t0 + 10 * SECOND, |f| frames.push(f));
        assert_eq!(frames, [16 * MS, 16 * MS]);
        // Nothing presented in the second before the cut.
        assert_eq!(m.fps(), 0);
    }

    #[test]
    fn the_clock_is_a_filetime() {
        // 2020-01-01 in 100 ns since 1601.
        assert!(now() > 132_223_104_000_000_000);
    }
}
