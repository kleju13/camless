//! Sensor history, for the app's charts.
//!
//! The service records every sensor, and the speed and duty of every fan, at most
//! once a second. The last hour stays in memory at that rate. The average and the
//! highest value of each minute go to disk, one CSV file per UTC day in `history/`
//! next to `camless.toml`, kept for 30 days.
//!
//! A file holds a header line, `#t,<series>,<series>,...`, then a line a minute: the
//! minute's start in Unix seconds, then the average and the highest value of each
//! series, both empty for a series without a reading that minute. A new header comes
//! whenever the set of series changes, e.g. when a device is plugged in, and after
//! each start of the service. A line that a crash cut short is skipped.

use serde::Serialize;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// How far back the samples in memory go.
pub const LIVE_SPAN_S: i64 = 3600;
/// How long minute files are kept, in days.
pub const KEEP_DAYS: i64 = 30;
/// The most points a query returns for each series.
const MAX_POINTS: i64 = 720;
/// Samples closer together than this are dropped; a little under a second, so a
/// one-second interval that runs early now and then keeps every sample.
const MIN_GAP_MS: i64 = 900;

/// One reading of every series at a moment. A value of NaN is a missing reading.
struct Sample {
    ms: i64,
    values: Vec<f32>,
}

/// One series over a minute.
#[derive(Clone, Copy)]
struct Acc {
    sum: f64,
    n: u32,
    max: f32,
}

impl Acc {
    fn add(&mut self, v: f32) {
        self.sum += v as f64;
        self.n += 1;
        self.max = self.max.max(v);
    }
}

struct Minute {
    /// Unix seconds of its start.
    start: i64,
    series: BTreeMap<String, Acc>,
}

pub struct History {
    /// Where minute files go; without one, history stays in memory only.
    dir: Option<PathBuf>,
    /// Series ids by index, as the samples store them.
    names: Vec<String>,
    index: HashMap<String, usize>,
    live: VecDeque<Sample>,
    minute: Option<Minute>,
    /// The day and the series of the last header written, so a file gets a new one
    /// only when either changes.
    header: Option<(i64, Vec<String>)>,
    /// The day files were last pruned on.
    pruned: Option<i64>,
    /// Why minutes are not being written, said once.
    error: Option<String>,
}

/// What a query returns: `count` points `step` seconds apart from `from`, and each
/// series' average and highest value at every point, `null` where there was none.
#[derive(Debug, Default, PartialEq, Serialize)]
pub struct Range {
    pub from: i64,
    pub step: i64,
    pub count: usize,
    pub series: BTreeMap<String, Series>,
}

#[derive(Debug, Default, PartialEq, Serialize)]
pub struct Series {
    pub avg: Vec<Option<f32>>,
    pub max: Vec<Option<f32>>,
}

impl History {
    pub fn new(dir: Option<PathBuf>) -> History {
        History {
            dir,
            names: Vec::new(),
            index: HashMap::new(),
            live: VecDeque::new(),
            minute: None,
            header: None,
            pruned: None,
            error: None,
        }
    }

    /// Every series' value at `ms` (Unix milliseconds). Ids with a comma or a line
    /// break cannot go in a file and are left out.
    pub fn record(&mut self, ms: i64, values: &[(String, f32)]) {
        if self.live.back().is_some_and(|s| (0..MIN_GAP_MS).contains(&(ms - s.ms))) {
            return;
        }
        let mut sample = vec![f32::NAN; self.names.len()];
        let start = ms.div_euclid(60_000) * 60;
        if self.minute.as_ref().is_some_and(|m| m.start != start) {
            let done = self.minute.take().unwrap();
            self.write(&done);
        }
        let minute = self.minute.get_or_insert_with(|| Minute { start, series: BTreeMap::new() });
        for (id, v) in values {
            if !v.is_finite() || id.contains([',', '\n', '\r']) {
                continue;
            }
            let i = match self.index.get(id) {
                Some(&i) => i,
                None => {
                    self.names.push(id.clone());
                    self.index.insert(id.clone(), self.names.len() - 1);
                    sample.push(f32::NAN);
                    self.names.len() - 1
                }
            };
            sample[i] = *v;
            minute.series.entry(id.clone()).or_insert(Acc { sum: 0.0, n: 0, max: f32::MIN }).add(*v);
        }
        self.live.push_back(Sample { ms, values: sample });
        while self.live.front().is_some_and(|s| ms - s.ms > LIVE_SPAN_S * 1000) {
            self.live.pop_front();
        }
    }

    /// Writes the minute being gathered, so a stop loses none of it.
    pub fn finish(&mut self) {
        if let Some(m) = self.minute.take() {
            self.write(&m);
        }
    }

    fn write(&mut self, m: &Minute) {
        let Some(dir) = self.dir.clone() else { return };
        let day = m.start.div_euclid(86_400);
        if let Err(e) = self.append(&dir, day, m) {
            let msg = format!("{e:#}");
            if self.error.as_ref() != Some(&msg) {
                eprintln!("history: not written to {}: {msg}", dir.display());
            }
            self.error = Some(msg);
            // The next minute starts the file again with a header.
            self.header = None;
            return;
        }
        self.error = None;
        if self.pruned != Some(day) {
            self.pruned = Some(day);
            prune(&dir, day - KEEP_DAYS);
        }
    }

    fn append(&mut self, dir: &Path, day: i64, m: &Minute) -> anyhow::Result<()> {
        std::fs::create_dir_all(dir)?;
        let names: Vec<String> = m.series.keys().cloned().collect();
        let mut text = String::new();
        if self.header.as_ref() != Some(&(day, names.clone())) {
            text.push_str(&header_line(&names));
        }
        text.push_str(&data_line(m));
        append_lines(&dir.join(day_file(day)), &text)?;
        self.header = Some((day, names));
        Ok(())
    }

    /// The samples in memory from `from` on, in buckets of `step` seconds.
    fn live_range(&self, from: i64, step: i64, count: usize, wanted: &dyn Fn(&str) -> bool) -> Range {
        let mut b = Buckets::new(from, step, count);
        let cols: Vec<(usize, &str)> =
            self.names.iter().enumerate().filter(|(_, n)| wanted(n)).map(|(i, n)| (i, n.as_str())).collect();
        for s in &self.live {
            let Some(at) = b.at(s.ms.div_euclid(1000)) else { continue };
            for &(i, name) in &cols {
                if let Some(&v) = s.values.get(i).filter(|v| v.is_finite()) {
                    b.add(name, at, v as f64, 1, v);
                }
            }
        }
        b.finish()
    }
}

/// History over the last `span` seconds up to `now` (Unix seconds), at most
/// [`MAX_POINTS`] points a series. The last hour comes from memory, longer spans
/// from the minute files. `ids` limits the series; without it all are returned.
pub fn query(h: &std::sync::Mutex<History>, now: i64, span: i64, ids: Option<&[String]>) -> Range {
    let span = span.clamp(60, KEEP_DAYS * 86_400);
    let wanted = |name: &str| ids.is_none_or(|ids| ids.iter().any(|i| i == name));
    let lock = || h.lock().unwrap_or_else(|e| e.into_inner());
    if span <= LIVE_SPAN_S {
        let step = (span + MAX_POINTS - 1) / MAX_POINTS;
        let (from, count) = buckets(now, span, step);
        return lock().live_range(from, step, count, &wanted);
    }
    // Whole minutes, so every bucket holds the same number of them.
    let step = ((span + MAX_POINTS - 1) / MAX_POINTS + 59) / 60 * 60;
    let (from, count) = buckets(now, span, step);
    let Some(dir) = lock().dir.clone() else { return Buckets::new(from, step, count).finish() };
    let mut b = Buckets::new(from, step, count);
    for day in from.div_euclid(86_400)..=now.div_euclid(86_400) {
        if let Ok(f) = std::fs::File::open(dir.join(day_file(day))) {
            read_day(std::io::BufReader::new(f), &mut b, &wanted);
        }
    }
    b.finish()
}

/// The first bucket's start, aligned to `step` so points stay put between queries,
/// and how many buckets it takes to reach `now`.
fn buckets(now: i64, span: i64, step: i64) -> (i64, usize) {
    let from = (now - span).div_euclid(step) * step;
    (from, ((now - from).div_euclid(step) + 1) as usize)
}

struct Buckets {
    from: i64,
    step: i64,
    count: usize,
    series: BTreeMap<String, (Vec<f64>, Vec<u32>, Vec<f32>)>,
}

impl Buckets {
    fn new(from: i64, step: i64, count: usize) -> Buckets {
        Buckets { from, step, count, series: BTreeMap::new() }
    }

    /// The bucket that Unix second `t` falls in.
    fn at(&self, t: i64) -> Option<usize> {
        let i = (t - self.from).div_euclid(self.step);
        (0..self.count as i64).contains(&i).then_some(i as usize)
    }

    fn add(&mut self, name: &str, at: usize, sum: f64, n: u32, max: f32) {
        let count = self.count;
        let (sums, ns, maxes) = self
            .series
            .entry(name.to_string())
            .or_insert_with(|| (vec![0.0; count], vec![0; count], vec![f32::MIN; count]));
        sums[at] += sum;
        ns[at] += n;
        maxes[at] = maxes[at].max(max);
    }

    fn finish(self) -> Range {
        let series = self
            .series
            .into_iter()
            .map(|(name, (sums, ns, maxes))| {
                let avg = sums.iter().zip(&ns).map(|(s, &n)| (n > 0).then(|| round1((s / n as f64) as f32))).collect();
                let max = maxes.iter().zip(&ns).map(|(&m, &n)| (n > 0).then(|| round1(m))).collect();
                (name, Series { avg, max })
            })
            .collect();
        Range { from: self.from, step: self.step, count: self.count, series }
    }
}

/// Adds one day file's minutes to `b`. Each minute counts once, whatever number of
/// samples it was made from.
fn read_day(r: impl BufRead, b: &mut Buckets, wanted: &dyn Fn(&str) -> bool) {
    let mut cols: Vec<Option<String>> = Vec::new();
    for line in r.lines() {
        let Ok(line) = line else { break };
        if let Some(head) = line.strip_prefix('#') {
            let mut names = head.split(',');
            if names.next() != Some("t") {
                cols.clear();
                continue;
            }
            cols = names.map(|n| wanted(n).then(|| n.to_string())).collect();
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        if cols.is_empty() || fields.len() != 1 + 2 * cols.len() {
            continue;
        }
        let Some(at) = fields[0].parse().ok().and_then(|t| b.at(t)) else { continue };
        for (i, name) in cols.iter().enumerate() {
            let Some(name) = name else { continue };
            let (Ok(avg), Ok(max)) = (fields[1 + 2 * i].parse::<f32>(), fields[2 + 2 * i].parse::<f32>()) else { continue };
            if avg.is_finite() && max.is_finite() {
                b.add(name, at, avg as f64, 1, max);
            }
        }
    }
}

/// Appends `text`, whole lines, to `path` in one write. When a crash cut the file's
/// last line short, `text` starts on a line of its own, so only the cut line is lost.
pub fn append_lines(path: &Path, text: &str) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new().create(true).read(true).append(true).open(path)?;
    let torn = f.seek(SeekFrom::End(0))? > 0 && {
        let mut last = [0u8];
        f.seek(SeekFrom::End(-1))?;
        f.read_exact(&mut last)?;
        last[0] != b'\n'
    };
    f.write_all(if torn { format!("\n{text}") } else { text.to_string() }.as_bytes())
}

fn header_line(names: &[String]) -> String {
    let mut s = String::from("#t");
    for n in names {
        s.push(',');
        s.push_str(n);
    }
    s.push('\n');
    s
}

fn data_line(m: &Minute) -> String {
    let mut s = m.start.to_string();
    for a in m.series.values() {
        s.push(',');
        s.push_str(&num((a.sum / a.n as f64) as f32));
        s.push(',');
        s.push_str(&num(a.max));
    }
    s.push('\n');
    s
}

fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

/// One decimal at most: `45.1`, `1200`.
fn num(v: f32) -> String {
    let r = round1(v);
    if r.fract() == 0.0 {
        format!("{}", r as i64)
    } else {
        format!("{r:.1}")
    }
}

/// `2026-10-20.csv` for days since 1970-01-01.
fn day_file(day: i64) -> String {
    let (y, m, d) = civil(day);
    format!("{y:04}-{m:02}-{d:02}.csv")
}

/// The day a file is for, if its name is one of ours.
fn file_day(name: &str) -> Option<i64> {
    let stem = name.strip_suffix(".csv")?;
    let mut parts = stem.split('-');
    let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return None;
    }
    let (y, m, d) = (y.parse().ok()?, m.parse().ok()?, d.parse().ok()?);
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then(|| days_from_civil(y, m, d))
}

/// Deletes the day files from before `oldest`.
fn prune(dir: &Path, oldest: i64) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        if name.to_str().and_then(file_day).is_some_and(|d| d < oldest) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// (year, month, day) of a day since 1970-01-01, after Howard Hinnant's
/// `civil_from_days`.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn v(pairs: &[(&str, f32)]) -> Vec<(String, f32)> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn dates_round_trip() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(day_file(10_957), "2000-01-01.csv");
        // 2024-02-29, a leap day, and the day after.
        let leap = days_from_civil(2024, 2, 29);
        assert_eq!(civil(leap), (2024, 2, 29));
        assert_eq!(civil(leap + 1), (2024, 3, 1));
        for day in [-1, 0, 59, 10_957, 20_000, 30_000] {
            let (y, m, d) = civil(day);
            assert_eq!(days_from_civil(y, m, d), day);
            assert_eq!(file_day(&day_file(day)), Some(day));
        }
        assert_eq!(file_day("notes.csv"), None);
        assert_eq!(file_day("2026-13-01.csv"), None);
        assert_eq!(file_day("2026-10-20.csv.bak"), None);
    }

    #[test]
    fn numbers_take_one_decimal_at_most() {
        assert_eq!(num(45.14), "45.1");
        assert_eq!(num(1200.0), "1200");
        assert_eq!(num(0.04), "0");
        assert_eq!(num(-3.24), "-3.2");
    }

    #[test]
    fn minutes_go_to_a_file_per_day_and_come_back() {
        let dir = temp_dir("camless-history-files");
        let h = Mutex::new(History::new(Some(dir.clone())));
        // 2026-10-20 00:00:00 UTC.
        let t0 = days_from_civil(2026, 10, 20) * 86_400;
        {
            let mut h = h.lock().unwrap();
            for s in 0..180 {
                let temp = if s < 60 { 40.0 } else { 50.0 + (s % 2) as f32 };
                let mut vals = v(&[("cpu.temp", temp)]);
                if s >= 60 {
                    vals.push(("control-hub/fan1.rpm".into(), 1000.0));
                }
                h.record((t0 + s) * 1000, &vals);
            }
            h.finish();
        }
        let text = std::fs::read_to_string(dir.join("2026-10-20.csv")).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines,
            [
                "#t,cpu.temp".to_string(),
                format!("{t0},40,40"),
                "#t,control-hub/fan1.rpm,cpu.temp".to_string(),
                format!("{},1000,1000,50.5,51", t0 + 60),
                format!("{},1000,1000,50.5,51", t0 + 120),
            ]
        );
        // Two hours at a point a minute.
        let r = query(&h, t0 + 7200, 7200, None);
        assert_eq!((r.from, r.step, r.count), (t0, 60, 121));
        let at = |t: i64| ((t - r.from) / r.step) as usize;
        let cpu = &r.series["cpu.temp"];
        assert_eq!(cpu.avg[at(t0)], Some(40.0));
        assert_eq!(cpu.max[at(t0 + 60)], Some(51.0));
        assert_eq!(cpu.avg[at(t0 + 180)], None);
        assert_eq!(r.series["control-hub/fan1.rpm"].avg[at(t0)], None);
        // Only what was asked for.
        let r = query(&h, t0 + 7200, 7200, Some(&["cpu.temp".to_string()]));
        assert_eq!(r.series.keys().collect::<Vec<_>>(), ["cpu.temp"]);
    }

    #[test]
    fn a_line_cut_short_is_skipped() {
        let t0 = 1_000_000_020 / 60 * 60;
        let text = format!("#t,a,b\n{t0},1,2,3,4\n{},1,2,3\n{},5,6,7,8\n", t0 + 60, t0 + 120);
        let mut b = Buckets::new(t0, 60, 3);
        read_day(text.as_bytes(), &mut b, &|_| true);
        let r = b.finish();
        assert_eq!(r.series["a"].avg, [Some(1.0), None, Some(5.0)]);
        assert_eq!(r.series["b"].max, [Some(4.0), None, Some(8.0)]);
    }

    #[test]
    fn a_header_after_a_cut_line_starts_a_line() {
        let dir = temp_dir("camless-history-torn");
        let t0 = days_from_civil(2026, 10, 20) * 86_400;
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("2026-10-20.csv"), format!("#t,cpu.temp\n{t0},40,40\n{},4", t0 + 60)).unwrap();
        let h = Mutex::new(History::new(Some(dir.clone())));
        {
            let mut h = h.lock().unwrap();
            for s in 120..180 {
                h.record((t0 + s) * 1000, &v(&[("cpu.temp", 50.0)]));
            }
            h.finish();
        }
        let text = std::fs::read_to_string(dir.join("2026-10-20.csv")).unwrap();
        assert!(text.ends_with(&format!("{},4\n#t,cpu.temp\n{},50,50\n", t0 + 60, t0 + 120)), "{text}");
        let r = query(&h, t0 + 7200, 7200, None);
        let cpu = &r.series["cpu.temp"];
        assert_eq!(cpu.avg[0], Some(40.0));
        assert_eq!(cpu.avg[1], None);
        assert_eq!(cpu.avg[2], Some(50.0));
    }

    #[test]
    fn the_last_hour_comes_from_memory() {
        let h = Mutex::new(History::new(None));
        let t0 = 1_800_000_000;
        {
            let mut h = h.lock().unwrap();
            for s in 0..4000 {
                h.record((t0 + s) * 1000, &v(&[("gpu.temp", s as f32)]));
                // A second sample within the same second is dropped.
                h.record((t0 + s) * 1000 + 300, &v(&[("gpu.temp", -1.0)]));
            }
        }
        let now = t0 + 3999;
        let r = query(&h, now, 600, None);
        assert_eq!((r.step, r.count), (1, 601));
        let g = &r.series["gpu.temp"];
        assert_eq!(g.avg.last().copied().flatten(), Some(3999.0));
        assert_eq!(g.avg[0], Some(3399.0));
        // An hour in five-second buckets; samples older than an hour are gone.
        let r = query(&h, now, 3600, None);
        assert_eq!((r.step, r.count), (5, 721));
        let g = &r.series["gpu.temp"];
        assert_eq!(g.max.last().copied().flatten(), Some(3999.0));
        assert!(g.avg.iter().flatten().all(|&x| x >= 399.0));
    }

    #[test]
    fn old_days_are_deleted() {
        let dir = temp_dir("camless-history-prune");
        std::fs::create_dir_all(&dir).unwrap();
        let today = days_from_civil(2026, 10, 20);
        for day in [today - 31, today - 30, today - 1] {
            std::fs::write(dir.join(day_file(day)), "#t\n").unwrap();
        }
        std::fs::write(dir.join("keep-me.txt"), "").unwrap();
        let mut h = History::new(Some(dir.clone()));
        h.record(today * 86_400_000, &v(&[("cpu.load", 5.0)]));
        h.finish();
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, ["2026-09-20.csv", "2026-10-19.csv", "2026-10-20.csv", "keep-me.txt"]);
    }
}
