// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (25 oct., phase 0) Les plugins « Uppwind Link » du DAW (tools/uppwind-link/plugin) : un deuxieme socket a l'ecoute,
//! sur 127.0.0.1 seulement, pour les instances de « Uppwind Link Return » (le son du DAW vers le studio, ASIO compris,
//! sans VB-CABLE) et de « Uppwind Link Notes » (les notes du studio vers une piste du DAW, sans loopMIDI). Le protocole
//! est la crate uwl-proto, partagee avec les plugins.
//!
//! - L'ecoute : --plugin-port N (defaut : un port libre au hasard), ecrit dans le fichier d'etat avec un secret a part
//!   (« pluginPort », « pluginSecret » ; le secret est garde d'un lancement a l'autre, comme le jeton). Les plugins le
//!   lisent. Ce secret n'est PAS le jeton du studio : un plugin ne peut ni filmer ni cliquer, il ne parle que de son et
//!   de notes. --no-plugins : pas d'ecoute.
//! - Une connexion qui ne vient pas de 127.0.0.1 est fermee ; 16 connexions au plus a la fois ; la premiere trame doit
//!   etre HELLO (2 s, 1 Ko au plus), avec le bon secret (compare en temps constant) ; apres 5 secrets faux en 30 s, les
//!   faux sont ralentis (1 s) et refuses pendant 30 s, le bon passe toujours.
//! - Le studio (session.rs, apres l'appairage seulement) :
//!   {"type":"instances"} → {"type":"instances","list":[{id,kind,name,host,pid,rate,block,…}]}
//!   {"type":"audio","action":"start","target":"plugin"|"plugin:<id>",…} → le son d'un Return, en paquets « UWA1 »
//!     (le meme chemin que la capture du DAW : audio/feed.rs ; le studio n'y voit qu'une autre source)
//!   {"type":"midi","to":"<id>","events":[{"t":<ms depuis 1970>,"d":[0x90,60,100],"n":1},…]} → vers un Notes ; sans
//!     « t » : tout de suite
//!   {"type":"emitted","id":"<id>"} → {"type":"emitted","id","list":[{n,t,offset,frames,late}]} : ou et quand le Notes
//!     les a sortis (la mesure de la gigue ; vide la liste)

pub mod conn;
pub mod resample;

use crate::audio::feed::{Feed, Out};
use crate::audio::packet::{self, Chunk, Packetizer, SampleFormat};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use uwl_proto as p;

/// Les messages MIDI d'un seul « midi » du studio.
pub const MAX_EVENTS_PER_MESSAGE: usize = 256;
/// Un message plus vieux ou plus lointain que ca est refuse (une horloge folle, un studio qui s'emballe).
pub const PAST_MS: f64 = 10_000.0;
pub const FUTURE_MS: f64 = 30_000.0;
pub const EMITTED_KEEP: usize = 8192;
pub const MAX_FAILURES: u32 = 5;
pub const LOCK: Duration = Duration::from_secs(30);
/// Les connexions de plugins ouvertes a la fois (une piste par instance : de quoi faire ; le studio a les siennes).
pub const MAX_CONNECTIONS: usize = 16;

/// Une place parmi les MAX_CONNECTIONS, rendue quand le fil de la connexion finit.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn now_ms() -> f64 {
    crate::audio::epoch_ms()
}

/// Le secret des plugins : celui du fichier d'etat (« pluginSecret ») s'il est la et bien forme, sinon un neuf.
pub fn load_secret(state: &Path) -> [u8; p::SECRET_LEN] {
    if let Ok(raw) = std::fs::read_to_string(state) {
        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            if let Some(s) = v["pluginSecret"].as_str().and_then(p::parse_secret) {
                return s;
            }
        }
    }
    let mut b = [0u8; p::SECRET_LEN];
    getrandom::fill(&mut b).expect("no system random source");
    b
}

/// Un abonne au son d'un Return : un flux « audio » d'une connexion du studio.
pub struct Sub {
    tx: Sender<Out>,
    stop: Arc<AtomicBool>,
    pk: Packetizer,
    out_rate: u32,
    rs: Option<(u32, resample::Linear)>,
    out_pos: u64,
    stereo: Vec<f32>,
    buf: Vec<f32>,
    packets: Vec<Vec<u8>>,
}

impl Sub {
    fn new(tx: Sender<Out>, stop: Arc<AtomicBool>, out_rate: u32, frames: usize, fmt: SampleFormat) -> Sub {
        Sub { tx, stop, pk: Packetizer::new(out_rate, 2, frames, fmt), out_rate, rs: None, out_pos: 0, stereo: Vec::new(), buf: Vec::new(), packets: Vec::new() }
    }

    /// Une trame AUDIO du plugin → des paquets « UWA1 » pour ce studio. false : l'abonnement est fini (le studio l'a
    /// arrete ou est parti).
    fn push(&mut self, h: &p::AudioHeader, samples: &[f32]) -> bool {
        if self.stop.load(Ordering::SeqCst) {
            return false;
        }
        let frames = h.frames as usize;
        let ch = h.channels.max(1) as usize;
        self.stereo.clear();
        if ch == 2 {
            self.stereo.extend_from_slice(samples);
        } else {
            for f in samples.chunks_exact(ch) {
                self.stereo.push(f[0]);
                self.stereo.push(if ch > 1 { f[1] } else { f[0] });
            }
        }
        let silent = h.flags & p::flags::SILENT != 0;
        let mut disc = h.flags & p::flags::DISCONTINUITY != 0;
        if h.rate == self.out_rate {
            self.rs = None;
            let c = Chunk { samples: &self.stereo, frames, pos: h.pos, t_ms: h.t_ms, silent, discontinuity: disc };
            self.pk.push(&c, &mut self.packets);
        } else {
            if self.rs.as_ref().map(|(r, _)| *r != h.rate).unwrap_or(true) {
                self.rs = Some((h.rate, resample::Linear::new(h.rate, self.out_rate)));
                disc = true;
            }
            let (_, rs) = self.rs.as_mut().unwrap();
            if disc {
                rs.reset();
                self.out_pos = (h.pos as f64 * self.out_rate as f64 / h.rate as f64).round() as u64;
            }
            self.buf.clear();
            rs.process(&self.stereo, &mut self.buf);
            let n = self.buf.len() / 2;
            if n > 0 {
                let c = Chunk { samples: &self.buf, frames: n, pos: self.out_pos, t_ms: h.t_ms, silent, discontinuity: disc };
                self.pk.push(&c, &mut self.packets);
                self.out_pos = self.out_pos.wrapping_add(n as u64);
            }
        }
        for pk in self.packets.drain(..) {
            if self.tx.send(Out::Packet(pk)).is_err() {
                return false;
            }
        }
        true
    }
}

/// Une instance de plugin connectee.
pub struct Instance {
    pub id: u64,
    pub kind: p::Kind,
    pub name: String,
    pub host: String,
    pub pid: u32,
    pub rate: AtomicU32,
    pub max_block: AtomicU32,
    /// (25 oct., Notes) son numero « Link tag » (trame TAG ; 0 : pas encore) : le studio vise « tag:<n> »
    pub tag: AtomicU32,
    pub since: Instant,
    /// le numero de la connexion (pour ne retirer que la bonne entree)
    pub serial: u64,
    writer: Mutex<Option<TcpStream>>,
    subs: Mutex<Vec<Sub>>,
    pub stats: Mutex<Value>,
    emitted: Mutex<VecDeque<p::Emitted>>,
    pub frames_in: AtomicU64,
    pub events_out: AtomicU64,
    /// l'age des trames AUDIO a leur arrivee (ms) : la latence du plugin au compagnon
    ages: Mutex<VecDeque<f64>>,
}

impl Instance {
    pub fn hex(&self) -> String {
        p::instance_hex(self.id)
    }

    fn json(&self) -> Value {
        let ages = self.age_summary();
        json!({
            "id": self.hex(), "kind": self.kind.name(), "name": self.name, "host": self.host, "daw": crate::audio::procs::daw_of_exe(&self.host),
            "pid": self.pid, "rate": self.rate.load(Ordering::Relaxed), "block": self.max_block.load(Ordering::Relaxed),
            "tag": self.tag.load(Ordering::Relaxed),
            "connectedS": (self.since.elapsed().as_secs_f64() * 10.0).round() / 10.0,
            "framesIn": self.frames_in.load(Ordering::Relaxed), "eventsOut": self.events_out.load(Ordering::Relaxed),
            "listeners": self.subs.lock().map(|s| s.len()).unwrap_or(0),
            "arrivalAgeMs": ages, "stats": self.stats.lock().map(|s| s.clone()).unwrap_or(Value::Null),
        })
    }

    fn age_summary(&self) -> Value {
        let Ok(a) = self.ages.lock() else { return Value::Null };
        if a.is_empty() {
            return Value::Null;
        }
        let mut v: Vec<f64> = a.iter().copied().collect();
        v.sort_by(|x, y| x.partial_cmp(y).unwrap());
        let q = |f: f64| (v[((v.len() - 1) as f64 * f).round() as usize] * 100.0).round() / 100.0;
        json!({ "n": v.len(), "median": q(0.5), "p90": q(0.9), "max": q(1.0) })
    }

    fn note_age(&self, age: f64) {
        if let Ok(mut a) = self.ages.lock() {
            if a.len() >= 2000 {
                a.pop_front();
            }
            a.push_back(age);
        }
    }

    /// Une trame AUDIO arrivee : a chaque abonne.
    fn push_audio(&self, h: &p::AudioHeader, samples: &[f32]) {
        self.frames_in.fetch_add(h.frames as u64, Ordering::Relaxed);
        self.note_age(now_ms() - h.t_ms);
        if let Ok(mut subs) = self.subs.lock() {
            subs.retain_mut(|s| s.push(h, samples));
        }
    }

    fn end_subs(&self, why: &str) {
        if let Ok(mut subs) = self.subs.lock() {
            for s in subs.drain(..) {
                let _ = s.tx.send(Out::Ended(why.to_string()));
            }
        }
    }

    fn keep_emitted(&self, list: &[p::Emitted]) {
        if let Ok(mut e) = self.emitted.lock() {
            for x in list {
                if e.len() >= EMITTED_KEEP {
                    e.pop_front();
                }
                e.push_back(*x);
            }
        }
    }
}

struct Guard {
    failures: u32,
    window: Instant,
    locked_until: Option<Instant>,
}

pub struct Hub {
    pub port: u16,
    secret: [u8; p::SECRET_LEN],
    instances: Mutex<HashMap<u64, Arc<Instance>>>,
    guard: Mutex<Guard>,
    serial: AtomicU64,
    /// combien de Notes sont connectes (les connexions du studio lisent alors toutes les 1 ms, pas 10)
    notes: AtomicU64,
    /// (25 oct.) +1 a chaque Return qui se connecte ou part : les flux du studio qui suivent les Return (audio/feed.rs,
    /// follow_returns) se le demandent a chaque tour
    returns_gen: AtomicU64,
}

static HUB: OnceLock<Arc<Hub>> = OnceLock::new();

/// Le hub du compagnon (None : --no-plugins, ou l'ecoute a echoue).
pub fn hub() -> Option<Arc<Hub>> {
    HUB.get().cloned()
}

/// Ecouter sur 127.0.0.1:`port` (0 : au hasard) et garder le hub pour les connexions du studio → le port.
pub fn start(port: u16, secret: [u8; p::SECRET_LEN]) -> Result<u16, String> {
    let hub = Hub::listen(port, secret)?;
    let p = hub.port;
    let _ = HUB.set(hub);
    Ok(p)
}

impl Hub {
    /// L'ecoute et son fil (les tests en ouvrent plusieurs ; le compagnon, un seul, par `start`).
    pub fn listen(port: u16, secret: [u8; p::SECRET_LEN]) -> Result<Arc<Hub>, String> {
        let l = TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("cannot listen for the plugins on 127.0.0.1:{port} ({e})"))?;
        let port = l.local_addr().map(|a| a.port()).unwrap_or(port);
        let hub = Arc::new(Hub {
            port,
            secret,
            instances: Mutex::new(HashMap::new()),
            guard: Mutex::new(Guard { failures: 0, window: Instant::now(), locked_until: None }),
            serial: AtomicU64::new(0),
            notes: AtomicU64::new(0),
            returns_gen: AtomicU64::new(0),
        });
        let h = hub.clone();
        // (audit de publication, comme le studio) au plus MAX_CONNECTIONS a la fois, comptees des l'acceptation : une
        // rafale de connexions a moitie ouvertes ne fait pas pousser les fils sans fin
        let active = Arc::new(AtomicUsize::new(0));
        std::thread::Builder::new()
            .name("uwl-plugins".into())
            .spawn(move || {
                let mut refused: u64 = 0;
                for s in l.incoming() {
                    match s {
                        Ok(s) => {
                            let peer = s.peer_addr();
                            if active.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                                refused += 1;
                                if refused == 1 || refused % 100 == 0 {
                                    crate::log(&format!("plugin link REFUSED a connection: {MAX_CONNECTIONS} are already open ({refused} refused so far)"));
                                }
                                continue;
                            }
                            // §7, regle 1 : la machine elle-meme, et rien d'autre
                            match peer {
                                Ok(a) if a.ip().is_loopback() => {
                                    let h2 = h.clone();
                                    active.fetch_add(1, Ordering::SeqCst);
                                    let slot = Slot(active.clone());
                                    std::thread::spawn(move || {
                                        let _slot = slot;
                                        conn::run(s, h2, a)
                                    });
                                }
                                _ => crate::log(&format!("plugin link REFUSED: {peer:?} is not this computer")),
                            }
                        }
                        Err(e) => crate::log(&format!("plugin link accept: {e}")),
                    }
                }
            })
            .map_err(|e| format!("cannot start the plugin listener ({e})"))?;
        Ok(hub)
    }

    pub(crate) fn secret_ok(&self, s: &[u8]) -> bool {
        p::ct_eq(s, &self.secret)
    }

    pub(crate) fn locked(&self) -> bool {
        let mut g = self.guard.lock().unwrap();
        match g.locked_until {
            Some(t) if Instant::now() < t => true,
            Some(_) => {
                g.locked_until = None;
                g.failures = 0;
                false
            }
            None => false,
        }
    }

    pub(crate) fn failure(&self) {
        let mut g = self.guard.lock().unwrap();
        if g.window.elapsed() > LOCK {
            g.window = Instant::now();
            g.failures = 0;
        }
        g.failures += 1;
        if g.failures >= MAX_FAILURES {
            g.locked_until = Some(Instant::now() + LOCK);
        }
    }

    pub(crate) fn next_serial(&self) -> u64 {
        self.serial.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Enregistrer une instance ; false si son identite est deja connectee (une piste dupliquee dans le DAW).
    pub(crate) fn register(&self, i: Arc<Instance>) -> bool {
        let mut m = self.instances.lock().unwrap();
        if m.contains_key(&i.id) {
            return false;
        }
        if i.kind == p::Kind::Notes {
            self.notes.fetch_add(1, Ordering::SeqCst);
        }
        let ret = i.kind == p::Kind::Return;
        m.insert(i.id, i);
        if ret {
            self.returns_gen.fetch_add(1, Ordering::SeqCst);
        }
        true
    }

    /// (25 oct.) Change quand un Return se connecte ou part.
    pub fn returns_generation(&self) -> u64 {
        self.returns_gen.load(Ordering::SeqCst)
    }

    pub(crate) fn unregister(&self, id: u64, serial: u64) {
        let mut m = self.instances.lock().unwrap();
        if m.get(&id).map(|i| i.serial == serial).unwrap_or(false) {
            if let Some(i) = m.remove(&id) {
                if i.kind == p::Kind::Notes {
                    self.notes.fetch_sub(1, Ordering::SeqCst);
                }
                if i.kind == p::Kind::Return {
                    self.returns_gen.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
    }

    pub fn notes_connected(&self) -> bool {
        self.notes.load(Ordering::Relaxed) > 0
    }

    pub fn get(&self, id: u64) -> Option<Arc<Instance>> {
        self.instances.lock().unwrap().get(&id).cloned()
    }

    pub fn list(&self) -> Vec<Arc<Instance>> {
        let mut v: Vec<Arc<Instance>> = self.instances.lock().unwrap().values().cloned().collect();
        v.sort_by_key(|i| i.serial);
        v
    }

    pub fn instances_json(&self) -> Value {
        json!({ "type": "instances", "port": self.port, "list": self.list().iter().map(|i| i.json()).collect::<Vec<_>>() })
    }

    /// Le studio demande le son d'un DAW (« auto », « live », « fl ») : un Return connecte de ce DAW passe avant la capture
    /// de Windows (il marche en ASIO, il est plus rapide). None : aucun, la capture de Windows comme avant.
    pub fn preferred_return(&self, target: &str) -> Option<String> {
        let want = match target {
            "auto" => None,
            "live" | "fl" => Some(target),
            _ => return None,
        };
        self.list()
            .into_iter()
            .filter(|i| i.kind == p::Kind::Return)
            .filter(|i| want.is_none() || crate::audio::procs::daw_of_exe(&i.host) == want)
            .last()
            .map(|i| format!("plugin:{}", i.hex()))
    }

    /// « plugin » : le Return connecte le plus recemment ; « plugin:<id> » : celui-la.
    pub fn find_return(&self, target: &str) -> Result<Arc<Instance>, String> {
        let rest = target.strip_prefix("plugin").unwrap_or("");
        if let Some(id) = rest.strip_prefix(':') {
            let id = p::parse_instance(id).ok_or("bad plugin id")?;
            return match self.get(id) {
                Some(i) if i.kind == p::Kind::Return => Ok(i),
                Some(_) => Err("this plugin is an Uppwind Link Notes, not a Return".into()),
                None => Err("this Uppwind Link Return is not connected".into()),
            };
        }
        if !rest.is_empty() {
            return Err("target is plugin or plugin:<id>".into());
        }
        self.list()
            .into_iter()
            .filter(|i| i.kind == p::Kind::Return)
            .last()
            .ok_or_else(|| "no Uppwind Link Return plugin is connected (put it on the master track of the DAW)".into())
    }

    /// Le son d'un Return pour une connexion du studio : un flux comme celui de la capture du DAW.
    pub fn start_feed(&self, target: &str, rate: u32, fmt: SampleFormat, packet_ms: f64) -> Result<Feed, String> {
        let inst = self.find_return(target)?;
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let frames = packet::frames_for(rate, packet_ms);
        inst.subs.lock().unwrap_or_else(|e| e.into_inner()).push(Sub::new(tx, stop.clone(), rate, frames, fmt));
        let in_rate = inst.rate.load(Ordering::Relaxed);
        let info = json!({
            "type": "audio", "state": "on", "pid": inst.pid, "exe": inst.host, "daw": crate::audio::procs::daw_of_exe(&inst.host),
            "rate": rate, "channels": 2, "format": fmt.name(), "frames": frames,
            "ms": (frames as f64 * 1000.0 / rate as f64 * 100.0).round() / 100.0, "capture": "plugin",
            "plugin": { "id": inst.hex(), "name": inst.name, "rate": in_rate, "block": inst.max_block.load(Ordering::Relaxed) },
            "resampled": in_rate != rate,
        });
        let label = format!("{} {} ({} pid {})", inst.name, inst.hex(), inst.host, inst.pid);
        Ok(Feed::from_parts(rx, stop, info, label))
    }

    /// (25 oct.) Le Notes qui porte ce numero « Link tag » (le Remote Script de Live le choisit, unique dans le set).
    /// Deux Notes connectes avec le meme numero (deux DAW ouverts, un set copie) : le plus recent.
    pub fn notes_by_tag(&self, tag: u32) -> Option<Arc<Instance>> {
        if tag == 0 {
            return None;
        }
        self.list().into_iter().filter(|i| i.kind == p::Kind::Notes && i.tag.load(Ordering::Relaxed) == tag).last()
    }

    /// « <id en hexadecimal> » ou (25 oct.) « tag:<n> » → le Notes vise.
    pub fn notes_target(&self, to: &str) -> Result<Arc<Instance>, String> {
        if let Some(t) = to.strip_prefix("tag:") {
            let n: u32 = t.parse().ok().filter(|n| *n >= 1 && *n <= p::TAG_MAX).ok_or("bad tag")?;
            return self.notes_by_tag(n).ok_or_else(|| format!("no Uppwind Link Notes with tag {n} is connected (is the helper track's plugin on, and the companion's state file readable?)"));
        }
        let id = p::parse_instance(to).ok_or("bad plugin id")?;
        self.get(id).ok_or_else(|| "this Uppwind Link Notes is not connected".to_string())
    }

    /// Des messages du studio vers un Notes → combien sont partis.
    pub fn send_midi(&self, to: &str, events: &[crate::protocol::MidiIn]) -> Result<usize, String> {
        let inst = self.notes_target(to)?;
        if inst.kind != p::Kind::Notes {
            return Err("this plugin is an Uppwind Link Return, not a Notes".into());
        }
        if events.is_empty() {
            return Ok(0);
        }
        if events.len() > MAX_EVENTS_PER_MESSAGE {
            return Err(format!("at most {MAX_EVENTS_PER_MESSAGE} events per message"));
        }
        let now = now_ms();
        let mut evs = Vec::with_capacity(events.len());
        for (k, e) in events.iter().enumerate() {
            let t = e.t.unwrap_or(now);
            if !t.is_finite() || t < now - PAST_MS || t > now + FUTURE_MS {
                return Err("event time out of range".into());
            }
            if e.d.is_empty() || e.d.len() > 3 {
                return Err("bad midi message".into());
            }
            let mut data = [0u8; 3];
            data[..e.d.len()].copy_from_slice(&e.d);
            let len = e.d.len() as u8;
            if !p::valid_midi(len, &data) {
                return Err("bad midi message (channel messages only)".into());
            }
            evs.push(p::Event { t_ms: t, n: e.n.unwrap_or(k as u32), len, data });
        }
        let mut out = Vec::with_capacity(8 + evs.len() * p::EVENT_BYTES);
        p::encode_events(&evs, &mut out);
        let mut w = inst.writer.lock().unwrap();
        let Some(s) = w.as_mut() else { return Err("this Uppwind Link Notes cannot receive yet".into()) };
        s.write_all(&out).map_err(|e| format!("the plugin did not take the notes ({e})"))?;
        inst.events_out.fetch_add(evs.len() as u64, Ordering::Relaxed);
        Ok(evs.len())
    }

    /// Ou et quand un Notes a sorti ses messages (vide la liste).
    pub fn take_emitted(&self, id: &str) -> Result<Vec<p::Emitted>, String> {
        let id = p::parse_instance(id).ok_or("bad plugin id")?;
        let inst = self.get(id).ok_or("this plugin is not connected")?;
        let mut e = inst.emitted.lock().unwrap();
        Ok(e.drain(..).collect())
    }
}

// ─── ce que session.rs appelle ───────────────────────────────────────────────────────────────────────────────

pub fn available() -> bool {
    HUB.get().is_some()
}

/// Un Notes est connecte : une note du studio ne doit pas attendre le repos de 10 ms de la connexion.
pub fn notes_connected() -> bool {
    HUB.get().map(|h| h.notes_connected()).unwrap_or(false)
}

pub fn instances_json() -> String {
    match hub() {
        Some(h) => h.instances_json().to_string(),
        None => json!({ "type": "instances", "list": [], "error": "the companion does not take plugins (--no-plugins)" }).to_string(),
    }
}

pub fn send_midi(to: &str, events: &[crate::protocol::MidiIn]) -> Result<usize, String> {
    hub().ok_or("the companion does not take plugins (--no-plugins)")?.send_midi(to, events)
}

pub fn emitted_json(id: &str) -> String {
    let r = hub().ok_or("the companion does not take plugins (--no-plugins)".to_string()).and_then(|h| h.take_emitted(id));
    match r {
        Ok(list) => json!({
            "type": "emitted", "id": id,
            "list": list.iter().map(|e| json!({ "n": e.n, "t": e.t_ms, "offset": e.offset, "frames": e.frames, "late": e.late_ms })).collect::<Vec<_>>(),
        })
        .to_string(),
        Err(e) => json!({ "type": "emitted", "id": id, "error": e }).to_string(),
    }
}

#[cfg(test)]
mod tests;
