/* Canvas drawings of the hardware: the Kraken pump head (LED ring + round LCD) and a
   fan with its LED ring. Shared so every concept shows the same lighting. */
(function () {
  'use strict';
  const C = window.Camless;
  const TAU = Math.PI * 2;

  // Sizes a canvas for its CSS box and device pixel ratio; returns the 2D context.
  function fit(canvas) {
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    const r = canvas.getBoundingClientRect();
    const w = Math.max(1, Math.round(r.width * dpr)), h = Math.max(1, Math.round(r.height * dpr));
    if (canvas.width !== w || canvas.height !== h) { canvas.width = w; canvas.height = h; }
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
        const g = ctx.createRadialGradient(x, y, 0, x, y, width * 3.2);
        g.addColorStop(0, C.color.css(c, 0.42 * glow));
        g.addColorStop(1, C.color.css(c, 0));
        ctx.fillStyle = g;
        ctx.beginPath();
        ctx.arc(x, y, width * 3.2, 0, TAU);
        ctx.fill();
      }
    }
    ctx.restore();
  }

  // The cooler's own liquid screen (FactoryBackupGraphic), approximated.
  function liquidScreen(ctx, cx, cy, r, liquid, font) {
    ctx.fillStyle = '#000';
    ctx.beginPath(); ctx.arc(cx, cy, r, 0, TAU); ctx.fill();
    ctx.strokeStyle = 'rgba(255,255,255,.14)';
    ctx.lineWidth = r * 0.035;
    ctx.beginPath(); ctx.arc(cx, cy, r * 0.84, Math.PI * 0.75, Math.PI * 2.25); ctx.stroke();
    const frac = C.util.clamp((liquid - 20) / 40, 0, 1);
    ctx.strokeStyle = liquid >= 50 ? '#ff5a4e' : '#ffffff';
    ctx.beginPath(); ctx.arc(cx, cy, r * 0.84, Math.PI * 0.75, Math.PI * 0.75 + frac * Math.PI * 1.5); ctx.stroke();
    ctx.fillStyle = '#fff';
    ctx.textAlign = 'center';
    ctx.textBaseline = 'alphabetic';
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
    ctx.fillText(s.liquid.toFixed(1) + ' °C liquid', cx, cy - r * 0.42);
  }

  /**
   * o: { leds, liquid, lcd: {mode, brightness, orientation}, snapshot, bezel, rim, font, glow, shadow }
   */
  function head(ctx, cx, cy, r, o) {
    const font = o.font || 'system-ui, sans-serif';
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
    // Ring.
    if (o.leds) ledRing(ctx, cx, cy, r * 0.9, o.leds, r * 0.07, o.glow ?? 1);
    // Screen.
    const sr = r * 0.78;
    const lcd = o.lcd || { mode: 'liquid', brightness: 100, orientation: 0 };
    ctx.save();
    ctx.beginPath(); ctx.arc(cx, cy, sr, 0, TAU); ctx.clip();
    ctx.translate(cx, cy);
    ctx.rotate(((o.rotation != null ? o.rotation : lcd.orientation) * Math.PI) / 180);
    ctx.translate(-cx, -cy);
    if (lcd.mode === 'api') apiScreen(ctx, cx, cy, sr, o.snapshot ? { cpu: o.snapshot.cpu.temp, gpu: o.snapshot.gpu.temp, liquid: o.snapshot.liquid } : { cpu: 50, gpu: 40, liquid: o.liquid }, font);
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

  window.KrakenDraw = { fit, head, fan, ledRing, roundRect, spin, liquidScreen, apiScreen };
})();
