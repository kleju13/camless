/* An SVG temperature-to-duty curve editor shared by several concepts. Styled through
   CSS custom properties (--ce-*) on the container, so each concept skins it. */
(function () {
  'use strict';
  const C = window.Camless;
  const NS = 'http://www.w3.org/2000/svg';
  const el = (tag, attrs = {}, parent) => {
    const e = document.createElementNS(NS, tag);
    for (const k in attrs) e.setAttribute(k, attrs[k]);
    if (parent) parent.appendChild(e);
    return e;
  };

  class CurveEditor {
    /**
     * opts.channel(): the channel from Camless.channels() to edit
     * opts.pad: {l,r,t,b} in px; opts.smooth: draw a soft live marker; opts.compact
     * opts.liveLabel: false, or (temp|null, duty, channel) => text next to the live dot
     * opts.tempLabel: (°C) => axis and tooltip text, for showing another unit
     */
    constructor(host, opts) {
      this.host = host;
      this.opts = Object.assign({ pad: { l: 40, r: 14, t: 14, b: 28 }, floor: true, table: false, labels: true }, opts);
      this.svg = el('svg', { class: 'ce', role: 'group' }, host);
      this.svg.style.display = 'block';
      this.svg.style.width = '100%';
      this.svg.style.height = '100%';
      this.svg.style.touchAction = 'none';
      this.svg.style.overflow = 'visible';
      const defs = el('defs', {}, this.svg);
      const id = 'ce' + Math.random().toString(36).slice(2, 8);
      this.ids = { hatch: id + 'h', grad: id + 'g' };
      const pat = el('pattern', { id: this.ids.hatch, width: 6, height: 6, patternUnits: 'userSpaceOnUse', patternTransform: 'rotate(45)' }, defs);
      el('rect', { width: 2.2, height: 6, fill: 'var(--ce-floor, rgba(255,120,60,.35))' }, pat);
      const grad = el('linearGradient', { id: this.ids.grad, x1: 0, y1: 0, x2: 0, y2: 1 }, defs);
      el('stop', { offset: '0%', 'stop-color': 'var(--ce-fill, #4c8dff)', 'stop-opacity': 'var(--ce-fill-top, .32)' }, grad);
      el('stop', { offset: '100%', 'stop-color': 'var(--ce-fill, #4c8dff)', 'stop-opacity': '0' }, grad);
      this.gGrid = el('g', { class: 'ce-grid' }, this.svg);
      this.floorPath = el('path', { class: 'ce-floor', fill: `url(#${this.ids.hatch})`, stroke: 'var(--ce-floor-line, rgba(255,120,60,.6))', 'stroke-width': 1, 'stroke-dasharray': '3 3' }, this.svg);
      this.tablePath = el('path', { class: 'ce-table', fill: 'none', stroke: 'var(--ce-table, rgba(255,255,255,.35))', 'stroke-width': 1 }, this.svg);
      this.area = el('path', { class: 'ce-area', fill: `url(#${this.ids.grad})` }, this.svg);
      this.line = el('path', { class: 'ce-line', fill: 'none', stroke: 'var(--ce-line, #4c8dff)', 'stroke-width': 'var(--ce-line-w, 2.25)', 'stroke-linejoin': 'round', 'stroke-linecap': 'round' }, this.svg);
      this.liveV = el('line', { class: 'ce-live-v', stroke: 'var(--ce-live, #fff)', 'stroke-width': 1, 'stroke-dasharray': '2 3', opacity: 0.6 }, this.svg);
      this.liveDot = el('circle', { class: 'ce-live-dot', r: 5, fill: 'var(--ce-live, #fff)' }, this.svg);
      this.liveHalo = el('circle', { class: 'ce-live-halo', r: 11, fill: 'var(--ce-live, #fff)', opacity: 0.18 }, this.svg);
      this.liveText = el('text', { class: 'ce-live-text', 'font-size': 11, fill: 'var(--ce-live-text, var(--ce-text, #ccc))' }, this.svg);
      this.gPts = el('g', { class: 'ce-points' }, this.svg);
      this.tip = el('g', { class: 'ce-tip', opacity: 0 }, this.svg);
      this.tipBg = el('rect', { rx: 6, fill: 'var(--ce-tip-bg, rgba(0,0,0,.8))' }, this.tip);
      this.tipText = el('text', { 'font-size': 11.5, fill: 'var(--ce-tip-fg, #fff)', 'font-weight': 600 }, this.tip);
      this.drag = null;
      this.liveX = null;
      this.ro = new ResizeObserver(() => this.layout());
      this.ro.observe(host);
      this.svg.addEventListener('dblclick', (e) => this.onDbl(e));
      this.svg.addEventListener('pointermove', (e) => this.onMove(e));
      this.svg.addEventListener('pointerup', (e) => this.onUp(e));
      this.svg.addEventListener('pointercancel', (e) => this.onUp(e));
      this.unsub = C.on('frame', () => this.updateLive());
      this.layout();
    }

    get ch() { return this.opts.channel(); }
    get range() { const s = this.ch.cfg.source; return this.opts.range || (C.sources[s] || C.sources.cpu).range; }
    get minDuty() { return this.ch.minDuty || 0; }

    layout() {
      const r = this.host.getBoundingClientRect();
      this.w = Math.max(120, r.width);
      this.h = Math.max(90, r.height);
      this.svg.setAttribute('viewBox', `0 0 ${this.w} ${this.h}`);
      this.render();
    }

    x(t) { const [a, b] = this.range; const p = this.opts.pad; return p.l + ((t - a) / (b - a)) * (this.w - p.l - p.r); }
    y(d) { const p = this.opts.pad; return p.t + (1 - d / 100) * (this.h - p.t - p.b); }
    t(px) { const [a, b] = this.range; const p = this.opts.pad; return a + ((px - p.l) / (this.w - p.l - p.r)) * (b - a); }
    d(py) { const p = this.opts.pad; return (1 - (py - p.t) / (this.h - p.t - p.b)) * 100; }

    local(e) { const r = this.svg.getBoundingClientRect(); return [((e.clientX - r.left) / r.width) * this.w, ((e.clientY - r.top) / r.height) * this.h]; }

    render() {
      const ch = this.ch;
      const fixed = ch.cfg.source === 'fixed';
      const [a, b] = this.range;
      // Grid.
      this.gGrid.textContent = '';
      const step = b - a > 50 ? 10 : 5;
      for (let t = Math.ceil(a / step) * step; t <= b; t += step) {
        el('line', { x1: this.x(t), x2: this.x(t), y1: this.y(0), y2: this.y(100), stroke: 'var(--ce-grid, rgba(255,255,255,.07))' }, this.gGrid);
        if (this.opts.labels) el('text', { x: this.x(t), y: this.h - 8, 'text-anchor': 'middle', 'font-size': 10.5, fill: 'var(--ce-text, #999)' }, this.gGrid).textContent = this.opts.tempLabel ? this.opts.tempLabel(t) : t + '°';
      }
      for (let d = 0; d <= 100; d += 25) {
        el('line', { x1: this.x(a), x2: this.x(b), y1: this.y(d), y2: this.y(d), stroke: 'var(--ce-grid, rgba(255,255,255,.07))' }, this.gGrid);
        if (this.opts.labels) el('text', { x: this.opts.pad.l - 8, y: this.y(d) + 3.5, 'text-anchor': 'end', 'font-size': 10.5, fill: 'var(--ce-text, #999)' }, this.gGrid).textContent = d + '%';
      }
      // Safety floor applies to every table stored on the Kraken, in liquid °C.
      const floorOn = this.opts.floor && ch.device === 'kraken' && ch.cfg.source === 'liquid';
      if (floorOn) {
        let dd = `M${this.x(45)},${this.y(0)}`;
        for (let t = 45; t <= Math.min(b, 60); t++) dd += `L${this.x(t)},${this.y(C.curve.safetyFloor(t))}`;
        dd += `L${this.x(Math.min(b, 60))},${this.y(0)}Z`;
        this.floorPath.setAttribute('d', dd);
      } else this.floorPath.setAttribute('d', '');
      // The 40-step table the cooler stores.
      if (this.opts.table && ch.device === 'kraken' && ch.cfg.source === 'liquid') {
        const tab = C.curve.krakenTable(ch.cfg.curve, this.minDuty);
        let dd = '';
        tab.forEach((v, i) => { const t0 = 20 + i, t1 = 21 + i; dd += `${i ? 'L' : 'M'}${this.x(t0)},${this.y(v)}L${this.x(Math.min(t1, b))},${this.y(v)}`; });
        this.tablePath.setAttribute('d', dd);
      } else this.tablePath.setAttribute('d', '');

      const pts = fixed ? [[a, ch.cfg.duty], [b, ch.cfg.duty]] : ch.cfg.curve;
      let line = `M${this.x(a)},${this.y(C.curve.dutyAtExact(pts, a))}`;
      for (const [t, d] of pts) if (t > a && t < b) line += `L${this.x(t)},${this.y(d)}`;
      line += `L${this.x(b)},${this.y(C.curve.dutyAtExact(pts, b))}`;
      this.line.setAttribute('d', line);
      this.area.setAttribute('d', line + `L${this.x(b)},${this.y(0)}L${this.x(a)},${this.y(0)}Z`);

      // Points. The focused one keeps keyboard focus when the curve is drawn again.
      const had = this.gPts.contains(document.activeElement) ? document.activeElement.getAttribute('data-i') ?? -1 : null;
      this.gPts.textContent = '';
      if (fixed) {
        const g = el('g', { class: 'ce-pt ce-pt-fixed', tabindex: 0, role: 'slider', 'aria-label': `Fixed duty ${ch.cfg.duty} %`, 'aria-valuenow': ch.cfg.duty, 'aria-valuemin': 0, 'aria-valuemax': 100 }, this.gPts);
        const cx = this.x((a + b) / 2), cy = this.y(ch.cfg.duty);
        el('circle', { cx, cy, r: 14, fill: 'transparent' }, g);
        el('rect', { x: cx - 16, y: cy - 6, width: 32, height: 12, rx: 6, class: 'ce-dot', fill: 'var(--ce-point, #fff)', stroke: 'var(--ce-line, #4c8dff)', 'stroke-width': 2 }, g);
        g.style.cursor = 'ns-resize';
        g.addEventListener('pointerdown', (e) => this.start(e, -1));
        g.addEventListener('keydown', (e) => this.key(e, -1));
      } else {
        ch.cfg.curve.forEach(([t, d], i) => {
          if (t < a - 0.01 || t > b + 0.01) return;
          const g = el('g', { class: 'ce-pt', tabindex: 0, role: 'slider', 'aria-label': `Point ${i + 1}: ${t} °C, ${d} %`, 'aria-valuenow': d, 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-keyshortcuts': 'Insert Delete', 'data-i': i }, this.gPts);
          el('circle', { cx: this.x(t), cy: this.y(d), r: 14, fill: 'transparent' }, g);
          el('circle', { cx: this.x(t), cy: this.y(d), r: this.drag && this.drag.i === i ? 7 : 5.5, class: 'ce-dot', fill: 'var(--ce-point, #fff)', stroke: 'var(--ce-line, #4c8dff)', 'stroke-width': 2.25 }, g);
          g.style.cursor = 'grab';
          g.addEventListener('pointerdown', (e) => this.start(e, i));
          g.addEventListener('keydown', (e) => this.key(e, i));
          g.addEventListener('contextmenu', (e) => { e.preventDefault(); this.remove(i); });
        });
      }
      if (had != null) this.focusPoint(+had);
      this.updateLive(true);
    }

    updateLive() {
      const ch = this.ch;
      const st = C.live(ch.id);
      if (!st) return;
      const [a, b] = this.range;
      const temp = st.temp;
      const show = temp != null && ch.cfg.source !== 'fixed';
      const tx = show ? C.util.clamp(temp, a, b) : (a + b) / 2;
      this.liveX = this.liveX == null ? tx : C.util.lerp(this.liveX, tx, 0.15);
      const x = this.x(this.liveX), y = this.y(st.shown);
      this.liveDot.setAttribute('cx', x); this.liveDot.setAttribute('cy', y);
      this.liveHalo.setAttribute('cx', x); this.liveHalo.setAttribute('cy', y);
      this.liveV.setAttribute('x1', x); this.liveV.setAttribute('x2', x);
      this.liveV.setAttribute('y1', this.y(0)); this.liveV.setAttribute('y2', y);
      this.liveV.style.display = show ? '' : 'none';
      this.liveText.setAttribute('x', x + 9);
      this.liveText.setAttribute('y', y - 9);
      this.liveText.setAttribute('text-anchor', x > this.w - 90 ? 'end' : 'start');
      if (x > this.w - 90) this.liveText.setAttribute('x', x - 9);
      const lab = this.opts.liveLabel;
      this.liveText.textContent = lab === false ? '' : typeof lab === 'function' ? lab(show ? temp : null, st.shown, ch)
        : (show ? `${temp.toFixed(1)}° → ${Math.round(st.shown)}%` : `${Math.round(st.shown)}%`);
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
      const ch = this.ch;
      const fine = e.shiftKey ? 0.25 : 1;
      if (this.drag.i === -1) {
        ch.cfg.duty = Math.round(C.util.clamp(this.d(py), this.minDuty, 100));
      } else {
        const [a, b] = this.range;
        const old = ch.cfg.curve[this.drag.i];
        const t = fine < 1 ? old[0] + (this.t(px) - old[0]) * fine : this.t(px);
        const d = fine < 1 ? old[1] + (this.d(py) - old[1]) * fine : this.d(py);
        ch.cfg.curve = C.curve.dragPoint(ch.cfg.curve, this.drag.i, t, d, [a, b], this.minDuty);
      }
      this.render();
      this.showTip(this.drag.i);
      this.emit();
    }

    onUp() {
      if (!this.drag) return;
      this.drag = null;
      this.host.classList.remove('ce-dragging');
      this.tip.setAttribute('opacity', 0);
      this.render();
      this.emit(true);
    }

    onDbl(e) {
      const ch = this.ch;
      if (ch.cfg.source === 'fixed') return;
      const [px] = this.local(e);
      const r = C.curve.addPoint(ch.cfg.curve, this.t(px), this.range);
      ch.cfg.curve = r.points;
      this.render();
      this.emit(true);
    }

    remove(i) {
      const ch = this.ch;
      ch.cfg.curve = C.curve.removePoint(ch.cfg.curve, i);
      this.render();
      this.emit(true);
    }

    key(e, i) {
      const ch = this.ch;
      const k = e.shiftKey ? 5 : 1;
      let dt = 0, dd = 0;
      if (e.key === 'ArrowUp') dd = k; else if (e.key === 'ArrowDown') dd = -k;
      else if (e.key === 'ArrowRight') dt = k; else if (e.key === 'ArrowLeft') dt = -k;
      else if ((e.key === 'Delete' || e.key === 'Backspace') && i >= 0) {
        e.preventDefault();
        this.remove(i);
        this.focusPoint(Math.min(i, ch.cfg.curve.length - 1));
        return;
      } else if ((e.key === 'Insert' || e.key === '+') && i >= 0) {
        // A new point halfway to the next one, or to the previous one from the last.
        // A lone point (camless accepts one) gets one halfway to the far end of the range.
        e.preventDefault();
        const pts = ch.cfg.curve, [a, b] = this.range, t = pts[i][0];
        const j = i < pts.length - 1 ? i + 1 : i - 1;
        const to = j >= 0 ? pts[j][0] : t - a > b - t ? a : b;
        const r = C.curve.addPoint(pts, (t + to) / 2, this.range);
        ch.cfg.curve = r.points;
        this.render();
        this.focusPoint(r.index);
        this.emit(true);
        return;
      } else return;
      e.preventDefault();
      if (i === -1) ch.cfg.duty = C.util.clamp(ch.cfg.duty + dd, this.minDuty, 100);
      else { const [t, d] = ch.cfg.curve[i]; ch.cfg.curve = C.curve.dragPoint(ch.cfg.curve, i, t + dt, d + dd, this.range, this.minDuty); }
      this.render();
      this.emit(true);
    }

    focusPoint(i) {
      const g = this.gPts.querySelector(i === -1 ? '.ce-pt' : `[data-i="${i}"]`);
      if (g) g.focus();
    }

    showTip(i) {
      const ch = this.ch;
      const [t, d] = i === -1 ? [null, ch.cfg.duty] : ch.cfg.curve[i];
      const txt = i === -1 ? `${d} %` : `${this.opts.tempLabel ? this.opts.tempLabel(t) : t + ' °C'}  ${d} %`;
      this.tipText.textContent = txt;
      const x = i === -1 ? this.x((this.range[0] + this.range[1]) / 2) : this.x(t);
      const y = this.y(d);
      const w = txt.length * 6.6 + 14;
      const tx = C.util.clamp(x - w / 2, 2, this.w - w - 2);
      const ty = y - 34 < 0 ? y + 14 : y - 34;
      this.tipBg.setAttribute('x', tx); this.tipBg.setAttribute('y', ty);
      this.tipBg.setAttribute('width', w); this.tipBg.setAttribute('height', 22);
      this.tipText.setAttribute('x', tx + 7); this.tipText.setAttribute('y', ty + 15);
      this.tip.setAttribute('opacity', 1);
    }

    emit(final) {
      if (this.opts.onChange) this.opts.onChange(this.ch, final);
      if (final) C.changed('curve');
    }

    destroy() { this.ro.disconnect(); this.unsub(); this.svg.remove(); }
  }

  window.CurveEditor = CurveEditor;
})();
