/* Floating demo controls for the layout pages: move between layouts and drive the
   simulated workload. Not part of any design. */
(function () {
  'use strict';
  // [folder, name, short label shown in the bar]
  const CONCEPTS = [
    ['cam-1-classic', 'CAM layout', 'CAM 1'],
    ['cam-2-devices', 'By device', 'CAM 2'],
    ['cam-3-simple', 'One page', 'CAM 3'],
  ];
  const script = document.currentScript;
  const pos = (script && script.dataset.position) || 'bottom-left';
  const theme = (script && script.dataset.theme) || 'dark';
  const here = location.pathname.split('/').filter(Boolean).slice(-2, -1)[0] || '';
  const idx = CONCEPTS.findIndex((c) => c[0] === here);

  const style = document.createElement('style');
  style.textContent = `
  .cl-demo{position:fixed;z-index:2147483000;display:flex;align-items:center;gap:6px;padding:5px;border-radius:12px;
    font:500 12px/1 system-ui,-apple-system,"Segoe UI",sans-serif;letter-spacing:0;backdrop-filter:blur(14px) saturate(1.4);
    -webkit-backdrop-filter:blur(14px) saturate(1.4);transition:opacity .2s,transform .25s cubic-bezier(.2,.8,.2,1);user-select:none}
  .cl-demo[data-theme=dark]{background:rgba(24,24,28,.72);color:#e9e9ee;box-shadow:0 6px 24px rgba(0,0,0,.35),inset 0 0 0 1px rgba(255,255,255,.08)}
  .cl-demo[data-theme=light]{background:rgba(255,255,255,.78);color:#24242a;box-shadow:0 6px 24px rgba(0,0,0,.12),inset 0 0 0 1px rgba(0,0,0,.08)}
  .cl-demo.bottom-left{left:12px;bottom:12px}.cl-demo.bottom-right{right:12px;bottom:12px}
  .cl-demo.top-right{right:12px;top:12px}.cl-demo.top-left{left:12px;top:12px}
  .cl-demo.bottom-center{left:50%;bottom:12px;transform:translateX(-50%)}
  .cl-demo.top-center{left:50%;top:9px;transform:translateX(-50%)}
  .cl-demo.top-center .cl-wide.cl-muted{display:none}
  @media (max-width:1400px){.cl-demo.top-center .cl-wide{display:none}}
  .cl-demo a,.cl-demo button{all:unset;cursor:pointer;padding:6px 8px;border-radius:8px;white-space:nowrap}
  .cl-demo a:hover,.cl-demo button:hover{background:rgba(127,127,140,.18)}
  .cl-demo a:focus-visible,.cl-demo button:focus-visible{outline:2px solid #6aa1ff;outline-offset:1px}
  .cl-demo .cl-sep{width:1px;align-self:stretch;margin:3px 2px;background:currentColor;opacity:.15}
  .cl-demo .cl-seg{display:flex;gap:2px}
  .cl-demo .cl-seg button[aria-pressed=true]{background:rgba(127,127,140,.28)}
  .cl-demo .cl-muted{opacity:.6}
  .cl-demo .cl-on{color:#ff9d6c}
  .cl-demo .cl-body{display:contents}
  .cl-demo[data-collapsed=true] .cl-body{display:none}
  .cl-demo .cl-name{font-weight:650;white-space:nowrap}
  .cl-demo .cl-size{min-width:74px;white-space:nowrap;font-variant-numeric:tabular-nums}
  @media (max-width:1180px){.cl-demo .cl-winctl{display:none}}
  html.cl-win body{display:grid;grid-template:minmax(0,1fr)/minmax(0,1fr);place-items:center;padding:16px 16px 60px;background:#2a2c31}
  html.cl-win .cam{width:var(--cl-w);height:var(--cl-h);max-width:100%;max-height:100%;min-width:600px;min-height:420px;resize:both;overflow:hidden;contain:layout paint;border-radius:8px;box-shadow:0 0 0 1px rgba(255,255,255,.1),0 24px 70px rgba(0,0,0,.5)}
  @media (max-width:760px){.cl-demo .cl-wide,.cl-demo .cl-full{display:none}
    .cl-demo.cl-demo{top:auto;bottom:8px;left:8px;right:auto;transform:none;gap:3px;max-width:calc(100vw - 16px)}
    .cl-demo a,.cl-demo button{padding:6px 7px}}
  `;
  document.head.appendChild(style);

  const bar = document.createElement('div');
  bar.className = 'cl-demo ' + pos;
  bar.dataset.theme = theme;
  bar.setAttribute('role', 'toolbar');
  bar.setAttribute('aria-label', 'Demo controls');
  // The cam-* pages can also be shown in a window of a set size, since camless is a small app.
  const app = document.querySelector('.cam');
  const SIZES = [['fill', 'Fill', 'Fill the browser'], ['1280x800', '1280', '1280 × 800 window'], ['1024x640', '1024', '1024 × 640 window'], ['800x560', '800', '800 × 560 window']];
  const prev = CONCEPTS[(idx + CONCEPTS.length - 1) % CONCEPTS.length];
  const next = CONCEPTS[(idx + 1) % CONCEPTS.length];
  bar.innerHTML = `
    <button class="cl-tg" title="Show or hide demo controls" aria-label="Demo controls">◐</button>
    <div class="cl-body">
      <a href="../index.html" title="All layouts" class="cl-muted">All</a>
      ${idx >= 0 ? `<a href="../${prev[0]}/index.html" title="${prev[1]}" aria-label="Previous layout">‹</a>
      <span class="cl-name">${CONCEPTS[idx][2]}<span class="cl-full"> ${CONCEPTS[idx][1]}</span></span>
      <a href="../${next[0]}/index.html" title="${next[1]}" aria-label="Next layout">›</a>` : ''}
      <span class="cl-sep"></span>
      <span class="cl-muted cl-wide">Load</span>
      <div class="cl-seg" data-k="load">
        <button data-v="auto">Auto</button><button data-v="idle">Idle</button><button data-v="gaming">Game</button><button data-v="render">Render</button>
      </div>
      <span class="cl-sep cl-wide"></span>
      <div class="cl-seg cl-wide" data-k="fault">
        <button data-v="cpu" title="Simulate a missing CPU temperature sensor">No CPU sensor</button>
        <button data-v="hot" title="Simulate liquid above 55 °C">Hot liquid</button>
      </div>
      ${app ? `<span class="cl-sep cl-wide cl-winctl"></span>
      <span class="cl-muted cl-wide cl-winctl">Window</span>
      <div class="cl-seg cl-wide cl-winctl" data-k="win">${SIZES.map(([v, l, t]) => `<button data-v="${v}" title="${t}">${l}</button>`).join('')}</div>
      <span class="cl-muted cl-wide cl-winctl cl-size" title="Drag the window's corner to resize it"></span>` : ''}
    </div>`;
  document.body.appendChild(bar);

  const C = window.Camless;
  const sync = () => {
    bar.querySelectorAll('[data-k=load] button').forEach((b) => b.setAttribute('aria-pressed', String(C.state.scenarioMode === b.dataset.v)));
    bar.querySelectorAll('[data-k=fault] button').forEach((b) => { const on = C.state.faults[b.dataset.v]; b.setAttribute('aria-pressed', String(on)); b.classList.toggle('cl-on', on); });
  };
  bar.querySelector('[data-k=load]').addEventListener('click', (e) => { const b = e.target.closest('button'); if (b) { C.setScenario(b.dataset.v); sync(); } });
  bar.querySelector('[data-k=fault]').addEventListener('click', (e) => { const b = e.target.closest('button'); if (b) { C.setFault(b.dataset.v, !C.state.faults[b.dataset.v]); sync(); } });
  // Storage can be off (private windows, blocked cookies); the bar then just forgets.
  const store = {
    get: (k) => { try { return localStorage.getItem(k); } catch (e) { return null; } },
    set: (k, v) => { try { localStorage.setItem(k, v); } catch (e) { /* not kept */ } },
  };
  const key = 'camless-demo-collapsed';
  // On a phone the open bar would cover controls at the bottom, so it starts folded.
  const saved = store.get(key);
  bar.dataset.collapsed = String(saved ? saved === '1' : matchMedia('(max-width: 760px)').matches);
  bar.querySelector('.cl-tg').addEventListener('click', () => {
    const c = bar.dataset.collapsed !== 'true';
    bar.dataset.collapsed = String(c);
    store.set(key, c ? '1' : '0');
  });
  if (app) {
    const wkey = 'camless-demo-window';
    const size = bar.querySelector('.cl-size');
    // A small screen has no room for a window, so the app fills it.
    const roomy = matchMedia('(min-width: 1181px)');
    let cur = 'fill';
    const setWin = (v) => {
      cur = v;
      app.style.width = app.style.height = '';
      const [w, h] = v.split('x').map(Number);
      document.documentElement.classList.toggle('cl-win', !!h && roomy.matches);
      if (h) { app.style.setProperty('--cl-w', w + 'px'); app.style.setProperty('--cl-h', h + 'px'); }
      bar.querySelectorAll('[data-k=win] button').forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.v === v)));
      store.set(wkey, v);
    };
    bar.querySelector('[data-k=win]').addEventListener('click', (e) => { const b = e.target.closest('button'); if (b) setWin(b.dataset.v); });
    new ResizeObserver(() => { const r = app.getBoundingClientRect(); size.textContent = `${Math.round(r.width)} × ${Math.round(r.height)}`; }).observe(app);
    roomy.addEventListener('change', () => setWin(cur));
    // camless is a small app, so it opens as a window unless a size was picked before.
    const was = store.get(wkey);
    setWin(SIZES.some((x) => x[0] === was) ? was : '1024x640');
  }
  C.on('scenario', sync);
  C.on('fault', sync);
  sync();
  window.CamlessConcepts = CONCEPTS;
})();
