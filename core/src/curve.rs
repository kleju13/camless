//! The curve engine: sensor readings in, a duty per fan out.
//!
//! Each step evaluates every curve once, mixes after their inputs, then applies each
//! fan's offset, limits and rate of change. A curve with no reading gives nothing,
//! and its fans fall back to the failsafe duty.

use crate::config::{curve_order, Config, CurveSpec, Fan, MixFunction, Point, Response};
use std::collections::HashMap;

/// Points sorted by sensor value, with duties that never fall as the value rises.
#[derive(Clone, Debug, PartialEq)]
pub struct Points(Vec<Point>);

impl Points {
    pub fn new(points: &[Point]) -> Points {
        let mut pts: Vec<Point> = points.iter().map(|&(t, d)| (t, d.min(100))).collect();
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        pts.dedup_by(|b, a| a.0 == b.0);
        // A hotter point never asks for less than a cooler one.
        let mut max = 0;
        for p in &mut pts {
            max = max.max(p.1);
            p.1 = max;
        }
        Points(pts)
    }

    pub fn as_slice(&self) -> &[Point] {
        &self.0
    }

    /// Linear between points, flat beyond the first and the last. No points is 100 %.
    pub fn duty_at(&self, value: f32) -> f32 {
        let pts = &self.0;
        let Some(first) = pts.first() else { return 100.0 };
        if value <= first.0 {
            return first.1 as f32;
        }
        for w in pts.windows(2) {
            let ((t0, d0), (t1, d1)) = (w[0], w[1]);
            if value <= t1 {
                let f = (value - t0) / (t1 - t0);
                return d0 as f32 + f * (d1 as f32 - d0 as f32);
            }
        }
        pts.last().unwrap().1 as f32
    }
}

/// Exponential approach to the sensor, with separate time constants up and down,
/// then hysteresis on top.
#[derive(Clone, Debug, Default)]
struct Follow {
    smooth: Option<f32>,
    held: Option<f32>,
}

impl Follow {
    fn update(&mut self, sample: Option<f32>, r: &Response, dt: f32) -> Option<f32> {
        let Some(sample) = sample else {
            *self = Follow::default();
            return None;
        };
        let v = match self.smooth {
            None => sample,
            Some(v) => {
                let tau = if sample > v { r.response_up } else { r.response_down };
                let k = if tau <= 0.0 { 1.0 } else { 1.0 - (-dt / tau).exp() };
                v + (sample - v) * k
            }
        };
        self.smooth = Some(v);
        let held = match self.held {
            Some(h) if (v - h).abs() < r.hysteresis => h,
            _ => v,
        };
        self.held = Some(held);
        Some(held)
    }
}

struct CurveState {
    spec: CurveSpec,
    points: Points,
    follow: Follow,
    loaded: bool,
    out: Option<f32>,
}

struct FanState {
    spec: Fan,
    curve: Option<usize>,
    out: Option<f32>,
}

/// What camless wants a fan channel at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Duty {
    /// No curve: camless does not touch the channel.
    Unmanaged,
    /// The curve has no reading; this is the failsafe duty.
    Failsafe(u8),
    Set(u8),
}

impl Duty {
    pub fn value(self) -> Option<u8> {
        match self {
            Duty::Unmanaged => None,
            Duty::Failsafe(d) | Duty::Set(d) => Some(d),
        }
    }
}

pub struct Engine {
    curves: Vec<CurveState>,
    index: HashMap<String, usize>,
    fans: Vec<FanState>,
    failsafe: u8,
}

impl Engine {
    /// `cfg` must have passed `Config::validate`.
    pub fn new(cfg: &Config) -> Engine {
        let order = curve_order(&cfg.curves).unwrap_or_else(|_| (0..cfg.curves.len()).collect());
        let curves: Vec<CurveState> = order
            .iter()
            .map(|&i| {
                let spec = cfg.curves[i].clone();
                let points = match &spec {
                    CurveSpec::Graph(g) => Points::new(&g.points),
                    _ => Points::new(&[]),
                };
                CurveState { spec, points, follow: Follow::default(), loaded: false, out: None }
            })
            .collect();
        let index: HashMap<String, usize> = curves.iter().enumerate().map(|(i, c)| (c.spec.name().to_string(), i)).collect();
        let fans = cfg
            .fans
            .iter()
            .map(|f| FanState { spec: f.clone(), curve: f.curve.as_ref().and_then(|c| index.get(c).copied()), out: None })
            .collect();
        Engine { curves, index, fans, failsafe: cfg.failsafe_duty }
    }

    /// Advances every curve by `dt` seconds. With `critical` set (the liquid is too
    /// hot), every managed fan goes to 100 % at once, past its limits.
    pub fn step(&mut self, sensor: &dyn Fn(&str) -> Option<f32>, dt: f32, critical: bool) {
        for i in 0..self.curves.len() {
            let out = match &self.curves[i].spec {
                CurveSpec::Fixed(f) => Some(f.duty as f32),
                CurveSpec::Graph(g) => {
                    let (r, reading) = (g.response(), sensor(&g.sensor));
                    let c = &mut self.curves[i];
                    c.follow.update(reading, &r, dt).map(|v| c.points.duty_at(v))
                }
                CurveSpec::Trigger(t) => {
                    let (r, reading) = (t.response(), sensor(&t.sensor));
                    let (idle_at, load_at, idle, load) = (t.idle_at, t.load_at, t.idle_duty, t.load_duty);
                    let c = &mut self.curves[i];
                    c.follow.update(reading, &r, dt).map(|v| {
                        if v >= load_at {
                            c.loaded = true;
                        } else if v <= idle_at {
                            c.loaded = false;
                        }
                        (if c.loaded { load } else { idle }) as f32
                    })
                }
                CurveSpec::Mix(m) => {
                    let inputs: Option<Vec<f32>> =
                        m.curves.iter().map(|n| self.index.get(n).and_then(|&j| self.curves[j].out)).collect();
                    inputs.filter(|v| !v.is_empty()).map(|v| match m.function {
                        MixFunction::Max => v.iter().copied().fold(f32::MIN, f32::max),
                        MixFunction::Min => v.iter().copied().fold(f32::MAX, f32::min),
                        MixFunction::Average => v.iter().sum::<f32>() / v.len() as f32,
                        MixFunction::Sum => v.iter().sum::<f32>().min(100.0),
                    })
                }
            };
            self.curves[i].out = out;
        }
        for f in &mut self.fans {
            let Some(ci) = f.curve else {
                f.out = None;
                continue;
            };
            let target = match self.curves[ci].out {
                _ if critical => 100.0,
                None => self.failsafe as f32,
                Some(v) => (v + f.spec.offset as f32).clamp(f.spec.min as f32, f.spec.max as f32),
            };
            f.out = Some(match f.out {
                Some(prev) if !critical => {
                    let up = if f.spec.step_up > 0.0 { f.spec.step_up * dt } else { f32::INFINITY };
                    let down = if f.spec.step_down > 0.0 { f.spec.step_down * dt } else { f32::INFINITY };
                    target.clamp(prev - down, prev + up)
                }
                _ => target,
            });
        }
    }

    pub fn duty(&self, channel: &str) -> Duty {
        let Some(f) = self.fans.iter().find(|f| f.spec.channel == channel) else { return Duty::Unmanaged };
        match (f.curve, f.out) {
            (None, _) | (_, None) => Duty::Unmanaged,
            (Some(ci), Some(v)) if self.curves[ci].out.is_none() => Duty::Failsafe(v.round() as u8),
            (_, Some(v)) => Duty::Set(v.round() as u8),
        }
    }

    /// Every managed channel and its duty after the last step.
    pub fn duties(&self) -> Vec<(&str, Duty)> {
        self.fans.iter().map(|f| (f.spec.channel.as_str(), self.duty(&f.spec.channel))).collect()
    }

    pub fn curve_value(&self, name: &str) -> Option<f32> {
        self.index.get(name).and_then(|&i| self.curves[i].out)
    }

    /// The sensor value a curve last acted on, after smoothing and hysteresis.
    pub fn curve_input(&self, name: &str) -> Option<f32> {
        self.index.get(name).and_then(|&i| self.curves[i].follow.held)
    }

    /// A fan whose curve a device could run by itself: a fixed duty, or a graph on
    /// one sensor, with the fan's offset and limits folded in. Rates of change,
    /// smoothing and hysteresis are host-only and left out.
    pub fn device_curve(&self, channel: &str) -> Option<DeviceCurve> {
        let f = self.fans.iter().find(|f| f.spec.channel == channel)?;
        let c = &self.curves[f.curve?];
        let (sensor, points) = match &c.spec {
            CurveSpec::Fixed(x) => (None, Points::new(&[(0.0, x.duty)])),
            CurveSpec::Graph(g) => (Some(g.sensor.clone()), c.points.clone()),
            _ => return None,
        };
        Some(DeviceCurve { sensor, points, offset: f.spec.offset, min: f.spec.min, max: f.spec.max })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceCurve {
    /// None for a fixed duty.
    pub sensor: Option<String>,
    points: Points,
    offset: i8,
    min: u8,
    max: u8,
}

impl DeviceCurve {
    pub fn duty_at(&self, value: f32) -> u8 {
        (self.points.duty_at(value) + self.offset as f32).clamp(self.min as f32, self.max as f32).round() as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn engine(toml: &str) -> Engine {
        Engine::new(&Config::parse(toml).unwrap())
    }

    const GRAPH: &str = "[[curve]]\nkind = \"graph\"\nname = \"C\"\nsensor = \"cpu.temp\"\n\
        points = [[30, 20], [40, 60], [50, 100]]\nresponse_up = 0\nresponse_down = 0\n\
        [[fan]]\nchannel = \"hub/fan1\"\ncurve = \"C\"\n";

    #[test]
    fn interpolates_and_clamps() {
        let c = Points::new(&[(30.0, 20), (40.0, 60), (50.0, 100)]);
        assert_eq!(c.duty_at(10.0), 20.0);
        assert_eq!(c.duty_at(35.0), 40.0);
        assert_eq!(c.duty_at(45.0), 80.0);
        assert_eq!(c.duty_at(90.0), 100.0);
        assert_eq!(Points::new(&[]).duty_at(30.0), 100.0);
    }

    #[test]
    fn sorts_and_forces_monotonic() {
        let c = Points::new(&[(40.0, 30), (20.0, 50), (60.0, 40)]);
        assert_eq!(c.as_slice(), &[(20.0, 50), (40.0, 50), (60.0, 50)]);
    }

    #[test]
    fn follows_a_sensor() {
        let mut e = engine(GRAPH);
        e.step(&|_| Some(35.0), 1.0, false);
        assert_eq!(e.duty("hub/fan1"), Duty::Set(40));
        assert_eq!(e.duty("hub/other"), Duty::Unmanaged);
    }

    #[test]
    fn missing_sensor_is_failsafe() {
        let mut e = engine(GRAPH);
        e.step(&|_| None, 1.0, false);
        assert_eq!(e.duty("hub/fan1"), Duty::Failsafe(70));
    }

    #[test]
    fn critical_overrides_limits() {
        let mut e = engine(&format!("{GRAPH}max = 50\n"));
        e.step(&|_| Some(45.0), 1.0, false);
        assert_eq!(e.duty("hub/fan1"), Duty::Set(50));
        e.step(&|_| Some(45.0), 1.0, true);
        assert_eq!(e.duty("hub/fan1"), Duty::Set(100));
    }

    #[test]
    fn rises_faster_than_it_falls() {
        let toml = GRAPH.replace("response_up = 0\nresponse_down = 0", "response_up = 1\nresponse_down = 10");
        let mut e = engine(&toml);
        e.step(&|_| Some(30.0), 1.0, false);
        e.step(&|_| Some(50.0), 1.0, false);
        // 1 - e^-1 of the 20 °C gap: 42.6 °C, 70.6 %.
        assert_eq!(e.duty("hub/fan1"), Duty::Set(71));
        for _ in 0..20 {
            e.step(&|_| Some(50.0), 1.0, false);
        }
        assert_eq!(e.duty("hub/fan1"), Duty::Set(100));
        e.step(&|_| Some(30.0), 1.0, false);
        // 1 - e^-0.1 of the way down: 48.1 °C, 92 %.
        assert_eq!(e.duty("hub/fan1"), Duty::Set(92));
    }

    #[test]
    fn hysteresis_holds_small_changes() {
        let toml = GRAPH.replace("response_up = 0", "hysteresis = 3\nresponse_up = 0");
        let mut e = engine(&toml);
        e.step(&|_| Some(35.0), 1.0, false);
        e.step(&|_| Some(37.0), 1.0, false);
        assert_eq!(e.duty("hub/fan1"), Duty::Set(40));
        e.step(&|_| Some(38.0), 1.0, false);
        assert_eq!(e.duty("hub/fan1"), Duty::Set(52));
        e.step(&|_| Some(36.0), 1.0, false);
        assert_eq!(e.duty("hub/fan1"), Duty::Set(52));
    }

    #[test]
    fn steps_limit_the_rate() {
        let mut e = engine(&format!("{GRAPH}step_up = 10\nstep_down = 2\n"));
        e.step(&|_| Some(30.0), 1.0, false);
        e.step(&|_| Some(50.0), 1.0, false);
        assert_eq!(e.duty("hub/fan1"), Duty::Set(30));
        e.step(&|_| Some(30.0), 1.0, false);
        assert_eq!(e.duty("hub/fan1"), Duty::Set(28));
    }

    #[test]
    fn mixes_and_triggers() {
        let toml = "\
[[curve]]\nkind = \"fixed\"\nname = \"F\"\nduty = 35\n\
[[curve]]\nkind = \"trigger\"\nname = \"T\"\nsensor = \"gpu.temp\"\nidle_duty = 20\nload_duty = 80\nidle_at = 50\nload_at = 60\nresponse_up = 0\nresponse_down = 0\n\
[[curve]]\nkind = \"mix\"\nname = \"M\"\ncurves = [\"F\", \"T\"]\nfunction = \"max\"\n\
[[fan]]\nchannel = \"hub/fan1\"\ncurve = \"M\"\n";
        let mut e = engine(toml);
        let mut at = |t: f32| {
            e.step(&|_| Some(t), 1.0, false);
            e.duty("hub/fan1")
        };
        assert_eq!(at(55.0), Duty::Set(35));
        assert_eq!(at(60.0), Duty::Set(80));
        assert_eq!(at(55.0), Duty::Set(80));
        assert_eq!(at(50.0), Duty::Set(35));
    }

    #[test]
    fn device_curve_folds_in_limits() {
        let mut e = engine(&format!("{GRAPH}offset = 5\nmin = 30\n"));
        e.step(&|_| Some(35.0), 1.0, false);
        let d = e.device_curve("hub/fan1").unwrap();
        assert_eq!(d.sensor.as_deref(), Some("cpu.temp"));
        assert_eq!((d.duty_at(20.0), d.duty_at(35.0), d.duty_at(60.0)), (30, 45, 100));
    }
}
