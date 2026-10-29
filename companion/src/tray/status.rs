// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Ce que la fenetre d'etat dit : le compagnon (son port), le studio (appaire ou non, le code), Live et FL (les versions
//! des scripts, par `--setup`), les ports MIDI, le retour audio, et les lignes d'aide.
//!
//! session.rs previent ce module a trois endroits (un studio appaire, une connexion fermee, une reponse « audio ») :
//! l'etat vit ici, en memoire, rien n'est ecrit sur le disque.

use crate::setup::scripts::Daw;
use crate::setup::{self, Report};
use crate::wizard::steps::{ports_of, Ports};
use serde::Serialize;
use std::sync::Mutex;

/// L'etat vivant (les connexions du studio, la derniere panne du retour audio).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Live {
    /// des studios appaires connectes en ce moment
    pub sessions: u32,
    /// un studio s'est appaire depuis le lancement
    pub paired_ever: bool,
    pub last_origin: Option<String>,
    /// secondes depuis 1970
    pub last_paired: Option<u64>,
    /// la derniere erreur de capture du retour audio (pas « aucun DAW ouvert »)
    pub audio_trouble: Option<String>,
}

static LIVE: Mutex<Live> = Mutex::new(Live { sessions: 0, paired_ever: false, last_origin: None, last_paired: None, audio_trouble: None });

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn origin_text(o: &str) -> String {
    o.chars().filter(|c| !c.is_control()).take(120).collect()
}

/// session.rs : un studio vient de s'appairer (le bon jeton).
pub fn paired(origin: &str) {
    if let Ok(mut l) = LIVE.lock() {
        l.sessions += 1;
        l.paired_ever = true;
        l.last_origin = Some(origin_text(origin));
        l.last_paired = Some(now());
    }
}

/// session.rs : la connexion d'un studio appaire s'est fermee.
pub fn closed(_origin: &str) {
    if let Ok(mut l) = LIVE.lock() {
        l.sessions = l.sessions.saturating_sub(1);
    }
}

/// Une erreur du retour audio qui dit que la capture ne marche pas ici (et pas : aucun DAW ouvert, une demande fausse).
pub fn is_capture_trouble(error: &str) -> bool {
    let e = error.to_ascii_lowercase();
    const NOT_TROUBLE: &[&str] = &["not a daw", "no daw", "no process", "test tone", "rate must", "format is", "audio action", "stop"];
    !e.trim().is_empty() && !NOT_TROUBLE.iter().any(|n| e.contains(n))
}

/// Une reponse « audio » sur l'etat : « on » efface la panne, une erreur de capture la note.
fn apply_reply(l: &mut Live, reply: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(reply) else { return };
    if v["state"] == "on" {
        l.audio_trouble = None;
    } else if v["state"] == "off" {
        if let Some(e) = v["error"].as_str().filter(|e| is_capture_trouble(e)) {
            l.audio_trouble = Some(e.chars().filter(|c| !c.is_control()).take(300).collect());
        }
    }
}

/// session.rs : la reponse a une demande « audio » du studio, ou la fin d'un flux.
pub fn audio_reply(reply: &str) {
    if let Ok(mut l) = LIVE.lock() {
        apply_reply(&mut l, reply);
    }
}

pub fn live() -> Live {
    LIVE.lock().map(|l| l.clone()).unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DawLine {
    pub daw: &'static str,
    pub name: String,
    pub versions: Vec<String>,
    pub running: Option<String>,
    /// les dossiers de script : « 1.2.0 up to date », « not installed »…
    pub scripts: Vec<ScriptLine>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScriptLine {
    pub folder: String,
    /// missing, up-to-date, outdated, different, newer, no-library
    pub status: String,
    pub installed: Option<String>,
    pub bundled: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Audio {
    /// la capture d'un processus marche sur ce Windows
    pub available: bool,
    pub error: Option<String>,
    /// le retour audio a echoue (la capture) : la fenetre propose alors le cable virtuel, en secours
    pub trouble: Option<String>,
}

/// Le modele de la fenetre d'etat.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatusModel {
    pub version: String,
    pub sandbox: Option<String>,
    /// le serveur tourne : son port (0 : pas lance, l'essai de la fenetre)
    pub port: u16,
    /// (29 oct.) UWC2-<port>-<jeton> : montre dans la fenetre et copie par le bouton, jamais dans une adresse
    pub code: String,
    pub live: Live,
    pub daws: Vec<DawLine>,
    pub ports: Ports,
    pub audio: Audio,
    pub autostart: bool,
}

/// Les DAW et leurs scripts, d'un rapport de `--setup` (une simulation).
pub fn daws_of(r: &Report) -> Vec<DawLine> {
    let mut v = Vec::new();
    let line = |d: Daw| -> Vec<ScriptLine> {
        r.scripts
            .iter()
            .filter(|t| t.daw == d)
            .map(|t| ScriptLine {
                folder: t.dir.display().to_string(),
                status: serde_json::to_value(t.status).ok().and_then(|x| x.as_str().map(String::from)).unwrap_or_default(),
                installed: t.installed_version.clone(),
                bundled: t.bundled_version.clone(),
            })
            .collect()
    };
    if r.ableton.found() {
        v.push(DawLine {
            daw: "ableton",
            name: r.ableton.programs.first().map(|p| p.name.clone()).unwrap_or_else(|| "Ableton Live".into()),
            versions: r.ableton.prefs.iter().map(|p| p.version.clone()).collect(),
            running: r.ableton.running.clone(),
            scripts: line(Daw::Ableton),
        });
    }
    if r.fl.found() {
        v.push(DawLine {
            daw: "fl",
            name: r.fl.programs.first().map(|p| p.name.clone()).unwrap_or_else(|| "FL Studio".into()),
            versions: r.fl.programs.iter().map(|p| p.version.clone()).filter(|x| !x.is_empty()).collect(),
            running: r.fl.running.clone(),
            scripts: line(Daw::Fl),
        });
    }
    v
}

/// Le modele complet : le rapport (une simulation de `--setup`), l'etat vivant, le retour audio.
pub fn model(r: &Report, port: u16, token: &str, live: Live, audio_available: Result<f64, String>, autostart: bool, sandbox: Option<String>) -> StatusModel {
    let (link, back, near) = ports_of(&r.midi.inputs, &r.midi.outputs);
    StatusModel {
        version: env!("CARGO_PKG_VERSION").into(),
        sandbox,
        port,
        code: if port > 0 && !token.is_empty() { crate::security::pairing_code(port, token) } else { String::new() },
        live,
        daws: daws_of(r),
        ports: Ports { link, back, near, loopmidi_installed: r.midi.loopmidi.is_some(), loopmidi_running: r.midi.loopmidi_running },
        audio: Audio { available: audio_available.is_ok(), error: audio_available.err(), trouble: None },
        autostart,
    }
    .with_trouble()
}

impl StatusModel {
    fn with_trouble(mut self) -> StatusModel {
        self.audio.trouble = self.live.audio_trouble.clone().or_else(|| self.audio.error.as_ref().map(|e| format!("this Windows cannot capture a single app: {e}")));
        self
    }
}

/// Le rapport de `--setup` pour la fenetre d'etat (rien n'est ecrit).
pub fn report(probe: &setup::env::Probe) -> Report {
    setup::gather(probe, &setup::Opts::default(), &setup::scripts::now_stamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_trouble_is_not_a_missing_daw() {
        assert!(is_capture_trouble("this Windows cannot capture a single app: E_NOTIMPL"));
        assert!(is_capture_trouble("ActivateAudioInterfaceAsync failed (0x80070005)"));
        assert!(!is_capture_trouble("no DAW matching \"auto\""));
        assert!(!is_capture_trouble("explorer.exe is not a DAW (Ableton Live or FL Studio)"));
        assert!(!is_capture_trouble("the test tone is not running (start the companion with --test-tone)"));
        assert!(!is_capture_trouble(""));
    }

    #[test]
    fn the_live_state_follows_the_sessions_and_the_audio_replies() {
        // l'etat global : les essais de session.rs s'y appairent aussi, en parallele ; on ne verifie que ce qu'ils ne
        // peuvent pas defaire
        paired("https://uppwind.lucasmoreau.fr");
        let l = live();
        assert!(l.sessions >= 1 && l.paired_ever && l.last_paired.is_some());
        closed("x");
        let mut l = Live::default();
        apply_reply(&mut l, r#"{"type":"audio","state":"off","error":"no DAW matching \"auto\""}"#);
        assert_eq!(l.audio_trouble, None, "no DAW open is not a capture failure");
        apply_reply(&mut l, r#"{"type":"audio","state":"off","error":"cannot capture Ableton Live 12 Suite.exe (0x88890008)"}"#);
        assert_eq!(l.audio_trouble.as_deref(), Some("cannot capture Ableton Live 12 Suite.exe (0x88890008)"));
        apply_reply(&mut l, r#"{"type":"audio","state":"off","reason":"stop"}"#);
        assert!(l.audio_trouble.is_some(), "a stop does not clear it");
        apply_reply(&mut l, r#"{"type":"audio","state":"on","pid":1}"#);
        assert_eq!(l.audio_trouble, None, "it works again");
        apply_reply(&mut l, "not json");
    }

    #[test]
    fn the_model_never_puts_the_code_without_a_server() {
        let probe = crate::setup::env::Probe::empty(crate::setup::env::Paths::in_root(std::path::Path::new("Z:\\nowhere")));
        let r = report(&probe);
        let m = model(&r, 0, "abc", Live::default(), Ok(10.0), false, None);
        assert_eq!(m.code, "", "no server, no code");
        let m = model(&r, 47410, "abc", Live::default(), Err("build 18363".into()), true, None);
        assert_eq!(m.code, "UWC2-47410-abc");
        assert!(m.audio.trouble.as_deref().is_some_and(|t| t.contains("cannot capture")), "{:?}", m.audio);
        assert!(m.daws.is_empty() && !m.ports.ok());
    }
}
