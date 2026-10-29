// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Ce que fait le bouton « Install » (et la desinstallation) : install.rs et setup/ dans l'ordre — rien n'est reecrit
//! ici —, plus l'entree « Applications installees » (arp.rs), le compagnon installe lance dans la zone de notification,
//! et, a la desinstallation, le retrait des scripts que le compagnon avait mis, avec le dossier d'avant remis s'il y en
//! avait un (suivi des copies de sauvegarde, de marqueur en marqueur).
//!
//! Tout passe par `Env` (les dossiers, la machine, le registre de l'entree) et `Hooks` (les effets sur le systeme :
//! fermer le compagnon qui tourne, lancer le nouveau, nettoyer apres coup) : les essais donnent un bac a sable et une
//! fausse machine, jamais les vrais dossiers de Live ou de FL, ni la vraie cle « Uninstall ».

use super::arp;
use super::steps::{target_id, Choices, Outcome, Todo};
use crate::install::{self, shortcut, Dirs, Shortcut};
use crate::plugin_install as plugins;
use crate::setup::env::{Probe, Registry};
use crate::setup::scripts::{self, Action, Daw, Status, Target, MARKER};
use crate::setup::{self, Mode, Opts, Report};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Ou et sur quoi l'installation agit.
pub struct Env<'a> {
    pub dirs: Dirs,
    /// la machine (la vraie, une fausse dans les essais, la vraie en lecture seule dans le bac a sable)
    pub probe: &'a Probe,
    /// le bac a sable sans fausse machine : les changements dans Live et FL sont seulement dits
    pub simulate_daws: bool,
    pub arp_reg: &'a dyn Registry,
    pub arp_key: String,
    /// le fichier d'etat du compagnon (le port, le jeton, le pid)
    pub state_file: PathBuf,
    /// des options en plus pour les raccourcis et les lancements (le bac a sable : --sandbox "<dossier>")
    pub extra_args: Vec<String>,
    /// l'horodatage des copies de sauvegarde
    pub stamp: String,
    /// (29 oct.) ou vont les plugins Uppwind Link (le dossier VST3 du systeme ; un dossier d'essai dans les tests ;
    /// `simulate` dans le bac a sable), et lesquels (ceux de l'exe ; un faux bundle dans les tests)
    pub plugins: plugins::Target,
    pub plugin_bundle: plugins::Bundle,
}

/// Les effets sur le systeme, a part (la fenetre les fait ; les essais n'en font aucun).
pub trait Hooks {
    fn progress(&self, _line: &str) {}
    /// un Uppwind Link qui tourne (l'icone) : on lui demande de s'arreter → ce qu'on en dit
    fn stop_running(&self) -> Option<String> {
        None
    }
    /// (29 oct.) macOS : le gardien des ports MIDI (midi_ports.rs), qui survit au compagnon, arrete → ce qu'on en dit ;
    /// Ok(None) : il n'y en avait pas
    fn stop_midi_ports(&self) -> Result<Option<String>, String> {
        Ok(None)
    }
    /// lance le compagnon installe dans la zone de notification → son port
    fn start(&self, _exe: &Path, _args: &[String]) -> Result<Option<u16>, String> {
        Ok(None)
    }
    /// apres la desinstallation : ce qui ne pouvait pas partir tant que le programme tournait (des fichiers, des
    /// dossiers s'ils sont vides, des arbres entiers)
    fn cleanup_later(&self, _files: &[PathBuf], _dirs: &[PathBuf], _trees: &[PathBuf]) {}
}

/// Efface un arbre, en reessayant 3 s (les processus de WebView2 lachent leurs fichiers une seconde apres la fermeture
/// de la fenetre) → true s'il est parti.
fn remove_tree(dir: &Path) -> bool {
    for _ in 0..30 {
        if !dir.exists() || std::fs::remove_dir_all(dir).is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

#[cfg(test)]
pub struct NoHooks;
#[cfg(test)]
impl Hooks for NoHooks {}

/// Une ligne de commande pour un raccourci : les options, puis celles du bac a sable (entre guillemets).
pub fn args_line(first: &[&str], extra: &[String]) -> String {
    let mut v: Vec<String> = first.iter().map(|s| s.to_string()).collect();
    for e in extra {
        v.push(if e.contains(' ') || e.is_empty() { format!("\"{e}\"") } else { e.clone() });
    }
    v.join(" ")
}

/// Le dossier des donnees de la fenetre d'etat (WebView2), dans le dossier de l'application.
pub fn webview_dir(dirs: &Dirs) -> PathBuf {
    dirs.app_dir().join("webview")
}

/// « Start with Windows » (install.rs : le raccourci du dossier Demarrage, sans fenetre : l'icone seulement) ; un essai
/// y ajoute ses options.
pub fn set_autostart(dirs: &Dirs, on: bool, extra_args: &[String]) -> Result<Vec<String>, String> {
    let l = install::autostart(dirs, on, true)?;
    if on && !extra_args.is_empty() {
        let su = Shortcut { target: dirs.exe(), args: args_line(&["--background"], extra_args), minimized: true };
        shortcut::write(&dirs.startup_link(), &su)?;
    }
    Ok(l)
}

/// Ce que la fenetre de desinstallation annonce (rien n'est change).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UninstallPlan {
    pub removes: Vec<String>,
    pub puts_back: Vec<String>,
    pub keeps: Vec<String>,
    /// FL tourne et ses reglages sont a remettre : il faut le fermer d'abord
    pub fl_running: Option<String>,
    pub simulated: bool,
    pub sandbox: Option<String>,
}

pub fn plan_uninstall(env: &Env) -> UninstallPlan {
    let mut p = UninstallPlan { simulated: env.simulate_daws, ..Default::default() };
    p.removes.push(format!("Uppwind Link, its Start menu shortcut and its start with Windows ({}).", env.dirs.app_dir().display()));
    let opts = Opts { apply: false, mode: Mode::Remove, daws: vec![Daw::Ableton, Daw::Fl], fl_settings: Some(true), ..Default::default() };
    let r = setup::gather(env.probe, &opts, &env.stamp);
    for t in r.scripts.iter().filter(|t| t.by_companion && !matches!(t.status, Status::Missing | Status::NoLibrary)) {
        p.removes.push(format!("The {} script it installed ({}).", t.daw.label(), t.dir.display()));
        if original_of(&t.dir, &env.probe.paths.backups()).is_some() {
            p.puts_back.push(format!("The {} script folder that was there before Uppwind Link.", t.daw.label()));
        }
    }
    // (29 oct.) les plugins Uppwind Link qu'il a mis
    let mut pr = plugins::inspect(&env.plugin_bundle, &env.plugins);
    plugins::plan(&mut pr, true, false);
    if pr.action == plugins::Action::Remove {
        p.removes.push(format!(
            "The Uppwind Link plugins ({}){}.",
            crate::shown_path(&pr.folder),
            if pr.admin && !env.plugins.simulate { ": Windows asks for administrator rights" } else { "" }
        ));
    }
    if r.fl_settings.available {
        p.puts_back.push("FL Studio’s MIDI settings, as they were before Uppwind Link changed them.".into());
        p.fl_running = r.fl.running.clone();
    }
    p.removes.push("Its log and its window data.".into());
    p.removes.push("Its entry in Windows Settings › Apps.".into());
    let backups = env.probe.paths.backups();
    if backups.is_dir() || r.scripts.iter().any(|t| t.by_companion) {
        p.keeps.push(format!("Copies of the files it replaced ({}).", backups.display()));
    }
    if env.state_file.exists() {
        p.keeps.push(format!("Your pairing code ({}): the studio stays paired if you install Uppwind Link again.", env.state_file.display()));
    }
    p
}

/// L'installation : le programme, les raccourcis, les scripts des DAW coches, les reglages de FL, le demarrage avec
/// Windows, l'entree « Applications installees », puis le compagnon lance. Chaque etape dit ce qu'elle a fait ; une
/// erreur n'arrete que ce qui en depend.
pub fn install(env: &Env, src: &Path, c: &Choices, h: &dyn Hooks) -> Outcome {
    let mut o = Outcome { simulated: env.simulate_daws, fl: "skipped".into(), ..Default::default() };
    if let Some(l) = h.stop_running() {
        o.done.push(l);
    }
    h.progress("Copying Uppwind Link to your user folder");
    match install::install(&env.dirs, src) {
        Ok(l) => o.done.extend(l),
        Err(e) => {
            o.errors.push(e);
            return o;
        }
    }
    let exe = env.dirs.exe();
    // le raccourci du menu Demarrer ouvre la fenetre d'etat (la console du compagnon reste celle d'un terminal)
    let sm = Shortcut { target: exe.clone(), args: args_line(&["--app"], &env.extra_args), minimized: true };
    if let Err(e) = shortcut::write(&env.dirs.start_menu_link(), &sm) {
        o.errors.push(e);
    }

    h.progress("Setting up your DAWs");
    let report = daws(env, c, &mut o);

    // (29 oct.) les plugins Uppwind Link (VST3) : sous Windows, l'invite d'administrateur arrive ici
    if c.wants_plugins() {
        h.progress("Installing the Uppwind Link plugins");
        install_plugins(env, &mut o, &|l| h.progress(l));
    }

    h.progress("Start with Windows");
    match set_autostart(&env.dirs, c.autostart, &env.extra_args) {
        Ok(l) => {
            o.autostart = c.autostart;
            if c.autostart || !l.iter().any(|x| x.contains("already off")) {
                o.done.extend(l);
            }
        }
        Err(e) => o.errors.push(e),
    }

    h.progress("Adding Uppwind Link to Installed apps");
    let entry = arp::Entry {
        version: env!("CARGO_PKG_VERSION").into(),
        location: env.dirs.app_dir().display().to_string(),
        exe: exe.display().to_string(),
        extra_args: args_line(&[], &env.extra_args),
        size_kb: std::fs::metadata(&exe).map(|m| m.len().div_ceil(1024)).unwrap_or(0),
        date: env.stamp.chars().take(8).collect(),
    };
    match arp::write(env.arp_reg, &env.arp_key, &entry) {
        Ok(l) => {
            o.listed = true;
            o.done.extend(l);
        }
        Err(e) => o.errors.push(e),
    }

    h.progress("Starting Uppwind Link");
    let mut args: Vec<String> = vec!["--background".into(), "--replace".into()];
    args.extend(env.extra_args.iter().cloned());
    match h.start(&exe, &args) {
        Ok(p) => o.port = p,
        Err(e) => o.errors.push(e),
    }
    o.todo = todo(env, c, report.as_ref(), &o);
    o.ok = o.errors.is_empty();
    o
}

fn script_line(t: &Target) -> String {
    let label = t.daw.label();
    match (t.action, t.after) {
        (Action::Install, Some(Status::UpToDate)) => format!("{label}: script {} installed in {}.", t.bundled_version, t.dir.display()),
        (Action::Update, Some(Status::UpToDate)) => format!(
            "{label}: script {} installed in {} (it replaced {}; the old folder is kept in {}).",
            t.bundled_version,
            t.dir.display(),
            t.installed_version.as_deref().map(|v| format!("version {v}")).unwrap_or_else(|| "a different copy".into()),
            t.backup.as_ref().map(|b| b.display().to_string()).unwrap_or_default()
        ),
        (Action::None, _) => format!("{label}: script {} already up to date in {}.", t.bundled_version, t.dir.display()),
        (Action::Skip, _) => format!("{label}: script left as it is: {}.", t.reason),
        _ => format!("{label}: script not installed ({}).", t.reason),
    }
}

/// Les scripts des DAW coches et les reglages de FL (setup::gather, apply, apply_fl_settings) → le rapport, pour la
/// liste de ce qui reste a faire.
fn daws(env: &Env, c: &Choices, o: &mut Outcome) -> Option<Report> {
    let want_ab = c.daws.iter().any(|d| d.starts_with("ableton:"));
    let want_fl = c.wants("fl");
    if !want_ab && !want_fl {
        return None;
    }
    let mut daws = Vec::new();
    if want_ab {
        daws.push(Daw::Ableton);
    }
    if want_fl {
        daws.push(Daw::Fl);
    }
    let fl_settings = want_fl && c.fl_settings;
    // (les plugins : a part, install_plugins, avec leur propre ligne de progression)
    let opts = Opts { apply: !env.simulate_daws, mode: Mode::Install, json: true, force: false, daws, fl_settings: Some(fl_settings), free_midi: vec![], plugins: false };
    let mut r = setup::gather(env.probe, &opts, &env.stamp);
    r.scripts.retain(|t| c.wants(&target_id(t)));
    if env.simulate_daws {
        for t in &r.scripts {
            o.scripts.push(format!("{}: {} in {} (simulated: nothing written).", t.daw.label(), t.reason, t.dir.display()));
        }
        if fl_settings {
            o.fl = "simulated".into();
            o.fl_lines.push(format!("FL Studio's MIDI settings: {} (simulated: nothing written).", r.fl_settings.reason));
        }
        return Some(r);
    }
    setup::apply(&mut r);
    if fl_settings {
        setup::apply_fl_settings(&mut r, env.probe, &|| setup::fl_running_now(env.probe), &env.stamp);
    }
    // (les lignes de setup/ disent la meme chose en detail : les scripts et les reglages de FL les resument)
    o.scripts.extend(r.scripts.iter().map(script_line));
    let blocked = r.fl_settings.action == "blocked";
    for e in &r.errors {
        // FL ouvert au moment d'ecrire : pas une erreur, une chose a faire (todo « fl »)
        if blocked && e.starts_with("FL Studio settings:") {
            continue;
        }
        o.errors.push(e.clone());
    }
    if fl_settings {
        o.fl = if r.fl_settings.done {
            "written"
        } else if blocked {
            "blocked"
        } else if r.fl_settings.action == "none" {
            "unchanged"
        } else {
            "failed"
        }
        .into();
        o.fl_lines = if r.fl_settings.done { r.fl_settings.message.clone() } else { vec![format!("{}.", r.fl_settings.reason)] };
    }
    let copied = r.scripts.iter().any(|t| t.backup.is_some() && t.after == Some(Status::UpToDate)) || r.fl_settings.done;
    if copied {
        o.backups = Some(env.probe.paths.backups().display().to_string());
    }
    Some(r)
}

/// (29 oct.) Les plugins Uppwind Link, embarques dans l'exe, dans le dossier VST3 (plugin_install/) : ce qui a ete
/// fait dans `o` (plugins, plugin_lines ; une vraie panne dans errors, l'invite refusee non).
fn install_plugins(env: &Env, o: &mut Outcome, progress: &dyn Fn(&str)) {
    let b = env.plugin_bundle;
    let mut r = plugins::inspect(&b, &env.plugins);
    plugins::plan(&mut r, false, false);
    let a = plugins::apply(&mut r, &b, &env.plugins, progress);
    o.plugins = match r.action {
        plugins::Action::None => "unchanged",
        plugins::Action::Skip | plugins::Action::Remove => "skipped",
        _ if env.plugins.simulate => "simulated",
        _ if a.refused => "refused",
        _ if !a.errors.is_empty() => "failed",
        plugins::Action::Install => "installed",
        plugins::Action::Update => "updated",
    }
    .into();
    match r.action {
        plugins::Action::None => o.plugin_lines.push(format!("Uppwind Link plugins: already up to date in {}.", crate::shown_path(&r.folder))),
        plugins::Action::Skip => o.plugin_lines.push(format!("Uppwind Link plugins: {}.", r.reason)),
        _ => {}
    }
    o.plugin_lines.extend(a.done);
    o.errors.extend(a.errors);
    o.plugins_restart = a.restart;
}

fn todo_of(id: &str, title: &str, lines: &[&str]) -> Todo {
    Todo { id: id.into(), title: title.into(), lines: lines.iter().map(|l| l.to_string()).collect() }
}

/// Ce qui reste a faire, dans l'ordre ou on le fait (les mots du guide du studio, link-setup.js).
fn todo(env: &Env, c: &Choices, r: Option<&Report>, o: &Outcome) -> Vec<Todo> {
    let mut v = Vec::new();
    if let Some(r) = r {
        let ab: Vec<&Target> = r.scripts.iter().filter(|t| t.daw == Daw::Ableton).collect();
        if !ab.is_empty() {
            let changed = ab.iter().any(|t| matches!(t.action, Action::Install | Action::Update));
            if changed && r.ableton.running.is_some() {
                v.push(todo_of("restart-live", "Restart Ableton Live", &["Live reads its scripts only when it starts."]));
            }
            let ok = |id: &str| r.checks.iter().any(|x| x.id == id && x.state == setup::State::Ok);
            if !(ok("ableton.surface") && ok("ableton.input") && ok("ableton.output")) {
                v.push(todo_of(
                    "live",
                    "In Ableton Live",
                    &[
                        "Settings › Tempo & MIDI (Live 11: Preferences › Link, Tempo & MIDI):",
                        "Control Surface: UppwindLink — Input “Uppwind Link”, Output “Uppwind Link Back”.",
                        "MIDI Ports: In “Uppwind Link”: Track and Remote on. Out “Uppwind Link Back”: Remote on.",
                    ],
                ));
            }
            let dev = r.ableton.settings.as_ref().is_some_and(|s| s.surfaces.iter().any(|x| x.name == "UppwindLinkDev")) || ab.iter().any(|t| t.neighbours.iter().any(|n| n == "UppwindLinkDev"));
            if dev {
                v.push(todo_of(
                    "live-dev",
                    "One Control Surface is enough",
                    &["UppwindLink also opens your plugins’ windows for Uppwind Link. Had UppwindLinkDev in a second slot? Set that slot to None."],
                ));
            }
            let mme = r.ableton.settings.as_ref().and_then(|s| s.driver.as_deref()).is_some_and(|d| d.to_ascii_lowercase().contains("mme"));
            if !mme {
                v.push(todo_of(
                    "live-audio",
                    "For your sound to come back to the studio",
                    &["In Live: Settings › Audio — Driver Type: MME/DirectX (not ASIO), Audio Output Device: an output you do not listen to (a TV’s HDMI output, an unused interface output), 48000 Hz."],
                ));
            }
        }
        if let Some(t) = r.scripts.iter().find(|t| t.daw == Daw::Fl) {
            let changed = matches!(t.action, Action::Install | Action::Update);
            let port = r.fl_settings.port.unwrap_or(setup::fl_settings::DEFAULT_PORT);
            match o.fl.as_str() {
                "written" => v.push(Todo {
                    id: "fl".into(),
                    title: if r.fl.running.is_some() { "Restart FL Studio".into() } else { "Start FL Studio".into() },
                    lines: vec![format!("Its MIDI settings are ready: Input “Uppwind Link”, Controller type “Uppwind Link (user)”, port {port}.")],
                }),
                "blocked" => v.push(todo_of("fl", "FL Studio was open", &["Its MIDI settings were not changed. Close FL Studio, then open Uppwind Link › Set up again."])),
                "unchanged" => {
                    if changed && r.fl.running.is_some() {
                        v.push(todo_of("restart-fl", "Restart FL Studio", &["FL reads its scripts when it starts."]));
                    }
                }
                _ => {
                    if !c.fl_settings {
                        v.push(Todo {
                            id: "fl".into(),
                            title: "In FL Studio".into(),
                            lines: vec![
                                "Options › MIDI settings:".into(),
                                format!("Input “Uppwind Link”: Enable, Controller type “Uppwind Link (user)”, Port {port}."),
                                format!("Output “Uppwind Link Back”: Port {port} — the same number."),
                            ],
                        });
                    }
                }
            }
            v.push(todo_of(
                "fl-audio",
                "For your sound to come back to the studio",
                &["In FL Studio: Options › Audio settings — Device: Primary Sound Driver (or a Windows output, not a hardware ASIO driver), playing into an output you do not listen to."],
            ));
        }
    }
    // (29 oct.) les plugins Uppwind Link : les retrouver dans le DAW, l'invite refusee, un DAW a relancer
    match o.plugins.as_str() {
        "installed" | "updated" => {
            let (live, fl) = r.map(|r| (r.ableton.found(), r.fl.found())).filter(|(l, f)| *l || *f).unwrap_or((true, true));
            let mut lines = Vec::new();
            if fl {
                lines.push("FL Studio: Options › Manage plugins › Find installed plugins (or restart FL Studio).".to_string());
            }
            if live {
                lines.push("Ableton Live: Settings › Plug-Ins: Use VST3 Plug-In System Folders on, then Rescan.".to_string());
            }
            lines.push("Then put Uppwind Link Return on the master: your DAW’s sound comes back with any audio driver, ASIO included.".to_string());
            v.push(Todo { id: "plugins-scan".into(), title: "Find the Uppwind Link plugins in your DAW".into(), lines });
        }
        "refused" => v.push(Todo {
            id: "plugins".into(),
            title: "Install the Uppwind Link plugins".into(),
            lines: vec![
                format!("The administrator prompt was refused, so they are not installed. FL Studio finds VST3 plugins only in {}.", crate::shown_path(&env.plugins.vst3)),
                "Open Uppwind Link › Set up again to install them (one prompt).".into(),
            ],
        }),
        _ => {}
    }
    if o.plugins_restart {
        v.push(todo_of("plugins-restart", "Restart your DAW", &["It still has the previous Uppwind Link plugins loaded: the new ones load when it starts again."]));
    }
    let (link, back, _) = super::steps::ports_of(&env.probe.midi_in, &env.probe.midi_out);
    if link.is_none() || back.is_none() {
        v.push(todo_of(
            "ports",
            "Create the two MIDI ports",
            &["In loopMIDI: “Uppwind Link” and “Uppwind Link Back” (type the name, click +). Uppwind Link reminds you until they are there."],
        ));
    }
    if o.port.is_some() || env.simulate_daws {
        v.push(todo_of("pair", "Pair the studio", &["Open the studio › Settings › Uppwind Link › Plugin windows: paste the code, then Pair."]));
    }
    v
}

/// Ce que la desinstallation a fait.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Removal {
    pub ok: bool,
    pub simulated: bool,
    pub done: Vec<String>,
    pub errors: Vec<String>,
    /// laisse en place, et ou
    pub kept: Vec<String>,
    /// a faire a la main
    pub todo: Vec<String>,
    /// rien n'a ete retire : pourquoi (FL ouvert)
    pub blocked: Option<String>,
}

/// Le marqueur d'un dossier de script installe par le compagnon : Some(la copie faite avant lui, s'il y en avait une) ;
/// None : pas de marqueur (un dossier de l'utilisateur).
fn marker_backup(dir: &Path) -> Option<Option<PathBuf>> {
    let raw = std::fs::read_to_string(dir.join(MARKER)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    Some(v["backup"].as_str().filter(|s| !s.is_empty()).map(PathBuf::from))
}

/// (26 oct.) Une copie de sauvegarde qu'on accepte de remettre : un vrai dossier (ni lien ni jonction) sous `backups`.
/// Le marqueur est un fichier que l'on peut modifier (ou qui vient d'un autre PC, une User Library partagee) : sans
/// ce controle, la desinstallation copierait n'importe quel dossier dans les Remote Scripts.
fn backup_ok(p: &Path, backups: &Path) -> bool {
    let Ok(m) = std::fs::symlink_metadata(p) else { return false };
    if !m.is_dir() || m.file_type().is_symlink() {
        return false;
    }
    match (std::fs::canonicalize(p), std::fs::canonicalize(backups)) {
        (Ok(a), Ok(b)) => a != b && a.starts_with(&b),
        _ => false,
    }
}

/// Le dossier qui etait la avant que le compagnon n'y ecrive la premiere fois : on remonte les copies de sauvegarde
/// (sous `backups` seulement) tant qu'elles portent le marqueur du compagnon ; None : il n'y avait rien (ou la copie
/// n'est plus la, ou le marqueur designe autre chose qu'une copie de sauvegarde).
pub fn original_of(dir: &Path, backups: &Path) -> Option<PathBuf> {
    let mut next = marker_backup(dir)??;
    for _ in 0..32 {
        if !backup_ok(&next, backups) {
            return None;
        }
        match marker_backup(&next) {
            Some(prev) => next = prev?,
            None => return Some(next),
        }
    }
    None
}

/// Les scripts que le compagnon avait installes : retires (une copie gardee), et le dossier d'avant remis.
fn scripts_out(env: &Env, o: &mut Removal) -> bool {
    let opts = Opts { apply: true, mode: Mode::Remove, daws: vec![Daw::Ableton, Daw::Fl], fl_settings: Some(false), ..Default::default() };
    let r = setup::gather(env.probe, &opts, &env.stamp);
    let mut removed_live = false;
    for t in &r.scripts {
        if matches!(t.status, Status::Missing | Status::NoLibrary) {
            continue;
        }
        let label = t.daw.label();
        if !t.by_companion {
            o.done.push(format!("{label}: the script in {} was not installed by Uppwind Link: left as it is.", t.dir.display()));
            continue;
        }
        if env.simulate_daws {
            o.done.push(format!("{label}: the script in {} would be removed (simulated: nothing changed).", t.dir.display()));
            continue;
        }
        let original = original_of(&t.dir, &env.probe.paths.backups());
        if let Err(e) = scripts::apply(t) {
            o.errors.push(format!("{label}: {e}"));
            continue;
        }
        removed_live |= t.daw == Daw::Ableton;
        o.done.push(format!("{label}: removed the script from {} (a copy is kept in {}).", t.dir.display(), t.backup.as_ref().map(|b| b.display().to_string()).unwrap_or_default()));
        if let Some(src) = original {
            match scripts::copy_dir(&src, &t.dir) {
                Ok(n) => o.done.push(format!("{label}: put back the folder that was there before Uppwind Link ({n} file{}, from {}).", if n == 1 { "" } else { "s" }, src.display())),
                Err(e) => o.errors.push(format!("{label}: could not put back {} from {} ({e})", t.dir.display(), src.display())),
            }
        }
    }
    removed_live
}

/// (29 oct.) Les plugins Uppwind Link : retires s'ils sont du compagnon (son releve, ou ses octets), laisses sinon.
/// L'invite d'administrateur refusee : ils restent (dit dans `kept`), la desinstallation continue.
fn plugins_out(env: &Env, o: &mut Removal, progress: &dyn Fn(&str)) {
    let b = env.plugin_bundle;
    let mut r = plugins::inspect(&b, &env.plugins);
    plugins::plan(&mut r, true, false);
    let shown = crate::shown_path(&r.folder);
    match r.action {
        plugins::Action::Remove => {}
        plugins::Action::Skip if r.supported && r.status != plugins::Status::Missing => {
            o.done.push(format!("Uppwind Link plugins in {shown}: not installed by Uppwind Link, left as they are."));
            return;
        }
        _ => return,
    }
    progress("Removing the Uppwind Link plugins");
    let a = plugins::apply(&mut r, &b, &env.plugins, progress);
    if a.refused {
        o.kept.extend(a.done);
    } else {
        o.done.extend(a.done);
    }
    o.errors.extend(a.errors);
}

/// Les copies du programme mises de cote parce qu'elles tournaient (install.rs les nomme ainsi).
fn asides(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    rd.flatten().map(|e| e.path()).filter(|p| p.file_name().map(|n| n.to_string_lossy().starts_with("uppwind-companion.old-")).unwrap_or(false)).collect()
}

/// La desinstallation : les reglages de FL remis (FL ouvert : rien n'est retire), les scripts du compagnon retires,
/// les donnees de la fenetre, le programme, ses raccourcis, son journal (install.rs), l'entree « Applications
/// installees ». Restent : les copies de sauvegarde et le fichier d'appairage (dits).
pub fn uninstall(env: &Env, h: &dyn Hooks) -> Removal {
    let mut o = Removal { simulated: env.simulate_daws, ..Default::default() };
    if let Some(l) = h.stop_running() {
        o.done.push(l);
    }
    h.progress("Putting FL Studio's settings back");
    if env.simulate_daws {
        o.done.push("FL Studio's settings: simulated, nothing changed.".into());
    } else {
        match setup::undo_fl_settings_with(env.probe, &env.stamp) {
            Ok(l) => o.done.extend(l),
            Err(e) => {
                o.blocked = Some(e);
                return o;
            }
        }
    }
    h.progress("Removing the DAW scripts");
    let removed_live = scripts_out(env, &mut o);
    // (29 oct.) les plugins Uppwind Link qu'il a mis (sous Windows : une invite d'administrateur)
    plugins_out(env, &mut o, &|l| h.progress(l));

    h.progress("Removing Uppwind Link");
    // (29 oct.) macOS : les ports MIDI partent avec leur gardien (pas avant : FL ouvert bloque tout ce qui precede)
    match h.stop_midi_ports() {
        Ok(Some(l)) => o.done.push(l),
        Ok(None) => {}
        Err(e) => o.errors.push(e),
    }
    let web = webview_dir(&env.dirs);
    let mut later: Vec<PathBuf> = Vec::new();
    if web.exists() {
        if remove_tree(&web) {
            o.done.push(format!("Removed the window data {}", web.display()));
        } else {
            later.push(web.clone());
        }
    }
    match install::uninstall(&env.dirs) {
        // le dossier qui reste a cause des copies de sauvegarde : dit plus bas, pas comme « une copie qui tourne »
        // (l'exe qui tourne, renomme a cote, part apres la sortie : dit plus bas en une phrase)
        Ok(l) => o.done.extend(l.into_iter().filter(|x| !x.contains("still holds a running copy") && !x.contains("renamed it to") && x != "Nothing was installed.")),
        Err(e) => o.errors.push(e),
    }
    match arp::remove(env.arp_reg, &env.arp_key) {
        Ok(l) => o.done.extend(l),
        Err(e) => o.errors.push(e),
    }
    let app = env.dirs.app_dir();
    let backups = env.probe.paths.backups();
    if backups.is_dir() {
        o.kept.push(format!("Copies of the files Uppwind Link replaced, in {} (delete the folder if you do not need them).", backups.display()));
    }
    if env.state_file.exists() {
        o.kept.push(format!("Your pairing code, in {} (the studio stays paired if you install Uppwind Link again).", env.state_file.display()));
    }
    let left = asides(&app);
    if !left.is_empty() || !later.is_empty() {
        h.cleanup_later(&left, &[app.clone()], &later);
        o.done.push("What was still in use (the running copy, its window data) is removed as soon as it closes.".into());
    }
    if removed_live {
        o.todo.push("In Ableton Live: the UppwindLink Control Surface is gone at the next start; set that slot to None.".into());
    }
    if env.probe.running("loopMIDI.exe") || !crate::setup::env::programs(env.probe.reg.as_ref(), |n| n == "loopMIDI").is_empty() {
        o.todo.push("loopMIDI stays installed (it is not part of Uppwind Link): remove it from Installed apps if you do not need it.".into());
    }
    o.ok = o.errors.is_empty();
    o
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::setup::env::{FakeRegistry, Paths};
    use crate::setup::tests::temp_root;
    use std::collections::HashMap;
    use std::ffi::OsString;

    /// Une fausse machine : Live (une User Library), FL 2026 avec ses reglages, loopMIDI et ses ports ; et le dossier
    /// d'installation, tout dans `root`.
    fn machine(root: &Path, fl_running: bool) -> (Probe, Dirs) {
        let mut p = Probe::empty(Paths::in_root(root));
        let ul = root.join("E").join("Ableton").join("User Library");
        std::fs::create_dir_all(&ul).unwrap();
        let prefs = p.paths.roaming.join("Ableton").join("Live 12.4.6").join("Preferences");
        std::fs::create_dir_all(&prefs).unwrap();
        std::fs::write(
            prefs.join("Library.cfg"),
            format!("<Ableton><UserLibrary><LibraryProject Id=\"0\"><ProjectName Value=\"User Library\" /><ProjectPath Value=\"{}\" /></LibraryProject></UserLibrary></Ableton>", ul.parent().unwrap().to_string_lossy().replace('\\', "/")),
        )
        .unwrap();
        std::fs::write(prefs.join("Log.txt"), "t: info: Audio In Out: Driver Type: ASIO\n").unwrap();
        let data = root.join("Documents").join("Image-Line");
        std::fs::create_dir_all(&data).unwrap();
        p.reg = Box::new(FakeRegistry::from_json(&serde_json::json!({
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Ableton Live 12 Suite": { "DisplayName": "Ableton Live 12 Suite", "DisplayVersion": "12" },
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\FL Studio 2026": { "DisplayName": "FL Studio 2026", "DisplayVersion": "26.1.7.5653" },
            r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\{l}": { "DisplayName": "loopMIDI", "DisplayVersion": "1.0.16.27" },
            r"HKCU\Software\Image-Line\Shared\Paths": { "Shared data": data.to_string_lossy() },
            r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI input\Uppwind Link": { "Type": "0", "Port": "-1", "Enabled": "0" },
            r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI output\Uppwind Link Back": { "Port": "-1" },
        })));
        p.midi_in = vec!["Uppwind Link".into(), "Uppwind Link Back".into()];
        p.midi_out = vec!["Uppwind Link".into(), "Uppwind Link Back".into()];
        p.processes = vec!["loopMIDI.exe".into()];
        if fl_running {
            p.processes.push("FL64.exe".into());
        }
        let env: HashMap<&str, OsString> = HashMap::from([(install::ROOT_ENV, root.join("inst").into_os_string())]);
        let dirs = Dirs::from_lookup(|k| env.get(k).cloned()).unwrap();
        (p, dirs)
    }

    struct Rec(std::cell::RefCell<Vec<String>>);
    impl Hooks for Rec {
        fn progress(&self, line: &str) {
            self.0.borrow_mut().push(line.to_string());
        }
        fn start(&self, exe: &Path, args: &[String]) -> Result<Option<u16>, String> {
            assert!(exe.is_file(), "starts the installed copy");
            assert_eq!(&args[..2], &["--background".to_string(), "--replace".to_string()]);
            Ok(Some(47999))
        }
    }

    fn env_for<'a>(dirs: &Dirs, probe: &'a Probe, reg: &'a FakeRegistry, root: &Path, stamp: &str) -> Env<'a> {
        Env {
            dirs: dirs.clone(),
            probe,
            simulate_daws: false,
            arp_reg: reg,
            arp_key: r"HKCU\Software\UppwindLinkTest\flow\UppwindLink".into(),
            state_file: root.join(".uppwind-companion.json"),
            extra_args: vec![],
            stamp: stamp.into(),
            // (29 oct.) un dossier d'essai : jamais le dossier VST3 du systeme, jamais d'invite
            plugins: plugins::Target::in_dir(&root.join("VST3")),
            plugin_bundle: plugins::Bundle { files: TEST_PLUGINS },
        }
    }

    /// (29 oct.) un faux bundle des plugins (celui de Windows : une DLL)
    static TEST_PLUGINS: &[(&str, &[u8], bool)] = &[("Contents/x86_64-win/Uppwind Link.vst3", b"MZ a test plugin", false)];

    fn fake_exe(root: &Path) -> PathBuf {
        let p = root.join("Downloads").join("uppwind-companion.exe");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"MZ the companion").unwrap();
        p
    }

    #[test]
    fn install_then_uninstall_leaves_the_machine_as_it_was() {
        let root = temp_root("flow");
        let (probe, dirs) = machine(&root, false);
        let reg = FakeRegistry::default();
        let fl_before = probe.reg.value(r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI input\Uppwind Link", "Port");
        let env = env_for(&dirs, &probe, &reg, &root, "20261024-120000");
        let mut m = super::super::steps::Model::from_report(&setup::gather(&probe, &Opts { fl_settings: Some(true), ..Default::default() }, "20261024-120000"), None, None, false, String::new());
        // (les plugins : leur propre essai, plus bas ; ici comme une construction sans plugins)
        m.plugins = Default::default();
        let c = Choices::defaults(&m);
        assert_eq!(c.daws.len(), 2, "{:?}", m.daws);
        let rec = Rec(Default::default());
        let o = install(&env, &fake_exe(&root), &c, &rec);
        assert!(o.ok, "{:#?}", o);
        assert!(rec.0.borrow().len() >= 4, "progress lines: {:?}", rec.0.borrow());
        // le programme, ses raccourcis (le menu Demarrer ouvre la fenetre), le demarrage avec Windows
        assert!(dirs.exe().is_file());
        assert_eq!(shortcut::read(&dirs.start_menu_link()).unwrap(), Shortcut { target: dirs.exe(), args: "--app".into(), minimized: true });
        assert_eq!(shortcut::read(&dirs.startup_link()).unwrap().args, "--background");
        assert!(o.autostart && o.listed && o.port == Some(47999));
        // l'entree « Applications installees »
        assert_eq!(arp::installed(&reg, &env.arp_key).unwrap().0, env!("CARGO_PKG_VERSION"));
        assert!(reg.str(&env.arp_key, "UninstallString").unwrap().ends_with("\" --uninstall"));
        // les scripts et les reglages de FL
        let ab = root.join("E").join("Ableton").join("User Library").join("Remote Scripts").join("UppwindLink");
        let fl = root.join("Documents").join("Image-Line").join("FL Studio").join("Settings").join("Hardware").join("Uppwind Link");
        assert!(ab.join("UppwindLink.py").is_file() && fl.join("device_UppwindLink.py").is_file());
        assert_eq!(o.fl, "written", "{:?}", o.fl_lines);
        assert_eq!(probe.reg.str(r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI input\Uppwind Link", "Port").as_deref(), Some("77"));
        assert!(o.backups.is_some());
        // ce qui reste : Live (ses reglages, le pilote ASIO), FL, l'appairage ; pas les ports (ils sont la)
        let ids: Vec<&str> = o.todo.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["live", "live-audio", "fl", "fl-audio", "pair"], "{:#?}", o.todo);
        assert!(!o.todo.iter().flat_map(|t| t.lines.iter()).any(|l| l.contains("CABLE")), "VB-CABLE is never offered at setup");

        // la desinstallation remet tout
        std::fs::write(&env.state_file, "{\"token\":\"x\"}").unwrap();
        std::fs::create_dir_all(webview_dir(&dirs)).unwrap();
        let u = uninstall(&env, &NoHooks);
        assert!(u.ok, "{:#?}", u);
        assert!(!ab.exists() && !fl.exists(), "the scripts are gone: {:#?}", u.done);
        assert_eq!(probe.reg.value(r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI input\Uppwind Link", "Port"), fl_before, "FL's settings put back");
        assert!(!dirs.exe().exists() && !dirs.start_menu_link().exists() && !dirs.startup_link().exists() && !webview_dir(&dirs).exists());
        assert!(!reg.key_exists(&env.arp_key), "the Installed apps entry is gone");
        assert!(u.kept.iter().any(|k| k.contains("backups")) && u.kept.iter().any(|k| k.contains("pairing code")), "{:?}", u.kept);
        assert!(u.todo.iter().any(|t| t.contains("set that slot to None")));
        assert_eq!(std::fs::read_to_string(&env.state_file).unwrap(), "{\"token\":\"x\"}", "the pairing file is left alone");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_marker_that_points_outside_the_backups_is_never_put_back() {
        let root = temp_root("flow-marker");
        let backups = root.join("backups");
        let inside = backups.join("Ableton-20261024-120000");
        let outside = root.join("Documents");
        for d in [&inside, &outside] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join("mine.txt"), "x").unwrap();
        }
        let script = root.join("Remote Scripts").join("UppwindLink");
        std::fs::create_dir_all(&script).unwrap();
        let mark = |to: &Path| std::fs::write(script.join(MARKER), serde_json::json!({ "backup": to.display().to_string() }).to_string()).unwrap();
        mark(&inside);
        assert_eq!(original_of(&script, &backups), Some(inside.clone()));
        // un marqueur modifie (ou venu d'un autre PC) : un dossier quelconque, le dossier des copies lui-meme, un chemin
        // qui en sort par « .. »
        for bad in [outside.clone(), backups.clone(), backups.join("..").join("Documents")] {
            mark(&bad);
            assert_eq!(original_of(&script, &backups), None, "{}", bad.display());
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn uninstall_puts_back_the_users_own_script_folder() {
        let root = temp_root("flow-orig");
        let (probe, dirs) = machine(&root, false);
        let reg = FakeRegistry::default();
        // l'utilisateur avait deja une version a lui (sans marqueur), avec un fichier a lui
        let ab = root.join("E").join("Ableton").join("User Library").join("Remote Scripts").join("UppwindLink");
        std::fs::create_dir_all(&ab).unwrap();
        std::fs::write(ab.join("UppwindLink.py"), "SCRIPT_VERSION = \"0.9.0\"\n").unwrap();
        std::fs::write(ab.join("mine.txt"), "keep me").unwrap();
        let c = Choices { daws: vec![format!("ableton:{}", root.join("E").join("Ableton").join("User Library").display()).to_ascii_lowercase()], fl_settings: false, autostart: false, open_studio: false };
        let o = install(&env_for(&dirs, &probe, &reg, &root, "20261024-130000"), &fake_exe(&root), &c, &NoHooks);
        assert!(o.ok, "{:#?}", o);
        assert!(o.scripts[0].contains("it replaced version 0.9.0"), "{:?}", o.scripts);
        assert!(!dirs.startup_link().exists(), "Start with Windows unticked");
        // une deuxieme installation (une mise a jour du compagnon) : sa copie porte le marqueur, on remonte plus loin
        std::fs::write(ab.join("UppwindLink.py"), "SCRIPT_VERSION = \"1.0.0\"\n").unwrap();
        let o2 = install(&env_for(&dirs, &probe, &reg, &root, "20261024-130100"), &fake_exe(&root), &c, &NoHooks);
        assert!(o2.ok, "{:#?}", o2);
        assert_eq!(original_of(&ab, &probe.paths.backups()).map(|p| p.join("mine.txt").is_file()), Some(true), "the user's own copy, two backups back");
        let u = uninstall(&env_for(&dirs, &probe, &reg, &root, "20261024-130200"), &NoHooks);
        assert!(u.ok, "{:#?}", u);
        assert_eq!(std::fs::read_to_string(ab.join("mine.txt")).unwrap(), "keep me", "the folder from before Uppwind Link is back");
        assert!(std::fs::read_to_string(ab.join("UppwindLink.py")).unwrap().contains("0.9.0"));
        assert!(!ab.join(MARKER).exists());
        // un dossier qui n'est pas du compagnon n'est jamais retire
        let u2 = uninstall(&env_for(&dirs, &probe, &reg, &root, "20261024-130300"), &NoHooks);
        assert!(u2.done.iter().any(|l| l.contains("not installed by Uppwind Link")), "{:?}", u2.done);
        assert!(ab.join("mine.txt").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fl_open_blocks_its_settings_but_not_the_rest_and_blocks_the_uninstall() {
        let root = temp_root("flow-flopen");
        let (probe, dirs) = machine(&root, true);
        let reg = FakeRegistry::default();
        let c = Choices { daws: vec!["fl".into()], fl_settings: true, autostart: false, open_studio: true };
        let o = install(&env_for(&dirs, &probe, &reg, &root, "20261024-140000"), &fake_exe(&root), &c, &NoHooks);
        assert!(o.ok, "FL being open is not an error: {:#?}", o);
        assert_eq!(o.fl, "blocked");
        assert!(o.todo.iter().any(|t| t.id == "fl" && t.title == "FL Studio was open"), "{:#?}", o.todo);
        assert_eq!(probe.reg.str(r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI input\Uppwind Link", "Port").as_deref(), Some("-1"), "not written");
        // FL ferme : ecrit ; puis FL rouvert : la desinstallation ne retire rien
        let (mut probe2, _) = machine(&root, false);
        let o2 = install(&env_for(&dirs, &probe2, &reg, &root, "20261024-140100"), &fake_exe(&root), &c, &NoHooks);
        assert_eq!(o2.fl, "written", "{:?}", o2.fl_lines);
        probe2.processes.push("FL64.exe".into());
        let u = uninstall(&env_for(&dirs, &probe2, &reg, &root, "20261024-140200"), &NoHooks);
        assert!(!u.ok && u.blocked.as_deref().is_some_and(|b| b.contains("close FL Studio")), "{:#?}", u);
        assert!(dirs.exe().is_file() && reg.key_exists(r"HKCU\Software\UppwindLinkTest\flow\UppwindLink"), "nothing was removed");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// (29 oct.) le gardien des ports MIDI (macOS, midi_ports.rs) : ce que le crochet en dit, et s'il a ete appele
    struct Keeper(Result<Option<String>, String>, std::cell::Cell<bool>);
    impl Hooks for Keeper {
        fn stop_midi_ports(&self) -> Result<Option<String>, String> {
            self.1.set(true);
            self.0.clone()
        }
    }

    #[test]
    fn the_midi_ports_keeper_goes_with_the_program_never_while_fl_blocks() {
        let root = temp_root("flow-keeper");
        let (mut probe, dirs) = machine(&root, false);
        let reg = FakeRegistry::default();
        let c = Choices { daws: vec!["fl".into()], fl_settings: true, autostart: false, open_studio: false };
        let o = install(&env_for(&dirs, &probe, &reg, &root, "20261029-120000"), &fake_exe(&root), &c, &NoHooks);
        assert_eq!(o.fl, "written", "{:?}", o.fl_lines);
        // FL ouvert : rien n'est retire, le gardien (et les ports de FL) restent
        probe.processes.push("FL64.exe".into());
        let k = Keeper(Ok(Some("Stopped the MIDI ports keeper (pid 7).".into())), Default::default());
        let u = uninstall(&env_for(&dirs, &probe, &reg, &root, "20261029-120100"), &k);
        assert!(u.blocked.is_some() && !k.1.get(), "{:#?}", u);
        // FL ferme : il part avec le programme, et c'est dit
        probe.processes.retain(|p| p != "FL64.exe");
        let u = uninstall(&env_for(&dirs, &probe, &reg, &root, "20261029-120200"), &k);
        assert!(k.1.get() && u.ok && u.done.iter().any(|l| l == "Stopped the MIDI ports keeper (pid 7)."), "{:#?}", u);
        // ne pas pouvoir l'arreter : dit, et la desinstallation n'est pas reussie
        let e = Keeper(Err("cannot stop the MIDI ports keeper (pid 7).".into()), Default::default());
        let u = uninstall(&env_for(&dirs, &probe, &reg, &root, "20261029-120300"), &e);
        assert!(e.1.get() && !u.ok && u.errors.iter().any(|l| l.contains("MIDI ports keeper")), "{:#?}", u);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_sandbox_simulates_the_daws_and_carries_its_folder() {
        let root = temp_root("flow-sandbox");
        let (probe, dirs) = machine(&root, false);
        let reg = FakeRegistry::default();
        let mut env = env_for(&dirs, &probe, &reg, &root, "20261024-150000");
        env.simulate_daws = true;
        env.plugins.simulate = true;
        env.extra_args = vec!["--sandbox".into(), r"D:\my sandbox".into()];
        let mut m = super::super::steps::Model::from_report(&setup::gather(&probe, &Opts { fl_settings: Some(true), ..Default::default() }, "x"), None, None, true, String::new());
        m.plugins = super::super::steps::tests::plugins("install");
        let o = install(&env, &fake_exe(&root), &Choices::defaults(&m), &NoHooks);
        assert!(o.ok && o.simulated, "{:#?}", o);
        assert!(o.scripts.iter().all(|s| s.contains("simulated")), "{:?}", o.scripts);
        assert_eq!(o.fl, "simulated");
        assert_eq!(o.plugins, "simulated", "{:?}", o.plugin_lines);
        assert!(!root.join("VST3").exists(), "no plugin written");
        assert!(!root.join("E").join("Ableton").join("User Library").join("Remote Scripts").exists(), "nothing written in Live's folders");
        assert_eq!(shortcut::read(&dirs.start_menu_link()).unwrap().args, r#"--app --sandbox "D:\my sandbox""#);
        assert_eq!(shortcut::read(&dirs.startup_link()).unwrap().args, r#"--background --sandbox "D:\my sandbox""#);
        assert!(reg.str(&env.arp_key, "UninstallString").unwrap().ends_with(r#"--uninstall --sandbox "D:\my sandbox""#));
        let u = uninstall(&env, &NoHooks);
        assert!(u.ok && u.simulated, "{:#?}", u);
        assert!(!dirs.exe().exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// (29 oct.) les plugins Uppwind Link : installes avec le reste (dans un dossier d'essai, sans invite), dits dans ce
    /// qui reste a faire, retires par la desinstallation ; ceux d'un autre, laisses.
    #[test]
    fn the_plugins_come_with_the_install_and_go_with_the_uninstall() {
        let root = temp_root("flow-plugins");
        let (probe, dirs) = machine(&root, false);
        let reg = FakeRegistry::default();
        let env = env_for(&dirs, &probe, &reg, &root, "20261029-130000");
        let dll = root.join("VST3").join("Uppwind Link.vst3").join("Contents").join("x86_64-win").join("Uppwind Link.vst3");
        let c = Choices { daws: vec!["fl".into(), super::super::steps::PLUGINS.into()], fl_settings: false, autostart: false, open_studio: false };
        let rec = Rec(Default::default());
        let o = install(&env, &fake_exe(&root), &c, &rec);
        assert!(o.ok, "{:#?}", o);
        assert_eq!(o.plugins, "installed", "{:?}", o.plugin_lines);
        assert!(o.plugin_lines.iter().any(|l| l.contains("installed in")), "{:?}", o.plugin_lines);
        assert_eq!(std::fs::read(&dll).unwrap(), TEST_PLUGINS[0].1);
        assert!(rec.0.borrow().iter().any(|l| l == "Installing the Uppwind Link plugins"), "{:?}", rec.0.borrow());
        let scan = o.todo.iter().find(|t| t.id == "plugins-scan").expect("say where to find them");
        assert!(scan.lines.iter().any(|l| l.contains("Find installed plugins")), "{scan:?}");
        // deja la : rien a faire, pas de ligne de recherche dans le DAW
        let o2 = install(&env, &fake_exe(&root), &c, &NoHooks);
        assert_eq!(o2.plugins, "unchanged");
        assert!(!o2.todo.iter().any(|t| t.id.starts_with("plugins")), "{:#?}", o2.todo);
        // pas coches : pas touches
        let mut c3 = c.clone();
        c3.daws.retain(|d| d != super::super::steps::PLUGINS);
        assert_eq!(install(&env, &fake_exe(&root), &c3, &NoHooks).plugins, "");
        // la desinstallation les annonce, puis les retire
        assert!(plan_uninstall(&env).removes.iter().any(|l| l.starts_with("The Uppwind Link plugins")), "{:?}", plan_uninstall(&env).removes);
        let u = uninstall(&env, &NoHooks);
        assert!(u.ok, "{:#?}", u);
        assert!(u.done.iter().any(|l| l.starts_with("Removed the Uppwind Link plugins")), "{:?}", u.done);
        assert!(!root.join("VST3").join("Uppwind Link.vst3").exists() && !env.plugins.marker.exists());
        // un bundle que le compagnon n'a pas mis : laisse
        std::fs::create_dir_all(dll.parent().unwrap()).unwrap();
        std::fs::write(&dll, "a build of my own").unwrap();
        assert!(!plan_uninstall(&env).removes.iter().any(|l| l.starts_with("The Uppwind Link plugins")));
        let u = uninstall(&env, &NoHooks);
        assert!(u.done.iter().any(|l| l.contains("not installed by Uppwind Link, left as they are")), "{:?}", u.done);
        assert!(dll.is_file());
        let _ = std::fs::remove_dir_all(&root);
    }
}
