/* camless UI concepts: simulated devices, sensors and config, shared by every mockup.
   Nothing here talks to hardware. Curve, safety and packet rules follow src/. */
(function () {
  'use strict';

  const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));
  const lerp = (a, b, t) => a + (b - a) * t;
  // Exponential approach with a time constant of tau seconds.
  const approach = (cur, target, dt, tau) => target + (cur - target) * Math.exp(-dt / tau);
  const smoothstep = (a, b, x) => { const t = clamp((x - a) / (b - a), 0, 1); return t * t * (3 - 2 * t); };
  // Cheap smooth noise in -1..1: a few incommensurate sines.
  const wobble = (t, seed = 0) =>
    (Math.sin(t * 0.71 + seed) * 0.5 + Math.sin(t * 1.93 + seed * 2.1) * 0.3 + Math.sin(t * 4.37 + seed * 3.7) * 0.2);

  /* ---------------------------------------------------------------- curves */

  const LIQUID_CRITICAL_C = 55;
  const KRAKEN_FIRST_C = 20;
  const KRAKEN_POINTS = 40;
  const PUMP_MIN_DUTY = 20;

  // Sorted, deduplicated by temperature, and non-decreasing (src/curve.rs).
  function normalize(points) {
    const pts = points.map(([t, d]) => [t, clamp(Math.round(d), 0, 100)]).sort((a, b) => a[0] - b[0]);
    const out = [];
    for (const p of pts) if (!out.length || out[out.length - 1][0] !== p[0]) out.push(p);
    let max = 0;
    for (const p of out) { max = Math.max(max, p[1]); p[1] = max; }
    return out;
  }

  // Linear between points, flat beyond the ends. Unrounded, for drawing.
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

  // 0 % at 45 °C liquid up to 100 % at 55 °C (src/daemon.rs safety_floor).
  const safetyFloor = (liquid) => clamp((Math.floor(liquid) - 45) * 10, 0, 100);

  // The 40 duties the Kraken stores for 20..59 °C liquid, floor applied.
  function krakenTable(points, minDuty = 0) {
    return Array.from({ length: KRAKEN_POINTS }, (_, i) => {
      const t = KRAKEN_FIRST_C + i;
      return clamp(Math.max(dutyAt(points, t), safetyFloor(t)), minDuty, 100);
    });
  }

  // Drag point i to (t, d), keeping temperatures ordered and pushing neighbours so
  // the curve stays non-decreasing. Returns a new array.
  function dragPoint(points, i, t, d, range = [20, 100], minDuty = 0) {
    const pts = points.map((p) => p.slice());
    const lo = i > 0 ? pts[i - 1][0] + 1 : range[0];
    const hi = i < pts.length - 1 ? pts[i + 1][0] - 1 : range[1];
    pts[i][0] = Math.round(clamp(t, lo, hi));
    pts[i][1] = Math.round(clamp(d, minDuty, 100));
    for (let j = i + 1; j < pts.length; j++) pts[j][1] = Math.max(pts[j][1], pts[i][1]);
    for (let j = i - 1; j >= 0; j--) pts[j][1] = Math.min(pts[j][1], pts[i][1]);
    return pts;
  }

  function addPoint(points, t, range = [20, 100]) {
    t = Math.round(clamp(t, range[0], range[1]));
    if (points.some((p) => p[0] === t)) return { points, index: points.findIndex((p) => p[0] === t) };
    const pts = normalize([...points.map((p) => p.slice()), [t, dutyAt(points, t)]]);
    return { points: pts, index: pts.findIndex((p) => p[0] === t) };
  }

  function removePoint(points, i) {
    if (points.length <= 2) return points;
    return points.filter((_, j) => j !== i);
  }

  /* ------------------------------------------------------------- lighting */

  const EFFECTS = {
    off: { label: 'Off', colors: [0, 0] },
    fixed: { label: 'Static', colors: [1, 1] },
    fading: { label: 'Fading', colors: [1, 8] },
    'spectrum-wave': { label: 'Spectrum wave', colors: [0, 0] },
    'covering-marquee': { label: 'Covering marquee', colors: [1, 8] },
    pulse: { label: 'Pulse', colors: [1, 8] },
    breathing: { label: 'Breathing', colors: [1, 8] },
    candle: { label: 'Candle', colors: [1, 1] },
    'rainbow-flow': { label: 'Rainbow flow', colors: [0, 0] },
    'super-rainbow': { label: 'Super rainbow', colors: [0, 0] },
    'rainbow-pulse': { label: 'Rainbow pulse', colors: [0, 0] },
    'tai-chi': { label: 'Tai chi', colors: [1, 2] },
    'water-cooler': { label: 'Water cooler', colors: [2, 2] },
  };
  const SPEEDS = ['slowest', 'slower', 'normal', 'faster', 'fastest'];
  const SPEED_MULT = { slowest: 0.35, slower: 0.6, normal: 1, faster: 1.6, fastest: 2.4 };

  function hexToRgb(h) {
    h = String(h || '000000').replace('#', '');
    const v = parseInt(h.length === 3 ? h.split('').map((c) => c + c).join('') : h, 16) || 0;
    return [(v >> 16) & 255, (v >> 8) & 255, v & 255];
  }
  const rgbToHex = ([r, g, b]) => [r, g, b].map((x) => clamp(Math.round(x), 0, 255).toString(16).padStart(2, '0')).join('');
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

  // Approximates what each firmware effect looks like on n LEDs at time t (seconds).
  function renderLight(light, t, n = 24) {
    const out = new Array(n);
    if (!light) { out.fill([0, 0, 0]); return out; }
    const mode = light.mode;
    const cols = (light.colors || []).map(hexToRgb);
    const c0 = cols[0] || [255, 255, 255];
    const c1 = cols[1] || [0, 0, 0];
    const s = t * (SPEED_MULT[light.speed] || 1);
    const dir = light.backward ? -1 : 1;
    const k = Math.max(1, cols.length);
    const pick = (i) => cols[((i % k) + k) % k] || c0;
    for (let i = 0; i < n; i++) {
      let c;
      const u = i / n;
      switch (mode) {
        case 'off': c = [0, 0, 0]; break;
        case 'fixed': c = c0; break;
        case 'fading': {
          const p = s / 3, idx = Math.floor(p);
          c = mix(pick(idx), pick(idx + 1), smoothstep(0.55, 1, p - idx));
          break;
        }
        case 'spectrum-wave': c = hsl(s * 36 + u * 40 * dir); break;
        case 'covering-marquee': {
          const pos = Math.floor(s * 9);
          const lap = Math.floor(pos / n), head = pos % n;
          const idx = dir > 0 ? i : n - 1 - i;
          c = idx <= head ? pick(lap) : pick(lap - 1);
          break;
        }
        case 'pulse': {
          const p = s / 1.3, idx = Math.floor(p), f = p - idx;
          c = scale(pick(idx), Math.pow(Math.max(0, Math.sin(f * Math.PI)), 3));
          break;
        }
        case 'breathing': {
          const p = s / 4.2, idx = Math.floor(p), f = p - idx;
          c = scale(pick(idx), 0.04 + 0.96 * (0.5 - 0.5 * Math.cos(f * Math.PI * 2)));
          break;
        }
        case 'candle': {
          const fl = 0.72 + 0.28 * wobble(s * 7 + i * 0.9, i);
          c = scale(mix(c0, [255, 140, 40], 0.15), clamp(fl, 0.3, 1));
          break;
        }
        case 'rainbow-flow': c = hsl(u * 360 + dir * s * 70); break;
        case 'super-rainbow': c = hsl(u * 720 + dir * s * 170); break;
        case 'rainbow-pulse': {
          const b = 0.5 - 0.5 * Math.cos(s * 2.2);
          c = scale(hsl(u * 360 + s * 20), 0.08 + 0.92 * b);
          break;
        }
        case 'tai-chi': {
          const a = u * Math.PI * 2 + dir * s * 1.7;
          c = mix(c0, c1, smoothstep(-0.35, 0.35, Math.sin(a)));
          break;
        }
        case 'water-cooler': {
          const seg = n / 3;
          const d = (((dir * -(i) + s * 7) % seg) + seg) % seg;
          c = mix(scale(c0, 0.55), c1, Math.exp(-d * 0.9));
          break;
        }
        default: c = c0;
      }
      out[i] = c;
    }
    return out;
  }

  function describeLight(light) {
    if (!light) return 'Not set';
    const e = EFFECTS[light.mode];
    if (!e) return light.mode;
    if (light.mode === 'off') return 'Off';
    if (light.mode === 'fixed') return `Static #${light.colors[0]}`;
    return e.label + (light.speed && light.speed !== 'normal' ? `, ${light.speed}` : '');
  }

  /* --------------------------------------------------------------- config */

  const DEFAULT_CONFIG = {
    interval_ms: 1000,
    failsafe_duty: 70,
    api: { listen: '127.0.0.1:30641' },
    pump: { source: 'cpu', curve: [[20, 61], [65, 61], [70, 71], [80, 82]] },
    radiator: { source: 'liquid', curve: [[30, 20], [40, 41], [50, 75], [55, 100]] },
    hub_fan: [
      { port: 1, name: 'Bottom front', source: 'gpu', curve: [[45, 25], [80, 80]] },
      { port: 2, name: 'Bottom rear', source: 'gpu', curve: [[45, 25], [80, 80]] },
      { port: 3, name: 'Rear exhaust', source: 'cpu', curve: [[40, 30], [70, 55], [85, 90]] },
      { port: 4, name: 'Side intake', source: 'fixed', duty: 40 },
    ],
    ring: { mode: 'tai-chi', colors: ['3232de', 'ffffff'], speed: 'normal' },
    external: { mode: 'fixed', colors: ['3232de'] },
    hub_led: [
      { port: 1, mode: 'breathing', colors: ['3232de'], speed: 'slower' },
      { port: 2, mode: 'breathing', colors: ['3232de'], speed: 'slower' },
      { port: 3, mode: 'fixed', colors: ['aa00ff'] },
    ],
    lcd: { mode: 'liquid', brightness: 100, orientation: 0 },
  };

  const clone = (o) => JSON.parse(JSON.stringify(o));

  const DEVICES = {
    kraken: { name: 'Kraken Elite V2', model: 'Kraken Elite 360 (2024)', id: '1e71:3012', firmware: '1.2.0', lcd: '640 × 640 round' },
    hub: { name: 'Control Hub', id: '1e71:2022', firmware: '1.0.7', ports: 5 },
    cpu: 'AMD Ryzen 7 9800X3D',
    gpu: 'NVIDIA GeForce RTX 4080 SUPER',
    ramGb: 32,
  };

  const SOURCES = {
    liquid: { label: 'Liquid', long: 'Liquid temperature', range: [20, 60] },
    cpu: { label: 'CPU', long: 'CPU temperature', range: [20, 100] },
    gpu: { label: 'GPU', long: 'GPU temperature', range: [20, 100] },
    fixed: { label: 'Fixed', long: 'Fixed speed', range: [20, 100] },
  };

  // Ready-made curves per source, so presets can retarget any channel.
  const PRESETS = {
    silent: {
      label: 'Silent',
      liquid: [[20, 20], [35, 25], [45, 50], [55, 100]],
      cpu: [[20, 20], [60, 25], [80, 55], [90, 85]],
      gpu: [[20, 20], [55, 22], [75, 50], [85, 80]],
      pump: [[20, 40], [60, 45], [80, 70], [90, 90]],
    },
    balanced: {
      label: 'Balanced',
      liquid: [[30, 20], [40, 41], [50, 75], [55, 100]],
      cpu: [[40, 30], [70, 55], [85, 90]],
      gpu: [[45, 25], [80, 80]],
      pump: [[20, 61], [65, 61], [70, 71], [80, 82]],
    },
    performance: {
      label: 'Performance',
      liquid: [[20, 45], [35, 60], [45, 85], [50, 100]],
      cpu: [[20, 45], [55, 65], [75, 90], [82, 100]],
      gpu: [[20, 40], [50, 60], [70, 90], [78, 100]],
      pump: [[20, 80], [60, 90], [75, 100]],
    },
  };

  /* -------------------------------------------------------------- channels */

  function coolingChannels(cfg) {
    const list = [];
    if (cfg.pump) list.push({ id: 'pump', label: 'Pump', device: 'kraken', kind: 'pump', cfg: cfg.pump, minDuty: PUMP_MIN_DUTY });
    if (cfg.radiator) list.push({ id: 'radiator', label: 'Radiator fans', device: 'kraken', kind: 'fan', cfg: cfg.radiator, minDuty: 0, fans: 3 });
    for (const f of cfg.hub_fan) {
      list.push({ id: 'hub' + f.port, label: f.name || `Hub fan ${f.port}`, device: 'hub', kind: 'fan', port: f.port, cfg: f, minDuty: 0, fans: 1 });
    }
    for (const c of list) c.runsOn = runsOn(c);
    return list;
  }

  // Liquid-sourced and fixed Kraken channels are stored on the cooler; everything
  // else needs camless running.
  function runsOn(ch) {
    return ch.device === 'kraken' && (ch.cfg.source === 'liquid' || ch.cfg.source === 'fixed') ? 'cooler' : 'camless';
  }

  function maxRpm(ch) {
    return ch.kind === 'pump' ? 2800 : ch.device === 'kraken' ? 1800 : 1500;
  }

  function rpmFor(ch, duty) {
    if (ch.kind === 'pump') return 600 + duty * 22;
    if (duty <= 0) return 0;
    return ch.device === 'kraken' ? 480 + duty * 13.2 : 380 + duty * 11.2;
  }

  /* ------------------------------------------------------------ simulation */

  const SCENARIOS = {
    idle: { label: 'Idle', cpu: 0.04, gpu: 0.03 },
    gaming: { label: 'Gaming', cpu: 0.38, gpu: 0.97 },
    render: { label: 'Render', cpu: 1.0, gpu: 0.12 },
  };
  const AUTO_SCHEDULE = [['idle', 22], ['gaming', 45], ['idle', 20], ['render', 35]];
  const PREFILL = [['idle', 140], ['gaming', 230], ['idle', 80], ['render', 100], ['idle', 50]];
  const HISTORY_SECONDS = 900;

  const state = {
    config: clone(DEFAULT_CONFIG),
    t: 0,
    scenarioMode: 'auto',
    scenario: 'idle',
    faults: { cpu: false, gpu: false, hot: false },
    load: { cpu: 0.04, gpu: 0.03 },
    cpu: { temp: 44, smooth: 44, power: 25 },
    gpu: { temp: 33, smooth: 33, power: 18 },
    liquid: 28,
    ram: { used: 11.2, total: 32 },
    ch: {},
    noise: 26,
    critical: false,
    history: [],
    events: [],
    logged: {},
  };

  let channelsCache = null;
  function channels() {
    if (!channelsCache) channelsCache = coolingChannels(state.config);
    return channelsCache;
  }

  function sourceTemp(src) {
    if (src === 'liquid') return state.faults.hot ? state.liquid : state.liquid;
    if (src === 'cpu') return state.faults.cpu ? null : state.cpu.smooth;
    if (src === 'gpu') return state.faults.gpu ? null : state.gpu.smooth;
    return null;
  }

  function targetDuty(ch) {
    const c = ch.cfg;
    let d;
    if (state.critical) d = 100;
    else if (c.source === 'fixed') d = c.duty ?? 0;
    else {
      const t = sourceTemp(c.source);
      d = t == null ? state.config.failsafe_duty : dutyAt(c.curve, t);
    }
    if (ch.device === 'kraken') d = Math.max(d, safetyFloor(state.liquid));
    return clamp(d, ch.minDuty, 100);
  }

  function why(ch) {
    const c = ch.cfg;
    if (state.critical) return `liquid at ${state.liquid.toFixed(1)} °C, so every fan runs at full speed`;
    if (c.source === 'fixed') return `set to a fixed ${c.duty} %`;
    const t = sourceTemp(c.source);
    if (t == null) return `${SOURCES[c.source].label} sensor is missing, so it holds the failsafe ${state.config.failsafe_duty} %`;
    const curveDuty = dutyAt(c.curve, t);
    const floor = ch.device === 'kraken' ? safetyFloor(state.liquid) : 0;
    if (floor > curveDuty) return `the safety floor asks for ${floor} % at ${state.liquid.toFixed(1)} °C liquid`;
    return `${SOURCES[c.source].label} is at ${t.toFixed(1)} °C, and the curve gives ${curveDuty} % there`;
  }

  let schedIdx = 0, schedLeft = AUTO_SCHEDULE[0][1];
  let prefillIdx = 0, prefillLeft = PREFILL[0][1];
  let prefilling = false;

  function setScenarioInternal(name) {
    if (state.scenario === name) return;
    const prev = state.scenario;
    state.scenario = name;
    const s = SCENARIOS[name];
    const text =
      name === 'gaming' ? 'Game started: GPU load up to 97 %' :
      name === 'render' ? 'Render started: all CPU cores busy' :
      prev === 'gaming' ? 'Game closed: GPU back to idle' :
      prev === 'render' ? 'Render finished: CPU back to idle' : `${s.label}`;
    pushEvent({ kind: 'load', text, scenario: name });
  }

  function pushEvent(e) {
    e.t = state.t;
    state.events.push(e);
    if (state.events.length > 400) state.events.shift();
    if (!prefilling) emit('event', e);
  }

  function step(dt) {
    state.t += dt;
    const cfg = state.config;

    // Workload.
    if (prefilling) {
      prefillLeft -= dt;
      if (prefillLeft <= 0) { prefillIdx = Math.min(prefillIdx + 1, PREFILL.length - 1); prefillLeft = PREFILL[prefillIdx][1]; setScenarioInternal(PREFILL[prefillIdx][0]); }
    } else if (state.scenarioMode === 'auto') {
      schedLeft -= dt;
      if (schedLeft <= 0) { schedIdx = (schedIdx + 1) % AUTO_SCHEDULE.length; schedLeft = AUTO_SCHEDULE[schedIdx][1]; setScenarioInternal(AUTO_SCHEDULE[schedIdx][0]); }
    }
    const sc = SCENARIOS[state.scenario];
    const t = state.t;
    const loadCpu = clamp(sc.cpu + (sc.cpu < 0.2 ? 0.03 : 0.07) * wobble(t * 0.9, 1) + (sc.cpu < 0.2 ? 0.02 * Math.max(0, wobble(t * 3.1, 9)) : 0), 0.01, 1);
    const loadGpu = clamp(sc.gpu + (sc.gpu > 0.5 ? 0.03 : 0.02) * wobble(t * 0.7, 4), 0.0, 1);
    state.load.cpu = approach(state.load.cpu, loadCpu, dt, 0.6);
    state.load.gpu = approach(state.load.gpu, loadGpu, dt, 1.2);

    // CPU: Ryzen 7 9800X3D on a 360 mm loop.
    const cpuPower = 22 + 98 * state.load.cpu;
    state.cpu.power = approach(state.cpu.power, cpuPower, dt, 0.5);
    const cpuTarget = state.liquid + 8 + state.cpu.power * 0.38 + 1.2 * wobble(t * 1.7, 2);
    state.cpu.temp = approach(state.cpu.temp, cpuTarget, dt, 2.2);

    // GPU: own air cooler, warms the case a little.
    const gpuPower = 16 + 304 * state.load.gpu;
    state.gpu.power = approach(state.gpu.power, gpuPower, dt, 1);
    const gpuTarget = 30 + state.gpu.power * 0.135 + 0.4 * wobble(t * 0.8, 5);
    state.gpu.temp = approach(state.gpu.temp, gpuTarget, dt, 7);

    // camless smooths host temperatures: rises in seconds, settles over half a minute.
    for (const k of ['cpu', 'gpu']) {
      const x = state[k];
      x.smooth = approach(x.smooth, x.temp, dt, x.temp > x.smooth ? 1.4 : 9);
    }

    // Liquid.
    const rad = state.ch.radiator ? state.ch.radiator.duty : 50;
    let liquidTarget = 27 + state.cpu.power * 0.055 + state.gpu.power * 0.006 - rad * 0.018;
    if (state.faults.hot) liquidTarget = 57.5;
    state.liquid = approach(state.liquid, liquidTarget, dt, state.faults.hot ? 5 : 22);
    if (!state.faults.hot && state.liquid > 50 && liquidTarget < 40) state.liquid = approach(state.liquid, liquidTarget, dt, 3);
    state.critical = state.liquid >= LIQUID_CRITICAL_C;

    state.ram.used = clamp(11.2 + (state.scenario === 'gaming' ? 7.4 : state.scenario === 'render' ? 12.5 : 0) + 0.25 * wobble(t * 0.2, 7), 4, 31);

    // Channels.
    let noiseSum = 0;
    for (const ch of channels()) {
      const target = targetDuty(ch);
      const st = state.ch[ch.id] || (state.ch[ch.id] = { duty: target, shown: target, rpm: rpmFor(ch, target), target });
      st.target = target;
      st.duty = target;
      st.shown = approach(st.shown, target, dt, 0.5);
      const rpmTarget = rpmFor(ch, st.shown) * (1 + 0.006 * wobble(t * 2.3, ch.id.length));
      st.rpm = approach(st.rpm, rpmTarget, dt, ch.kind === 'pump' ? 1.6 : 1.1);
      st.temp = ch.cfg.source === 'fixed' ? null : sourceTemp(ch.cfg.source);
      if (ch.kind === 'fan') noiseSum += Math.pow(st.rpm / maxRpm(ch), 2.2) * (ch.fans || 1);
      logDuty(ch, st);
    }
    state.noise = approach(state.noise, 24 + 9 * Math.log10(1 + 8 * noiseSum), dt, 1);
  }

  function logDuty(ch, st) {
    const last = state.logged[ch.id];
    if (!last) { state.logged[ch.id] = { duty: st.duty, t: state.t }; return; }
    if (Math.abs(st.duty - last.duty) >= 10 && state.t - last.t > 4) {
      const reason = ch.cfg.source === 'fixed' ? 'fixed' : (sourceTemp(ch.cfg.source) == null ? 'sensor missing' : `${SOURCES[ch.cfg.source].label} ${sourceTemp(ch.cfg.source).toFixed(0)} °C`);
      pushEvent({ kind: 'duty', channel: ch.id, label: ch.label, from: last.duty, to: st.duty, reason, text: `${ch.label} ${last.duty} → ${st.duty} %` });
      state.logged[ch.id] = { duty: st.duty, t: state.t };
    }
  }

  function sample() {
    const s = { t: state.t, liquid: state.liquid, cpu: state.faults.cpu ? null : state.cpu.temp, gpu: state.faults.gpu ? null : state.gpu.temp, cpuLoad: state.load.cpu, gpuLoad: state.load.gpu, cpuPower: state.cpu.power, gpuPower: state.gpu.power, noise: state.noise, scenario: state.scenario, duty: {}, rpm: {} };
    for (const ch of channels()) { const st = state.ch[ch.id]; if (st) { s.duty[ch.id] = st.duty; s.rpm[ch.id] = st.rpm; } }
    state.history.push(s);
    if (state.history.length > HISTORY_SECONDS) state.history.shift();
  }

  // Ten minutes of history, so charts have something in them on first load.
  function prefill() {
    prefilling = true;
    state.t = 0;
    const total = PREFILL.reduce((a, p) => a + p[1], 0);
    let acc = 0;
    for (let i = 0; i < total * 4; i++) {
      step(0.25);
      acc += 0.25;
      if (acc >= 1) { acc -= 1; sample(); }
    }
    prefilling = false;
    state.scenario = 'idle';
  }

  /* ------------------------------------------------------------------ bus */

  const listeners = {};
  function on(evt, fn) { (listeners[evt] = listeners[evt] || []).push(fn); return () => off(evt, fn); }
  function off(evt, fn) { listeners[evt] = (listeners[evt] || []).filter((f) => f !== fn); }
  // A listener dropped by an earlier one, such as a page built again, is not called.
  function emit(evt, arg) { for (const f of listeners[evt] || []) { if (!listeners[evt].includes(f)) continue; try { f(arg); } catch (e) { console.error(e); } } }

  let lastFrame = 0, tickAcc = 0, fastAcc = 0;
  function loop(now) {
    const dt = lastFrame ? Math.min(0.1, (now - lastFrame) / 1000) : 0.016;
    lastFrame = now;
    step(dt);
    emit('frame', dt);
    fastAcc += dt;
    if (fastAcc >= 0.25) { fastAcc = 0; emit('fast'); }
    tickAcc += dt;
    if (tickAcc >= 1) { tickAcc -= 1; sample(); emit('tick'); }
    requestAnimationFrame(loop);
  }

  /* ----------------------------------------------------------------- TOML */

  const q = (s) => JSON.stringify(String(s));
  const arr = (a) => '[' + a.map((v) => (Array.isArray(v) ? arr(v) : typeof v === 'string' ? q(v) : v)).join(', ') + ']';

  function coolingToml(c) {
    const l = [`source = ${q(c.source)}`];
    if (c.source === 'fixed') l.push(`duty = ${c.duty}`);
    else l.push(`curve = ${arr(c.curve)}`);
    return l;
  }
  function lightToml(x) {
    const l = [`mode = ${q(x.mode)}`];
    if (x.colors && x.colors.length) l.push(`colors = ${arr(x.colors)}`);
    if (x.speed) l.push(`speed = ${q(x.speed)}`);
    if (x.backward) l.push('backward = true');
    return l;
  }

  function toToml(cfg = state.config) {
    const out = [`interval_ms = ${cfg.interval_ms}`, `failsafe_duty = ${cfg.failsafe_duty}`];
    if (cfg.api) out.push('', '[api]', `listen = ${q(cfg.api.listen)}`);
    for (const k of ['pump', 'radiator']) if (cfg[k]) out.push('', `[${k}]`, ...coolingToml(cfg[k]));
    for (const f of cfg.hub_fan) {
      out.push('', '[[hub_fan]]', `port = ${f.port}`);
      if (f.name) out.push(`name = ${q(f.name)}`);
      out.push(...coolingToml(f));
    }
    for (const k of ['ring', 'external']) if (cfg[k]) out.push('', `[${k}]`, ...lightToml(cfg[k]));
    for (const h of cfg.hub_led) out.push('', '[[hub_led]]', `port = ${h.port}`, ...lightToml(h));
    if (cfg.lcd) out.push('', '[lcd]', `mode = ${q(cfg.lcd.mode)}`, `brightness = ${cfg.lcd.brightness}`, `orientation = ${cfg.lcd.orientation}`);
    return out.join('\n') + '\n';
  }

  const KEYS = {
    '': ['interval_ms', 'pawnio_module', 'failsafe_duty'],
    api: ['listen'],
    pump: ['source', 'curve', 'duty'],
    radiator: ['source', 'curve', 'duty'],
    hub_fan: ['port', 'name', 'source', 'curve', 'duty'],
    ring: ['mode', 'colors', 'speed', 'backward'],
    external: ['mode', 'colors', 'speed', 'backward'],
    hub_led: ['port', 'mode', 'colors', 'speed', 'backward'],
    lcd: ['mode', 'brightness', 'orientation'],
  };

  // Parses the subset of TOML camless.toml uses. Values on one line, JSON-compatible.
  // Returns { config, errors: [{ line, message }], lines: { path: line } }.
  function parseToml(text) {
    const cfg = { hub_fan: [], hub_led: [] };
    const errors = [];
    const lines = {};
    let table = '', target = cfg, skip = false;
    text.split('\n').forEach((raw, i) => {
      const n = i + 1;
      let s = '', inStr = false;
      for (const ch of raw) { if (ch === '"') inStr = !inStr; if (ch === '#' && !inStr) break; s += ch; }
      s = s.trim();
      if (!s) return;
      let m;
      if ((m = s.match(/^\[\[\s*([a-z_]+)\s*\]\]$/))) {
        table = m[1];
        skip = table !== 'hub_fan' && table !== 'hub_led';
        if (skip) { errors.push({ line: n, message: `unknown array table [[${table}]]` }); target = {}; return; }
        target = {}; cfg[table].push(target); lines[`${table}.${cfg[table].length - 1}`] = n; return;
      }
      if ((m = s.match(/^\[\s*([a-z_]+)\s*\]$/))) {
        table = m[1];
        skip = !KEYS[table] || table === 'hub_fan' || table === 'hub_led';
        if (skip) { errors.push({ line: n, message: `unknown table [${table}]` }); target = {}; return; }
        target = cfg[table] = {}; lines[table] = n; return;
      }
      if ((m = s.match(/^([A-Za-z0-9_]+)\s*=\s*(.+)$/))) {
        if (skip) return;
        const key = m[1];
        if (!KEYS[table].includes(key)) { errors.push({ line: n, message: `unknown field \`${key}\`${table ? ` in [${table}]` : ''}, expected one of ${KEYS[table].map((k) => '`' + k + '`').join(', ')}` }); return; }
        try { target[key] = JSON.parse(m[2].replace(/'/g, '"')); }
        catch (e) { errors.push({ line: n, message: `${key}: cannot read value ${m[2].trim()}` }); return; }
        lines[`${table}${table ? '.' : ''}${key}${table === 'hub_fan' || table === 'hub_led' ? '#' + (cfg[table].length - 1) : ''}`] = n;
        return;
      }
      errors.push({ line: n, message: `expected key = value, found ${s}` });
    });
    if (cfg.interval_ms == null) cfg.interval_ms = 1000;
    if (cfg.failsafe_duty == null) cfg.failsafe_duty = 70;
    if (cfg.lcd) { cfg.lcd.brightness ??= 100; cfg.lcd.orientation ??= 0; }
    if (cfg.api) cfg.api.listen ??= '127.0.0.1:30641';
    return { config: cfg, errors, lines };
  }

  // The checks in src/config.rs, with its wording.
  function validate(cfg) {
    const errs = [];
    const cooling = (name, c) => {
      if (!c.source || !SOURCES[c.source]) return errs.push({ at: name, message: `${name}: unknown source ${q(c.source)}` });
      if (c.source === 'fixed' && c.duty == null) errs.push({ at: name, message: `${name}: source "fixed" needs duty` });
      if (c.source !== 'fixed' && !(c.curve && c.curve.length)) errs.push({ at: name, message: `${name}: needs a curve` });
      if ((c.duty != null && (c.duty > 100 || c.duty < 0)) || (c.curve || []).some((p) => !Array.isArray(p) || p[1] > 100 || p[1] < 0 || !isFinite(p[0]))) errs.push({ at: name, message: `${name}: duties are 0 to 100 %` });
    };
    const light = (name, l) => {
      const mode = String(l.mode || '').toLowerCase();
      const e = EFFECTS[mode];
      if (!e) return errs.push({ at: name, message: `${name}: unknown lighting mode ${q(l.mode)}` });
      for (const c of l.colors || []) if (!/^#?[0-9a-fA-F]{6}$/.test(c)) errs.push({ at: name, message: `color ${q(c)} is not RRGGBB` });
      if (mode === 'fixed' && !(l.colors || []).length) errs.push({ at: name, message: `${name}: fixed needs one color` });
      else if ((l.colors || []).length < e.colors[0]) errs.push({ at: name, message: `${name}: ${mode} needs at least ${e.colors[0]} color(s)` });
      if (l.speed && !SPEEDS.includes(l.speed)) errs.push({ at: name, message: `${name}: unknown speed ${q(l.speed)}` });
    };
    (cfg.hub_fan || []).forEach((f, i) => {
      if (!(f.port >= 1 && f.port <= 5)) errs.push({ at: `hub_fan#${i}`, message: `hub_fan port ${f.port} is not 1 to 5` });
      if (cfg.hub_fan.slice(0, i).some((g) => g.port === f.port)) errs.push({ at: `hub_fan#${i}`, message: `hub_fan port ${f.port} is listed twice` });
      cooling(`hub_fan ${f.port}`, f);
    });
    (cfg.hub_led || []).forEach((l, i) => {
      if (!(l.port >= 1 && l.port <= 5)) errs.push({ at: `hub_led#${i}`, message: `hub_led port ${l.port} is not 1 to 5` });
      light(`hub_led ${l.port}`, l);
    });
    for (const k of ['pump', 'radiator']) if (cfg[k]) cooling(k, cfg[k]);
    if (!(cfg.failsafe_duty >= 0 && cfg.failsafe_duty <= 100)) errs.push({ at: 'failsafe_duty', message: 'failsafe_duty must be 0 to 100' });
    for (const k of ['ring', 'external']) if (cfg[k]) light(k, cfg[k]);
    if (cfg.lcd) {
      if (!['liquid', 'api'].includes(cfg.lcd.mode)) errs.push({ at: 'lcd', message: 'lcd mode must be "liquid" or "api"' });
      if (cfg.lcd.orientation % 90 !== 0 || cfg.lcd.orientation >= 360) errs.push({ at: 'lcd', message: 'lcd orientation must be 0, 90, 180 or 270' });
    }
    return errs;
  }

  /* -------------------------------------------------------------- packets */

  const SPEED_VALUE = [
    [[0x32, 0x00], [0x32, 0x00], [0x32, 0x00], [0x32, 0x00], [0x32, 0x00]],
    [[0x50, 0x00], [0x3c, 0x00], [0x28, 0x00], [0x14, 0x00], [0x0a, 0x00]],
    [[0x5e, 0x01], [0x2c, 0x01], [0xfa, 0x00], [0x96, 0x00], [0x50, 0x00]],
    [[0x40, 0x06], [0x14, 0x05], [0xe8, 0x03], [0x20, 0x03], [0x58, 0x02]],
    [[0x20, 0x03], [0xbc, 0x02], [0xf4, 0x01], [0x90, 0x01], [0x2c, 0x01]],
    [[0x19, 0x00], [0x14, 0x00], [0x0f, 0x00], [0x07, 0x00], [0x04, 0x00]],
    [[0x28, 0x00], [0x1e, 0x00], [0x14, 0x00], [0x0a, 0x00], [0x04, 0x00]],
    [[0x32, 0x00], [0x28, 0x00], [0x1e, 0x00], [0x14, 0x00], [0x0a, 0x00]],
  ];
  const EFFECT_PARAMS = {
    fading: [0x01, 1, 8], 'spectrum-wave': [0x02, 2, 0], 'covering-marquee': [0x04, 2, 8], pulse: [0x06, 5, 8],
    breathing: [0x07, 6, 8], candle: [0x08, 0, 1], 'rainbow-flow': [0x0b, 2, 0], 'super-rainbow': [0x0c, 2, 0],
    'rainbow-pulse': [0x0d, 2, 0], 'tai-chi': [0x0e, 7, 2], 'water-cooler': [0x0f, 6, 2],
  };
  const grb = (h) => { const [r, g, b] = hexToRgb(h); return [g, r, b]; };

  function lightPackets(channel, l) {
    if (!l || l.mode === 'off') return [[0x26, 0x04, channel, 0x00, ...new Array(72).fill(0)]];
    if (l.mode === 'fixed') {
      const c = grb(l.colors[0]);
      const colors = [0x26, 0x04, channel, 0x00];
      for (let i = 0; i < 24; i++) colors.push(...c);
      return [colors, [0x26, 0x06, channel, 0x00, 0x01, 0x00, 0x00, 24, 0x00, 0x00, 0x80, 0x00, 0x32, 0x00, 0x00, 0x01]];
    }
    const [mode, sc, max] = EFFECT_PARAMS[l.mode] || [0, 0, 0];
    const cols = max === 0 ? ['000000'] : (l.colors || []).slice(0, max);
    const msg = [0x2a, 0x04, channel, channel, mode, ...SPEED_VALUE[sc][Math.max(0, SPEEDS.indexOf(l.speed || 'normal'))]];
    for (let i = 0; i < 16; i++) msg.push(...(cols[i] ? grb(cols[i]) : [0, 0, 0]));
    let count = cols.length;
    const dirByte = (l.mode === 'covering-marquee' ? 0x04 : 0) + (l.backward ? 0x02 : 0);
    let rel = 0;
    if (['fading', 'pulse', 'breathing'].includes(l.mode)) rel = 0x08;
    else if (l.mode === 'tai-chi') rel = 0x05;
    else if (l.mode === 'water-cooler') { rel = 0x05; count = 1; }
    msg.push(dirByte, count, rel, 24, 0x03);
    return [msg];
  }

  // What `camless check` would print as sends, for the current readings.
  function packets(cfg = state.config) {
    const out = [];
    const chs = coolingChannels(cfg);
    for (const ch of chs.filter((c) => c.device === 'kraken')) {
      const hdr = ch.kind === 'pump' ? [0x72, 0x01, 0x01, 0x00] : [0x72, 0x02, 0x01, 0x01];
      let pts;
      if (ch.cfg.source === 'liquid') pts = ch.cfg.curve;
      else if (ch.cfg.source === 'fixed') pts = [[0, ch.cfg.duty]];
      else { const t = sourceTemp(ch.cfg.source); pts = [[0, t == null ? cfg.failsafe_duty : dutyAt(ch.cfg.curve, t)]]; }
      out.push({ device: 'kraken', label: `${ch.label}: ${ch.runsOn === 'cooler' ? 'stored curve' : 'flat table at the current duty'}`, bytes: [...hdr, ...krakenTable(pts, ch.minDuty)] });
    }
    const hub = [null, null, null, null, null];
    for (const ch of chs.filter((c) => c.device === 'hub')) hub[ch.port - 1] = state.ch[ch.id] ? state.ch[ch.id].duty : targetDuty(ch);
    if (hub.some((d) => d != null)) {
      let mask = 0; const msg = [0x62, 0x01, 0, 0, 0, 0, 0, 0];
      hub.forEach((d, i) => { if (d != null) { mask |= 1 << i; msg[3 + i] = Math.min(100, d); } });
      msg[2] = mask;
      out.push({ device: 'hub', label: 'Hub fan duties, sent when they change', bytes: msg });
    }
    if (cfg.ring) lightPackets(0b001, cfg.ring).forEach((b, i) => out.push({ device: 'kraken', label: `Ring lighting${i ? ' (apply)' : ''}`, bytes: b }));
    if (cfg.external) lightPackets(0b010, cfg.external).forEach((b, i) => out.push({ device: 'kraken', label: `RGB header${i ? ' (apply)' : ''}`, bytes: b }));
    if (cfg.lcd) {
      out.push({ device: 'kraken', label: 'LCD brightness and rotation', bytes: [0x30, 0x02, 0x01, Math.min(100, cfg.lcd.brightness), 0x00, 0x00, 0x01, (cfg.lcd.orientation / 90) & 3] });
      if (cfg.lcd.mode === 'liquid') out.push({ device: 'kraken', label: 'LCD liquid screen', bytes: [0x38, 0x01, 0x02, 0x00] });
    }
    return out;
  }
  const hex = (bytes) => bytes.map((b) => b.toString(16).padStart(2, '0')).join(' ');

  /* ----------------------------------------------------- public interface */

  function changed(what = 'config') {
    channelsCache = null;
    for (const k of Object.keys(state.ch)) if (!channels().some((c) => c.id === k)) delete state.ch[k];
    emit('config', what);
  }

  function setScenario(mode) {
    state.scenarioMode = mode;
    if (mode !== 'auto') setScenarioInternal(mode);
    emit('scenario', mode);
  }

  function setFault(name, on) {
    state.faults[name] = on;
    const text = {
      cpu: on ? 'CPU temperature sensor lost: CPU-driven channels hold the failsafe duty' : 'CPU temperature sensor is back',
      gpu: on ? 'GPU temperature sensor lost' : 'GPU temperature sensor is back',
      hot: on ? 'Liquid heating up (test)' : 'Liquid test ended',
    }[name];
    pushEvent({ kind: 'fault', text, fault: name, on });
    emit('fault', name);
  }

  // Retarget every channel to a preset, keeping each channel's source.
  function applyPreset(name) {
    const p = PRESETS[name];
    if (!p) return;
    const cfg = state.config;
    if (cfg.pump && cfg.pump.source !== 'fixed') cfg.pump.curve = clone(cfg.pump.source === 'liquid' ? p.liquid.map(([t, d]) => [t, Math.max(d, 40)]) : p.pump);
    if (cfg.radiator && cfg.radiator.source !== 'fixed') cfg.radiator.curve = clone(p[cfg.radiator.source]);
    for (const f of cfg.hub_fan) if (f.source !== 'fixed') f.curve = clone(p[f.source]);
    else f.duty = { silent: 30, balanced: 40, performance: 70 }[name];
    state.preset = name;
    pushEvent({ kind: 'preset', text: `Switched to ${p.label}` });
    changed('preset');
  }

  function snapshot() {
    return {
      t: state.t,
      scenario: state.scenario,
      scenarioMode: state.scenarioMode,
      liquid: state.liquid,
      cpu: { temp: state.faults.cpu ? null : state.cpu.temp, load: state.load.cpu, power: state.cpu.power },
      gpu: { temp: state.faults.gpu ? null : state.gpu.temp, load: state.load.gpu, power: state.gpu.power },
      ram: { ...state.ram },
      noise: state.noise,
      critical: state.critical,
      faults: { ...state.faults },
    };
  }

  const Camless = {
    get config() { return state.config; },
    set config(c) { state.config = c; changed('replace'); },
    get state() { return state; },
    get history() { return state.history; },
    get events() { return state.events; },
    defaults: () => clone(DEFAULT_CONFIG),
    devices: DEVICES,
    sources: SOURCES,
    scenarios: SCENARIOS,
    presets: PRESETS,
    channels,
    live: (id) => state.ch[id],
    why: (id) => { const ch = channels().find((c) => c.id === id); return ch ? why(ch) : ''; },
    snapshot,
    on, off, changed,
    setScenario, setFault, applyPreset,
    curve: { normalize, dutyAt, dutyAtExact, safetyFloor, krakenTable, dragPoint, addPoint, removePoint, KRAKEN_FIRST_C, KRAKEN_POINTS, PUMP_MIN_DUTY, LIQUID_CRITICAL_C },
    light: { EFFECTS, SPEEDS, render: renderLight, describe: describeLight },
    color: { hexToRgb, rgbToHex, mix, scale, hsl, css },
    util: { clamp, lerp, approach, smoothstep, wobble, clone, maxRpm: (ch) => maxRpm(ch) },
    toml: { stringify: toToml, parse: parseToml, validate },
    packets, hex,
    footprint: { camless: { ramMb: 30, cpuPct: 0.2, exeMb: 1.2 }, cam: { ramMb: 1200, lcdCpuPct: 45 } },
  };

  window.Camless = Camless;
  prefill();
  for (const ch of channels()) state.logged[ch.id] = { duty: state.ch[ch.id].duty, t: state.t };
  requestAnimationFrame(loop);
})();
