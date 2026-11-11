// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
/* Uppwind Link — les pages des fenetres du compagnon : l'installation (wizard), l'etat (status), la desinstallation
   (uninstall). La page ne decide rien : elle dessine l'etat que le compagnon lui envoie (window.__uwl.recv) et lui
   renvoie chaque geste (window.ipc.postMessage, du JSON). Aucune requete reseau (CSP default-src 'none'), aucun
   texte insere comme du HTML : tout passe par textContent. */
(() => {
    'use strict';
    const PAGE = document.body.dataset.page || 'wizard';
    const MARK = (document.querySelector('.uw-mark') || {}).src || '';
    const main = document.getElementById('main');
    const foot = document.getElementById('foot');
    const headRight = document.getElementById('head-right');
    const send = (cmd, extra) => {
        try { window.ipc.postMessage(JSON.stringify(Object.assign({ cmd }, extra || {}))); } catch (e) { /* hors du compagnon */ }
    };

    // ── un petit constructeur d'elements ─────────────────────────────────────────────────────────────
    function h(tag, props, ...kids) {
        const el = document.createElement(tag);
        if (props) {
            for (const [k, v] of Object.entries(props)) {
                if (v == null || v === false) continue;
                if (k === 'class') el.className = v;
                else if (k.startsWith('on') && typeof v === 'function') el.addEventListener(k.slice(2), v);
                else if (k === 'checked' || k === 'disabled') el[k] = !!v;
                else el.setAttribute(k, v === true ? '' : String(v));
            }
        }
        for (const kid of kids.flat(Infinity)) {
            if (kid == null || kid === false || kid === '') continue;
            el.append(kid instanceof Node ? kid : document.createTextNode(String(kid)));
        }
        return el;
    }
    const q = (s) => h('span', { class: 'uw-q' }, `“${s}”`);
    const svgCheck = () => {
        const ns = 'http://www.w3.org/2000/svg';
        const s = document.createElementNS(ns, 'svg');
        s.setAttribute('viewBox', '0 0 12 12');
        const p = document.createElementNS(ns, 'path');
        p.setAttribute('d', 'M2.5 6.3l2.3 2.3 4.7-5');
        s.append(p);
        return s;
    };
    function check(label, checked, onChange, opts = {}) {
        const input = h('input', { type: 'checkbox', checked, disabled: opts.disabled });
        input.addEventListener('change', () => onChange(input.checked));
        return h('label', { class: 'uw-check' + (opts.disabled ? ' is-disabled' : '') }, input, h('span', { class: 'uw-box' }, svgCheck()),
            h('span', { class: 'uw-grow' }, h('span', { class: 'uw-card-title' }, label), opts.sub ? h('div', { class: 'uw-card-sub' }, opts.sub) : null));
    }
    const btn = (text, onClick, cls = '', disabled = false) => h('button', { type: 'button', class: `uw-btn ${cls}`, onclick: onClick, disabled }, text);
    const st = (cls, text) => h('span', { class: `uw-st ${cls}` }, text);
    const note = (cls, ...kids) => h('div', { class: `uw-note ${cls}` }, ...kids);
    const list = (cls, items) => h('ul', { class: `uw-done ${cls}` }, items.map((t) => h('li', null, t)));
    function set(el, ...kids) { el.replaceChildren(...kids.flat(Infinity).filter((k) => k != null && k !== false)); }
    function render(mainKids, footKids, right) {
        const y = main.scrollTop;
        set(main, mainKids);
        set(foot, footKids);
        if (right !== undefined) set(headRight, right);
        main.scrollTop = y;
    }
    let toastTimer = 0;
    function toast(text, cls = 'is-bad') {
        let t = document.getElementById('toast');
        if (!t) {
            t = h('div', { id: 'toast', class: 'uw-note uw-banner' });
            foot.before(t);
        }
        t.className = `uw-note ${cls}`;
        t.textContent = text;
        t.hidden = false;
        clearTimeout(toastTimer);
        toastTimer = setTimeout(() => { t.hidden = true; }, 6000);
    }
    let readySent = false;
    const ready = () => { if (!readySent) { readySent = true; send('ready'); } };

    // ── (11 nov.) la barre de titre de la fenetre d'etat : sans le cadre de Windows, la page dessine Reduire, Agrandir,
    // Fermer ; la tete sert a deplacer la fenetre (double-clic : agrandir), les bords a la redimensionner ──────────
    const FRAMELESS = PAGE === 'status';
    const NS = 'http://www.w3.org/2000/svg';
    function icon(d, size = 10) {
        const s = document.createElementNS(NS, 'svg');
        s.setAttribute('viewBox', `0 0 ${size} ${size}`);
        s.setAttribute('aria-hidden', 'true');
        for (const one of [].concat(d)) {
            const p = document.createElementNS(NS, 'path');
            p.setAttribute('d', one);
            s.append(p);
        }
        return s;
    }
    const ICON_MIN = 'M1 5.5h8';
    const ICON_MAX = 'M1.5 1.5h7v7h-7z';
    const ICON_RESTORE = ['M1.5 3.5h5v5h-5z', 'M3.5 3.5v-2h5v5h-2'];
    const ICON_CLOSE = ['M1.5 1.5l7 7', 'M8.5 1.5l-7 7'];
    const ICON_GEAR = ['M8 5.2a2.8 2.8 0 1 0 0 5.6 2.8 2.8 0 0 0 0-5.6z',
        'M8 1.2v1.6M8 13.2v1.6M1.2 8h1.6M13.2 8h1.6M3.2 3.2l1.1 1.1M11.7 11.7l1.1 1.1M3.2 12.8l1.1-1.1M11.7 4.3l1.1-1.1'];
    let maxBtn = null;
    let settingsBtn = null;
    function setMaximized(on) {
        if (!maxBtn) return;
        maxBtn.replaceChildren(icon(on ? ICON_RESTORE : ICON_MAX));
        maxBtn.title = on ? 'Restore' : 'Maximize';
        maxBtn.setAttribute('aria-label', maxBtn.title);
        document.body.classList.toggle('is-max', !!on);
    }
    if (FRAMELESS) {
        document.body.classList.add('is-frameless');
        const head = document.querySelector('.uw-head');
        const winBtn = (cls, title, d, onClick) => {
            const b = h('button', { type: 'button', class: `uw-winbtn ${cls}`, title, 'aria-label': title, onclick: onClick });
            b.append(icon(d));
            return b;
        };
        settingsBtn = h('button', { type: 'button', class: 'uw-btn uw-btn-sm uw-settings-btn', title: 'Settings', onclick: () => toggleSettings() });
        settingsBtn.append(icon(ICON_GEAR, 16), h('span', null, 'Settings'));
        maxBtn = winBtn('', 'Maximize', ICON_MAX, () => send('win', { action: 'maximize' }));
        head.append(settingsBtn, h('div', { class: 'uw-winctl' },
            winBtn('', 'Minimize', ICON_MIN, () => send('win', { action: 'minimize' })),
            maxBtn,
            winBtn('is-close', 'Close', ICON_CLOSE, () => send('close'))));
        // deplacer : un bouton enfonce sur la tete, hors de ses boutons
        head.addEventListener('mousedown', (e) => {
            if (e.button !== 0 || e.target.closest('button') || edgeAt(e)) return;
            if (e.detail === 2) { send('win', { action: 'maximize' }); return; }
            send('win', { action: 'drag' });
        });
        // redimensionner : les 6 pixels du bord
        const EDGE = 6;
        function edgeAt(e) {
            if (document.body.classList.contains('is-max')) return '';
            const w = window.innerWidth;
            const hh = window.innerHeight;
            const n = e.clientY < EDGE ? 'n' : e.clientY >= hh - EDGE ? 's' : '';
            const we = e.clientX < EDGE ? 'w' : e.clientX >= w - EDGE ? 'e' : '';
            return n + we;
        }
        // (le curseur par une classe : la CSP n'a pas de style en ligne)
        document.addEventListener('mousemove', (e) => {
            const edge = edgeAt(e);
            if ((document.body.dataset.edge || '') !== edge) document.body.dataset.edge = edge;
        });
        document.addEventListener('mousedown', (e) => {
            const edge = edgeAt(e);
            if (e.button === 0 && edge) { e.preventDefault(); send('win', { action: 'resize', edge }); }
        }, true);
    }

    // ── l'installation ─────────────────────────────────────────────────────────────────────────────────
    const W = { s: null, progress: [], code: null };
    const FLOW = ['daws', 'ports', 'options', 'progress'];

    function dots(step) {
        const at = step === 'done' ? 4 : FLOW.indexOf(step);
        if (step === 'welcome') return null;
        return h('div', { class: 'uw-dots' }, step === 'done' ? 'Done' : `Step ${Math.min(at + 1, 4)} of 4`,
            FLOW.map((_, i) => h('span', { class: 'uw-dot' + (i < at ? ' is-done' : i === at ? ' is-on' : '') })));
    }
    function sandboxBanner(m) {
        if (!m.sandbox) return null;
        return note('is-warn uw-banner', h('b', null, 'Sandbox. '), `Everything installs in ${m.sandbox}. `,
            m.simulate_daws ? 'Ableton Live and FL Studio are not changed: those steps are only simulated.' : 'The DAWs are the fake ones of UWC_SETUP_ROOT.');
    }
    const choices = () => JSON.parse(JSON.stringify(W.s.choices));
    function choose(mut) { const c = choices(); mut(c); send('choose', { choices: c }); }

    function scriptLine(d) {
        switch (d.action) {
            case 'install': return `UppwindLink script ${d.bundled_version} will be installed.`;
            case 'update': return d.installed_version ? `Script ${d.installed_version} → ${d.bundled_version} (the old one is kept).` : `A different copy is replaced by ${d.bundled_version} (the old one is kept).`;
            case 'none': return `Script ${d.bundled_version}: already up to date.`;
            default: return d.reason ? `${d.reason[0].toUpperCase()}${d.reason.slice(1)}.` : 'Nothing to do here.';
        }
    }

    // (29 oct.) les plugins Uppwind Link : une invite de Windows (l'administrateur) pour les mettre dans Program Files
    const PLUGINS = 'plugins';
    function adminNote(s) {
        const p = s.model.plugins || {};
        const asks = p.admin && !s.model.simulate_daws && s.choices.daws.includes(PLUGINS) && (p.action === 'install' || p.action === 'update');
        return (asks ? 'One administrator prompt, for the plugins only.' : 'No administrator rights.') + ' Uninstall any time from Windows Settings › Apps.';
    }
    // (10 nov.) macOS : leurs Audio Units (Logic Pro) suivent la meme case ; quand elles n'en sont pas au meme point que
    // le VST3, chacun dit le sien
    function formatLine(q) {
        switch (q.action) {
            case 'install': return 'will be installed';
            case 'update': return q.status === 'outdated' ? 'an older version will be updated' : 'different files will be replaced';
            case 'none': return 'already up to date';
            default: return q.reason || 'nothing to do';
        }
    }
    function pluginLine(p) {
        if (p.not_in_build) return 'Not in this build of Uppwind Link (a development build).';
        if (p.au && (p.au.action !== p.action || p.au.status !== p.status)) return `VST3: ${formatLine(p)}. Audio Units, for Logic Pro: ${formatLine(p.au)}.`;
        switch (p.action) {
            case 'install': return 'Uppwind Link Return and Uppwind Link Notes will be installed.';
            case 'update': return p.status === 'outdated' ? 'An older version is installed: it will be updated.' : 'Different files are installed: they will be replaced.';
            case 'none': return 'Already up to date.';
            default: return p.reason ? `${p.reason[0].toUpperCase()}${p.reason.slice(1)}.` : 'Nothing to do here.';
        }
    }
    function pluginCard(s) {
        const p = s.model.plugins;
        if (!p || !(p.available || p.not_in_build)) return null;
        const on = s.choices.daws.includes(PLUGINS) && p.selectable;
        const body = [h('div', null, pluginLine(p)), p.folder ? h('div', { class: 'uw-path' }, p.folder) : null, p.au && p.au.folder ? h('div', { class: 'uw-path' }, p.au.folder) : null];
        if (p.admin && on && (p.action === 'install' || p.action === 'update')) {
            body.push(note('', 'Windows asks for administrator rights once, to put them where FL Studio looks for VST3 plugins. Nothing else runs as administrator.'));
        }
        if (p.held) body.push(note('', 'A DAW still has an older copy loaded: restart it after the install.'));
        return h('div', { class: 'uw-card' + (p.selectable ? '' : ' is-off') },
            check(p.au ? 'Uppwind Link plugins (VST3 and Audio Units)' : 'Uppwind Link plugins (VST3)', on, (v) => choose((x) => { x.daws = x.daws.filter((i) => i !== PLUGINS); if (v) x.daws.push(PLUGINS); }),
                { disabled: !p.selectable, sub: p.au ? 'Needed for FL Studio, and for Logic Pro, which loads only Audio Units. They also bring your DAW’s sound to the studio.'
                    : 'Needed for FL Studio. They also bring your DAW’s sound to the studio with any audio driver, ASIO included.' }),
            h('div', { class: 'uw-card-body' }, body));
    }

    function wizWelcome(s) {
        const m = s.model;
        const upd = m.update_from != null;
        const again = upd && m.update_from === m.version;
        const chips = m.daws.map((d) => h('span', { class: 'uw-chip' }, st('is-ok', ''), d.name));
        chips.push(h('span', { class: 'uw-chip' }, st(m.ports.link && m.ports.back ? 'is-ok' : 'is-warn', ''), m.ports.link && m.ports.back ? 'MIDI ports' : 'MIDI ports missing'));
        if (!m.daws.length) chips.unshift(h('span', { class: 'uw-chip' }, st('is-warn', ''), 'No DAW found'));
        if (m.plugins && m.plugins.available) chips.push(h('span', { class: 'uw-chip' }, st('is-ok', ''), 'Uppwind Link plugins'));
        return [
            sandboxBanner(m),
            h('div', { class: 'uw-hero' },
                h('img', { class: 'uw-hero-mark', src: MARK, alt: '' }),
                h('h1', null, again ? 'Set up Uppwind Link again' : upd ? 'Update Uppwind Link' : 'Uppwind Link'),
                h('p', null, 'Play the instruments of Ableton Live and FL Studio from Uppwind Studio, and use their plugin windows there.'),
                h('p', null, again ? `Version ${m.version} is installed. This looks at your DAWs again and puts back what is missing; your pairing stays.`
                    : upd ? (m.update_from ? `Version ${m.update_from} is installed. This is ${m.version}: your settings and your pairing stay.` : `An earlier version is installed. This is ${m.version}: your settings and your pairing stay.`)
                        : 'It settles in your user folder and waits next to the clock, ready when the studio needs it.'),
                h('div', { class: 'uw-found' }, chips)),
        ];
    }

    function wizDaws(s) {
        const m = s.model;
        const c = s.choices;
        const kids = [sandboxBanner(m), h('h1', null, 'Your DAWs'),
            h('p', { class: 'uw-lede' }, 'Uppwind Link adds a small script to each DAW you tick. Anything it replaces is copied first.')];
        if (!m.daws.length) {
            kids.push(note('is-warn', 'No Ableton Live or FL Studio on this computer. You can still install Uppwind Link: once a DAW is installed, open Uppwind Link › Set up again.'));
        }
        for (const d of m.daws) {
            const on = c.daws.includes(d.id);
            const body = [h('div', null, scriptLine(d)), h('div', { class: 'uw-path' }, d.folder)];
            if (d.daw === 'ableton' && d.running && on && (d.action === 'install' || d.action === 'update')) {
                body.push(note('', `Live is open: restart it after the install, it reads scripts when it starts.`));
            }
            if (d.daw === 'fl' && d.fl_settings) {
                const f = d.fl_settings;
                if (f.changes > 0) {
                    body.push(check('Apply FL Studio’s MIDI settings for me', c.fl_settings && on, (v) => choose((x) => { x.fl_settings = v; }),
                        { disabled: !on, sub: 'A backup is kept; uninstalling puts them back.' }));
                    if (f.blocked_by && on && c.fl_settings) {
                        body.push(note('is-warn', h('div', { class: 'uw-row' }, h('span', { class: 'uw-grow' }, h('b', null, 'Close FL Studio to apply its settings. '), 'Uppwind Link never closes it for you.'),
                            btn(s.checking ? 'Checking…' : 'Check again', () => send('recheck'), 'uw-btn-sm', s.checking))));
                    }
                } else {
                    body.push(h('div', { class: 'uw-small' }, f.reason ? `MIDI settings: ${f.reason}.` : 'MIDI settings: already right.'));
                }
            }
            kids.push(h('div', { class: 'uw-card' + (d.selectable ? '' : ' is-off') },
                check(d.name, on && d.selectable, (v) => choose((x) => { x.daws = v ? [...x.daws, d.id] : x.daws.filter((i) => i !== d.id); }),
                    { disabled: !d.selectable, sub: d.versions.length ? (d.daw === 'ableton' ? `Live ${d.versions.join(', ')}` : d.versions.join(', ')) : null }),
                h('div', { class: 'uw-card-body' }, body)));
        }
        kids.push(pluginCard(s));
        return kids;
    }

    function loopmidiCard(p, checking, onCheck) {
        return h('div', { class: 'uw-card' },
            h('div', { class: 'uw-row' },
                h('div', { class: 'uw-grow' }, h('div', { class: 'uw-card-title' }, 'loopMIDI'),
                    h('div', { class: 'uw-card-sub' }, 'loopMIDI is free software by Tobias Erichsen for private, non-commercial use.')),
                btn('Get loopMIDI', () => send('open', { link: 'loopmidi' }), 'uw-btn-sm')),
            h('div', { class: 'uw-card-body' },
                h('div', null, st(p.loopmidi_running ? 'is-ok' : p.loopmidi_installed ? 'is-warn' : '', p.loopmidi_running ? 'Installed and running' : p.loopmidi_installed ? 'Installed, not running: start it' : 'Not installed')),
                h('ol', { class: 'uw-steps' },
                    h('li', null, 'Install loopMIDI from its page, then start it.'),
                    h('li', null, 'Type ', q('Uppwind Link'), ' under “New port-name”, click +.'),
                    h('li', null, 'Type ', q('Uppwind Link Back'), ', click + again. Leave loopMIDI running: its tray icon has Autostart.')),
                h('div', { class: 'uw-row' }, btn(checking ? 'Checking…' : 'Check again', onCheck, 'uw-btn-sm', checking),
                    h('span', { class: 'uw-small' }, 'You can skip this: Uppwind Link reminds you until the ports are there.'))));
    }
    function portRows(p) {
        const row = (name, found) => h('div', { class: 'uw-row' }, h('span', { class: 'uw-grow' }, q(name)),
            found ? st('is-ok', found === name ? 'Found' : `Found as “${found}”`) : st('is-warn', 'Missing'));
        const kids = [row('Uppwind Link', p.link), row('Uppwind Link Back', p.back)];
        if (p.near && p.near.length) {
            kids.push(note('is-warn', `Almost: ${p.near.map((n) => `“${n}”`).join(', ')}. In loopMIDI, remove it and create it again with the exact name.`));
        }
        return kids;
    }

    function wizPorts(s) {
        const p = s.model.ports;
        const ok = p.link && p.back;
        return [sandboxBanner(s.model), h('h1', null, 'MIDI ports'),
            h('p', { class: 'uw-lede' }, 'The studio and your DAW talk through two virtual MIDI ports.'),
            h('div', { class: 'uw-card' }, portRows(p)),
            ok ? note('is-neon', 'Both ports are there. Nothing to do.') : loopmidiCard(p, s.checking, () => send('recheck'))];
    }

    function wizOptions(s) {
        const c = s.choices;
        return [sandboxBanner(s.model), h('h1', null, 'Options'),
            h('p', { class: 'uw-lede' }, 'You can change these later from Uppwind Link’s icon, next to the clock.'),
            h('div', { class: 'uw-card' }, check('Start with Windows', c.autostart, (v) => choose((x) => { x.autostart = v; }), { sub: 'Uppwind Link waits in the notification area: the studio finds it when you need it.' })),
            h('div', { class: 'uw-card' }, check('Open the studio when done', c.open_studio, (v) => choose((x) => { x.open_studio = v; }), { sub: 'In your browser, to pair it.' })),
            h('div', { class: 'uw-k' }, 'Where'),
            h('div', { class: 'uw-path' }, s.model.install_dir),
            h('p', { class: 'uw-small' }, adminNote(s))];
    }

    function wizProgress() {
        const items = W.progress.map((l, i) => h('li', { class: i === W.progress.length - 1 ? 'is-now' : 'is-done' }, i === W.progress.length - 1 ? h('span', { class: 'uw-spin' }) : null, l));
        return [h('h1', null, 'Installing'), h('p', { class: 'uw-lede' }, 'A few seconds.'), h('ul', { class: 'uw-progress' }, items)];
    }

    // Le code d'appairage est cache par defaut (un partage d'ecran le montrerait) : « Show » le revele, « Copy » marche
    // sans le reveler. W.reveal survit aux rafraichissements de la fenetre.
    function masked(code) {
        const m = /^(UWC2?-\d+-)/.exec(code);
        return (m ? m[1] : '') + '••••••••••••••••';
    }
    function codeBox(code) {
        const shown = !!W.reveal;
        return h('div', { class: 'uw-code' },
            h('code', null, shown ? code : masked(code)),
            btn(shown ? 'Hide' : 'Show', () => { W.reveal = !shown; redraw(); }, 'uw-btn-sm'),
            btn('Copy', (e) => { send('copy', { what: 'code' }); e.currentTarget.textContent = 'Copied'; }, 'uw-btn-sm uw-btn-primary'));
    }

    function wizDone(s) {
        const o = s.outcome || {};
        const kids = [sandboxBanner(s.model)];
        kids.push(h('h1', null, o.ok ? 'Uppwind Link is ready' : 'Installed, with a problem'));
        kids.push(h('p', { class: 'uw-lede' }, o.ok ? (o.port ? `It runs next to the clock (port ${o.port}). What is left takes a minute:` : 'What is left takes a minute:') : 'Some steps did not work: see below.'));
        if (o.errors && o.errors.length) kids.push(h('div', { class: 'uw-card' }, list('is-bad', o.errors)));
        // l'appairage d'abord (le code a copier), puis les reglages des DAW
        const items = [...(o.todo || [])].sort((a, b) => (b.id === 'pair') - (a.id === 'pair'));
        const todo = items.map((t) => h('div', { class: 'uw-card' + (t.id === 'pair' ? ' is-neon' : '') }, h('div', { class: 'uw-card-title' }, t.title),
            t.lines.map((l) => h('p', null, l)),
            t.id === 'pair' && W.code ? [codeBox(W.code), h('div', { class: 'uw-row' }, btn('Open the studio', () => send('open', { link: 'studio' }), 'uw-btn-sm'))] : null,
            t.id === 'ports' ? h('div', { class: 'uw-row' }, btn('Get loopMIDI', () => send('open', { link: 'loopmidi' }), 'uw-btn-sm'), h('span', { class: 'uw-small' }, 'loopMIDI is free software by Tobias Erichsen for private, non-commercial use.')) : null));
        if (todo.length) kids.push(h('div', { class: 'uw-todo' }, todo));
        const did = [...(o.scripts || []), ...(o.fl_lines || []), ...(o.plugin_lines || [])];
        if (o.backups) did.push(`Copies of what was replaced: ${o.backups}`);
        if (o.autostart) did.push('Starts with Windows.');
        if (o.listed) did.push('Listed in Windows Settings › Apps (uninstall from there).');
        kids.push(h('details', { class: 'uw-more' }, h('summary', null, 'What was done'), list('', did.length ? did : (o.done || []))));
        return kids;
    }

    // le dernier etat recu : « Show » / « Hide » redessinent la meme vue
    function redraw() {
        const m = W.last;
        if (!m) return;
        if (m.type === 'status') status(m);
        else wizard(m);
    }

    function wizard(msg) {
        if (msg.type === 'state') W.last = msg;
        if (msg.type === 'progress') {
            W.progress.push(msg.line);
            if (W.s && W.s.step === 'progress') render(wizProgress(), [], dots('progress'));
            return;
        }
        if (msg.type !== 'state') return;
        W.s = msg.wizard;
        W.progress = msg.progress || W.progress;
        W.code = msg.code || null;
        const s = W.s;
        const back = btn('Back', () => send('back'));
        const next = (label = 'Next', primary = true) => btn(label, () => send('next'), primary ? 'uw-btn-primary uw-btn-big' : 'uw-btn-big');
        let body;
        let footer;
        switch (s.step) {
            case 'welcome':
                body = wizWelcome(s);
                footer = [h('span', { class: 'uw-small' }, adminNote(s)), h('span', { class: 'uw-sp' }),
                    h('button', { type: 'button', class: 'uw-link', onclick: () => send('customize') }, 'Customize'),
                    btn(s.model.update_from === s.model.version ? 'Set up' : s.model.update_from != null ? 'Update' : 'Install', () => send('install'), 'uw-btn-primary uw-btn-big')];
                break;
            case 'daws': body = wizDaws(s); footer = [back, h('span', { class: 'uw-sp' }), next(s.express ? 'Continue' : 'Next')]; break;
            case 'ports': {
                const ok = s.model.ports.link && s.model.ports.back;
                body = wizPorts(s);
                footer = [back, h('span', { class: 'uw-sp' }), ok ? next(s.express ? 'Continue' : 'Next') : next(s.express ? 'Skip and install' : 'Skip', false)];
                break;
            }
            case 'options': body = wizOptions(s); footer = [back, h('span', { class: 'uw-sp' }), btn(s.model.update_from != null ? 'Update' : 'Install', () => send('next'), 'uw-btn-primary uw-btn-big')]; break;
            case 'progress': body = wizProgress(); footer = []; break;
            case 'done': {
                const o = s.outcome || {};
                body = wizDone(s);
                footer = [h('span', { class: 'uw-sp' }), btn(s.choices.open_studio && o.ok ? 'Finish and open the studio' : 'Finish', () => send('finish'), 'uw-btn-primary uw-btn-big')];
                break;
            }
            default: body = [];
        }
        render(body, footer, dots(s.step));
        ready();
    }

    // ── l'etat ─────────────────────────────────────────────────────────────────────────────────────────
    let helpOpen = false;
    // (9 nov., audit F2) « New code… » demande une confirmation sur place (il deconnecte le studio)
    let confirmNew = false;
    const ago = (s) => (s < 60 ? 'just now' : s < 5400 ? `${Math.round(s / 60)} min ago` : `${Math.round(s / 3600)} h ago`);
    // (11 nov.) le panneau Settings, et ce que la mise a jour en cours dit
    let settingsOpen = false;
    let updLine = '';
    let updBusy = false;
    let checking = false;
    function toggleSettings(on) {
        settingsOpen = on === undefined ? !settingsOpen : !!on;
        if (settingsBtn) settingsBtn.classList.toggle('is-on', settingsOpen);
        if (W.last && W.last.status) status(W.last);
    }

    // Les reglages : seulement cette fenetre les change (le studio n'a aucune de ces commandes).
    function settingsView(s) {
        const R = s.remote || {};
        const D = s.discord || {};
        const U = s.update || {};
        const running = !!s.port;
        const idle = R.idle_disarm_min || 10;
        const setting = (label, on, onChange, sub, disabled) => h('div', { class: 'uw-card' }, check(label, on, onChange, { sub, disabled }));
        // la mise a jour
        const last = U.last && U.last.version && U.last.version === s.version && U.last.from && U.last.from !== U.last.version
            ? (U.last.ok ? note('is-neon', `Updated from ${U.last.from} to ${U.last.version}.`)
                : note('is-bad', `The update to ${U.last.version} had a problem: ${(U.last.errors || []).join(' ') || 'see the log.'}`))
            : null;
        let line;
        if (checking || U.checking) line = st('', 'Checking GitHub…');
        else if (updBusy || U.installing) line = st('is-ok', updLine || 'Installing the new version…');
        else if (U.newer) line = st('is-ok', `Uppwind Link ${U.latest} is available`);
        else if (U.latest) line = st('is-ok', `Up to date · the latest is ${U.latest}${U.checked_ago_s != null ? `, checked ${ago(U.checked_ago_s)}` : ''}`);
        else if (U.error) line = st('is-warn', `Could not check: ${U.error}`);
        else line = st('', 'Not checked yet');
        const updButtons = h('div', { class: 'uw-row' },
            btn(checking ? 'Checking…' : 'Check for updates', () => { checking = true; status(W.last); send('checkupdate'); }, 'uw-btn-sm', checking || updBusy),
            U.newer ? (U.supported
                ? btn(updBusy ? 'Updating…' : `Update to ${U.latest}`, () => { updBusy = true; updLine = 'Downloading the new version…'; status(W.last); send('update'); }, 'uw-btn-sm uw-btn-primary', updBusy || !running)
                : btn('Download it', () => send('open', { link: 'releases' }), 'uw-btn-sm uw-btn-primary')) : null);
        const updates = h('div', { class: 'uw-card' },
            h('div', { class: 'uw-card-title' }, `Uppwind Link ${s.version}`),
            h('div', { class: 'uw-card-body is-flush' }, line, updButtons, last,
                check('Update automatically', !!U.auto, (v) => send('autoupdate', { on: v }), {
                    disabled: !running || !U.supported,
                    sub: U.supported ? 'Checks GitHub every 6 hours and installs a newer version by itself, while the studio is not connected. Off until you tick it.'
                        : 'Updates from the app are for Windows: download the new version from its release page.',
                }),
                h('p', { class: 'uw-small' }, 'Updates are the only time Uppwind Link itself goes on the internet, and only to its GitHub releases (github.com/alakazam777/uppwind-app). Show on Discord, when you turn it on, hands what you play to the Discord app on this computer, and Discord publishes it on your profile. Each download is checked against the SHA-256 published with the release before it is installed. The program is not code-signed yet, so the check proves the file is the one published there, not who published it.')));
        return [
            h('div', { class: 'uw-row uw-settings-head' }, h('h1', { class: 'uw-grow' }, 'Settings'), btn('Done', () => toggleSettings(false), 'uw-btn-sm uw-btn-primary')),
            h('p', { class: 'uw-lede' }, 'Only this window changes these settings: the studio cannot.'),
            h('div', { class: 'uw-k' }, 'General'),
            setting('Start with Windows', s.autostart, (v) => send('autostart', { on: v }), 'Uppwind Link waits next to the clock: the studio finds it when you need it.'),
            h('div', { class: 'uw-k' }, 'Updates'),
            updates,
            h('div', { class: 'uw-k' }, 'Studio'),
            setting('Remote control', !!R.armed, (v) => send('arm', { on: v }),
                running ? `The studio can capture plugin windows, send them gestures and take your DAW’s sound. Turns itself off after ${idle} min without the studio, and at every start.` : 'Appears once Uppwind Link runs.', !running),
            setting('Show on Discord', !!D.allowed, (v) => send('discord', { on: v }),
                running ? 'What you play in Music shows on your Discord profile (title, artist, cover). Music must be open, with Show on Discord turned on there too.' : 'Appears once Uppwind Link runs.', !running),
            h('div', { class: 'uw-k' }, 'Set up'),
            h('div', { class: 'uw-card' }, h('div', { class: 'uw-row' },
                h('div', { class: 'uw-grow' }, h('div', { class: 'uw-card-title' }, 'Set up again'), h('div', { class: 'uw-card-sub' }, 'Looks at your DAWs again and puts back what is missing; your pairing stays.')),
                btn('Set up again…', () => send('setup'), 'uw-btn-sm'))),
        ];
    }

    function status(msg) {
        // (11 nov.) la barre de titre : agrandie ou non
        if (msg.type === 'win') { setMaximized(!!msg.maximized); return; }
        // (11 nov.) la mise a jour : une ligne tout de suite ; la relecture suit
        if (msg.type === 'update') {
            checking = false;
            updBusy = !!msg.busy;
            updLine = msg.text || '';
            if (msg.text) toast(msg.text, 'is-neon');
            if (W.last && W.last.status) status(W.last);
            return;
        }
        // (9 nov., audit F1) arme ou desarme (ici, ou par le menu de l'icone) : tout de suite ; la relecture suit
        if (msg.type === 'armed') {
            if (W.last && W.last.status) {
                const r = W.last.status.remote || {};
                W.last.status.remote = Object.assign({}, r, { armed: !!msg.armed, asked_ago_s: msg.armed ? null : r.asked_ago_s });
                status(W.last);
            }
            return;
        }
        // (9 nov., audit F2) le nouveau code : cache, comme toujours ; la relecture le montre
        if (msg.type === 'newcode') {
            W.reveal = false;
            toast('New pairing code: the old one no longer works. Copy this one and paste it in the studio (Settings › Uppwind Link › Plugin windows).', 'is-neon');
            return;
        }
        if (msg.type !== 'status') return;
        W.last = msg;
        const s = msg.status;
        const L = s.live;
        const R = s.remote || {};
        const cell = (k, ...v) => [h('div', { class: 'uw-cell-k' }, k), h('div', { class: 'uw-cell-v' }, ...v)];
        const when = L.last_paired ? new Date(L.last_paired * 1000).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' }) : '';
        // (9 nov., audit F1) l'armement : sans lui, le studio ne filme, ne pilote ni n'ecoute rien. (F4) ce qui se passe.
        const idle = R.idle_disarm_min || 10;
        const activity = [];
        if (L.watching) activity.push(`Capturing ${L.watching}`);
        if (L.last_input && Date.now() / 1000 - L.last_input < 10) activity.push('gestures just now');
        if (L.audio_streams > 0) activity.push('sending your DAW’s sound');
        const remote = !s.port ? [h('div', { class: 'uw-small' }, 'Appears once Uppwind Link runs.')]
            : R.armed ? [
                st('is-ok', 'On · the studio can capture plugin windows, send them gestures and take your DAW’s sound'),
                activity.length ? h('div', { class: 'uw-small' }, activity.join(' · ')) : null,
                h('div', { class: 'uw-row' }, btn('Turn off', () => send('arm', { on: false }), 'uw-btn-sm'),
                    h('span', { class: 'uw-small' }, R.idle_left_min != null ? `Turns itself off in ${R.idle_left_min} min if the studio does not come back.` : `Turns itself off after ${idle} min without the studio, and at every start.`)),
            ] : [
                st('is-warn', 'Off · the studio cannot capture plugin windows, send them gestures or take your DAW’s sound'),
                R.asked_ago_s != null ? note('is-warn', `The studio asked for it ${ago(R.asked_ago_s)}.`) : null,
                h('div', { class: 'uw-row' }, btn('Allow remote control', () => send('arm', { on: true }), 'uw-btn-sm uw-btn-primary'),
                    h('span', { class: 'uw-small' }, `Until you turn it off, ${idle} min without the studio, or the next start.`)),
            ];
        const newCode = !s.code ? null : confirmNew
            ? note('is-warn', h('p', null, 'A new pairing code disconnects the studio, and the old code stops working everywhere. You then paste the new one in the studio.'),
                h('div', { class: 'uw-row' }, btn('Make a new code', () => { confirmNew = false; send('newcode'); }, 'uw-btn-sm uw-btn-danger'),
                    btn('Cancel', () => { confirmNew = false; status(msg); }, 'uw-btn-sm')))
            : h('button', { type: 'button', class: 'uw-link', onclick: () => { confirmNew = true; status(msg); } }, 'New code…');
        const studio = [
            L.sessions > 0 ? st('is-ok', `Paired · connected now${L.last_origin ? ` (${L.last_origin.replace(/^https?:\/\//, '')})` : ''}`)
                : L.paired_ever ? st('is-ok', `Paired · last connected ${when}`) : st('is-warn', 'Not paired yet'),
            s.code ? codeBox(s.code) : h('div', { class: 'uw-small' }, 'The pairing code appears once Uppwind Link runs.'),
            h('div', { class: 'uw-row' }, btn('Open the studio', () => send('open', { link: 'studio' }), 'uw-btn-sm'),
                h('span', { class: 'uw-small' }, 'Paste the code once: Settings › Uppwind Link › Plugin windows › Pair.')),
            newCode,
        ];
        const daws = s.daws.length ? s.daws.map((d) => h('div', null,
            h('div', { class: 'uw-row' }, h('b', { class: 'uw-grow' }, d.name, d.versions.length ? h('span', { class: 'uw-small' }, ` · ${d.versions.join(', ')}`) : null),
                d.running ? st('is-ok', 'Open') : st('', 'Closed')),
            d.scripts.map((t) => h('div', { class: 'uw-small' }, t.status === 'up-to-date' ? `Script ${t.bundled} · up to date`
                : t.status === 'missing' ? 'Script not installed (Set up again installs it)'
                    : t.status === 'outdated' ? `Script ${t.installed || '?'} · ${t.bundled} available (Set up again)`
                        : t.status === 'newer' ? `Script ${t.installed || '?'} (newer than this Uppwind Link)`
                            : t.status === 'no-library' ? 'User Library not found' : `Script ${t.installed || ''} · differs from ${t.bundled}`)))) : [h('div', { class: 'uw-small' }, 'No Ableton Live or FL Studio found.')];
        const p = s.ports;
        const ports = [h('div', null, portRows(p))];
        if (!(p.link && p.back)) ports.push(h('div', { class: 'uw-row' }, btn('Get loopMIDI', () => send('open', { link: 'loopmidi' }), 'uw-btn-sm'),
            h('span', { class: 'uw-small' }, 'loopMIDI is free software by Tobias Erichsen for private, non-commercial use. Create the two ports in it.')));
        const sound = [s.audio.available ? st('is-ok', 'Available: Uppwind Link sends your DAW’s sound to the studio') : st('is-bad', 'Not available on this Windows')];
        if (s.audio.trouble) sound.push(note('is-bad', `The last audio return failed: ${s.audio.trouble}`));
        const help = h('div', null,
            h('button', { type: 'button', class: 'uw-link', onclick: () => { helpOpen = !helpOpen; status(msg); } }, 'Sound not coming back?'),
            helpOpen ? note('', h('p', null, 'Your DAW must play through Windows, into an output you do not listen to:'),
                h('ul', { class: 'uw-done is-kept' },
                    h('li', null, 'Ableton Live: Settings › Audio › Driver Type MME/DirectX (not ASIO).'),
                    h('li', null, 'FL Studio: Options › Audio settings › Primary Sound Driver (not a hardware ASIO driver).')),
                s.audio.trouble ? h('p', null, h('b', null, 'Still silent? '), 'Install VB-CABLE, a virtual audio cable, and pick ', q('Virtual cable'),
                    ' in the studio: Settings › Uppwind Link › Audio return. ', h('button', { type: 'button', class: 'uw-link', onclick: () => send('open', { link: 'vbcable' }) }, 'VB-CABLE’s page')) : null) : null);
        // (11 nov., discord.rs) ce qu'on ecoute dans Music, sur le profil Discord : seulement si c'est permis ici
        const D = s.discord || {};
        const discord = !s.port ? [h('div', { class: 'uw-small' }, 'Appears once Uppwind Link runs.')]
            : D.allowed ? [
                D.shown ? st('is-ok', 'On · showing what you play in Music on your Discord profile')
                    : D.discord === 'absent' ? st('is-warn', 'On · open the Discord desktop app to show what you play')
                        : st('is-ok', 'On · what you play in Music will show on your Discord profile'),
                h('div', { class: 'uw-row' }, btn('Turn off', () => send('discord', { on: false }), 'uw-btn-sm'),
                    h('span', { class: 'uw-small' }, 'Music must be open in your browser, and Show on Discord turned on there.')),
            ] : [
                st('', 'Off · nothing is shown on Discord'),
                h('div', { class: 'uw-row' }, btn('Show on Discord', () => send('discord', { on: true }), 'uw-btn-sm'),
                    h('span', { class: 'uw-small' }, 'Your Discord friends see the title, artist and cover of what you play in Music.')),
            ];
        const sb = s.sandbox ? note('is-warn uw-banner', h('b', null, 'Sandbox. '), `This Uppwind Link runs from ${s.sandbox}.`) : null;
        const pill = s.port ? h('span', { class: 'uw-pill is-ok' }, `Running · port ${s.port}`) : h('span', { class: 'uw-pill' }, 'Not running');
        if (settingsOpen) {
            render([sb, settingsView(s)], [h('span', { class: 'uw-sp' }), btn('Done', () => toggleSettings(false), 'uw-btn-primary')], pill);
        } else {
            // (11 nov.) une version plus recente, deja lue : un bandeau, et la mise a jour dans Settings
            const U = s.update || {};
            const upd = U.newer && !updBusy ? note('is-neon uw-banner', h('div', { class: 'uw-row' },
                h('span', { class: 'uw-grow' }, h('b', null, `Uppwind Link ${U.latest} is available. `), U.supported ? 'Your settings and your pairing stay.' : 'Download it from its release page.'),
                U.supported ? btn('Update', () => { updBusy = true; updLine = 'Downloading the new version…'; send('update'); status(msg); }, 'uw-btn-sm uw-btn-primary', !s.port)
                    : btn('Download it', () => send('open', { link: 'releases' }), 'uw-btn-sm uw-btn-primary'))) : null;
            const busy = updBusy ? note('is-neon uw-banner', updLine || 'Installing the new version…') : null;
            render([sb, upd, busy, h('div', { class: 'uw-grid' }, cell('Remote control', remote), cell('Studio', studio), cell('DAWs', daws), cell('MIDI ports', ports), cell('Sound', sound), cell('Discord', discord), cell('Help', help))],
                [h('span', { class: 'uw-sp' }), btn('Check again', () => send('recheck'), 'uw-btn-sm'), btn('Close', () => send('close'))],
                pill);
        }
        const brand = document.getElementById('brand');
        if (brand) set(brand, 'Uppwind Link', h('small', null, s.version));
        ready();
    }

    // ── la desinstallation ─────────────────────────────────────────────────────────────────────────────
    const U = { progress: [] };
    function uninstall(msg) {
        if (msg.type === 'progress') { U.progress.push(msg.line); }
        if (msg.type !== 'uninstall' && msg.type !== 'progress') return;
        if (msg.type === 'uninstall') Object.assign(U, msg);
        const plan = U.plan || { removes: [], puts_back: [], keeps: [] };
        const r = U.removal;
        if (U.running && !r) {
            render([h('h1', null, 'Uninstalling'), h('ul', { class: 'uw-progress' }, U.progress.map((l, i) => h('li', { class: i === U.progress.length - 1 ? 'is-now' : 'is-done' }, i === U.progress.length - 1 ? h('span', { class: 'uw-spin' }) : null, l)))], []);
            return;
        }
        if (r && r.blocked) {
            render([h('h1', null, 'Nothing was removed'), note('is-bad', r.blocked), h('p', { class: 'uw-lede' }, 'Close FL Studio, then try again.')],
                [btn('Cancel', () => send('cancel')), h('span', { class: 'uw-sp' }), btn('Try again', () => send('confirm'), 'uw-btn-danger uw-btn-big')]);
            ready();
            return;
        }
        if (r) {
            render([h('h1', null, r.ok ? 'Uppwind Link is uninstalled' : 'Uninstalled, with a problem'),
                r.simulated ? note('is-warn uw-banner', 'Sandbox: Ableton Live and FL Studio were not changed (simulated).') : null,
                r.errors.length ? h('div', { class: 'uw-card' }, list('is-bad', r.errors)) : null,
                r.todo.length ? [h('div', { class: 'uw-k' }, 'Left for you'), h('div', { class: 'uw-card' }, list('is-kept', r.todo))] : null,
                r.kept.length ? [h('div', { class: 'uw-k' }, 'Kept'), list('is-kept', r.kept)] : null,
                h('details', { class: 'uw-more' }, h('summary', null, 'What was done'), list('', r.done))],
                [h('span', { class: 'uw-sp' }), btn('Close', () => send('close'), 'uw-btn-primary uw-btn-big')]);
            ready();
            return;
        }
        render([h('h1', null, 'Uninstall Uppwind Link'),
            h('p', { class: 'uw-lede' }, 'This puts your computer back the way it was before Uppwind Link.'),
            plan.simulated ? note('is-warn uw-banner', `Sandbox: only ${plan.sandbox || 'its folder'} is touched; Ableton Live and FL Studio are not changed.`) : null,
            plan.fl_running ? note('is-warn', h('b', null, 'Close FL Studio first: '), 'its settings are put back before anything is removed.') : null,
            h('div', { class: 'uw-k' }, 'Removed'), list('', plan.removes),
            plan.puts_back.length ? [h('div', { class: 'uw-k' }, 'Put back'), list('', plan.puts_back)] : null,
            plan.keeps.length ? [h('div', { class: 'uw-k' }, 'Kept'), list('is-kept', plan.keeps)] : null],
        [btn('Cancel', () => send('cancel')), h('span', { class: 'uw-sp' }), btn('Uninstall', () => send('confirm'), 'uw-btn-danger uw-btn-big')]);
        ready();
    }

    const pages = { wizard, status, uninstall };
    window.__uwl = Object.freeze({
        recv(msg) {
            if (!msg || typeof msg !== 'object') return;
            if (msg.type === 'error') {
                // (11 nov.) une verification ou une mise a jour qui echoue : ses boutons reviennent
                if (checking || updBusy) { checking = false; updBusy = false; if (W.last && W.last.status) status(W.last); }
                toast(msg.message || 'Something went wrong.');
                return;
            }
            if (msg.type === 'copied') { toast('Copied.', 'is-neon'); return; }
            (pages[PAGE] || wizard)(msg);
        },
    });
    document.addEventListener('keydown', (e) => {
        if (e.key === 'Escape' && PAGE === 'status') { if (settingsOpen) toggleSettings(false); else send('close'); }
        if ((e.ctrlKey || e.metaKey) && ['p', 'f', 'g', 'r', 'u'].includes(e.key.toLowerCase())) e.preventDefault();
    });
    document.addEventListener('contextmenu', (e) => e.preventDefault());
    document.addEventListener('dragover', (e) => e.preventDefault());
    document.addEventListener('drop', (e) => e.preventDefault());
    send('init');
})();
