//! Temperature to duty curves.

use crate::protocol::{KRAKEN_CURVE_FIRST_C, KRAKEN_CURVE_POINTS};

/// Points of (°C, duty %), sorted and made non-decreasing.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve(Vec<(f32, u8)>);

impl Curve {
    pub fn new(points: &[(f32, u8)]) -> Curve {
        let mut pts: Vec<(f32, u8)> = points.iter().map(|&(t, d)| (t, d.min(100))).collect();
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        pts.dedup_by(|b, a| a.0 == b.0);
        // A hotter point never asks for less than a cooler one.
        let mut max = 0;
        for p in &mut pts {
            max = max.max(p.1);
            p.1 = max;
        }
        Curve(pts)
    }

    pub fn flat(duty: u8) -> Curve {
        Curve(vec![(0.0, duty.min(100))])
    }

    #[cfg(test)]
    pub fn points(&self) -> &[(f32, u8)] {
        &self.0
    }

    /// Linear between points, held flat beyond the first and last one.
    pub fn duty_at(&self, temp: f32) -> u8 {
        let pts = &self.0;
        let Some(first) = pts.first() else { return 100 };
        if temp <= first.0 {
            return first.1;
        }
        for w in pts.windows(2) {
            let ((t0, d0), (t1, d1)) = (w[0], w[1]);
            if temp <= t1 {
                let f = (temp - t0) / (t1 - t0);
                return (d0 as f32 + f * (d1 as f32 - d0 as f32)).round() as u8;
            }
        }
        pts.last().unwrap().1
    }

    /// The 40 duties the Kraken stores for liquid temperatures 20..=59 °C.
    pub fn kraken_table(&self) -> [u8; KRAKEN_CURVE_POINTS] {
        std::array::from_fn(|i| self.duty_at((KRAKEN_CURVE_FIRST_C as usize + i) as f32))
    }
}

/// Follows a temperature with a little damping, so short spikes (a shader compile,
/// a game loading) don't make the fans surge and settle every few seconds.
#[derive(Clone, Debug)]
pub struct Smoother {
    value: Option<f32>,
    rise: f32,
    fall: f32,
}

impl Smoother {
    /// `rise` and `fall` are the share of the gap closed per sample (0..=1).
    pub fn new(rise: f32, fall: f32) -> Smoother {
        Smoother { value: None, rise, fall }
    }

    pub fn update(&mut self, sample: f32) -> f32 {
        let v = match self.value {
            None => sample,
            Some(v) if sample > v => v + (sample - v) * self.rise,
            Some(v) => v + (sample - v) * self.fall,
        };
        self.value = Some(v);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_and_clamps() {
        let c = Curve::new(&[(30.0, 20), (40.0, 60), (50.0, 100)]);
        assert_eq!(c.duty_at(10.0), 20);
        assert_eq!(c.duty_at(35.0), 40);
        assert_eq!(c.duty_at(45.0), 80);
        assert_eq!(c.duty_at(90.0), 100);
    }

    #[test]
    fn sorts_and_forces_monotonic() {
        let c = Curve::new(&[(40.0, 30), (20.0, 50), (60.0, 40)]);
        assert_eq!(c.points(), &[(20.0, 50), (40.0, 50), (60.0, 50)]);
    }

    #[test]
    fn kraken_table_covers_20_to_59() {
        let c = Curve::new(&[(20.0, 20), (59.0, 98)]);
        let t = c.kraken_table();
        assert_eq!(t[0], 20);
        assert_eq!(t[39], 98);
        assert_eq!(t[10], 40);
    }

    #[test]
    fn empty_curve_is_full_speed() {
        assert_eq!(Curve::new(&[]).duty_at(30.0), 100);
    }

    #[test]
    fn smoother_rises_faster_than_it_falls() {
        let mut s = Smoother::new(0.5, 0.1);
        assert_eq!(s.update(40.0), 40.0);
        assert_eq!(s.update(60.0), 50.0);
        assert!((s.update(40.0) - 49.0).abs() < 1e-4);
    }
}
