/* camless's window: Monitoring, Games, Cooling, Lighting and Settings in NZXT CAM's
   layout, built on the data in store.js and the controls in kit.js. */
(function () {
  'use strict';
  const S = window.S, K = window.K, D = window.KrakenDraw;
  const { h, esc, I, track, row, select, choice, slider, toggle, button, modal, toast } = K;
  const { clamp, clone } = S.util;
  const TAU = Math.PI * 2;
  const FONT = K.FONT;

  /* --------------------------------------------------------------- helpers */

  const n1 = (v) => String(Math.round(v * 10) / 10);
  const listText = (xs) => (xs.length < 2 ? xs[0] || '' : `${xs.slice(0, -1).join(', ')} and ${xs[xs.length - 1]}`);
  const usbId = (u) => `${u.vid.toString(16).padStart(4, '0')}:${u.pid.toString(16).padStart(4, '0')}`;
  const failsafe = () => (S.config ? S.config.failsafe_duty : 70);
  const devName = (id) => { const d = S.device(id); return d ? d.name : id; };
  const hasSetup = (c) => !!c && ['curve', 'fan', 'light', 'lcd'].some((k) => (c[k] || []).length);
  const cap = (s) => (s ? s[0].toUpperCase() + s.slice(1) : s);

  // A sensor value with its unit, temperatures in the unit picked in Settings.
  function reading(id, v, dp = 0) {
    if (v == null) return 'N/A';
    const u = S.unitOf(id);
    if (u === 'celsius') return K.temp(v, dp);
    if (u === 'percent') return `${Math.round(v)}%`;
    return `${Math.round(v)} W`;
  }
  function axisText(id, t) {
    const u = S.unitOf(id);
    if (u === 'celsius') return `${Math.round(K.toUnit(t))}°`;
    if (u === 'percent') return `${Math.round(t)}%`;
    return `${Math.round(t)} W`;
  }
  // A fan's name, with its device's when there is more than one device.
  function fanLabel(id) {
    const many = new Set(S.fans().map((f) => f.dev)).size > 1;
    return many ? `${S.fanName(id)} (${devName(S.devOf(id))})` : S.fanName(id);
  }

  // Runs an app action; a refusal shows as a toast.
  async function act(path, body) {
    try { return await S.action(path, body); } catch (e) { toast(e.message); return null; }
  }

  function input(o) {
    const el = h(`<input class="cam-input" type="${o.type || 'text'}" spellcheck="false" autocomplete="off">`);
    if (o.value != null) el.value = o.value;
    if (o.placeholder) el.placeholder = o.placeholder;
    if (o.label) el.setAttribute('aria-label', o.label);
    for (const k of ['min', 'max', 'step']) if (o[k] != null) el.setAttribute(k, o[k]);
    if (o.width) el.style.width = o.width;
    if (o.onChange) el.addEventListener('change', () => o.onChange(el.value, el));
    el.addEventListener('keydown', (e) => {
      if (e.key !== 'Enter') return;
      e.preventDefault();
      if (o.onEnter) o.onEnter(el.value); else el.blur();
    });
    return el;
  }

  // A number with its unit; o.get() and o.set(v). Values outside min..max are clamped.
  function numField(o) {
    const wrap = h('<span class="cam-num"></span>');
    const inp = input({
      type: 'number', value: n1(o.get()), label: o.label, min: o.min, max: o.max, step: o.step || 'any',
      onChange: (v, el) => {
        const x = parseFloat(v);
        if (!Number.isFinite(x)) { el.value = n1(o.get()); return; }
        const y = Math.round(clamp(x, o.min ?? -Infinity, o.max ?? Infinity) * 10) / 10;
        el.value = n1(y);
        o.set(y);
      },
    });
    wrap.appendChild(inp);
    if (o.unit) { const u = h('<span class="cam-small"></span>'); u.textContent = o.unit; wrap.appendChild(u); }
    wrap.input = inp;
    wrap.set = (v) => { if (document.activeElement !== inp) inp.value = n1(v); };
    return wrap;
  }

  // Asks for a name. Resolves to it, trimmed, or null.
  function ask(o) {
    return new Promise((resolve) => {
      let done = false, inp = null, err = null;
      const submit = (api) => {
        const v = inp.value.trim();
        const e = o.validate ? o.validate(v) : v ? '' : 'Enter a name.';
        if (e) { err.textContent = e; inp.focus(); return false; }
        done = true;
        resolve(v);
        if (api) api.close();
        return true;
      };
      modal({
        title: o.title,
        render(body, api) {
          if (o.text) body.appendChild(h(`<p class="cam-hint cam-lead">${o.text}</p>`));
          const lab = h('<label class="cam-field"><span></span></label>');
          lab.firstElementChild.textContent = o.label;
          inp = input({ value: o.value || '', onEnter: () => submit(api) });
          inp.addEventListener('input', () => { err.textContent = ''; });
          lab.appendChild(inp);
          err = h('<p class="cam-field-err" role="alert"></p>');
          body.append(lab, err);
          if (o.extra) o.extra(body);
        },
        actions: [{ label: 'Cancel' }, { label: o.ok || 'Save', primary: true, onClick: () => submit(null) }],
        onClose: () => { if (!done) resolve(null); },
        ready: () => inp.select(),
      });
    });
  }

  function confirmBox(o) {
    return new Promise((resolve) => {
      let done = false;
      modal({
        title: o.title,
        render(body) { body.appendChild(h(`<p class="cam-hint cam-lead">${o.text}</p>`)); },
        actions: [{ label: 'Cancel' }, { label: o.ok, primary: true, onClick: () => { done = true; resolve(true); } }],
        onClose: () => { if (!done) resolve(false); },
      });
    });
  }

  // `camless.exe report`, run by the app; it stops waiting once `alive()` is false.
  async function gatherReport(alive) {
    const failed = (e) => ({ error: e.message });
    let r = await S.api('POST', 'report', {}).catch(failed);
    while (alive() && r.busy) {
      await new Promise((ok) => setTimeout(ok, 400));
      r = await S.api('GET', 'report').catch(failed);
    }
    return r;
  }

  /* The devices camless drives that hardware/verified.toml does not list. The sidebar
     asks how they work until the person says they submitted a report about this setup. */
  const unverified = () => S.devices().filter((d) => !d.verified);
  // Experimental: unverified, and no other app drives it the way camless does.
  const verifyTitle = (list) => (list.some((d) => d.experimental) ? 'Experimental hardware' : 'Unverified hardware');
  const EXPERIMENTAL = 'Experimental: no other app drives this model the way camless does, and its driver follows liquidctl alone.';
  const setupKey = (list) => list.map((d) => `${d.id}@${d.firmware || ''}`).sort().join(',');
  const UNVERIFIED = 'We’ve detected that your hardware configuration hasn’t been verified yet.';

  // Either report goes straight to a new GitHub issue with the report filled in; the
  // person checks it there before submitting, as the form asks.
  function verifyDialog() {
    const list = unverified();
    let closed = false;
    const note = h('<p class="cam-hint" aria-live="polite"></p>');
    const m = modal({
      title: verifyTitle(list),
      render(body) {
        body.appendChild(h(`<p class="cam-lead"><b>${UNVERIFIED}</b></p>`));
        const rows = h('<div class="cam-rows cam-box"></div>');
        for (const d of list) rows.appendChild(row(d.name, `Firmware ${esc(d.firmware || 'unknown')}${d.experimental ? '. Experimental' : ''}`, null));
        body.appendChild(rows);
        const exp = list.some((d) => d.experimental) ? ' An experimental driver goes further: no other app drives that model the way camless does, and the driver follows liquidctl alone.' : '';
        body.appendChild(h(`<p class="cam-hint">camless drives ${list.length > 1 ? 'these devices' : 'this device'} from other projects’ notes and captures of CAM, not from a test on the same hardware, so something may be off.${exp} Tell us how it goes. Either button gathers a hardware report and opens a GitHub issue with it filled in. You check it there before you submit; nothing is sent from here.</p>`));
        body.appendChild(note);
      },
      actions: [
        { label: 'Cancel' },
        { label: 'Something is wrong', onClick: () => { send('problem'); return false; } },
        { label: 'It works', primary: true, onClick: () => { send('works'); return false; } },
        { label: 'I submitted it', primary: true, onClick: submitted },
      ],
      onClose: () => { closed = true; },
    });
    const btns = m.buttons.slice(1, 3), done = m.buttons[3];
    done.hidden = true;
    const fail = (html) => { note.innerHTML = html; btns.forEach((b) => { b.disabled = false; }); };
    async function send(kind) {
      btns.forEach((b) => { b.disabled = true; });
      note.textContent = 'Gathering the report…';
      const r = await gatherReport(() => !closed);
      if (closed) return;
      if (r.error || !r.text) return fail(`<span class="cam-err">The report could not be made: ${esc(r.error || 'camless.exe printed nothing')}</span>`);
      if (!r.fits) {
        try { await navigator.clipboard.writeText(r.text); } catch (e) { return fail(`<span class="cam-err">The report could not be copied: ${esc(e.message)}</span>`); }
      }
      const o = await act('report/issue', { kind });
      if (!o) return fail('');
      // Opening the browser sends nothing: the issue exists only once it is submitted.
      btns.forEach((b) => { b.hidden = true; });
      done.hidden = false;
      done.focus();
      note.textContent = `${o.filled ? 'GitHub opened with the report filled in. Check it there, then submit it.' : 'GitHub opened. Paste the report from your clipboard into the Report field, then submit it.'} Once it is submitted, press I submitted it and the sidebar stops asking about this setup.`;
    }
    function submitted() {
      K.setPref('verifyReported', [...new Set([...(K.prefs.verifyReported || []), setupKey(list)])]);
      toast('Thanks. The sidebar stops asking about this setup.');
    }
  }

  // A hardware report for a GitHub issue. The app gathers it and shows all of it; Open on
  // GitHub fills in a new issue, which reaches GitHub only once the person submits it there.
  function reportDialog() {
    let report = null, closed = false;
    const pre = h('<pre class="cam-report" tabindex="0">Gathering the report…</pre>');
    const note = h('<p class="cam-hint"></p>');
    const m = modal({
      title: 'Hardware report',
      wide: true,
      render(body) {
        body.appendChild(h('<p class="cam-hint cam-lead">For a device camless does not drive yet, or drives wrong. The report is read only: standard USB descriptor reads, no commands sent to the devices. It leaves out serial numbers, device paths, and your user and PC names. It goes into a public GitHub issue, and only once you submit it there.</p>'));
        body.append(pre, note);
      },
      actions: [
        { label: 'Cancel' },
        { label: 'Copy', onClick: () => { copy(); return false; } },
        { label: 'Open on GitHub', primary: true, onClick: () => { openIssue(); return false; } },
      ],
      onClose: () => { closed = true; },
    });
    const [, copyBtn, openBtn] = m.buttons;
    copyBtn.disabled = openBtn.disabled = true;

    (async () => {
      const r = await gatherReport(() => !closed);
      if (closed) return;
      if (r.error || !r.text) {
        pre.textContent = 'The report could not be made.';
        note.innerHTML = `<span class="cam-err">${esc(r.error || 'camless.exe printed nothing')}</span>`;
        return;
      }
      report = r;
      pre.textContent = r.text;
      copyBtn.disabled = openBtn.disabled = false;
      note.textContent = r.fits
        ? 'Open on GitHub fills in a new issue with this report. Check it there, then submit.'
        : 'This report is too long for a link. Open on GitHub copies it to your clipboard, and you paste it into the issue.';
    })();

    async function copy() {
      try { await navigator.clipboard.writeText(report.text); toast('Report copied.'); } catch (e) { toast(`The report could not be copied: ${e.message}`); }
    }

    async function openIssue() {
      if (!report.fits) {
        try { await navigator.clipboard.writeText(report.text); } catch (e) { toast(`The report could not be copied: ${e.message}`); return; }
      }
      const r = await act('report/issue', {});
      if (!r) return;
      const ids = ((S.status && S.status.unsupported) || []).map(usbId);
      K.setPref('reported', [...new Set([...(K.prefs.reported || []), ...ids])]);
      m.close();
      toast(r.filled ? 'GitHub opened with the report filled in. Check it, then submit.' : 'GitHub opened. Paste the report from your clipboard into the Report field.');
    }
  }

  /* ----------------------------------------------------------------- shell */

  // Games is experimental: in the bar only once it is turned on in Settings.
  const gamesOn = () => !!(S.config && S.config.games && S.config.games.enabled);
  const NAV = [
    { id: 'monitoring', label: 'Monitoring', icon: 'monitoring' },
    { id: 'cooling', label: 'Cooling', icon: 'cooling' },
    { id: 'lighting', label: 'Lighting', icon: 'lighting' },
    { id: 'games', label: 'Games', icon: 'games', shown: gamesOn },
    { spacer: true },
    { id: 'settings', label: 'Settings', icon: 'settings' },
  ];
  const navShown = (id) => { const n = NAV.find((x) => x.id === id); return !!n && (!n.shown || n.shown()); };
  const PAGES = { monitoring, games, cooling, lighting, settings };
  let root = null, main = null, content = null, dispose = null, page = null, built = null;
  // The session open on the Games page, by its start, or 'now' for the game being
  // played. It stays open while the page is built again, until another page opens.
  let gameOpen = null;

  /* What the pages are built from. When it changes, say after a profile switch or
     once the service answers, the page is built again. */
  function layoutKey() {
    if (!S.server) return 'loading';
    return JSON.stringify([
      !!S.config,
      S.devices().map((d) => [d.id, d.connected, !!d.lcd, !!(d.lcd && d.lcd.frames)]),
      S.fans().map((f) => f.id + (f.hw && f.hw.connected === false && !S.fanCfg(f.id) ? '-' : '')),
      S.lights().map((l) => l.id),
      S.screens().map((s) => s.id),
      gamesOn(),
    ]);
  }
  // Not while something is dragged or a dialog or menu is open.
  const busy = () => !!document.querySelector('.ce-dragging, .cam-backdrop, .cam-menu');

  function boot() {
    const light = K.prefs.theme === 'light';
    document.body.classList.toggle('light', light);
    root = h(`<div class="cam" data-theme="${light ? 'light' : 'dark'}"><div class="cam-body"><nav class="cam-nav" aria-label="Main"></nav><main class="cam-main"><div class="cam-content"></div></main></div></div>`);
    document.body.prepend(root);
    K.setRoot(root);
    const nav = root.querySelector('.cam-nav');
    main = root.querySelector('.cam-main');
    content = root.querySelector('.cam-content');
    nav.innerHTML = `<div class="cam-logo">${K.LOGO}<span>camless</span></div>` + NAV.map((n) => (n.spacer ? '<div class="spacer"></div>'
      : `<a href="#${n.id}" data-id="${n.id}" title="${esc(n.label)}">${I[n.icon]}<span class="lbl">${esc(n.label)}</span></a>`)).join('');
    nav.appendChild(serviceBadge());
    nav.appendChild(updateBadge());
    nav.appendChild(verifyBadge());
    const navItems = () => nav.querySelectorAll('a[data-id]').forEach((a) => { a.hidden = !navShown(a.dataset.id); });
    S.on('config', navItems);
    navItems();
    K.on('prefs', (k) => {
      if (k !== 'theme') return;
      root.dataset.theme = K.prefs.theme;
      document.body.classList.toggle('light', K.prefs.theme === 'light');
    });
    const maybe = () => { if (layoutKey() !== built && !busy()) go(); };
    S.on('state', maybe);
    S.on('config', maybe);
    addEventListener('hashchange', go);
    go();
  }

  // The service at the bottom of the bar, always in view: whether it runs, with
  // administrator rights or not, and a button to start or restart it.
  function serviceBadge() {
    const el = h(`<div class="cam-svc"><button type="button" class="st"><i class="dot"></i><span class="lbl"><b>Service</b><small></small></span></button><button type="button" class="act cam-iconbtn"></button></div>`);
    const st = el.firstElementChild, act_ = el.lastElementChild, small = st.querySelector('small');
    st.addEventListener('click', () => { location.hash = 'settings'; });
    act_.addEventListener('click', () => act(act_.dataset.path, {}));
    const upd = () => {
      const sv = S.server;
      if (!sv) return;
      const s = S.status, info = s && s.service, busy = sv.service_busy;
      const apiOff = !s && S.config && S.config.api && S.config.api.enabled === false;
      const [text, tone, tip] = busy ? [`${cap(busy)}…`, 'busy', `The camless service is ${busy}.`]
        : apiOff ? ['Not visible', 'off', 'Local API is off, so this window cannot see the service.']
        : !s ? ['Not running', 'bad', 'The camless service is not running. Curves that camless sets are not applied.']
        : info && !info.elevated ? ['Not as admin', 'warn', 'Running without administrator rights, so CPU temperature and power are off. Restart it and answer Yes when Windows asks.']
        : info && info.cpu_sensor_error ? ['CPU temp off', 'warn', `Running, but CPU temperature is off: ${info.cpu_sensor_error}`]
        : ['Running', 'ok', info ? 'Running as administrator, with CPU temperature and power.' : 'Running.'];
      small.textContent = text;
      el.dataset.tone = tone;
      st.title = tip + ' Open Settings for more.';
      const path = busy || apiOff || !sv.can_start ? null : s ? 'service/restart' : 'service/start';
      act_.hidden = !path;
      if (path && act_.dataset.path !== path) {
        act_.dataset.path = path;
        act_.innerHTML = s ? I.rotate : I.play;
        act_.title = s ? (info && !info.elevated ? 'Restart as administrator' : 'Restart the service') : 'Start the service';
        act_.setAttribute('aria-label', act_.title);
      }
    };
    S.on('state', upd);
    S.on('config', upd);
    upd();
    return el;
  }

  // " beta" after a beta's version, unless its tag says so already, as 0.5.0-beta.1 does.
  const betaTag = (u) => (u.beta && !/beta/i.test(u.latest || '') ? ' beta' : '');

  // Under the service, only while there is a newer camless: the check at launch found it.
  function updateBadge() {
    const el = h(`<button type="button" class="cam-upd" hidden>${I.download}<span class="lbl"><b></b><small></small></span></button>`);
    const b = el.querySelector('b'), small = el.querySelector('small');
    el.addEventListener('click', () => { if (!S.server.update.busy) updateNow(); });
    const upd = () => {
      const u = S.server && S.server.update;
      el.hidden = !u || !(u.available || u.busy === 'downloading' || u.busy === 'installing');
      if (el.hidden) return;
      const [text, tip] = u.busy ? [`${cap(u.busy)}…`, `${cap(u.busy)} camless ${u.latest}.`]
        : u.error ? ['Update failed', `${u.error} Click to try again.`]
        : ['Update available', `camless ${u.latest}${betaTag(u)} is out; this is ${S.server.app_version}. Click to update.`];
      b.textContent = text;
      small.textContent = `camless ${u.latest}${betaTag(u)}`;
      el.dataset.tone = u.busy ? 'busy' : u.error ? 'bad' : '';
      el.title = tip;
      el.setAttribute('aria-label', `${text}: camless ${u.latest}`);
    };
    S.on('state', upd);
    upd();
    return el;
  }

  function verifyBadge() {
    const el = h(`<button type="button" class="cam-upd verify" hidden>${I.info}<span class="lbl"><b>Unverified hardware</b><small>Tell us if it works</small></span></button>`);
    el.title = `${UNVERIFIED} Click to tell us how it works.`;
    el.setAttribute('aria-label', 'Unverified hardware: tell us if it works');
    el.addEventListener('click', verifyDialog);
    const upd = () => {
      const list = S.status ? unverified() : [];
      el.hidden = !list.length || (K.prefs.verifyReported || []).includes(setupKey(list));
      const t = verifyTitle(list);
      el.querySelector('.lbl b').textContent = t;
      el.setAttribute('aria-label', `${t}: tell us if it works`);
    };
    S.on('state', upd);
    K.on('prefs', upd);
    upd();
    return el;
  }

  function go() {
    built = layoutKey();
    let id = decodeURIComponent(location.hash.slice(1)) || K.prefs.page || 'cooling';
    // A page that is not in the bar, such as Games while it is off, opens Cooling. Until
    // the config loads, Games stays where it is.
    if (!PAGES[id] || (S.config && !navShown(id))) id = 'cooling';
    if (K.prefs.page !== id) K.setPref('page', id);
    if (id !== page) gameOpen = null;
    const top = main.scrollTop;
    if (dispose) dispose();
    K.closeOverlays();
    content.textContent = '';
    dispose = K.withScope(() => {
      alerts(content);
      if (S.server) PAGES[id](content);
      else K.pageHead(content, NAV.find((n) => n.id === id).label);
    });
    root.querySelectorAll('.cam-nav a').forEach((a) => { if (a.dataset.id === id) a.setAttribute('aria-current', 'page'); else a.removeAttribute('aria-current'); });
    main.scrollTop = id === page ? top : 0;
    page = id;
  }

  /* -------------------------------------------------------------- alerts */

  // What needs attention, above every page.
  function currentAlerts() {
    const sv = S.server, out = [];
    if (!sv) return out;
    if (sv.load_error) {
      out.push({ kind: 'error', html: `<b>The config file has a mistake, so camless does not change it.</b> Fix it and this window picks it up.<span class="cam-err">${esc(sv.load_error)}</span>` });
    }
    if (S.st.saveError) out.push({ kind: 'error', html: `<b>The last change was not saved.</b><span class="cam-err">${esc(S.st.saveError)}</span>` });
    const status = S.status;
    if (!status) {
      const apiOff = S.config && S.config.api && S.config.api.enabled === false;
      if (sv.service_busy) out.push({ kind: 'info', html: `<b>${cap(sv.service_busy)} the camless service…</b> If Windows asks for administrator rights, answer Yes for CPU temperature.` });
      else if (apiOff) out.push({ kind: 'warn', html: '<b>Local API is off, so this window cannot see the camless service.</b> Changes are still saved, and the service applies them. Turn Local API on in Settings; it takes effect when the service restarts.' });
      else {
        out.push({
          kind: 'warn',
          html: '<b>The camless service is not running.</b> Curves that camless sets are not applied until it starts; curves stored on a cooler keep running. Changes here are saved.' + (sv.service_note ? `<span class="cam-err">${esc(sv.service_note)}</span>` : ''),
          button: sv.can_start ? { label: 'Start camless', onClick: () => act('service/start', {}) } : null,
        });
      }
      return out;
    }
    if (sv.service_busy) out.push({ kind: 'info', html: `<b>${cap(sv.service_busy)} the camless service…</b>` });
    else if (status.service && !status.service.elevated) {
      out.push({
        kind: 'warn',
        html: '<b>The service runs without administrator rights, so CPU temperature and power are off.</b> Curves on CPU temperature hold the failsafe speed. Restart it and answer Yes when Windows asks.',
        button: sv.can_start ? { label: 'Restart as administrator', onClick: () => act('service/restart', {}) } : null,
      });
    }
    if (status.config_error) out.push({ kind: 'error', html: `<b>The service could not apply the saved settings, so it keeps running the previous ones.</b><span class="cam-err">${esc(status.config_error)}</span>` });
    const hot = status.sensors.filter((s) => S.isLiquid(s.id) && s.value != null && s.value >= 55);
    if (hot.length) {
      const max = Math.max(...hot.map((s) => s.value));
      out.push({ kind: 'error', html: `<b>Liquid is at ${K.temp(max)}.</b> Every fan camless controls runs at 100% until it cools below ${K.temp(55, 0)}.` });
    }
    for (const d of status.devices) if (d.error) out.push({ kind: 'warn', html: `<b>${esc(d.name)} is not available.</b> ${esc(cap(d.error))}` });
    // Until a report about them went to GitHub.
    const fresh = (status.unsupported || []).filter((u) => !(K.prefs.reported || []).includes(usbId(u)));
    if (fresh.length) {
      out.push({
        kind: 'info',
        html: `<b>camless found ${fresh.length > 1 ? 'NZXT devices' : 'an NZXT device'} it does not drive yet: ${esc(listText(fresh.map((u) => `${u.product || 'NZXT device'} (${usbId(u)})`)))}.</b> A hardware report on GitHub helps add ${fresh.length > 1 ? 'them' : 'it'}.`,
        button: { label: 'Create report', onClick: reportDialog },
      });
    }
    const fs = status.fans.filter((f) => f.failsafe);
    if (fs.length) {
      const names = listText(fs.map((f) => fanLabel(f.channel)));
      out.push({ kind: 'warn', html: `<b>${esc(names)} ${fs.length > 1 ? 'run' : 'runs'} at the failsafe ${failsafe()}%.</b> ${fs.length > 1 ? 'Their curves have' : 'Its curve has'} no sensor reading.` });
    }
    return out;
  }

  function alerts(host) {
    const box = h('<div class="cam-alerts"></div>');
    host.appendChild(box);
    let last = '';
    const upd = () => {
      const list = currentAlerts();
      const key = JSON.stringify(list.map((a) => [a.kind, a.html, !!a.button]));
      if (key === last) return;
      last = key;
      box.textContent = '';
      for (const a of list) {
        const el = h(`<div class="cam-alert${a.kind === 'warn' ? ' warn' : a.kind === 'info' ? ' info' : ''}" role="${a.kind === 'error' ? 'alert' : 'status'}">${a.kind === 'info' ? I.info : I.warn}<div class="txt">${a.html}</div></div>`);
        if (a.button) el.appendChild(button(a.button.label, { small: true, onClick: a.button.onClick }));
        box.appendChild(el);
      }
    };
    track(S.on('state', upd));
    track(S.on('saved', upd));
    track(S.on('config', upd));
    track(K.on('prefs', upd));
    upd();
  }

  function emptyState(host, html) {
    host.appendChild(h(`<div class="cam-panel cam-empty">${html}</div>`));
  }

  /* -------------------------------------------------------------- profiles */

  function unusedName(base) {
    const taken = (S.server.profiles || []).map((p) => p.name.toLowerCase());
    if (!taken.includes(base.toLowerCase())) return base;
    for (let i = 2; ; i++) if (!taken.includes(`${base} ${i}`.toLowerCase())) return `${base} ${i}`;
  }

  // The rules camless's config has for a profile name, which is also a file name.
  function profileNameError(v, except) {
    if (!v) return 'Enter a name.';
    if (/[<>:"/\\|?*\u0000-\u001f]/.test(v)) return 'A profile name cannot contain < > : " / \\ | ? *';
    if (v.endsWith('.')) return 'A profile name cannot end with a dot.';
    if (v.length > 60) return 'Use at most 60 characters.';
    if (/^(con|prn|aux|nul|com\d|lpt\d)$/i.test(v.split('.')[0].trim())) return 'Windows keeps this name for itself.';
    if ((S.server.profiles || []).some((p) => p.name.toLowerCase() === v.toLowerCase() && p.name !== except)) return 'There is already a profile with this name.';
    return '';
  }

  async function switchTo(name) {
    // The setup running now would be lost, so it becomes a profile first.
    if (S.config && !S.config.profile && hasSetup(S.config)) return saveCurrent(name);
    if (!(await act('profiles/switch', { name }))) return false;
    toast(`Switched to ${name}.`);
    return true;
  }

  async function saveCurrent(next) {
    const name = await ask({
      title: next ? 'Keep the current setup' : 'Save as profile',
      text: next ? `The setup running now is in no profile. Save it as one before switching to <b>${esc(next)}</b>, so it is kept.` : 'The setup running now becomes a profile and keeps running.',
      label: 'Profile name', value: unusedName('My setup'), ok: next ? 'Save and switch' : 'Save', validate: (v) => profileNameError(v),
    });
    const from = (S.config && S.config.profile) || '';
    if (!name || !(await act('profiles/save-current', { name }))) return false;
    S.carryMemo(from, name, false);
    if (next) return switchTo(next);
    toast(`Saved as profile ${name}.`);
    return true;
  }

  async function newProfile() {
    let copy = true;
    const name = await ask({
      title: 'New profile', label: 'Name', value: unusedName('Profile'), ok: 'Create', validate: (v) => profileNameError(v),
      extra: (body) => {
        const box = h('<div class="cam-rows cam-box"></div>');
        box.appendChild(row('Start from', null, choice({ options: [{ value: true, label: 'Current setup' }, { value: false, label: 'Nothing' }], value: true, label: 'Start from', onChange: (v) => { copy = v; } })));
        body.appendChild(box);
      },
    });
    if (name && (await act('profiles/create', { name, copy }))) toast(`Created profile ${name}. Switch to it to use it.`);
  }

  async function renameProfile(from) {
    const to = await ask({ title: 'Rename profile', label: 'Name', value: from, ok: 'Rename', validate: (v) => profileNameError(v, from) });
    if (to && to !== from && (await act('profiles/rename', { from, to }))) { S.carryMemo(from, to, true); toast(`Renamed to ${to}.`); }
  }

  async function deleteProfile(name) {
    const ok = await confirmBox({ title: 'Delete profile', text: `Delete <b>${esc(name)}</b>? Its curves, fans, lighting and LCD settings are removed from this PC.`, ok: 'Delete' });
    if (ok && (await act('profiles/delete', { name }))) toast(`Deleted ${name}.`);
  }

  async function importCam() {
    const ok = await confirmBox({
      title: 'Import from NZXT CAM',
      text: "camless reads CAM's active profile from its own files and makes a camless profile of it: cooling curves, fan names, lighting and the LCD mode. CAM is not changed, and what runs now stays until you switch.",
      ok: 'Import',
    });
    if (!ok) return;
    const r = await act('import/cam', {});
    if (r) imported(r.name, r.notes);
  }

  function importFile() {
    document.querySelectorAll('input[type=file].cam-pick').forEach((x) => x.remove());
    const inp = h('<input type="file" class="cam-pick" accept=".toml" hidden>');
    inp.addEventListener('change', async () => {
      const f = inp.files[0];
      inp.remove();
      if (!f) return;
      if (f.size > 1 << 20) { toast(`${f.name} is too large for a camless.toml.`); return; }
      const r = await act('import/file', { name: f.name, text: await f.text() });
      if (r) imported(r.name, []);
    });
    document.body.appendChild(inp);
    inp.click();
  }

  function imported(name, notes) {
    modal({
      title: 'Imported',
      render(body) {
        body.appendChild(h(`<p class="cam-hint cam-lead">It is the new profile <b>${esc(name)}</b>. Switch to it to apply it.</p>`));
        if (notes && notes.length) {
          const ul = h('<ul class="cam-notes"></ul>');
          for (const n of notes) { const li = h('<li></li>'); li.textContent = n; ul.appendChild(li); }
          body.append(h('<p class="cam-hint">Notes from the import:</p>'), ul);
        }
      },
      actions: [{ label: 'Not now' }, { label: 'Switch to it', primary: true, onClick: () => { switchTo(name); } }],
    });
  }

  function profilesDialog() {
    modal({
      title: 'Profiles',
      render(body) {
        body.appendChild(h('<p class="cam-hint cam-lead">A profile is a whole setup: curves, fans, lighting and the LCD. Switching applies it at once.</p>'));
        const list = h('<div class="cam-rows cam-box"></div>');
        body.appendChild(list);
        const draw = K.region(list);
        let key = null;
        const upd = () => {
          const sv = S.server;
          const k = JSON.stringify([sv.profiles, S.config && S.config.profile, hasSetup(S.config)]);
          if (k === key) return;
          key = k;
          draw((host) => {
            const cur = S.config && S.config.profile;
            if (S.config && !cur) {
              host.appendChild(hasSetup(S.config)
                ? row('Current setup', 'In no profile yet. Save it as one to keep it when you switch.', button('Save as profile', { small: true, onClick: () => saveCurrent() }))
                : row('Current setup', 'Empty, and in no profile.', null));
            }
            for (const p of sv.profiles || []) {
              const running = p.name === cur;
              const r = row(p.name, p.error ? `<span class="cam-bad">${esc(p.error)}</span>` : esc(p.summary), null);
              const ctl = r.lastElementChild;
              if (running) ctl.appendChild(h('<span class="cam-status">Running</span>'));
              else {
                const sw = button('Switch', { small: true, primary: true, onClick: () => switchTo(p.name) });
                sw.disabled = !!p.error;
                ctl.appendChild(sw);
              }
              ctl.appendChild(button('Rename', { small: true, onClick: () => renameProfile(p.name) }));
              const del = button('Delete', { small: true, onClick: () => deleteProfile(p.name) });
              del.disabled = running;
              if (running) del.title = 'Switch to another profile before deleting this one.';
              ctl.appendChild(del);
              host.appendChild(r);
            }
          });
        };
        track(S.on('state', upd));
        upd();
      },
      actions: [
        { label: 'New profile', onClick: () => { newProfile(); return false; } },
        { label: 'Import from NZXT CAM', onClick: () => { importCam(); return false; } },
        { label: 'Import file', onClick: () => { importFile(); return false; } },
        { label: 'Done', primary: true },
      ],
    });
  }

  // The profile picker in the Cooling and Lighting page heads.
  function profileControls() {
    const NONE = '\u0000';
    const wrap = h('<div class="cam-profile"><span class="cam-small">Profile</span></div>');
    const opts = () => {
      const o = (S.server.profiles || []).map((p) => ({ value: p.name, label: p.name, desc: p.error ? 'has a mistake' : '' }));
      if (S.config && !S.config.profile) o.unshift({ value: NONE, label: 'None' });
      return o;
    };
    const value = () => (S.config && S.config.profile) || NONE;
    const sel = select({ options: opts(), value: value(), label: 'Profile', onChange: async (v) => { if (v === NONE || !(await switchTo(v))) sel.set(value()); } });
    wrap.append(sel, button('Manage', { small: true, onClick: profilesDialog }));
    let key = '';
    track(S.on('state', () => { const k = JSON.stringify([opts(), value()]); if (k !== key) { key = k; sel.setOptions(opts(), value()); } }));
    return wrap;
  }

  /* -------------------------------------------------------------- fan looks */

  /* Which NZXT fans are on each lighting channel, for the pictures and effects. CAM
     asks for this; camless guesses from what the device reports and keeps any
     correction with this PC's window settings. */
  const FAN_MODELS = [
    { value: 'core', label: 'RGB Core', hint: 'LEDs in the hub', leds: 8 },
    { value: 'duo', label: 'RGB Duo', hint: 'LED rings on both sides', leds: 20 },
    { value: 'rgb', label: 'RGB', hint: 'LED ring on the front', leds: 12 },
    { value: 'plain', label: 'without RGB', hint: 'No LEDs', leds: 0 },
  ];
  const FAN_FINISH = {
    black: { frame: '#1c1d21', pad: '#2a2c32', well: '#0a0b0d', blade: '#4b4e58', hub: '#26282d' },
    white: { frame: '#dfe0e4', pad: '#f2f3f5', well: '#1d1f24', blade: '#e9ebef', hub: '#cfd1d6' },
  };

  // F360 RGB Core is three F120 RGB Core, F280 RGB two F140 RGB, and so on.
  function guessFan(lightId) {
    const l = S.lights().find((x) => x.id === lightId);
    let model = null, size = 120, count = 0;
    for (const a of (l && l.hw && l.hw.accessories) || []) {
      const m = /^F(\d{3})\s+RGB(?:\s+(Core|Duo))?$/i.exec(a.trim());
      if (!m) continue;
      const mm = +m[1];
      size = mm % 140 === 0 ? 140 : 120;
      count += Math.max(1, Math.round(mm / size));
      model = m[2] ? m[2].toLowerCase() : 'rgb';
    }
    return model ? { model, size, count: clamp(count, 1, 3) } : null;
  }
  const fanSpec = (lightId) => Object.assign({ model: 'rgb', size: 120, color: 'black', count: 1 }, guessFan(lightId), (K.prefs.fans || {})[lightId]);
  const fanModel = (f) => FAN_MODELS.find((m) => m.value === f.model) || FAN_MODELS[2];
  const fanModelName = (f) => (f.model === 'plain' ? `${f.size} mm fan without RGB` : `F${f.size} ${fanModel(f).label}`);
  const fanText = (lightId, withColor = true) => { const f = fanSpec(lightId); return `${f.count > 1 ? f.count + ' × ' : ''}${fanModelName(f)}${withColor ? ', ' + f.color : ''}`; };

  // The light channel a fan's LEDs are on: led2 for fan2, the RGB header for a cooler's fans.
  function lightForFan(fanId) {
    const f = S.fan(fanId);
    if (!f || !f.device || f.pump) return null;
    const lights = f.device.lights.filter((l) => l.id !== 'ring');
    const num = (s) => (/(\d+)$/.exec(s) || [])[1];
    const n = num(fanId.split('/')[1]);
    if (n) { const l = lights.find((x) => num(x.id) === n); if (l) return `${f.dev}/${l.id}`; }
    return lights.length === 1 ? `${f.dev}/${lights[0].id}` : null;
  }

  const dim = (leds, k) => leds.map((c) => [c[0] * k, c[1] * k, c[2] * k]);
  function mean(leds) {
    const s = [0, 0, 0];
    for (const c of leds) { s[0] += c[0]; s[1] += c[1]; s[2] += c[2]; }
    return s.map((v) => v / Math.max(1, leds.length));
  }

  // One fan from the front, in its model and color; leds are its share of the channel.
  function drawFan(ctx, cx, cy, r, angle, leds, f, blur = 0) {
    const P = FAN_FINISH[f.color] || FAN_FINISH.black;
    const s = r * 1.08;
    ctx.save();
    ctx.fillStyle = P.frame;
    D.roundRect(ctx, cx - s, cy - s, s * 2, s * 2, r * 0.16);
    ctx.fill();
    ctx.fillStyle = P.pad;
    for (const dx of [-1, 1]) for (const dy of [-1, 1]) { ctx.beginPath(); ctx.arc(cx + dx * s * 0.8, cy + dy * s * 0.8, r * 0.075, 0, TAU); ctx.fill(); }
    ctx.fillStyle = P.well;
    ctx.beginPath(); ctx.arc(cx, cy, r * 0.97, 0, TAU); ctx.fill();
    const lit = leds.length > 0;
    if (f.model === 'duo' && lit) {
      // The front ring, with the back one showing through behind the blades.
      const half = Math.ceil(leds.length / 2);
      D.ledRing(ctx, cx, cy, r * 0.83, dim(leds.slice(half), 0.5), r * 0.04, 0.35);
      D.ledRing(ctx, cx, cy, r * 0.925, leds.slice(0, half), r * 0.08, 0.9);
    } else if (f.model === 'rgb' && lit) {
      D.ledRing(ctx, cx, cy, r * 0.925, leds, r * 0.05, 0.7);
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
    if (f.model === 'core' && lit) {
      // The LEDs sit in the hub and light the blades from the middle.
      const c = mean(leds);
      ctx.globalCompositeOperation = 'lighter';
      ctx.fillStyle = D.glowAt0(c, 0.5, r * 0.2, r * 0.82);
      ctx.beginPath(); ctx.arc(0, 0, r * 0.82, 0, TAU); ctx.fill();
      ctx.globalCompositeOperation = 'source-over';
      ctx.fillStyle = P.hub;
      ctx.beginPath(); ctx.arc(0, 0, r * 0.28, 0, TAU); ctx.fill();
      D.ledRing(ctx, 0, 0, r * 0.19, leds, r * 0.09, 0.9);
      ctx.fillStyle = P.hub;
      ctx.beginPath(); ctx.arc(0, 0, r * 0.11, 0, TAU); ctx.fill();
    } else {
      ctx.fillStyle = P.hub;
      ctx.beginPath(); ctx.arc(0, 0, r * 0.25, 0, TAU); ctx.fill();
    }
    ctx.restore();
  }

  // The LEDs of every fan on a channel as one chain, so effects run across them.
  function fanLeds(lightId, f, time) {
    const L = fanModel(f).leds;
    if (!L || !lightId) return () => [];
    const all = S.renderLight(S.lightOf(lightId), time, L * f.count);
    return (i) => all.slice(i * L, (i + 1) * L);
  }

  const rpmOf = (fanId) => { const f = S.fan(fanId); return (f && f.hw && f.hw.rpm) || 0; };
  const ringOf = (dev, time) => (S.lights().some((l) => l.id === `${dev}/ring`) ? S.renderLight(S.lightOf(`${dev}/ring`), time, 24) : null);
  function headOpts(dev, time, o = {}) {
    const liquid = S.value(`${dev}.liquid`), d = S.device(dev);
    // A screen without streamed pictures keeps its own display, whatever the config says.
    const lcd = !d || !d.lcd ? { mode: 'liquid', brightness: 100 } : d.lcd.frames === false ? { ...S.lcdOf(dev), mode: 'liquid' } : S.lcdOf(dev);
    return Object.assign({
      leds: ringOf(dev, time), liquid, lcd,
      sensors: { cpu: S.value('cpu.temp'), gpu: S.value('gpu.temp'), liquid },
      font: FONT, rotation: 0,
    }, o);
  }

  /* -------------------------------------------------------------- cooling */

  const PRESET_OPTS = [{ value: 'fixed', label: 'Fixed' }, { value: 'silent', label: 'Silent' }, { value: 'performance', label: 'Performance' }, { value: 'custom', label: 'Custom' }];
  const KIND = { graph: 'Graph', fixed: 'Fixed', mix: 'Mix', trigger: 'Trigger' };
  const MIX = { max: 'Highest', min: 'Lowest', average: 'Average', sum: 'Sum' };
  // The curve a point is being dragged on, so cards that share it follow.
  let dragging = null;

  function editCurve(name, patch, final) {
    dragging = final ? null : name;
    S.edit((c) => {
      const x = (c.curve || []).find((k) => k.name === name);
      if (!x) return;
      if (patch.points) x.points = patch.points;
      if (patch.duty != null) x.duty = patch.duty;
    }, { live: !final, what: final ? 'curve' : 'drag' });
  }

  function describeCurve(c) {
    if (c.kind === 'graph') return `${S.sourceLabel(c.sensor)}, ${c.points.length} point${c.points.length === 1 ? '' : 's'}`;
    if (c.kind === 'fixed') return `Fixed at ${c.duty}%`;
    if (c.kind === 'mix') return `${MIX[c.function] || c.function} of ${listText(c.curves)}`;
    return `${S.sourceLabel(c.sensor)}: ${c.load_duty}% from ${reading(c.sensor, c.load_at)}, back to ${c.idle_duty}% at ${reading(c.sensor, c.idle_at)}`;
  }

  // Liquid first, then CPU and GPU, then everything else the daemon reads.
  function sourceOptions(current) {
    const rank = (s) => (S.isLiquid(s.id) ? 0 : s.id === 'cpu.temp' ? 1 : s.id === 'gpu.temp' ? 2 : s.unit === 'celsius' ? 3 : s.unit === 'percent' ? 4 : 5);
    const opts = S.sensors().slice().sort((a, b) => rank(a) - rank(b)).map((s) => ({ value: s.id, label: S.sourceLabel(s.id), desc: s.value == null ? 'no reading' : reading(s.id, s.value) }));
    if (current && !opts.some((o) => o.value === current)) opts.push({ value: current, label: current, desc: 'not found' });
    return opts;
  }

  function curveOpts(nameOf, fanId) {
    const cur = () => S.curve(nameOf());
    return {
      model: () => {
        const c = cur(), f = fanId ? S.fan(fanId) : null;
        const minDuty = f ? f.minDuty : 0;
        if (c && c.kind === 'fixed') return { kind: 'fixed', duty: c.duty, range: [20, 100], minDuty };
        if (!c || c.kind !== 'graph') return { kind: 'fixed', duty: 0, range: [20, 100], disabled: true };
        // The cooler's own floor applies to what it stores, by its liquid temperature.
        const floor = !!f && f.onDeviceCurves && c.sensor === `${f.dev}.liquid`;
        return { kind: 'graph', points: c.points, range: S.range(c.sensor, c.points), minDuty, floor };
      },
      set: (patch, final) => editCurve(nameOf(), patch, final),
      live: () => { const v = S.curveValue(nameOf()); return v && v.duty != null ? { x: v.input, y: v.duty } : null; },
      // A fixed curve has no sensor, so its axis has no values.
      xLabel: (t) => { const c = cur(); return c && c.sensor ? axisText(c.sensor, t) : ''; },
      liveLabel: (x, y) => (x == null || !cur() ? `${Math.round(y)}%` : `${S.sourceLabel(cur().sensor)}: ${reading(cur().sensor, x)}`),
      onFrame: (fn) => S.on('frame', fn),
    };
  }

  // A channel the way CAM draws it: name, live speed, presets, source, curve.
  function coolingCard(id) {
    const f0 = S.fan(id);
    const lightId = lightForFan(id);
    const el = h(`<section class="cam-panel cam-ch">
      <div class="cam-ch-head">${lightId ? '<button type="button" class="cam-ch-pic"><canvas aria-hidden="true"></canvas></button>' : '<span class="cam-ch-pic"><canvas aria-hidden="true"></canvas></span>'}
        <div class="names"><h3 class="cam-ch-name"></h3><p class="cam-ch-sub"></p></div>
        <div class="cam-ch-live"><span class="rpm"><b class="num">--</b> RPM</span><span class="duty num"><b>--</b><small>%</small></span></div>
        <button type="button" class="cam-iconbtn" data-a="more">${I.more}</button></div>
      <div class="presets"></div>
      <div class="cam-ch-src"><span>Source:</span></div>
      <p class="cam-ch-note" hidden>${I.warn}<span></span></p>
      <p class="cam-ch-share" hidden></p>
      <div class="cam-ch-curve"></div>
      <p class="cam-ch-off" hidden></p>
      <div class="cam-ch-foot"><button type="button" class="cam-link" data-a="apply">Apply this curve to other channels</button><span class="grow"></span><span class="cam-where" tabindex="0"></span></div>
    </section>`);
    const $ = (s) => el.querySelector(s);
    const name = $('.cam-ch-name'), sub = $('.cam-ch-sub'), more = $('[data-a=more]');
    const nameOf = () => { const c = S.fanCfg(id); return c && c.curve; };
    const cur = () => S.curve(nameOf());
    const pc = choice({ options: PRESET_OPTS, value: S.preset(id), label: 'Preset', onChange: (p) => S.setPreset(id, p) });
    $('.presets').appendChild(pc);
    const src = select({ options: sourceOptions(), value: null, label: 'Source', onChange: (s) => S.setSource(id, s) });
    $('.cam-ch-src').appendChild(src);
    const box = $('.cam-ch-curve');
    const ed = new window.CurveEditor(box, curveOpts(nameOf, id));
    track(() => ed.destroy());
    const note = $('.cam-ch-note'), share = $('.cam-ch-share'), off = $('.cam-ch-off'), where = $('.cam-where'), apply = $('[data-a=apply]');
    const rpmB = $('.rpm b'), dutyB = $('.duty b');
    more.addEventListener('click', () => fanSettings(id));
    apply.addEventListener('click', () => applyDialog(id));
    share.addEventListener('click', (e) => { if (e.target.closest('[data-a=fork]')) { S.fork(id); toast(`${S.fanName(id)} has its own curve now.`); } });
    off.addEventListener('click', (e) => { if (e.target.closest('[data-a=edit]') && cur()) curveDialog(cur().name); });
    chPicture($('.cam-ch-pic'), id, lightId);

    function subText(f) {
      const parts = [];
      if (lightId) parts.push(fanText(lightId, false));
      if (f.minDuty) parts.push(`Never below ${f.minDuty}%`);
      if (f.hw && f.hw.connected === false) parts.push('No fan detected');
      if (f.missing && S.status) parts.push('Not found now');
      return parts.join(', ');
    }

    function refresh(what) {
      const f = S.fan(id) || f0, c = cur(), p = S.preset(id);
      name.textContent = S.fanName(id);
      el.setAttribute('aria-label', fanLabel(id));
      more.setAttribute('aria-label', `${S.fanName(id)} settings`);
      more.title = 'Name, curve and limits';
      sub.textContent = subText(f);
      sub.hidden = !sub.textContent;
      sub.title = sub.textContent;
      pc.set(p);
      const shown = p !== 'none' && p !== 'advanced';
      $('.cam-ch-src').hidden = !shown;
      box.hidden = !shown;
      off.hidden = shown;
      apply.hidden = !c;
      if (p === 'none') off.innerHTML = 'Not managed by camless: the device keeps its last setting. Pick a preset to control it.';
      else if (p === 'advanced') off.innerHTML = `Follows <b>${esc(c.name)}</b>, a ${c.kind} curve: ${esc(describeCurve(c))}. <button type="button" class="cam-link" data-a="edit">Edit curve</button>`;
      if (shown) {
        const sensor = c.kind === 'graph' ? c.sensor : S.defaultSensor(id);
        src.setOptions(sourceOptions(sensor), sensor);
        src.setDisabled(c.kind !== 'graph');
      }
      const u = c ? S.users(c.name) : { fans: [], mixes: [] };
      const others = u.fans.filter((x) => x !== id).map(fanLabel);
      const bits = [];
      if (others.length) bits.push(`Also drives ${esc(listText(others))}.`);
      if (u.mixes.length) bits.push(`Part of ${esc(listText(u.mixes))}.`);
      share.hidden = !bits.length;
      if (bits.length) share.innerHTML = `${I.info}<span>${bits.join(' ')} </span>${others.length ? '<button type="button" class="cam-link" data-a="fork">Give this fan its own curve</button>' : ''}`;
      // While a point is dragged, the editor draws itself; cards sharing the curve follow.
      if (what !== 'drag' || (c && c.name === dragging && !box.classList.contains('ce-dragging'))) ed.render();
      live();
    }

    function live() {
      const f = S.fan(id), v = S.fanValue(id), c = cur();
      rpmB.textContent = K.rpm(f && f.hw ? f.hw.rpm : null);
      const duty = v && v.duty != null ? v.duty : f && f.hw ? f.hw.duty : null;
      dutyB.textContent = duty == null ? '--' : Math.round(duty);
      const failing = !!(v && v.failsafe);
      note.hidden = !failing;
      if (failing) {
        const what = c && c.kind === 'graph' ? `No reading from ${S.sourceLabel(c.sensor)}.` : 'This curve has no reading.';
        note.lastElementChild.textContent = `${what} Holding ${failsafe()}% until it comes back.`;
      }
      where.hidden = !v || !c;
      if (v && c) {
        const w = v.on_device
          ? { html: `${I.kraken}<span>Stored on the cooler</span>`, title: 'The cooler runs this curve itself, so it keeps working when camless is closed.' }
          : { html: `${I.chip}<span>Set by camless</span>`, title: `camless sets this speed as the sensor changes. If the service stops, it goes to ${failsafe()}%.` };
        if (where.dataset.on !== String(v.on_device)) { where.innerHTML = w.html; where.dataset.on = String(v.on_device); }
        where.title = w.title;
        where.setAttribute('aria-label', w.title);
      }
    }

    track(S.on('config', refresh));
    track(S.on('state', live));
    track(K.on('prefs', (k) => { if (k === 'unit') ed.render(); else if (k === 'fans') refresh(); }));
    refresh();
    return el;
  }

  /* The hardware on a card: the pump head, or one of the channel's fans spinning at its
     speed and lit as on the Lighting page. A fan opens the model picker. */
  function chPicture(pic, id, lightId) {
    const cv = pic.firstElementChild;
    const pump = S.fan(id) && S.fan(id).pump;
    if (pic.tagName === 'BUTTON') {
      const paint = () => { pic.title = `Fans on this channel: ${fanText(lightId)}. Change`; pic.setAttribute('aria-label', pic.title); };
      paint();
      track(K.on('prefs', (k) => { if (k === 'fans') paint(); }));
      pic.addEventListener('click', () => fanDialog(lightId));
    }
    let time = 0, angle = 0;
    track(S.on('frame', (dt) => {
      time += dt;
      const rpm = pump ? 0 : rpmOf(id);
      angle += D.spin(rpm) * dt * TAU;
      if (!D.visible(cv)) return;
      if (pump) {
        const o = headOpts(S.devOf(id), time, { shadow: false });
        D.paint(cv, o, (ctx, w, hh) => D.head(ctx, w / 2, hh / 2, Math.min(w, hh) / 2 - 1, o));
        return;
      }
      const f = lightId ? fanSpec(lightId) : { model: 'plain', size: 120, color: 'black', count: 1 };
      const leds = fanLeds(lightId, f, time)(0);
      D.paint(cv, [angle, rpm, f, leds], (ctx, w, hh) => drawFan(ctx, w / 2, hh / 2, Math.min(w, hh) / 2 / 1.1, angle, leds, f, rpm / 1500));
    }));
  }

  // Makes other channels follow this channel's curve. They share it from then on.
  function applyDialog(id) {
    const name = S.fanCfg(id) && S.fanCfg(id).curve;
    if (!name) return;
    const others = S.fans().filter((f) => f.id !== id);
    const picked = new Set();
    modal({
      title: 'Apply this curve to other channels',
      render(body, api) {
        body.appendChild(h(`<p class="cam-hint cam-lead">The channels you pick follow <b>${esc(name)}</b> (${esc(describeCurve(S.curve(name)))}). It stays one curve: a later change applies to all of them, until one gets its own curve again.</p>`));
        for (const f of others) {
          const c = S.fanCfg(f.id);
          const now = c && c.curve === name ? 'already follows it' : f.minDuty ? `never below ${f.minDuty}%` : c && c.curve ? `follows ${c.curve}` : '';
          const lab = h(`<label class="cam-check"><input type="checkbox"><span>${esc(fanLabel(f.id))}</span><small>${esc(now)}</small></label>`);
          const box = lab.querySelector('input');
          if (c && c.curve === name) { box.checked = true; box.disabled = true; }
          box.addEventListener('change', () => { if (box.checked) picked.add(f.id); else picked.delete(f.id); api.buttons[1].disabled = !picked.size; });
          body.appendChild(lab);
        }
      },
      actions: [
        { label: 'Cancel' },
        { label: 'Apply', primary: true, onClick: () => { if (!picked.size) return false; S.useCurve([...picked], name); toast(`${picked.size} channel${picked.size > 1 ? 's follow' : ' follows'} ${name}.`); } },
      ],
      ready: (api) => { api.buttons[1].disabled = true; },
    });
  }

  // A fan's name, curve and limits.
  function fanSettings(id) {
    const c0 = S.fanCfg(id) || {};
    const f = S.fan(id);
    const d = { name: c0.name || '', curve: c0.curve || '', offset: c0.offset || 0, min: c0.min || 0, max: c0.max ?? 100, step_up: c0.step_up || 0, step_down: c0.step_down || 0 };
    const pct = (v) => `${v}%`;
    modal({
      title: `${S.fanName(id)} settings`,
      render(body) {
        const rows = h('<div class="cam-rows cam-box"></div>');
        const curves = [{ value: '', label: 'Not managed by camless' }].concat(S.curves().map((c) => ({ value: c.name, label: c.name, desc: KIND[c.kind] })));
        rows.append(
          row('Name', 'Shown in camless only.', input({ value: d.name, placeholder: (f && f.hw && f.hw.name.trim()) || id, label: 'Name', width: '200px', onChange: (v) => { d.name = v.trim(); } })),
          row('Curve', 'Channels that pick the same curve share it.', select({ options: curves, value: d.curve, label: 'Curve', onChange: (v) => { d.curve = v; } })),
          row('Offset', 'Added to the curve’s speed.', slider({ min: Math.min(-30, d.offset), max: Math.max(30, d.offset), value: d.offset, label: 'Offset', format: (v) => (v > 0 ? '+' : '') + v + '%', onInput: (v) => { d.offset = v; } })),
          row('Minimum', null, slider({ min: 0, max: 100, value: d.min, label: 'Minimum', format: pct, onInput: (v) => { d.min = v; } })),
          row('Maximum', f && f.minDuty ? `The device does not go below ${f.minDuty}%.` : null, slider({ min: 0, max: 100, value: d.max, label: 'Maximum', format: pct, onInput: (v) => { d.max = v; } })),
          row('Fastest rise', 'In % per second; 0 is no limit.', numField({ get: () => d.step_up, set: (v) => { d.step_up = v; }, min: 0, max: 100, unit: '% / s', label: 'Fastest rise' })),
          row('Fastest fall', 'In % per second; 0 is no limit.', numField({ get: () => d.step_down, set: (v) => { d.step_down = v; }, min: 0, max: 100, unit: '% / s', label: 'Fastest fall' })),
        );
        body.appendChild(rows);
      },
      actions: [{ label: 'Cancel' }, {
        label: 'Save', primary: true, onClick: () => {
          if (d.min > d.max) { toast('The minimum must not be above the maximum.'); return false; }
          S.edit((c) => {
            const x = S.ensureFan(c, id);
            Object.assign(x, { name: d.name, offset: d.offset, min: d.min, max: d.max, step_up: d.step_up, step_down: d.step_down });
            if (d.curve) x.curve = d.curve; else delete x.curve;
            for (const k of ['name', 'offset', 'min', 'step_up', 'step_down']) if (!x[k]) delete x[k];
            if (x.max === 100) delete x.max;
            S.tidyFans(c);
          }, { what: 'cooling', now: true });
        },
      }],
    });
  }

  // CAM's summary row: temperatures, then each fan's speed.
  function tiles(host) {
    const el = h('<div class="cam-tiles"></div>');
    const items = [];
    const add = (k, v, hot) => {
      const t = h('<div class="cam-panel cam-tile"><div class="k"></div><div class="v"></div></div>');
      t.firstElementChild.textContent = k;
      t.title = k;
      el.appendChild(t);
      items.push({ t, v, hot });
    };
    const tval = (id) => { const v = S.value(id); return v == null ? 'N/A' : `${K.tnum(v)}<small>${K.deg()}</small>`; };
    for (const s of S.sensors().filter((x) => S.isLiquid(x.id))) add(S.sourceLabel(s.id), () => tval(s.id), () => S.value(s.id) >= 55);
    add('CPU', () => tval('cpu.temp'));
    add('GPU', () => tval('gpu.temp'));
    for (const f of S.fans()) if (!(f.hw && f.hw.connected === false)) add(fanLabel(f.id), () => `${K.rpm(f.hw ? rpmOf(f.id) : null)}<small>RPM</small>`);
    const upd = () => {
      for (const { t, v, hot } of items) {
        const html = v();
        if (t.lastElementChild.innerHTML !== html) t.lastElementChild.innerHTML = html;
        t.classList.toggle('hot', !!(hot && hot()));
        t.classList.toggle('na', html === 'N/A');
      }
    };
    track(S.on('state', upd));
    track(K.on('prefs', upd));
    upd();
    host.appendChild(el);
  }

  function deviceStatus(id) {
    const el = h('<span class="cam-status"></span>');
    const upd = () => {
      const d = S.device(id);
      const [text, ok] = !S.status ? ['Service not running', false] : !d ? ['Not found', false] : d.error ? ['Not available', false] : d.connected ? ['Ready', true] : ['Not connected', false];
      el.textContent = text;
      el.classList.toggle('off', !ok);
    };
    track(S.on('state', upd));
    upd();
    return el;
  }

  function cooling(host) {
    K.pageHead(host, 'Cooling', S.config ? profileControls() : null);
    if (!S.config) { emptyState(host, 'Cooling can be changed again once the config file loads.'); return; }
    tiles(host);
    const fans = S.fans();
    if (!fans.length) {
      emptyState(host, S.status ? 'camless found no cooler or fan controller it supports.' : 'Fans and pumps show up here once the camless service runs.');
    }
    const devs = [...new Set(fans.map((f) => f.dev))];
    for (const dev of devs) {
      const s = K.section(host, devName(dev), deviceStatus(dev));
      const grid = h('<div class="cam-grid2 collapse"></div>');
      const idle = [];
      for (const f of fans.filter((x) => x.dev === dev)) {
        if (f.hw && f.hw.connected === false && !S.fanCfg(f.id)) idle.push(f);
        else grid.appendChild(coolingCard(f.id));
      }
      s.appendChild(grid);
      if (idle.length) {
        const list = h('<div class="cam-idle"><h3 class="cam-h3">Inactive</h3></div>');
        for (const f of idle) {
          const r = h(`<div class="cam-panel cam-inactive">${I.cooling}<b></b><span class="grow"></span><span>No fan detected</span></div>`);
          r.querySelector('b').textContent = S.fanName(f.id);
          list.appendChild(r);
        }
        s.appendChild(list);
      }
    }
    curvesSection(host);
  }

  /* ------------------------------------------------------------- curves */

  function mixesIn(a, b, seen = new Set()) {
    const c = S.curve(a);
    if (!c || c.kind !== 'mix' || seen.has(a)) return false;
    seen.add(a);
    return c.curves.some((n) => n === b || mixesIn(n, b, seen));
  }

  function usersText(name) {
    const u = S.users(name);
    const parts = [];
    if (u.fans.length) parts.push(`Drives ${listText(u.fans.map(fanLabel))}`);
    if (u.mixes.length) parts.push(`part of ${listText(u.mixes)}`);
    return parts.length ? cap(parts.join(', ')) + '.' : 'Not used.';
  }

  function curvesSection(host) {
    const s = K.section(host, 'Curves', button('New curve', { small: true, icon: 'plus', onClick: newCurveDialog }));
    s.appendChild(h('<p class="cam-hint cam-lead">Every managed channel follows a named curve. Channels can share one, and mixes and triggers are made here; a channel picks one in its settings.</p>'));
    const list = h('<div class="cam-panel cam-rows"></div>');
    s.appendChild(list);
    const draw = K.region(list);
    let key = null;
    const upd = () => {
      const k = JSON.stringify([S.curves().map((c) => [c.name, describeCurve(c)]), S.curves().map((c) => usersText(c.name))]);
      if (k === key) return;
      key = k;
      draw((el) => {
        if (!S.curves().length) { el.appendChild(row('No curves yet', 'Pick a preset on a channel above, or make a curve here.', null)); return; }
        for (const c of S.curves()) {
          const u = S.users(c.name), used = u.fans.length + u.mixes.length > 0;
          const r = row(c.name, `${esc(KIND[c.kind])}: ${esc(describeCurve(c))}. ${esc(usersText(c.name))}`, [
            button('Edit', { small: true, onClick: () => curveDialog(c.name) }),
            button('Delete', { small: true, onClick: () => deleteCurve(c.name) }),
          ]);
          const del = r.querySelectorAll('.cam-btn')[1];
          del.disabled = used;
          if (used) del.title = 'Channels or mixes still use it.';
          el.appendChild(r);
        }
      });
    };
    track(S.on('config', upd));
    track(K.on('prefs', upd));
    upd();
  }

  async function deleteCurve(name) {
    if (await confirmBox({ title: 'Delete curve', text: `Delete <b>${esc(name)}</b>?`, ok: 'Delete' })) S.deleteCurve(name);
  }

  function newCurveDialog() {
    let kind = 'graph';
    const can = { mix: S.curves().length > 0 };
    const KINDS = [
      { value: 'graph', label: 'Graph', hint: 'Speed from a sensor through points you drag.' },
      { value: 'fixed', label: 'Fixed', hint: 'One speed all the time.' },
      { value: 'mix', label: 'Mix', hint: 'The highest, lowest, average or sum of other curves, say of a CPU and a GPU curve.' },
      { value: 'trigger', label: 'Trigger', hint: 'Two speeds: up once a sensor passes one value, back down only below another.' },
    ];
    modal({
      title: 'New curve',
      render(body) {
        const box = h('<div class="cam-rows cam-box" role="radiogroup" aria-label="Kind of curve"></div>');
        for (const k of KINDS) {
          const lab = h(`<label class="cam-check cam-radio"><input type="radio" name="kind" value="${k.value}"><span><b></b><small></small></span></label>`);
          lab.querySelector('b').textContent = k.label;
          lab.querySelector('small').textContent = k.value === 'mix' && !can.mix ? 'Make another curve first.' : k.hint;
          const r = lab.querySelector('input');
          r.checked = k.value === kind;
          r.disabled = k.value === 'mix' && !can.mix;
          r.addEventListener('change', () => { if (r.checked) kind = k.value; });
          box.appendChild(lab);
        }
        body.appendChild(box);
      },
      actions: [{ label: 'Cancel' }, { label: 'Create', primary: true, onClick: () => { const name = S.newCurve(kind); setTimeout(() => curveDialog(name)); } }],
    });
  }

  // Everything about one curve. Changes apply as they are made.
  function curveDialog(name0) {
    let name = name0;
    const c0 = S.curve(name);
    if (!c0) return;
    const change = (fn) => S.edit((c) => { const x = (c.curve || []).find((k) => k.name === name); if (x) fn(x); }, { what: 'curve' });
    // Response settings at their defaults are left out of the file.
    const setResp = (x, k, v, def) => { if (v === def) delete x[k]; else x[k] = v; };
    modal({
      title: `${KIND[c0.kind]} curve`,
      wide: c0.kind === 'graph',
      render(body) {
        const rows = h('<div class="cam-rows cam-box"></div>');
        const err = h('<p class="cam-field-err" role="alert"></p>');
        const nameIn = input({
          value: name, label: 'Name', width: '220px',
          onChange: (v, el) => {
            v = v.trim();
            if (v === name) return;
            if (!v || S.curve(v)) { err.textContent = v ? `There is already a curve called ${v}.` : 'A curve needs a name.'; el.value = name; return; }
            err.textContent = '';
            S.renameCurve(name, v);
            name = v;
          },
        });
        rows.appendChild(row('Name', null, nameIn));
        const c = () => S.curve(name);
        if (c0.kind === 'graph') {
          const src = select({
            options: sourceOptions(c0.sensor), value: c0.sensor, label: 'Source',
            onChange: (s) => S.setCurveSensor(name, s),
          });
          rows.appendChild(row('Source', null, src));
          const ed = h('<div class="cam-ch-curve cam-ch-curve-big"></div>');
          const er = h('<div class="cam-editor-row"></div>');
          er.appendChild(ed);
          rows.appendChild(er);
          const editor = new window.CurveEditor(ed, curveOpts(() => name, null));
          track(() => editor.destroy());
          track(S.on('config', (what) => { if (what !== 'drag' || !ed.classList.contains('ce-dragging')) editor.render(); }));
          rows.appendChild(h('<p class="cam-hint cam-pad-x">Drag a point to move it, double-click to add one, right-click or Delete to remove one.</p>'));
          const unit = () => (S.unitOf(c().sensor) === 'celsius' ? '°C' : S.UNIT[S.unitOf(c().sensor)]);
          const hy = numField({ get: () => c().hysteresis || 0, set: (v) => change((x) => setResp(x, 'hysteresis', v, 0)), min: 0, max: 50, unit: unit(), label: 'Hysteresis' });
          rows.append(
            row('Hysteresis', 'The sensor must move this far before the speed follows.', hy),
            row('Rise', 'Seconds to cover about two thirds of a rise; 0 follows at once.', numField({ get: () => c().response_up ?? 2, set: (v) => change((x) => setResp(x, 'response_up', v, 2)), min: 0, max: 600, unit: 's', label: 'Rise' })),
            row('Fall', 'The same for a fall, slower so a short dip does not make the fans surge and settle.', numField({ get: () => c().response_down ?? 10, set: (v) => change((x) => setResp(x, 'response_down', v, 10)), min: 0, max: 600, unit: 's', label: 'Fall' })),
          );
          track(S.on('config', () => { if (c()) hy.lastElementChild.textContent = unit(); }));
        } else if (c0.kind === 'fixed') {
          rows.appendChild(row('Speed', null, slider({ min: 0, max: 100, value: c0.duty, label: 'Speed', format: (v) => v + '%', onInput: (v) => S.edit((cf) => { const x = cf.curve.find((k) => k.name === name); if (x) x.duty = v; }, { live: true, what: 'curve' }), onChange: () => S.save() })));
        } else if (c0.kind === 'mix') {
          rows.appendChild(row('Function', null, choice({ options: Object.keys(MIX).map((k) => ({ value: k, label: MIX[k] })), value: c0.function, label: 'Function', onChange: (v) => change((x) => { x.function = v; }) })));
          const picks = h('<div class="cam-picks"></div>');
          for (const o of S.curves().filter((k) => k.name !== name && !mixesIn(k.name, name))) {
            const lab = h(`<label class="cam-check"><input type="checkbox"><span></span><small></small></label>`);
            lab.querySelector('span').textContent = o.name;
            lab.querySelector('small').textContent = describeCurve(o);
            const b = lab.querySelector('input');
            b.checked = c0.curves.includes(o.name);
            b.addEventListener('change', () => {
              const now = c().curves;
              if (!b.checked && now.length === 1 && now[0] === o.name) { b.checked = true; toast('A mix needs at least one curve.'); return; }
              change((x) => { x.curves = b.checked ? [...x.curves, o.name] : x.curves.filter((n) => n !== o.name); });
            });
            picks.appendChild(lab);
          }
          const r = row('Curves', null, null);
          r.classList.add('cam-row-stack');
          r.appendChild(picks);
          rows.appendChild(r);
        } else {
          const src = select({ options: sourceOptions(c0.sensor), value: c0.sensor, label: 'Source', onChange: (s) => change((x) => { x.sensor = s; }) });
          const unit = () => (S.unitOf(c().sensor) === 'celsius' ? '°C' : S.UNIT[S.unitOf(c().sensor)]);
          // load_at stays above idle_at; a value that would cross it is refused.
          const at = (k, other, above) => numField({
            get: () => c()[k], label: k === 'load_at' ? 'Up at' : 'Down at', unit: unit(),
            set: (v) => {
              const o = c()[other];
              if (above ? v <= o : v >= o) { err.textContent = 'The speed goes up at a higher value than it goes back down at.'; fields[k].input.value = n1(c()[k]); return; }
              err.textContent = '';
              change((x) => { x[k] = v; });
            },
          });
          const fields = { load_at: at('load_at', 'idle_at', true), idle_at: at('idle_at', 'load_at', false) };
          rows.append(
            row('Source', null, src),
            row('Load speed', null, slider({ min: 0, max: 100, value: c0.load_duty, label: 'Load speed', format: (v) => v + '%', onInput: (v) => S.edit((cf) => { const x = cf.curve.find((k) => k.name === name); if (x) x.load_duty = v; }, { live: true, what: 'curve' }), onChange: () => S.save() })),
            row('Up at', 'The load speed starts when the sensor reaches this.', fields.load_at),
            row('Idle speed', null, slider({ min: 0, max: 100, value: c0.idle_duty, label: 'Idle speed', format: (v) => v + '%', onInput: (v) => S.edit((cf) => { const x = cf.curve.find((k) => k.name === name); if (x) x.idle_duty = v; }, { live: true, what: 'curve' }), onChange: () => S.save() })),
            row('Down at', 'Back to the idle speed only once the sensor falls to this.', fields.idle_at),
          );
          track(S.on('config', () => { if (c()) for (const f of Object.values(fields)) f.lastElementChild.textContent = unit(); }));
        }
        body.append(rows, err);
        const saveErr = () => { if (S.st.saveError && !err.textContent) err.textContent = S.st.saveError; };
        track(S.on('saved', saveErr));
      },
      actions: [{ label: 'Done', primary: true }],
    });
  }

  /* -------------------------------------------------------------- lighting */

  const EFFECT_ORDER = ['fixed', 'breathing', 'fading', 'pulse', 'candle', 'covering-marquee', 'spectrum-wave', 'rainbow-flow', 'super-rainbow', 'rainbow-pulse', 'tai-chi', 'water-cooler', 'off'];
  const SPEED_NAME = { slowest: 'Slowest', slower: 'Slow', normal: 'Normal', faster: 'Fast', fastest: 'Fastest' };
  const PALETTE = ['ffffff', 'bf4cbf', '3232de', '21d373', 'ff8a00', 'ff022d', '00c8ff', 'ffd400'];
  const LEAVE = '';

  // The name given in camless, else CAM's: Ring, Channel 1, RGB connector.
  const lightLabel = (id) => S.lightName(id) || defaultLightLabel(id);
  function defaultLightLabel(id) {
    const ch = id.split('/')[1];
    if (ch === 'ring') return 'Ring';
    if (ch === 'rgb') return 'RGB connector';
    const m = /^led(\d+)$/.exec(ch);
    if (m) return `Channel ${m[1]}`;
    const l = S.lights().find((x) => x.id === id);
    return (l && l.hw && l.hw.name) || ch;
  }
  const isRing = (id) => id.split('/')[1] === 'ring';

  /* The colors a channel had before an effect with fewer, or none, took them, so an
     effect that takes more gets them back: Fixed red, then Spectrum Wave, then Fixed
     is red again. By place, so a color used twice comes back twice. Kept for as long
     as the window is open. */
  const colorMemo = {};
  // The channel's colors in their places, then the ones its effect does not show.
  function memoColors(id, had = S.lightOf(id).colors || []) {
    return (colorMemo[id] = had.concat((colorMemo[id] || []).slice(had.length)));
  }
  function fitColors(x) {
    const [mn, mx] = S.EFFECTS[S.effectKey(x.mode)].colors;
    x.colors = memoColors(x.channel, x.colors || []).slice(0, mx);
    let i = 0;
    while (x.colors.length < mn) x.colors.push(PALETTE.find((c) => !x.colors.includes(c)) || PALETTE[i++ % PALETTE.length]);
  }

  // CAM's step for telling it which fans are on a channel, with a picture of each look.
  function fanDialog(lightId) {
    let pick = fanSpec(lightId);
    const guess = guessFan(lightId);
    const l = S.lights().find((x) => x.id === lightId);
    const acc = ((l && l.hw && l.hw.accessories) || []).filter((a) => a !== 'simulated');
    modal({
      title: `Fans on ${lightLabel(lightId)}`,
      wide: true,
      render(body) {
        body.appendChild(h(`<p class="cam-hint cam-lead">${acc.length ? `The device reports ${esc(listText(acc))}. ` : ''}Pick the fans so the preview and effects match your case.${guess ? '' : ' camless could not tell from the device.'}</p>`));
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
        const opts = h('<div class="cam-rows cam-box"></div>');
        opts.append(
          row('How many', null, choice({ options: [1, 2, 3].map((n) => ({ value: n, label: String(n) })), value: pick.count, label: 'How many fans', onChange: (v) => { pick = Object.assign({}, pick, { count: v }); } })),
          row('Size', null, choice({ options: [{ value: 120, label: '120 mm' }, { value: 140, label: '140 mm' }], value: pick.size, label: 'Fan size', onChange: (v) => { pick = Object.assign({}, pick, { size: v }); paint(); } })),
          row('Color', null, choice({ options: [{ value: 'black', label: 'Black' }, { value: 'white', label: 'White' }], value: pick.color, label: 'Fan color', onChange: (v) => { pick = Object.assign({}, pick, { color: v }); } })),
        );
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
        const fanId = S.fans().find((f) => lightForFan(f.id) === lightId);
        track(S.on('frame', (dt) => {
          time += dt;
          angle += D.spin(fanId ? rpmOf(fanId.id) : 600) * dt * TAU * 0.5;
          for (const c of cards) {
            if (!D.visible(c.cv)) continue;
            const f = Object.assign({}, pick, { model: c.m.value, count: 1 });
            const leds = fanLeds(lightId, f, time)(0);
            D.paint(c.cv, [angle, f, leds], (ctx, w, hh) => drawFan(ctx, w / 2, hh / 2, Math.min(w, hh) / 2 / 1.12, angle, leds, f));
          }
        }));
      },
      actions: [{ label: 'Cancel' }, { label: 'Save', primary: true, onClick: () => { K.setPref('fans', Object.assign({}, K.prefs.fans, { [lightId]: pick })); toast(`${lightLabel(lightId)}: ${fanText(lightId, false)}.`); } }],
    });
  }

  // CAM's color row: a swatch per color, and + Add a color up to what the effect takes.
  function colorSwatches(id) {
    const sw = h('<div class="cam-swatches"></div>');
    sw.paint = () => {
      const x = S.lightOf(id);
      const [mn, mx] = S.EFFECTS[S.effectKey(x.mode)].colors;
      sw.textContent = '';
      const cols = (x.colors || []).slice(0, mx);
      cols.forEach((c, i) => {
        const s = h(`<span class="cam-swatch" style="--c:#${esc(c)}"><input type="color" value="#${esc(c)}" aria-label="Color ${i + 1}, #${esc(c)}">${cols.length > mn ? `<button type="button" class="x" aria-label="Remove color ${i + 1}">×</button>` : ''}</span>`);
        const inp = s.querySelector('input');
        inp.addEventListener('input', () => { s.style.setProperty('--c', inp.value); S.editLight(id, (y) => { y.colors[i] = inp.value.slice(1); }); memoColors(id); });
        const xb = s.querySelector('.x');
        if (xb) xb.addEventListener('click', (e) => { e.preventDefault(); memoColors(id).splice(i, 1); S.editLight(id, (y) => { y.colors.splice(i, 1); }); sw.paint(); });
        sw.appendChild(s);
      });
      if (cols.length < mx) {
        const add = h('<button type="button" class="cam-link">+ Add a color</button>');
        add.addEventListener('click', () => {
          // A color the effect hid comes back first.
          const kept = memoColors(id);
          S.editLight(id, (y) => { y.colors = (y.colors || []).slice(0, mx); y.colors.push(kept[y.colors.length] || PALETTE.find((c) => !y.colors.includes(c)) || 'ffffff'); });
          memoColors(id);
          sw.paint();
        });
        sw.appendChild(add);
      }
    };
    sw.paint();
    return sw;
  }

  function drawLight(cv, id, time, angles) {
    if (!D.visible(cv)) return;
    if (isRing(id)) {
      const o = headOpts(S.devOf(id), time, { leds: S.renderLight(S.lightOf(id), time, 24), shadowColor: 'rgba(0,0,0,.45)' });
      D.paint(cv, o, (ctx, w, hh) => D.head(ctx, w / 2, hh / 2, Math.min(w, hh) / 2 - 10, o));
      return;
    }
    const fan = S.fans().find((f) => lightForFan(f.id) === id);
    const rpm = fan ? rpmOf(fan.id) : 0;
    const f = fanSpec(id);
    const n = f.count;
    const all = fanLeds(id, f, time);
    const leds = Array.from({ length: n }, (_, i) => all(i));
    const at = angles.slice(0, n).map((a) => a || 0);
    D.paint(cv, [f, rpm, at, leds], (ctx, w, hh) => {
      if (n > 1) {
        const r = Math.min(w / 2.4, hh / (2.3 * n + 0.1));
        for (let i = 0; i < n; i++) drawFan(ctx, w / 2, hh / 2 + (i - (n - 1) / 2) * r * 2.3, r, at[i] || 0, leds[i], f, rpm / 1800);
      } else {
        drawFan(ctx, w / 2, hh / 2, Math.min(w, hh) / 2.6, at[0] || 0, leds[0], f, rpm / 1500);
      }
    });
  }

  function lightEditor(host, id) {
    const el = h('<div class="cam-panel cam-light"><div class="cam-light-prev"><div class="cam-prev-box"><canvas role="img"></canvas><p></p></div></div><div class="cam-light-ctl cam-rows"></div></div>');
    const cv = el.querySelector('canvas'), cap1 = el.querySelector('.cam-light-prev p');
    const ctl = el.querySelector('.cam-light-ctl');
    const ring = isRing(id);
    // A light that lists its modes takes only those; NZXT's Hue 2 lights take them all.
    const hw = (S.lights().find((x) => x.id === id) || {}).hw;
    const takes = (k) => !(hw && hw.modes && hw.modes.length) || hw.modes.includes(k);
    const opts = [{ value: LEAVE, label: 'Not set by camless' }].concat(EFFECT_ORDER.filter(takes).map((k) => ({ value: k, label: S.EFFECTS[k].label })));
    const fx = select({
      options: opts, value: LEAVE, label: 'Effect',
      onChange: (m) => {
        if (m === LEAVE) { memoColors(id); S.leaveLight(id); }
        else S.editLight(id, (x) => { x.mode = m; fitColors(x); });
      },
    });
    const rEffect = row('Effect', '', fx);
    const sw = colorSwatches(id);
    const rColors = row('Colors', null, sw);
    const speedIx = () => Math.max(0, S.SPEEDS.indexOf(S.lightOf(id).speed || 'normal'));
    const sp = slider({ min: 0, max: 4, step: 1, value: speedIx(), label: 'Speed', format: (i) => SPEED_NAME[S.SPEEDS[i]], onInput: (i) => S.editLight(id, (x) => { x.speed = S.SPEEDS[i]; }) });
    const rSpeed = row('Speed', null, sp);
    const dir = toggle({ checked: !!S.lightOf(id).backward, label: 'Invert direction', onChange: (v) => S.editLight(id, (x) => { x.backward = v; }) });
    const rDir = row('Direction', null, [dir, h('<span class="cam-small">Invert direction</span>')]);
    const nameIn = input({ value: S.lightName(id), placeholder: defaultLightLabel(id), label: 'Channel name', width: '200px', onChange: (v) => S.setLightName(id, v.trim()) });
    const rName = row('Name', 'Shown in camless only.', nameIn);
    const fanLbl = h('<span class="cam-fanname"></span>');
    const rFans = ring ? null : row('Fans', '', [fanLbl, button('Change', { small: true, onClick: () => fanDialog(id) })]);
    ctl.appendChild(rName);
    if (rFans) ctl.appendChild(rFans);
    ctl.append(rEffect, rColors, rSpeed, rDir);
    const l = S.lights().find((x) => x.id === id);
    cap1.textContent = ring ? `${(l && l.leds) || 24} LEDs around the screen` : (l && l.hw && l.hw.name) || '';

    // The preview's label follows every edit, a new name or color included.
    function label() {
      const x = S.lightOf(id), plain = !ring && fanSpec(id).model === 'plain';
      cv.setAttribute('aria-label', `${lightLabel(id)}${rFans ? ', ' + fanText(id) : ''}: ${plain ? 'no LEDs' : S.describeLight(x)}`);
    }
    function refresh() {
      const x = S.lightOf(id), k = S.effectKey(x.mode), E = S.EFFECTS[k];
      const plain = !ring && fanSpec(id).model === 'plain';
      fx.set(x.unmanaged ? LEAVE : k);
      rEffect.hint(x.unmanaged ? 'camless leaves this channel as it is. Pick an effect to set it.' : '');
      rEffect.hidden = plain;
      rColors.hidden = plain || x.unmanaged || E.colors[1] === 0;
      rSpeed.hidden = plain || x.unmanaged || !E.speed;
      rDir.hidden = plain || x.unmanaged || !E.dir;
      sp.set(speedIx());
      dir.set(!!x.backward);
      if (rFans) {
        fanLbl.textContent = fanText(id);
        rFans.hint(plain ? 'These fans have no LEDs, so this channel has nothing to light.' : '');
      }
      label();
      if (document.activeElement !== nameIn) nameIn.value = S.lightName(id);
      sw.paint();
    }
    // Only a change of effect rebuilds the controls, so a color picker stays open while it is dragged.
    let lastMode = null;
    track(S.on('config', (what) => { const x = S.lightOf(id); const m = x.unmanaged ? LEAVE : x.mode; if (what !== 'lighting' || m !== lastMode) { lastMode = m; refresh(); } else label(); }));
    track(K.on('prefs', (k) => { if (k === 'fans') refresh(); }));
    lastMode = S.lightOf(id).unmanaged ? LEAVE : S.lightOf(id).mode;
    refresh();
    let time = 0;
    const angles = [0, 0.7, 1.4];
    track(S.on('frame', (dt) => {
      time += dt;
      const fan = ring ? null : S.fans().find((f) => lightForFan(f.id) === id);
      if (fan) for (let i = 0; i < angles.length; i++) angles[i] += D.spin(rpmOf(fan.id)) * dt * TAU;
      drawLight(cv, id, time, angles);
    }));
    host.appendChild(el);
  }

  /* ------------------------------------------------------------------- LCD */

  const KRAKENDIAL = 'http://127.0.0.1:30640/';
  // The service's Local API when camless.toml sets none: camless_core::DEFAULT_API.
  const API_DEFAULT = { enabled: true, listen: '127.0.0.1:30641' };

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
      D.roundRect(ctx, r * 0.84, dy - tw * 0.62, r * 0.2, tw * 1.24, r * 0.04);
      ctx.fill();
    }
    ctx.restore();
  }
  // The head as installed: tubes where they leave it, the picture upright.
  function drawHeadInstalled(cv, dev, time, o) {
    if (!D.visible(cv)) return;
    const opts = headOpts(dev, time, { shadow: o.shadow, shadowColor: 'rgba(0,0,0,.4)' });
    D.paint(cv, [o.orientation, opts], (ctx, w, hh) => {
      const r = Math.min(w, hh) / 2 / 1.42;
      D.cached(ctx, `tubes|${r}|${o.orientation}`, w / 2, hh / 2, r * 1.45, (c, x, y) => drawTubes(c, x, y, r, o.orientation));
      D.head(ctx, w / 2, hh / 2, r, opts);
    });
  }

  // KrakenDial is made for the Kraken Elite's round 640 × 640 screen.
  function dialHint(dev) {
    const d = S.device(dev), lcd = d && d.lcd;
    const fits = !lcd || (lcd.width === 640 && lcd.height === 640 && lcd.round);
    return ` We recommend checking out <button type="button" class="cam-link" data-a="krakendial">KrakenDial</button>: temperatures, loads and your own faces on the screen.${fits ? '' : ' It is made for the Kraken Elite’s round 640 × 640 screen, so on this one it may not fit.'}`;
  }

  function lcdEditor(host, dev) {
    const el = h('<div class="cam-panel cam-lcd"><div class="cam-light-prev"><div class="cam-prev-box"><canvas role="img"></canvas><p></p></div></div><div class="cam-light-ctl cam-rows"></div></div>');
    const cv = el.querySelector('canvas'), cap1 = el.querySelector('.cam-light-prev p');
    const ctl = el.querySelector('.cam-light-ctl');
    const L = () => S.lcdOf(dev);
    // Some coolers take pictures only into flash memory, which camless does not write.
    const stills = () => { const d = S.device(dev); return !!(d && d.lcd && d.lcd.frames === false); };
    const mode = select({
      options: [{ value: 'liquid', label: 'Liquid temperature' }].concat(stills() ? [] : [{ value: 'web', label: 'Web page' }, { value: 'frames', label: 'Frames from a program' }]),
      value: L().mode, label: 'Display mode',
      onChange: (v) => S.editLcd(dev, (l) => {
        l.mode = v;
        // Web mode needs an address; KrakenDial's until another is entered.
        if (v === 'web' && !/^https?:\/\//.test(l.url || '')) l.url = KRAKENDIAL;
      }),
    });
    const rMode = row('Display mode', '', mode);
    rMode.addEventListener('click', (e) => { if (e.target.closest('[data-a=krakendial]')) act('open-link', { name: 'krakendial' }); });
    const url = input({
      value: L().url || '', label: 'Page address', width: '100%', placeholder: KRAKENDIAL,
      onChange: (v, inp) => {
        v = v.trim();
        if (!/^https?:\/\/\S+$/.test(v)) { toast('The address starts with http:// or https://.'); inp.value = L().url || ''; return; }
        S.editLcd(dev, (l) => { l.url = v; });
      },
    });
    const rUrl = row('Page', '', url);
    const fps = slider({ min: 1, max: 30, value: L().fps || 10, label: 'Frames per second', format: (v) => `${v} fps`, onInput: (v) => S.editLcd(dev, (l) => { l.fps = v; }, { live: true }), onChange: () => S.save() });
    const rFps = row('Frame rate', 'More frames look smoother and cost the browser more CPU.', fps);
    const br = slider({ min: 0, max: 100, step: 5, value: L().brightness, label: 'Brightness', format: (v) => v + '%', onInput: (v) => S.editLcd(dev, (l) => { l.brightness = v; }, { live: true }), onChange: () => S.save() });
    const rBr = row('Brightness', null, br);
    const rotVal = h('<span class="cam-small num"></span>');
    const rRot = row('Rotate display', 'Match the tubes to how the cooler is installed.', [rotVal, button('Rotate display', { icon: 'rotate', small: true, onClick: () => rotateDialog(dev) })]);
    ctl.append(rMode, rUrl, rFps, rBr, rRot);
    function refresh() {
      const l = { ...L() };
      if (stills()) l.mode = 'liquid';
      mode.set(l.mode);
      if (document.activeElement !== url) url.value = l.url || '';
      br.set(l.brightness);
      fps.set(l.fps || 10);
      rotVal.textContent = l.orientation + '°';
      rUrl.hidden = rFps.hidden = l.mode !== 'web';
      const api = Object.assign({}, API_DEFAULT, S.config && S.config.api);
      rMode.hint(stills()
        ? 'The cooler draws this screen itself. It shows other pictures only from its flash memory, and camless does not write there, so web pages and frames are not offered.'
        : l.mode === 'liquid'
        ? 'The cooler draws this screen itself, so it stays on when camless is closed.'
        : l.mode === 'web'
          ? `camless shows the page the way CAM’s Web Integration does, so pages made for CAM work. It runs in a hidden Edge window, which takes memory and CPU; the other modes need none.${dialHint(dev)}`
          : `A program on this PC posts frames to <code>http://${esc(api.listen)}/v1/lcd/frame</code>. When they stop, the screen goes back to the cooler’s own display.${api.enabled === false ? ' <b>Turn on Local API in Settings first.</b>' : ''}`);
      cap1.textContent = l.mode === 'liquid' ? 'Liquid temperature, drawn by the cooler' : l.mode === 'web' ? 'A web page' : 'Frames from a program';
      cv.setAttribute('aria-label', `Screen preview: ${cap1.textContent}, ${l.brightness}% brightness, rotated ${l.orientation}°`);
      webState();
    }
    function webState() {
      const l = L(), d = S.device(dev), w = d && d.web;
      if (l.mode !== 'web' || stills()) return;
      const text = !S.status ? '' : !w ? 'Starting the page…' : w.error ? `<span class="cam-bad">${esc(cap(w.error))}</span>` : `On the screen at ${n1(w.fps)} fps${w.browser ? ` through ${esc(w.browser)}` : ''}${w.background_frames === 1 ? ', over a still background that the page handed over' : w.background_frames ? `, over a background of ${w.background_frames} frames that the page handed over` : ''}.`;
      rUrl.hint(text);
    }
    track(S.on('config', refresh));
    track(S.on('state', webState));
    refresh();
    let time = 0;
    track(S.on('frame', (dt) => { time += dt; drawHeadInstalled(cv, dev, time, { orientation: L().orientation }); }));
    host.appendChild(el);
  }

  function rotateDialog(dev) {
    let pick = S.lcdOf(dev).orientation;
    modal({
      title: 'Screen rotation',
      render(body) {
        body.appendChild(h('<p class="cam-hint cam-lead">Match the tubes to how the cooler is installed.</p>'));
        const grid = h('<div class="cam-rot" role="radiogroup" aria-label="Screen rotation"></div>');
        const cvs = [];
        for (const d of [0, 90, 180, 270]) {
          const b = h(`<button type="button" role="radio" aria-checked="${d === pick}"><canvas aria-hidden="true"></canvas><span>${d}°</span></button>`);
          b.addEventListener('click', () => { pick = d; grid.querySelectorAll('button').forEach((x, i) => x.setAttribute('aria-checked', String([0, 90, 180, 270][i] === pick))); });
          grid.appendChild(b);
          cvs.push([b.querySelector('canvas'), d]);
        }
        body.appendChild(grid);
        const draw = () => { for (const [cv, d] of cvs) drawHeadInstalled(cv, dev, 0, { orientation: d, shadow: false }); };
        requestAnimationFrame(draw);
        track(S.on('state', draw));
      },
      actions: [{ label: 'Cancel' }, { label: 'Save', primary: true, onClick: () => { S.editLcd(dev, (l) => { l.orientation = pick; }); toast(`Screen rotated to ${pick}°.`); } }],
    });
  }

  /* --------------------------------------------------------- lighting page */

  const lightState = { device: null, tab: {} };

  // The Control Hub: a dark box with a lit dot per fan port in use.
  function drawHub(ctx, w, hh, dev) {
    const d = S.device(dev);
    const ports = d ? d.fans : [];
    const bw = Math.min(w * 0.92, hh * 1.4), bh = bw * 0.62;
    const x = (w - bw) / 2, y = (hh - bh) / 2;
    ctx.fillStyle = '#16171b';
    D.roundRect(ctx, x, y, bw, bh, bw * 0.08); ctx.fill();
    ctx.strokeStyle = 'rgba(255,255,255,.14)'; ctx.lineWidth = 1;
    D.roundRect(ctx, x + 0.5, y + 0.5, bw - 1, bh - 1, bw * 0.08); ctx.stroke();
    const n = Math.max(1, ports.length), step = 0.84 / n;
    ports.forEach((p, i) => {
      const px = x + bw * (0.08 + step * (i + 0.5)), py = y + bh * 0.62;
      ctx.fillStyle = '#25272c';
      D.roundRect(ctx, px - bw * step * 0.32, py - bh * 0.16, bw * step * 0.64, bh * 0.32, 2); ctx.fill();
      ctx.fillStyle = p.connected === false ? '#3a3c42' : '#21d373';
      ctx.beginPath(); ctx.arc(px, y + bh * 0.26, Math.max(1.5, bw * 0.025), 0, TAU); ctx.fill();
    });
  }

  function deviceThumb(cv, dev, time) {
    if (!D.visible(cv)) return;
    const d = S.device(dev);
    if ((d && d.lcd) || S.lights().some((l) => l.id === `${dev}/ring`)) {
      const o = headOpts(dev, time, { shadow: false });
      D.paint(cv, o, (ctx, w, hh) => D.head(ctx, w / 2, hh / 2, Math.min(w, hh) / 2 - 2, o));
    } else {
      D.paint(cv, ['hub', ((S.device(dev) || {}).fans || []).map((p) => p.connected !== false)], (ctx, w, hh) => drawHub(ctx, w, hh, dev));
    }
  }

  function lighting(host) {
    K.pageHead(host, 'Lighting', S.config ? profileControls() : null);
    if (!S.config) { emptyState(host, 'Lighting can be changed again once the config file loads.'); return; }
    const devs = [...new Set(S.lights().map((l) => l.dev).concat(S.screens().map((s) => s.id)))];
    if (!devs.length) { emptyState(host, S.status ? 'camless found no device with lighting or a screen.' : 'Lighting shows up here once the camless service runs.'); return; }
    if (!devs.includes(lightState.device)) lightState.device = devs[0];
    if (devs.length > 1) {
      const pick = h('<div class="cam-devpick" role="group" aria-label="Device"></div>');
      const thumbs = [];
      for (const dev of devs) {
        const b = h(`<button type="button" aria-pressed="${dev === lightState.device}"><canvas aria-hidden="true"></canvas><span><b></b></span></button>`);
        b.querySelector('b').textContent = devName(dev);
        b.lastElementChild.appendChild(deviceStatus(dev));
        b.addEventListener('click', () => { lightState.device = dev; pick.querySelectorAll('button').forEach((x, i) => x.setAttribute('aria-pressed', String(devs[i] === dev))); renderDevice(); });
        pick.appendChild(b);
        thumbs.push([b.querySelector('canvas'), dev]);
      }
      host.appendChild(pick);
      let time = 0;
      track(S.on('frame', (dt) => { time += dt; for (const [cv, dev] of thumbs) deviceThumb(cv, dev, time); }));
    }
    const tabsHost = h('<div></div>');
    const body = h('<div></div>');
    host.append(tabsHost, body);
    const draw = K.region(body);
    function renderDevice() {
      const dev = lightState.device;
      const tabs = (S.screens().some((s) => s.id === dev) ? [{ id: 'lcd', label: 'LCD Display' }] : [])
        .concat(S.lights().filter((l) => l.dev === dev).map((l) => ({ id: l.id, label: lightLabel(l.id) })));
      if (!tabs.some((t) => t.id === lightState.tab[dev])) lightState.tab[dev] = tabs[0].id;
      tabsHost.textContent = '';
      tabsHost.appendChild(K.tabStrip(tabs, lightState.tab[dev], (k) => { lightState.tab[dev] = k; showTab(); }, { sub: true, label: `${devName(dev)} channels` }));
      showTab();
      function showTab() {
        const k = lightState.tab[dev];
        draw((b) => { if (k === 'lcd') lcdEditor(b, dev); else lightEditor(b, k); });
      }
    }
    renderDevice();
    // A channel renamed in its tab: only the tab labels change.
    track(S.on('config', (what) => {
      if (what !== 'lighting' && what !== 'replace') return;
      for (const b of tabsHost.querySelectorAll('[role=tab]')) if (b.dataset.id !== 'lcd') b.textContent = lightLabel(b.dataset.id);
    }));
  }

  /* ------------------------------------------------------------ monitoring */

  const SERIES_COLORS = [['#be9fff', '#640a92'], ['#21d373', '#0f8a4c'], ['#4cc2ff', '#0a6fd6'], ['#ffb547', '#b86e00'], ['#ff6b8a', '#c2183d'], ['#2dd4bf', '#0f766e'], ['#e879f9', '#a21caf'], ['#a3e635', '#4d7c0f']];
  const seriesColor = (i) => SERIES_COLORS[i % SERIES_COLORS.length][K.prefs.theme === 'light' ? 1 : 0];

  function gauge(label) {
    const R = 50, Ci = TAU * R, Lr = Ci * 0.75;
    const el = h(`<div class="cam-gauge" role="img"><svg viewBox="0 0 124 124" aria-hidden="true"><g transform="rotate(135 62 62)"><circle class="track" cx="62" cy="62" r="${R}" fill="none" stroke-width="8" stroke-linecap="round" stroke-dasharray="${Lr} ${Ci}"/><circle class="val" cx="62" cy="62" r="${R}" fill="none" stroke-width="8" stroke-linecap="round" stroke-dasharray="0 ${Ci}"/></g><text class="big" x="62" y="66" text-anchor="middle">--</text><text class="unit" x="62" y="84" text-anchor="middle"></text></svg><span></span></div>`);
    el.lastElementChild.textContent = label;
    const val = el.querySelector('.val'), big = el.querySelector('.big'), unit = el.querySelector('.unit');
    el.set = (frac, text, u, hot) => {
      val.setAttribute('stroke-dasharray', `${Math.max(0.01, Lr * clamp(frac || 0, 0, 1))} ${Ci}`);
      big.textContent = text;
      unit.textContent = u;
      el.classList.toggle('hot', !!hot);
      el.setAttribute('aria-label', `${label} ${text}${u}`);
    };
    return el;
  }

  // [fraction of the dial, number, unit] for a sensor.
  function dial(id, v) {
    if (v == null) return [0, 'N/A', ''];
    const u = S.unitOf(id);
    if (u === 'percent') return [v / 100, String(Math.round(v)), '%'];
    if (u === 'watt') return [v / 300, String(Math.round(v)), 'W'];
    const [lo, hi] = S.isLiquid(id) ? [20, 60] : [20, 100];
    return [(v - lo) / (hi - lo), String(Math.round(K.toUnit(v))), K.deg()];
  }

  function monitoring(host) {
    K.pageHead(host, 'PC Monitoring');
    host.appendChild(h('<svg width="0" height="0" style="position:absolute" aria-hidden="true"><defs><linearGradient id="cam-gauge-grad" x1="0" y1="1" x2="1" y2="0"><stop offset="0" stop-color="#7416e6"/><stop offset="1" stop-color="#bf4cbf"/></linearGradient></defs></svg>'));
    const grid = h('<div class="cam-mon"></div>');
    const sens = (id) => () => { const v = S.value(id); return dial(id, v).concat([S.isLiquid(id) && v >= 55]); };
    const stat = (id) => () => reading(id, S.value(id));
    const cards = [
      { title: 'CPU', g: [['Load', sens('cpu.load')], ['Temperature', sens('cpu.temp')]], stats: [['Power', stat('cpu.power')]] },
      { title: 'GPU', g: [['Load', sens('gpu.load')], ['Temperature', sens('gpu.temp')]], stats: [['Power', stat('gpu.power')]] },
      { title: 'Memory', g: [['In use', sens('ram.load')]], stats: [] },
    ];
    for (const d of S.devices().filter((x) => S.sensors().some((s) => s.id === `${x.id}.liquid`))) {
      const pump = d.fans.find((f) => f.pump), fans = d.fans.filter((f) => !f.pump);
      const g = [['Liquid', sens(`${d.id}.liquid`)]];
      if (pump) g.push(['Pump', () => { const f = S.fan(`${d.id}/${pump.id}`); const v = f && f.hw ? f.hw.duty : null; return v == null ? [0, 'N/A', ''] : [v / 100, String(v), '%']; }]);
      const stats = [];
      if (pump) stats.push(['Pump', () => `${K.rpm(rpmOf(`${d.id}/${pump.id}`))} RPM`]);
      for (const f of fans) stats.push([fans.length > 1 ? f.name.trim() : 'Fan', () => `${K.rpm(rpmOf(`${d.id}/${f.id}`))} RPM`]);
      cards.push({ title: d.name, sub: d.firmware ? `Firmware ${d.firmware}` : '', g, stats });
    }
    const upd = [];
    for (const c of cards) {
      const el = h('<section class="cam-panel cam-moncard"><header><h3></h3><span></span></header><div class="cam-gauges"></div><div class="cam-monstats"></div></section>');
      el.querySelector('h3').textContent = c.title;
      el.querySelector('header span').textContent = c.sub || '';
      const gs = c.g.map(([label, f]) => { const g = gauge(label); el.querySelector('.cam-gauges').appendChild(g); return [g, f]; });
      const ss = c.stats.map(([label, f]) => { const s = h('<span></span>'); s.textContent = label; const b = h('<b></b>'); s.appendChild(b); el.querySelector('.cam-monstats').appendChild(s); return [b, f]; });
      if (!ss.length) el.querySelector('.cam-monstats').remove();
      upd.push(() => { for (const [g, f] of gs) { const [fr, t, u, hot] = f(); g.set(fr, t, u, hot); } for (const [b, f] of ss) b.textContent = f(); });
      grid.appendChild(el);
    }
    host.appendChild(grid);
    const tick = () => { for (const f of upd) f(); };
    track(S.on('state', tick));
    track(K.on('prefs', tick));
    tick();
    host.appendChild(historyCharts());
  }

  /* History from the service: the last hour a second at a time, and 30 days of
     minutes. A small chart for each kind of reading, all over the range picked above
     them. The pointer over one shows that moment in all of them. */
  const HIST_RANGES = [
    { value: 600, label: '10 min' }, { value: 3600, label: '1 hour' }, { value: 86400, label: '24 hours' },
    { value: 604800, label: '7 days' }, { value: 2592000, label: '30 days' },
  ];
  const hasSensor = (id) => S.sensors().some((s) => s.id === id);

  // The charts this PC has readings for, each with its series and the reading now.
  function histPanels() {
    const sensorsOf = (pairs) => pairs.filter(([id]) => hasSensor(id)).map(([id, label]) => ({ id, label, now: () => S.value(id) }));
    const liquid = S.sensors().filter((s) => S.isLiquid(s.id)).map((s) => {
      const d = S.device(s.id.split('.')[0]);
      return { id: s.id, label: d ? d.name : S.sourceLabel(s.id), now: () => S.value(s.id) };
    });
    const fans = S.fans().filter((f) => f.hw && f.hw.connected !== false)
      .map((f) => ({ id: `${f.id}.rpm`, label: fanLabel(f.id), now: () => { const x = S.fan(f.id); return x && x.hw ? x.hw.rpm : null; } }));
    return [
      { id: 'liquid', title: 'Liquid temperature', kind: 'temp', series: liquid, area: true, mark: 55 },
      { id: 'temp', title: 'CPU and GPU temperature', kind: 'temp', series: sensorsOf([['cpu.temp', 'CPU'], ['gpu.temp', 'GPU']]) },
      { id: 'load', title: 'Load', kind: 'load', series: sensorsOf([['cpu.load', 'CPU'], ['gpu.load', 'GPU'], ['ram.load', 'Memory']]) },
      { id: 'power', title: 'Power', kind: 'power', series: sensorsOf([['cpu.power', 'CPU'], ['gpu.power', 'GPU']]) },
      { id: 'fans', title: 'Fan and pump speeds', kind: 'rpm', series: fans },
      // Frame rates are recorded only while Games is on.
      gamesOn() ? { id: 'fps', title: 'Frame rate', kind: 'fps', area: true, series: [{ id: 'fps', label: 'Game in front', now: () => { const g = S.status && S.status.game; return g ? g.fps : null; } }] } : null,
    ].filter((p) => p && p.series.length);
  }

  // A temperature is drawn in the unit picked in Settings, so its grid lines are round.
  const histConv = (kind) => (kind === 'temp' ? K.toUnit : (v) => v);
  const NICE = [0.5, 1, 2, 2.5, 5, 10, 20, 25, 50, 100, 200, 250, 500, 1000, 2000, 2500, 5000, 10000];

  // { lo, hi, step, label } of an axis around the readings from lo0 to hi0 (drawn units).
  function histAxis(kind, lo0, hi0) {
    if (kind === 'load') return { lo: 0, hi: 100, step: 25, label: (v) => `${v}%` };
    if (kind === 'temp') {
      // Close around the readings, so that a change of a degree shows, and at least
      // 10 degrees tall, so that noise does not fill the chart.
      let a = lo0 ?? 30, b = hi0 ?? 50;
      const pad = Math.max(1, (b - a) * 0.15);
      a -= pad; b += pad;
      const least = K.prefs.unit === 'f' ? 18 : 10;
      if (b - a < least) { const m = (a + b) / 2; a = m - least / 2; b = m + least / 2; }
      const step = NICE.find((k) => (b - a) / k <= 5);
      return { lo: Math.floor(a / step) * step, hi: Math.ceil(b / step) * step, step, label: (v) => `${Math.round(v)}°` };
    }
    const least = kind === 'power' ? 50 : kind === 'rpm' ? 1000 : 60;
    const want = Math.max(least, (hi0 ?? 0) * 1.1);
    const step = NICE.find((k) => want / k <= 5);
    const label = kind === 'power' ? (v) => `${v} W` : kind === 'rpm' ? (v) => K.rpm(v) : (v) => String(v);
    return { lo: 0, hi: Math.ceil(want / step) * step, step, label };
  }

  function histValue(kind, id, v) {
    if (v == null) return 'N/A';
    if (kind === 'rpm') return `${K.rpm(v)} RPM`;
    if (kind === 'fps') return `${Math.round(v)} fps`;
    return reading(id, v, kind === 'temp' ? 1 : 0);
  }

  /* The charts over the range picked above them, or over a window of their own, `win`:
     { from, to, panels }, a game session's in ms. A window without an end goes on to
     now and loads again every few seconds, for the game being played. A session's
     charts are only those with readings. */
  function historyCharts(win) {
    const el = h('<section class="cam-section cam-hist"><header><h2 class="cam-h2"></h2><span class="grow"></span></header><p class="cam-hint cam-hist-note" hidden></p><div class="cam-hgrid"></div></section>');
    el.querySelector('h2').textContent = win ? 'During the session' : 'History';
    const grid = el.querySelector('.cam-hgrid'), note = el.querySelector('.cam-hist-note');
    let span = HIST_RANGES.some((r) => r.value === K.prefs.histSpan) ? K.prefs.histSpan : 600;
    let panels = [], key = '', data = null, live = [], hoverT = null, asked = 0;
    // What the legends switched off; a session's are not kept.
    const hidden = new Set(win ? [] : K.prefs.histHidden || []);
    if (!win) el.firstElementChild.appendChild(choice({ options: HIST_RANGES, value: span, label: 'Range', onChange: (v) => { span = v; K.setPref('histSpan', v); load(); } }));
    // The time the charts cover, in ms.
    const range = () => { const now = Date.now(); return win ? [win.from, win.to ?? now] : [now - span * 1000, now]; };
    const during = () => (win ? 'during the session' : `over the last ${HIST_RANGES.find((r) => r.value === span).label}`);
    const keyOf = (list) => list.map((p) => `${p.id}:${p.series.map((s) => s.id).join(',')}`).join('|');
    const has = (id) => !!data && !!data.series[id] && data.series[id].avg.some((v) => v != null);
    const wanted = () => (win ? win.panels.map((p) => ({ ...p, series: p.series.filter((s) => has(s.id)) })).filter((p) => p.series.length) : histPanels());

    function build(list) {
      key = keyOf(list);
      grid.textContent = '';
      panels = list.map((p) => {
        const box = h(`<section class="cam-panel cam-hpanel"><header><h3></h3><div class="cam-hlegend"></div></header><div class="cam-chart-plot"><canvas role="img"></canvas><div class="cam-chart-tip" hidden></div><p class="cam-chart-note" hidden>No readings in this range.</p></div></section>`);
        box.querySelector('h3').textContent = p.title;
        const cv = box.querySelector('canvas');
        const items = p.series.map((s, i) => {
          const b = h('<button type="button" class="cam-legend"><i></i><span></span><b class="num"></b><small class="num"></small></button>');
          b.children[1].textContent = s.label;
          b.firstElementChild.style.setProperty('--c', seriesColor(i));
          b.setAttribute('aria-pressed', String(!hidden.has(s.id)));
          b.addEventListener('click', () => {
            if (hidden.has(s.id)) hidden.delete(s.id); else hidden.add(s.id);
            if (!win) K.setPref('histHidden', [...hidden]);
            b.setAttribute('aria-pressed', String(!hidden.has(s.id)));
            drawAll();
          });
          box.querySelector('.cam-hlegend').appendChild(b);
          return { s, b, now: b.children[2], stat: b.children[3] };
        });
        const panel = Object.assign({}, p, { box, cv, items, tip: box.querySelector('.cam-chart-tip'), empty: box.querySelector('.cam-chart-note') });
        cv.addEventListener('pointermove', (e) => { const r = cv.getBoundingClientRect(); hoverT = panel.tAt ? panel.tAt(e.clientX - r.left) : null; drawAll(); });
        cv.addEventListener('pointerleave', () => { hoverT = null; drawAll(); });
        grid.appendChild(box);
        return panel;
      });
    }

    async function load() {
      const ask = ++asked;
      const ids = [...new Set((win ? win.panels : panels).flatMap((p) => p.series.map((s) => s.id)))].map(encodeURIComponent).join(',');
      const [t0, t1] = range();
      const window_ = win ? `from=${Math.floor(t0 / 1000)}&to=${Math.ceil(t1 / 1000)}` : `span=${span}`;
      let r = null, err = null;
      if (ids && S.status) {
        try { r = await S.api('GET', `history?${window_}&ids=${ids}`); } catch (e) { err = e; }
      }
      if (ask !== asked) return;
      data = r;
      live = [];
      if (win && keyOf(wanted()) !== key) build(wanted());
      const step = data ? data.step : 0;
      note.textContent = !S.status ? (win ? 'The service is not running, so the readings of the session cannot be read now.' : 'The service records history while it runs. It is not running now.')
        : err ? `No history: ${err.message}`
        : !panels.length ? (win ? 'No readings were kept for this session: camless keeps them for 30 days, while the service runs.' : 'Nothing to show here on this PC.')
        : win && step >= 60 ? `Every point stands for ${step === 60 ? 'a minute' : `${step / 60} minutes`}: the line is the average, and the shade above it how high the readings went.`
        : !win && span > 3600 ? 'Every point stands for several minutes: the line is their average, and the shade above it how high they went.' : '';
      note.hidden = !note.textContent;
      drawAll();
    }

    // [{ t (ms), v: { id: [average, highest] } }], oldest first: the service's points,
    // then the readings since they were loaded.
    function points() {
      const out = [];
      if (data) {
        const ids = Object.keys(data.series);
        for (let i = 0; i < data.count; i++) {
          const v = {};
          for (const id of ids) { const x = data.series[id]; if (x.avg[i] != null) v[id] = [x.avg[i], x.max[i]]; }
          out.push({ t: (data.from + i * data.step) * 1000, v });
        }
      }
      const after = out.length ? out[out.length - 1].t + data.step * 1000 : 0;
      return out.concat(live.filter((p) => p.t >= after));
    }

    function drawAll() {
      const [t0, t1] = range();
      const pts = points().filter((p) => p.t >= t0 - (data ? data.step * 1000 : 0));
      for (const p of panels) drawPanel(p, pts, t0, t1);
    }

    function drawPanel(p, pts, t0, t1) {
      const { cv } = p;
      if (!cv.isConnected) return;
      const conv = histConv(p.kind);
      const shown = p.series.filter((s) => !hidden.has(s.id));
      const color = (s) => seriesColor(p.series.indexOf(s));
      // Per series over the range: lowest and mean of the averages, highest of the peaks.
      // `any` counts shown series only, for the axis; `has` any series, for the note.
      let lo0 = null, hi0 = null, any = false, has = false;
      for (const it of p.items) {
        let mn = Infinity, mx = -Infinity, sum = 0, n = 0;
        for (const pt of pts) {
          const v = pt.v[it.s.id];
          if (!v) continue;
          mn = Math.min(mn, v[0]); mx = Math.max(mx, v[1] ?? v[0]); sum += v[0]; n++;
        }
        it.now.textContent = it.s.now ? histValue(p.kind, it.s.id, it.s.now()) : '';
        it.stat.textContent = n && p.series.length <= 3 ? `avg ${histValue(p.kind, it.s.id, sum / n)}, max ${histValue(p.kind, it.s.id, mx)}` : '';
        it.b.title = n ? `${it.s.label} ${during()}: lowest ${histValue(p.kind, it.s.id, mn)}, average ${histValue(p.kind, it.s.id, sum / n)}, highest ${histValue(p.kind, it.s.id, mx)}` : it.s.label;
        if (n) has = true;
        if (n && !hidden.has(it.s.id)) { any = true; lo0 = Math.min(lo0 ?? Infinity, conv(mn)); hi0 = Math.max(hi0 ?? -Infinity, conv(mx)); }
      }
      p.empty.hidden = has || !data;
      const { ctx, w, h: hh } = D.fit(cv);
      ctx.clearRect(0, 0, w, hh);
      const css = getComputedStyle(cv);
      const gridC = css.getPropertyValue('--ce-grid').trim(), text = css.getPropertyValue('--t4').trim(), ink = css.getPropertyValue('--t3').trim();
      const ax = histAxis(p.kind, lo0, hi0);
      const pl = 46, pr = 8, pt = 8, pb = 20;
      const X = (t) => pl + ((t - t0) / (t1 - t0)) * (w - pl - pr);
      const Y = (v) => pt + (1 - (clamp(conv(v), ax.lo, ax.hi) - ax.lo) / (ax.hi - ax.lo)) * (hh - pt - pb);
      p.tAt = (x) => (x < pl || x > w - pr ? null : t0 + ((x - pl) / (w - pl - pr)) * (t1 - t0));
      ctx.font = `500 10.5px ${FONT}`;
      ctx.textAlign = 'right';
      for (let v = ax.lo; v <= ax.hi + 1e-6; v += ax.step) {
        const y = Math.round(pt + (1 - (v - ax.lo) / (ax.hi - ax.lo)) * (hh - pt - pb)) + 0.5;
        ctx.strokeStyle = gridC; ctx.lineWidth = 1;
        ctx.beginPath(); ctx.moveTo(pl, y); ctx.lineTo(w - pr, y); ctx.stroke();
        ctx.fillStyle = text; ctx.fillText(ax.label(Math.round(v * 10) / 10), pl - 8, y + 3.5);
      }
      // Time labels from the right, leaving out those that would touch the one before.
      ctx.textAlign = 'center';
      let room = Infinity;
      const ticks = win ? clockTicks(t0, t1) : histTicks(span, t1);
      for (const [x, s] of ticks.map(([t, s]) => [clamp(X(t), pl + 18, w - pr - 18), s]).sort((a, b) => b[0] - a[0])) {
        const half = ctx.measureText(s).width / 2;
        if (x + half > room - 8) continue;
        ctx.fillText(s, x, hh - 5);
        room = x - half;
      }
      // Where camless sends every fan to 100%, when the readings come near it.
      if (p.mark != null && conv(p.mark) >= ax.lo && conv(p.mark) <= ax.hi) {
        const y = Math.round(Y(p.mark)) + 0.5;
        ctx.save();
        ctx.strokeStyle = css.getPropertyValue('--alert').trim(); ctx.globalAlpha = 0.7; ctx.setLineDash([4, 4]);
        ctx.beginPath(); ctx.moveTo(pl, y); ctx.lineTo(w - pr, y); ctx.stroke();
        ctx.setLineDash([]); ctx.globalAlpha = 1; ctx.fillStyle = css.getPropertyValue('--alert').trim(); ctx.textAlign = 'left';
        ctx.fillText(`${K.temp(p.mark, 0)}: every fan at 100%`, pl + 6, y - 5);
        ctx.restore();
      }
      // A gap in the readings, such as the service being off, breaks the line.
      const tick = (S.config && S.config.interval_ms) || 1000;
      const gap = Math.max(3e3, tick * 2.5, (data ? data.step : 1) * 2000);
      const runs = (s) => {
        const out = [];
        let cur = null, last = 0;
        for (const pt of pts) {
          const v = pt.v[s.id];
          if (!v || pt.t - last > gap) cur = null;
          if (!v) continue;
          last = pt.t;
          if (!cur) out.push((cur = []));
          cur.push([X(pt.t), Y(v[0]), Y(v[1] ?? v[0])]);
        }
        return out;
      };
      const bottom = hh - pb;
      for (const s of shown) {
        const c = color(s), rs = runs(s);
        for (const r of rs) {
          // How high the readings went within each point, over several minutes a point.
          if (data && data.step > 1) {
            ctx.beginPath();
            r.forEach(([x, , ym], i) => (i ? ctx.lineTo(x, ym) : ctx.moveTo(x, ym)));
            for (let i = r.length - 1; i >= 0; i--) ctx.lineTo(r[i][0], r[i][1]);
            ctx.closePath();
            ctx.globalAlpha = shown.length === 1 ? 0.22 : 0.1; ctx.fillStyle = c; ctx.fill(); ctx.globalAlpha = 1;
          }
          if (p.area && shown.length === 1) {
            const g = ctx.createLinearGradient(0, pt, 0, bottom);
            const rgb = S.color.hexToRgb(c);
            g.addColorStop(0, S.color.css(rgb, 0.28)); g.addColorStop(1, S.color.css(rgb, 0));
            ctx.beginPath();
            r.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
            ctx.lineTo(r[r.length - 1][0], bottom); ctx.lineTo(r[0][0], bottom); ctx.closePath();
            ctx.fillStyle = g; ctx.fill();
          }
          ctx.strokeStyle = c; ctx.lineWidth = 1.8; ctx.lineJoin = 'round';
          ctx.beginPath();
          r.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
          ctx.stroke();
          if (r.length === 1) { ctx.fillStyle = c; ctx.beginPath(); ctx.arc(r[0][0], r[0][1], 2, 0, TAU); ctx.fill(); }
        }
      }
      cv.setAttribute('aria-label', `${p.title} ${during()}`);
      p.tip.hidden = true;
      if (hoverT == null || !pts.length) return;
      let best = null;
      for (const q of pts) if (!best || Math.abs(q.t - hoverT) < Math.abs(best.t - hoverT)) best = q;
      const on = shown.filter((s) => best.v[s.id]);
      const x = X(best.t);
      if (!on.length || x < pl || x > w - pr) return;
      ctx.strokeStyle = ink; ctx.lineWidth = 1;
      ctx.beginPath(); ctx.moveTo(Math.round(x) + 0.5, pt); ctx.lineTo(Math.round(x) + 0.5, bottom); ctx.stroke();
      for (const s of on) { ctx.fillStyle = color(s); ctx.beginPath(); ctx.arc(x, Y(best.v[s.id][0]), 3, 0, TAU); ctx.fill(); }
      // A point that stands for several samples also says how high it went.
      const many = data && data.step > 1 && best.t < (data.from + data.count * data.step) * 1000;
      p.tip.innerHTML = `<time>${esc(histTime(best.t, (t1 - t0) / 1000))}</time>` + on.map((s) => {
        const [avg, max] = best.v[s.id];
        const peak = many && max != null && histValue(p.kind, s.id, max) !== histValue(p.kind, s.id, avg) ? `<small>max ${esc(histValue(p.kind, s.id, max))}</small>` : '';
        return `<div><i style="--c:${color(s)}"></i><span>${esc(s.label)}</span><b>${esc(histValue(p.kind, s.id, avg))}</b>${peak}</div>`;
      }).join('');
      p.tip.hidden = false;
      const flip = x + 14 + p.tip.offsetWidth > w;
      p.tip.style.left = `${flip ? x - 14 - p.tip.offsetWidth : x + 14}px`;
    }

    // The last hour moves with every reading; every range loads again each minute, and
    // the session being played every few seconds.
    track(S.on('state', () => {
      if (!S.status) return drawAll();
      if (!data && note.textContent.startsWith('The service')) { load(); return; }
      if (win) return drawAll();
      const k = keyOf(histPanels());
      if (k !== key) { build(histPanels()); load(); return; }
      if (span <= 3600) {
        const v = {};
        for (const p of panels) for (const s of p.series) { const x = s.now(); if (x != null) v[s.id] = [x, x]; }
        live.push({ t: Date.now(), v });
        while (live.length && live[0].t < Date.now() - span * 1000) live.shift();
      }
      drawAll();
    }));
    const timer = setInterval(() => { if (!document.hidden && (!win || win.to == null)) load(); }, win ? 10e3 : 60e3);
    track(() => clearInterval(timer));
    track(K.on('prefs', (k) => { if (k === 'theme') build(win ? wanted() : histPanels()); drawAll(); }));
    const ro = new ResizeObserver(() => drawAll());
    ro.observe(grid);
    track(() => ro.disconnect());
    if (!win) build(histPanels());
    load();
    return el;
  }

  // Clock times across a window, about five of them: [ms, text].
  function clockTicks(t0, t1) {
    const every = [1, 2, 5, 10, 15, 30, 60, 120, 180, 360, 720, 1440].find((m) => (t1 - t0) / 60e3 / m <= 5) || 2880;
    const d = new Date(t0);
    d.setSeconds(0, 0);
    if (every < 60) d.setMinutes(Math.floor(d.getMinutes() / every) * every);
    else d.setHours(Math.floor(d.getHours() / (every / 60)) * (every / 60), 0);
    const out = [];
    for (let t = d.getTime(); t <= t1; t += every * 60e3) {
      if (t >= t0) out.push([t, new Date(t).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })]);
    }
    return out;
  }

  // Where the time axis is labelled: [ms, text].
  function histTicks(span, now) {
    const out = [];
    if (span <= 3600) {
      const every = span <= 600 ? 120 : 600;
      for (let s = 0; s <= span; s += every) out.push([now - s * 1000, s ? `-${s / 60} min` : 'now']);
      return out;
    }
    if (span <= 86400) {
      const hour = new Date(now);
      hour.setMinutes(0, 0, 0);
      for (let t = hour.getTime(); t > now - span * 1000; t -= 3600e3) {
        const d = new Date(t);
        if (d.getHours() % 4 === 0) out.push([t, d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })]);
      }
      return out;
    }
    const week = span <= 604800;
    const d = new Date(now);
    d.setHours(0, 0, 0, 0);
    for (let i = 0; d.getTime() > now - span * 1000; d.setDate(d.getDate() - 1), i++) {
      if (!week && i % 5) continue;
      out.push([d.getTime(), d.toLocaleDateString(undefined, week ? { weekday: 'short' } : { month: 'short', day: 'numeric' })]);
    }
    return out;
  }

  function histTime(t, span) {
    const d = new Date(t);
    if (span <= 3600) return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' });
    return d.toLocaleString(undefined, { weekday: 'short', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' });
  }

  /* ----------------------------------------------------------------- games */

  // "1 h 04 min", or "12 min" under an hour.
  function playTime(s) {
    const m = Math.floor((s || 0) / 60);
    return m >= 60 ? `${Math.floor(m / 60)} h ${String(m % 60).padStart(2, '0')} min` : `${m} min`;
  }
  const fpsText = (v) => (v == null ? 'N/A' : String(Math.round(v)));
  // The highest temperature of a session among the sensors `pick` takes, or null.
  function sessionPeak(s, pick) {
    let top = null;
    for (const [id, t] of Object.entries(s.temps || {})) if (pick(id) && (top == null || t.max > top)) top = t.max;
    return top;
  }
  const PEAKS = [['CPU max', (id) => id === 'cpu.temp'], ['GPU max', (id) => id === 'gpu.temp'], ['Liquid max', S.isLiquid]];
  const clock = (ms) => new Date(ms).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  const shortDay = (ms) => new Date(ms).toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
  const plural = (n, one) => `${n} ${one}${n === 1 ? '' : 's'}`;
  // Today, Yesterday, or the date.
  function dayLabel(ms) {
    const d = new Date(ms), today = new Date();
    d.setHours(0, 0, 0, 0);
    today.setHours(0, 0, 0, 0);
    if (d.getTime() === today.getTime()) return 'Today';
    today.setDate(today.getDate() - 1);
    if (d.getTime() === today.getTime()) return 'Yesterday';
    return d.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long', year: d.getFullYear() === today.getFullYear() ? undefined : 'numeric' });
  }

  // Each game's sessions, play time and frames, when it was last played and its Steam
  // app id. The average weighs each session by its frames, and so does the 1 % low,
  // which is the sessions' lows averaged, not the low of all the frames together.
  function byGame(sessions) {
    const m = new Map();
    for (const s of sessions) {
      const g = m.get(s.game) || { game: s.game, n: 0, played: 0, frames: 0, secs: 0, lows: 0, lowFrames: 0, last: 0, steam: null };
      g.n++;
      g.played += s.played;
      g.last = Math.max(g.last, s.end);
      g.steam = g.steam || s.steam || null;
      if (s.fps) { g.frames += s.frames; g.secs += s.frames / s.fps; }
      if (s.low1 && s.frames) { g.lows += s.low1 * s.frames; g.lowFrames += s.frames; }
      m.set(s.game, g);
    }
    return [...m.values()].sort((a, b) => b.last - a.last);
  }
  const avgFps = (g) => (g.secs ? g.frames / g.secs : null);

  // A game's initials on a colour of its own, under its cover from Steam's cache on
  // this PC when there is one. Nothing is downloaded.
  function cover(name, steam, cls) {
    const el = h(`<div class="cam-cover${cls ? ` ${cls}` : ''}" aria-hidden="true"><span></span></div>`);
    let hash = 7;
    for (const c of name) hash = (hash * 31 + c.codePointAt(0)) >>> 0;
    el.style.setProperty('--hue', String(hash % 360));
    el.firstElementChild.textContent = name.split(/[\s:_\-–.]+/).filter(Boolean).slice(0, 2).map((w) => [...w][0]).join('').toUpperCase();
    if (steam) {
      const img = new Image();
      img.alt = '';
      img.addEventListener('load', () => { el.classList.add('art'); el.appendChild(img); });
      img.src = `/api/game-art?steam=${encodeURIComponent(steam)}`;
    }
    return el;
  }

  // A card for each game; picking one shows only its sessions.
  function gameCards(list, picked, pick) {
    const el = h('<div class="cam-gcards"></div>');
    for (const g of list) {
      const b = h('<button type="button" class="cam-panel cam-gcard"><div class="body"><h3></h3><p class="fps"></p><p class="meta"></p></div></button>');
      b.prepend(cover(g.game, g.steam));
      b.querySelector('h3').textContent = g.game;
      const avg = avgFps(g), fps = b.querySelector('.fps');
      if (avg == null) fps.innerHTML = '<span>No frame rates</span>';
      else {
        fps.innerHTML = `<b class="num">${fpsText(avg)}</b><span>avg FPS</span>${g.lowFrames ? `<small class="num">1% low ${fpsText(g.lows / g.lowFrames)}</small>` : ''}`;
        if (g.lowFrames) fps.title = 'The 1% lows of its sessions, averaged over their frames.';
      }
      b.querySelector('.meta').textContent = `${playTime(g.played)} · ${plural(g.n, 'session')} · ${shortDay(g.last)}`;
      b.setAttribute('aria-pressed', String(picked === g.game));
      b.addEventListener('click', () => pick(picked === g.game ? null : g.game));
      el.appendChild(b);
    }
    return el;
  }

  // Sessions under a heading for each day, newest first; each opens its own view.
  function sessionList(list, open) {
    const el = h('<div class="cam-slist"></div>');
    let day = null, box = null;
    for (const s of list) {
      const d = dayLabel(s.start);
      if (d !== day) {
        day = d;
        const head = h('<h3 class="cam-sday"></h3>');
        head.textContent = d;
        box = h('<div class="cam-panel cam-sbox"></div>');
        el.append(head, box);
      }
      const b = h('<button type="button" class="cam-srow"><span class="what"><b></b><small></small></span></button>');
      b.prepend(cover(s.game, s.steam, 'small'));
      b.querySelector('.what b').textContent = s.game;
      b.querySelector('.what small').textContent = `${clock(s.start)} – ${clock(s.end)} · ${playTime(s.played)}`;
      const stats = [['avg FPS', fpsText(s.fps)], ['1% low', fpsText(s.low1)], ...PEAKS.map(([label, pick]) => {
        const v = sessionPeak(s, pick);
        return [label, v == null ? 'N/A' : K.temp(v, 0), 'temp'];
      })];
      for (const [label, v, cls] of stats) {
        const st = h(`<span class="st${cls ? ` ${cls}` : ''}"><b class="num"></b><small></small></span>`);
        st.firstElementChild.textContent = v;
        st.lastElementChild.textContent = label;
        b.appendChild(st);
      }
      b.insertAdjacentHTML('beforeend', `<span class="go">${I.chevron}</span>`);
      b.addEventListener('click', () => open(s));
      box.appendChild(b);
    }
    return el;
  }

  // The charts of a session: the frame rate, then what it may have held it back.
  function sessionPanels(s) {
    const liquids = [...new Set([...Object.keys(s.temps || {}), ...S.sensors().map((x) => x.id)])].filter(S.isLiquid);
    const liquid = liquids.map((id) => { const d = S.device(id.split('.')[0]); return { id, label: d ? d.name : id.split('.')[0] }; });
    return [
      { id: 'fps', title: 'Frame rate', kind: 'fps', area: true, series: [{ id: 'fps', label: 'FPS' }] },
      { id: 'temp', title: 'CPU and GPU temperature', kind: 'temp', series: [{ id: 'cpu.temp', label: 'CPU' }, { id: 'gpu.temp', label: 'GPU' }] },
      { id: 'liquid', title: 'Liquid temperature', kind: 'temp', area: true, mark: 55, series: liquid },
      { id: 'load', title: 'Load', kind: 'load', series: [{ id: 'cpu.load', label: 'CPU' }, { id: 'gpu.load', label: 'GPU' }] },
    ].filter((p) => p.series.length);
  }

  /* One session: what it came to, and the readings over it from the history. `s` is a
     kept session, or { live: true, ... } for the game being played, which goes on
     until it closes. `others` are the game's other sessions, to compare with. */
  function sessionView(host, s, others, back) {
    const bar = h('<div class="cam-sback"></div>');
    bar.appendChild(button('All sessions', { small: true, onClick: back }));
    const hero = h('<section class="cam-panel cam-shero"><div class="info"><h2></h2><p class="when"></p><div class="cam-stiles"></div></div></section>');
    hero.prepend(cover(s.game, s.steam, 'big'));
    hero.querySelector('h2').textContent = s.game;
    const whenEl = hero.querySelector('.when'), tiles = hero.querySelector('.cam-stiles');
    const tile = (label) => {
      const t = h('<div><small></small><b class="num"></b><span></span></div>');
      t.firstElementChild.textContent = label;
      tiles.appendChild(t);
      return t;
    };
    const set = (t, v, sub) => { t.children[1].textContent = v; t.children[2].textContent = sub || ''; };
    const [played, avg, low1, low01] = ['Played', 'Average FPS', '1% low', '0.1% low'].map(tile);
    host.append(bar, hero);
    // How this session's frame rate compares with the game's others.
    const usual = avgFps(byGame(others)[0] || {});
    const versus = (fps) => {
      if (fps == null || usual == null) return '';
      const d = Math.round(((fps - usual) / usual) * 100);
      return d === 0 ? 'as usual for this game' : `${d > 0 ? '+' : ''}${d}% vs its other sessions`;
    };
    const win = { from: s.start, to: s.live ? null : s.end, panels: sessionPanels(s) };
    function fill() {
      const g = s.live && S.status && S.status.game;
      if (s.live && g && g.start === s.start) Object.assign(s, { played: g.played, fps: g.avg_fps, low1: g.low1 });
      else if (s.live && win.to == null) win.to = Date.now();
      const end = win.to == null ? 'now' : clock(win.to);
      whenEl.textContent = `${dayLabel(s.start)}, ${clock(s.start)} – ${end}${s.exe ? ` · ${s.exe}` : ''}${s.live && win.to != null ? ' · the game has closed' : ''}`;
      set(played, playTime(s.played));
      set(avg, fpsText(s.fps), versus(s.fps));
      set(low1, fpsText(s.low1));
      set(low01, fpsText(s.low01));
    }
    for (const [label, pick] of PEAKS) {
      const v = sessionPeak(s, pick);
      if (v != null) set(tile(label), K.temp(v, 0));
    }
    fill();
    if (s.live) track(S.on('state', fill));
    host.appendChild(historyCharts(win));
    // Every temperature the session kept, also once the history of it is gone.
    const temps = Object.entries(s.temps || {});
    if (temps.length) {
      const order = (id) => (id === 'cpu.temp' ? 0 : id === 'gpu.temp' ? 1 : S.isLiquid(id) ? 2 : 3);
      temps.sort(([a], [b]) => order(a) - order(b) || a.localeCompare(b));
      K.section(host, 'Temperatures while in front').appendChild(table([
        { label: 'Sensor', value: ([id]) => S.sourceLabel(id) },
        { label: 'Average', num: true, value: ([, t]) => K.temp(t.avg, 1) },
        { label: 'Highest', num: true, value: ([, t]) => K.temp(t.max, 1) },
      ], temps));
    }
  }

  // A table of `rows`, a column for each of `cols`: { label, value(row), num }.
  function table(cols, rows) {
    const el = h('<div class="cam-panel cam-tablewrap"><table class="cam-table"><thead><tr></tr></thead><tbody></tbody></table></div>');
    const head = el.querySelector('tr'), body = el.querySelector('tbody');
    for (const c of cols) { const th = h(`<th scope="col"${c.num ? ' class="num"' : ''}></th>`); th.textContent = c.label; head.appendChild(th); }
    for (const r of rows) {
      const tr = h('<tr></tr>');
      cols.forEach((c, i) => { const td = h(`<td class="${c.num ? 'num' : i === 0 ? 'name' : ''}"></td>`); td.textContent = c.value(r); tr.appendChild(td); });
      body.appendChild(tr);
    }
    return el;
  }

  function games(host) {
    K.pageHead(host, 'Games');
    const body = h('<div></div>');
    host.appendChild(body);
    const draw = K.region(body);
    let sessions = null, err = null, all = false, picked = null, had = false, asked = 0, shown = '';

    const open = (s) => { gameOpen = s === 'now' ? 'now' : s.start; show(); main.scrollTop = 0; };
    // A game's Steam app id from any of its sessions, for those kept before camless
    // recorded it.
    const steamOf = (game) => { const x = (sessions || []).find((y) => y.game === game && y.steam); return x ? x.steam : null; };
    const back = () => { gameOpen = null; show(); };

    // The list, or the session that is open.
    function show() {
      const g = S.status && S.status.game;
      if (gameOpen === 'now' && g) {
        const s = { live: true, game: g.name, exe: g.exe, steam: g.steam || steamOf(g.name), start: g.start, end: null, played: g.played, fps: g.avg_fps, low1: g.low1, low01: null, temps: {} };
        shown = 'now';
        draw((el) => sessionView(el, s, (sessions || []).filter((x) => x.game === g.name), back));
        return;
      }
      const s = gameOpen != null && sessions && sessions.find((x) => x.start === gameOpen);
      if (s) {
        s.steam = s.steam || steamOf(s.game);
        shown = `s${s.start}`;
        draw((el) => sessionView(el, s, sessions.filter((x) => x.game === s.game && x !== s), back));
        return;
      }
      // Until the sessions load, the one asked for.
      if (gameOpen !== 'now' && gameOpen != null && !sessions && !err && S.status) return;
      gameOpen = null;
      shown = 'list';
      draw(list);
    }

    function list(el) {
      const off = h('<div class="cam-panel cam-gamenote" hidden></div>');
      const now = h('<section class="cam-panel cam-moncard cam-game"><header><h3></h3><span></span><span class="grow"></span></header><div class="cam-monstats"></div><p class="cam-muted"></p></section>');
      el.append(off, now);
      const head = now.querySelector('header'), stats = now.querySelector('.cam-monstats'), idle = now.querySelector('p');
      const stat = (label) => { const x = h('<span></span>'); x.textContent = label; const b = h('<b></b>'); x.appendChild(b); stats.appendChild(x); return b; };
      const [fpsB, avgB, lowB, playedB] = ['FPS', 'Average', '1% low', 'Played'].map(stat);
      const charts = button('Open session', { small: true, onClick: () => open('now') });
      head.appendChild(charts);
      let art = null;
      const lists = h('<div></div>');
      const hint = h('<p class="cam-hint cam-gamehint"></p>');
      hint.innerHTML = 'Games from Steam, Epic Games and Xbox are found on their own. For another one, add its program or folder to <code>[games] add</code> in camless.toml; <code>[games] skip</code> leaves one out. A session ends when the game closes or after 3 minutes in the background. Sessions under a minute are not kept. Covers come from Steam\'s own cache on this PC; camless downloads none.';
      el.append(lists, hint);

      function live() {
        const g = S.status && S.status.game, info = S.status && S.status.service, why = info && info.fps_error;
        off.hidden = !why;
        off.textContent = !why ? '' : why === 'needs administrator rights'
          ? 'Frame rates need the service to run as administrator. Play time and temperatures are still recorded.'
          : `No frame rates: ${why}. Play time and temperatures are still recorded.`;
        now.querySelector('h3').textContent = g ? g.name : 'Now playing';
        head.children[1].textContent = g ? (g.in_front ? g.exe : `${g.exe}, in the background`) : '';
        charts.hidden = !g;
        const want = g ? `${g.name}|${g.steam || steamOf(g.name) || ''}` : '';
        if ((art ? art.dataset.key : '') !== want) {
          if (art) art.remove();
          art = g ? cover(g.name, g.steam || steamOf(g.name), 'small') : null;
          if (art) { art.dataset.key = want; now.prepend(art); }
          now.classList.toggle('has-art', !!art);
        }
        stats.hidden = !g;
        idle.hidden = !!g;
        idle.textContent = S.status ? 'No game is running.' : 'Games show up here while the camless service runs.';
        if (g) {
          fpsB.textContent = fpsText(g.fps);
          avgB.textContent = fpsText(g.avg_fps);
          lowB.textContent = fpsText(g.low1);
          playedB.textContent = playTime(g.played);
        }
      }

      function render() {
        lists.textContent = '';
        if (err) { emptyState(lists, `The sessions could not be loaded: ${esc(err.message)}`); return; }
        if (!sessions) return;
        if (!sessions.length) { emptyState(lists, 'No sessions yet. A game played for a minute or more shows up here.'); return; }
        for (const x of sessions) x.steam = x.steam || steamOf(x.game);
        const gamesList = byGame(sessions);
        if (picked && !gamesList.some((g) => g.game === picked)) picked = null;
        K.section(lists, 'Your games').appendChild(gameCards(gamesList, picked, (g) => { picked = g; all = false; render(); }));
        const mine = picked ? sessions.filter((s) => s.game === picked) : sessions;
        const shownList = all ? mine : mine.slice(0, 20);
        const extra = [];
        if (picked) extra.push(button('All games', { small: true, onClick: () => { picked = null; render(); } }));
        if (mine.length > shownList.length) extra.push(button(`Show all ${mine.length}`, { small: true, onClick: () => { all = true; render(); } }));
        K.section(lists, picked ? `Sessions of ${picked}` : 'Sessions', extra).appendChild(sessionList(shownList, open));
      }
      track(S.on('state', live));
      track(K.on('prefs', (k) => { if (k === 'unit') render(); }));
      live();
      render();
    }

    async function load() {
      const ask = ++asked;
      let r = null, e = null;
      try { r = await S.api('GET', 'games'); } catch (x) { e = x; }
      if (ask !== asked || !body.isConnected) return;
      sessions = r ? r.sessions : null;
      err = e;
      // A session that just ended is in the list now; the open one stays as it is.
      if (shown === 'list' || shown === '') show();
    }

    track(S.on('state', () => {
      const g = S.status && S.status.game;
      // A session that just ended is in the list now, and so is a service that came back.
      if ((had && !g) || (S.status && !sessions && !err && !asked)) load();
      had = !!g;
    }));
    show();
    if (S.status) load();
  }

  /* -------------------------------------------------------------- settings */

  function settings(host) {
    K.pageHead(host, 'Settings');
    const sv = S.server;
    const panel = (title, rows) => { const s = K.section(host, title); const p = h('<div class="cam-panel cam-rows"></div>'); p.append(...rows.filter(Boolean)); s.appendChild(p); return s; };
    const editCfg = (fn, o) => S.edit(fn, Object.assign({ what: 'settings' }, o));

    // The switches show what Windows has, read again every second.
    const sandboxHint = 'In this test copy the switch changes nothing in Windows.';
    const autoHint = () => sv.sandbox ? sandboxHint : `camless starts in the tray when you sign in, and starts the service${S.server.admin_task ? ' as administrator.' : ', asking Windows for administrator rights.'}`;
    const autoRow = row('Start with Windows', autoHint(), null);
    const autoT = toggle({ checked: sv.autostart, label: 'Start with Windows', onChange: (v) => setAutostart(v, autoT) });
    autoRow.lastElementChild.appendChild(autoT);
    track(S.on('state', () => { if (!autoT.dataset.busy) autoT.set(S.server.autostart); autoRow.hint(autoHint()); }));

    panel('Service', [serviceRow(), adminRow()]);

    panel('General', [
      row('Temperature display', null, choice({ options: [{ value: 'c', label: 'Celsius' }, { value: 'f', label: 'Fahrenheit' }], value: K.prefs.unit, label: 'Temperature display', onChange: (v) => K.setPref('unit', v) })),
      row('Dark mode', null, toggle({ checked: K.prefs.theme !== 'light', label: 'Dark mode', onChange: (v) => K.setPref('theme', v ? 'dark' : 'light') })),
      row('Animations', 'How smoothly fans and lights move in this window. Lower uses less CPU and GPU. It slows down by itself when the window is not in use.', choice({
        options: [{ value: 'smooth', label: 'Smooth' }, { value: 'balanced', label: 'Balanced' }, { value: 'low', label: 'Low' }, { value: 'off', label: 'Off' }],
        value: K.prefs.motion || 'balanced', label: 'Animations', onChange: (v) => K.setPref('motion', v),
      })),
      autoRow,
      row('Closing the window', 'camless stays in the tray, and fans keep following their curves. Exit from the tray icon’s menu; the service keeps running either way.', null),
    ]);

    if (S.config) {
      const c = S.config;
      const fsMin = Math.min(30, c.failsafe_duty);
      const intervals = [250, 500, 1000, 2000, 5000];
      if (!intervals.includes(c.interval_ms)) intervals.push(c.interval_ms);
      intervals.sort((a, b) => a - b);
      const hot = () => `The Kraken raises its own pump and fan speeds from ${K.temp(45, 0)} liquid, even when camless is closed. At ${K.temp(55, 0)} every fan camless controls runs at 100%. This is always on.`;
      const hotRow = row('Hot liquid', hot(), h('<span class="cam-status">Always on</span>'));
      track(K.on('prefs', (k) => { if (k === 'unit') hotRow.hint(hot()); }));
      panel('Fan safety', [
        row('Failsafe speed', 'Channels whose sensor has no reading hold this speed, and so does every channel camless sets when the service stops.', slider({ min: fsMin, max: 100, value: c.failsafe_duty, label: 'Failsafe speed', format: (v) => v + '%', onInput: (v) => editCfg((x) => { x.failsafe_duty = v; }, { live: true }), onChange: () => S.save() })),
        row('Update interval', 'How often camless reads the sensors and sets the speeds.', select({ options: intervals.map((v) => ({ value: v, label: v < 1000 ? `${v} ms` : `${v / 1000} s` })), value: c.interval_ms, label: 'Update interval', onChange: (v) => editCfg((x) => { x.interval_ms = v; }) })),
        hotRow,
      ]);

      const api = Object.assign({}, API_DEFAULT, c.api);
      panel('Local API', [
        row('Local API', 'This window and LCD pages such as KrakenDial read the service through it. Only this PC can reach it. A change takes effect when the service restarts.', toggle({
          checked: api.enabled !== false, label: 'Local API',
          onChange: async (v) => {
            if (!v && !(await confirmBox({ title: 'Turn off Local API', text: 'After the service restarts, this window can no longer show readings or the service’s state, and pages on the LCD stop working. Settings are still saved and applied.', ok: 'Turn off' }))) { go(); return; }
            editCfg((x) => { x.api = Object.assign({}, x.api || { listen: api.listen }, { enabled: v }); if (v && x.api.listen === API_DEFAULT.listen) delete x.api; });
          },
        })),
        row('Address', null, h(`<code>http://${esc(api.listen)}</code>`)),
      ]);
    }

    if (S.config) {
      const gamesRow = row('Games', '', toggle({
        checked: gamesOn(), label: 'Games',
        onChange: (v) => {
          editCfg((x) => { x.games = Object.assign({}, x.games, { enabled: v }); if (!v) delete x.games.enabled; if (!Object.keys(x.games).length) delete x.games; });
          toast(v ? 'Games is at the bottom of the sidebar now.' : 'Games is off. Sessions recorded so far stay on this PC.');
        },
      }));
      gamesRow.hint('New, and off until you turn it on. The service then watches which program is in front and keeps each game session: play time, frame rates and temperatures. Frame rates need the service to run as administrator. Sessions stay on this PC.');
      panel('Experimental', [gamesRow]);
    }

    panel('Profiles and backup', [
      S.config ? row('Profiles', `Running: <b>${esc((S.config.profile) || 'no profile')}</b>.`, button('Manage profiles', { small: true, onClick: profilesDialog })) : null,
      S.config ? row('Import from NZXT CAM', "Make a profile from CAM's curves, fan names, lighting and LCD mode.", button('Import', { small: true, onClick: importCam })) : null,
      S.config ? row('Import a file', 'Make a profile from a <code>camless.toml</code> saved earlier.', button('Import file', { small: true, onClick: importFile })) : null,
      row('Config folder', `<code>${esc(sv.config_path)}</code>`, sv.sandbox ? null : button('Open folder', { small: true, onClick: () => act('open-folder', {}) })),
    ]);

    const devRows = S.devices().map((d) => row(d.name, `Firmware ${esc(d.firmware || 'unknown')}. ${d.verified ? 'Verified: reported working on this model.' : `${d.experimental ? EXPERIMENTAL : 'Not verified yet: this driver follows other projects’ notes and captures of CAM.'} <button type="button" class="cam-link" data-a="verify">Tell us how it works</button>`}`, deviceStatus(d.id)));
    for (const r of devRows) r.addEventListener('click', (e) => { if (e.target.closest('[data-a=verify]')) verifyDialog(); });
    const unsupported = ((S.status && S.status.unsupported) || []).map((u) => row(u.product || 'NZXT device', `No camless driver yet (${usbId(u)}).`, null));
    panel('Devices', devRows.concat(unsupported, [
      !devRows.length && !unsupported.length ? row(S.status ? 'No devices found' : 'The service is not running', null, null) : null,
      row('Hardware report', 'A device camless does not drive yet, a wrong reading, or an LCD that stays dark: a report opens a GitHub issue with what a fix needs. You see all of it first.', button('Create report', { small: true, onClick: reportDialog })),
      row('Firmware updates', 'camless shows firmware versions but does not install updates. To update, open NZXT CAM once.', null),
    ]));

    panel('About', [
      row('camless', `Window ${esc(sv.app_version)}${S.status ? `, service ${esc(S.status.version)}` : ''}.`, null),
      S.config ? channelRow() : null,
      updateRow(),
    ]);
  }

  // Stable offers releases only; Beta offers betas too, the GitHub pre-releases such
  // as 0.5.0-beta.1. Switching checks again at once.
  function channelRow() {
    return row('Update channel', 'Beta also offers betas: new features before a release, tried by fewer people. Back on Stable, camless keeps the version it has until a newer release comes out.', select({
      options: [{ value: 'stable', label: 'Stable' }, { value: 'beta', label: 'Beta' }],
      value: S.config.updates === 'beta' ? 'beta' : 'stable',
      label: 'Update channel',
      onChange: (v) => {
        S.edit((x) => { if (v === 'beta') x.updates = 'beta'; else delete x.updates; }, { what: 'settings' });
        act('update/check', {});
      },
    }));
  }

  // The latest release on GitHub: what the check found, Check now, and Update when it is newer.
  function updateRow() {
    const r = row('Updates', '', null), ctl = r.lastElementChild;
    const upd = () => {
      const sv = S.server, u = sv.update;
      let hint;
      if (u.busy === 'checking') hint = 'Checking GitHub for a newer camless…';
      else if (u.busy) hint = `${cap(u.busy)} camless ${esc(u.latest)}…`;
      else if (u.available) hint = `camless ${esc(u.latest)}${betaTag(u)} is out. Updating installs the app and the service together, then opens camless again.`;
      else if (u.latest) hint = u.latest === sv.app_version ? 'This is the latest version.' : `The latest ${u.beta ? 'beta' : 'release'} is ${esc(u.latest)}.`;
      else hint = '';
      if (!u.busy) hint += `${hint ? ' ' : ''}camless checks GitHub once each time it starts.`;
      if (u.error && !u.busy) hint += `<span class="cam-err">${esc(u.error)}</span>`;
      r.hint(hint);
      const key = JSON.stringify([u.busy || null, !!u.available]);
      if (ctl.dataset.key === key) return;
      ctl.dataset.key = key;
      ctl.replaceChildren();
      if (u.busy) return;
      if (u.available) ctl.append(button('Release notes', { small: true, onClick: () => act('update/notes', {}) }), button('Update', { small: true, primary: true, onClick: updateNow }));
      else ctl.append(button('Check now', { small: true, onClick: () => act('update/check', {}) }));
    };
    track(S.on('state', upd));
    upd();
    return r;
  }

  async function updateNow() {
    const sv = S.server, v = esc(sv.update.latest);
    const ok = await confirmBox({
      title: `Update to camless ${sv.update.latest}`,
      text: `camless downloads ${v}, closes, installs the app and the service together, and opens again.${sv.admin_task ? '' : ' Windows then asks for administrator rights for the service, as at every start.'} For those few seconds, fans that camless sets run at the failsafe speed; curves stored on a cooler keep running.`,
      ok: 'Update',
    });
    if (ok) act('update/install', {});
  }

  // The service's state, with Start, Restart and Stop; the app runs them in the background.
  function serviceRow() {
    const r = row('camless service', '', null), ctl = r.lastElementChild;
    const stopIt = async () => {
      const ok = await confirmBox({ title: 'Stop the service', text: 'Fans that camless sets go to the failsafe speed and the LCD shows its own screen until the service starts again. Curves stored on a cooler keep running.', ok: 'Stop' });
      if (ok) act('service/stop', {});
    };
    const upd = () => {
      const sv = S.server, st = S.status, info = st && st.service, busy = sv.service_busy;
      let hint;
      if (busy) hint = `${cap(busy)}…`;
      else if (!st) hint = 'Not running, so curves that camless sets are not applied. Curves stored on a cooler keep running.';
      else if (!info) hint = 'Running. This service is older than the app, so it does not say whether it has administrator rights.';
      else if (!info.elevated) hint = 'Running without administrator rights, so CPU temperature and power are off. Restart asks Windows for them.';
      else if (info.cpu_sensor_error) hint = `Running as administrator, but CPU temperature is off: ${esc(info.cpu_sensor_error)}`;
      else hint = 'Running as administrator, with CPU temperature and power.';
      if (info) hint += ` Process ${info.pid}.`;
      r.hint(hint);
      const key = JSON.stringify([!!st, busy || null, !!sv.can_start]);
      if (ctl.dataset.key === key) return;
      ctl.dataset.key = key;
      ctl.replaceChildren();
      if (!sv.can_start || busy) return;
      if (!st) ctl.append(button('Start', { small: true, primary: true, onClick: () => act('service/start', {}) }));
      else ctl.append(button('Restart', { small: true, onClick: () => act('service/restart', {}) }), button('Stop', { small: true, onClick: stopIt }));
    };
    track(S.on('state', upd));
    upd();
    return r;
  }

  // Off until the user turns it on. Windows asks first, so it is set up in the background.
  function adminRow() {
    const title = 'Start as administrator without asking';
    const r = row(title, '', null);
    const t = toggle({ checked: S.server.admin_task, label: title, onChange: (v) => setAdminTask(v, t) });
    r.lastElementChild.appendChild(t);
    const upd = () => {
      const sv = S.server;
      if (!t.dataset.busy && !sv.admin_task_busy) t.set(sv.admin_task);
      let hint;
      if (sv.sandbox) hint = 'In this test copy the switch changes nothing in Windows.';
      else if (sv.admin_task_busy) hint = 'Waiting for Windows…';
      else hint = 'Windows asks once, when you turn this on. From then on camless starts the service as administrator without asking, also when you sign in. Any program you run could then use camless to get administrator rights without a prompt too.';
      if (sv.admin_task_error && !sv.admin_task_busy) hint += `<span class="cam-err">${esc(sv.admin_task_error)}</span>`;
      r.hint(hint);
    };
    track(S.on('state', upd));
    upd();
    return r;
  }

  async function setAdminTask(on, t) {
    t.dataset.busy = '1';
    try {
      if ((await act('admin-task', { on })) === null) t.set(!on);
    } finally {
      delete t.dataset.busy;
    }
  }

  async function setAutostart(on, t) {
    t.dataset.busy = '1';
    try {
      const r = await S.api('PUT', 'autostart', { on });
      t.set(r.autostart);
    } catch (e) {
      toast(e.message);
      t.set(!on);
    } finally {
      delete t.dataset.busy;
    }
  }

  boot();
})();
