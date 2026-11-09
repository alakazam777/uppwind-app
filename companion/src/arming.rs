// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (9 nov. 2026, audit de securite F1) L'armement : la capture des fenetres de plugins (« watch »), les gestes
//! (« input ») et le son du DAW (« audio ») ne partent que si la personne devant l'ordinateur l'a permis, dans le
//! compagnon lui-meme : la fenetre d'etat, le menu de l'icone, ou « arm » dans la console. Le studio ne peut pas armer
//! (aucun message du protocole ne le fait) : un script qui aurait le jeton (une faille du site, une autre page de la
//! meme origine) ne filme ni ne pilote rien sans un geste sur la machine.
//!
//! - Desarme a chaque lancement, sauf --demo (la fenetre de demonstration n'existe pas : rien n'est filme ni injecte).
//! - Desarme tout seul apres IDLE_DISARM sans aucun studio connecte : l'onglet ferme, la soiree finie, rien ne reste
//!   arme jusqu'au lendemain.
//! - Chaque changement fait avancer `generation` : chaque connexion le voit (session.rs) et, desarmee, arrete tout de
//!   suite sa capture, ses gestes et son son.
//! - Le reste du protocole ne change pas (la liste des fenetres, Open/Close dans Live, les notes vers les plugins
//!   Uppwind Link, le lien du script de Live) : rien de cela ne filme l'ecran ni ne pilote le clavier et la souris.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Sans aucun studio connecte pendant ce temps, l'armement retombe.
pub const IDLE_DISARM: Duration = Duration::from_secs(10 * 60);

/// Le code des refus (« unwatched », « audio ») : le studio le reconnait.
pub const CODE: &str = "disarmed";

/// Ce qu'un studio lit quand le compagnon est desarme : le studio d'avant le montre tel quel.
#[cfg(not(target_os = "macos"))]
pub const REFUSAL: &str = "remote control is off in Uppwind Link: allow it there (its window or its icon)";
/// (9 nov.) Sur un Mac, ni fenetre d'etat ni icone : la console du Terminal ou il tourne.
#[cfg(target_os = "macos")]
pub const REFUSAL: &str = "remote control is off in Uppwind Link: type arm in the Terminal where it runs";

#[derive(Debug)]
struct State {
    armed: bool,
    /// des studios appaires connectes en ce moment
    sessions: u32,
    /// depuis quand aucun studio n'est connecte (None : un studio est la)
    idle_since: Option<Instant>,
    /// la derniere demande refusee parce que desarme (la fenetre d'etat la montre)
    asked: Option<Instant>,
}

pub struct Arming {
    state: Mutex<State>,
    generation: AtomicU64,
    /// le desarmement d'inactivite (sans objet en --demo)
    auto: bool,
}

impl Arming {
    /// `armed` : l'etat au lancement (vrai seulement en --demo) ; `auto` : desarmer apres IDLE_DISARM sans studio.
    pub fn new(armed: bool, auto: bool) -> Arming {
        Arming { state: Mutex::new(State { armed, sessions: 0, idle_since: Some(Instant::now()), asked: None }), generation: AtomicU64::new(0), auto }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Le desarmement d'inactivite, s'il est l'heure (sous le verrou).
    fn expire(&self, s: &mut State, now: Instant) {
        if !(self.auto && s.armed && s.sessions == 0) {
            return;
        }
        if let Some(t) = s.idle_since {
            if now.saturating_duration_since(t) >= IDLE_DISARM {
                s.armed = false;
                self.generation.fetch_add(1, Ordering::SeqCst);
                crate::log(&format!("remote control: disarmed (no studio connected for {} minutes)", IDLE_DISARM.as_secs() / 60));
            }
        }
    }

    /// Arme maintenant ?
    pub fn armed(&self) -> bool {
        self.armed_at(Instant::now())
    }

    pub fn armed_at(&self, now: Instant) -> bool {
        let mut s = self.lock();
        self.expire(&mut s, now);
        s.armed
    }

    /// La personne devant l'ordinateur arme ou desarme (`by` : d'ou, pour le journal). → true si l'etat a change.
    pub fn set(&self, on: bool, by: &str) -> bool {
        self.set_at(on, by, Instant::now())
    }

    pub fn set_at(&self, on: bool, by: &str, now: Instant) -> bool {
        let mut s = self.lock();
        self.expire(&mut s, now);
        if s.armed == on {
            return false;
        }
        s.armed = on;
        if on {
            s.asked = None;
            if s.sessions == 0 {
                // le compte des minutes sans studio repart de l'armement
                s.idle_since = Some(now);
            }
        }
        self.generation.fetch_add(1, Ordering::SeqCst);
        if on {
            crate::log(&format!("remote control: ARMED ({by}): the studio may capture plugin windows, send gestures to them and take the DAW's sound"));
        } else {
            crate::log(&format!("remote control: disarmed ({by})"));
        }
        true
    }

    /// Change a chaque armement ou desarmement (les connexions le suivent).
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// session.rs : un studio vient de prouver le code.
    pub fn session_opened(&self) {
        self.session_opened_at(Instant::now());
    }

    pub fn session_opened_at(&self, now: Instant) {
        let mut s = self.lock();
        // l'inactivite passee compte avant que ce studio n'arrive
        self.expire(&mut s, now);
        s.sessions += 1;
        s.idle_since = None;
    }

    /// session.rs : la connexion d'un studio appaire s'est fermee.
    pub fn session_closed(&self) {
        self.session_closed_at(Instant::now());
    }

    pub fn session_closed_at(&self, now: Instant) {
        let mut s = self.lock();
        s.sessions = s.sessions.saturating_sub(1);
        if s.sessions == 0 {
            s.idle_since = Some(now);
        }
    }

    /// session.rs : une capture, un geste ou le son demandes pendant que c'est desarme (la fenetre d'etat le dit).
    pub fn note_refused(&self) {
        self.lock().asked = Some(Instant::now());
    }

    /// Depuis combien de temps un studio a demande en vain (None : rien depuis le dernier armement).
    pub fn asked_ago(&self) -> Option<Duration> {
        self.lock().asked.map(|t| t.elapsed())
    }

    /// Les minutes avant le desarmement d'inactivite (None : un studio est connecte, desarme, ou --demo).
    pub fn idle_left(&self) -> Option<Duration> {
        let mut s = self.lock();
        let now = Instant::now();
        self.expire(&mut s, now);
        if !(self.auto && s.armed && s.sessions == 0) {
            return None;
        }
        s.idle_since.map(|t| IDLE_DISARM.saturating_sub(now.saturating_duration_since(t)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn it_starts_disarmed_and_only_the_person_arms_it() {
        let a = Arming::new(false, true);
        assert!(!a.armed());
        let g = a.generation();
        assert!(a.set(true, "test"));
        assert!(a.armed());
        assert!(a.generation() > g, "every change is seen by the connections");
        assert!(!a.set(true, "test"), "no change, no new generation");
        assert!(a.set(false, "test"));
        assert!(!a.armed());
    }

    #[test]
    fn it_disarms_itself_after_ten_minutes_without_a_studio() {
        let a = Arming::new(false, true);
        let t0 = Instant::now();
        a.set_at(true, "test", t0);
        assert!(a.armed_at(t0 + 9 * MIN), "not yet");
        let g = a.generation();
        assert!(!a.armed_at(t0 + IDLE_DISARM), "ten minutes without a studio");
        assert!(a.generation() > g, "the disarming is seen like any other");
    }

    #[test]
    fn a_connected_studio_keeps_it_armed_and_the_count_starts_when_it_leaves() {
        let a = Arming::new(false, true);
        let t0 = Instant::now();
        a.set_at(true, "test", t0);
        a.session_opened_at(t0 + MIN);
        assert!(a.armed_at(t0 + 60 * MIN), "a studio connected for an hour: still armed");
        a.session_closed_at(t0 + 60 * MIN);
        assert!(a.armed_at(t0 + 69 * MIN));
        assert!(!a.armed_at(t0 + 70 * MIN), "ten minutes after the studio left");
    }

    #[test]
    fn a_studio_that_comes_back_after_the_idle_time_finds_it_disarmed() {
        let a = Arming::new(false, true);
        let t0 = Instant::now();
        a.set_at(true, "test", t0);
        a.session_opened_at(t0 + 11 * MIN);
        assert!(!a.armed_at(t0 + 11 * MIN), "the idle time before it came counts");
    }

    #[test]
    fn the_demo_stays_armed() {
        let a = Arming::new(true, false);
        let t0 = Instant::now();
        assert!(a.armed_at(t0 + 24 * 60 * MIN));
        assert_eq!(a.idle_left(), None);
    }

    #[test]
    fn a_refused_request_is_remembered_until_the_next_arming() {
        let a = Arming::new(false, true);
        assert_eq!(a.asked_ago(), None);
        a.note_refused();
        assert!(a.asked_ago().is_some());
        a.set(true, "test");
        assert_eq!(a.asked_ago(), None);
        assert!(a.idle_left().is_some_and(|d| d <= IDLE_DISARM && d > IDLE_DISARM - MIN));
    }
}
