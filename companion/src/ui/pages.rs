// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Ce que chaque fenetre fait des gestes de sa page, sans fenetre : une demande (ipc.rs) → des effets (`Out`), et les
//! travaux longs (`Job`, dans un fil) qui reviennent en `Done`. Les essais appellent ces fonctions directement.

use super::ipc::{Link, Request};
use crate::tray::status::StatusModel;
use crate::wizard::flow::{Removal, UninstallPlan};
use crate::wizard::steps::{Choices, Effect, Event, Model, Outcome, Step, Wizard};
use serde_json::{json, Value};

/// Ce que la fenetre doit faire.
#[derive(Debug)]
pub enum Out {
    /// a la page (window.__uwl.recv)
    Send(Value),
    /// un lien de la liste fermee, dans le navigateur
    Open(Link),
    /// dans le presse-papiers
    Copy(String),
    /// fermer cette fenetre
    Close,
    /// fermer et sortir avec ce code
    Exit(i32),
    /// un travail dans un fil
    Run(Job),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    /// relire la machine (la fenetre d'installation) ; la version installee
    Detect(Option<String>),
    Install(Choices),
    /// le modele de la fenetre d'etat
    Status,
    UninstallPlan,
    Uninstall,
    Autostart(bool),
    /// la fenetre d'installation, lancee depuis la fenetre d'etat
    SetupAgain,
}

#[derive(Debug)]
pub enum Done {
    Detected(Result<Model, String>),
    /// ce que l'installation a fait, et le code d'appairage du compagnon lance
    Installed(Outcome, Option<String>),
    Status(Result<Box<StatusModel>, String>),
    Plan(Result<UninstallPlan, String>),
    Removed(Removal),
    Autostart(Result<Vec<String>, String>),
    Launched(Result<(), String>),
}

/// Une fenetre, vue de sa page.
pub trait View {
    fn page(&self) -> super::ipc::Page;
    fn title(&self) -> &'static str;
    /// en pixels logiques
    fn size(&self) -> (f64, f64);
    fn on_request(&mut self, r: Request) -> Vec<Out>;
    fn on_done(&mut self, d: Done) -> Vec<Out>;
    fn on_progress(&mut self, line: String) -> Vec<Out> {
        vec![Out::Send(json!({ "type": "progress", "line": line }))]
    }
}

fn error(msg: &str) -> Out {
    Out::Send(json!({ "type": "error", "message": msg }))
}

// ── la fenetre d'installation ──────────────────────────────────────────────────────────────────────────

pub struct WizardPage {
    pub w: Wizard,
    pub progress: Vec<String>,
    /// le code d'appairage du compagnon installe et lance (la page « Done »)
    pub code: Option<String>,
    /// l'essai --smoke-test : rien ne s'installe
    smoke: bool,
}

impl WizardPage {
    pub fn new(w: Wizard) -> WizardPage {
        WizardPage { w, progress: vec![], code: None, smoke: false }
    }
    pub fn smoke(mut self) -> WizardPage {
        self.smoke = true;
        self
    }
    pub fn state(&self) -> Value {
        json!({ "type": "state", "wizard": self.w, "progress": self.progress, "code": self.code })
    }
    fn event(&mut self, e: Event) -> Vec<Out> {
        match self.w.on(e) {
            Ok(fx) => {
                let mut out = Vec::new();
                for f in fx {
                    match f {
                        Effect::Render => out.push(Out::Send(self.state())),
                        Effect::Detect => out.push(Out::Run(Job::Detect(self.w.model.update_from.clone()))),
                        Effect::Install(c) => {
                            if self.smoke {
                                // l'essai : on revient a la page d'avant, rien n'est fait
                                let _ = self.w.on(Event::Finished(Outcome { errors: vec!["smoke test: nothing was installed".into()], ..Default::default() }));
                                out.push(Out::Send(self.state()));
                            } else {
                                self.progress.clear();
                                out.push(Out::Run(Job::Install(c)));
                            }
                        }
                        Effect::Finish { open_studio } => {
                            if open_studio {
                                out.push(Out::Open(Link::Studio));
                            }
                            out.push(Out::Exit(0));
                        }
                    }
                }
                out
            }
            Err(e) => vec![error(&e)],
        }
    }
}

impl View for WizardPage {
    fn page(&self) -> super::ipc::Page {
        super::ipc::Page::Wizard
    }
    fn title(&self) -> &'static str {
        "Uppwind Link Setup"
    }
    fn size(&self) -> (f64, f64) {
        (720.0, 580.0)
    }
    fn on_request(&mut self, r: Request) -> Vec<Out> {
        match r {
            Request::Init => vec![Out::Send(self.state())],
            Request::Install => self.event(Event::Install),
            Request::Customize => self.event(Event::Customize),
            Request::Next => self.event(Event::Next),
            Request::Back => self.event(Event::Back),
            Request::Choose(c) => self.event(Event::Choose(c)),
            Request::Recheck => self.event(Event::Recheck),
            Request::Finish => self.event(Event::Finish),
            Request::Open(l) => vec![Out::Open(l)],
            Request::CopyCode => match &self.code {
                Some(c) => vec![Out::Copy(c.clone()), Out::Send(json!({ "type": "copied" }))],
                None => vec![error("No pairing code yet.")],
            },
            Request::Close => {
                if self.w.step == Step::Progress {
                    vec![error("Wait a few seconds: Uppwind Link is installing.")]
                } else {
                    // fermer avant la fin : rien n'a ete installe (1) ; apres : comme Finish, sans ouvrir le studio
                    vec![Out::Exit(if self.w.step == Step::Done { 0 } else { 1 })]
                }
            }
            _ => vec![],
        }
    }
    fn on_done(&mut self, d: Done) -> Vec<Out> {
        match d {
            Done::Detected(Ok(m)) => self.event(Event::Detected(m)),
            Done::Detected(Err(e)) => {
                self.w.checking = false;
                vec![Out::Send(self.state()), error(&e)]
            }
            Done::Installed(o, code) => {
                self.code = code;
                self.event(Event::Finished(o))
            }
            _ => vec![],
        }
    }
    fn on_progress(&mut self, line: String) -> Vec<Out> {
        self.progress.push(line.clone());
        vec![Out::Send(json!({ "type": "progress", "line": line }))]
    }
}

// ── la fenetre d'etat ──────────────────────────────────────────────────────────────────────────────────

pub struct StatusPage {
    pub model: Option<StatusModel>,
    /// une relecture en route
    pub busy: bool,
}

impl StatusPage {
    pub fn new() -> StatusPage {
        StatusPage { model: None, busy: false }
    }
    fn refresh(&mut self) -> Vec<Out> {
        if self.busy {
            return vec![];
        }
        self.busy = true;
        vec![Out::Run(Job::Status)]
    }
    /// L'horloge de la fenetre (toutes les 3 s tant qu'elle est ouverte).
    pub fn tick(&mut self) -> Vec<Out> {
        self.refresh()
    }
}

impl Default for StatusPage {
    fn default() -> Self {
        Self::new()
    }
}

impl View for StatusPage {
    fn page(&self) -> super::ipc::Page {
        super::ipc::Page::Status
    }
    fn title(&self) -> &'static str {
        "Uppwind Link"
    }
    fn size(&self) -> (f64, f64) {
        (680.0, 620.0)
    }
    fn on_request(&mut self, r: Request) -> Vec<Out> {
        match r {
            Request::Init => {
                let mut out: Vec<Out> = self.model.iter().map(|m| Out::Send(json!({ "type": "status", "status": m }))).collect();
                out.extend(self.refresh());
                out
            }
            Request::Recheck => self.refresh(),
            // VB-CABLE : seulement quand le retour audio a echoue (l'aide ne le montre qu'alors)
            Request::Open(Link::VbCable) if !self.model.as_ref().is_some_and(|m| m.audio.trouble.is_some()) => vec![error("Not now.")],
            Request::Open(l) => vec![Out::Open(l)],
            Request::CopyCode => match self.model.as_ref().map(|m| m.code.clone()).filter(|c| !c.is_empty()) {
                Some(c) => vec![Out::Copy(c), Out::Send(json!({ "type": "copied" }))],
                None => vec![error("No pairing code: Uppwind Link is not running.")],
            },
            Request::Autostart(on) => vec![Out::Run(Job::Autostart(on))],
            Request::SetupAgain => vec![Out::Run(Job::SetupAgain)],
            Request::Close => vec![Out::Close],
            _ => vec![],
        }
    }
    fn on_done(&mut self, d: Done) -> Vec<Out> {
        match d {
            Done::Status(Ok(m)) => {
                self.busy = false;
                let out = vec![Out::Send(json!({ "type": "status", "status": m }))];
                self.model = Some(*m);
                out
            }
            Done::Status(Err(e)) => {
                self.busy = false;
                vec![error(&e)]
            }
            Done::Autostart(Ok(_)) => {
                self.busy = false;
                self.refresh()
            }
            Done::Autostart(Err(e)) | Done::Launched(Err(e)) => vec![error(&e)],
            _ => vec![],
        }
    }
}

// ── la fenetre de desinstallation ──────────────────────────────────────────────────────────────────────

pub struct UninstallPage {
    pub plan: Option<UninstallPlan>,
    pub removal: Option<Removal>,
    pub running: bool,
    smoke: bool,
}

impl UninstallPage {
    pub fn new() -> UninstallPage {
        UninstallPage { plan: None, removal: None, running: false, smoke: false }
    }
    pub fn smoke(mut self) -> UninstallPage {
        self.smoke = true;
        self
    }
    fn state(&self) -> Value {
        json!({ "type": "uninstall", "plan": self.plan, "removal": self.removal, "running": self.running })
    }
}

impl Default for UninstallPage {
    fn default() -> Self {
        Self::new()
    }
}

impl View for UninstallPage {
    fn page(&self) -> super::ipc::Page {
        super::ipc::Page::Uninstall
    }
    fn title(&self) -> &'static str {
        "Uninstall Uppwind Link"
    }
    fn size(&self) -> (f64, f64) {
        (640.0, 540.0)
    }
    fn on_request(&mut self, r: Request) -> Vec<Out> {
        match r {
            Request::Init => vec![Out::Send(self.state()), Out::Run(Job::UninstallPlan)],
            Request::Confirm if self.running || self.removal.as_ref().is_some_and(|x| x.blocked.is_none()) => vec![],
            Request::Confirm if self.smoke => vec![error("Smoke test: nothing is uninstalled.")],
            Request::Confirm => {
                self.running = true;
                self.removal = None;
                vec![Out::Send(self.state()), Out::Run(Job::Uninstall)]
            }
            Request::Cancel | Request::Close if self.running => vec![error("Wait a few seconds: Uppwind Link is being removed.")],
            Request::Cancel | Request::Close => vec![Out::Exit(match &self.removal {
                Some(r) if r.blocked.is_none() && r.ok => 0,
                _ => 1,
            })],
            _ => vec![],
        }
    }
    fn on_done(&mut self, d: Done) -> Vec<Out> {
        match d {
            Done::Plan(Ok(p)) => {
                self.plan = Some(p);
                vec![Out::Send(self.state())]
            }
            Done::Plan(Err(e)) => vec![error(&e)],
            Done::Removed(r) => {
                self.running = false;
                self.removal = Some(r);
                vec![Out::Send(self.state())]
            }
            _ => vec![],
        }
    }
}

// ── les travaux ────────────────────────────────────────────────────────────────────────────────────────

/// Fait un travail (dans un fil) → son resultat ; `progress` : ses lignes, au fil de l'eau.
pub fn run_job(job: Job, progress: &dyn Fn(&str)) -> Done {
    use crate::wizard::{self, arp, flow};
    match job {
        Job::Detect(update_from) => Done::Detected(wizard::detect(update_from)),
        Job::Install(c) => {
            let p = match wizard::parts() {
                Ok(p) => p,
                Err(e) => return Done::Installed(Outcome { errors: vec![e], ..Default::default() }, None),
            };
            let src = match std::env::current_exe() {
                Ok(s) => s,
                Err(e) => return Done::Installed(Outcome { errors: vec![format!("cannot find this program's own file ({e})")], ..Default::default() }, None),
            };
            let reg = arp::ArpRegistry;
            let env = p.env(&reg);
            let hooks = wizard::RealHooks { progress: |l: &str| progress(l), state_file: p.state_file.clone() };
            let o = flow::install(&env, &src, &c, &hooks);
            let code = o.port.and_then(|_| wizard::pairing_code(&p.state_file));
            Done::Installed(o, code)
        }
        Job::Status => Done::Status(crate::tray::status_model().map(Box::new)),
        Job::UninstallPlan => Done::Plan(wizard::parts().map(|p| {
            let reg = arp::ArpRegistry;
            let mut plan = flow::plan_uninstall(&p.env(&reg));
            plan.sandbox = wizard::sandbox().map(|d| d.display().to_string());
            plan
        })),
        Job::Uninstall => match wizard::parts() {
            Ok(p) => {
                let reg = arp::ArpRegistry;
                let env = p.env(&reg);
                let hooks = wizard::RealHooks { progress: |l: &str| progress(l), state_file: p.state_file.clone() };
                // (l'exe qui tourne, mis de cote par install.rs, part apres la sortie : flow.rs le demande)
                Done::Removed(flow::uninstall(&env, &hooks))
            }
            Err(e) => Done::Removed(Removal { errors: vec![e], ..Default::default() }),
        },
        Job::Autostart(on) => Done::Autostart(wizard::parts().and_then(|p| flow::set_autostart(&p.dirs, on, &p.extra_args))),
        Job::SetupAgain => Done::Launched(wizard::parts().and_then(|p| {
            let mut args = vec!["--wizard".to_string()];
            args.extend(p.extra_args.clone());
            let exe = if p.dirs.exe().is_file() { p.dirs.exe() } else { std::env::current_exe().map_err(|e| e.to_string())? };
            wizard::spawn_detached(&exe, &args).map(|_| ())
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wizard::steps::tests::{item, model};

    fn sends(out: &[Out]) -> Vec<&Value> {
        out.iter().filter_map(|o| if let Out::Send(v) = o { Some(v) } else { None }).collect()
    }

    #[test]
    fn the_wizard_page_turns_requests_into_events_and_jobs() {
        let mut p = WizardPage::new(Wizard::new(model(vec![item("fl", "fl", "install", None, 4)], false)));
        let out = p.on_request(Request::Init);
        assert_eq!(sends(&out)[0]["wizard"]["step"], "welcome");
        // Install : les ports manquent, la page des ports
        let out = p.on_request(Request::Install);
        assert_eq!(sends(&out)[0]["wizard"]["step"], "ports");
        let out = p.on_request(Request::Recheck);
        assert!(matches!(out.last(), Some(Out::Run(Job::Detect(None)))), "{out:?}");
        let out = p.on_done(Done::Detected(Ok(model(vec![item("fl", "fl", "install", None, 4)], true))));
        assert_eq!(sends(&out)[0]["wizard"]["model"]["ports"]["link"], "Uppwind Link");
        let out = p.on_request(Request::Next);
        assert!(matches!(out.last(), Some(Out::Run(Job::Install(c))) if c.daws == vec!["fl".to_string()]), "{out:?}");
        // pendant l'installation : la progression ; fermer est refuse
        assert_eq!(sends(&p.on_progress("Copying".into()))[0]["line"], "Copying");
        assert!(matches!(p.on_request(Request::Close)[0], Out::Send(ref v) if v["type"] == "error"));
        assert!(matches!(p.on_request(Request::CopyCode)[0], Out::Send(ref v) if v["type"] == "error"), "no code yet");
        let out = p.on_done(Done::Installed(Outcome { ok: true, port: Some(47410), ..Default::default() }, Some("UWC-47410-abc".into())));
        assert_eq!(sends(&out)[0]["wizard"]["step"], "done");
        assert_eq!(sends(&out)[0]["code"], "UWC-47410-abc");
        assert!(matches!(&p.on_request(Request::CopyCode)[0], Out::Copy(c) if c == "UWC-47410-abc"));
        let out = p.on_request(Request::Finish);
        assert!(matches!(out[0], Out::Open(Link::Studio)) && matches!(out[1], Out::Exit(0)), "{out:?}");
    }

    #[test]
    fn the_smoke_test_installs_nothing() {
        let mut p = WizardPage::new(Wizard::new(model(vec![], true))).smoke();
        p.on_request(Request::Customize);
        p.on_request(Request::Next);
        p.on_request(Request::Next);
        let out = p.on_request(Request::Next);
        assert!(!out.iter().any(|o| matches!(o, Out::Run(Job::Install(_)))), "{out:?}");
        assert_eq!(p.w.step, Step::Done);
        let mut u = UninstallPage::new().smoke();
        assert!(!u.on_request(Request::Confirm).iter().any(|o| matches!(o, Out::Run(Job::Uninstall))));
    }

    #[test]
    fn the_status_page_copies_only_a_real_code_and_vbcable_only_after_a_failure() {
        let mut s = StatusPage::new();
        assert!(matches!(s.on_request(Request::Init).last(), Some(Out::Run(Job::Status))));
        assert!(s.on_request(Request::Recheck).is_empty(), "one refresh at a time");
        assert!(matches!(s.on_request(Request::CopyCode)[0], Out::Send(ref v) if v["type"] == "error"));
        let probe = crate::setup::env::Probe::empty(crate::setup::env::Paths::in_root(std::path::Path::new("Z:\\nowhere")));
        let r = crate::tray::status::report(&probe);
        let m = crate::tray::status::model(&r, 47410, "abc", Default::default(), Ok(1.0), true, None);
        let out = s.on_done(Done::Status(Ok(Box::new(m.clone()))));
        assert_eq!(sends(&out)[0]["status"]["code"], "UWC-47410-abc");
        assert!(matches!(&s.on_request(Request::CopyCode)[0], Out::Copy(c) if c == "UWC-47410-abc"));
        assert!(matches!(s.on_request(Request::Open(Link::VbCable))[0], Out::Send(ref v) if v["type"] == "error"), "no failure: no VB-CABLE");
        let mut bad = m;
        bad.audio.trouble = Some("cannot capture".into());
        s.on_done(Done::Status(Ok(Box::new(bad))));
        assert!(matches!(s.on_request(Request::Open(Link::VbCable))[0], Out::Open(Link::VbCable)));
        assert!(matches!(s.on_request(Request::Autostart(false))[0], Out::Run(Job::Autostart(false))));
        assert!(matches!(s.on_request(Request::Close)[0], Out::Close));
    }

    #[test]
    fn the_uninstall_page_confirms_once_and_waits() {
        let mut u = UninstallPage::new();
        assert!(matches!(u.on_request(Request::Init)[1], Out::Run(Job::UninstallPlan)));
        let out = u.on_request(Request::Confirm);
        assert!(matches!(out[1], Out::Run(Job::Uninstall)));
        assert!(u.on_request(Request::Confirm).is_empty(), "not twice");
        assert!(matches!(u.on_request(Request::Cancel)[0], Out::Send(ref v) if v["type"] == "error"), "not while it runs");
        u.on_done(Done::Removed(Removal { blocked: Some("FL Studio is running".into()), ..Default::default() }));
        assert!(matches!(u.on_request(Request::Confirm)[1], Out::Run(Job::Uninstall)), "try again after closing FL");
        u.on_done(Done::Removed(Removal { ok: true, ..Default::default() }));
        assert!(u.on_request(Request::Confirm).is_empty());
        assert!(matches!(u.on_request(Request::Close)[0], Out::Exit(0)));
    }
}
