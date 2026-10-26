// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! La fenetre d'installation, sans fenetre : ce qu'elle montre (`Model`, tire du rapport de `--setup`), ce que
//! l'utilisateur coche (`Choices`), et les etapes (`Wizard`, une machine a etats). La page (src/ui/) ne fait que
//! dessiner l'etat que la machine lui envoie ; chaque geste lui revient comme un evenement, verifie ici.
//!
//!   Welcome ──Install──► les etapes qui demandent quelque chose (un DAW ouvert qui bloque, des ports MIDI manquants),
//!          │             puis Progress ──► Done
//!          └─Customize─► Daws ► Ports ► Options ──Install──► Progress ──► Done

use crate::setup::scripts::{Action, Daw, Status, Target};
use crate::setup::{fl_settings, is_back_port, is_link_port, same_path, Report};
use serde::{Deserialize, Serialize};

/// Un DAW trouve : une User Library de Live, ou FL Studio.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DawItem {
    /// « ableton:<User Library> » ou « fl »
    pub id: String,
    /// « ableton » ou « fl »
    pub daw: &'static str,
    /// « Ableton Live 12 Suite », « FL Studio 2026 »
    pub name: String,
    /// les versions qui se servent de ce dossier (Live : celles qui partagent la User Library)
    pub versions: Vec<String>,
    /// le dossier du script
    pub folder: String,
    /// missing, up-to-date, outdated, different, newer, no-library
    pub status: String,
    pub installed_version: Option<String>,
    pub bundled_version: String,
    /// install, update, none, skip
    pub action: String,
    pub reason: String,
    /// le DAW tourne (le nom de son exe)
    pub running: Option<String>,
    /// false : rien a y faire (User Library absente)
    pub selectable: bool,
    /// FL : ses reglages MIDI
    pub fl_settings: Option<FlItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FlItem {
    /// combien de valeurs le compagnon ecrirait (0 : deja bons, ou FL n'a encore rien enregistre)
    pub changes: usize,
    /// FL tourne : rien ne s'ecrit tant qu'il est ouvert
    pub blocked_by: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Ports {
    /// le nom trouve pour « Uppwind Link » (entree ou sortie de Windows)
    pub link: Option<String>,
    pub back: Option<String>,
    /// des noms presque justes (« Upwind Link », « UppwindLink ») : a renommer
    pub near: Vec<String>,
    pub loopmidi_installed: bool,
    pub loopmidi_running: bool,
}

impl Ports {
    pub fn ok(&self) -> bool {
        self.link.is_some() && self.back.is_some()
    }
}

/// Ce que la fenetre montre.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Model {
    pub version: String,
    /// une mise a jour : la version installee (« » : inconnue)
    pub update_from: Option<String>,
    /// le bac a sable (--sandbox) : tout s'installe dans ce dossier
    pub sandbox: Option<String>,
    /// les changements dans Live et FL ne sont que simules (le bac a sable)
    pub simulate_daws: bool,
    pub install_dir: String,
    pub daws: Vec<DawItem>,
    pub ports: Ports,
}

/// Les noms de ports « presque justes » (comme le guide du studio, link-setup.js portReport) : on les montre pour qu'on
/// les corrige. « Uppwind Link 1 » ou « IAC Driver (Uppwind Link) » sont justes (le studio les prend).
pub fn ports_of(inputs: &[String], outputs: &[String]) -> (Option<String>, Option<String>, Vec<String>) {
    let all: Vec<&String> = inputs.iter().chain(outputs.iter()).collect();
    let link = all.iter().find(|n| is_link_port(n)).map(|n| n.to_string());
    let back = all.iter().find(|n| is_back_port(n)).map(|n| n.to_string());
    let squash = |n: &str| n.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphabetic()).collect::<String>();
    let mut near: Vec<String> = Vec::new();
    for n in all {
        let s = squash(n);
        if (s.contains("uppwind") || s.contains("upwind")) && !is_link_port(n) && !is_back_port(n) && !near.contains(n) {
            near.push(n.clone());
        }
    }
    (link, back, near)
}

/// L'identifiant d'un dossier de script : « ableton:<User Library> » (en minuscules), « fl ».
pub fn target_id(t: &Target) -> String {
    match t.daw {
        Daw::Ableton => format!("ableton:{}", t.library.display()).to_ascii_lowercase(),
        Daw::Fl => "fl".into(),
    }
}

fn status_name(s: Status) -> String {
    serde_json::to_value(s).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()
}
fn action_name(a: Action) -> String {
    serde_json::to_value(a).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()
}

impl Model {
    /// Le modele d'un rapport de `--setup` (une simulation, avec --fl-settings : il dit ce qui s'ecrirait).
    pub fn from_report(r: &Report, update_from: Option<String>, sandbox: Option<String>, simulate_daws: bool, install_dir: String) -> Model {
        let mut daws = Vec::new();
        for t in &r.scripts {
            let common = |id: String, daw: &'static str, name: String, versions: Vec<String>, running: Option<String>, fl: Option<FlItem>| DawItem {
                id,
                daw,
                name,
                versions,
                folder: t.dir.display().to_string(),
                status: status_name(t.status),
                installed_version: t.installed_version.clone(),
                bundled_version: t.bundled_version.clone(),
                action: action_name(t.action),
                reason: t.reason.clone(),
                running,
                // une User Library absente, ou une version installee plus recente : montre, pas coche
                selectable: t.action != Action::Skip,
                fl_settings: fl,
            };
            match t.daw {
                Daw::Ableton => {
                    let a = &r.ableton;
                    let versions: Vec<String> = a.prefs.iter().filter(|p| same_path(&p.user_library, &t.library)).map(|p| p.version.clone()).collect();
                    let name = a.programs.first().map(|p| p.name.clone()).unwrap_or_else(|| "Ableton Live".into());
                    daws.push(common(target_id(t), "ableton", name, versions, a.running.clone(), None));
                }
                Daw::Fl => {
                    let f = &r.fl;
                    let x = &r.fl_settings;
                    let changes = x.changes.iter().filter(|c| c.opt_in == fl_settings::OPT_SETTINGS).count();
                    let fl = FlItem { changes, blocked_by: f.running.clone().filter(|_| changes > 0), reason: x.reason.clone() };
                    let name = f.programs.first().map(|p| p.name.clone()).unwrap_or_else(|| "FL Studio".into());
                    let versions = f.programs.iter().map(|p| p.version.clone()).filter(|v| !v.is_empty()).collect();
                    daws.push(common("fl".into(), "fl", name, versions, f.running.clone(), Some(fl)));
                }
            }
        }
        let (link, back, near) = ports_of(&r.midi.inputs, &r.midi.outputs);
        Model {
            version: env!("CARGO_PKG_VERSION").into(),
            update_from,
            sandbox,
            simulate_daws,
            install_dir,
            daws,
            ports: Ports { link, back, near, loopmidi_installed: r.midi.loopmidi.is_some(), loopmidi_running: r.midi.loopmidi_running },
        }
    }

    pub fn daw(&self, id: &str) -> Option<&DawItem> {
        self.daws.iter().find(|d| d.id == id)
    }
}

/// Ce que l'utilisateur choisit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choices {
    /// les DawItem.id coches
    pub daws: Vec<String>,
    /// « Apply FL Studio's MIDI settings for me »
    pub fl_settings: bool,
    /// « Start with Windows »
    pub autostart: bool,
    /// « Open the studio when done »
    pub open_studio: bool,
}

impl Choices {
    /// Tout ce qui a un sens, coche.
    pub fn defaults(m: &Model) -> Choices {
        Choices {
            daws: m.daws.iter().filter(|d| d.selectable && d.action != "skip").map(|d| d.id.clone()).collect(),
            fl_settings: true,
            autostart: true,
            open_studio: true,
        }
    }

    /// Les choix que la page envoie : seulement des DAW connus et cochables, sans doublon.
    pub fn validated(mut self, m: &Model) -> Result<Choices, String> {
        let mut seen: Vec<String> = Vec::new();
        for id in &self.daws {
            match m.daw(id) {
                Some(d) if d.selectable => {
                    if !seen.contains(id) {
                        seen.push(id.clone());
                    }
                }
                Some(_) => return Err(format!("{id} cannot be installed")),
                None => return Err("unknown DAW".into()),
            }
        }
        self.daws = seen;
        Ok(self)
    }

    pub fn wants(&self, id: &str) -> bool {
        self.daws.iter().any(|d| d == id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Step {
    Welcome,
    Daws,
    Ports,
    Options,
    Progress,
    Done,
}

/// Ce que l'installation a fait (flow.rs) : la page « Done ».
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Outcome {
    pub ok: bool,
    /// une simulation (bac a sable) : rien n'a change dans Live ni dans FL
    pub simulated: bool,
    /// ce qui a ete fait, en phrases
    pub done: Vec<String>,
    pub errors: Vec<String>,
    /// les scripts : « Ableton Live: UppwindLink 1.2.0 installed in … »
    pub scripts: Vec<String>,
    /// FL : written, unchanged, blocked, skipped, none
    pub fl: String,
    pub fl_lines: Vec<String>,
    /// le dossier des copies de sauvegarde, si des copies ont ete faites
    pub backups: Option<String>,
    pub autostart: bool,
    /// l'entree « Applications installees »
    pub listed: bool,
    /// le compagnon installe tourne : son port
    pub port: Option<u16>,
    /// ce qui reste a faire, dans l'ordre
    pub todo: Vec<Todo>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Todo {
    /// live, live-dev, live-audio, fl, fl-audio, ports, pair, restart-live, restart-fl
    pub id: String,
    pub title: String,
    pub lines: Vec<String>,
}

/// Ce que la machine demande a la fenetre de faire.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// redessiner (l'etat a change)
    Render,
    /// relire la machine (« Check again »)
    Detect,
    /// lancer l'installation avec ces choix
    Install(Choices),
    /// fermer la fenetre ; ouvrir le studio d'abord si demande
    Finish { open_studio: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Welcome › Install : le chemin court
    Install,
    /// Welcome › Customize : toutes les etapes
    Customize,
    Next,
    Back,
    /// la page envoie ses cases
    Choose(Choices),
    /// « Check again »
    Recheck,
    /// la machine relue
    Detected(Model),
    /// l'installation est finie
    Finished(Outcome),
    /// Done › Finish
    Finish,
}

#[derive(Debug, Clone, Serialize)]
pub struct Wizard {
    pub step: Step,
    /// le chemin court (Install depuis Welcome) : seulement les etapes qui demandent quelque chose
    pub express: bool,
    pub model: Model,
    pub choices: Choices,
    pub outcome: Option<Outcome>,
    /// une relecture en cours
    pub checking: bool,
    #[serde(skip)]
    history: Vec<Step>,
}

impl Wizard {
    pub fn new(model: Model) -> Wizard {
        let choices = Choices::defaults(&model);
        Wizard { step: Step::Welcome, express: false, model, choices, outcome: None, checking: false, history: vec![] }
    }

    /// Une etape demande-t-elle quelque chose ? (le chemin court ne montre que celles-la)
    pub fn needs_attention(&self, s: Step) -> bool {
        match s {
            // aucun DAW trouve (le dire), ou FL ouvert alors que ses reglages sont coches
            Step::Daws => {
                self.model.daws.is_empty()
                    || self.model.daws.iter().any(|d| self.choices.wants(&d.id) && self.choices.fl_settings && d.fl_settings.as_ref().is_some_and(|f| f.blocked_by.is_some()))
            }
            Step::Ports => !self.model.ports.ok(),
            _ => false,
        }
    }

    /// L'etape apres `from`, ou None : il est temps d'installer. Le chemin court saute celles qui ne demandent rien
    /// (recalcule a chaque pas : une etape reglee entre-temps, FL ferme par exemple, n'arrete plus).
    fn after(&self, from: Step) -> Option<Step> {
        const ORDER: [Step; 3] = [Step::Daws, Step::Ports, Step::Options];
        let start = match from {
            Step::Welcome => 0,
            s => ORDER.iter().position(|x| *x == s).map(|i| i + 1).unwrap_or(ORDER.len()),
        };
        ORDER[start..].iter().copied().find(|s| !self.express || self.needs_attention(*s))
    }

    fn go(&mut self, to: Step) {
        self.history.push(self.step);
        self.step = to;
    }

    fn start(&mut self) -> Vec<Effect> {
        self.go(Step::Progress);
        vec![Effect::Render, Effect::Install(self.choices.clone())]
    }

    /// Un evenement → ce que la fenetre doit faire ; une erreur : l'evenement n'a pas de sens a cette etape (rien ne
    /// change).
    pub fn on(&mut self, ev: Event) -> Result<Vec<Effect>, String> {
        let busy = matches!(self.step, Step::Progress | Step::Done);
        match ev {
            Event::Install | Event::Customize if self.step != Step::Welcome => Err("only from the first page".into()),
            Event::Install | Event::Customize => {
                self.express = ev == Event::Install;
                match self.after(Step::Welcome) {
                    Some(s) => {
                        self.go(s);
                        Ok(vec![Effect::Render])
                    }
                    None => Ok(self.start()),
                }
            }
            Event::Next if busy || self.step == Step::Welcome => Err("no next page here".into()),
            Event::Next => match self.after(self.step) {
                Some(s) => {
                    self.go(s);
                    Ok(vec![Effect::Render])
                }
                None => Ok(self.start()),
            },
            Event::Back if busy => Err("the installation has started".into()),
            Event::Back => match self.history.pop() {
                Some(s) => {
                    self.step = s;
                    if s == Step::Welcome {
                        self.express = false;
                    }
                    Ok(vec![Effect::Render])
                }
                None => Err("no page before this one".into()),
            },
            Event::Choose(_) if busy => Err("the installation has started".into()),
            Event::Choose(c) => {
                self.choices = c.validated(&self.model)?;
                Ok(vec![Effect::Render])
            }
            Event::Recheck if busy => Err("the installation has started".into()),
            Event::Recheck => {
                self.checking = true;
                Ok(vec![Effect::Render, Effect::Detect])
            }
            Event::Detected(m) => {
                self.checking = false;
                // les DAW coches restent coches ; ceux qui sont apparus le sont aussi
                let before: Vec<String> = self.model.daws.iter().map(|d| d.id.clone()).collect();
                let mut daws: Vec<String> = self.choices.daws.iter().filter(|id| m.daw(id).is_some_and(|d| d.selectable)).cloned().collect();
                for d in &m.daws {
                    if !before.contains(&d.id) && d.selectable && d.action != "skip" {
                        daws.push(d.id.clone());
                    }
                }
                self.model = m;
                self.choices.daws = daws;
                Ok(vec![Effect::Render])
            }
            Event::Finished(o) if self.step == Step::Progress => {
                self.outcome = Some(o);
                self.go(Step::Done);
                Ok(vec![Effect::Render])
            }
            Event::Finished(_) => Err("nothing was being installed".into()),
            Event::Finish if self.step == Step::Done => Ok(vec![Effect::Finish { open_studio: self.choices.open_studio && self.outcome.as_ref().is_some_and(|o| o.ok) }]),
            Event::Finish => Err("not finished".into()),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn item(id: &str, daw: &'static str, action: &str, running: Option<&str>, fl_changes: usize) -> DawItem {
        DawItem {
            id: id.into(),
            daw,
            name: if daw == "fl" { "FL Studio 2026".into() } else { "Ableton Live 12 Suite".into() },
            versions: vec!["12.4.6".into()],
            folder: format!(r"C:\x\{id}"),
            status: if action == "install" { "missing".into() } else { "up-to-date".into() },
            installed_version: None,
            bundled_version: "1.2.0".into(),
            action: action.into(),
            reason: String::new(),
            running: running.map(String::from),
            selectable: action != "skip",
            fl_settings: (daw == "fl").then(|| FlItem { changes: fl_changes, blocked_by: running.filter(|_| fl_changes > 0).map(String::from), reason: String::new() }),
        }
    }

    pub fn model(daws: Vec<DawItem>, ports_ok: bool) -> Model {
        Model {
            version: "0.1.0".into(),
            update_from: None,
            sandbox: None,
            simulate_daws: false,
            install_dir: r"C:\Users\x\AppData\Local\Uppwind Link".into(),
            daws,
            ports: Ports {
                link: ports_ok.then(|| "Uppwind Link".into()),
                back: ports_ok.then(|| "Uppwind Link Back".into()),
                near: vec![],
                loopmidi_installed: ports_ok,
                loopmidi_running: ports_ok,
            },
        }
    }

    #[test]
    fn install_goes_straight_to_the_progress_when_nothing_needs_the_user() {
        let mut w = Wizard::new(model(vec![item("ableton:e:\\ul", "ableton", "install", None, 0), item("fl", "fl", "install", None, 4)], true));
        assert_eq!(w.choices, Choices { daws: vec!["ableton:e:\\ul".into(), "fl".into()], fl_settings: true, autostart: true, open_studio: true }, "everything ticked");
        let fx = w.on(Event::Install).unwrap();
        assert_eq!(w.step, Step::Progress);
        assert_eq!(fx, vec![Effect::Render, Effect::Install(w.choices.clone())]);
        assert!(w.on(Event::Back).is_err(), "no way back once it runs");
        assert!(w.on(Event::Install).is_err());
        assert!(w.on(Event::Finish).is_err(), "not finished yet");
        w.on(Event::Finished(Outcome { ok: true, ..Default::default() })).unwrap();
        assert_eq!(w.step, Step::Done);
        assert_eq!(w.on(Event::Finish).unwrap(), vec![Effect::Finish { open_studio: true }]);
    }

    #[test]
    fn install_stops_where_the_user_is_needed() {
        // FL ouvert, ses reglages coches ; ports manquants
        let mut w = Wizard::new(model(vec![item("ableton:e:\\ul", "ableton", "install", Some("Ableton Live 12 Suite.exe"), 0), item("fl", "fl", "install", Some("FL64.exe"), 4)], false));
        assert!(w.needs_attention(Step::Daws) && w.needs_attention(Step::Ports) && !w.needs_attention(Step::Options));
        w.on(Event::Install).unwrap();
        assert_eq!(w.step, Step::Daws);
        // FL ferme, re-verifie : la page des DAW ne demande plus rien, mais on y est ; Next passe aux ports
        w.on(Event::Recheck).unwrap();
        assert!(w.checking);
        let fixed = model(vec![item("ableton:e:\\ul", "ableton", "install", None, 0), item("fl", "fl", "install", None, 4)], false);
        w.on(Event::Detected(fixed)).unwrap();
        assert!(!w.checking && !w.needs_attention(Step::Daws));
        w.on(Event::Next).unwrap();
        assert_eq!(w.step, Step::Ports, "the ports are still missing");
        // on peut passer : l'installation part (pas d'Options dans le chemin court)
        let fx = w.on(Event::Next).unwrap();
        assert_eq!(w.step, Step::Progress);
        assert!(matches!(fx[1], Effect::Install(_)));
    }

    #[test]
    fn unticking_fl_settings_lifts_the_block() {
        let mut w = Wizard::new(model(vec![item("fl", "fl", "install", Some("FL64.exe"), 4)], true));
        assert!(w.needs_attention(Step::Daws));
        let mut c = w.choices.clone();
        c.fl_settings = false;
        w.on(Event::Choose(c)).unwrap();
        assert!(!w.needs_attention(Step::Daws), "FL may stay open if its settings are not written");
        let mut w2 = Wizard::new(model(vec![item("fl", "fl", "install", Some("FL64.exe"), 0)], true));
        assert!(!w2.needs_attention(Step::Daws), "nothing to write: FL can stay open");
        assert_eq!(w2.on(Event::Install).unwrap()[1], Effect::Install(w2.choices.clone()));
    }

    #[test]
    fn customize_walks_every_page_and_back() {
        let mut w = Wizard::new(model(vec![item("ableton:e:\\ul", "ableton", "install", None, 0)], true));
        w.on(Event::Customize).unwrap();
        assert_eq!(w.step, Step::Daws);
        w.on(Event::Next).unwrap();
        assert_eq!(w.step, Step::Ports, "shown even when the ports are there");
        w.on(Event::Next).unwrap();
        assert_eq!(w.step, Step::Options);
        w.on(Event::Back).unwrap();
        w.on(Event::Back).unwrap();
        assert_eq!(w.step, Step::Daws);
        w.on(Event::Back).unwrap();
        assert_eq!(w.step, Step::Welcome);
        assert!(w.on(Event::Back).is_err());
        assert!(w.on(Event::Next).is_err(), "Welcome has Install or Customize, not Next");
        w.on(Event::Customize).unwrap();
        w.on(Event::Next).unwrap();
        w.on(Event::Next).unwrap();
        let mut c = w.choices.clone();
        c.autostart = false;
        c.open_studio = false;
        w.on(Event::Choose(c.clone())).unwrap();
        assert_eq!(w.on(Event::Next).unwrap(), vec![Effect::Render, Effect::Install(c)]);
        w.on(Event::Finished(Outcome { ok: true, ..Default::default() })).unwrap();
        assert_eq!(w.on(Event::Finish).unwrap(), vec![Effect::Finish { open_studio: false }]);
    }

    #[test]
    fn choices_are_checked() {
        let m = model(vec![item("ableton:e:\\ul", "ableton", "install", None, 0), item("ableton:f:\\gone", "ableton", "skip", None, 0)], true);
        let mut w = Wizard::new(m);
        assert_eq!(w.choices.daws, vec!["ableton:e:\\ul".to_string()], "a User Library on a missing drive is not ticked");
        let bad = Choices { daws: vec!["fl".into()], ..w.choices.clone() };
        assert!(w.on(Event::Choose(bad)).is_err(), "not on this computer");
        let skip = Choices { daws: vec!["ableton:f:\\gone".into()], ..w.choices.clone() };
        assert!(w.on(Event::Choose(skip)).is_err());
        let dup = Choices { daws: vec!["ableton:e:\\ul".into(), "ableton:e:\\ul".into()], ..w.choices.clone() };
        w.on(Event::Choose(dup)).unwrap();
        assert_eq!(w.choices.daws.len(), 1);
        let none = Choices { daws: vec![], ..w.choices.clone() };
        w.on(Event::Choose(none)).unwrap();
        assert!(w.choices.daws.is_empty(), "the companion alone, without DAW scripts, is a choice");
    }

    #[test]
    fn a_failed_install_does_not_open_the_studio() {
        let mut w = Wizard::new(model(vec![], true));
        assert!(w.needs_attention(Step::Daws), "no DAW: say so");
        w.on(Event::Install).unwrap();
        assert_eq!(w.step, Step::Daws);
        w.on(Event::Next).unwrap();
        assert_eq!(w.step, Step::Progress);
        w.on(Event::Finished(Outcome { ok: false, errors: vec!["x".into()], ..Default::default() })).unwrap();
        assert_eq!(w.on(Event::Finish).unwrap(), vec![Effect::Finish { open_studio: false }]);
        assert!(w.on(Event::Finished(Outcome::default())).is_err(), "only once");
    }

    #[test]
    fn port_names_close_enough_or_to_rename() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let (l, b, near) = ports_of(&s(&["loopMIDI Port", "Uppwind Link 1", "Uppwind Link Back"]), &s(&["Uppwind Link 1", "Uppwind Link Back"]));
        assert_eq!((l.as_deref(), b.as_deref()), (Some("Uppwind Link 1"), Some("Uppwind Link Back")));
        assert!(near.is_empty());
        let (l, b, near) = ports_of(&s(&["Upwind Link", "UppwindLink Back"]), &s(&[]));
        assert_eq!((l, b), (None, None));
        assert_eq!(near, s(&["Upwind Link", "UppwindLink Back"]), "shown so they get renamed");
        let (l, _, _) = ports_of(&s(&[]), &s(&["IAC Driver (Uppwind Link)"]));
        assert!(l.is_some());
    }
}
