/* Components for the cam-* pages: camless's settings laid out the way NZXT CAM lays
   them out, with CAM's names for things, so people moving from CAM find everything
   where they expect it. Runs on the shared simulation in camless-sim.js. */
(function () {
  'use strict';
  const C = window.Camless, K = window.KrakenDraw;
  const { clamp, clone } = C.util;
  const TAU = Math.PI * 2;
  const DEV = C.devices;

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
    swap: ic('<path d="M4 8h13l-3.5-3.5M20 16H7l3.5 3.5"/>'),
    warn: ic('<path d="M12 3.5l9 16H3z"/><path d="M12 10v4.2M12 17h.01"/>'),
    info: ic('<circle cx="12" cy="12" r="8.5"/><path d="M12 11v5M12 8h.01"/>'),
    chip: ic('<rect x="6" y="6" width="12" height="12" rx="1.5"/><path d="M9.5 3v3M14.5 3v3M9.5 18v3M14.5 18v3M3 9.5h3M3 14.5h3M18 9.5h3M18 14.5h3"/>'),
    plus: ic('<path d="M12 5v14M5 12h14"/>'),
    curve: ic('<path d="M3 19h18M4 17c5 0 6-9 9-9s3.5 2 7-4"/>'),
  };
  const LOGO = '<svg viewBox="0 0 32 32" aria-hidden="true"><circle cx="16" cy="16" r="12" fill="none" stroke="#bf4cbf" stroke-width="4.5" stroke-dasharray="3.3 1.4"/><circle cx="16" cy="16" r="5.5" fill="currentColor"/></svg>';
  const WIN = '<span><svg viewBox="0 0 10 10"><path d="M0 5.5h10"/></svg></span><span><svg viewBox="0 0 10 10"><rect x=".5" y=".5" width="9" height="9"/></svg></span><span><svg viewBox="0 0 10 10"><path d="M0 0l10 10M10 0L0 10"/></svg></span>';
  const FONT = '"Montserrat", system-ui, sans-serif';

  /* ------------------------------------------------------------ prefs, bus */

  const PREF_KEY = 'camless-cam-prefs';
  const prefs = Object.assign({ theme: 'dark', unit: 'c', tray: true, startup: true, startMin: true, miniDir: 'vertical', miniTop: true }, readPrefs());
  function readPrefs() { try { return JSON.parse(localStorage.getItem(PREF_KEY)) || {}; } catch (e) { return {}; } }
  function setPref(k, v) {
    prefs[k] = v;
    try { localStorage.setItem(PREF_KEY, JSON.stringify(prefs)); } catch (e) { /* private mode */ }
    emit('prefs', k);
  }
  const listeners = {};
  function on(evt, fn) { (listeners[evt] = listeners[evt] || []).push(fn); return () => { listeners[evt] = listeners[evt].filter((f) => f !== fn); }; }
  function emit(evt, arg) { for (const f of (listeners[evt] || []).slice()) { try { f(arg); } catch (e) { console.error(e); } } }

  const toUnit = (c) => (prefs.unit === 'f' ? c * 1.8 + 32 : c);
  const deg = () => (prefs.unit === 'f' ? '°F' : '°C');
  const tnum = (c, dp = 1) => (c == null ? '--' : toUnit(c).toFixed(dp));
  const temp = (c, dp = 1) => (c == null ? 'N/A' : tnum(c, dp) + deg());
  const rpm = (v) => Math.round(v || 0).toLocaleString('en-US');

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
      if (e.key === 'Escape') { e.preventDefault(); close(); return; }
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

  let toastEl = null, toastTimer = 0;
  function toast(text) {
    if (toastEl) toastEl.remove();
    toastEl = h('<div class="cam-toast" role="status"></div>');
    toastEl.textContent = text;
    layer().appendChild(toastEl);
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { if (toastEl) toastEl.remove(); toastEl = null; }, 3400);
  }

  /* ----------------------------------------------------------------- shell */

  // o: { nav: [{id, label, icon, sub} | {group} | {spacer}] or null, pages: {id: (host, rest) => {}}, initial }
  function shell(o) {
    root = h(`<div class="cam" data-theme="${prefs.theme === 'light' ? 'light' : 'dark'}">
      <header class="cam-title">
        <div class="cam-logo">${LOGO}<span>camless</span></div>
        <div class="grow"></div>
        <button type="button" class="cam-tb" data-act="mini" aria-pressed="false" title="Launch Mini Mode">${I.mini}<span class="t">Mini Mode</span></button>
        <div class="cam-win" aria-hidden="true">${WIN}</div>
      </header>
      <div class="cam-body${o.nav ? '' : ' nonav'}">
        ${o.nav ? '<nav class="cam-nav" aria-label="Main"></nav>' : ''}
        <main class="cam-main"><div class="cam-content"></div></main>
      </div>
    </div>`);
    document.body.prepend(root);
    const nav = root.querySelector('.cam-nav'), main = root.querySelector('.cam-main'), content = root.querySelector('.cam-content');
    if (nav) {
      nav.innerHTML = o.nav.map((n) => n.spacer ? '<div class="spacer"></div>'
        : n.group ? `<div class="group">${esc(n.group)}</div>`
        : `<a href="#${n.id}" data-id="${n.id}" title="${esc(n.label)}">${I[n.icon] || ''}<span class="lbl">${esc(n.label)}${n.sub ? `<small>${n.sub}</small>` : ''}</span></a>`).join('');
    }
    root.querySelector('[data-act=mini]').addEventListener('click', () => miniMode());
    on('prefs', (k) => { if (k === 'theme') root.dataset.theme = prefs.theme; });

    let dispose = null, page = null;
    // An import can add or remove fan channels while a page is open, so it is built again.
    const layout = () => C.channels().map((c) => c.id).join();
    let built = layout();
    C.on('config', () => { if (layout() !== built) go(); });
    function go() {
      built = layout();
      const hash = decodeURIComponent(location.hash.slice(1)) || o.initial;
      let [id, ...rest] = hash.split('/');
      if (!o.pages[id]) [id, ...rest] = o.initial.split('/');
      if (dispose) dispose();
      closeOverlays();
      content.textContent = '';
      dispose = withScope(() => { alerts(content); o.pages[id](content, rest.join('/')); });
      if (nav) nav.querySelectorAll('a').forEach((a) => { if (a.dataset.id === id) a.setAttribute('aria-current', 'page'); else a.removeAttribute('aria-current'); });
      if (id !== page) main.scrollTop = 0;
      page = id;
      emit('page', id);
    }
    addEventListener('hashchange', go);
    go();
    return { root, main, content, go };
  }

  // Critical liquid and lost sensors, shown above every page.
  function alerts(host) {
    const el = h('<div class="cam-alert" role="status" hidden></div>');
    host.appendChild(el);
    let last = '';
    const upd = () => {
      const s = C.snapshot();
      let cls = '', html = '';
      if (s.critical) { cls = 'cam-alert'; html = `${I.warn}<div><b>Liquid is at ${temp(s.liquid)}.</b> The pump and every fan run at 100% until it cools below ${temp(55, 0)}.</div>`; }
      else if (s.faults.cpu) { cls = 'cam-alert warn'; html = `${I.warn}<div><b>CPU temperature is unavailable.</b> Channels that follow it hold ${C.config.failsafe_duty}% until it comes back.</div>`; }
      if (html === last) return;
      last = html;
      el.hidden = !html;
      el.className = cls || 'cam-alert';
      el.innerHTML = html;
    };
    track(C.on('tick', upd));
    track(C.on('fault', upd));
    track(on('prefs', upd));
    upd();
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

  /* --------------------------------------------------------------- cooling */

  const chName = (ch) => (ch.kind === 'pump' ? 'AIO Pump' : ch.device === 'kraken' ? 'Fan' : `Fan Connector ${ch.port}`);
  const chTile = (ch) => (ch.kind === 'pump' ? `${DEV.kraken.name} AIO Pump` : ch.device === 'kraken' ? `${DEV.kraken.name} Fan` : `${DEV.hub.name} Fan ${ch.port}`);
  function chSub(ch, withDevice) {
    const own = ch.kind === 'pump' ? 'Never below 20%' : ch.device === 'kraken' ? `3 × ${fanName(fanSpec('radiator'))}` : [ch.cfg.name, fanName(fanSpec(ch.id))].filter(Boolean).join(', ');
    if (!withDevice) return own;
    const dev = ch.device === 'kraken' ? DEV.kraken.name : DEV.hub.name;
    return own ? `${dev}, ${midSentence(own)}` : dev;
  }
  const channel = (id) => C.channels().find((c) => c.id === id);
  // Lowercases a leading word for use mid-sentence, leaving acronyms such as RGB alone.
  const midSentence = (s) => (/^[A-Z][a-z]/.test(s) ? s[0].toLowerCase() + s.slice(1) : s);

  const PRESET_OPTS = [{ value: 'fixed', label: 'Fixed' }, { value: 'silent', label: 'Silent' }, { value: 'performance', label: 'Performance' }, { value: 'custom', label: 'Custom' }];
  const SOURCE_OPTS = [{ value: 'liquid', label: 'Liquid' }, { value: 'cpu', label: 'CPU' }, { value: 'gpu', label: 'GPU' }];
  const PRESET_LABEL = { fixed: 'Fixed', silent: 'Silent', performance: 'Performance', custom: 'Custom' };

  /* CAM keeps a preset per channel: Fixed, Silent, Performance or Custom. camless only
     stores a source and a curve, so the preset and each source's custom curve live here. */
  let memo = {};
  C.on('config', (what) => { if (what === 'replace') memo = {}; });
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
  function presetCurve(p, ch, src) {
    const P = C.presets[p];
    if (ch.kind === 'pump') return clone(src === 'liquid' ? P.liquid.map(([t, d]) => [t, Math.max(d, 40)]) : P.pump);
    return clone(P[src]);
  }
  function model(ch) {
    if (memo[ch.id]) return memo[ch.id];
    const c = ch.cfg;
    const m = { source: c.source === 'fixed' ? (ch.device === 'kraken' ? 'liquid' : 'cpu') : c.source, custom: {}, duty: c.source === 'fixed' ? c.duty : null, preset: 'custom' };
    if (c.source === 'fixed') m.preset = 'fixed';
    else {
      for (const p of ['silent', 'performance']) if (same(c.curve, presetCurve(p, ch, c.source))) m.preset = p;
      if (m.preset === 'custom') m.custom[c.source] = clone(c.curve);
    }
    return (memo[ch.id] = m);
  }
  function setPreset(ch, p, quiet) {
    const m = model(ch), c = ch.cfg;
    if (m.preset === 'custom' && c.source !== 'fixed') m.custom[c.source] = clone(c.curve);
    if (m.preset === 'fixed') m.duty = c.duty;
    m.preset = p;
    if (p === 'fixed') {
      const st = C.live(ch.id);
      c.duty = clamp(Math.round(m.duty ?? (st ? st.shown : 50)), ch.minDuty, 100);
      c.source = 'fixed';
      delete c.curve;
    } else {
      c.source = m.source;
      c.curve = p === 'custom' ? clone(m.custom[m.source] || presetCurve('balanced', ch, m.source)) : presetCurve(p, ch, m.source);
      delete c.duty;
    }
    if (!quiet) C.changed('cooling');
  }
  function setSource(ch, s) {
    const m = model(ch), c = ch.cfg;
    if (m.preset === 'custom' && c.source !== 'fixed') m.custom[c.source] = clone(c.curve);
    m.source = s;
    if (m.preset !== 'fixed') {
      c.source = s;
      c.curve = m.preset === 'custom' ? clone(m.custom[s] || presetCurve('balanced', ch, s)) : presetCurve(m.preset, ch, s);
    }
    C.changed('cooling');
  }
  // Called while a curve is dragged: editing a preset's curve makes it Custom.
  function curveEdited(ch) {
    const m = model(ch), c = ch.cfg;
    if (c.source === 'fixed') { m.duty = c.duty; return m.preset; }
    if (m.preset !== 'custom') m.preset = 'custom';
    m.custom[c.source] = clone(c.curve);
    return m.preset;
  }
  function applyCurve(src, targets) {
    const ms = model(src);
    for (const t of targets) {
      const mt = model(t), c = t.cfg;
      mt.preset = ms.preset;
      mt.source = ms.source;
      c.source = src.cfg.source;
      if (c.source === 'fixed') { c.duty = Math.max(t.minDuty, src.cfg.duty); mt.duty = c.duty; delete c.curve; }
      else { c.curve = src.cfg.curve.map(([x, d]) => [x, Math.max(d, t.minDuty)]); delete c.duty; if (mt.preset === 'custom') mt.custom[c.source] = clone(c.curve); }
    }
    C.changed('cooling');
  }
  // One preset for every channel; fixed channels stay fixed, as in CAM.
  function setAllPresets(p) {
    for (const ch of C.channels()) if (model(ch).preset !== 'fixed') setPreset(ch, p, true);
    C.changed('cooling');
  }
  function allPreset() {
    const ps = [...new Set(C.channels().map((ch) => model(ch).preset).filter((p) => p !== 'fixed'))];
    return ps.length === 1 ? ps[0] : null;
  }

  function whereText(ch) {
    return ch.runsOn === 'cooler'
      ? { html: `${I.kraken}<span>Stored on the cooler</span>`, title: 'The Kraken runs this curve itself, so it keeps working when camless is closed.' }
      : { html: `${I.chip}<span>Set by camless</span>`, title: `camless sets this speed every second. If camless closes, it goes to ${C.config.failsafe_duty}%, and the Kraken still speeds up on its own as the liquid warms.` };
  }

  function curveOpts(get, onEdit) {
    return {
      channel: get,
      pad: { l: 40, r: 14, t: 20, b: 26 },
      floor: true,
      tempLabel: (t) => Math.round(toUnit(t)) + '°',
      liveLabel: (t, d, ch) => (t == null ? `${Math.round(d)}%` : `${C.sources[ch.cfg.source].label}: ${Math.round(toUnit(t))}°`),
      onChange: (ch, final) => onEdit(ch, final),
    };
  }

  // A channel the way CAM draws it: name, live speed, presets, source, curve.
  function coolingCard(id, o = {}) {
    const ch0 = channel(id);
    const el = h(`<section class="cam-panel cam-ch">
      <div class="cam-ch-head">${ch0.kind === 'pump' ? '<span class="cam-ch-pic"><canvas aria-hidden="true"></canvas></span>' : '<button type="button" class="cam-ch-pic"><canvas aria-hidden="true"></canvas></button>'}<div class="names"><h3 class="cam-ch-name"></h3><p class="cam-ch-sub"></p></div>
        <div class="cam-ch-live"><span class="rpm"><b class="num">0</b> RPM</span><span class="duty num"><b>0</b><small>%</small></span></div></div>
      <div class="presets"></div>
      <div class="cam-ch-src"><span>Temperatures source:</span></div>
      <p class="cam-ch-note" hidden>${I.warn}<span></span></p>
      <div class="cam-ch-curve${o.short ? ' short' : ''}"></div>
      <div class="cam-ch-foot"><button type="button" class="cam-link">Apply this curve to other channels</button><span class="grow"></span><span class="cam-where" tabindex="0"></span></div>
    </section>`);
    el.setAttribute('aria-label', chTile(ch0));
    el.querySelector('.cam-ch-name').textContent = chName(ch0);
    const get = () => channel(id);
    const pc = choice({ options: PRESET_OPTS, value: model(ch0).preset, label: `${chName(ch0)} preset`, onChange: (p) => setPreset(get(), p) });
    el.querySelector('.presets').appendChild(pc);
    const src = select({ options: SOURCE_OPTS, value: model(ch0).source, label: 'Temperatures source', onChange: (s) => setSource(get(), s) });
    el.querySelector('.cam-ch-src').appendChild(src);
    const ed = new window.CurveEditor(el.querySelector('.cam-ch-curve'), curveOpts(get, (ch) => pc.set(curveEdited(ch))));
    chPicture(el.querySelector('.cam-ch-pic'), id);
    track(() => ed.destroy());
    el.querySelector('.cam-link').addEventListener('click', () => applyDialog(get()));
    const where = el.querySelector('.cam-where');
    const rpmB = el.querySelector('.rpm b'), dutyB = el.querySelector('.duty b');
    const note = el.querySelector('.cam-ch-note'), noteT = note.lastElementChild;

    function refresh() {
      const ch = get();
      if (!ch) return;
      const m = model(ch);
      el.querySelector('.cam-ch-sub').textContent = o.sub != null ? o.sub : chSub(ch, o.withDevice);
      el.querySelector('.cam-ch-sub').hidden = !el.querySelector('.cam-ch-sub').textContent;
      el.querySelector('.cam-ch-sub').title = el.querySelector('.cam-ch-sub').textContent;
      pc.set(m.preset);
      src.set(m.source);
      src.setDisabled(m.preset === 'fixed');
      const w = whereText(ch);
      where.innerHTML = w.html;
      where.title = w.title;
      where.setAttribute('aria-label', w.title);
      ed.render();
      live();
    }
    function live() {
      const ch = get(), st = C.live(id);
      if (!ch || !st) return;
      rpmB.textContent = rpm(st.rpm);
      dutyB.textContent = Math.round(st.shown);
      const missing = ch.cfg.source !== 'fixed' && st.temp == null;
      note.hidden = !missing;
      if (missing) noteT.textContent = `Temperature source is unavailable. Holding ${C.config.failsafe_duty}% until it comes back.`;
    }
    track(C.on('config', refresh));
    track(C.on('tick', live));
    track(on('prefs', (k) => { if (k === 'unit') ed.render(); else if (k === 'fans') refresh(); }));
    refresh();
    return el;
  }

  /* The hardware on a cooling card: the pump head, or one of the channel's fans in its
     model, spinning at its speed and lit as on the Lighting page. A fan opens the model picker. */
  function chPicture(pic, id) {
    const cv = pic.firstElementChild;
    const target = () => (id === 'radiator' ? lightTargets('kraken')[1] : lightTargets('hub').find((t) => t.fan === id));
    if (pic.tagName === 'BUTTON') {
      const label = () => { const t = target(); return t ? `Fans on ${t.label}: ${fanText(t)}. Change` : ''; };
      const paint = () => { pic.title = label(); pic.setAttribute('aria-label', pic.title); };
      paint();
      track(on('prefs', (k) => { if (k === 'fans') paint(); }));
      pic.addEventListener('click', () => { const t = target(); if (t) fanDialog(t); });
    }
    let time = 0, angle = 0;
    track(C.on('frame', (dt) => {
      time += dt;
      if (!cv.isConnected) return;
      const { ctx, w, h: hh } = K.fit(cv);
      ctx.clearRect(0, 0, w, hh);
      if (id === 'pump') {
        K.head(ctx, w / 2, hh / 2, Math.min(w, hh) / 2 - 1, { leds: C.light.render(C.config.ring, time, 24), liquid: C.state.liquid, lcd: C.config.lcd, snapshot: C.snapshot(), font: FONT, shadow: false, rotation: 0 });
        return;
      }
      const t = target(), st = C.live(id) || { rpm: 0 };
      angle += K.spin(st.rpm) * dt * TAU;
      const f = fanSpec(id);
      drawFan(ctx, w / 2, hh / 2, Math.min(w, hh) / 2 / 1.1, angle, t ? fanLeds(t, f, time, fanCount(t))(0) : [], f, st.rpm / 1500);
    }));
  }

  function applyDialog(src) {
    const others = C.channels().filter((c) => c.id !== src.id);
    const picked = new Set();
    const what = src.cfg.source === 'fixed' ? `fixed ${src.cfg.duty}%` : `${PRESET_LABEL[model(src).preset]} curve on ${C.sources[src.cfg.source].label} temperature`;
    modal({
      title: 'Apply this curve to other channels',
      render(body, api) {
        body.appendChild(h(`<p class="cam-hint" style="margin:0 0 10px">Select channels to apply curve. They get ${esc(chName(src).replace('AIO ', ''))}'s ${esc(what)}.</p>`));
        for (const c of others) {
          const lab = h(`<label class="cam-check"><input type="checkbox"><span>${esc(chTile(c))}</span><small>${esc(c.kind === 'pump' ? 'never below 20%' : c.cfg.name || '')}</small></label>`);
          lab.querySelector('input').addEventListener('change', (e) => { if (e.target.checked) picked.add(c.id); else picked.delete(c.id); if (api.buttons[1]) api.buttons[1].disabled = !picked.size; });
          body.appendChild(lab);
        }
      },
      actions: [
        { label: 'Cancel' },
        { label: 'Apply', primary: true, onClick: () => { if (!picked.size) return false; applyCurve(src, others.filter((c) => picked.has(c.id))); toast(`Applied to ${picked.size} channel${picked.size > 1 ? 's' : ''}.`); } },
      ],
      ready: (api) => { api.buttons[1].disabled = true; },
    });
  }

  // CAM's summary grid. keys: liquid, cpu, gpu, noise, cpuLoad, gpuLoad, or channel ids.
  const TILES = {
    liquid: { k: 'Liquid', v: (s) => tile(tnum(s.liquid), deg()), hot: (s) => s.critical },
    cpu: { k: 'CPU', v: (s) => (s.cpu.temp == null ? 'N/A' : tile(tnum(s.cpu.temp), deg())), na: (s) => s.cpu.temp == null },
    gpu: { k: 'GPU', v: (s) => (s.gpu.temp == null ? 'N/A' : tile(tnum(s.gpu.temp), deg())), na: (s) => s.gpu.temp == null },
    noise: { k: 'Case Noise', v: (s) => tile(Math.round(s.noise), 'dB') },
    cpuLoad: { k: 'CPU Load', v: (s) => tile(Math.round(s.cpu.load * 100), '%') },
    gpuLoad: { k: 'GPU Load', v: (s) => tile(Math.round(s.gpu.load * 100), '%') },
  };
  const tile = (v, u) => `${v}<small>${u}</small>`;
  function tiles(host, keys) {
    const el = h('<div class="cam-tiles"></div>');
    const items = keys.map((key) => {
      const ch = TILES[key] ? null : channel(key);
      const def = TILES[key] || { k: chTile(ch), v: () => tile(rpm((C.live(key) || {}).rpm), 'RPM') };
      const t = h('<div class="cam-panel cam-tile"><div class="k"></div><div class="v"></div></div>');
      t.firstElementChild.textContent = def.k;
      t.title = def.k;
      el.appendChild(t);
      return { t, def };
    });
    const upd = () => {
      const s = C.snapshot();
      for (const { t, def } of items) {
        t.lastElementChild.innerHTML = def.v(s);
        t.classList.toggle('hot', !!(def.hot && def.hot(s)));
        t.classList.toggle('na', !!(def.na && def.na(s)));
      }
    };
    track(C.on('tick', upd));
    track(on('prefs', upd));
    upd();
    host.appendChild(el);
    return el;
  }

  function inactiveRow(port) {
    return h(`<div class="cam-panel cam-inactive">${I.cooling}<b>Fan Connector ${port}</b><span class="grow"></span><span>No fan detected</span></div>`);
  }

  // Every cooling channel of one device, as CAM lists them.
  function deviceChannels(host, device, o = {}) {
    const grid = h(`<div class="cam-grid2${o.collapse ? ' collapse' : ''}"></div>`);
    for (const ch of C.channels().filter((c) => c.device === device)) grid.appendChild(coolingCard(ch.id, o));
    host.appendChild(grid);
    if (device === 'hub') {
      const used = new Set(C.config.hub_fan.map((f) => f.port));
      const free = [];
      for (let p = 1; p <= DEV.hub.ports; p++) if (!used.has(p)) free.push(p);
      if (free.length) {
        const s = h('<div class="cam-section" style="margin-top:20px"><header><h2 class="cam-h2 cam-muted" style="font-size:13px">Inactive</h2></header><div style="display:grid;gap:8px"></div></div>');
        for (const p of free) s.lastElementChild.appendChild(inactiveRow(p));
        host.appendChild(s);
      }
    }
    return grid;
  }

  /* -------------------------------------------------------------- lighting */

  const EFFECT_ORDER = ['fixed', 'breathing', 'fading', 'pulse', 'candle', 'covering-marquee', 'spectrum-wave', 'rainbow-flow', 'super-rainbow', 'rainbow-pulse', 'tai-chi', 'water-cooler', 'off'];
  const EFFECT_NAME = { off: 'Off', fixed: 'Fixed', fading: 'Fading', 'spectrum-wave': 'Spectrum Wave', 'covering-marquee': 'Covering Marquee', pulse: 'Pulse', breathing: 'Breathing', candle: 'Candle', 'rainbow-flow': 'Rainbow Flow', 'super-rainbow': 'Super Rainbow', 'rainbow-pulse': 'Rainbow Pulse', 'tai-chi': 'Tai Chi', 'water-cooler': 'Water Cooler' };
  const SPEED_NAME = { slowest: 'Slowest', slower: 'Slow', normal: 'Normal', faster: 'Fast', fastest: 'Fastest' };
  const DIRECTIONAL = new Set(['spectrum-wave', 'covering-marquee', 'rainbow-flow', 'super-rainbow', 'tai-chi', 'water-cooler']);
  const STILL = new Set(['fixed', 'off']);
  const PALETTE = ['ffffff', 'bf4cbf', '3232de', '21d373', 'ff8a00', 'ff022d', '00c8ff', 'ffd400'];

  function hubLed(port, create) {
    const list = C.config.hub_led;
    let x = list.find((l) => l.port === port);
    if (!x && create) { x = { port, mode: 'off', colors: ['ffffff'], speed: 'normal' }; list.push(x); list.sort((a, b) => a.port - b.port); }
    return x || { port, mode: 'off', colors: ['ffffff'], speed: 'normal', placeholder: true };
  }
  function lightTargets(device) {
    if (device === 'kraken') {
      return [
        { key: 'ring', label: 'Ring', sub: '24 LEDs around the screen', kind: 'ring', get: () => C.config.ring },
        { key: 'external', label: 'Channel 1', sub: 'RGB connector, 3 radiator fans', kind: 'fans', n: 3, fan: 'radiator', get: () => C.config.external },
      ];
    }
    return C.config.hub_fan.map((f) => f.port).sort((a, b) => a - b).map((p) => ({
      key: 'hub' + p, label: `Channel ${p}`, sub: (C.config.hub_fan.find((f) => f.port === p) || {}).name || `Port ${p}`,
      kind: 'fans', n: 1, fan: 'hub' + p, port: p, get: () => hubLed(p, false),
    }));
  }
  function editLight(t, fn) {
    let x = t.get();
    if (x.placeholder) x = hubLed(t.port, true);
    fn(x);
    C.changed('lighting');
  }
  function fitColors(x) {
    const [mn, mx] = C.light.EFFECTS[x.mode].colors;
    if (!x.colors) x.colors = [];
    if (mx > 0 && x.colors.length > mx) x.colors = x.colors.slice(0, mx);
    let i = 0;
    while (x.colors.length < mn) x.colors.push(PALETTE.find((c) => !x.colors.includes(c)) || PALETTE[i++ % PALETTE.length]);
  }

  /* Which NZXT fans sit on each lighting channel. CAM asks for this so its preview and
     effects match the case; camless keeps the answer with its own app settings. The
     LED counts only spread effects across the preview; they are not shown. */
  const FAN_MODELS = [
    { value: 'core', label: 'RGB Core', hint: 'LEDs in the hub', leds: 8 },
    { value: 'duo', label: 'RGB Duo', hint: 'LED rings on both sides', leds: 20 },
    { value: 'rgb', label: 'RGB', hint: 'LED ring on the front', leds: 12 },
    { value: 'plain', label: 'without RGB', hint: 'No LEDs', leds: 0 },
  ];
  const FAN_DEFAULTS = {
    radiator: { model: 'core', size: 120, color: 'black' },
    hub1: { model: 'duo', size: 120, color: 'black' },
    hub2: { model: 'duo', size: 120, color: 'black' },
    hub3: { model: 'rgb', size: 140, color: 'black' },
    hub4: { model: 'plain', size: 140, color: 'black' },
  };
  function fanSpec(key) { return Object.assign({ model: 'rgb', size: 120, color: 'black' }, FAN_DEFAULTS[key], (prefs.fans || {})[key]); }
  function setFan(key, spec) { setPref('fans', Object.assign({}, prefs.fans, { [key]: spec })); }
  function fanModel(f) { return FAN_MODELS.find((m) => m.value === f.model) || FAN_MODELS[0]; }
  function fanName(f) { return f.model === 'plain' ? `${f.size} mm fan without RGB` : `F${f.size} ${fanModel(f).label}`; }
  const fanCount = (t) => (t.fan === 'radiator' ? 3 : 1);
  const fanText = (t, withColor = true) => { const f = fanSpec(t.fan), n = fanCount(t); return `${n > 1 ? n + ' × ' : ''}${fanName(f)}${withColor ? ', ' + f.color : ''}`; };

  const FAN_FINISH = {
    black: { frame: '#1c1d21', pad: '#2a2c32', well: '#0a0b0d', blade: '#4b4e58', hub: '#26282d' },
    white: { frame: '#dfe0e4', pad: '#f2f3f5', well: '#1d1f24', blade: '#e9ebef', hub: '#cfd1d6' },
  };
  const dim = (leds, k) => leds.map((c) => [c[0] * k, c[1] * k, c[2] * k]);
  function mean(leds) {
    const s = [0, 0, 0];
    for (const c of leds) { s[0] += c[0]; s[1] += c[1]; s[2] += c[2]; }
    return s.map((v) => v / Math.max(1, leds.length));
  }
  // One fan seen from the front, in the chosen model and color. leds: this fan's share of the channel.
  function drawFan(ctx, cx, cy, r, angle, leds, f, blur = 0) {
    const P = FAN_FINISH[f.color] || FAN_FINISH.black;
    const s = r * 1.08;
    ctx.save();
    ctx.fillStyle = P.frame;
    K.roundRect(ctx, cx - s, cy - s, s * 2, s * 2, r * 0.16);
    ctx.fill();
    ctx.fillStyle = P.pad;
    for (const dx of [-1, 1]) for (const dy of [-1, 1]) { ctx.beginPath(); ctx.arc(cx + dx * s * 0.8, cy + dy * s * 0.8, r * 0.075, 0, TAU); ctx.fill(); }
    ctx.fillStyle = P.well;
    ctx.beginPath(); ctx.arc(cx, cy, r * 0.97, 0, TAU); ctx.fill();
    const lit = leds.length > 0;
    // Duo: the front ring, with the back ring showing through behind the blades.
    if (f.model === 'duo' && lit) {
      const half = Math.ceil(leds.length / 2);
      K.ledRing(ctx, cx, cy, r * 0.83, dim(leds.slice(half), 0.5), r * 0.04, 0.35);
      K.ledRing(ctx, cx, cy, r * 0.925, leds.slice(0, half), r * 0.08, 0.9);
    } else if (f.model === 'rgb' && lit) {
      K.ledRing(ctx, cx, cy, r * 0.925, leds, r * 0.05, 0.7);
    }
    ctx.translate(cx, cy);
    ctx.save();
    ctx.rotate(angle);
    const b = clamp(blur, 0, 1);
    ctx.fillStyle = P.blade;
    for (let k = 0; k < 7; k++) {
      ctx.save();
      ctx.rotate((k * TAU) / 7);
      ctx.globalAlpha = 1 - b * 0.55;
      ctx.beginPath();
      ctx.moveTo(r * 0.24, -r * 0.05);
      ctx.bezierCurveTo(r * 0.45, -r * 0.32, r * 0.78, -r * 0.3, r * 0.82, -r * 0.08);
      ctx.bezierCurveTo(r * 0.7, r * 0.06, r * 0.45, r * 0.12, r * 0.24, r * 0.1);
      ctx.closePath();
      ctx.fill();
      ctx.restore();
    }
    if (b > 0.05) {
      ctx.globalAlpha = b * 0.3;
      ctx.beginPath(); ctx.arc(0, 0, r * 0.8, 0, TAU); ctx.arc(0, 0, r * 0.24, 0, TAU, true); ctx.fill();
      ctx.globalAlpha = 1;
    }
    ctx.restore();
    // Core: the LEDs sit in the hub and light the blades from the middle.
    if (f.model === 'core' && lit) {
      const c = mean(leds);
      ctx.globalCompositeOperation = 'lighter';
      const g = ctx.createRadialGradient(0, 0, r * 0.2, 0, 0, r * 0.82);
      g.addColorStop(0, C.color.css(c, 0.5));
      g.addColorStop(1, C.color.css(c, 0));
      ctx.fillStyle = g;
      ctx.beginPath(); ctx.arc(0, 0, r * 0.82, 0, TAU); ctx.fill();
      ctx.globalCompositeOperation = 'source-over';
      ctx.fillStyle = P.hub;
      ctx.beginPath(); ctx.arc(0, 0, r * 0.28, 0, TAU); ctx.fill();
      K.ledRing(ctx, 0, 0, r * 0.19, leds, r * 0.09, 0.9);
      ctx.fillStyle = P.hub;
      ctx.beginPath(); ctx.arc(0, 0, r * 0.11, 0, TAU); ctx.fill();
    } else {
      ctx.fillStyle = P.hub;
      ctx.beginPath(); ctx.arc(0, 0, r * 0.25, 0, TAU); ctx.fill();
    }
    ctx.restore();
  }
  // The LEDs of every fan on a channel, rendered as one chain so effects run across them.
  function fanLeds(t, f, time, n) {
    const L = fanModel(f).leds;
    if (!L) return () => [];
    const all = C.light.render(t.get(), time, L * n);
    return (i) => all.slice(i * L, (i + 1) * L);
  }

  // Animated pictures of the hardware, drawn for camless.
  function drawLight(cv, t, time, angles) {
    const { ctx, w, h: hh } = K.fit(cv);
    ctx.clearRect(0, 0, w, hh);
    if (t.kind === 'ring') {
      K.head(ctx, w / 2, hh / 2, Math.min(w, hh) / 2 - 10, { leds: C.light.render(t.get(), time, 24), liquid: C.state.liquid, lcd: C.config.lcd, snapshot: C.snapshot(), font: FONT, rotation: 0, shadowColor: 'rgba(0,0,0,.45)' });
      return;
    }
    const st = C.live(t.fan) || { rpm: 0 };
    const f = fanSpec(t.fan), n = fanCount(t);
    const leds = fanLeds(t, f, time, n);
    if (n === 3) {
      const r = Math.min(w / 2.4, hh / 6.9);
      for (let i = 0; i < 3; i++) drawFan(ctx, w / 2, hh / 2 + (i - 1) * r * 2.3, r, angles[i] || 0, leds(i), f, st.rpm / 1800);
    } else {
      drawFan(ctx, w / 2, hh / 2, Math.min(w, hh) / 2.6, angles[0] || 0, leds(0), f, st.rpm / 1500);
    }
  }
  // A still, small picture of a channel: the ring, or one of its fans.
  function lightThumb(cv, t, time) {
    const { ctx, w, h: hh } = K.fit(cv);
    ctx.clearRect(0, 0, w, hh);
    if (t.kind === 'ring') { K.ledRing(ctx, w / 2, hh / 2, w / 2 - 6, C.light.render(t.get(), time, 24), 4, 0.45); return; }
    const f = fanSpec(t.fan);
    drawFan(ctx, w / 2, hh / 2, Math.min(w, hh) / 2 / 1.1, 0.3, fanLeds(t, f, time, 1)(0), f);
  }

  // CAM's step for telling it which fans are plugged in, with a picture of each look.
  function fanDialog(t) {
    let pick = fanSpec(t.fan);
    const n = fanCount(t);
    modal({
      title: `Fans on ${t.label}`,
      wide: true,
      render(body) {
        body.appendChild(h(`<p class="cam-hint" style="margin:0 0 14px">${n > 1 ? `The Kraken's ${n} radiator fans share this channel.` : `${esc(t.sub)} is on this channel.`} Pick the model so the preview and the effects match your case.</p>`));
        const grid = h('<div class="cam-fanpick" role="radiogroup" aria-label="Fan model"></div>');
        const cards = FAN_MODELS.map((m) => {
          const b = h('<button type="button" role="radio"><canvas aria-hidden="true"></canvas><b></b><span></span></button>');
          b.lastElementChild.textContent = m.hint;
          b.addEventListener('click', () => { pick = Object.assign({}, pick, { model: m.value }); paint(); });
          grid.appendChild(b);
          return { b, m, cv: b.querySelector('canvas') };
        });
        grid.addEventListener('keydown', (e) => {
          const d = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 0;
          if (!d) return;
          e.preventDefault();
          const i = cards.findIndex((c) => c.m.value === pick.model);
          const nx = cards[(i + d + cards.length) % cards.length];
          nx.b.click();
          nx.b.focus();
        });
        body.appendChild(grid);
        const opts = h('<div class="cam-rows cam-fanopts"></div>');
        if (n === 1) opts.appendChild(row('Size', null, choice({ options: [{ value: 120, label: '120 mm' }, { value: 140, label: '140 mm' }], value: pick.size, label: 'Fan size', onChange: (v) => { pick = Object.assign({}, pick, { size: v }); paint(); } })));
        opts.appendChild(row('Color', null, choice({ options: [{ value: 'black', label: 'Black' }, { value: 'white', label: 'White' }], value: pick.color, label: 'Fan color', onChange: (v) => { pick = Object.assign({}, pick, { color: v }); paint(); } })));
        body.appendChild(opts);
        function paint() {
          for (const c of cards) {
            const sel = c.m.value === pick.model;
            c.b.setAttribute('aria-checked', String(sel));
            c.b.tabIndex = sel ? 0 : -1;
            c.b.children[1].textContent = c.m.value === 'plain' ? 'Without RGB' : `F${pick.size} ${c.m.label}`;
          }
        }
        paint();
        let time = 0, angle = 0;
        track(C.on('frame', (dt) => {
          time += dt;
          angle += K.spin((C.live(t.fan) || { rpm: 0 }).rpm) * dt * TAU * 0.5;
          for (const c of cards) {
            const f = Object.assign({}, pick, { model: c.m.value });
            const { ctx, w, h: hh } = K.fit(c.cv);
            ctx.clearRect(0, 0, w, hh);
            drawFan(ctx, w / 2, hh / 2, Math.min(w, hh) / 2 / 1.12, angle, fanLeds(t, f, time, 1)(0), f);
          }
        }));
      },
      actions: [{ label: 'Cancel' }, { label: 'Save', primary: true, onClick: () => { setFan(t.fan, pick); toast(`${t.label}: ${n > 1 ? n + ' × ' : ''}${fanName(pick)}.`); } }],
    });
  }

  // CAM's color row: one swatch per color, + Add a color up to what the effect takes.
  function colorSwatches(t, o = {}) {
    const sw = h('<div class="cam-swatches"></div>');
    sw.paint = () => {
      const x = t.get();
      const [mn, mx] = C.light.EFFECTS[x.mode].colors;
      sw.textContent = '';
      const cols = (x.colors || []).slice(0, mx);
      cols.forEach((c, i) => {
        const s = h(`<span class="cam-swatch" style="--c:#${esc(c)}"><input type="color" value="#${esc(c)}" aria-label="${esc(t.label)} color ${i + 1}, #${esc(c)}">${cols.length > mn ? `<button type="button" class="x" aria-label="Remove color ${i + 1}">×</button>` : ''}</span>`);
        const inp = s.querySelector('input');
        inp.addEventListener('input', () => { s.style.setProperty('--c', inp.value); editLight(t, (y) => { y.colors[i] = inp.value.slice(1); }); });
        const xb = s.querySelector('.x');
        if (xb) xb.addEventListener('click', (e) => { e.preventDefault(); editLight(t, (y) => { y.colors.splice(i, 1); }); sw.paint(); });
        sw.appendChild(s);
      });
      if (cols.length < mx) {
        const add = h(`<button type="button" class="${o.short ? 'cam-swatch-add' : 'cam-link'}">${o.short ? I.plus : '+ Add a color'}</button>`);
        if (o.short) add.setAttribute('aria-label', `Add a color to ${t.label}`);
        add.addEventListener('click', () => { editLight(t, (y) => { y.colors = (y.colors || []).slice(0, mx); y.colors.push(PALETTE.find((c) => !y.colors.includes(c)) || 'ffffff'); }); sw.paint(); });
        sw.appendChild(add);
      }
    };
    sw.paint();
    return sw;
  }

  function lightEditor(host, t) {
    const el = h(`<div class="cam-panel cam-light"><div class="cam-light-prev"><div style="width:100%;display:grid;justify-items:center"><canvas></canvas><p></p></div></div><div class="cam-light-ctl cam-rows"></div></div>`);
    const cv = el.querySelector('canvas');
    cv.setAttribute('role', 'img');
    el.querySelector('.cam-light-prev p').textContent = t.sub;
    const ctl = el.querySelector('.cam-light-ctl');
    const fx = select({ options: EFFECT_ORDER.map((k) => ({ value: k, label: EFFECT_NAME[k] })), value: t.get().mode, label: 'Effect', onChange: (m) => editLight(t, (x) => { x.mode = m; fitColors(x); }) });
    const rEffect = row('Effect', null, fx);
    const sw = colorSwatches(t);
    const rColors = row('Colors', null, sw);
    const sp = slider({ min: 0, max: 4, step: 1, value: Math.max(0, C.light.SPEEDS.indexOf(t.get().speed || 'normal')), label: 'Speed', format: (i) => SPEED_NAME[C.light.SPEEDS[i]], onInput: (i) => editLight(t, (x) => { x.speed = C.light.SPEEDS[i]; }) });
    const rSpeed = row('Speed', null, sp);
    const dir = toggle({ checked: !!t.get().backward, label: 'Invert Direction', onChange: (v) => editLight(t, (x) => { if (v) x.backward = true; else delete x.backward; }) });
    const rDir = row('Direction', null, [dir, h('<span class="cam-small">Invert Direction</span>')]);
    const fanLbl = h('<span class="cam-fanname"></span>');
    const rFans = t.kind === 'fans' ? row('Fans', '', [fanLbl, button('Change', { small: true, onClick: () => fanDialog(t) })]) : null;
    if (rFans) ctl.appendChild(rFans);
    ctl.append(rEffect, rColors, rSpeed, rDir);

    function refresh() {
      const x = t.get();
      const plain = !!rFans && fanSpec(t.fan).model === 'plain';
      fx.set(x.mode);
      rEffect.hidden = plain;
      rColors.hidden = plain || C.light.EFFECTS[x.mode].colors[1] === 0;
      rSpeed.hidden = plain || STILL.has(x.mode);
      rDir.hidden = plain || !DIRECTIONAL.has(x.mode);
      sp.set(Math.max(0, C.light.SPEEDS.indexOf(x.speed || 'normal')));
      dir.set(!!x.backward);
      if (rFans) {
        fanLbl.textContent = fanText(t);
        rFans.hint(!plain ? '' : fanCount(t) > 1 ? 'These fans have no LEDs, so this channel has nothing to light.' : 'This fan has no LEDs, so this channel has nothing to light.');
      }
      cv.setAttribute('aria-label', `${t.label}${rFans ? ', ' + fanText(t) : ''}: ${plain ? 'no LEDs' : C.light.describe(x)}`);
      sw.paint();
    }
    // Only effect changes rebuild the controls, so a color picker stays open while it is dragged.
    let lastMode = null;
    track(C.on('config', (what) => { const m = t.get().mode; if (what !== 'lighting' || m !== lastMode) { lastMode = m; refresh(); } }));
    track(on('prefs', (k) => { if (k === 'fans') refresh(); }));
    lastMode = t.get().mode;
    refresh();
    let time = 0;
    const angles = [0, 0.7, 1.4];
    track(C.on('frame', (dt) => {
      time += dt;
      const st = t.fan ? C.live(t.fan) : null;
      if (st) for (let i = 0; i < angles.length; i++) angles[i] += K.spin(st.rpm) * dt * TAU;
      drawLight(cv, t, time, angles);
    }));
    host.appendChild(el);
    return el;
  }

  /* ------------------------------------------------------------------- LCD */

  // Two sleeved tubes with their fittings, leaving the head at `deg` (0 = to the right).
  function drawTubes(ctx, cx, cy, r, deg) {
    ctx.save();
    ctx.translate(cx, cy);
    ctx.rotate((deg * Math.PI) / 180);
    const tw = r * 0.26, x0 = r * 0.8, len = r * 0.62;
    for (const dy of [-r * 0.33, r * 0.33]) {
      const g = ctx.createLinearGradient(x0, 0, x0 + len, 0);
      g.addColorStop(0, '#4a4d56'); g.addColorStop(0.55, 'rgba(74,77,86,.75)'); g.addColorStop(1, 'rgba(74,77,86,0)');
      ctx.fillStyle = g;
      ctx.fillRect(x0, dy - tw / 2, len, tw);
      ctx.fillStyle = '#5d616b';
      K.roundRect(ctx, r * 0.84, dy - tw * 0.62, r * 0.2, tw * 1.24, r * 0.04);
      ctx.fill();
    }
    ctx.restore();
  }
  // The head as installed: tubes where they leave the head, picture upright.
  function drawHeadInstalled(cv, o) {
    const { ctx, w, h: hh } = K.fit(cv);
    ctx.clearRect(0, 0, w, hh);
    const r = Math.min(w, hh) / 2 / 1.42;
    const cx = w / 2, cy = hh / 2;
    drawTubes(ctx, cx, cy, r, o.orientation);
    K.head(ctx, cx, cy, r, { leds: o.leds, liquid: C.state.liquid, lcd: Object.assign({}, C.config.lcd, { mode: o.mode || C.config.lcd.mode }), snapshot: C.snapshot(), font: FONT, rotation: 0, shadow: o.shadow, shadowColor: 'rgba(0,0,0,.4)' });
  }

  function lcdEditor(host) {
    const el = h(`<div class="cam-panel cam-lcd"><div class="cam-light-prev"><div style="width:100%;display:grid;justify-items:center"><canvas role="img"></canvas><p></p></div></div><div class="cam-light-ctl cam-rows"></div></div>`);
    const cv = el.querySelector('canvas'), cap = el.querySelector('.cam-light-prev p');
    const ctl = el.querySelector('.cam-light-ctl');
    const L = () => C.config.lcd;
    const mode = select({ options: [{ value: 'liquid', label: 'Liquid Temperature' }, { value: 'api', label: 'Local API' }], value: L().mode, label: 'Display Mode', onChange: (v) => { L().mode = v; C.changed('lcd'); } });
    const rMode = row('Display Mode', '', mode);
    const br = slider({ min: 0, max: 100, step: 5, value: L().brightness, label: 'Brightness', format: (v) => v + '%', onInput: (v) => { L().brightness = v; }, onChange: () => C.changed('lcd') });
    const rBr = row('Brightness', null, br);
    const rotVal = h('<span class="cam-small num"></span>');
    const rRot = row('Rotate Display', 'Match the tubes to your Kraken installation.', [rotVal, button('Rotate Display', { icon: 'rotate', small: true, onClick: rotateDialog })]);
    ctl.append(rMode, rBr, rRot);
    function refresh() {
      const l = L();
      mode.set(l.mode);
      br.set(l.brightness);
      rotVal.textContent = l.orientation + '°';
      const apiOff = !C.config.api;
      rMode.hint(l.mode === 'liquid'
        ? 'The cooler draws this screen itself, so it stays on when camless is closed.'
        : `A page on this PC, such as KrakenDial, draws on the screen through <code>${esc(C.config.api ? C.config.api.listen : '127.0.0.1:30641')}</code>, like CAM's Web Integration. If it stops for 5 seconds, the screen goes back to liquid temperature.${apiOff ? ' <b>Turn on Local API in Settings first.</b>' : ''}`);
      cap.textContent = l.mode === 'liquid' ? 'Liquid temperature, drawn by the cooler' : 'Frames from a page on this PC';
      cv.setAttribute('aria-label', `Screen preview: ${cap.textContent}, ${l.brightness}% brightness, rotated ${l.orientation}°`);
    }
    track(C.on('config', refresh));
    refresh();
    let time = 0;
    track(C.on('frame', (dt) => { time += dt; drawHeadInstalled(cv, { orientation: L().orientation, leds: C.light.render(C.config.ring, time, 24) }); }));
    host.appendChild(el);
    return el;
  }

  function rotateDialog() {
    let pick = C.config.lcd.orientation;
    modal({
      title: 'Kraken Screen Rotation',
      render(body) {
        body.appendChild(h('<p class="cam-hint" style="margin:0 0 14px">Match the tubes to your Kraken installation.</p>'));
        const grid = h('<div class="cam-rot" role="radiogroup" aria-label="Screen rotation"></div>');
        const cvs = [];
        for (const d of [0, 90, 180, 270]) {
          const b = h(`<button type="button" role="radio" aria-checked="${d === pick}"><canvas></canvas><span>${d}°</span></button>`);
          b.addEventListener('click', () => { pick = d; grid.querySelectorAll('button').forEach((x, i) => x.setAttribute('aria-checked', String([0, 90, 180, 270][i] === pick))); });
          grid.appendChild(b);
          cvs.push([b.querySelector('canvas'), d]);
        }
        body.appendChild(grid);
        const draw = () => { for (const [cv, d] of cvs) drawHeadInstalled(cv, { orientation: d, shadow: false, leds: C.light.render(C.config.ring, 0, 24) }); };
        requestAnimationFrame(draw);
        track(C.on('tick', draw));
      },
      actions: [{ label: 'Cancel' }, { label: 'Save', primary: true, onClick: () => { C.config.lcd.orientation = pick; C.changed('lcd'); toast(`Screen rotated to ${pick}°.`); } }],
    });
  }

  /* --------------------------------------------------------- lighting page */

  const lightState = { device: 'kraken', tab: { kraken: 'lcd', hub: 'hub1' } };
  function deviceThumb(cv, device, time) {
    const { ctx, w, h: hh } = K.fit(cv);
    ctx.clearRect(0, 0, w, hh);
    if (device === 'kraken') {
      K.head(ctx, w / 2, hh / 2, Math.min(w, hh) / 2 - 2, { leds: C.light.render(C.config.ring, time, 24), liquid: C.state.liquid, lcd: C.config.lcd, snapshot: C.snapshot(), font: FONT, shadow: false, rotation: 0 });
    } else drawHub(ctx, w, hh, time);
  }
  // The Control Hub: a dark box with a lit dot per connected fan port.
  function drawHub(ctx, w, hh) {
    const bw = Math.min(w * 0.92, hh * 1.4), bh = bw * 0.62;
    const x = (w - bw) / 2, y = (hh - bh) / 2;
    ctx.fillStyle = '#16171b';
    K.roundRect(ctx, x, y, bw, bh, bw * 0.08); ctx.fill();
    ctx.strokeStyle = 'rgba(255,255,255,.14)'; ctx.lineWidth = 1;
    K.roundRect(ctx, x + 0.5, y + 0.5, bw - 1, bh - 1, bw * 0.08); ctx.stroke();
    const used = new Set(C.config.hub_fan.map((f) => f.port));
    for (let p = 1; p <= DEV.hub.ports; p++) {
      const px = x + bw * (0.16 + (p - 1) * 0.17), py = y + bh * 0.62;
      ctx.fillStyle = '#25272c';
      K.roundRect(ctx, px - bw * 0.055, py - bh * 0.16, bw * 0.11, bh * 0.32, 2); ctx.fill();
      ctx.fillStyle = used.has(p) ? '#21d373' : '#3a3c42';
      ctx.beginPath(); ctx.arc(px, y + bh * 0.26, Math.max(1.5, bw * 0.025), 0, TAU); ctx.fill();
    }
  }

  // CAM's Lighting page: pick a device, then a channel, then its effect.
  function lightingPage(host, o = {}) {
    const devices = o.devices || ['kraken', 'hub'];
    if (!devices.includes(lightState.device)) lightState.device = devices[0];
    if (devices.length > 1) {
      const pick = h('<div class="cam-devpick" role="group" aria-label="Device"></div>');
      const thumbs = [];
      for (const d of devices) {
        const b = h(`<button type="button" aria-pressed="${d === lightState.device}"><canvas></canvas><span><b>${esc(DEV[d].name)}</b><span class="cam-status">Ready</span></span></button>`);
        b.addEventListener('click', () => { lightState.device = d; pick.querySelectorAll('button').forEach((x, i) => x.setAttribute('aria-pressed', String(devices[i] === d))); renderDevice(); });
        pick.appendChild(b);
        thumbs.push([b.querySelector('canvas'), d]);
      }
      host.appendChild(pick);
      let time = 0;
      track(C.on('frame', (dt) => { time += dt; for (const [cv, d] of thumbs) deviceThumb(cv, d, time); }));
    }
    const tabsHost = h('<div></div>');
    const body = h('<div></div>');
    host.append(tabsHost, body);
    const draw = region(body);
    function renderDevice() {
      const d = lightState.device;
      const targets = lightTargets(d);
      const tabs = (d === 'kraken' && o.lcd !== false ? [{ key: 'lcd', label: 'LCD Display' }] : []).concat(targets);
      if (!tabs.some((t) => t.key === lightState.tab[d])) lightState.tab[d] = tabs[0].key;
      tabsHost.textContent = '';
      tabsHost.appendChild(tabStrip(tabs.map((t) => ({ id: t.key, label: t.label })), lightState.tab[d], (k) => { lightState.tab[d] = k; showTab(); }, { sub: true, label: `${DEV[d].name} channels` }));
      showTab();
      function showTab() {
        const k = lightState.tab[d];
        draw((b) => { if (k === 'lcd') lcdEditor(b); else lightEditor(b, targets.find((t) => t.key === k)); });
      }
    }
    renderDevice();
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

  /* ------------------------------------------------------------ monitoring */

  const SERIES = [
    { key: 'cpu', label: 'CPU', dark: '#be9fff', light: '#640a92' },
    { key: 'gpu', label: 'GPU', dark: '#21d373', light: '#0f8a4c' },
    { key: 'liquid', label: 'Liquid', dark: '#4cc2ff', light: '#0a6fd6' },
  ];
  const seriesColor = (s) => (prefs.theme === 'light' ? s.light : s.dark);

  function gauge(label, max) {
    const R = 50, Ci = TAU * R, L = Ci * 0.75;
    const el = h(`<div class="cam-gauge"><svg viewBox="0 0 124 124" aria-hidden="true"><g transform="rotate(135 62 62)"><circle class="track" cx="62" cy="62" r="${R}" fill="none" stroke-width="8" stroke-linecap="round" stroke-dasharray="${L} ${Ci}"/><circle class="val" cx="62" cy="62" r="${R}" fill="none" stroke-width="8" stroke-linecap="round" stroke-dasharray="0 ${Ci}"/></g><text class="big" x="62" y="66" text-anchor="middle">--</text><text class="unit" x="62" y="84" text-anchor="middle"></text></svg><span></span></div>`);
    el.lastElementChild.textContent = label;
    const val = el.querySelector('.val'), big = el.querySelector('.big'), unit = el.querySelector('.unit');
    el.set = (frac, text, u, hot) => {
      val.setAttribute('stroke-dasharray', `${Math.max(0.01, L * clamp(frac, 0, 1))} ${Ci}`);
      big.textContent = text;
      unit.textContent = u;
      el.classList.toggle('hot', !!hot);
      el.setAttribute('aria-label', `${label} ${text}${u}`);
    };
    el.setAttribute('role', 'img');
    return el;
  }

  function monitoring(host) {
    host.appendChild(h('<svg width="0" height="0" style="position:absolute" aria-hidden="true"><defs><linearGradient id="cam-gauge-grad" x1="0" y1="1" x2="1" y2="0"><stop offset="0" stop-color="#7416e6"/><stop offset="1" stop-color="#bf4cbf"/></linearGradient></defs></svg>'));
    const grid = h('<div class="cam-mon"></div>');
    const tf = (c, lo, hi) => (c == null ? 0 : (c - lo) / (hi - lo));
    const cards = [
      { title: 'CPU', sub: DEV.cpu, g: [['Load', (s) => [s.cpu.load, Math.round(s.cpu.load * 100), '%']], ['Temperature', (s) => [tf(s.cpu.temp, 20, 100), s.cpu.temp == null ? 'N/A' : Math.round(toUnit(s.cpu.temp)), s.cpu.temp == null ? '' : deg()]]], stats: [['Power', (s) => Math.round(s.cpu.power) + ' W']] },
      { title: 'GPU', sub: DEV.gpu, g: [['Load', (s) => [s.gpu.load, Math.round(s.gpu.load * 100), '%']], ['Temperature', (s) => [tf(s.gpu.temp, 20, 100), s.gpu.temp == null ? 'N/A' : Math.round(toUnit(s.gpu.temp)), s.gpu.temp == null ? '' : deg()]]], stats: [['Power', (s) => Math.round(s.gpu.power) + ' W']] },
      { title: 'RAM', sub: `${DEV.ramGb} GB`, g: [['RAM Load', (s) => [s.ram.used / s.ram.total, Math.round((s.ram.used / s.ram.total) * 100), '%']]], stats: [['Used', (s) => s.ram.used.toFixed(1) + ' GB'], ['Free', (s) => (s.ram.total - s.ram.used).toFixed(1) + ' GB']] },
      { title: DEV.kraken.name, sub: DEV.kraken.model, g: [['Liquid', (s) => [tf(s.liquid, 20, 60), toUnit(s.liquid).toFixed(1), deg(), s.critical]], ['Pump', () => { const st = C.live('pump') || { shown: 0 }; return [st.shown / 100, Math.round(st.shown), '%']; }]], stats: [['Pump', () => rpm((C.live('pump') || {}).rpm) + ' RPM'], ['Fan', () => rpm((C.live('radiator') || {}).rpm) + ' RPM']] },
    ];
    const upd = [];
    for (const c of cards) {
      const el = h('<section class="cam-panel cam-moncard"><header><h3></h3><span></span></header><div class="cam-gauges"></div><div class="cam-monstats"></div></section>');
      el.querySelector('h3').textContent = c.title;
      el.querySelector('header span').textContent = c.sub;
      const gs = c.g.map(([label, f]) => { const g = gauge(label); el.querySelector('.cam-gauges').appendChild(g); return [g, f]; });
      const ss = c.stats.map(([label, f]) => { const s = h('<span></span>'); s.textContent = label; const b = h('<b></b>'); s.appendChild(b); el.querySelector('.cam-monstats').appendChild(s); return [b, f]; });
      upd.push((s) => { for (const [g, f] of gs) { const [fr, t, u, hot] = f(s); g.set(fr, String(t), u, hot); } for (const [b, f] of ss) b.textContent = f(s); });
      grid.appendChild(el);
    }
    host.appendChild(grid);
    const tick = () => { const s = C.snapshot(); for (const f of upd) f(s); };
    track(C.on('tick', tick));
    track(on('prefs', tick));
    tick();
    host.appendChild(tempChart());
  }

  function tempChart() {
    const el = h('<section class="cam-panel cam-chart"><header><h3>Temperatures</h3></header><canvas role="img" aria-label="CPU, GPU and liquid temperature over the last 10 minutes"></canvas></section>');
    const shown = { cpu: true, gpu: true, liquid: true };
    for (const s of SERIES) {
      const b = h(`<button type="button" class="cam-legend" aria-pressed="true"><i></i>${s.label}</button>`);
      b.addEventListener('click', () => { shown[s.key] = !shown[s.key]; b.setAttribute('aria-pressed', String(shown[s.key])); draw(); });
      el.firstElementChild.appendChild(b);
      const paint = () => b.firstElementChild.style.setProperty('--c', seriesColor(s));
      paint();
      track(on('prefs', paint));
    }
    el.firstElementChild.appendChild(h('<span class="cam-small">Last 10 minutes</span>'));
    const cv = el.querySelector('canvas');
    function draw() {
      if (!cv.isConnected) return;
      const { ctx, w, h: hh } = K.fit(cv);
      ctx.clearRect(0, 0, w, hh);
      const css = getComputedStyle(cv);
      const grid = css.getPropertyValue('--ce-grid').trim(), text = css.getPropertyValue('--t4').trim();
      const pl = 40, pr = 8, pt = 8, pb = 22;
      const hist = C.history.slice(-600);
      const lo = 20, hi = 100;
      const X = (i) => pl + (i / Math.max(1, 599)) * (w - pl - pr);
      const Y = (v) => pt + (1 - (v - lo) / (hi - lo)) * (hh - pt - pb);
      ctx.font = `500 10.5px ${FONT}`;
      ctx.textAlign = 'right';
      for (let v = 20; v <= 100; v += 20) {
        ctx.strokeStyle = grid; ctx.lineWidth = 1;
        ctx.beginPath(); ctx.moveTo(pl, Math.round(Y(v)) + 0.5); ctx.lineTo(w - pr, Math.round(Y(v)) + 0.5); ctx.stroke();
        ctx.fillStyle = text; ctx.fillText(Math.round(toUnit(v)) + '°', pl - 8, Y(v) + 3.5);
      }
      ctx.textAlign = 'center';
      for (let m = 10; m >= 0; m -= 2) ctx.fillText(m ? `-${m} min` : 'now', clamp(X(599 - m * 60), pl + 16, w - pr - 14), hh - 5);
      const off = 600 - hist.length;
      for (const s of SERIES) {
        if (!shown[s.key]) continue;
        ctx.strokeStyle = seriesColor(s);
        ctx.lineWidth = 1.8;
        ctx.lineJoin = 'round';
        ctx.beginPath();
        let pen = false;
        hist.forEach((p, i) => {
          const v = p[s.key];
          if (v == null) { pen = false; return; }
          const x = X(i + off), y = Y(clamp(v, lo, hi));
          if (pen) ctx.lineTo(x, y); else { ctx.moveTo(x, y); pen = true; }
        });
        ctx.stroke();
      }
    }
    track(C.on('tick', draw));
    track(on('prefs', draw));
    const ro = new ResizeObserver(draw);
    ro.observe(cv);
    track(() => ro.disconnect());
    requestAnimationFrame(draw);
    return el;
  }

  /* -------------------------------------------------------------- settings */

  function download(name, text) {
    const a = document.createElement('a');
    a.href = URL.createObjectURL(new Blob([text], { type: 'text/plain' }));
    a.download = name;
    document.body.appendChild(a);
    a.click();
    setTimeout(() => { URL.revokeObjectURL(a.href); a.remove(); }, 0);
  }

  function importCamDialog() {
    modal({
      title: 'Import from NZXT CAM',
      render(body) {
        body.appendChild(h(`<div><p class="cam-hint" style="margin:0 0 10px">camless reads the active profile from CAM's own files in <code>%APPDATA%\\NZXT CAM</code>: cooling curves, fan names, lighting for every channel and the LCD mode.</p><p class="cam-hint" style="margin:0">Your current camless settings are replaced. CAM itself is not changed, and it can stay installed.</p></div>`));
      },
      actions: [{ label: 'Cancel' }, { label: 'Import', primary: true, onClick: () => { C.config = C.defaults(); toast("Imported CAM's active profile: 6 cooling channels, 5 lighting channels and the LCD mode."); } }],
    });
  }

  function importFile() {
    const inp = h('<input type="file" accept=".toml,text/plain" hidden>');
    inp.addEventListener('change', async () => {
      const f = inp.files[0];
      inp.remove();
      if (!f) return;
      const r = C.toml.parse(await f.text());
      const errs = r.errors.concat(C.toml.validate(r.config));
      if (errs.length) { toast(`${f.name} was not imported: ${errs[0].message}${errs[0].line ? ` (line ${errs[0].line})` : ''}.`); return; }
      // camless reads lighting modes in any case, and without [ring], [external] or [lcd]
      // the Kraken keeps what it shows now.
      for (const l of [r.config.ring, r.config.external, ...r.config.hub_led]) if (l) l.mode = String(l.mode).toLowerCase();
      for (const k of ['ring', 'external', 'lcd']) r.config[k] ??= C.util.clone(C.config[k]);
      C.config = r.config;
      toast(`Imported ${f.name}.`);
    });
    document.body.appendChild(inp);
    inp.click();
  }

  // o.sections: which groups to show, in order.
  function settings(host, o = {}) {
    const want = o.sections || ['general', 'safety', 'api', 'mini', 'backup', 'firmware'];
    const panel = (title, rows) => { const s = section(host, title); const p = h('<div class="cam-panel cam-rows"></div>'); p.append(...rows); s.appendChild(p); return p; };
    const hotText = () => `The Kraken speeds up on its own from ${temp(45, 0)} liquid, even when camless is closed. At ${temp(55, 0)} the pump and every fan run at 100%. This is always on.`;
    const hot = row('Hot liquid', hotText(), h('<span class="cam-status">Always on</span>'));
    track(on('prefs', (k) => { if (k === 'unit') hot.hint(hotText()); }));
    const build = {
      general: () => panel('General', [
        row('Language', null, select({ options: [{ value: 'en', label: 'English' }], value: 'en', label: 'Language' })),
        row('Temperature Display', null, choice({ options: [{ value: 'c', label: 'Celsius' }, { value: 'f', label: 'Fahrenheit' }], value: prefs.unit, label: 'Temperature Display', onChange: (v) => setPref('unit', v) })),
        row('Minimize camless to tray when closing', 'Fans keep following their curves while camless runs in the tray.', toggle({ checked: prefs.tray, label: 'Minimize camless to tray when closing', onChange: (v) => setPref('tray', v) })),
        row('Start camless on Windows startup', null, toggle({ checked: prefs.startup, label: 'Start camless on Windows startup', onChange: (v) => setPref('startup', v) })),
        row('Start camless minimized when launching on Windows startup', null, toggle({ checked: prefs.startMin, label: 'Start camless minimized when launching on Windows startup', onChange: (v) => setPref('startMin', v) })),
        row('Enable Dark Mode', null, toggle({ checked: prefs.theme !== 'light', label: 'Enable Dark Mode', onChange: (v) => setPref('theme', v ? 'dark' : 'light') })),
      ]),
      safety: () => panel('Fan Safety', [
        row('If a temperature sensor is missing', 'Channels that follow the missing sensor hold this speed. camless also sets it when it closes.', slider({ min: 40, max: 100, step: 5, value: C.config.failsafe_duty, label: 'Failsafe speed', format: (v) => v + '%', onInput: (v) => { C.config.failsafe_duty = v; }, onChange: () => C.changed('failsafe') })),
        hot,
      ]),
      api: () => {
        const addr = h(`<code>${esc(C.config.api ? C.config.api.listen : '127.0.0.1:30641')}</code>`);
        return panel('Local API', [
          row('Allow pages on this PC to draw on the LCD', 'Pages such as KrakenDial read sensors and post frames to camless. Only this PC can reach it.', toggle({ checked: !!C.config.api, label: 'Local API', onChange: (v) => { if (v) C.config.api = { listen: '127.0.0.1:30641' }; else delete C.config.api; C.changed('api'); } })),
          row('Address', null, addr),
        ]);
      },
      mini: () => panel('Mini Mode', [
        row('Orientation', null, choice({ options: [{ value: 'vertical', label: 'Vertical' }, { value: 'horizontal', label: 'Horizontal' }], value: prefs.miniDir, label: 'Mini Mode orientation', onChange: (v) => setPref('miniDir', v) })),
        row('Always on Top', null, toggle({ checked: prefs.miniTop, label: 'Always on Top', onChange: (v) => setPref('miniTop', v) })),
        row('Mini Mode', 'A small window with temperatures and load.', button('Launch Mini Mode', { icon: 'mini', small: true, onClick: () => miniMode(true) })),
      ]),
      backup: () => panel('Settings Backup', [
        row('Import from NZXT CAM', "Bring over CAM's curves, fan names, lighting and LCD mode.", button('Import', { small: true, onClick: importCamDialog })),
        row('Export', 'Saves every setting as <code>camless.toml</code>.', button('Export', { small: true, onClick: () => { download('camless.toml', C.toml.stringify(C.config)); toast('Saved camless.toml.'); } })),
        row('Import', 'Load a <code>camless.toml</code> saved earlier.', button('Import File', { small: true, onClick: importFile })),
      ]),
      firmware: () => panel('NZXT Firmware', [
        row(DEV.kraken.name, `Firmware ${DEV.kraken.firmware}`, h('<span class="cam-status">Ready</span>')),
        row(DEV.hub.name, `Firmware ${DEV.hub.firmware}`, h('<span class="cam-status">Ready</span>')),
        row('Updates', 'camless shows firmware versions but does not install updates. To update, open NZXT CAM once.', null),
      ]),
    };
    for (const k of want) build[k]();
  }

  /* ------------------------------------------------------------- mini mode */

  let mini = null;
  const MINI_READS = [
    ['CPU Temp', (s) => temp(s.cpu.temp)],
    ['GPU Temp', (s) => temp(s.gpu.temp)],
    ['Liquid', (s) => temp(s.liquid), (s) => s.critical],
    ['CPU Load', (s) => Math.round(s.cpu.load * 100) + '%'],
    ['GPU Load', (s) => Math.round(s.gpu.load * 100) + '%'],
  ];
  function miniMode(force) {
    const want = force != null ? force : !mini;
    const tb = root && root.querySelector('[data-act=mini]');
    if (!want) { if (mini) mini.close(); return; }
    if (mini) return;
    const el = h(`<div class="cam-mini ${prefs.miniDir === 'horizontal' ? 'horizontal' : ''}" role="dialog" aria-label="Mini Mode"><header><svg class="logo" viewBox="0 0 32 32" aria-hidden="true"><circle cx="16" cy="16" r="12" fill="none" stroke="#bf4cbf" stroke-width="4.5" stroke-dasharray="3.3 1.4"/></svg><b>Mini Mode</b><button type="button" data-a="dir" aria-label="Switch orientation" title="Switch orientation">${I.swap}</button><button type="button" data-a="close" aria-label="Hide Mini Mode" title="Hide Mini Mode">${I.close}</button></header><div class="reads"></div></div>`);
    const reads = el.querySelector('.reads');
    const rows = MINI_READS.map(([k]) => { const r = h('<div class="read"><span></span><b class="num"></b></div>'); r.firstElementChild.textContent = k; reads.appendChild(r); return r.lastElementChild; });
    const upd = () => { const s = C.snapshot(); MINI_READS.forEach(([, f, hot], i) => { rows[i].textContent = f(s); rows[i].classList.toggle('hot', !!(hot && hot(s))); }); };
    const offs = [C.on('tick', upd), on('prefs', (k) => { upd(); if (k === 'miniDir') el.classList.toggle('horizontal', prefs.miniDir === 'horizontal'); })];
    upd();
    el.querySelector('[data-a=dir]').addEventListener('click', () => setPref('miniDir', prefs.miniDir === 'horizontal' ? 'vertical' : 'horizontal'));
    el.querySelector('[data-a=close]').addEventListener('click', () => mini.close());
    // Drag by the header, kept inside the window.
    const head = el.firstElementChild;
    let drag = null;
    head.addEventListener('pointerdown', (e) => {
      if (e.target.closest('button')) return;
      const r = el.getBoundingClientRect();
      drag = { dx: e.clientX - r.left, dy: e.clientY - r.top };
      head.setPointerCapture(e.pointerId);
    });
    head.addEventListener('pointermove', (e) => {
      if (!drag) return;
      const r = el.getBoundingClientRect(), v = viewBox();
      el.style.left = clamp(e.clientX - drag.dx, v.left + 4, v.right - r.width - 4) - v.ox + 'px';
      el.style.top = clamp(e.clientY - drag.dy, v.top + 4, v.bottom - r.height - 4) - v.oy + 'px';
      el.style.right = 'auto'; el.style.bottom = 'auto';
    });
    head.addEventListener('pointerup', () => { drag = null; });
    layer().appendChild(el);
    if (tb) tb.setAttribute('aria-pressed', 'true');
    mini = { el, close() { offs.forEach((f) => f()); el.remove(); mini = null; if (tb) tb.setAttribute('aria-pressed', 'false'); } };
    el.querySelector('[data-a=close]').focus({ preventScroll: true });
  }

  window.CamKit = {
    I, h, esc, prefs, setPref, on, temp, tnum, deg, toUnit, rpm, track, region,
    select, choice, slider, toggle, button, row, modal, toast, tabStrip,
    shell, pageHead, section, tiles,
    chName, chSub, chTile, channel, midSentence, model, setPreset, setSource, setAllPresets, allPreset, curveOpts, curveEdited, PRESET_OPTS, SOURCE_OPTS, PRESET_LABEL, whereText,
    coolingCard, deviceChannels, inactiveRow, applyDialog,
    EFFECT_ORDER, EFFECT_NAME, SPEED_NAME, DIRECTIONAL, lightTargets, editLight, fitColors, colorSwatches, lightEditor, lightingPage, lcdEditor, rotateDialog, drawHeadInstalled, deviceThumb, drawHub,
    FAN_MODELS, fanSpec, setFan, fanName, fanText, drawFan, lightThumb, fanDialog, viewBox,
    monitoring, tempChart, settings, miniMode, importCamDialog,
    get root() { return root; },
  };
})();
