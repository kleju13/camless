/* The controls the pages are built from, in NZXT CAM's look: selects, switches,
   sliders, dialogs, toasts and tabs. Nothing here knows about camless's data. */
(function () {
  'use strict';
  const { clamp } = window.S.util;

  const h = (html) => { const t = document.createElement('template'); t.innerHTML = html.trim(); return t.content.firstElementChild; };
  const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  let uidN = 0;
  const uid = (p = 'k') => `${p}${++uidN}`;
  const ic = (body) => `<svg class="i" viewBox="0 0 24 24" aria-hidden="true">${body}</svg>`;

  const I = {
    monitoring: ic('<path d="M3 12.5h4l2.5-6 4 12 2.6-6H21"/>'),
    specs: ic('<rect x="5" y="3.5" width="14" height="17" rx="2"/><path d="M9 8h6M9 12h6M9 16h3"/>'),
    cooling: ic('<circle cx="12" cy="12" r="1.8"/><path d="M12 10.2C11 6 12 3 15 3c2.2 0 3 2.4.8 4.4L13.6 9.6M13.8 12c4.2-1 7.2 0 7.2 3 0 2.2-2.4 3-4.4.8l-2.2-2.2M12 13.8c1 4.2 0 7.2-3 7.2-2.2 0-3-2.4-.8-4.4l2.2-2.2M10.2 12C6 13 3 12 3 9c0-2.2 2.4-3 4.4-.8l2.2 2.2"/>'),
    lighting: ic('<path d="M9.2 17.5h5.6M10 20.5h4M12 3a6 6 0 0 0-3.6 10.8c.7.6 1.1 1.4 1.1 2.3v.4h5v-.4c0-.9.4-1.7 1.1-2.3A6 6 0 0 0 12 3z"/>'),
    lcd: ic('<circle cx="12" cy="12" r="8.5"/><circle cx="12" cy="12" r="5"/>'),
    settings: ic('<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/>'),
    kraken: ic('<circle cx="11" cy="12" r="7.5"/><circle cx="11" cy="12" r="4"/><path d="M18.5 9.5H22M18.5 14.5H22"/>'),
    hub: ic('<rect x="3.5" y="6.5" width="17" height="11" rx="2"/><path d="M7.5 10.5v3M10.5 10.5v3M13.5 10.5v3M16.5 10.5v3"/>'),
    mini: ic('<rect x="3" y="4.5" width="18" height="15" rx="2"/><rect x="12" y="11.5" width="6.5" height="5.5" rx="1"/>'),
    chevron: ic('<path d="M6.5 9.5l5.5 5.5 5.5-5.5"/>'),
    check: ic('<path d="M5 12.5l4.5 4.5L19 7.5"/>'),
    close: ic('<path d="M6 6l12 12M18 6L6 18"/>'),
    rotate: ic('<path d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3M19.5 4.5v4.5H15"/>'),
    play: ic('<path d="M8 5.5v13l10-6.5z"/>'),
    swap: ic('<path d="M4 8h13l-3.5-3.5M20 16H7l3.5 3.5"/>'),
    warn: ic('<path d="M12 3.5l9 16H3z"/><path d="M12 10v4.2M12 17h.01"/>'),
    info: ic('<circle cx="12" cy="12" r="8.5"/><path d="M12 11v5M12 8h.01"/>'),
    chip: ic('<rect x="6" y="6" width="12" height="12" rx="1.5"/><path d="M9.5 3v3M14.5 3v3M9.5 18v3M14.5 18v3M3 9.5h3M3 14.5h3M18 9.5h3M18 14.5h3"/>'),
    plus: ic('<path d="M12 5v14M5 12h14"/>'),
    curve: ic('<path d="M3 19h18M4 17c5 0 6-9 9-9s3.5 2 7-4"/>'),
    more: ic('<circle cx="5.5" cy="12" r="1.3"/><circle cx="12" cy="12" r="1.3"/><circle cx="18.5" cy="12" r="1.3"/>'),
  };
  // c and a minus, for cam-less. tools/icon/make_icon.py draws the same mark for the exe and the tray.
  const LOGO = '<svg viewBox="0 0 64 64" aria-hidden="true"><path d="M35.607 42.607A15 15 0 1 1 35.607 21.393" fill="none" stroke="currentColor" stroke-width="9" stroke-linecap="round"/><path d="M43 32H55" stroke="var(--orchid)" stroke-width="9" stroke-linecap="round"/></svg>';
  const FONT = '"Montserrat", system-ui, sans-serif';

  /* ------------------------------------------------------------ prefs, bus */

  /* How the app looks and which fans are on each lighting channel. They stay with
     this PC's WebView2 profile, not in camless.toml. */
  const PREF_KEY = 'camless-prefs';
  const prefs = Object.assign({ theme: 'dark', unit: 'c', fans: {} }, readPrefs());
  function readPrefs() { try { return JSON.parse(localStorage.getItem(PREF_KEY)) || {}; } catch (e) { return {}; } }
  function setPref(k, v) {
    prefs[k] = v;
    try { localStorage.setItem(PREF_KEY, JSON.stringify(prefs)); } catch (e) { /* storage off */ }
    emit('prefs', k);
  }
  const listeners = {};
  function on(evt, fn) { (listeners[evt] = listeners[evt] || []).push(fn); return () => { listeners[evt] = listeners[evt].filter((f) => f !== fn); }; }
  function emit(evt, arg) { for (const f of (listeners[evt] || []).slice()) { try { f(arg); } catch (e) { console.error(e); } } }

  const toUnit = (c) => (prefs.unit === 'f' ? c * 1.8 + 32 : c);
  const deg = () => (prefs.unit === 'f' ? '°F' : '°C');
  const tnum = (c, dp = 1) => (c == null ? '--' : toUnit(c).toFixed(dp));
  const temp = (c, dp = 1) => (c == null ? 'N/A' : tnum(c, dp) + deg());
  const rpm = (v) => (v == null ? '--' : Math.round(v).toLocaleString('en-US'));

  /* Each page render collects its subscriptions so navigating away drops them. */
  let scope = null;
  function track(fn) { if (scope) scope.push(fn); return fn; }
  function withScope(fn) {
    const mine = [], prev = scope;
    scope = mine;
    try { fn(); } finally { scope = prev; }
    return () => { for (const f of mine.splice(0).reverse()) { try { f(); } catch (e) { console.error(e); } } };
  }
  // A host whose contents can be re-rendered without leaking the previous render's subscriptions.
  function region(host) {
    let dispose = null;
    track(() => dispose && dispose());
    return (render) => { if (dispose) dispose(); host.textContent = ''; dispose = withScope(() => render(host)); };
  }

  let root = null;
  const layer = () => root || document.body;
  /* The app's visible box, and where position: fixed starts inside it. They differ from
     the browser window when the demo shows camless in a smaller window of its own. */
  function viewBox() {
    const probe = h('<div style="position:fixed;left:0;top:0;width:0;height:0;visibility:hidden"></div>');
    layer().appendChild(probe);
    const o = probe.getBoundingClientRect();
    probe.remove();
    const b = root ? root.getBoundingClientRect() : { left: 0, top: 0, right: innerWidth, bottom: innerHeight };
    return { ox: o.left, oy: o.top, left: Math.max(0, b.left), top: Math.max(0, b.top), right: Math.min(innerWidth, b.right), bottom: Math.min(innerHeight, b.bottom) };
  }

  /* --------------------------------------------------------------- controls */

  function select(o) {
    const el = h(`<div class="cam-select"><button type="button" class="cam-select-btn" aria-haspopup="listbox" aria-expanded="false"><span class="v"></span>${I.chevron}</button></div>`);
    const btn = el.firstElementChild, vEl = btn.firstElementChild;
    let value = o.value, options = o.options, menu = null, active = 0;
    const lid = uid('ls');
    const paint = () => {
      const opt = options.find((x) => x.value === value);
      vEl.textContent = opt ? opt.label : '';
      if (o.label) btn.setAttribute('aria-label', `${o.label}: ${opt ? opt.label : 'none'}`);
    };
    const setActive = (i) => {
      active = clamp(i, 0, options.length - 1);
      [...menu.children].forEach((li, j) => li.classList.toggle('active', j === active));
      menu.setAttribute('aria-activedescendant', `${lid}-${active}`);
      menu.children[active].scrollIntoView({ block: 'nearest' });
    };
    function place() {
      const v = viewBox(), r = btn.getBoundingClientRect();
      menu.style.minWidth = r.width + 'px';
      const mh = Math.min(menu.scrollHeight, 320), mw = menu.offsetWidth;
      const below = v.bottom - r.bottom - 12, above = r.top - v.top - 12;
      const down = below >= mh || above < mh;
      menu.style.top = Math.max(v.top + 8, down ? r.bottom + 4 : r.top - Math.min(mh, above) - 4) - v.oy + 'px';
      menu.style.maxHeight = Math.max(120, down ? below : above) + 'px';
      menu.style.left = Math.max(v.left + 8, Math.min(r.left, v.right - mw - 8)) - v.ox + 'px';
    }
    const outside = (e) => { if (menu && !menu.contains(e.target) && !btn.contains(e.target)) close(false); };
    const onScroll = (e) => { if (menu && e.target !== menu) close(false); };
    function open() {
      if (menu) return;
      menu = h(`<ul class="cam-menu" role="listbox" tabindex="-1" id="${lid}"></ul>`);
      if (o.label) menu.setAttribute('aria-label', o.label);
      options.forEach((x, i) => {
        const sel = x.value === value;
        const li = h(`<li role="option" id="${lid}-${i}" aria-selected="${sel}"><span>${esc(x.label)}</span>${x.desc ? `<span class="d">${esc(x.desc)}</span>` : ''}${sel ? I.check : ''}</li>`);
        li.addEventListener('click', () => pick(x.value));
        li.addEventListener('pointermove', () => { if (active !== i) setActive(i); });
        menu.appendChild(li);
      });
      layer().appendChild(menu);
      place();
      setActive(Math.max(0, options.findIndex((x) => x.value === value)));
      btn.setAttribute('aria-expanded', 'true');
      btn.setAttribute('aria-controls', lid);
      menu.addEventListener('keydown', onKey);
      menu.focus({ preventScroll: true });
      document.addEventListener('pointerdown', outside, true);
      document.addEventListener('scroll', onScroll, true);
      addEventListener('resize', closeQuiet);
    }
    function closeQuiet() { close(false); }
    function close(refocus) {
      if (!menu) return;
      document.removeEventListener('pointerdown', outside, true);
      document.removeEventListener('scroll', onScroll, true);
      removeEventListener('resize', closeQuiet);
      menu.remove();
      menu = null;
      btn.setAttribute('aria-expanded', 'false');
      if (refocus) btn.focus({ preventScroll: true });
    }
    function pick(v) {
      close(true);
      if (v === value) return;
      value = v;
      paint();
      if (o.onChange) o.onChange(v);
    }
    function onKey(e) {
      const k = e.key;
      if (k === 'ArrowDown') setActive(active + 1);
      else if (k === 'ArrowUp') setActive(active - 1);
      else if (k === 'Home') setActive(0);
      else if (k === 'End') setActive(options.length - 1);
      else if (k === 'Enter' || k === ' ') pick(options[active].value);
      else if (k === 'Escape') close(true);
      else if (k === 'Tab') { close(false); return; }
      else return;
      e.preventDefault();
      e.stopPropagation();
    }
    btn.addEventListener('click', () => (menu ? close(true) : open()));
    btn.addEventListener('keydown', (e) => { if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); open(); } });
    el.set = (v) => { value = v; paint(); };
    el.setOptions = (opts, v) => { options = opts; if (v !== undefined) value = v; paint(); };
    el.setDisabled = (d) => { el.classList.toggle('disabled', !!d); btn.disabled = !!d; };
    el.value = () => value;
    track(() => close(false));
    paint();
    return el;
  }

  function choice(o) {
    const el = h('<div class="cam-choice" role="radiogroup"></div>');
    if (o.label) el.setAttribute('aria-label', o.label);
    let value = o.value;
    for (const x of o.options) {
      const b = h(`<button type="button" role="radio" data-v="${esc(x.value)}">${esc(x.label)}</button>`);
      b.addEventListener('click', () => set(x.value, true));
      el.appendChild(b);
    }
    el.addEventListener('keydown', (e) => {
      const d = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 0;
      if (!d) return;
      e.preventDefault();
      const btns = [...el.children];
      const i = Math.max(0, btns.findIndex((b) => b.dataset.v === String(value)));
      const n = btns[(i + d + btns.length) % btns.length];
      set(o.options.find((x) => String(x.value) === n.dataset.v).value, true);
      n.focus();
    });
    function set(v, user) {
      value = v;
      const btns = [...el.children];
      let any = false;
      for (const b of btns) { const sel = b.dataset.v === String(v); any = any || sel; b.setAttribute('aria-checked', String(sel)); b.tabIndex = sel ? 0 : -1; }
      if (!any && btns[0]) btns[0].tabIndex = 0;
      if (user && o.onChange) o.onChange(v);
    }
    el.set = (v) => set(v, false);
    set(value, false);
    return el;
  }

  function slider(o) {
    const el = h(`<div class="cam-slider"><input type="range" min="${o.min}" max="${o.max}" step="${o.step || 1}"><output></output></div>`);
    const inp = el.firstElementChild, out = el.lastElementChild;
    inp.value = o.value;
    if (o.label) inp.setAttribute('aria-label', o.label);
    const paint = () => {
      inp.style.setProperty('--p', ((inp.value - o.min) / (o.max - o.min)) * 100 + '%');
      out.textContent = o.format ? o.format(+inp.value) : inp.value;
      inp.setAttribute('aria-valuetext', out.textContent);
    };
    inp.addEventListener('input', () => { paint(); if (o.onInput) o.onInput(+inp.value); });
    inp.addEventListener('change', () => { if (o.onChange) o.onChange(+inp.value); });
    el.set = (v) => { inp.value = v; paint(); };
    paint();
    return el;
  }

  function toggle(o) {
    const b = h(`<button type="button" class="cam-toggle" role="switch" aria-checked="${!!o.checked}"></button>`);
    if (o.label) b.setAttribute('aria-label', o.label);
    b.addEventListener('click', () => { const v = b.getAttribute('aria-checked') !== 'true'; b.setAttribute('aria-checked', String(v)); if (o.onChange) o.onChange(v); });
    b.set = (v) => b.setAttribute('aria-checked', String(!!v));
    return b;
  }

  function button(label, o = {}) {
    const b = h(`<button type="button" class="cam-btn${o.primary ? ' primary' : ''}${o.small ? ' sm' : ''}">${o.icon ? I[o.icon] : ''}<span></span></button>`);
    b.lastElementChild.textContent = label;
    if (o.onClick) b.addEventListener('click', o.onClick);
    return b;
  }

  // A settings row: title and hint on the left, controls on the right. Hints are trusted markup.
  function row(title, hint, ctl) {
    const r = h(`<div class="cam-row"><div class="txt"><b></b>${hint != null ? `<span>${hint}</span>` : ''}</div><div class="ctl"></div></div>`);
    r.querySelector('b').textContent = title;
    if (ctl) r.lastElementChild.append(...[].concat(ctl));
    r.hint = (html) => { const s = r.querySelector('.txt span'); if (s) s.innerHTML = html; };
    return r;
  }

  /* ---------------------------------------------------------- modal, toast */

  let overlays = [];
  function modal(o) {
    const prev = document.activeElement;
    const bd = h(`<div class="cam-backdrop"><div class="cam-modal${o.wide ? ' wide' : ''}" role="dialog" aria-modal="true"><header><h2></h2><button type="button" class="cam-iconbtn" aria-label="Close">${I.close}</button></header><div class="body"></div></div></div>`);
    const dlg = bd.firstElementChild, title = dlg.querySelector('h2'), body = dlg.querySelector('.body');
    title.id = uid('mt');
    title.textContent = o.title;
    dlg.setAttribute('aria-labelledby', title.id);
    layer().appendChild(bd);
    let dispose = null;
    const close = () => {
      if (!bd.isConnected) return;
      if (dispose) dispose();
      bd.remove();
      overlays = overlays.filter((f) => f !== close);
      if (o.onClose) o.onClose();
      if (prev && prev.isConnected && prev.focus) prev.focus({ preventScroll: true });
    };
    const api = { close, body, footer: null, buttons: [] };
    dispose = withScope(() => o.render && o.render(body, api));
    if (o.actions) {
      const f = h('<footer></footer>');
      for (const a of o.actions) {
        const b = button(a.label, { primary: a.primary });
        b.addEventListener('click', () => { if (!a.onClick || a.onClick(api) !== false) close(); });
        f.appendChild(b);
        api.buttons.push(b);
      }
      dlg.appendChild(f);
      api.footer = f;
    }
    if (o.ready) o.ready(api);
    dlg.querySelector('header button').addEventListener('click', close);
    bd.addEventListener('pointerdown', (e) => { if (e.target === bd) close(); });
    bd.addEventListener('keydown', (e) => {
      if (e.key !== 'Tab') return;
      const f = [...dlg.querySelectorAll('button, input, [tabindex="0"], a[href]')].filter((x) => !x.disabled && x.offsetParent !== null);
      if (!f.length) return;
      const first = f[0], last = f[f.length - 1];
      if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
      else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
    });
    overlays.push(close);
    const first = body.querySelector('button, input, [tabindex="0"]') || dlg.querySelector('header button');
    first.focus({ preventScroll: true });
    return api;
  }
  const closeOverlays = () => overlays.slice().forEach((f) => f());
  // Escape closes the top modal even after a redraw dropped focus out of it.
  document.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape' || e.defaultPrevented || !overlays.length) return;
    e.preventDefault();
    overlays[overlays.length - 1]();
  });

  let toastEl = null, toastTimer = 0;
  function toast(text) {
    if (toastEl) toastEl.remove();
    toastEl = h('<div class="cam-toast" role="status"></div>');
    toastEl.textContent = text;
    layer().appendChild(toastEl);
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { if (toastEl) toastEl.remove(); toastEl = null; }, 3400);
  }

  function tabStrip(items, value, onPick, o = {}) {
    const el = h(`<div class="cam-tabs${o.sub ? ' sub' : ''}" role="tablist"></div>`);
    if (o.label) el.setAttribute('aria-label', o.label);
    const set = (v) => el.querySelectorAll('button').forEach((b) => { const s = b.dataset.id === v; b.setAttribute('aria-selected', String(s)); b.tabIndex = s ? 0 : -1; });
    for (const it of items) {
      const b = h(`<button type="button" role="tab" data-id="${esc(it.id)}"></button>`);
      b.textContent = it.label;
      b.addEventListener('click', () => { set(it.id); onPick(it.id); });
      el.appendChild(b);
    }
    el.addEventListener('keydown', (e) => {
      const d = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
      if (!d) return;
      const bs = [...el.children], i = bs.findIndex((b) => b === document.activeElement);
      const n = bs[(i + d + bs.length) % bs.length];
      n.focus(); n.click();
    });
    set(value);
    return el;
  }

  function pageHead(host, title, extra) {
    const el = h('<div class="cam-pagehead"><h1 class="cam-h1"></h1><div class="grow"></div></div>');
    el.firstElementChild.textContent = title;
    if (extra) el.append(...[].concat(extra));
    host.appendChild(el);
    return el;
  }

  function section(host, title, extra) {
    const el = h('<section class="cam-section"><header><h2 class="cam-h2"></h2><span class="grow"></span></header></section>');
    el.querySelector('h2').textContent = title;
    if (extra) el.firstElementChild.append(...[].concat(extra));
    host.appendChild(el);
    return el;
  }

  window.K = {
    I, LOGO, FONT, h, esc, uid, prefs, setPref, on, emit, temp, tnum, deg, toUnit, rpm,
    track, withScope, region, viewBox, setRoot: (r) => { root = r; },
    select, choice, slider, toggle, button, row, modal, toast, closeOverlays, tabStrip, pageHead, section,
  };
})();
