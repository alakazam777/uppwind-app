// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! `--setup` : l'installation d'Uppwind Link par le compagnon (24 oct., premiere etape du « tout-en-un »).
//!
//!   uppwind-companion --setup                    regarde et dit ce qu'il ferait (rien n'est change : c'est le defaut)
//!   uppwind-companion --setup --apply            installe ou met a jour les scripts de Live et de FL
//!   uppwind-companion --setup --remove [--apply] retire les scripts (une copie est gardee)
//!   options : --json (le rapport en JSON, pour le guide du studio) · --daw ableton|fl|all · --force (remplace une
//!             version installee plus recente)
//!             --fl-settings : ecrit aussi les reglages MIDI et audio de FL (registre, FL ferme ; fl_settings.rs) ;
//!             avec --remove : les remet comme avant · --no-fl-settings : sans poser la question
//!             --fl-free-midi all|<nom> : desactive dans FL un peripherique MIDI USB, pour que le navigateur l'ouvre
//!   Sans --fl-settings ni --no-fl-settings, --apply dans une console demande (o/N) ; l'installeur passera l'option.
//!
//! Ce que `--setup` regarde (en lecture seule) : les DAW installes et leurs versions, la User Library de Live
//! (Library.cfg) et ce que Live a applique (Log.txt), le dossier de donnees de FL et ses reglages MIDI/audio
//! (registre), les ports MIDI (loopMIDI), le retour du son (par le compagnon ; VB-CABLE seulement en secours), les
//! scripts installes.
//! Ce qu'il peut ecrire, et seulement avec --apply : le dossier du script dans la User Library de Live
//! (Remote Scripts\UppwindLink) et dans le dossier de FL (Settings\Hardware\Uppwind Link), et une copie de ce qui
//! etait la dans %LOCALAPPDATA%\Uppwind Link\backups ; avec --fl-settings en plus, les reglages de FL sous
//! HKCU\Software\Image-Line\FL Studio <NN> (Devices et la case « Auto close »), FL ferme, apres une copie .reg.
//! Jamais les preferences de Live, jamais un pilote, jamais HKLM ni rien qui demande l'administrateur.
//! `--uninstall` (main.rs) remet d'abord les reglages de FL que le compagnon a ecrits.

pub mod ableton;
pub mod env;
pub mod fl;
pub mod fl_settings;
pub mod scripts;
pub mod winsys;

use env::{programs, Endpoint, Probe, Program};
use scripts::{Action, Daw, Status, Target};
use serde::Serialize;
use std::path::Path;

pub use scripts::cmp_versions;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// en place
    Ok,
    /// a faire (le guide dit comment)
    Todo,
    /// en place mais faux, ou risque
    Warn,
    /// a savoir
    Info,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub id: String,
    pub state: State,
    pub text: String,
}

impl Check {
    fn new(id: &str, state: State, text: &str) -> Check {
        Check { id: id.into(), state, text: text.into() }
    }
    pub fn ok(id: &str, text: &str) -> Check {
        Check::new(id, State::Ok, text)
    }
    pub fn todo(id: &str, text: &str) -> Check {
        Check::new(id, State::Todo, text)
    }
    pub fn warn(id: &str, text: &str) -> Check {
        Check::new(id, State::Warn, text)
    }
    pub fn info(id: &str, text: &str) -> Check {
        Check::new(id, State::Info, text)
    }
}

/// « Uppwind Link » (Windows), « IAC Driver (Uppwind Link) » (Live sur macOS), « IAC Driver Uppwind Link » (FL, Chrome).
pub fn is_link_port(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("uppwind link") && !n.contains("back")
}
pub fn is_back_port(name: &str) -> bool {
    name.to_ascii_lowercase().contains("uppwind link back")
}

/// Deux chemins designent-ils le meme dossier ? (Windows : sans casse, / ou \)
pub fn same_path(a: &Path, b: &Path) -> bool {
    let n = |p: &Path| {
        let s = p.to_string_lossy().replace('/', "\\");
        let s = s.trim_end_matches('\\').to_string();
        if cfg!(windows) {
            s.to_ascii_lowercase()
        } else {
            s
        }
    };
    n(a) == n(b)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Install,
    Remove,
}

#[derive(Debug, Clone)]
pub struct Opts {
    pub apply: bool,
    pub mode: Mode,
    pub json: bool,
    pub force: bool,
    pub daws: Vec<Daw>,
    /// Some(true) : --fl-settings ; Some(false) : --no-fl-settings ; None : demander (console), sinon non
    pub fl_settings: Option<bool>,
    /// --fl-free-midi : « all » ou des noms de peripheriques USB a desactiver dans FL
    pub free_midi: Vec<String>,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts { apply: false, mode: Mode::Install, json: false, force: false, daws: vec![Daw::Ableton, Daw::Fl], fl_settings: None, free_midi: vec![] }
    }
}

pub const HELP: &str = "uppwind-companion --setup [--apply] [--remove] [--json] [--daw ableton|fl|all] [--force]\n\
  [--fl-settings | --no-fl-settings] [--fl-free-midi all|NAME]\n\
  Without --apply nothing is changed: it says what it would do.\n\
  --fl-settings also writes FL Studio's MIDI and audio settings (FL closed, a backup first); with --remove it puts\n\
  them back. --fl-free-midi disables a USB MIDI device in FL, so the browser can open it.";

pub fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--setup" => {}
            "--apply" => o.apply = true,
            "--dry-run" => o.apply = false,
            "--remove" | "--uninstall" => o.mode = Mode::Remove,
            "--json" => o.json = true,
            "--force" => o.force = true,
            "--fl-settings" => o.fl_settings = Some(true),
            "--no-fl-settings" => o.fl_settings = Some(false),
            "--fl-free-midi" => match it.next() {
                Some(n) if !n.starts_with("--") && !n.trim().is_empty() => o.free_midi.push(n.trim().to_string()),
                _ => return Err("--fl-free-midi takes all, or the name of a MIDI device as FL lists it".into()),
            },
            "--daw" => {
                o.daws = match it.next().map(|s| s.to_ascii_lowercase()).as_deref() {
                    Some("ableton") | Some("live") => vec![Daw::Ableton],
                    Some("fl") | Some("fl-studio") => vec![Daw::Fl],
                    Some("all") => vec![Daw::Ableton, Daw::Fl],
                    _ => return Err("--daw is ableton, fl or all".into()),
                }
            }
            "-h" | "--help" => return Err(HELP.into()),
            other => return Err(format!("unknown --setup option {other}\n{HELP}")),
        }
    }
    Ok(o)
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Os {
    pub name: String,
    pub build: u32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Midi {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub loopmidi: Option<Program>,
    pub loopmidi_running: bool,
    pub tevirtualmidi: Option<Program>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Audio {
    pub vbcable: Option<Program>,
    pub cable_input: Option<Endpoint>,
    pub cable_output: Option<Endpoint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub companion: &'static str,
    /// « dry-run » ou « apply »
    pub mode: &'static str,
    /// « install » ou « remove »
    pub intent: &'static str,
    /// true : la fausse machine de UWC_SETUP_ROOT
    pub fake_machine: bool,
    pub os: Os,
    pub midi: Midi,
    pub audio: Audio,
    pub ableton: ableton::Ableton,
    pub fl: fl::Fl,
    pub scripts: Vec<Target>,
    /// les reglages de FL que --fl-settings ecrirait ou remettrait (fl_settings.rs)
    pub fl_settings: fl_settings::FlSettings,
    pub checks: Vec<Check>,
    pub done: Vec<String>,
    pub errors: Vec<String>,
}

fn os_of(probe: &Probe) -> Os {
    let k = r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let build: u32 = probe.reg.str(k, "CurrentBuild").and_then(|b| b.parse().ok()).unwrap_or(0);
    let mut name = probe.reg.str(k, "ProductName").unwrap_or_else(|| std::env::consts::OS.to_string());
    // Windows 11 garde « Windows 10 » dans ProductName : c'est le numero de build qui tranche
    if build >= 22000 {
        name = name.replace("Windows 10", "Windows 11");
    }
    if let Some(v) = probe.reg.str(k, "DisplayVersion") {
        name = format!("{name} {v}");
    }
    Os { name, build }
}

fn machine_checks(r: &Report) -> Vec<Check> {
    let mut c = Vec::new();
    let has = |v: &[String], f: fn(&str) -> bool| v.iter().find(|n| f(n)).cloned();
    let ins = (has(&r.midi.inputs, is_link_port), has(&r.midi.inputs, is_back_port));
    let outs = (has(&r.midi.outputs, is_link_port), has(&r.midi.outputs, is_back_port));
    let all = ins.0.is_some() && ins.1.is_some() && outs.0.is_some() && outs.1.is_some();
    if all {
        c.push(Check::ok("midi.ports", "MIDI ports \"Uppwind Link\" and \"Uppwind Link Back\" are there (in and out)."));
    } else {
        let mut miss = Vec::new();
        if ins.0.is_none() || outs.0.is_none() {
            miss.push("\"Uppwind Link\"");
        }
        if ins.1.is_none() || outs.1.is_none() {
            miss.push("\"Uppwind Link Back\"");
        }
        let how = match (&r.midi.loopmidi, r.midi.loopmidi_running) {
            (Some(_), true) => "create them in loopMIDI (type the name, click +)",
            (Some(_), false) => "start loopMIDI (its tray icon has Autostart), then create them",
            (None, _) if cfg!(windows) => "install loopMIDI (tobias-erichsen.de), then create them",
            (None, _) => "Audio MIDI Setup › IAC Driver: Device is online, add the two buses",
        };
        c.push(Check::todo("midi.ports", &format!("MIDI port{} {} missing: {how}.", if miss.len() > 1 { "s" } else { "" }, miss.join(" and "))));
    }
    if all && r.midi.loopmidi.is_some() && !r.midi.loopmidi_running {
        c.push(Check::info("midi.loopmidi", "The ports exist but loopMIDI is not running: another driver provides them."));
    }
    if cfg!(windows) {
        // le son du DAW revient par le compagnon lui-meme (la capture d'un processus, Windows 10 2004 et apres) :
        // VB-CABLE n'est qu'un secours, propose seulement quand ce Windows ne sait pas faire (decision du 24 oct.)
        let cable = match (&r.audio.cable_input, &r.audio.cable_output) {
            (Some(i), Some(o)) => Some((i, o)),
            _ => None,
        };
        if r.os.build >= NO_CABLE_BUILD {
            c.push(Check::ok(
                "audio.return",
                "Audio return: the companion captures your DAW's sound itself, no virtual cable needed. In the DAW, play through Windows (not ASIO), to an output you don't listen to.",
            ));
            if let Some((i, o)) = cable {
                c.push(Check::info("audio.cable", &format!("VB-CABLE is installed (\"{}\", \"{}\"): not needed, only a fallback if the companion's return does not work for you.", i.name, o.name)));
                for e in [i, o] {
                    if e.rate != 0 && e.rate != 48000 {
                        c.push(Check::info("audio.rate", &format!("\"{}\" runs at {} Hz: if you use VB-CABLE, set 48000 Hz (Sound control panel › Properties › Advanced).", e.name, e.rate)));
                    }
                }
            }
        } else {
            let this = if r.os.build > 0 { format!("this Windows (build {})", r.os.build) } else { "this Windows".to_string() };
            match cable {
                Some((i, o)) => {
                    c.push(Check::ok("audio.return", &format!("Audio return: VB-CABLE (\"{}\" and \"{}\"): {this} cannot capture a single app's sound.", i.name, o.name)));
                    for e in [i, o] {
                        if e.rate != 0 && e.rate != 48000 {
                            c.push(Check::todo("audio.rate", &format!("\"{}\" runs at {} Hz: set 48000 Hz (Sound control panel › Properties › Advanced).", e.name, e.rate)));
                        }
                    }
                }
                None if r.audio.vbcable.is_some() => c.push(Check::todo(
                    "audio.return",
                    &format!("Audio return: {this} cannot capture a single app's sound, and VB-CABLE is installed but its devices are missing or disabled: reboot, or enable CABLE Input / CABLE Output in the Sound control panel."),
                )),
                None => c.push(Check::todo(
                    "audio.return",
                    &format!("Audio return: {this} cannot capture a single app's sound (Windows 10 version 2004 or later can): update Windows, or install VB-CABLE as a fallback (vb-audio.com/Cable: run its setup as administrator, then reboot)."),
                )),
            }
        }
    }
    c
}

/// Le premier Windows qui sait capturer le son d'un seul processus (Windows 10 version 2004, build 19041) : le retour
/// audio par le compagnon, sans cable.
pub const NO_CABLE_BUILD: u32 = 19041;

fn script_checks(r: &Report) -> Vec<Check> {
    let mut c = Vec::new();
    for t in &r.scripts {
        let id = format!("{}.script", t.daw.key());
        let st = t.after.unwrap_or(t.status);
        let what = format!("{} script in {}", t.daw.label(), t.dir.display());
        c.push(match st {
            Status::UpToDate => Check::ok(&id, &format!("{what}: up to date ({}).", t.bundled_version)),
            Status::Missing if r.intent == "remove" => Check::ok(&id, &format!("{what}: not installed.")),
            Status::Missing => Check::todo(&id, &format!("{what}: not installed.")),
            Status::NoLibrary => Check::todo(&id, &format!("{what}: {}.", t.reason)),
            Status::Newer => Check::info(&id, &format!("{what}: {} is newer than this companion's {}.", t.installed_version.as_deref().unwrap_or("?"), t.bundled_version)),
            Status::Outdated | Status::Different => Check::todo(
                &id,
                &format!("{what}: {} installed, the companion has {} ({} file(s) differ).", t.installed_version.as_deref().unwrap_or("?"), t.bundled_version, t.files.iter().filter(|f| f.state != "same" && f.state != "extra").count()),
            ),
        });
        if !t.neighbours.is_empty() {
            c.push(Check::info(&format!("{}.neighbours", t.daw.key()), &format!("Also in {}: {} (left alone).", t.dir.parent().map(|p| p.display().to_string()).unwrap_or_default(), t.neighbours.join(", "))));
        }
        let changes = matches!(t.action, Action::Install | Action::Update | Action::Remove);
        let running = match t.daw {
            Daw::Ableton => r.ableton.running.clone(),
            Daw::Fl => r.fl.running.clone(),
        };
        if changes {
            if let Some(p) = running {
                c.push(Check::info(&format!("{}.restart", t.daw.key()), &format!("{} is running ({p}): restart it after the change, it reads its scripts when it starts.", t.daw.label())));
            }
        }
    }
    c
}

/// Regarde la machine et prepare le plan, sans rien ecrire.
pub fn gather(probe: &Probe, o: &Opts, stamp: &str) -> Report {
    let reg = probe.reg.as_ref();
    let midi = Midi {
        inputs: probe.midi_in.clone(),
        outputs: probe.midi_out.clone(),
        loopmidi: programs(reg, |n| n == "loopMIDI").into_iter().next(),
        loopmidi_running: probe.running("loopMIDI.exe"),
        tevirtualmidi: programs(reg, |n| n.starts_with("teVirtualMIDI")).into_iter().next(),
    };
    let audio = Audio {
        vbcable: programs(reg, |n| n.to_ascii_uppercase().starts_with("VBCABLE")).into_iter().next(),
        cable_input: probe.render.iter().find(|e| e.active && e.name.starts_with("CABLE Input")).cloned(),
        cable_output: probe.capture.iter().find(|e| e.active && e.name.starts_with("CABLE Output")).cloned(),
    };
    let ab = ableton::detect(probe);
    let flx = fl::detect(probe);
    let mut targets = Vec::new();
    if o.daws.contains(&Daw::Ableton) && ab.found() {
        let b = scripts::bundle(Daw::Ableton);
        for ul in &ab.user_libraries {
            targets.push(scripts::inspect(&b, &ul.join("Remote Scripts").join(ableton::FOLDER), ul));
        }
    }
    if o.daws.contains(&Daw::Fl) && flx.found() {
        if let (Some(hw), Some(data)) = (&flx.hardware, &flx.user_data) {
            targets.push(scripts::inspect(&scripts::bundle(Daw::Fl), &hw.join(fl::FOLDER), data));
        }
    }
    let backups = probe.paths.backups();
    for t in &mut targets {
        scripts::plan(t, o.mode, o.force, &backups, stamp);
    }
    // deux cibles le meme jour dans la meme seconde : des noms de sauvegarde distincts
    let mut seen: Vec<std::path::PathBuf> = Vec::new();
    for t in &mut targets {
        if let Some(b) = t.backup.clone() {
            let mut b2 = b.clone();
            let mut n = 2;
            while seen.iter().any(|s| s == &b2) {
                b2 = b.parent().unwrap().with_file_name(format!("{}-{n}", b.parent().unwrap().file_name().unwrap().to_string_lossy())).join(b.file_name().unwrap());
                n += 1;
            }
            seen.push(b2.clone());
            t.backup = Some(b2);
        }
    }
    let fls = fl_settings::prepare(reg, fl::IL, &flx, probe, o, &backups, stamp);
    let mut r = Report {
        companion: env!("CARGO_PKG_VERSION"),
        mode: if o.apply { "apply" } else { "dry-run" },
        intent: if o.mode == Mode::Remove { "remove" } else { "install" },
        fake_machine: probe.fake,
        os: os_of(probe),
        midi,
        audio,
        ableton: ab,
        fl: flx,
        scripts: targets,
        fl_settings: fls,
        checks: vec![],
        done: vec![],
        errors: vec![],
    };
    finish_checks(&mut r);
    r
}

fn finish_checks(r: &mut Report) {
    let mut c = machine_checks(r);
    c.extend(script_checks(r));
    if r.ableton.found() {
        c.extend(ableton::checks(&r.ableton));
    }
    if r.fl.found() {
        c.extend(fl::checks(&r.fl));
    }
    c.extend(fl_settings_checks(r));
    r.checks = c;
}

fn fl_settings_checks(r: &Report) -> Vec<Check> {
    let f = &r.fl_settings;
    let mut c = Vec::new();
    let base = f.changes.iter().filter(|x| x.opt_in == fl_settings::OPT_SETTINGS).count();
    match (r.intent, f.action.as_str()) {
        ("install", "offer") if base > 0 => c.push(Check::info(
            "fl.write",
            &format!("--fl-settings can write {base} of FL Studio's settings for you (FL closed; a backup is made first, uninstalling the companion puts them back)."),
        )),
        (_, "blocked") => c.push(Check::warn("fl.write", &format!("{}.", f.reason))),
        ("remove", "offer") => c.push(Check::info("fl.write", "The companion changed FL Studio's settings: --remove --fl-settings --apply puts them back.")),
        _ => {}
    }
    if !f.usb_inputs.is_empty() && r.intent == "install" && !f.done {
        let names: Vec<String> = f.usb_inputs.iter().map(|u| format!("\"{}\"{}", u.name, if u.plugged { "" } else { " (not plugged in now)" })).collect();
        c.push(Check::info(
            "fl.usb",
            &format!("FL Studio holds {} (enabled in its MIDI settings): the browser cannot open {} while FL runs. --fl-free-midi all disables {} in FL.", names.join(", "), if names.len() == 1 { "it" } else { "them" }, if names.len() == 1 { "it" } else { "them" }),
        ));
    }
    if r.intent == "install" {
        if let Some(p) = f.previous.iter().find(|p| p.restored_at.is_none() && p.state == "written") {
            c.push(Check::info(
                "fl.written",
                &format!("The companion wrote FL Studio settings on {} (backup {}): uninstalling it, or --setup --remove --fl-settings --apply, puts them back.", fl_settings::pretty_stamp(&p.written_at), p.backup_reg.display()),
            ));
        }
    }
    c
}

/// --apply : fait le plan, relit chaque dossier.
pub fn apply(r: &mut Report) {
    for t in &mut r.scripts {
        match scripts::apply(t) {
            Ok(lines) => r.done.extend(lines),
            Err(e) => r.errors.push(format!("{}: {e}", t.daw.label())),
        }
        let again = scripts::inspect(&scripts::bundle(t.daw), &t.dir, &t.library);
        t.after = Some(again.status);
    }
    finish_checks(r);
}

/// FL tourne-t-il en ce moment ? (relu juste avant d'ecrire ; la fausse machine garde sa liste)
pub(crate) fn fl_running_now(probe: &Probe) -> Option<String> {
    if probe.fake {
        fl::fl_process(&probe.processes)
    } else {
        fl::fl_process(&winsys::processes())
    }
}

/// --apply avec --fl-settings (ou --fl-free-midi) : ecrit les reglages de FL ; avec --remove : les remet. Puis relit FL.
pub fn apply_fl_settings(r: &mut Report, probe: &Probe, running: &dyn Fn() -> Option<String>, stamp: &str) {
    let reg = probe.reg.as_ref();
    if r.intent == "install" {
        if r.fl_settings.changes.iter().any(|c| c.chosen) {
            match fl_settings::write(reg, &mut r.fl_settings, running, stamp) {
                Ok(lines) => r.done.extend(lines),
                Err(e) => r.errors.push(format!("FL Studio settings: {e}")),
            }
        }
    } else if r.fl_settings.requested {
        let (lines, errors) = fl_settings::restore(reg, &mut r.fl_settings, running, stamp);
        r.done.extend(lines);
        r.errors.extend(errors.into_iter().map(|e| format!("FL Studio settings: {e}")));
    }
    r.fl = fl::detect(probe);
    finish_checks(r);
}

/// --uninstall (main.rs) : avant de retirer le compagnon, remet les reglages de FL qu'il a ecrits → les lignes a dire.
/// FL ouvert : refuse (rien n'est retire), sinon le moyen simple de revenir en arriere partirait avec l'exe.
pub fn undo_fl_settings_for_uninstall() -> Result<Vec<String>, String> {
    // un essai de l'installeur (UWC_INSTALL_ROOT) ne touche jamais au vrai registre
    if std::env::var_os(crate::install::ROOT_ENV).is_some_and(|v| !v.is_empty()) && std::env::var_os(env::ROOT_ENV).is_none_or(|v| v.is_empty()) {
        return Ok(vec![]);
    }
    let probe = Probe::from_env()?;
    undo_fl_settings_with(&probe, &scripts::now_stamp())
}

/// (24 oct.) La meme chose sur une machine donnee (la desinstallation de src/wizard/flow.rs, et ses essais).
pub fn undo_fl_settings_with(probe: &Probe, stamp: &str) -> Result<Vec<String>, String> {
    let o = Opts { apply: true, mode: Mode::Remove, fl_settings: Some(true), daws: vec![], ..Default::default() };
    let mut r = gather(probe, &o, stamp);
    match r.fl_settings.action.as_str() {
        "none" => return Ok(vec![]),
        "blocked" => {
            return Err(format!(
                "{}.\nThe companion puts back the FL Studio settings it changed before it goes: close FL Studio, then uninstall again (nothing was removed).",
                r.fl_settings.reason
            ))
        }
        _ => {}
    }
    apply_fl_settings(&mut r, probe, &|| fl_running_now(probe), stamp);
    if r.fl_settings.action == "blocked" {
        return Err(format!("{}\nNothing was removed.", r.errors.join("\n")));
    }
    let mut lines = vec!["FL Studio's settings, put back as they were before the companion changed them:".to_string()];
    lines.extend(r.done.iter().map(|l| format!("  {l}")));
    for e in &r.errors {
        lines.push(format!("  could not: {e}"));
    }
    lines.extend(r.fl_settings.message.iter().cloned());
    Ok(lines)
}

fn print_text(r: &Report) {
    println!(
        "Uppwind Link setup ({})",
        if r.mode == "apply" { "changes applied" } else { "dry run: nothing is changed, --apply does it" }
    );
    if r.fake_machine {
        println!("(fake machine from {})", env::ROOT_ENV);
    }
    println!();
    println!("Computer: {}{}", r.os.name, if r.os.build > 0 { format!(" (build {})", r.os.build) } else { String::new() });
    let lm = r.midi.loopmidi.as_ref().map(|p| format!("loopMIDI {}{}", p.version, if r.midi.loopmidi_running { ", running" } else { ", not running" })).unwrap_or("loopMIDI not installed".into());
    println!("MIDI: {lm}; inputs: {}", if r.midi.inputs.is_empty() { "none".into() } else { r.midi.inputs.join(", ") });
    println!("      outputs: {}", if r.midi.outputs.is_empty() { "none".into() } else { r.midi.outputs.join(", ") });
    let ep = |e: &Option<Endpoint>| e.as_ref().map(|e| format!("\"{}\" {} Hz", e.name, e.rate)).unwrap_or("missing".into());
    println!(
        "Audio return: {}; VB-CABLE (optional fallback) {}, playback {}, recording {}",
        if r.os.build >= NO_CABLE_BUILD { "by the companion, no cable needed" } else { "this Windows needs VB-CABLE" },
        if r.audio.vbcable.is_some() { "installed" } else { "not installed" },
        ep(&r.audio.cable_input),
        ep(&r.audio.cable_output)
    );
    println!();
    let a = &r.ableton;
    if a.found() {
        let progs: Vec<String> = a.programs.iter().map(|p| p.name.clone()).collect();
        println!("Ableton Live: {}{}", if progs.is_empty() { "found".into() } else { progs.join(", ") }, a.running.as_ref().map(|p| format!(" (running: {p})")).unwrap_or_default());
        for e in &a.exes {
            println!("  program: {}", e.display());
        }
        for p in &a.prefs {
            println!(
                "  preferences {}{}: {} — User Library {} ({})",
                p.version,
                if a.last_used.as_deref() == Some(p.version.as_str()) { " (last used)" } else { "" },
                p.folder.display(),
                p.user_library.display(),
                p.user_library_from
            );
        }
    } else {
        println!("Ableton Live: not found");
    }
    let f = &r.fl;
    if f.found() {
        let progs: Vec<String> = f.programs.iter().map(|p| format!("{} {}", p.name, p.version)).collect();
        println!("FL Studio: {}{}", if progs.is_empty() { "found".into() } else { progs.join(", ") }, f.running.as_ref().map(|p| format!(" (running: {p})")).unwrap_or_default());
        if let Some(e) = &f.exe {
            println!("  program: {}", e.display());
        }
        if let Some(h) = &f.hardware {
            println!("  scripts folder: {}", h.display());
        }
    } else {
        println!("FL Studio: not found");
    }
    println!();
    println!("Scripts:");
    if r.scripts.is_empty() {
        println!("  (no DAW found)");
    }
    for t in &r.scripts {
        println!("  {} → {}", t.daw.label(), t.dir.display());
        let changed: Vec<String> = t.files.iter().filter(|f| f.state != "same").map(|f| format!("{} {}", f.name, f.state)).collect();
        println!(
            "    now: {:?}{}{}",
            t.status,
            t.installed_version.as_ref().map(|v| format!(", installed {v}")).unwrap_or_default(),
            if changed.is_empty() { String::new() } else { format!(" [{}]", changed.join(", ")) }
        );
        let verb = if r.mode == "apply" { "did" } else { "would" };
        println!("    {verb}: {:?} — {}", t.action, t.reason);
        if let Some(b) = &t.backup {
            println!("    backup: {}", b.display());
        }
        if let Some(a) = t.after {
            println!("    after: {a:?}");
        }
    }
    print_fl_settings(r);
    println!();
    println!("Checks:");
    for c in &r.checks {
        let tag = match c.state {
            State::Ok => "ok  ",
            State::Todo => "todo",
            State::Warn => "warn",
            State::Info => "info",
        };
        println!("  [{tag}] {}", c.text);
    }
    if !r.done.is_empty() {
        println!();
        println!("Done:");
        for d in &r.done {
            println!("  {d}");
        }
    }
    if !r.errors.is_empty() {
        println!();
        println!("Errors:");
        for e in &r.errors {
            println!("  {e}");
        }
    }
    if r.fl_settings.done && !r.fl_settings.message.is_empty() {
        println!();
        for m in &r.fl_settings.message {
            println!("{m}");
        }
    }
    let fla = r.fl_settings.action.as_str();
    if r.mode != "apply" && (r.scripts.iter().any(|t| matches!(t.action, Action::Install | Action::Update | Action::Remove)) || fla == "write" || fla == "restore") {
        println!();
        println!("Run again with --apply to do it.");
    }
    if fla == "offer" {
        println!(
            "{}",
            if r.intent == "install" { "Add --fl-settings (FL Studio closed) to write FL's settings too." } else { "Add --fl-settings to put FL's settings back too." }
        );
    }
}

/// La partie « reglages de FL » du rapport texte → true si elle a ete imprimee.
fn print_fl_settings(r: &Report) -> bool {
    let f = &r.fl_settings;
    if !f.available && f.previous.is_empty() {
        if r.fl.found() && r.intent == "install" {
            println!();
            println!("FL Studio settings: {}.", f.reason);
            return true;
        }
        return false;
    }
    let root = f.key.clone().unwrap_or_default();
    let detail = |c: &fl_settings::Change| format!("{} · {}: {} → {}", fl_settings::rel(&c.key, &root), c.name, fl_settings::show(c.before.as_ref()), fl_settings::show(Some(&c.after)));
    println!();
    println!("FL Studio settings ({root}; FL reads them when it starts and saves them when it closes):");
    if r.intent == "install" {
        let applied = r.mode == "apply" && f.done;
        let (base, usb): (Vec<&fl_settings::Change>, Vec<&fl_settings::Change>) = f.changes.iter().partition(|c| c.opt_in == fl_settings::OPT_SETTINGS);
        if base.is_empty() {
            println!("  MIDI and audio: already right (input, output and port {}), nothing to write.", f.port.unwrap_or(fl_settings::DEFAULT_PORT));
        } else {
            let verb = if applied && base[0].chosen {
                "changed"
            } else if base[0].chosen {
                "would change"
            } else {
                "would change, with --fl-settings"
            };
            println!("  {verb}:");
            for c in &base {
                println!("    - {}", c.text);
                println!("        {}", detail(c));
            }
        }
        if !usb.is_empty() {
            println!("  optional, with --fl-free-midi all (or a name): FL holds these USB MIDI devices, so the browser cannot open them:");
            for c in &usb {
                let tag = if applied && c.chosen {
                    " (done)"
                } else if c.chosen {
                    " (chosen)"
                } else {
                    ""
                };
                println!("    - {}{tag}", c.text);
                println!("        {}", detail(c));
            }
        }
        for u in &f.usb_unknown {
            println!("  --fl-free-midi \"{u}\": not a USB MIDI device enabled in FL, left alone.");
        }
        if let (Some(b), Some(m)) = (&f.backup_reg, &f.manifest) {
            if !applied {
                println!("  before writing: a copy of the whole Devices key{} in", if f.changes.iter().any(|c| c.group == "audio") { " and of General\\MIDIForm" } else { "" });
                println!("    {}", b.display());
                println!("    and what changes, to undo it, in {}", m.display());
            }
        }
        for m in &f.manual {
            println!("  stays manual: {m}");
        }
        for p in f.previous.iter().filter(|p| p.restored_at.is_none() && p.state == "written") {
            println!("  written by the companion on {}: {} (uninstalling, or --remove --fl-settings --apply, puts it back)", fl_settings::pretty_stamp(&p.written_at), p.manifest.display());
        }
    } else {
        for l in &f.restore {
            println!("  - {}", l.text);
        }
        for p in &f.previous {
            println!(
                "  {} on {}: {}{}",
                if p.restored_at.is_some() { "put back" } else { "written" },
                fl_settings::pretty_stamp(p.restored_at.as_deref().unwrap_or(&p.written_at)),
                p.manifest.display(),
                if p.state == "rolled-back" { " (rolled back at the time: nothing to undo)" } else { "" }
            );
        }
    }
    let verb = match (r.mode, f.action.as_str()) {
        ("apply", "write") if f.done => "written",
        ("apply", "restore") if f.done => "put back",
        (_, a) => a,
    };
    println!("  status: {verb} — {}", f.reason);
    true
}

/// Une question o/N dans la console.
fn ask(q: &str) -> bool {
    use std::io::Write;
    print!("{q} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    std::io::stdin().read_line(&mut s).is_ok() && matches!(s.trim().to_ascii_lowercase().as_str(), "y" | "yes" | "o" | "oui")
}

fn console() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// --apply sans --fl-settings ni --no-fl-settings, dans une console : montre le plan de FL et demande → true si une
/// reponse change le plan.
fn ask_fl_settings(o: &mut Opts, r: &Report) -> bool {
    let f = &r.fl_settings;
    if o.fl_settings.is_some() || f.blocked_by.is_some() {
        return false;
    }
    let base_offer = f.changes.iter().any(|c| c.opt_in == fl_settings::OPT_SETTINGS && !c.chosen);
    let usb_offer = o.mode == Mode::Install && o.free_midi.is_empty() && !f.usb_inputs.is_empty();
    let restore_offer = o.mode == Mode::Remove && f.action == "offer";
    if !base_offer && !usb_offer && !restore_offer {
        return false;
    }
    print_fl_settings(r);
    println!();
    let mut changed = false;
    if base_offer || restore_offer {
        let q = if o.mode == Mode::Install {
            "Write these FL Studio settings now? A copy is made first, and uninstalling the companion puts them back."
        } else {
            "Put back the FL Studio settings the companion changed?"
        };
        let yes = ask(q);
        o.fl_settings = Some(yes);
        changed |= yes;
    }
    if usb_offer {
        let names: Vec<String> = f.usb_inputs.iter().map(|u| format!("\"{}\"", u.name)).collect();
        if ask(&format!("Also disable {} in FL Studio, so the browser can open {}?", names.join(", "), if names.len() == 1 { "it" } else { "them" })) {
            o.free_midi = vec!["all".into()];
            changed = true;
        }
    }
    println!();
    changed
}

/// `--setup …` → le code de sortie (0, 1 si une ecriture a echoue, 2 si les options sont fausses).
pub fn main(args: &[String]) -> i32 {
    let mut o = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let probe = match Probe::from_env() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let stamp = scripts::now_stamp();
    let mut r = gather(&probe, &o, &stamp);
    if o.apply && !o.json && console() && ask_fl_settings(&mut o, &r) {
        r = gather(&probe, &o, &stamp);
    }
    if o.apply {
        apply(&mut r);
        apply_fl_settings(&mut r, &probe, &|| fl_running_now(&probe), &stamp);
    }
    if o.json {
        println!("{}", serde_json::to_string_pretty(&r).unwrap());
    } else {
        print_text(&r);
    }
    if r.errors.is_empty() {
        0
    } else {
        1
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::env::{FakeRegistry, Paths};
    use super::*;
    use std::path::PathBuf;

    /// Un dossier neuf sous le dossier temporaire : jamais les vrais dossiers de Live ou de FL.
    pub fn temp_root(tag: &str) -> PathBuf {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let d = std::env::temp_dir().join(format!("uwc-setup-test-{tag}-{}-{t}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Une fausse machine typique : Live 12.4.3 et 12.4.6 (la meme User Library, deplacee sur un autre disque), FL
    /// Studio 2026, loopMIDI et ses ports, VB-CABLE a 44,1 kHz.
    fn machine(root: &Path) -> Probe {
        let mut p = Probe::empty(Paths::in_root(root));
        let ul_parent = root.join("D").join("Music").join("Ableton");
        std::fs::create_dir_all(ul_parent.join("User Library")).unwrap();
        for (v, log) in [("12.4.6", "old"), ("12.4.3", "new")] {
            let prefs = p.paths.roaming.join("Ableton").join(format!("Live {v}")).join("Preferences");
            std::fs::create_dir_all(&prefs).unwrap();
            std::fs::write(
                prefs.join("Library.cfg"),
                format!("<Ableton><UserLibrary><LibraryProject Id=\"0\"><ProjectName Value=\"User Library\" /><ProjectPath Value=\"{}\" /></LibraryProject></UserLibrary></Ableton>", ul_parent.to_string_lossy().replace('\\', "/")),
            )
            .unwrap();
            std::fs::write(prefs.join("Log.txt"), format!("t: info: Audio In Out: Sample Rate: 44100\nt: info: {log}\n")).unwrap();
            if v == "12.4.6" {
                // la plus vieille : son Log.txt date
                let f = std::fs::File::options().write(true).open(prefs.join("Log.txt")).unwrap();
                f.set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)).unwrap();
            }
        }
        let prog = p.paths.program_data.join("Ableton").join("Live 12 Suite").join("Program");
        std::fs::create_dir_all(&prog).unwrap();
        std::fs::write(prog.join("Ableton Live 12 Suite.exe"), b"MZ").unwrap();
        let data = root.join("Documents").join("Image-Line");
        std::fs::create_dir_all(&data).unwrap();
        p.reg = Box::new(FakeRegistry::from_json(&serde_json::json!({
            r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion": { "ProductName": "Windows 10 Pro", "DisplayVersion": "22H2", "CurrentBuild": "19045" },
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Ableton Live 12 Suite": { "DisplayName": "Ableton Live 12 Suite", "DisplayVersion": "12" },
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\FL Studio 2026": { "DisplayName": "FL Studio 2026", "DisplayVersion": "26.1.0.1000" },
            r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\{l}": { "DisplayName": "loopMIDI", "DisplayVersion": "1.0.16.0" },
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\VB:VBCABLE": { "DisplayName": "VBCABLE, The Virtual Audio Cable" },
            r"HKCU\Software\Image-Line\Shared\Paths": { "Shared data": data.to_string_lossy() },
            r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI input\Uppwind Link": { "Type": -3, "ScriptFolder": "Uppwind Link", "Port": 77, "Enabled": 1 },
        })));
        p.midi_in = vec!["loopMIDI Port".into(), "Uppwind Link".into(), "Uppwind Link Back".into()];
        p.midi_out = vec!["Microsoft GS Wavetable Synth".into(), "Uppwind Link".into(), "Uppwind Link Back".into()];
        p.render = vec![Endpoint::new("CABLE Input (VB-Audio Virtual Cable)", 44100, "a")];
        p.capture = vec![Endpoint::new("CABLE Output (VB-Audio Virtual Cable)", 48000, "b")];
        p.processes = vec!["loopMIDI.exe".into(), "explorer.exe".into()];
        p
    }

    fn state(r: &Report, id: &str) -> Option<State> {
        r.checks.iter().find(|c| c.id == id).map(|c| c.state)
    }

    #[test]
    fn options() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let o = parse(&s(&["--setup"])).unwrap();
        assert!(!o.apply && o.mode == Mode::Install && o.daws.len() == 2, "a dry run by default");
        let o = parse(&s(&["--setup", "--apply", "--daw", "fl", "--json"])).unwrap();
        assert!(o.apply && o.json && o.daws == vec![Daw::Fl]);
        assert_eq!(parse(&s(&["--remove"])).unwrap().mode, Mode::Remove);
        assert!(parse(&s(&["--daw", "logic"])).is_err());
        assert!(parse(&s(&["--nope"])).is_err());
    }

    #[test]
    fn fl_settings_options() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let o = parse(&s(&["--setup", "--apply", "--fl-settings", "--fl-free-midi", "Keys 49", "--fl-free-midi", "all"])).unwrap();
        assert_eq!(o.fl_settings, Some(true));
        assert_eq!(o.free_midi, vec!["Keys 49".to_string(), "all".to_string()]);
        assert_eq!(parse(&s(&["--setup", "--no-fl-settings"])).unwrap().fl_settings, Some(false));
        assert_eq!(parse(&s(&["--setup"])).unwrap().fl_settings, None, "asked in a console, never assumed");
        assert!(parse(&s(&["--fl-free-midi"])).is_err());
        assert!(parse(&s(&["--fl-free-midi", "--apply"])).is_err());
        let o = parse(&s(&["--setup", "--uninstall", "--fl-settings"])).unwrap();
        assert!(o.mode == Mode::Remove && o.fl_settings == Some(true));
    }

    #[test]
    fn fl_settings_through_the_setup() {
        let root = temp_root("flset");
        let p = machine(&root);
        // la simulation : propose, n'ecrit rien
        let before = count(&root);
        let r = gather(&p, &Opts::default(), "20261024-100000");
        assert_eq!(count(&root), before);
        assert_eq!(r.fl_settings.action, "offer");
        assert_eq!(state(&r, "fl.write"), Some(State::Info));
        assert_eq!(state(&r, "fl.output"), Some(State::Todo));
        let j = serde_json::to_value(&r).unwrap();
        assert_eq!(j["fl_settings"]["action"], "offer");
        assert_eq!(j["fl_settings"]["port"], 77);
        assert_eq!(j["fl_settings"]["changes"][0]["key"], r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI output\Uppwind Link Back");
        assert_eq!(j["fl_settings"]["changes"][0]["after"], "77");
        assert_eq!(j["fl_settings"]["changes"][0]["before"], serde_json::Value::Null);
        assert_eq!(j["fl_settings"]["changes"][0]["chosen"], false);
        // FL ouvert : refuse
        let o = Opts { apply: true, fl_settings: Some(true), daws: vec![Daw::Fl], ..Default::default() };
        let mut pr = machine(&root);
        pr.processes.push("FL64.exe".into());
        let mut r = gather(&pr, &o, "20261024-100100");
        assert_eq!(r.fl_settings.action, "blocked");
        assert_eq!(state(&r, "fl.write"), Some(State::Warn));
        apply_fl_settings(&mut r, &pr, &|| fl::fl_process(&pr.processes), "20261024-100100");
        assert!(r.errors.iter().any(|e| e.contains("close FL Studio first")), "{:?}", r.errors);
        assert!(!p.paths.backups().exists());
        // FL ferme : ecrit, et la verification passe au vert
        let mut r = gather(&p, &o, "20261024-100200");
        assert_eq!(r.fl_settings.action, "write");
        apply_fl_settings(&mut r, &p, &|| None, "20261024-100200");
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert!(r.fl_settings.done && r.done.iter().any(|l| l.contains("backup")));
        assert_eq!(state(&r, "fl.output"), Some(State::Ok));
        assert!(r.fl_settings.message.iter().any(|m| m.contains("Uninstalling the companion")));
        let r2 = gather(&p, &Opts::default(), "20261024-100300");
        assert_eq!(r2.fl_settings.action, "none");
        assert_eq!(state(&r2, "fl.written"), Some(State::Info));
        // --remove --fl-settings : remis, la cle creee retiree
        let rm = Opts { apply: true, mode: Mode::Remove, fl_settings: Some(true), daws: vec![Daw::Fl], ..Default::default() };
        let mut r = gather(&p, &rm, "20261024-100400");
        assert_eq!(r.fl_settings.action, "restore");
        apply_fl_settings(&mut r, &p, &|| None, "20261024-100400");
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert!(!p.reg.key_exists(r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI output"));
        assert_eq!(state(&r, "fl.output"), Some(State::Todo), "as before");
        assert_eq!(gather(&p, &rm, "20261024-100500").fl_settings.action, "none");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn port_names_on_both_systems() {
        assert!(is_link_port("Uppwind Link") && !is_link_port("Uppwind Link Back"));
        assert!(is_link_port("IAC Driver (Uppwind Link)") && is_link_port("IAC Driver Uppwind Link"));
        assert!(is_back_port("IAC Driver (Uppwind Link Back)") && !is_back_port("Uppwind Link"));
        assert!(same_path(Path::new("D:/music/Ableton/User Library"), Path::new(r"d:\Music\Ableton\User Library\")) == cfg!(windows));
    }

    #[test]
    fn a_dry_run_looks_and_writes_nothing() {
        let root = temp_root("dry");
        let p = machine(&root);
        let before = count(&root);
        let r = gather(&p, &Opts::default(), "20261024-093012");
        assert_eq!(count(&root), before, "nothing written");
        assert_eq!(r.os.name, "Windows 10 Pro 22H2");
        assert_eq!(r.ableton.prefs.len(), 2);
        assert_eq!(r.ableton.last_used.as_deref(), Some("12.4.3"));
        assert_eq!(r.ableton.user_libraries.len(), 1, "both versions share one User Library: {:?}", r.ableton.user_libraries);
        assert_eq!(r.ableton.exes.len(), 1);
        assert_eq!(r.scripts.len(), 2);
        assert!(r.scripts.iter().all(|t| t.status == Status::Missing && t.action == Action::Install), "{:#?}", r.scripts);
        assert!(r.scripts[0].dir.ends_with(Path::new("User Library").join("Remote Scripts").join("UppwindLink")));
        assert!(r.scripts[1].dir.ends_with(Path::new("Image-Line").join("FL Studio").join("Settings").join("Hardware").join("Uppwind Link")));
        assert_eq!(state(&r, "midi.ports"), Some(State::Ok));
        // (24 oct.) le son revient par le compagnon : VB-CABLE, installe ici, n'est qu'un secours
        assert_eq!(state(&r, "audio.return"), Some(State::Ok));
        assert_eq!(state(&r, "audio.cable"), Some(State::Info));
        assert_eq!(state(&r, "audio.rate"), Some(State::Info), "CABLE Input at 44.1 kHz only matters with the cable");
        assert_eq!(state(&r, "ableton.script"), Some(State::Todo));
        assert_eq!(state(&r, "ableton.rate"), Some(State::Info), "44.1 kHz is fine without the cable");
        assert!(r.checks.iter().all(|c| !(c.state == State::Todo && c.text.contains("VB-CABLE"))), "VB-CABLE is never a to-do here: {:#?}", r.checks);
        let j = serde_json::to_value(&r).unwrap();
        assert_eq!(j["mode"], "dry-run");
        assert_eq!(j["scripts"][0]["status"], "missing");
        assert_eq!(j["scripts"][0]["action"], "install");
        assert_eq!(j["checks"].as_array().unwrap().iter().find(|c| c["id"] == "midi.ports").unwrap()["state"], "ok");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Un Windows trop ancien pour la capture d'un processus : la, et la seulement, VB-CABLE est propose.
    #[cfg(windows)]
    #[test]
    fn vb_cable_is_offered_only_when_windows_cannot_capture_one_app() {
        let root = temp_root("oldwin");
        let mut p = machine(&root);
        p.reg = Box::new(FakeRegistry::from_json(&serde_json::json!({
            r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion": { "ProductName": "Windows 10 Pro", "DisplayVersion": "1909", "CurrentBuild": "18363" },
        })));
        p.render.clear();
        p.capture.clear();
        let r = gather(&p, &Opts::default(), "20261024-093013");
        let c = r.checks.iter().find(|c| c.id == "audio.return").unwrap();
        assert_eq!(c.state, State::Todo);
        assert!(c.text.contains("VB-CABLE") && c.text.contains("build 18363"), "{}", c.text);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn count(dir: &Path) -> usize {
        std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| if e.path().is_dir() { 1 + count(&e.path()) } else { 1 }).sum()).unwrap_or(0)
    }

    #[test]
    fn apply_installs_then_updates_with_a_backup_then_removes() {
        let root = temp_root("apply");
        let p = machine(&root);
        let o = Opts { apply: true, ..Default::default() };
        // 1. installer
        let mut r = gather(&p, &o, "20261024-090000");
        apply(&mut r);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert!(r.scripts.iter().all(|t| t.after == Some(Status::UpToDate)), "{:#?}", r.scripts);
        let ab = r.scripts[0].dir.clone();
        let flp = r.scripts[1].dir.clone();
        assert!(ab.join("UppwindLink.py").is_file() && ab.join(scripts::MARKER).is_file());
        assert!(flp.join("device_UppwindLink.py").is_file());
        assert_eq!(state(&r, "ableton.script"), Some(State::Ok));
        // 2. rien a refaire
        let r2 = gather(&p, &o, "20261024-090100");
        assert!(r2.scripts.iter().all(|t| t.status == Status::UpToDate && t.action == Action::None && t.backup.is_none()));
        // 3. une version plus ancienne, modifiee a la main → mise a jour, l'ancienne gardee
        let old = std::fs::read_to_string(ab.join("UppwindLink.py")).unwrap();
        let v = scripts::bundle(Daw::Ableton).version();
        std::fs::write(ab.join("UppwindLink.py"), old.replace(&format!("SCRIPT_VERSION = \"{v}\""), "SCRIPT_VERSION = \"0.9.0\"")).unwrap();
        std::fs::write(ab.join("mine.py"), "# a file of the user's").unwrap();
        std::fs::create_dir_all(ab.join("__pycache__")).unwrap();
        std::fs::write(ab.join("__pycache__").join("x.pyc"), "pyc").unwrap();
        let mut r3 = gather(&p, &o, "20261024-090200");
        assert_eq!(r3.scripts[0].status, Status::Outdated);
        assert_eq!(r3.scripts[0].installed_version.as_deref(), Some("0.9.0"));
        assert!(r3.scripts[0].files.iter().any(|f| f.name == "mine.py" && f.state == "extra"));
        let bk = r3.scripts[0].backup.clone().unwrap();
        assert!(bk.starts_with(p.paths.backups()), "backups live outside the DAW folders: {}", bk.display());
        apply(&mut r3);
        assert!(r3.errors.is_empty(), "{:?}", r3.errors);
        assert_eq!(r3.scripts[0].after, Some(Status::UpToDate));
        assert!(std::fs::read_to_string(bk.join("UppwindLink.py")).unwrap().contains("0.9.0"), "the old version is in the backup");
        assert!(bk.join("mine.py").is_file() && bk.join("__pycache__").join("x.pyc").is_file());
        assert!(!ab.join("mine.py").exists(), "the folder is the bundle again");
        // 4. une version plus recente : laissee, sauf --force
        let cur = std::fs::read_to_string(ab.join("UppwindLink.py")).unwrap();
        std::fs::write(ab.join("UppwindLink.py"), cur.replace(&format!("SCRIPT_VERSION = \"{v}\""), "SCRIPT_VERSION = \"99.0.0\"")).unwrap();
        let mut r4 = gather(&p, &o, "20261024-090300");
        assert_eq!((r4.scripts[0].status, r4.scripts[0].action), (Status::Newer, Action::Skip));
        apply(&mut r4);
        assert!(std::fs::read_to_string(ab.join("UppwindLink.py")).unwrap().contains("99.0.0"), "not replaced");
        let r5 = gather(&p, &Opts { apply: true, force: true, ..Default::default() }, "20261024-090400");
        assert_eq!(r5.scripts[0].action, Action::Update);
        // 5. retirer : les deux dossiers partent, une copie de chacun reste
        let mut r6 = gather(&p, &Opts { apply: true, mode: Mode::Remove, ..Default::default() }, "20261024-090500");
        assert!(r6.scripts.iter().all(|t| t.action == Action::Remove));
        let bks: Vec<PathBuf> = r6.scripts.iter().map(|t| t.backup.clone().unwrap()).collect();
        apply(&mut r6);
        assert!(r6.errors.is_empty(), "{:?}", r6.errors);
        assert!(!ab.exists() && !flp.exists());
        assert!(bks.iter().all(|b| b.is_dir()), "{bks:?}");
        assert!(r6.scripts.iter().all(|t| t.after == Some(Status::Missing)));
        assert_eq!(state(&r6, "ableton.script"), Some(State::Ok), "not installed is what was asked");
        let r7 = gather(&p, &Opts { mode: Mode::Remove, ..Default::default() }, "20261024-090600");
        assert!(r7.scripts.iter().all(|t| t.action == Action::None));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_user_library_is_never_created() {
        let root = temp_root("nolib");
        let p = machine(&root);
        std::fs::remove_dir_all(root.join("D")).unwrap();
        let mut r = gather(&p, &Opts { apply: true, daws: vec![Daw::Ableton], ..Default::default() }, "20261024-090000");
        assert_eq!(r.scripts.len(), 1);
        assert_eq!((r.scripts[0].status, r.scripts[0].action), (Status::NoLibrary, Action::Skip));
        apply(&mut r);
        assert!(!root.join("D").exists(), "no User Library made up on a missing drive");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failed_write_puts_the_old_folder_back() {
        let root = temp_root("rollback");
        let p = machine(&root);
        let o = Opts { apply: true, daws: vec![Daw::Fl], ..Default::default() };
        let dir = gather(&p, &o, "20261024-090000").scripts[0].dir.clone();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("device_UppwindLink.py"), "SCRIPT_VERSION = \"0.1.0\"\n").unwrap();
        std::fs::write(dir.join("mine.txt"), "keep me").unwrap();
        let r = gather(&p, &o, "20261024-090100");
        assert_eq!(r.scripts[0].action, Action::Update);
        // un script dont un nom de fichier est interdit par Windows : l'ecriture echoue apres la sauvegarde
        static BAD: &[(&str, &[u8])] = &[("device_UppwindLink.py", b"SCRIPT_VERSION = \"2.0.0\"\n"), ("bad<name>.py", b"x")];
        let b = scripts::Bundle { daw: Daw::Fl, folder: "Uppwind Link", files: BAD, version_file: "device_UppwindLink.py" };
        let e = scripts::apply_with(&r.scripts[0], &b).unwrap_err();
        assert!(e.contains("put back"), "{e}");
        assert_eq!(std::fs::read_to_string(dir.join("mine.txt")).unwrap(), "keep me", "the old folder is back");
        assert!(std::fs::read_to_string(dir.join("device_UppwindLink.py")).unwrap().contains("0.1.0"));
        assert!(!dir.join(scripts::MARKER).exists());
        assert!(r.scripts[0].backup.as_ref().unwrap().join("mine.txt").is_file(), "and the copy stays");
        let _ = std::fs::remove_dir_all(&root);
    }
}
