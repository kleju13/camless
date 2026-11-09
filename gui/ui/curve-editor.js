/* An SVG sensor-to-duty curve editor. It edits whatever `model()` describes and hands
   each change to `set()`; it keeps no curve of its own. Styled through the --ce-*
   custom properties of the page. */
(function () {
  'use strict';
  const { clamp, lerp } = window.S.util;
  const CV = window.S.curveMath;
  const NS = 'http://www.w3.org/2000/svg';
  const el = (tag, attrs = {}, parent) => {
    const e = document.createElementNS(NS, tag);
    for (const k in attrs) e.setAttribute(k, attrs[k]);
    if (parent) parent.appendChild(e);
    return e;
  };

  class CurveEditor {
    /**
     * opts.model(): { kind: 'graph' | 'fixed', points, duty, range: [lo, hi], minDuty, floor, unit }
     * opts.set(patch, final): patch is { points } or { duty }; final once a drag ends
     * opts.live(): { x, y, hold }, the input the curve acts on and its duty, either may be
     *   null; hold marks a speed held whatever the input, drawn as a line across the plot
     * opts.xLabel(x): axis and tooltip text for a sensor value
     * opts.liveLabel(x, y): the reading's text, in the plot's corner or in opts.liveEl
     * opts.warmFrom(): the sensor value from which the axis is marked as warm, or null
     * opts.heat(x): the color of a sensor value, for a curve over a temperature, or null
     * opts.onFrame(fn): subscribes to the animation clock, returns the unsubscribe
     */
    constructor(host, opts) {
      this.host = host;
      this.opts = Object.assign({ pad: { l: 40, r: 14, t: 20, b: 26 }, pointR: 5.5 }, opts);
      this.svg = el('svg', { class: 'ce', role: 'group' }, host);
      Object.assign(this.svg.style, { display: 'block', width: '100%', height: '100%', touchAction: 'none', overflow: 'visible' });
      const defs = el('defs', {}, this.svg);
      const id = 'ce' + Math.random().toString(36).slice(2, 8);
      this.ids = { hatch: id + 'h', heat: id + 'g', clip: id + 'c' };
      this.clipRect = el('rect', { x: 0, y: -40, width: 0, height: 4000 }, el('clipPath', { id: this.ids.clip }, defs));
      this.heatGrad = el('linearGradient', { id: this.ids.heat, gradientUnits: 'userSpaceOnUse' }, defs);
      const pat = el('pattern', { id: this.ids.hatch, width: 6, height: 6, patternUnits: 'userSpaceOnUse', patternTransform: 'rotate(45)' }, defs);
      el('rect', { width: 2.2, height: 6, fill: 'var(--ce-floor, rgba(255,120,60,.35))' }, pat);
      this.gGrid = el('g', { class: 'ce-grid' }, this.svg);
      this.floorPath = el('path', { class: 'ce-floor', fill: `url(#${this.ids.hatch})`, stroke: 'var(--ce-floor-line, rgba(255,120,60,.6))', 'stroke-width': 1, 'stroke-dasharray': '3 3' }, this.svg);
      this.track = el('path', { class: 'ce-track', fill: 'none', stroke: 'var(--ce-track, rgba(255,255,255,.3))', 'stroke-width': 1.5, 'stroke-linejoin': 'round', 'stroke-linecap': 'round' }, this.svg);
      this.area = el('path', { class: 'ce-area', fill: 'var(--ce-fill, #4c8dff)', 'fill-opacity': 'var(--ce-fill-top, .08)' }, this.svg);
      this.line = el('path', { class: 'ce-line', fill: 'none', stroke: 'var(--ce-line, #4c8dff)', 'stroke-width': 'var(--ce-line-w, 2.25)', 'stroke-linejoin': 'round', 'stroke-linecap': 'round' }, this.svg);
      this.liveV = el('line', { class: 'ce-live-v', stroke: 'var(--ce-live, #fff)', 'stroke-width': 1.5, 'stroke-opacity': .85 }, this.svg);
      this.liveDot = el('circle', { class: 'ce-live-dot', r: 3.5, fill: 'var(--ce-live, #fff)', stroke: 'var(--panel, #000)', 'stroke-width': 2 }, this.svg);
      this.liveH = el('line', { class: 'ce-live-h', stroke: 'var(--ce-hold, #f5a524)', 'stroke-width': 1.5, 'stroke-dasharray': '5 4' }, this.svg);
      this.liveH.style.display = 'none';
      this.gPts = el('g', { class: 'ce-points' }, this.svg);
      this.liveText = el('text', { class: 'ce-live-text', 'font-size': 11.5, fill: 'var(--ce-live-text, var(--ce-text, #ccc))', 'pointer-events': 'none' }, this.svg);
      this.tip = el('g', { class: 'ce-tip', opacity: 0 }, this.svg);
      this.tipBg = el('rect', { rx: 6, fill: 'var(--ce-tip-bg, rgba(0,0,0,.8))' }, this.tip);
      this.tipText = el('text', { 'font-size': 11.5, fill: 'var(--ce-tip-fg, #fff)', 'font-weight': 600 }, this.tip);
      this.drag = null;
      this.liveX = null;
      this.liveY = null;
      this.ro = new ResizeObserver(() => this.layout());
      this.ro.observe(host);
      this.svg.addEventListener('dblclick', (e) => this.onDbl(e));
      this.svg.addEventListener('pointermove', (e) => this.onMove(e));
      this.svg.addEventListener('pointerup', (e) => this.onUp(e));
      this.svg.addEventListener('pointercancel', (e) => this.onUp(e));
      this.unsub = this.opts.onFrame ? this.opts.onFrame((dt) => this.updateLive(false, dt)) : () => {};
      this.layout();
    }

    get m() { return this.opts.model(); }
    label(x) { return this.opts.xLabel ? this.opts.xLabel(x) : String(Math.round(x)); }

    layout() {
      const r = this.host.getBoundingClientRect();
      this.w = Math.max(120, r.width);
      this.h = Math.max(60, r.height);
      this.svg.setAttribute('viewBox', `0 0 ${this.w} ${this.h}`);
      this.render();
    }

    x(t) { const [a, b] = this.range; const p = this.opts.pad; return p.l + ((t - a) / (b - a)) * (this.w - p.l - p.r); }
    y(d) { const p = this.opts.pad; return p.t + (1 - d / 100) * (this.h - p.t - p.b); }
    t(px) { const [a, b] = this.range; const p = this.opts.pad; return a + ((px - p.l) / (this.w - p.l - p.r)) * (b - a); }
    d(py) { const p = this.opts.pad; return (1 - (py - p.t) / (this.h - p.t - p.b)) * 100; }

    local(e) { const r = this.svg.getBoundingClientRect(); return [((e.clientX - r.left) / r.width) * this.w, ((e.clientY - r.top) / r.height) * this.h]; }

    render() {
      const m = this.m;
      if (!m) return;
      // The range stays put while a point is dragged, so the axis does not slide.
      if (!this.drag || !this.range) this.range = m.range;
      const fixed = m.kind === 'fixed';
      const [a, b] = this.range;
      this.gGrid.textContent = '';
      const span = b - a;
      const step = (this.opts.xStep && this.opts.xStep(span)) || (span > 200 ? 50 : span > 50 ? 10 : 5);
      // The scale printed along the axes; a card keeps only the line across its middle.
      const quiet = this.opts.quiet, rule = 'var(--tick-major, rgba(255,255,255,.3))';
      const ruler = (x1, y1, x2, y2) => el('line', { x1, y1, x2, y2, stroke: rule, 'shape-rendering': 'crispEdges' }, this.gGrid);
      const fine = step >= 20 ? step / 4 : step / 2;
      for (let t = Math.ceil(a / fine) * fine; t <= b; t += fine) {
        const major = Math.abs(t / step - Math.round(t / step)) < 1e-6;
        const m = ruler(this.x(t), this.y(0), this.x(t), this.y(0) + (major ? 6 : 3));
        if (!major) m.setAttribute('opacity', .55);
      }
      for (let t = Math.ceil(a / step) * step; t <= b; t += step) {
        if (!quiet) el('line', { x1: this.x(t), x2: this.x(t), y1: this.y(0), y2: this.y(100), stroke: 'var(--ce-grid, rgba(255,255,255,.07))' }, this.gGrid);
        el('text', { x: this.x(t), y: this.h - 8, 'text-anchor': 'middle', 'font-size': 12, fill: 'var(--ce-text, #999)' }, this.gGrid).textContent = this.label(t);
      }
      const yStep = this.opts.yStep || 25;
      for (let d = 0; d <= 100; d += yStep / (yStep >= 50 ? 5 : 2.5)) {
        const major = d % yStep === 0;
        const m = ruler(this.x(a) - (major ? 6 : 3), this.y(d), this.x(a), this.y(d));
        if (!major) m.setAttribute('opacity', .55);
      }
      ruler(this.x(a), this.y(0), this.x(b), this.y(0));
      const warm = this.opts.warmFrom ? this.opts.warmFrom() : null;
      if (warm != null && warm < b) {
        this.warmLabel = el('text', { class: 'ce-redline-label', x: this.x(b), y: 12, 'text-anchor': 'end' }, this.gGrid);
        this.warmLabel.textContent = `Warm from ${this.label(warm)}`;
      }
      for (let d = 0; d <= 100; d += yStep) {
        if (d && (!quiet || d === 50)) el('line', { x1: this.x(a), x2: this.x(b), y1: this.y(d), y2: this.y(d), stroke: 'var(--ce-grid, rgba(255,255,255,.07))' }, this.gGrid);
        el('text', { x: this.opts.pad.l - 9, y: this.y(d) + 3.5, 'text-anchor': 'end', 'font-size': 12, fill: 'var(--ce-text, #999)' }, this.gGrid).textContent = d === 100 ? '100%' : String(d);
      }
      // The cooler's own floor, on curves in its liquid temperature.
      if (m.floor && !fixed) {
        let dd = `M${this.x(45)},${this.y(0)}`;
        for (let t = 45; t <= Math.min(b, 60); t++) dd += `L${this.x(t)},${this.y(CV.safetyFloor(t))}`;
        dd += `L${this.x(Math.min(b, 60))},${this.y(0)}Z`;
        this.floorPath.setAttribute('d', dd);
      } else this.floorPath.setAttribute('d', '');
      // A channel's own minimum, on the cards where the plot is all there is.
      if (quiet && m.minDuty && !fixed && !((this.opts.live && this.opts.live()) || {}).hold) el('path', { d: `M${this.x(a)},${this.y(0)}V${this.y(m.minDuty)}H${this.x(b)}V${this.y(0)}Z`, fill: `url(#${this.ids.hatch})` }, this.gGrid);

      const pts = fixed ? [[a, m.duty], [b, m.duty]] : m.points;
      let line = `M${this.x(a)},${this.y(CV.dutyAtExact(pts, a))}`;
      for (const [t, d] of pts) if (t > a && t < b) line += `L${this.x(t)},${this.y(d)}`;
      line += `L${this.x(b)},${this.y(CV.dutyAtExact(pts, b))}`;
      // Over a temperature, the curve takes the color of the temperature under it.
      const heat = !fixed && this.opts.heat && this.opts.heat(a) ? this.opts.heat : null;
      if (heat) {
        this.heatGrad.setAttribute('x1', this.x(a)); this.heatGrad.setAttribute('x2', this.x(b));
        this.heatGrad.setAttribute('y1', 0); this.heatGrad.setAttribute('y2', 0);
        this.heatGrad.textContent = '';
        for (let i = 0; i <= 16; i++) el('stop', { offset: i / 16, 'stop-color': heat(a + ((b - a) * i) / 16) }, this.heatGrad);
      }
      const paint = heat ? `url(#${this.ids.heat})` : null;
      this.line.setAttribute('stroke', paint || 'var(--ce-line, #4c8dff)');
      this.area.setAttribute('fill', (this.opts.quiet && paint) || 'var(--ce-fill, #4c8dff)');
      // On a card the color goes only as far as the reading, as in a thermometer.
      const meter = !!heat && !!this.opts.quiet;
      this.track.setAttribute('d', meter ? line : '');
      for (const p of [this.line, this.area]) { if (meter) p.setAttribute('clip-path', `url(#${this.ids.clip})`); else p.removeAttribute('clip-path'); }
      this.line.setAttribute('d', line);
      this.area.setAttribute('d', line + `L${this.x(b)},${this.y(0)}L${this.x(a)},${this.y(0)}Z`);

      // The focused point keeps keyboard focus when the curve is drawn again.
      const had = this.gPts.contains(document.activeElement) ? document.activeElement.getAttribute('data-i') ?? -1 : null;
      this.gPts.textContent = '';
      if (m.disabled) {
        // A curve it cannot edit is drawn, not offered.
      } else if (fixed) {
        const g = el('g', { class: 'ce-pt ce-pt-fixed', tabindex: 0, role: 'slider', 'aria-label': `Fixed duty ${m.duty} %`, 'aria-valuenow': m.duty, 'aria-valuemin': 0, 'aria-valuemax': 100 }, this.gPts);
        const cx = this.x((a + b) / 2), cy = this.y(m.duty);
        el('circle', { cx, cy, r: 14, fill: 'transparent' }, g);
        el('rect', { x: cx - 16, y: cy - 6, width: 32, height: 12, rx: 6, class: 'ce-dot', fill: 'var(--ce-point, #fff)', stroke: 'var(--ce-line, #4c8dff)', 'stroke-width': 2 }, g);
        g.style.cursor = 'ns-resize';
        g.addEventListener('pointerdown', (e) => this.start(e, -1));
        g.addEventListener('keydown', (e) => this.key(e, -1));
      } else {
        m.points.forEach(([t, d], i) => {
          if (t < a - 0.01 || t > b + 0.01) return;
          const g = el('g', { class: 'ce-pt', tabindex: 0, role: 'slider', 'aria-label': `Point ${i + 1}: ${this.label(t)}, ${d} %`, 'aria-valuenow': d, 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-keyshortcuts': 'Insert Delete', 'data-i': i }, this.gPts);
          el('circle', { cx: this.x(t), cy: this.y(d), r: 14, fill: 'transparent' }, g);
          el('rect', { x: this.x(t) - (this.drag && this.drag.i === i ? this.opts.pointR + 1 : this.opts.pointR - .5), y: this.y(d) - (this.drag && this.drag.i === i ? this.opts.pointR + 1 : this.opts.pointR - .5), width: 2 * (this.drag && this.drag.i === i ? this.opts.pointR + 1 : this.opts.pointR - .5), height: 2 * (this.drag && this.drag.i === i ? this.opts.pointR + 1 : this.opts.pointR - .5), rx: 99, class: 'ce-dot', fill: 'var(--ce-point, #fff)', stroke: 'var(--ce-line, #4c8dff)', 'stroke-width': 2 }, g);
          g.style.cursor = 'grab';
          g.addEventListener('pointerdown', (e) => this.start(e, i));
          g.addEventListener('keydown', (e) => this.key(e, i));
          g.addEventListener('contextmenu', (e) => { e.preventDefault(); this.remove(i); });
        });
      }
      if (had != null) this.focusPoint(+had);
      this.updateLive(true);
    }

    // `now` jumps to the reading; otherwise the dot eases towards it over time, the
    // same at any frame rate. Attributes are only written when they change: each
    // write invalidates style, and the next measurement forces a layout.
    updateLive(now, dt) {
      const m = this.m;
      if (!m || !this.range) return;
      const live = this.opts.live ? this.opts.live() : null;
      const [a, b] = this.range;
      const show = !!live && live.y != null;
      const hasX = show && live.x != null && m.kind !== 'fixed';
      const hold = show && !!live.hold;
      const vis = `${show}${hasX}${hold}`;
      if (now === true || vis !== this.liveVis) {
        this.liveVis = vis;
        this.liveText.style.display = show && !this.opts.liveEl ? '' : 'none';
        if (!show && this.opts.liveEl) this.opts.liveEl.textContent = '';
        this.liveH.style.display = hold ? '' : 'none';
        this.host.classList.toggle('ce-hold', hold);
        this.liveV.style.display = hasX ? '' : 'none';
        this.liveDot.style.display = hasX && !hold ? '' : 'none';
        this.liveAt = null;
        this.clipRect.setAttribute('width', 0);
      }
      if (!show) return;
      const tx = hasX ? clamp(live.x, a, b) : (a + b) / 2;
      // A frame without motion (dt 0) shows the reading as it is.
      const k = now === true || dt === 0 ? 1 : 1 - Math.pow(0.85, (dt || 1 / 60) * 60);
      this.liveX = this.liveX == null ? tx : lerp(this.liveX, tx, k);
      this.liveY = this.liveY == null ? live.y : lerp(this.liveY, live.y, k);
      const x = this.x(this.liveX), y = this.y(this.liveY);
      const text = this.opts.liveLabel ? this.opts.liveLabel(hasX ? live.x : null, live.y) : `${Math.round(live.y)}%`;
      const at = this.liveAt;
      if (at && Math.abs(at.x - x) < 0.05 && Math.abs(at.y - y) < 0.05 && at.text === text && at.w === this.w && at.h === this.h) return;
      this.liveAt = { x, y, text, w: this.w, h: this.h };
      this.liveV.setAttribute('x1', x); this.liveV.setAttribute('x2', x);
      // The needle: from the axis up to the curve, where the dot says the exact point.
      this.liveV.setAttribute('y1', this.y(0)); this.liveV.setAttribute('y2', y);
      this.liveDot.setAttribute('cx', x); this.liveDot.setAttribute('cy', y);
      if (hasX && !hold) this.clipRect.setAttribute('width', x);
      if (hasX && this.opts.heat) { const c = this.opts.heat(this.liveX); if (c) this.liveDot.setAttribute('fill', c); }
      if (hold) { this.liveH.setAttribute('x1', this.x(a)); this.liveH.setAttribute('x2', this.x(b)); this.liveH.setAttribute('y1', y); this.liveH.setAttribute('y2', y); }
      // One place for the reading, so it never sits on a point or on the curve.
      // Above the plot, over the needle when there is one, and always inside the chart.
      const half = text.length * 3.3;
      // Keeps clear of the "Warm from" label at the right end of the same line.
      const warmRoom = this.warmLabel && this.warmLabel.isConnected ? this.warmLabel.getComputedTextLength() + 16 : 0;
      this.liveText.setAttribute('x', hasX ? clamp(x - half, this.opts.pad.l, Math.max(this.opts.pad.l, this.w - this.opts.pad.r - half * 2 - warmRoom)) : this.opts.pad.l);
      this.liveText.setAttribute('y', 12);
      const target = this.opts.liveEl || this.liveText;
      if (target.textContent !== text) target.textContent = text;
    }

    start(e, i) {
      e.preventDefault();
      e.stopPropagation();
      this.svg.setPointerCapture(e.pointerId);
      this.drag = { i, id: e.pointerId };
      this.host.classList.add('ce-dragging');
      this.render();
      this.showTip(i);
    }

    onMove(e) {
      if (!this.drag) return;
      const [px, py] = this.local(e);
      const m = this.m;
      const fine = e.shiftKey ? 0.25 : 1;
      if (this.drag.i === -1) {
        const d = fine < 1 ? m.duty + (this.d(py) - m.duty) * fine : this.d(py);
        this.opts.set({ duty: Math.round(clamp(d, m.minDuty || 0, 100)) }, false);
      } else {
        const old = m.points[this.drag.i];
        const t = fine < 1 ? old[0] + (this.t(px) - old[0]) * fine : this.t(px);
        const d = fine < 1 ? old[1] + (this.d(py) - old[1]) * fine : this.d(py);
        this.opts.set({ points: CV.dragPoint(m.points, this.drag.i, t, d, this.range, m.minDuty || 0) }, false);
      }
      this.render();
      this.showTip(this.drag.i);
    }

    onUp() {
      if (!this.drag) return;
      this.drag = null;
      this.host.classList.remove('ce-dragging');
      this.tip.setAttribute('opacity', 0);
      const m = this.m;
      this.opts.set(m.kind === 'fixed' ? { duty: m.duty } : { points: m.points }, true);
      this.render();
    }

    onDbl(e) {
      const m = this.m;
      if (m.kind === 'fixed' || m.disabled) return;
      const [px] = this.local(e);
      this.opts.set({ points: CV.addPoint(m.points, this.t(px), this.range).points }, true);
      this.render();
    }

    remove(i) {
      this.opts.set({ points: CV.removePoint(this.m.points, i) }, true);
      this.render();
    }

    key(e, i) {
      const m = this.m;
      const k = e.shiftKey ? 5 : 1;
      let dt = 0, dd = 0;
      if (e.key === 'ArrowUp') dd = k; else if (e.key === 'ArrowDown') dd = -k;
      else if (e.key === 'ArrowRight') dt = k; else if (e.key === 'ArrowLeft') dt = -k;
      else if ((e.key === 'Delete' || e.key === 'Backspace') && i >= 0) {
        e.preventDefault();
        this.remove(i);
        this.focusPoint(Math.min(i, this.m.points.length - 1));
        return;
      } else if ((e.key === 'Insert' || e.key === '+') && i >= 0) {
        // A new point halfway to the next one, or to the previous one from the last.
        // A lone point gets one halfway to the far end of the range.
        e.preventDefault();
        const pts = m.points, [a, b] = this.range, t = pts[i][0];
        const j = i < pts.length - 1 ? i + 1 : i - 1;
        const to = j >= 0 ? pts[j][0] : t - a > b - t ? a : b;
        const r = CV.addPoint(pts, (t + to) / 2, this.range);
        this.opts.set({ points: r.points }, true);
        this.render();
        this.focusPoint(r.index);
        return;
      } else return;
      e.preventDefault();
      if (i === -1) this.opts.set({ duty: clamp(m.duty + dd, m.minDuty || 0, 100) }, true);
      else { const [t, d] = m.points[i]; this.opts.set({ points: CV.dragPoint(m.points, i, t + dt, d + dd, this.range, m.minDuty || 0) }, true); }
      this.render();
    }

    focusPoint(i) {
      const g = this.gPts.querySelector(i === -1 ? '.ce-pt' : `[data-i="${i}"]`);
      if (g) g.focus();
    }

    showTip(i) {
      const m = this.m;
      const [t, d] = i === -1 ? [null, m.duty] : m.points[i];
      const txt = i === -1 ? `${d} %` : `${this.label(t)}  ${d} %`;
      this.tipText.textContent = txt;
      const x = i === -1 ? this.x((this.range[0] + this.range[1]) / 2) : this.x(t);
      const y = this.y(d);
      const w = txt.length * 6.6 + 14;
      const tx = clamp(x - w / 2, 2, this.w - w - 2);
      const ty = y - 34 < 0 ? y + 14 : y - 34;
      this.tipBg.setAttribute('x', tx); this.tipBg.setAttribute('y', ty);
      this.tipBg.setAttribute('width', w); this.tipBg.setAttribute('height', 22);
      this.tipText.setAttribute('x', tx + 7); this.tipText.setAttribute('y', ty + 15);
      this.tip.setAttribute('opacity', 1);
    }

    destroy() { this.ro.disconnect(); this.unsub(); this.svg.remove(); }
  }

  window.CurveEditor = CurveEditor;
})();
