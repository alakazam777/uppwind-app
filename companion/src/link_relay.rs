// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (25 oct., sans loopMIDI) Le lien du studio avec le Remote Script de Live, par le canal local du script (live.rs) au
//! lieu des ports loopMIDI. Les memes messages : des morceaux SysEx du protocole Uppwind Link (HELLO, LIST, BIND, SET,
//! TEMPO, CHANGED…), en hexadecimal dans du JSON, que le compagnon ne lit pas (il verifie seulement que ce sont des
//! morceaux du lien : F0 7D 55 57 … F7, 212 octets au plus).
//!
//!   studio --{"type":"link","sysex":[…]}--> compagnon --UDP {"cmd":"LINK","in":[…],"token"}--> script
//!   studio <--{"type":"link","sysex":[…]}-- compagnon <--{"ok":true,"out":[…],"more"}-------- script
//!
//! Le script ne peut rien envoyer de lui-meme sur UDP sans adresse : un releve (une requete LINK vide) attend toujours
//! chez lui ce qu'il aura a dire (CHANGED, TEMPO_CHANGED, un BIND_ACK qui attendait Live).
//!
//! (26 oct.) LA LATENCE. Avant : une seule requete a la fois, et le script ne lit son canal que dans update_display
//! (toutes les ~100 ms, mesure dans Live 12) : une commande attendait la reponse au releve en cours (jusqu'a 100 ms),
//! puis le update_display suivant (100 ms de plus) ; d'ou les ~180 ms de PING. Maintenant : un socket qui reste ouvert,
//! les commandes du studio partent TOUT DE SUITE, a cote du releve en cours (plusieurs requetes en route, chacune avec
//! son numero : un fil a part lit les reponses) ; un script 1.3.1 (« link » 2) lit son canal toutes les 5 ms
//! (Live.Base.Timer) et garde le releve vide (« wait ») jusqu'a ce qu'il ait quelque chose : plus de releves a vide en
//! boucle. Un script 1.3.0 (« link » 1) : un releve au plus toutes les POLL, comme avant.
//! Live absent : « link down » une fois, puis un essai toutes les DOWN_RETRY ; de retour : « link up ».

use crate::protocol::{link_chunk_ok, MAX_LINK_CHUNKS};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const POLL: Duration = Duration::from_millis(40);
pub const DOWN_RETRY: Duration = Duration::from_millis(1000);
pub const REQUEST_TIMEOUT: Duration = Duration::from_millis(1500);
/// (26 oct.) un script « link » 2 garde un releve vide au plus ca (il repart aussitot qu'il a quelque chose).
pub const WAIT_MS: u64 = 400;
/// (26 oct.) des requetes du studio en route en meme temps, au plus (le script en lit 16 par lecture).
pub const MAX_IN_FLIGHT: usize = 8;
/// Une requete LINK reste sous MAX_IN (8192 octets) du canal du script : le jeton, l'id et le JSON compris.
pub const MAX_REQUEST_HEX: usize = 6800;
/// Ce que le studio a envoye et qui n'est pas encore parti (Live tres lent, ou parti) : au-dela, on le jette.
pub const MAX_QUEUED: usize = 1024;

/// Ce que le fil dit a la connexion du studio.
#[derive(Debug, PartialEq)]
pub enum LinkOut {
    Chunks(Vec<String>),
    Up,
    Down(String),
}

/// Les fils du relais en vie, toutes connexions comprises. Un relais arrete (« stop ») finit son echange en cours
/// (jusqu'a REQUEST_TIMEOUT) : sans plafond, une page qui alterne « link » et « stop » ferait pousser les fils.
pub const MAX_RELAYS: usize = 32;
static RELAYS: AtomicUsize = AtomicUsize::new(0);

struct Alive(&'static AtomicUsize);
impl Drop for Alive {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Ce que recoit le fil qui envoie : des morceaux du studio, ou « une reponse est arrivee » (le fil qui lit).
enum In {
    Studio(Vec<String>),
    Answered,
}

pub struct Relay {
    tx: Sender<In>,
    rx: Receiver<LinkOut>,
    stop: Arc<AtomicBool>,
}

impl Relay {
    /// Le fil, sur le fichier d'etat du script (None : celui de live.rs). Err : trop de relais en vie.
    pub fn start(state: Option<PathBuf>) -> Result<Relay, String> {
        Self::start_counted(state, &RELAYS, MAX_RELAYS)
    }

    fn start_counted(state: Option<PathBuf>, count: &'static AtomicUsize, max: usize) -> Result<Relay, String> {
        if count.fetch_add(1, Ordering::SeqCst) >= max {
            count.fetch_sub(1, Ordering::SeqCst);
            return Err("too many link relays: wait a moment".to_string());
        }
        let alive = Alive(count);
        let (tx, in_rx) = mpsc::channel::<In>();
        let (out_tx, rx) = mpsc::channel::<LinkOut>();
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let wake = tx.clone();
        std::thread::Builder::new()
            .name("uwc-link".into())
            .spawn(move || {
                let _alive = alive;
                run(state, in_rx, wake, out_tx, st)
            })
            .map_err(|e| format!("link relay: {e}"))?;
        Ok(Relay { tx, rx, stop })
    }

    /// Des morceaux du studio → le script. Err : un morceau qui n'est pas du lien, ou trop d'un coup.
    pub fn send(&self, chunks: Vec<String>) -> Result<(), String> {
        if chunks.len() > MAX_LINK_CHUNKS {
            return Err(format!("at most {MAX_LINK_CHUNKS} chunks per link message"));
        }
        if let Some(bad) = chunks.iter().position(|h| !link_chunk_ok(h)) {
            return Err(format!("chunk {bad} is not an Uppwind Link SysEx chunk"));
        }
        if chunks.is_empty() {
            return Ok(());
        }
        self.tx.send(In::Studio(chunks)).map_err(|_| "the link relay stopped".to_string())
    }

    /// Ce que le fil a pour le studio, sans attendre.
    pub fn poll(&self) -> Vec<LinkOut> {
        let mut v = Vec::new();
        while let Ok(o) = self.rx.try_recv() {
            v.push(o);
        }
        v
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

struct Pending {
    deadline: Instant,
    idle: bool,
}

/// Ce que partagent le fil qui envoie et celui qui lit.
#[derive(Default)]
struct Shared {
    target: Option<SocketAddr>,
    level: u64,
    pending: HashMap<u64, Pending>,
    /// un releve est en route
    idle_busy: bool,
    /// le prochain releve, pas avant (un script « link » 1 : POLL apres la reponse)
    idle_next: Option<Instant>,
    /// le script a encore des morceaux (« more ») : un releve tout de suite
    more: bool,
    up: Option<bool>,
    /// une reponse « ok: false » (le script refuse le lien)
    refused: Option<String>,
}

fn lock(s: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    s.lock().unwrap_or_else(|p| p.into_inner())
}

fn run(state: Option<PathBuf>, rx: Receiver<In>, wake: Sender<In>, tx: Sender<LinkOut>, stop: Arc<AtomicBool>) {
    let path = state.unwrap_or_else(crate::live::state_file);
    let sock = match UdpSocket::bind("127.0.0.1:0") {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(LinkOut::Down(format!("no local socket for the link ({e})")));
            return;
        }
    };
    let _ = sock.set_read_timeout(Some(Duration::from_millis(100)));
    let shared = Arc::new(Mutex::new(Shared::default()));
    if let Ok(reader) = sock.try_clone() {
        let (sh, out, st) = (shared.clone(), tx.clone(), stop.clone());
        let _ = std::thread::Builder::new().name("uwc-link-rx".into()).spawn(move || read_answers(reader, sh, out, wake, st));
    }
    let mut token = String::new();
    let mut port = 0u16;
    let mut queue: Vec<String> = Vec::new();
    let mut dropped = 0u64;
    let mut retry_at = Instant::now();
    let mut buf_wait;
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        // attendre : le studio parle, une reponse arrive, l'heure d'un releve ou d'un delai
        let now = Instant::now();
        {
            let s = lock(&shared);
            let cmds = s.pending.values().filter(|p| !p.idle).count();
            buf_wait = if !queue.is_empty() && cmds < MAX_IN_FLIGHT {
                Duration::ZERO
            } else {
                let mut w = Duration::from_millis(50);
                if s.target.is_none() {
                    w = w.min(retry_at.saturating_duration_since(now));
                } else if !s.idle_busy {
                    w = if s.more { Duration::ZERO } else { w.min(s.idle_next.map(|t| t.saturating_duration_since(now)).unwrap_or(Duration::ZERO)) };
                }
                if let Some(d) = s.pending.values().map(|p| p.deadline).min() {
                    w = w.min(d.saturating_duration_since(now));
                }
                w
            };
        }
        match rx.recv_timeout(buf_wait) {
            Ok(In::Studio(c)) => queue.extend(c),
            Ok(In::Answered) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        while let Ok(m) = rx.try_recv() {
            if let In::Studio(c) = m {
                queue.extend(c);
            }
        }
        if queue.len() > MAX_QUEUED {
            dropped += queue.len() as u64;
            queue.clear();
            crate::log(&format!("link relay: Live's script does not read the link ({dropped} chunks dropped)"));
        }
        let now = Instant::now();
        // une requete sans reponse a temps, ou refusee : Live n'est plus la (ce qui n'est pas parti est perdu : le studio
        // le sait, « down », et refera sa session)
        let failed = {
            let mut s = lock(&shared);
            let late = s.pending.values().any(|p| p.deadline <= now);
            let why = s.refused.take().or_else(|| {
                late.then(|| format!("Live did not answer on udp://127.0.0.1:{port} within {:.1} s (Live closed?)", REQUEST_TIMEOUT.as_secs_f64()))
            });
            if why.is_some() {
                s.pending.clear();
                s.idle_busy = false;
                s.more = false;
                s.target = None;
            }
            why.map(|e| {
                let first = s.up != Some(false);
                s.up = Some(false);
                (e, first)
            })
        };
        if let Some((e, first)) = failed {
            queue.clear();
            retry_at = now + DOWN_RETRY;
            if first {
                crate::log(&format!("link relay: Live's script does not answer ({e})"));
                if tx.send(LinkOut::Down(e)).is_err() {
                    return;
                }
            }
            continue;
        }
        // le script : son port et son jeton (relus apres une panne : Live a pu redemarrer)
        if lock(&shared).target.is_none() {
            if now < retry_at && queue.is_empty() {
                continue;
            }
            match crate::live::link_target_at(&path) {
                Ok((p, t, level)) => {
                    port = p;
                    token = t;
                    let mut s = lock(&shared);
                    s.target = Some(SocketAddr::from(([127, 0, 0, 1], p)));
                    s.level = level;
                    s.idle_next = None;
                }
                Err(e) => {
                    queue.clear();
                    retry_at = now + DOWN_RETRY;
                    let first = {
                        let mut s = lock(&shared);
                        let f = s.up != Some(false);
                        s.up = Some(false);
                        f
                    };
                    if first {
                        crate::log(&format!("link relay: Live's script does not answer ({e})"));
                        if tx.send(LinkOut::Down(e)).is_err() {
                            return;
                        }
                    }
                    continue;
                }
            }
        }
        let target = SocketAddr::from(([127, 0, 0, 1], port));
        // les commandes du studio : tout de suite, sans attendre le releve en route
        loop {
            let cmds = lock(&shared).pending.values().filter(|p| !p.idle).count();
            if queue.is_empty() || cmds >= MAX_IN_FLIGHT {
                break;
            }
            let mut take = 0;
            let mut size = 0;
            while take < queue.len() && take < MAX_LINK_CHUNKS && size + queue[take].len() <= MAX_REQUEST_HEX {
                size += queue[take].len();
                take += 1;
            }
            let batch: Vec<String> = queue.drain(..take.max(1)).collect();
            let id = crate::live::new_request_id();
            lock(&shared).pending.insert(id, Pending { deadline: now + REQUEST_TIMEOUT, idle: false });
            let msg = json!({ "cmd": "LINK", "in": batch, "token": token, "id": id });
            let _ = sock.send_to(msg.to_string().as_bytes(), target);
        }
        // le releve : un seul en route ; il attend chez un script « link » 2
        let idle = {
            let mut s = lock(&shared);
            let due = !s.idle_busy && (s.more || s.idle_next.map(|t| now >= t).unwrap_or(true));
            if due {
                let wait = s.level >= 2 && s.up == Some(true);
                let id = crate::live::new_request_id();
                let extra = if wait { Duration::from_millis(WAIT_MS) } else { Duration::ZERO };
                s.pending.insert(id, Pending { deadline: now + REQUEST_TIMEOUT + extra, idle: true });
                s.idle_busy = true;
                s.more = false;
                Some(if wait {
                    json!({ "cmd": "LINK", "in": [], "wait": WAIT_MS, "token": token, "id": id })
                } else {
                    json!({ "cmd": "LINK", "in": [], "token": token, "id": id })
                })
            } else {
                None
            }
        };
        if let Some(m) = idle {
            let _ = sock.send_to(m.to_string().as_bytes(), target);
        }
    }
}

/// Le fil qui lit : chaque reponse du script (par son numero) → le studio, et reveille le fil qui envoie.
fn read_answers(sock: UdpSocket, shared: Arc<Mutex<Shared>>, tx: Sender<LinkOut>, wake: Sender<In>, stop: Arc<AtomicBool>) {
    let mut buf = vec![0u8; 65536];
    while !stop.load(Ordering::SeqCst) {
        let Ok((n, from)) = sock.recv_from(&mut buf) else { continue }; // le delai (ou 10054 sous Windows)
        let Ok(rep) = serde_json::from_slice::<Value>(&buf[..n]) else { continue };
        let Some(id) = rep["id"].as_u64() else { continue };
        let mut s = lock(&shared);
        if s.target != Some(from) {
            continue;
        }
        let Some(p) = s.pending.remove(&id) else { continue };
        if p.idle {
            s.idle_busy = false;
            s.idle_next = Some(Instant::now() + if s.level >= 2 { Duration::ZERO } else { POLL });
        }
        if rep["ok"].as_bool() != Some(true) {
            s.refused = Some(rep["error"].as_str().unwrap_or("Live's script refused the link").chars().take(200).collect());
            drop(s);
            let _ = wake.send(In::Answered);
            continue;
        }
        let out: Vec<String> = rep["out"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).filter(|h| link_chunk_ok(h)).map(|h| h.to_ascii_lowercase()).collect())
            .unwrap_or_default();
        if rep["more"].as_bool() == Some(true) {
            s.more = true;
        }
        let first = s.up != Some(true);
        s.up = Some(true);
        // envoye sous le verrou : l'ordre des reponses est celui de leur arrivee
        if first && tx.send(LinkOut::Up).is_err() {
            return;
        }
        if !out.is_empty() && tx.send(LinkOut::Chunks(out)).is_err() {
            return;
        }
        drop(s);
        let _ = wake.send(In::Answered);
    }
}

// Le message pour le studio.
pub fn studio_json(o: &LinkOut) -> String {
    match o {
        LinkOut::Chunks(c) => json!({ "type": "link", "sysex": c }).to_string(),
        LinkOut::Up => json!({ "type": "link", "state": "up" }).to_string(),
        LinkOut::Down(e) => json!({ "type": "link", "state": "down", "error": e }).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::net::UdpSocket;
    use std::time::Instant;

    fn temp_state(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("uwc-link-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(crate::live::STATE_NAME)
    }

    const HELLO: &str = "f07d555701010100017b7d7b7df7";

    /// Un faux script : il lit le jeton, garde les morceaux recus, repond « out » (une boite qu'on remplit).
    fn fake_script(path: &PathBuf, answers: usize, outbox: Vec<(Vec<String>, bool)>) -> std::thread::JoinHandle<Vec<Vec<String>>> {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = sock.local_addr().unwrap().port();
        std::fs::write(path, json!({"port": port, "token": "t0k", "script": "UppwindLink", "version": "1.3.0", "link": 1}).to_string()).unwrap();
        std::thread::spawn(move || {
            let mut got = Vec::new();
            let mut buf = [0u8; 9000];
            let mut k = 0;
            let mut last_more = false;
            sock.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            while got.len() < answers {
                let Ok((n, from)) = sock.recv_from(&mut buf) else { break };
                let req: Value = serde_json::from_slice(&buf[..n]).unwrap();
                assert_eq!((req["cmd"].as_str(), req["token"].as_str()), (Some("LINK"), Some("t0k")));
                let ins: Vec<String> = req["in"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
                if k > 0 && ins.is_empty() && !last_more {
                    // un releve ordinaire, rien a dire : il ne compte pas
                    sock.send_to(json!({"id": req["id"], "ok": true, "out": [], "more": false}).to_string().as_bytes(), from).unwrap();
                    continue;
                }
                let (out, more) = outbox.get(k).cloned().unwrap_or_default();
                k += 1;
                last_more = more;
                sock.send_to(json!({"id": req["id"], "ok": true, "out": out, "more": more}).to_string().as_bytes(), from).unwrap();
                got.push(ins);
            }
            got
        })
    }

    fn collect(r: &Relay, until: impl Fn(&[LinkOut]) -> bool) -> Vec<LinkOut> {
        let t0 = Instant::now();
        let mut all = Vec::new();
        while !until(&all) {
            assert!(t0.elapsed() < Duration::from_secs(4), "timed out: {all:?}");
            all.extend(r.poll());
            std::thread::sleep(Duration::from_millis(5));
        }
        all
    }

    #[test]
    fn the_studio_talks_to_live_through_the_script_channel() {
        let path = temp_state("ok");
        let ack = "f07d555701020100017b7d7b7df7".to_string();
        let fake = fake_script(&path, 3, vec![(vec![], false), (vec![ack.clone()], true), (vec!["f07d555701120000017b7df7".into()], false)]);
        let r = Relay::start(Some(path.clone())).unwrap();
        // le premier releve : le canal repond, « up »
        let got = collect(&r, |a| a.contains(&LinkOut::Up));
        assert_eq!(got[0], LinkOut::Up);
        r.send(vec![HELLO.into()]).unwrap();
        // la reponse du script, puis (« more ») ce qu'il avait encore, sans attendre le releve suivant
        let got = collect(&r, |a| a.iter().filter(|o| matches!(o, LinkOut::Chunks(_))).count() >= 2);
        assert_eq!(got[0], LinkOut::Chunks(vec![ack]));
        let seen = fake.join().unwrap();
        assert_eq!(seen[0], Vec::<String>::new(), "the first request only collects");
        assert_eq!(seen[1], vec![HELLO.to_string()]);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// (26 oct.) Un script 1.3.1 (« link » 2) : il garde le releve vide (« wait ») ; une commande du studio part a cote,
    /// tout de suite, et sa reponse revient sans attendre ce releve.
    #[test]
    fn a_command_does_not_wait_for_the_pending_read() {
        let path = temp_state("pipe");
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = sock.local_addr().unwrap().port();
        std::fs::write(&path, json!({"port": port, "token": "t0k", "version": "1.3.1", "link": 2}).to_string()).unwrap();
        let pong = "f07d555701120000017b7df7".to_string();
        let script = {
            let pong = pong.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 9000];
                let mut held: Vec<(Value, std::net::SocketAddr)> = Vec::new();
                let mut waits = 0;
                sock.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                loop {
                    let Ok((n, from)) = sock.recv_from(&mut buf) else { return (waits, held.len()) };
                    let req: Value = serde_json::from_slice(&buf[..n]).unwrap();
                    let ins = req["in"].as_array().unwrap().len();
                    if ins == 0 {
                        if req["wait"].as_u64() == Some(WAIT_MS) {
                            waits += 1;
                            held.push((req, from)); // garde, comme le canal du script
                            continue;
                        }
                        sock.send_to(json!({"id": req["id"], "ok": true, "out": [], "more": false}).to_string().as_bytes(), from).unwrap();
                        continue;
                    }
                    sock.send_to(json!({"id": req["id"], "ok": true, "out": [pong], "more": false}).to_string().as_bytes(), from).unwrap();
                    // le releve garde repart avec ce que le script pousse ensuite
                    if let Some((h, a)) = held.pop() {
                        sock.send_to(json!({"id": h["id"], "ok": true, "out": ["f07d555701150000017b7df7"], "more": false}).to_string().as_bytes(), a).unwrap();
                        return (waits, 0);
                    }
                }
            })
        };
        let r = Relay::start(Some(path.clone())).unwrap();
        collect(&r, |a| a.contains(&LinkOut::Up));
        std::thread::sleep(Duration::from_millis(60)); // le releve « wait » est maintenant garde chez le script
        let t0 = Instant::now();
        r.send(vec![HELLO.into()]).unwrap();
        let got = collect(&r, |a| a.iter().any(|o| matches!(o, LinkOut::Chunks(_))));
        let ms = t0.elapsed().as_millis();
        assert_eq!(got[0], LinkOut::Chunks(vec![pong]));
        assert!(ms < 40, "the command waited {ms} ms");
        let mut got = got;
        if got.len() < 2 {
            got.extend(collect(&r, |a| a.iter().any(|o| matches!(o, LinkOut::Chunks(_)))));
        }
        assert_eq!(got[1], LinkOut::Chunks(vec!["f07d555701150000017b7df7".into()]));
        let (waits, _) = script.join().unwrap();
        assert!(waits >= 1, "a link-2 script gets reads that wait");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn only_link_chunks_go_and_a_silent_live_is_reported_once() {
        let path = temp_state("down");
        let mute = UdpSocket::bind("127.0.0.1:0").unwrap();
        std::fs::write(&path, json!({"port": mute.local_addr().unwrap().port(), "token": "t0k", "link": 1}).to_string()).unwrap();
        let r = Relay::start(Some(path.clone())).unwrap();
        assert!(r.send(vec!["90407f".into()]).unwrap_err().contains("not an Uppwind Link"));
        assert!(r.send(vec![HELLO.into(); MAX_LINK_CHUNKS + 1]).is_err());
        let got = collect(&r, |a| a.iter().any(|o| matches!(o, LinkOut::Down(_))));
        match &got[0] {
            LinkOut::Down(e) => assert!(e.contains("did not answer") && !e.contains(&path.display().to_string()), "{e}"),
            o => panic!("{o:?}"),
        }
        std::thread::sleep(Duration::from_millis(400));
        assert!(r.poll().is_empty(), "said once");
        let v: Value = serde_json::from_str(&studio_json(&LinkOut::Down("x".into()))).unwrap();
        assert_eq!(v, json!({"type": "link", "state": "down", "error": "x"}));
        drop(r);
        drop(mute);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn link_and_stop_in_a_loop_cannot_grow_the_threads() {
        // un Live muet : chaque relais arrete reste en vie jusqu'a la fin de son echange (REQUEST_TIMEOUT)
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let path = temp_state("cap");
        let mute = UdpSocket::bind("127.0.0.1:0").unwrap();
        std::fs::write(&path, json!({"port": mute.local_addr().unwrap().port(), "token": "t0k", "link": 1}).to_string()).unwrap();
        let mut live = Vec::new();
        let mut refused = 0;
        for _ in 0..12 {
            match Relay::start_counted(Some(path.clone()), &COUNT, 4) {
                Ok(r) => live.push(r),
                Err(e) => {
                    assert!(e.contains("too many"), "{e}");
                    refused += 1;
                }
            }
        }
        assert_eq!((live.len(), refused, COUNT.load(Ordering::SeqCst)), (4, 8, 4));
        // « stop » : les fils finissent leur echange en cours, puis la place revient
        drop(live);
        let t0 = Instant::now();
        while COUNT.load(Ordering::SeqCst) > 0 {
            assert!(t0.elapsed() < REQUEST_TIMEOUT * 3, "the stopped relays never ended");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(Relay::start_counted(Some(path.clone()), &COUNT, 4).is_ok());
        drop(mute);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}


