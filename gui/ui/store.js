/* camless's data for the page: the daemon's status and the config draft, both from
   the app's own API (backend.rs). The page edits the draft; it is saved a moment
   after the last change, and the daemon applies it by itself. */
(function () {
  'use strict';

  const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));
  const lerp = (a, b, t) => a + (b - a) * t;
  const clone = (o) => (o == null ? o : JSON.parse(JSON.stringify(o)));
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
  const smoothstep = (a, b, x) => { const t = clamp((x - a) / (b - a), 0, 1); return t * t * (3 - 2 * t); };
  const wobble = (t, seed = 0) => Math.sin(t * 0.71 + seed) * 0.5 + Math.sin(t * 1.93 + seed * 2.1) * 0.3 + Math.sin(t * 4.37 + seed * 3.7) * 0.2;

  /* ------------------------------------------------------------------- bus */

  const listeners = {};
  function on(evt, fn) {
    (listeners[evt] = listeners[evt] || []).push(fn);
    // The animation clock runs only while a picture listens.
    if (evt === 'frame') schedule();
    return () => off(evt, fn);
  }
  function off(evt, fn) { listeners[evt] = (listeners[evt] || []).filter((f) => f !== fn); }
  // A listener dropped by an earlier one, such as a page built again, is not called.
  function emit(evt, arg) {
    for (const f of (listeners[evt] || []).slice()) {
      if (!(listeners[evt] || []).includes(f)) continue;
      try { f(arg); } catch (e) { console.error(e); }
    }
  }

  /* ------------------------------------------------------------------- api */

  async function api(method, path, body) {
    const init = { method, cache: 'no-store' };
    if (method !== 'GET') { init.headers = { 'Content-Type': 'application/json' }; init.body = JSON.stringify(body ?? {}); }
    const r = await fetch('/api/' + path, init);
    const text = await r.text();
    let data = {};
    try { data = text ? JSON.parse(text) : {}; } catch (e) { /* not JSON */ }
    if (!r.ok) throw new Error(data.error || `${path} answered ${r.status}`);
    return data;
  }

  /* ----------------------------------------------------------------- state */

  const st = {
    server: null,
    status: null,
    // The draft; null while the file on disk does not load.
    config: null,
    // The config on disk as last seen, to tell edits made elsewhere.
    disk: null,
    dirty: false,
    saving: null,
    // What failed to save and why; tried again after the next edit.
    failed: null,
    saveError: null,
    linkDown: 0,
  };

  let polling = 0;
  async function poll() {
    clearTimeout(polling);
    try {
      const s = await api('GET', 'state');
      take(s);
      st.linkDown = 0;
    } catch (e) {
      st.linkDown++;
      emit('state');
    }
    polling = setTimeout(poll, document.hidden ? 4000 : 1000);
  }

  function take(s) {
    const first = !st.server;
    st.server = s;
    st.status = s.status;
    if (s.config) {
      const disk = JSON.stringify(s.config);
      // Edits made elsewhere, e.g. by hand, are taken unless there are edits here.
      if (!st.config || (disk !== st.disk && !st.dirty && !st.saving)) {
        st.config = s.config;
        st.disk = disk;
        st.failed = null;
        st.saveError = null;
        emit('config', 'replace');
      }
    } else if (st.config || first) {
      st.config = null;
      st.disk = null;
      st.dirty = false;
      emit('config', 'replace');
    }
    emit('state');
  }

  /* ------------------------------------------------------------------ edit */

  const SAVE_AFTER = 350;
  let saveTimer = 0;

  /* Changes the draft. o.live: the user is still dragging, so it waits for the
     final change before it saves. */
  function edit(fn, o = {}) {
    if (!st.config) return;
    fn(st.config);
    st.dirty = true;
    emit('config', o.what || 'edit');
    clearTimeout(saveTimer);
    if (!o.live) saveTimer = setTimeout(save, o.now ? 0 : SAVE_AFTER);
  }

  async function save() {
    clearTimeout(saveTimer);
    while (st.saving) await st.saving;
    if (!st.dirty || !st.config) return;
    const sent = JSON.stringify(st.config);
    if (sent === st.failed) return;
    st.dirty = false;
    const run = (async () => {
      try {
        const r = await api('PUT', 'config', JSON.parse(sent));
        st.disk = JSON.stringify(r.config);
        st.failed = null;
        st.saveError = null;
        // What was written, unless the draft moved on meanwhile.
        if (!st.dirty && st.disk !== sent) { st.config = r.config; emit('config', 'replace'); }
      } catch (e) {
        st.dirty = true;
        st.failed = sent;
        st.saveError = e.message;
      }
      emit('saved');
    })();
    st.saving = run;
    try { await run; } finally { st.saving = null; }
  }

  // The app asks for this before it closes the window.
  window.camlessClose = () => save();
  addEventListener('pagehide', () => { save(); });

  async function action(path, body) {
    await save();
    const r = await api('POST', path, body);
    await poll();
    return r;
  }

  /* ---------------------------------------------------------------- curves */

  // Linear between points, flat beyond the ends; unrounded, for drawing.
  function dutyAtExact(points, t) {
    if (!points || !points.length) return 100;
    if (t <= points[0][0]) return points[0][1];
    for (let i = 1; i < points.length; i++) {
      const [t0, d0] = points[i - 1], [t1, d1] = points[i];
      if (t <= t1) return d0 + ((t - t0) / (t1 - t0)) * (d1 - d0);
    }
    return points[points.length - 1][1];
  }
  const dutyAt = (points, t) => Math.round(dutyAtExact(points, t));

  // Sorted by value, one point per value, duties never falling.
  function normalize(points) {
    const pts = points.map(([t, d]) => [t, clamp(Math.round(d), 0, 100)]).sort((a, b) => a[0] - b[0]);
    const out = [];
    for (const p of pts) if (!out.length || out[out.length - 1][0] !== p[0]) out.push(p);
    let max = 0;
    for (const p of out) { max = Math.max(max, p[1]); p[1] = max; }
    return out;
  }

  // Drag point i to (t, d), keeping values ordered and pushing neighbours so the
  // curve stays non-decreasing. Returns a new array.
  function dragPoint(points, i, t, d, range, minDuty = 0) {
    const pts = points.map((p) => p.slice());
    const lo = i > 0 ? pts[i - 1][0] + 1 : range[0];
    const hi = i < pts.length - 1 ? pts[i + 1][0] - 1 : range[1];
    pts[i][0] = Math.round(clamp(t, lo, Math.max(lo, hi)));
    pts[i][1] = Math.round(clamp(d, minDuty, 100));
    for (let j = i + 1; j < pts.length; j++) pts[j][1] = Math.max(pts[j][1], pts[i][1]);
    for (let j = i - 1; j >= 0; j--) pts[j][1] = Math.min(pts[j][1], pts[i][1]);
    return pts;
  }

  function addPoint(points, t, range) {
    t = Math.round(clamp(t, range[0], range[1]));
    if (points.some((p) => p[0] === t)) return { points, index: points.findIndex((p) => p[0] === t) };
    const pts = normalize([...points.map((p) => p.slice()), [t, dutyAt(points, t)]]);
    return { points: pts, index: pts.findIndex((p) => p[0] === t) };
  }

  const removePoint = (points, i) => (points.length <= 2 ? points : points.filter((_, j) => j !== i));

  // The Kraken's own floor: 0 % at 45 °C liquid up to 100 % at 55 °C.
  const safetyFloor = (liquid) => clamp((Math.floor(liquid) - 45) * 10, 0, 100);

  /* --------------------------------------------------------------- sensors */

  const UNIT = { celsius: '°C', percent: '%', watt: 'W' };
  const FALLBACK_SENSORS = [
    ['cpu.temp', 'CPU temperature', 'celsius'], ['cpu.load', 'CPU load', 'percent'], ['cpu.power', 'CPU power', 'watt'],
    ['gpu.temp', 'GPU temperature', 'celsius'], ['gpu.load', 'GPU load', 'percent'], ['gpu.power', 'GPU power', 'watt'],
    ['ram.load', 'Memory in use', 'percent'],
  ];

  // The daemon's sensors, or the usual ones and those the config names while it is not running.
  function sensors() {
    if (st.status) return st.status.sensors || [];
    const out = FALLBACK_SENSORS.map(([id, name, unit]) => ({ id, name, unit, value: null }));
    for (const c of curves()) if (c.sensor && !out.some((s) => s.id === c.sensor)) out.push({ id: c.sensor, name: c.sensor, unit: unitGuess(c.sensor), value: null });
    return out;
  }
  const unitGuess = (id) => (/load$/.test(id || '') ? 'percent' : /power$/.test(id || '') ? 'watt' : 'celsius');
  const sensor = (id) => sensors().find((s) => s.id === id) || null;
  const value = (id) => { const s = st.status && st.status.sensors.find((x) => x.id === id); return s ? s.value : null; };
  const unitOf = (id) => (sensor(id) || { unit: unitGuess(id) }).unit;
  const isLiquid = (id) => /\.liquid$/.test(id || '');

  // What CAM calls the source: Liquid, CPU or GPU, else the sensor's own name.
  function sourceLabel(id) {
    if (id === 'cpu.temp') return 'CPU';
    if (id === 'gpu.temp') return 'GPU';
    if (isLiquid(id)) {
      const liquids = sensors().filter((s) => isLiquid(s.id));
      if (liquids.length < 2) return 'Liquid';
      const dev = device(id.slice(0, -'.liquid'.length));
      return `Liquid (${dev ? dev.name : id})`;
    }
    const s = sensor(id);
    return s ? s.name : id;
  }

  // The x range shown for a sensor, widened to fit the points.
  function range(id, points) {
    const unit = unitOf(id);
    let [lo, hi] = isLiquid(id) ? [20, 60] : unit === 'percent' ? [0, 100] : unit === 'watt' ? [0, 400] : [20, 100];
    for (const p of points || []) { lo = Math.min(lo, p[0]); hi = Math.max(hi, p[0]); }
    return [lo, Math.max(hi, lo + 1)];
  }

  /* CAM's two ready-made curves for a sensor, as the old app had them, and where
     Custom starts when there is nothing to go back to. */
  function presets(id) {
    const unit = unitOf(id);
    if (isLiquid(id)) return { silent: [[25, 25], [30, 30], [35, 40], [40, 60], [45, 80], [50, 100]], performance: [[25, 40], [30, 50], [35, 65], [40, 85], [45, 100]], custom: [[25, 30], [30, 40], [35, 50], [40, 70], [45, 90], [50, 100]] };
    if (unit === 'percent') return { silent: [[0, 25], [50, 35], [80, 60], [100, 80]], performance: [[0, 40], [40, 55], [70, 85], [90, 100]], custom: [[0, 30], [50, 45], [80, 70], [100, 90]] };
    if (unit === 'watt') return { silent: [[0, 25], [100, 35], [200, 60], [300, 85]], performance: [[0, 40], [80, 55], [160, 80], [250, 100]], custom: [[0, 30], [100, 45], [200, 70], [300, 95]] };
    return { silent: [[30, 25], [50, 30], [65, 45], [75, 70], [85, 100]], performance: [[30, 40], [50, 50], [65, 70], [75, 90], [80, 100]], custom: [[30, 30], [50, 40], [65, 55], [75, 80], [85, 100]] };
  }

  /* --------------------------------------------------------------- devices */

  const devices = () => (st.status && st.status.devices) || [];
  const device = (id) => devices().find((d) => d.id === id) || null;
  const devOf = (channel) => String(channel).split('/')[0];

  /* ------------------------------------------------------------------ fans */

  const cfg = () => st.config;
  const curves = () => (st.config && st.config.curve) || [];
  const curve = (name) => curves().find((c) => c.name === name) || null;

  // Every fan and pump: from the daemon when it runs, plus those only the config names.
  function fans() {
    const out = [];
    for (const d of devices()) {
      for (const f of d.fans) out.push({ id: `${d.id}/${f.id}`, dev: d.id, device: d, hw: f, pump: f.pump, minDuty: f.min_duty, onDeviceCurves: f.device_curves });
    }
    for (const f of (st.config && st.config.fan) || []) {
      if (!out.some((c) => c.id === f.channel)) out.push({ id: f.channel, dev: devOf(f.channel), device: device(devOf(f.channel)), hw: null, pump: /pump/.test(f.channel), minDuty: 0, onDeviceCurves: false, missing: true });
    }
    return out;
  }
  const fan = (id) => fans().find((f) => f.id === id) || null;
  const fanCfg = (id) => ((st.config && st.config.fan) || []).find((f) => f.channel === id) || null;
  function fanName(id) {
    const c = fanCfg(id), f = fan(id);
    return (c && c.name) || (f && f.hw && f.hw.name.trim()) || id.split('/')[1] || id;
  }
  const fanValue = (id) => (st.status && st.status.fans.find((f) => f.channel === id)) || null;
  const curveValue = (name) => (st.status && st.status.curves.find((c) => c.name === name)) || null;

  // The entry for a fan, made if it has none yet.
  function ensureFan(c, id) {
    c.fan = c.fan || [];
    let f = c.fan.find((x) => x.channel === id);
    if (!f) { f = { channel: id }; c.fan.push(f); c.fan.sort((a, b) => (a.channel < b.channel ? -1 : 1)); }
    return f;
  }
  // A fan entry that holds only defaults says nothing, so it goes.
  function tidyFans(c) {
    c.fan = (c.fan || []).filter((f) => f.curve || f.name || f.offset || f.min || (f.max != null && f.max !== 100) || f.step_up || f.step_down);
  }

  function uniqueCurveName(base, c = st.config) {
    const taken = new Set((c.curve || []).map((x) => x.name));
    let name = base.trim() || 'Curve', n = 2;
    const root = name;
    while (taken.has(name)) name = `${root} ${n++}`;
    return name;
  }

  // Who uses a curve: fans by channel, and mixes by name.
  function users(name) {
    const fansUsing = ((st.config && st.config.fan) || []).filter((f) => f.curve === name).map((f) => f.channel);
    const mixes = curves().filter((c) => c.kind === 'mix' && c.curves.includes(name)).map((c) => c.name);
    return { fans: fansUsing, mixes };
  }

  // The sensor a new curve for a fan follows: the fan's own cooler's liquid, any
  // liquid, then the CPU. Liquid needs no driver or administrator rights.
  function defaultSensor(id) {
    const ids = sensors().map((s) => s.id);
    const own = `${devOf(id)}.liquid`;
    if (ids.includes(own)) return own;
    const f = fan(id);
    const liquid = ids.find(isLiquid);
    if (liquid && f && f.onDeviceCurves) return liquid;
    if (ids.includes('cpu.temp')) return 'cpu.temp';
    return liquid || ids[0] || 'cpu.temp';
  }

  /* CAM gives each channel a preset: Fixed, Silent, Performance or Custom. Here a fan
     follows a named curve, so the preset is read from that curve. Mixes and triggers
     have none: they are edited with the curves. */
  function preset(id) {
    const f = fanCfg(id);
    const c = f && f.curve ? curve(f.curve) : null;
    if (!c) return 'none';
    if (c.kind === 'fixed') return 'fixed';
    if (c.kind !== 'graph') return 'advanced';
    const p = presets(c.sensor);
    if (same(c.points, p.silent)) return 'silent';
    if (same(c.points, p.performance)) return 'performance';
    return 'custom';
  }

  /* What a curve held before a preset or a source replaced it, so going back to
     Custom, Fixed or that source finds it again: its custom points by sensor, its
     fixed duty and its sensor. Kept on this PC for each profile, so a fan put on
     Fixed for a while, say to tell which fan is which, gets its own curve back
     after a restart too. */
  const MEMO_KEY = 'camless-curve-memo';
  const memos = (() => { try { return JSON.parse(localStorage.getItem(MEMO_KEY)) || {}; } catch (e) { return {}; } })();
  const memoProfile = () => (st.config && st.config.profile) || '';
  const memoBook = () => (memos[memoProfile()] = memos[memoProfile()] || {});
  function memoOf(name) {
    const book = memoBook();
    return (book[name] = book[name] || { custom: {}, duty: null });
  }
  function keepMemo() {
    try { localStorage.setItem(MEMO_KEY, JSON.stringify(memos)); } catch (e) { /* storage off */ }
  }
  on('config', (what) => {
    if (what === 'replace') { const book = memoBook(); for (const k of Object.keys(book)) if (!curve(k)) delete book[k]; }
    if (what === 'replace' || what === 'cooling' || what === 'curve') keepMemo();
  });

  // The fan's curve, made for it if it has none.
  function ownCurve(c, id, make) {
    const f = ensureFan(c, id);
    let cur = f.curve && (c.curve || []).find((x) => x.name === f.curve);
    if (!cur) {
      cur = make(uniqueCurveName(fanName(id), c));
      c.curve = c.curve || [];
      c.curve.push(cur);
      f.curve = cur.name;
    }
    return cur;
  }

  function graphOf(name, sensorId, points) {
    return { kind: 'graph', name, sensor: sensorId, points };
  }

  function setPreset(id, p) {
    edit((c) => {
      const s = defaultSensor(id);
      const cur = ownCurve(c, id, (name) => (p === 'fixed' ? { kind: 'fixed', name, duty: 50 } : graphOf(name, s, clone(presets(s)[p]))));
      const m = memoOf(cur.name);
      if (cur.kind === 'graph' && preset(id) === 'custom') m.custom[cur.sensor] = clone(cur.points);
      if (p === 'fixed') {
        if (cur.kind === 'fixed') return;
        const v = fanValue(id), now = v && v.duty != null ? v.duty : 50;
        m.sensor = cur.sensor;
        replaceCurve(c, cur, { kind: 'fixed', name: cur.name, duty: m.duty ?? now });
        return;
      }
      if (cur.kind !== 'graph') {
        // Back from Fixed: the sensor and the custom points it had before.
        if (cur.kind === 'fixed') m.duty = cur.duty;
        const sens = m.sensor || s;
        const points = p === 'custom' ? m.custom[sens] || presets(sens).custom : presets(sens)[p];
        replaceCurve(c, cur, graphOf(cur.name, sens, clone(points)));
        return;
      }
      if (p === 'silent' || p === 'performance') cur.points = clone(presets(cur.sensor)[p]);
      else if (p === 'custom' && preset(id) !== 'custom') cur.points = clone(m.custom[cur.sensor] || presets(cur.sensor).custom);
    }, { what: 'cooling' });
  }

  // Swaps a curve for another of a different kind, under the same name.
  function replaceCurve(c, old, next) {
    const i = c.curve.indexOf(old);
    for (const k of Object.keys(old)) delete old[k];
    Object.assign(old, next);
    if (i < 0) c.curve.push(old);
  }

  function setSource(id, sensorId) {
    const was = preset(id);
    edit((c) => {
      const f = ensureFan(c, id);
      const cur = (c.curve || []).find((x) => x.name === f.curve);
      if (!cur || cur.kind !== 'graph') return;
      const m = memoOf(cur.name);
      if (was === 'custom') m.custom[cur.sensor] = clone(cur.points);
      cur.sensor = sensorId;
      if (was === 'silent' || was === 'performance') cur.points = clone(presets(sensorId)[was]);
      else cur.points = clone(m.custom[sensorId] || presets(sensorId).custom);
    }, { what: 'cooling' });
  }

  /* A graph curve's sensor, from its own dialog. Points stay for a sensor in the same
     unit; for another unit they are kept, and the ones the curve had on that sensor
     before come back. */
  function setCurveSensor(name, sensorId) {
    edit((c) => {
      const x = (c.curve || []).find((k) => k.name === name);
      if (!x || x.kind !== 'graph' || x.sensor === sensorId) return;
      if (unitOf(sensorId) !== unitOf(x.sensor)) {
        const m = memoOf(name);
        m.custom[x.sensor] = clone(x.points);
        x.points = clone(m.custom[sensorId] || presets(sensorId).custom);
      }
      x.sensor = sensorId;
    }, { what: 'curve' });
  }

  // A copy of the fan's curve that only this fan follows.
  function fork(id) {
    edit((c) => {
      const f = ensureFan(c, id);
      const cur = (c.curve || []).find((x) => x.name === f.curve);
      if (!cur) return;
      const copy = clone(cur);
      copy.name = uniqueCurveName(fanName(id), c);
      c.curve.push(copy);
      f.curve = copy.name;
    }, { what: 'cooling' });
  }

  function useCurve(ids, name) {
    edit((c) => { for (const id of ids) { const f = ensureFan(c, id); if (name) f.curve = name; else delete f.curve; } tidyFans(c); }, { what: 'cooling' });
  }

  function renameCurve(old, name) {
    edit((c) => {
      for (const x of c.curve || []) {
        if (x.name === old) x.name = name;
        if (x.kind === 'mix') x.curves = x.curves.map((n) => (n === old ? name : n));
      }
      for (const f of c.fan || []) if (f.curve === old) f.curve = name;
      const book = memoBook();
      if (book[old]) { book[name] = book[old]; delete book[old]; }
    }, { what: 'cooling' });
  }

  function newCurve(kind) {
    let name = '';
    edit((c) => {
      const ids = sensors().map((s) => s.id);
      const s = ids.find(isLiquid) || 'cpu.temp';
      c.curve = c.curve || [];
      if (kind === 'fixed') { name = uniqueCurveName('Fixed', c); c.curve.push({ kind, name, duty: 50 }); }
      else if (kind === 'mix') { name = uniqueCurveName('Mix', c); c.curve.push({ kind, name, curves: c.curve.slice(0, 2).map((x) => x.name), function: 'max' }); }
      else if (kind === 'trigger') { name = uniqueCurveName('Trigger', c); c.curve.push({ kind, name, sensor: ids.includes('gpu.temp') ? 'gpu.temp' : s, idle_duty: 30, load_duty: 70, idle_at: 55, load_at: 65 }); }
      else { name = uniqueCurveName('Curve', c); c.curve.push(graphOf(name, s, presets(s).silent)); }
    }, { what: 'cooling' });
    return name;
  }

  function deleteCurve(name) {
    edit((c) => { c.curve = (c.curve || []).filter((x) => x.name !== name); }, { what: 'cooling' });
  }

  /* -------------------------------------------------------------- lighting */

  const EFFECTS = {
    off: { label: 'Off', colors: [0, 0] },
    fixed: { label: 'Fixed', colors: [1, 1] },
    fading: { label: 'Fading', colors: [1, 8], speed: true },
    'spectrum-wave': { label: 'Spectrum Wave', colors: [0, 0], speed: true, dir: true },
    'covering-marquee': { label: 'Covering Marquee', colors: [1, 8], speed: true, dir: true },
    pulse: { label: 'Pulse', colors: [1, 8], speed: true },
    breathing: { label: 'Breathing', colors: [1, 8], speed: true },
    candle: { label: 'Candle', colors: [1, 1] },
    'rainbow-flow': { label: 'Rainbow Flow', colors: [0, 0], speed: true, dir: true },
    'super-rainbow': { label: 'Super Rainbow', colors: [0, 0], speed: true, dir: true },
    'rainbow-pulse': { label: 'Rainbow Pulse', colors: [0, 0], speed: true },
    'tai-chi': { label: 'Tai Chi', colors: [1, 2], speed: true, dir: true },
    'water-cooler': { label: 'Water Cooler', colors: [2, 2], speed: true },
  };
  const SPEEDS = ['slowest', 'slower', 'normal', 'faster', 'fastest'];
  const SPEED_MULT = { slowest: 0.35, slower: 0.6, normal: 1, faster: 1.6, fastest: 2.4 };
  // Accepts CAM's PascalCase names too, as the daemon does.
  function effectKey(mode) {
    const k = String(mode || '').replace(/[^a-z0-9]/gi, '').toLowerCase();
    return Object.keys(EFFECTS).find((e) => e.replace(/-/g, '') === k) || 'off';
  }

  function lights() {
    const out = [];
    for (const d of devices()) for (const l of d.lights) out.push({ id: `${d.id}/${l.id}`, dev: d.id, device: d, hw: l, leds: l.leds || 24 });
    for (const l of (st.config && st.config.light) || []) {
      if (!out.some((c) => c.id === l.channel)) out.push({ id: l.channel, dev: devOf(l.channel), device: device(devOf(l.channel)), hw: null, leds: 24, missing: true });
    }
    return out;
  }
  const lightCfg = (id) => ((st.config && st.config.light) || []).find((l) => l.channel === id) || null;
  // What a channel shows: its entry, or off when camless leaves it alone.
  const lightOf = (id) => lightCfg(id) || { channel: id, mode: 'off', colors: [], unmanaged: true };
  function editLight(id, fn, o) {
    edit((c) => {
      c.light = c.light || [];
      let l = c.light.find((x) => x.channel === id);
      if (!l) { l = { channel: id, mode: 'off', colors: [] }; c.light.push(l); c.light.sort((a, b) => (a.channel < b.channel ? -1 : 1)); }
      l.mode = effectKey(l.mode) === l.mode ? l.mode : effectKey(l.mode);
      fn(l);
      if (!l.backward) delete l.backward;
      if (!l.speed || l.speed === 'normal') delete l.speed;
    }, Object.assign({ what: 'lighting' }, o));
  }

  function hexToRgb(h) {
    h = String(h || '000000').replace('#', '');
    const v = parseInt(h.length === 3 ? h.split('').map((c) => c + c).join('') : h, 16) || 0;
    return [(v >> 16) & 255, (v >> 8) & 255, v & 255];
  }
  const mix = (a, b, t) => [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)];
  const scale = (c, k) => [c[0] * k, c[1] * k, c[2] * k];
  function hsl(h, s = 1, l = 0.5) {
    h = ((h % 360) + 360) % 360;
    const k = (n) => (n + h / 30) % 12;
    const a = s * Math.min(l, 1 - l);
    const f = (n) => l - a * Math.max(-1, Math.min(k(n) - 3, Math.min(9 - k(n), 1)));
    return [f(0) * 255, f(8) * 255, f(4) * 255];
  }
  const css = (c, a = 1) => `rgba(${c[0] | 0},${c[1] | 0},${c[2] | 0},${a})`;

  // Roughly what each firmware effect looks like on n LEDs at time t (seconds). The
  // devices animate effects themselves; this only shows the colors and the motion.
  function renderLight(light, t, n = 24) {
    const out = new Array(n);
    const mode = light ? effectKey(light.mode) : 'off';
    const cols = ((light && light.colors) || []).map(hexToRgb);
    const c0 = cols[0] || [255, 255, 255];
    const c1 = cols[1] || [0, 0, 0];
    const s = t * (SPEED_MULT[(light && light.speed) || 'normal'] || 1);
    const dir = light && light.backward ? -1 : 1;
    const k = Math.max(1, cols.length);
    const pick = (i) => cols[((i % k) + k) % k] || c0;
    for (let i = 0; i < n; i++) {
      let c;
      const u = i / n;
      switch (mode) {
        case 'off': c = [0, 0, 0]; break;
        case 'fixed': c = c0; break;
        case 'fading': { const p = s / 3, idx = Math.floor(p); c = mix(pick(idx), pick(idx + 1), smoothstep(0.55, 1, p - idx)); break; }
        case 'spectrum-wave': c = hsl(s * 36 + u * 40 * dir); break;
        case 'covering-marquee': {
          const pos = Math.floor(s * 9), lap = Math.floor(pos / n), head = pos % n;
          const idx = dir > 0 ? i : n - 1 - i;
          c = idx <= head ? pick(lap) : pick(lap - 1);
          break;
        }
        case 'pulse': { const p = s / 1.3, idx = Math.floor(p); c = scale(pick(idx), Math.pow(Math.max(0, Math.sin((p - idx) * Math.PI)), 3)); break; }
        case 'breathing': { const p = s / 4.2, idx = Math.floor(p); c = scale(pick(idx), 0.04 + 0.96 * (0.5 - 0.5 * Math.cos((p - idx) * Math.PI * 2))); break; }
        case 'candle': c = scale(mix(c0, [255, 140, 40], 0.15), clamp(0.72 + 0.28 * wobble(s * 7 + i * 0.9, i), 0.3, 1)); break;
        case 'rainbow-flow': c = hsl(u * 360 + dir * s * 70); break;
        case 'super-rainbow': c = hsl(u * 720 + dir * s * 170); break;
        case 'rainbow-pulse': c = scale(hsl(u * 360 + s * 20), 0.08 + 0.92 * (0.5 - 0.5 * Math.cos(s * 2.2))); break;
        case 'tai-chi': c = mix(c0, c1, smoothstep(-0.35, 0.35, Math.sin(u * Math.PI * 2 + dir * s * 1.7))); break;
        case 'water-cooler': { const seg = n / 3, d = (((-i + s * 7) % seg) + seg) % seg; c = mix(scale(c0, 0.55), c1, Math.exp(-d * 0.9)); break; }
        default: c = c0;
      }
      out[i] = c;
    }
    return out;
  }

  function describeLight(light) {
    if (!light || light.unmanaged) return 'Not set by camless';
    const k = effectKey(light.mode);
    if (k === 'off') return 'Off';
    if (k === 'fixed') return `Fixed #${(light.colors || [])[0] || ''}`;
    return EFFECTS[k].label + (light.speed && light.speed !== 'normal' ? `, ${light.speed}` : '');
  }

  /* ------------------------------------------------------------------- LCD */

  function screens() {
    const out = devices().filter((d) => d.lcd).map((d) => ({ id: d.id, device: d }));
    for (const l of (st.config && st.config.lcd) || []) if (!out.some((s) => s.id === l.device)) out.push({ id: l.device, device: null });
    return out;
  }
  const lcdDefaults = (id) => ({ device: id, mode: 'liquid', brightness: 100, orientation: 0, fps: 10 });
  const lcdOf = (id) => ((st.config && st.config.lcd) || []).find((l) => l.device === id) || lcdDefaults(id);
  function editLcd(id, fn, o) {
    edit((c) => {
      c.lcd = c.lcd || [];
      let l = c.lcd.find((x) => x.device === id);
      if (!l) { l = lcdDefaults(id); c.lcd.push(l); }
      fn(l);
    }, Object.assign({ what: 'lcd' }, o));
  }

  /* ----------------------------------------------------------------- frame */

  // One animation clock for every picture. Each frame costs the whole window a paint
  // and the GPU process a present, at the display's rate (240 Hz is common), so it
  // ticks at a fixed, lower rate, only while a picture listens and the window shows,
  // and slower when nobody has touched the window for a while.
  const MOTION_FPS = { smooth: 60, balanced: 30, low: 12, off: 0 };
  const IDLE_AFTER = 20000;
  // A window in the background can still be scrolled or hovered; it slows down soon after.
  const IDLE_AFTER_UNFOCUSED = 2000;
  const IDLE_FPS = 12;
  // `due` is when the next frame should run: a step after the last one was due rather than
  // after it ran, since a frame runs up to a display refresh late and the rate would drift.
  let lastFrame = 0, due = 0, lastInput = performance.now(), focused = document.hasFocus(), timer = 0, queued = false;
  const reduced = matchMedia('(prefers-reduced-motion: reduce)');

  function motion() {
    const m = window.K && K.prefs.motion;
    return reduced.matches ? 'off' : (m in MOTION_FPS ? m : 'balanced');
  }
  function fps() {
    const f = MOTION_FPS[motion()];
    const idle = performance.now() - lastInput > (focused ? IDLE_AFTER : IDLE_AFTER_UNFOCUSED);
    return idle ? Math.min(f, IDLE_FPS) : f;
  }
  function wanted() { return !document.hidden && (listeners.frame || []).length > 0; }

  function schedule() {
    if (timer || queued || !wanted()) return;
    const f = fps();
    // Without motion the pictures still show the latest state, once per change. Motion
    // turned on while this frame waits finds it queued, so the clock starts from here.
    if (!f) { queued = true; requestAnimationFrame(() => { queued = false; lastFrame = due = 0; if (wanted()) emit('frame', 0); if (fps()) schedule(); }); return; }
    const wait = due ? Math.max(0, due - performance.now() - 4) : 0;
    timer = setTimeout(() => { timer = 0; queued = true; requestAnimationFrame(frame); }, wait);
  }
  function frame(now) {
    queued = false;
    if (!wanted()) { lastFrame = due = 0; return; }
    const dt = lastFrame ? Math.min(0.1, (now - lastFrame) / 1000) : 1 / 60;
    lastFrame = now;
    emit('frame', dt);
    const f = fps();
    if (!f) return;
    const step = 1000 / f;
    due = due && now - due < step ? due + step : now + step;
    schedule();
  }
  // Back to full rate at once when someone uses the window. Without motion the pointer
  // asks for no frame: a change does (below).
  function woke() {
    const was = fps();
    lastInput = performance.now();
    const f = fps();
    if (!f) return;
    if (f !== was && timer) { clearTimeout(timer); timer = 0; due = lastFrame && f ? lastFrame + 1000 / f : 0; }
    schedule();
  }
  for (const evt of ['pointermove', 'pointerdown', 'wheel', 'keydown']) addEventListener(evt, woke, { passive: true, capture: true });
  addEventListener('focus', () => { focused = true; woke(); });
  addEventListener('blur', () => { focused = false; lastInput = Math.min(lastInput, performance.now() - IDLE_AFTER_UNFOCUSED); });
  reduced.addEventListener('change', schedule);
  // Without motion the pictures are drawn once per change: new readings, an edit, a pref
  // or a pick in a dialog. kraken-draw.js asks for a frame when a picture shows or resizes.
  function changed() { if (!fps()) schedule(); }
  on('state', changed);
  on('config', changed);
  for (const evt of ['click', 'input', 'change']) addEventListener(evt, changed, { passive: true, capture: true });
  // The motion setting lives with the page's other prefs, in kit.js, loaded later.
  addEventListener('DOMContentLoaded', () => K.on('prefs', (k) => { if (k === 'motion') { lastFrame = due = 0; schedule(); } else changed(); }));
  document.addEventListener('visibilitychange', () => { lastFrame = due = 0; if (!document.hidden) { poll(); schedule(); } });

  window.S = {
    get st() { return st; },
    get config() { return st.config; },
    get status() { return st.status; },
    get server() { return st.server; },
    on, off, emit, schedule, api, action, edit, save, poll,
    util: { clamp, lerp, clone, same, smoothstep },
    curveMath: { dutyAt, dutyAtExact, normalize, dragPoint, addPoint, removePoint, safetyFloor },
    UNIT, sensors, sensor, value, unitOf, isLiquid, sourceLabel, range, presets,
    devices, device, devOf,
    curves, curve, fans, fan, fanCfg, fanName, fanValue, curveValue, users, defaultSensor, preset,
    ensureFan, tidyFans, setPreset, setSource, setCurveSensor, fork, useCurve, renameCurve, newCurve, deleteCurve, uniqueCurveName,
    EFFECTS, SPEEDS, effectKey, lights, lightCfg, lightOf, editLight, renderLight, describeLight,
    color: { hexToRgb, mix, scale, hsl, css },
    screens, lcdOf, editLcd,
    cfg,
  };

  poll();
})();
