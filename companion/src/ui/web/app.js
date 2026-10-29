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
    function pluginLine(p) {
        if (p.not_in_build) return 'Not in this build of Uppwind Link (a development build).';
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
        const body = [h('div', null, pluginLine(p)), p.folder ? h('div', { class: 'uw-path' }, p.folder) : null];
        if (p.admin && on && (p.action === 'install' || p.action === 'update')) {
            body.push(note('', 'Windows asks for administrator rights once, to put them where FL Studio looks for VST3 plugins. Nothing else runs as administrator.'));
        }
        if (p.held) body.push(note('', 'A DAW still has an older copy loaded: restart it after the install.'));
        return h('div', { class: 'uw-card' + (p.selectable ? '' : ' is-off') },
            check('Uppwind Link plugins (VST3)', on, (v) => choose((x) => { x.daws = x.daws.filter((i) => i !== PLUGINS); if (v) x.daws.push(PLUGINS); }),
                { disabled: !p.selectable, sub: 'Needed for FL Studio. They also bring your DAW’s sound to the studio with any audio driver, ASIO included.' }),
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
    function status(msg) {
        if (msg.type !== 'status') return;
        W.last = msg;
        const s = msg.status;
        const L = s.live;
        const cell = (k, ...v) => [h('div', { class: 'uw-cell-k' }, k), h('div', { class: 'uw-cell-v' }, ...v)];
        const when = L.last_paired ? new Date(L.last_paired * 1000).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' }) : '';
        const studio = [
            L.sessions > 0 ? st('is-ok', `Paired · connected now${L.last_origin ? ` (${L.last_origin.replace(/^https?:\/\//, '')})` : ''}`)
                : L.paired_ever ? st('is-ok', `Paired · last connected ${when}`) : st('is-warn', 'Not paired yet'),
            s.code ? codeBox(s.code) : h('div', { class: 'uw-small' }, 'The pairing code appears once Uppwind Link runs.'),
            h('div', { class: 'uw-row' }, btn('Open the studio', () => send('open', { link: 'studio' }), 'uw-btn-sm'),
                h('span', { class: 'uw-small' }, 'Paste the code once: Settings › Uppwind Link › Plugin windows › Pair.')),
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
        const sb = s.sandbox ? note('is-warn uw-banner', h('b', null, 'Sandbox. '), `This Uppwind Link runs from ${s.sandbox}.`) : null;
        render([sb, h('div', { class: 'uw-grid' }, cell('Studio', studio), cell('DAWs', daws), cell('MIDI ports', ports), cell('Sound', sound), cell('Help', help))],
            [check('Start with Windows', s.autostart, (v) => send('autostart', { on: v })),
                h('button', { type: 'button', class: 'uw-link', onclick: () => send('setup') }, 'Set up again…'),
                h('span', { class: 'uw-sp' }), btn('Check again', () => send('recheck'), 'uw-btn-sm'), btn('Close', () => send('close'))],
            s.port ? h('span', { class: 'uw-pill is-ok' }, `Running · port ${s.port}`) : h('span', { class: 'uw-pill' }, 'Not running'));
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
            if (msg.type === 'error') { toast(msg.message || 'Something went wrong.'); return; }
            if (msg.type === 'copied') { toast('Copied.', 'is-neon'); return; }
            (pages[PAGE] || wizard)(msg);
        },
    });
    document.addEventListener('keydown', (e) => {
        if (e.key === 'Escape' && PAGE === 'status') send('close');
        if ((e.ctrlKey || e.metaKey) && ['p', 'f', 'g', 'r', 'u'].includes(e.key.toLowerCase())) e.preventDefault();
    });
    document.addEventListener('contextmenu', (e) => e.preventDefault());
    document.addEventListener('dragover', (e) => e.preventDefault());
    document.addEventListener('drop', (e) => e.preventDefault());
    send('init');
})();
