// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! --ui-preview <dossier> : les pages des fenetres en fichiers HTML, chacune avec un faux compagnon (un script qui
//! repond a la page avec un etat d'exemple), pour les voir dans un navigateur ou les capturer en Chrome headless.
//! Les memes gabarit, style et script que les vraies fenetres ; seuls les etats sont inventes (une machine type).

use super::ipc::Page;
use crate::tray::status::{Audio, DawLine, Live, ScriptLine, StatusModel};
use crate::wizard::flow::{Removal, UninstallPlan};
use crate::wizard::steps::{Choices, DawItem, FlItem, Model, Outcome, PluginItem, Ports, Step, Todo, Wizard};
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

fn status_model(trouble: bool) -> StatusModel {
    StatusModel {
        version: env!("CARGO_PKG_VERSION").into(),
        sandbox: None,
        port: 47410,
        code: CODE.into(),
        live: Live { sessions: 1, paired_ever: true, last_origin: Some("https://uppwind.lucasmoreau.fr".into()), last_paired: Some(1_792_852_800), audio_trouble: None },
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
    }
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
pub fn pages() -> Vec<(&'static str, Page, Vec<Value>, Option<&'static str>)> {
    let c = |m: &Model| Choices::defaults(m);
    let mut daws_w = Wizard::new(model(false, true, true));
    daws_w.step = Step::Daws;
    daws_w.choices = c(&daws_w.model);
    let ports_w = wizard_at(model(false, false, false), Step::Ports, false, None);
    vec![
        ("wizard-1-welcome", Page::Wizard, vec![state(wizard_at(model(false, false, false), Step::Welcome, false, None), &[], None)], None),
        ("wizard-2-daws", Page::Wizard, vec![state(serde_json::to_value(&daws_w).unwrap(), &[], None)], None),
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
        ("status-sound-help", Page::Status, vec![json!({ "type": "status", "status": status_model(true) })], Some(".uw-cell-v:last-child .uw-link")),
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
        assert_eq!(pages.len(), 10);
        for (name, page, msgs, _) in &pages {
            assert!(!msgs.is_empty(), "{name}");
            let h = super::super::html(*page, "n", Some(&bridge(msgs, None)));
            assert!(h.contains(&format!("data-page=\"{}\"", page.name())), "{name}");
            assert!(!h.contains("</script>\")") && h.matches("<script").count() == 2, "{name}: the mock, then the page");
        }
    }
}
