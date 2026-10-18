/* Canvas drawings of the hardware: the Kraken pump head (LED ring and round LCD) and
   a fan with its LED ring. The page draws what the config asks for; the devices
   animate their effects themselves. */
(function () {
  'use strict';
  const C = window.S;
  const TAU = Math.PI * 2;

  // A canvas's CSS box comes from a ResizeObserver, and whether it is on screen from an
  // IntersectionObserver: measuring it every frame would force a layout each time. A
  // picture that resizes or comes into view asks for a frame, as without motion no
  // other frame may come.
  const boxes = new WeakMap(), shown = new WeakMap();
  const ro = new ResizeObserver((entries) => {
    let resized = false;
    for (const e of entries) {
      const b = e.borderBoxSize && e.borderBoxSize[0];
      const box = b ? { width: b.inlineSize, height: b.blockSize } : { width: e.contentRect.width, height: e.contentRect.height };
      const was = boxes.get(e.target);
      if (!was || was.width !== box.width || was.height !== box.height) resized = true;
      boxes.set(e.target, box);
    }
    if (resized) C.schedule();
  });
  const io = new IntersectionObserver((entries) => {
    let appeared = false;
    for (const e of entries) {
      if (e.isIntersecting && shown.get(e.target) === false) appeared = true;
      shown.set(e.target, e.isIntersecting);
      // A picture taken off the page is not followed any more.
      if (!e.target.isConnected) { io.unobserve(e.target); ro.unobserve(e.target); boxes.delete(e.target); }
    }
    if (appeared) C.schedule();
  });
  function watch(canvas) {
    if (boxes.has(canvas)) return;
    const r = canvas.getBoundingClientRect();
    boxes.set(canvas, { width: r.width, height: r.height });
    ro.observe(canvas);
    io.observe(canvas);
  }
  // Whether drawing on `canvas` would show: on the page and not scrolled out of view.
  function visible(canvas) {
    if (!canvas.isConnected) return false;
    watch(canvas);
    return shown.get(canvas) !== false;
  }

  // Draws a picture only when it shows and would come out different from what is on
  // the canvas: `key` holds everything the picture depends on. A canvas left as it
  // is costs the window no paint.
  function paint(canvas, key, draw) {
    if (!visible(canvas)) return;
    const f = fit(canvas);
    const k = `${f.w}x${f.h}|${typeof key === 'string' ? key : JSON.stringify(key)}`;
    if (canvas._painted === k) return;
    canvas._painted = k;
    f.ctx.clearRect(0, 0, f.w, f.h);
    draw(f.ctx, f.w, f.h);
  }

  // Sizes a canvas for its CSS box and device pixel ratio; returns the 2D context.
  function fit(canvas) {
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    watch(canvas);
    const r = boxes.get(canvas);
    const w = Math.max(1, Math.round(r.width * dpr)), h = Math.max(1, Math.round(r.height * dpr));
    // Resizing clears the canvas, so what `paint` drew there is gone too.
    if (canvas.width !== w || canvas.height !== h) { canvas.width = w; canvas.height = h; canvas._painted = null; }
    const ctx = canvas.getContext('2d');
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    return { ctx, w: r.width, h: r.height };
  }

  function ledRing(ctx, cx, cy, r, leds, width, glow = 1) {
    const n = leds.length;
    const seg = TAU / n;
    ctx.save();
    ctx.lineCap = 'butt';
    for (let i = 0; i < n; i++) {
      const c = leds[i];
      const a0 = -Math.PI / 2 + i * seg + seg * 0.08, a1 = a0 + seg * 0.84;
      ctx.strokeStyle = C.color.css(c);
      ctx.lineWidth = width;
      ctx.beginPath();
      ctx.arc(cx, cy, r, a0, a1);
      ctx.stroke();
    }
    if (glow > 0) {
      ctx.globalCompositeOperation = 'lighter';
      for (let i = 0; i < n; i++) {
        const c = leds[i];
        const lum = (c[0] + c[1] + c[2]) / 765;
        if (lum < 0.02) continue;
        const a = -Math.PI / 2 + (i + 0.5) * seg;
        const x = cx + Math.cos(a) * r, y = cy + Math.sin(a) * r;
        ctx.translate(x, y);
        ctx.fillStyle = glowAt0(c, 0.42 * glow, 0, width * 3.2);
        ctx.beginPath();
        ctx.arc(0, 0, width * 3.2, 0, TAU);
        ctx.fill();
        ctx.translate(-x, -y);
      }
    }
    ctx.restore();
  }

  // Radial glows around the origin by color and size, made once: a gradient holds memory
  // outside the script heap that only a full garbage collection frees, and a ring made
  // one per LED per frame, ~2 MB a second. Colors are rounded to 4 bits a channel,
  // which a soft glow does not show.
  const glows = new Map();
  const GLOWS = 256;
  const glowCtx = document.createElement('canvas').getContext('2d');
  function glowAt0(c, alpha, r0, r1) {
    const q = [c[0] >> 4, c[1] >> 4, c[2] >> 4];
    const k = `${q}|${alpha}|${r0.toFixed(1)}|${r1.toFixed(1)}`;
    let g = glows.get(k);
    if (g) glows.delete(k);
    else {
      const col = [q[0] * 17, q[1] * 17, q[2] * 17];
      g = glowCtx.createRadialGradient(0, 0, r0, 0, 0, r1);
      g.addColorStop(0, C.color.css(col, alpha));
      g.addColorStop(1, C.color.css(col, 0));
      if (glows.size >= GLOWS) glows.delete(glows.keys().next().value);
    }
    glows.set(k, g);
    return g;
  }

  // The cooler's own liquid screen (FactoryBackupGraphic), approximated.
  function liquidScreen(ctx, cx, cy, r, liquid, font) {
    ctx.fillStyle = '#000';
    ctx.beginPath(); ctx.arc(cx, cy, r, 0, TAU); ctx.fill();
    ctx.strokeStyle = 'rgba(255,255,255,.14)';
    ctx.lineWidth = r * 0.035;
    ctx.beginPath(); ctx.arc(cx, cy, r * 0.84, Math.PI * 0.75, Math.PI * 2.25); ctx.stroke();
    ctx.fillStyle = '#fff';
    ctx.textAlign = 'center';
    ctx.textBaseline = 'alphabetic';
    if (liquid == null) {
      ctx.font = `600 ${r * 0.5}px ${font}`;
      ctx.fillText('--', cx, cy + r * 0.16);
      ctx.fillStyle = 'rgba(255,255,255,.62)';
      ctx.font = `500 ${r * 0.12}px ${font}`;
      ctx.fillText('Liquid', cx, cy + r * 0.5);
      return;
    }
    const frac = C.util.clamp((liquid - 20) / 40, 0, 1);
    ctx.strokeStyle = liquid >= 50 ? '#ff5a4e' : '#ffffff';
    ctx.beginPath(); ctx.arc(cx, cy, r * 0.84, Math.PI * 0.75, Math.PI * 0.75 + frac * Math.PI * 1.5); ctx.stroke();
    ctx.font = `600 ${r * 0.62}px ${font}`;
    const whole = Math.floor(liquid);
    const tw = ctx.measureText(String(whole)).width;
    ctx.font = `500 ${r * 0.2}px ${font}`;
    const sw = Math.max(ctx.measureText('°C').width, ctx.measureText('.0').width);
    const x0 = cx - (tw + sw + r * 0.03) / 2;
    ctx.textAlign = 'left';
    ctx.font = `600 ${r * 0.62}px ${font}`;
    ctx.fillText(String(whole), x0, cy + r * 0.2);
    ctx.font = `500 ${r * 0.2}px ${font}`;
    ctx.fillText('°C', x0 + tw + r * 0.03, cy - r * 0.14);
    ctx.fillText('.' + Math.floor((liquid - whole) * 10), x0 + tw + r * 0.03, cy + r * 0.2);
    ctx.textAlign = 'center';
    ctx.fillStyle = 'rgba(255,255,255,.62)';
    ctx.font = `500 ${r * 0.12}px ${font}`;
    ctx.fillText('Liquid', cx, cy + r * 0.5);
  }

  // What a page posting frames to /v1/lcd/frame might draw.
  function apiScreen(ctx, cx, cy, r, s, font) {
    const g = ctx.createRadialGradient(cx, cy - r * 0.3, r * 0.1, cx, cy, r);
    g.addColorStop(0, '#1d2340'); g.addColorStop(1, '#05060c');
    ctx.fillStyle = g;
    ctx.beginPath(); ctx.arc(cx, cy, r, 0, TAU); ctx.fill();
    const arc = (val, max, rr, col, a0, a1) => {
      ctx.lineCap = 'round';
      ctx.lineWidth = r * 0.07;
      ctx.strokeStyle = 'rgba(255,255,255,.1)';
      ctx.beginPath(); ctx.arc(cx, cy, rr, a0, a1, a1 < a0); ctx.stroke();
      ctx.strokeStyle = col;
      const f = C.util.clamp(val / max, 0, 1);
      ctx.beginPath(); ctx.arc(cx, cy, rr, a0, a0 + (a1 - a0) * f, a1 < a0); ctx.stroke();
    };
    arc(s.cpu || 0, 100, r * 0.82, '#7aa7ff', Math.PI * 0.62, Math.PI * 1.38);
    arc(s.gpu || 0, 100, r * 0.82, '#8af0c4', Math.PI * 0.38, -Math.PI * 0.38);
    ctx.fillStyle = '#fff';
    ctx.textAlign = 'center';
    ctx.font = `600 ${r * 0.34}px ${font}`;
    ctx.fillText(s.cpu == null ? '--' : Math.round(s.cpu) + '°', cx - r * 0.3, cy + r * 0.1);
    ctx.fillText(s.gpu == null ? '--' : Math.round(s.gpu) + '°', cx + r * 0.3, cy + r * 0.1);
    ctx.font = `500 ${r * 0.11}px ${font}`;
    ctx.fillStyle = 'rgba(255,255,255,.6)';
    ctx.fillText('CPU', cx - r * 0.3, cy + r * 0.3);
    ctx.fillText('GPU', cx + r * 0.3, cy + r * 0.3);
    if (s.liquid != null) ctx.fillText(s.liquid.toFixed(1) + ' °C liquid', cx, cy - r * 0.42);
  }

  // A web page on the screen: the page cannot be drawn here, so its address is.
  function webScreen(ctx, cx, cy, r, url, font) {
    const g = ctx.createRadialGradient(cx, cy - r * 0.4, r * 0.1, cx, cy, r);
    g.addColorStop(0, '#20262e'); g.addColorStop(1, '#07090b');
    ctx.fillStyle = g;
    ctx.beginPath(); ctx.arc(cx, cy, r, 0, TAU); ctx.fill();
    ctx.strokeStyle = 'rgba(255,255,255,.55)';
    ctx.lineWidth = Math.max(1, r * 0.025);
    const gr = r * 0.2, gy = cy - r * 0.18;
    ctx.beginPath(); ctx.arc(cx, gy, gr, 0, TAU); ctx.stroke();
    ctx.beginPath(); ctx.ellipse(cx, gy, gr * 0.45, gr, 0, 0, TAU); ctx.stroke();
    ctx.beginPath(); ctx.moveTo(cx - gr, gy); ctx.lineTo(cx + gr, gy); ctx.stroke();
    let host = '';
    try { host = url ? new URL(url).host : ''; } catch (e) { host = url; }
    ctx.fillStyle = 'rgba(255,255,255,.8)';
    ctx.textAlign = 'center';
    ctx.textBaseline = 'alphabetic';
    ctx.font = `500 ${r * 0.11}px ${font}`;
    const text = host || 'No address yet';
    let t = text;
    while (t.length > 4 && ctx.measureText(t).width > r * 1.4) t = t.slice(0, -2);
    ctx.fillText(t === text ? t : t + '…', cx, cy + r * 0.3);
    ctx.fillStyle = 'rgba(255,255,255,.5)';
    ctx.font = `500 ${r * 0.09}px ${font}`;
    ctx.fillText('Web page', cx, cy + r * 0.48);
  }

  // Parts of a picture that stay the same while its LEDs change, drawn once into a
  // canvas of their own and copied in each frame: blurring the head's shadow and
  // drawing its screen on every frame cost more than the rest of the picture.
  const layers = new Map();
  const LAYERS = 12;
  // Draws `draw(ctx, cx, cy)` around (cx, cy) on `ctx`, `ext` units out each way, from a
  // cached layer under `key`. The layer lines up with device pixels, so it copies sharp.
  function cached(ctx, key, cx, cy, ext, draw) {
    const m = ctx.getTransform();
    if (m.b || m.c || m.a !== m.d) { draw(ctx, cx, cy); return; }
    const s = m.a;
    const x0 = Math.floor((cx - ext) * s + m.e), y0 = Math.floor((cy - ext) * s + m.f);
    const lx = (cx * s + m.e - x0) / s, ly = (cy * s + m.f - y0) / s;
    const size = Math.ceil((ext * 2 + 2) * s);
    const k = `${key}|${s}|${lx.toFixed(2)}|${ly.toFixed(2)}|${size}`;
    let c = layers.get(k);
    if (c) layers.delete(k);
    else {
      // A canvas element, not an OffscreenCanvas, so text uses the page's own fonts.
      c = document.createElement('canvas');
      c.width = c.height = size;
      const lc = c.getContext('2d');
      lc.scale(s, s);
      draw(lc, lx, ly);
      if (layers.size >= LAYERS) layers.delete(layers.keys().next().value);
    }
    layers.set(k, c);
    ctx.save();
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.drawImage(c, x0, y0);
    ctx.restore();
  }

  function bezel(ctx, cx, cy, r, o) {
    ctx.save();
    if (o.shadow !== false) {
      ctx.shadowColor = o.shadowColor || 'rgba(0,0,0,.35)';
      ctx.shadowBlur = r * 0.25;
      ctx.shadowOffsetY = r * 0.08;
    }
    ctx.fillStyle = o.bezel || '#16171b';
    ctx.beginPath(); ctx.arc(cx, cy, r, 0, TAU); ctx.fill();
    ctx.restore();
    // Rim highlight.
    const rim = ctx.createLinearGradient(cx, cy - r, cx, cy + r);
    rim.addColorStop(0, o.rimTop || 'rgba(255,255,255,.22)');
    rim.addColorStop(1, 'rgba(255,255,255,0)');
    ctx.strokeStyle = rim;
    ctx.lineWidth = Math.max(1, r * 0.015);
    ctx.beginPath(); ctx.arc(cx, cy, r - ctx.lineWidth / 2, 0, TAU); ctx.stroke();
  }

  function screen(ctx, cx, cy, sr, o, lcd) {
    const font = o.font || 'system-ui, sans-serif';
    ctx.save();
    ctx.beginPath(); ctx.arc(cx, cy, sr, 0, TAU); ctx.clip();
    ctx.translate(cx, cy);
    ctx.rotate(((o.rotation != null ? o.rotation : lcd.orientation) * Math.PI) / 180);
    ctx.translate(-cx, -cy);
    if (lcd.mode === 'frames') apiScreen(ctx, cx, cy, sr, o.sensors || { liquid: o.liquid }, font);
    else if (lcd.mode === 'web') webScreen(ctx, cx, cy, sr, lcd.url, font);
    else liquidScreen(ctx, cx, cy, sr, o.liquid, font);
    ctx.restore();
    // Brightness and glass.
    ctx.save();
    ctx.beginPath(); ctx.arc(cx, cy, sr, 0, TAU); ctx.clip();
    ctx.fillStyle = `rgba(0,0,0,${(1 - (lcd.brightness ?? 100) / 100) * 0.85})`;
    ctx.fillRect(cx - sr, cy - sr, sr * 2, sr * 2);
    const glass = ctx.createLinearGradient(cx - sr, cy - sr, cx + sr * 0.3, cy + sr * 0.6);
    glass.addColorStop(0, 'rgba(255,255,255,.10)');
    glass.addColorStop(0.45, 'rgba(255,255,255,.02)');
    glass.addColorStop(0.46, 'rgba(255,255,255,0)');
    ctx.fillStyle = glass;
    ctx.fillRect(cx - sr, cy - sr, sr * 2, sr * 2);
    ctx.restore();
  }

  /**
   * o: { leds, liquid, lcd: { mode, brightness, orientation, url }, sensors: { cpu, gpu, liquid },
   *      rotation, bezel, rimTop, font, glow, shadow, shadowColor }
   */
  function head(ctx, cx, cy, r, o) {
    const shadow = o.shadow !== false;
    // The shadow reaches its blur plus its offset below the bezel.
    const ext = r * (shadow ? 1.5 : 1.02);
    cached(ctx, JSON.stringify(['bezel', r, o.bezel, o.rimTop, shadow && (o.shadowColor || '')]), cx, cy, ext,
      (c, x, y) => bezel(c, x, y, r, o));
    if (o.leds) ledRing(ctx, cx, cy, r * 0.9, o.leds, r * 0.07, o.glow ?? 1);
    // The screen covers the ring's glow inside it, so it goes on top.
    const sr = r * 0.78;
    const lcd = o.lcd || { mode: 'liquid', brightness: 100, orientation: 0 };
    const what = lcd.mode === 'frames' ? o.sensors || { liquid: o.liquid } : lcd.mode === 'web' ? lcd.url : o.liquid;
    cached(ctx, JSON.stringify(['screen', sr, lcd.mode, lcd.brightness, o.rotation != null ? o.rotation : lcd.orientation, what, o.font]), cx, cy, sr + 1,
      (c, x, y) => screen(c, x, y, sr, o, lcd));
  }

  /**
   * A fan seen from the front: frame, LED ring, blades rotated by `angle` radians.
   * o: { leds, frame, blade, hub, glow, blur }
   */
  function fan(ctx, cx, cy, r, angle, o = {}) {
    ctx.save();
    ctx.fillStyle = o.frame || '#1a1b1f';
    const s = r * 1.08;
    roundRect(ctx, cx - s, cy - s, s * 2, s * 2, r * 0.16);
    ctx.fill();
    ctx.fillStyle = o.well || '#0c0d10';
    ctx.beginPath(); ctx.arc(cx, cy, r * 0.96, 0, TAU); ctx.fill();
    if (o.leds) ledRing(ctx, cx, cy, r * 0.9, o.leds, r * 0.07, o.glow ?? 0.8);
    // Blades.
    ctx.translate(cx, cy);
    ctx.rotate(angle);
    const blades = 7;
    ctx.fillStyle = o.blade || 'rgba(225,228,236,.82)';
    const blur = C.util.clamp(o.blur || 0, 0, 1);
    for (let k = 0; k < blades; k++) {
      ctx.save();
      ctx.rotate((k * TAU) / blades);
      ctx.globalAlpha = 1 - blur * 0.55;
      ctx.beginPath();
      ctx.moveTo(r * 0.24, -r * 0.05);
      ctx.bezierCurveTo(r * 0.45, -r * 0.32, r * 0.78, -r * 0.3, r * 0.82, -r * 0.08);
      ctx.bezierCurveTo(r * 0.7, r * 0.06, r * 0.45, r * 0.12, r * 0.24, r * 0.1);
      ctx.closePath();
      ctx.fill();
      ctx.restore();
    }
    if (blur > 0.05) {
      ctx.globalAlpha = blur * 0.35;
      ctx.fillStyle = o.blade || 'rgba(225,228,236,.82)';
      ctx.beginPath(); ctx.arc(0, 0, r * 0.8, 0, TAU); ctx.arc(0, 0, r * 0.24, 0, TAU, true); ctx.fill();
      ctx.globalAlpha = 1;
    }
    ctx.fillStyle = o.hub || '#2a2c33';
    ctx.beginPath(); ctx.arc(0, 0, r * 0.25, 0, TAU); ctx.fill();
    ctx.restore();
  }

  function roundRect(ctx, x, y, w, h, r) {
    ctx.beginPath();
    ctx.moveTo(x + r, y);
    ctx.arcTo(x + w, y, x + w, y + h, r);
    ctx.arcTo(x + w, y + h, x, y + h, r);
    ctx.arcTo(x, y + h, x, y, r);
    ctx.arcTo(x, y, x + w, y, r);
    ctx.closePath();
  }

  // Visible rotation speed for a given rpm: real speeds strobe on screen, so this is
  // a calm, monotonic mapping.
  const spin = (rpm) => (rpm / 60) * 0.55;

  window.KrakenDraw = { fit, paint, visible, cached, glowAt0, head, fan, ledRing, roundRect, spin, liquidScreen, apiScreen, webScreen };
})();
