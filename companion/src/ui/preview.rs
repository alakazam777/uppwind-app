// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! --ui-preview <dossier> : les pages des fenetres en fichiers HTML, chacune avec un faux compagnon (un script qui
//! repond a la page avec un etat d'exemple), pour les voir dans un navigateur ou les capturer en Chrome headless.
//! Les memes gabarit, style et script que les vraies fenetres ; seuls les etats sont inventes (une machine type).

use super::ipc::Page;
use crate::tray::status::{Audio, DawLine, Live, ScriptLine, StatusModel};
use crate::wizard::flow::{Removal, UninstallPlan};
use crate::wizard::steps::{AuItem, Choices, DawItem, FlItem, Model, Outcome, PluginItem, Ports, Step, Todo, Wizard};
use serde_json::{json, Value};
use std::path::Path;

const UL: &str = r"%USERPROFILE%\Documents\Ableton\User Library";
const FLHW: &str = r"%USERPROFILE%\Documents\Image-Line\FL Studio\Settings\Hardware";
const VST3: &str = r"C:\Program Files\Common Files\VST3\Uppwind Link.vst3";
const CODE: &str = "UWC-47410-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"; // a made-up code

fn daws(fl_running: bool, live_running: bool) -> Vec<DawItem> {
    vec![
        DawItem {
            id: format!("ableton:{}", UL.to_ascii_lowercase()),
            daw: "ableton",
            name: "Ableton Live 12 Suite".into(),
            versions: vec!["12.4.3".into(), "12.4.6".into()],
            folder: format!(r"{UL}\Remote Scripts\UppwindLink"),
            status: "outdated".into(),
            installed_version: Some("1.0.0".into()),
            bundled_version: "1.2.0".into(),
            action: "update".into(),
            reason: "update 1.0.0 → 1.2.0".into(),
            running: live_running.then(|| "Ableton Live 12 Suite.exe".into()),
            selectable: true,
            fl_settings: None,
        },
        DawItem {
            id: "fl".into(),
            daw: "fl",
            name: "FL Studio 2026".into(),
            versions: vec!["26.1.7.5653".into()],
            folder: format!(r"{FLHW}\Uppwind Link"),
            status: "missing".into(),
            installed_version: None,
            bundled_version: "1.1.0".into(),
            action: "install".into(),
            reason: "install 1.1.0".into(),
            running: fl_running.then(|| "FL64.exe".into()),
            selectable: true,
            fl_settings: Some(FlItem { changes: 4, blocked_by: fl_running.then(|| "FL64.exe".into()), reason: "4 values to write".into() }),
        },
    ]
}

fn model(ports_ok: bool, fl_running: bool, live_running: bool) -> Model {
    Model {
        version: env!("CARGO_PKG_VERSION").into(),
        update_from: None,
        sandbox: None,
        simulate_daws: false,
        install_dir: r"%USERPROFILE%\AppData\Local\Uppwind Link".into(),
        daws: daws(fl_running, live_running),
        ports: Ports {
            link: ports_ok.then(|| "Uppwind Link".into()),
            back: ports_ok.then(|| "Uppwind Link Back".into()),
            near: vec![],
            loopmidi_installed: ports_ok,
            loopmidi_running: ports_ok,
        },
        plugins: PluginItem {
            available: true,
            not_in_build: false,
            status: "missing".into(),
            action: "install".into(),
            reason: "install them".into(),
            folder: VST3.into(),
            admin: true,
            selectable: true,
            held: false,
            au: None,
        },
    }
}

fn wizard_at(m: Model, step: Step, express: bool, outcome: Option<Outcome>) -> Value {
    let mut w = Wizard::new(m);
    w.step = step;
    w.express = express;
    w.outcome = outcome;
    serde_json::to_value(&w).unwrap()
}

fn state(w: Value, progress: &[&str], code: Option<&str>) -> Value {
    json!({ "type": "state", "wizard": w, "progress": progress, "code": code })
}

fn outcome() -> Outcome {
    let t = |id: &str, title: &str, lines: &[&str]| Todo { id: id.into(), title: title.into(), lines: lines.iter().map(|l| l.to_string()).collect() };
    Outcome {
        ok: true,
        simulated: false,
        done: vec![],
        errors: vec![],
        scripts: vec![
            format!(r"Ableton Live: script 1.2.0 installed in {UL}\Remote Scripts\UppwindLink (it replaced version 1.0.0; the old folder is kept in %USERPROFILE%\AppData\Local\Uppwind Link\backups\ableton-20261024-151200\UppwindLink)."),
            format!(r"FL Studio: script 1.1.0 installed in {FLHW}\Uppwind Link."),
        ],
        fl: "written".into(),
        fl_lines: vec!["FL Studio’s MIDI settings were written (4 values); a copy of the whole Devices key is in backups\\fl-registry-20261024-151200.reg. Uninstalling Uppwind Link puts them back.".into()],
        plugins: "installed".into(),
        plugin_lines: vec![format!("Uppwind Link plugins (Uppwind Link Return, Uppwind Link Notes) installed in {VST3} (with administrator rights).")],
        plugins_restart: false,
        backups: Some(r"%USERPROFILE%\AppData\Local\Uppwind Link\backups".into()),
        autostart: true,
        listed: true,
        port: Some(47410),
        todo: vec![
            t("live", "In Ableton Live", &[
                "Settings › Tempo & MIDI (Live 11: Preferences › Link, Tempo & MIDI):",
                "Control Surface: UppwindLink — Input “Uppwind Link”, Output “Uppwind Link Back”.",
                "MIDI Ports: In “Uppwind Link”: Track and Remote on. Out “Uppwind Link Back”: Remote on.",
            ]),
            t("live-dev", "One Control Surface is enough", &["UppwindLink also opens your plugins’ windows for Uppwind Link. Had UppwindLinkDev in a second slot? Set that slot to None."]),
            t("fl", "Start FL Studio", &["Its MIDI settings are ready: Input “Uppwind Link”, Controller type “Uppwind Link (user)”, port 77."]),
            t("plugins-scan", "Find the Uppwind Link plugins in your DAW", &[
                "FL Studio: Options › Manage plugins › Find installed plugins (or restart FL Studio).",
                "Ableton Live: Settings › Plug-Ins: Use VST3 Plug-In System Folders on, then Rescan.",
                "Then put Uppwind Link Return on the master: your DAW’s sound comes back with any audio driver, ASIO included.",
            ]),
            t("pair", "Pair the studio", &["Open the studio › Settings › Uppwind Link › Plugin windows: paste the code, then Pair."]),
        ],
    }
}

/// (9 nov., audit F1/F4) La fenetre d'etat armee, pendant que le studio filme Diva, y joue et ecoute le DAW.
fn status_armed() -> StatusModel {
    let mut m = status_model(false);
    m.remote = crate::tray::status::Remote { armed: true, idle_left_min: None, asked_ago_s: None, idle_disarm_min: 10 };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    m.live.watches = 1;
    m.live.watching = Some("Diva/2-Lead".into());
    m.live.audio_streams = 1;
    m.live.last_input = Some(now);
    m
}

fn status_model(trouble: bool) -> StatusModel {
    StatusModel {
        version: env!("CARGO_PKG_VERSION").into(),
        sandbox: None,
        port: 47410,
        code: CODE.into(),
        // (9 nov., audit F1) l'apercu : desarme, un studio vient de demander (la vue la plus utile a relire)
        remote: crate::tray::status::Remote { armed: false, idle_left_min: None, asked_ago_s: Some(20), idle_disarm_min: 10 },
        live: Live { sessions: 1, paired_ever: true, last_origin: Some("https://beta.uppwind.com".into()), last_paired: Some(1_792_852_800), audio_trouble: None, ..Live::default() },
        daws: vec![
            DawLine {
                daw: "ableton",
                name: "Ableton Live 12 Suite".into(),
                versions: vec!["12.4.3".into(), "12.4.6".into()],
                running: Some("Ableton Live 12 Suite.exe".into()),
                scripts: vec![ScriptLine { folder: format!(r"{UL}\Remote Scripts\UppwindLink"), status: "up-to-date".into(), installed: Some("1.2.0".into()), bundled: "1.2.0".into() }],
            },
            DawLine {
                daw: "fl",
                name: "FL Studio 2026".into(),
                versions: vec!["26.1.7.5653".into()],
                running: None,
                scripts: vec![ScriptLine { folder: format!(r"{FLHW}\Uppwind Link"), status: "up-to-date".into(), installed: Some("1.1.0".into()), bundled: "1.1.0".into() }],
            },
        ],
        ports: Ports { link: Some("Uppwind Link".into()), back: if trouble { None } else { Some("Uppwind Link Back".into()) }, near: vec![], loopmidi_installed: true, loopmidi_running: true },
        audio: Audio { available: true, error: None, trouble: trouble.then(|| "cannot capture Ableton Live 12 Suite.exe: the audio engine refused the process loopback (0x88890008)".into()) },
        autostart: true,
        // (11 nov.) la presence Discord : off (la vue d'un premier lancement)
        discord: crate::tray::status::Discord::default(),
        update: crate::update::View { supported: true, ..Default::default() },
    }
}

/// (11 nov., update.rs) Une version plus recente vient d'etre lue sur GitHub.
fn status_update_found() -> StatusModel {
    let mut m = status_model(false);
    m.update = crate::update::View { supported: true, latest: Some("0.4.0".into()), newer: true, checked_ago_s: Some(4), ..Default::default() };
    m
}

/// (11 nov.) Le panneau Settings : a jour, la case automatique cochee, Discord permis.
fn status_settings() -> StatusModel {
    let mut m = status_model(false);
    m.update = crate::update::View { supported: true, auto: true, latest: Some(env!("CARGO_PKG_VERSION").into()), checked_ago_s: Some(7200), ..Default::default() };
    m.discord = crate::tray::status::Discord { allowed: true, discord: "connected", shown: true };
    m
}

fn plan() -> UninstallPlan {
    UninstallPlan {
        removes: vec![
            r"Uppwind Link, its Start menu shortcut and its start with Windows (%USERPROFILE%\AppData\Local\Uppwind Link).".into(),
            format!(r"The Ableton Live script it installed ({UL}\Remote Scripts\UppwindLink)."),
            format!(r"The FL Studio script it installed ({FLHW}\Uppwind Link)."),
            format!("The Uppwind Link plugins ({VST3}): Windows asks for administrator rights."),
            "Its log and its window data.".into(),
            "Its entry in Windows Settings › Apps.".into(),
        ],
        puts_back: vec!["The Ableton Live script folder that was there before Uppwind Link.".into(), "FL Studio’s MIDI settings, as they were before Uppwind Link changed them.".into()],
        keeps: vec![
            r"Copies of the files it replaced (%USERPROFILE%\AppData\Local\Uppwind Link\backups).".into(),
            r"Your pairing code (%USERPROFILE%\.uppwind-companion.json): the studio stays paired if you install Uppwind Link again.".into(),
        ],
        fl_running: None,
        simulated: false,
        sandbox: None,
    }
}

/// Le faux compagnon : il repond a « init » avec ces messages, puis clique `click` s'il est donne.
fn bridge(msgs: &[Value], click: Option<&str>) -> String {
    let m = serde_json::to_string(msgs).unwrap().replace("</", "<\\/");
    let c = serde_json::to_string(&click).unwrap();
    format!(
        "(()=>{{const msgs={m};const click={c};window.ipc={{postMessage(raw){{const x=JSON.parse(raw);if(x.cmd==='init')setTimeout(()=>{{for(const v of msgs)window.__uwl.recv(v);if(click){{const el=document.querySelector(click);if(el){{el.click();const mn=document.getElementById('main');mn.scrollTop=mn.scrollHeight;}}}}}},0);}}}};}})();"
    )
}

/// Les pages d'exemple : (nom du fichier, page, messages, clic).
/// (10 nov.) Les plugins sur un Mac : le VST3 deja a jour, leurs Audio Units (Logic Pro) pas encore la.
fn mac_plugins() -> PluginItem {
    PluginItem {
        available: true,
        not_in_build: false,
        status: "up-to-date".into(),
        action: "none".into(),
        reason: String::new(),
        folder: "~/Library/Audio/Plug-Ins/VST3/Uppwind Link.vst3".into(),
        admin: false,
        selectable: true,
        held: false,
        au: Some(AuItem {
            status: "missing".into(),
            action: "install".into(),
            reason: "install them".into(),
            folder: "~/Library/Audio/Plug-Ins/Components/Uppwind Link.component".into(),
        }),
    }
}

pub fn pages() -> Vec<(&'static str, Page, Vec<Value>, Option<&'static str>)> {
    let c = |m: &Model| Choices::defaults(m);
    let mut daws_w = Wizard::new(model(false, true, true));
    daws_w.step = Step::Daws;
    daws_w.choices = c(&daws_w.model);
    let ports_w = wizard_at(model(false, false, false), Step::Ports, false, None);
    // (10 nov.) macOS : le VST3 a jour, leurs Audio Units (Logic Pro) a poser — la meme case dit les deux
    let mut daws_mac = Wizard::new(Model { plugins: mac_plugins(), ..model(false, false, false) });
    daws_mac.step = Step::Daws;
    daws_mac.choices = c(&daws_mac.model);
    vec![
        ("wizard-1-welcome", Page::Wizard, vec![state(wizard_at(model(false, false, false), Step::Welcome, false, None), &[], None)], None),
        ("wizard-2-daws", Page::Wizard, vec![state(serde_json::to_value(&daws_w).unwrap(), &[], None)], None),
        ("wizard-2-daws-mac", Page::Wizard, vec![state(serde_json::to_value(&daws_mac).unwrap(), &[], None)], None),
        ("wizard-3-ports", Page::Wizard, vec![state(ports_w, &[], None)], None),
        ("wizard-4-options", Page::Wizard, vec![state(wizard_at(model(true, false, false), Step::Options, false, None), &[], None)], None),
        (
            "wizard-5-progress",
            Page::Wizard,
            vec![state(wizard_at(model(true, false, false), Step::Progress, false, None), &["Copying Uppwind Link to your user folder", "Setting up your DAWs", "Start with Windows"], None)],
            None,
        ),
        ("wizard-6-done", Page::Wizard, vec![state(wizard_at(model(true, false, false), Step::Done, false, Some(outcome())), &[], Some(CODE))], None),
        ("status", Page::Status, vec![json!({ "type": "status", "status": status_model(false) })], None),
        // (9 nov., audit F1/F4) arme, et ce qui se passe : une fenetre filmee, des gestes, le son du DAW
        ("status-armed", Page::Status, vec![json!({ "type": "status", "status": status_armed() })], None),
        // (9 nov., audit F2) « New code… » demande confirmation (le studio sera deconnecte)
        ("status-new-code", Page::Status, vec![json!({ "type": "status", "status": status_model(false) })], Some(".uw-cell-v:nth-child(4) .uw-link")),
        ("status-sound-help", Page::Status, vec![json!({ "type": "status", "status": status_model(true) })], Some(".uw-cell-v:last-child .uw-link")),
        // (11 nov.) une mise a jour trouvee ; le panneau Settings (le bouton de la barre de titre)
        ("status-update", Page::Status, vec![json!({ "type": "status", "status": status_update_found() })], None),
        ("status-settings", Page::Status, vec![json!({ "type": "status", "status": status_settings() })], Some(".uw-settings-btn")),
        ("uninstall-1-confirm", Page::Uninstall, vec![json!({ "type": "uninstall", "plan": plan(), "removal": null, "running": false })], None),
        (
            "uninstall-2-done",
            Page::Uninstall,
            vec![json!({ "type": "uninstall", "plan": plan(), "running": false, "removal": Removal {
                ok: true,
                simulated: false,
                done: vec!["FL Studio's settings, put back as they were before the companion changed them.".into(), "Ableton Live: removed the script from its Remote Scripts folder (a copy is kept).".into(), "Removed from Windows' Installed apps.".into()],
                errors: vec![],
                kept: plan().keeps,
                todo: vec!["In Ableton Live: the UppwindLink Control Surface is gone at the next start; set that slot to None.".into(), "loopMIDI stays installed (it is not part of Uppwind Link): remove it from Installed apps if you do not need it.".into()],
                blocked: None,
            } })],
            None,
        ),
    ]
}

/// Ecrit les pages dans `dir` → 0.
pub fn write_all(dir: &Path) -> i32 {
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("cannot create {} ({e})", dir.display());
        return 1;
    }
    for (name, page, msgs, click) in pages() {
        let p = dir.join(format!("{name}.html"));
        if let Err(e) = std::fs::write(&p, super::html(page, &super::nonce(), Some(&bridge(&msgs, click)))) {
            eprintln!("cannot write {} ({e})", p.display());
            return 1;
        }
        println!("{}", p.display());
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preview_page_is_a_whole_page_with_its_messages() {
        let pages = pages();
        assert_eq!(pages.len(), 15);
        for (name, page, msgs, _) in &pages {
            assert!(!msgs.is_empty(), "{name}");
            let h = super::super::html(*page, "n", Some(&bridge(msgs, None)));
            assert!(h.contains(&format!("data-page=\"{}\"", page.name())), "{name}");
            assert!(!h.contains("</script>\")") && h.matches("<script").count() == 2, "{name}: the mock, then the page");
        }
    }
}
