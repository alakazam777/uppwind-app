// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Le flux du retour audio pour UNE connexion du studio (24 oct.) : un fil qui capture le DAW (ou, en mode demo, fabrique
//! un sinus), le decoupe en paquets « UWA1 » (packet.rs) et les passe a la connexion, qui les envoie en binaire.
//!
//! Studio → compagnon : {"type":"audio","action":"start","target":"auto|live|fl|test-tone|<pid>","rate":48000,
//!                       "format":"f32|i16","ms":5}   puis {"type":"audio","action":"stop"}
//! Compagnon → studio  : {"type":"audio","state":"on",pid,exe,daw,rate,channels,format,frames,ms,capture}
//!                       {"type":"audio","state":"off","reason"|"error"}  et les paquets binaires « UWA1 ».

use super::packet::{self, Packetizer, SampleFormat};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub enum Out {
    Packet(Vec<u8>),
    /// la fin du flux (le DAW est parti, la capture a casse) : la raison
    Ended(String),
}

pub struct Feed {
    rx: Receiver<Out>,
    stop: Arc<AtomicBool>,
    pub info: Value,
    pub sent: u64,
    /// les paquets jetes parce que le socket etait plein (session.rs, pump_audio), et depuis quand il l'est
    pub dropped: u64,
    pub stalled: Option<Instant>,
    pub started: Instant,
    pub label: String,
    /// (25 oct.) la demande du studio quand elle suit les plugins Return (« auto », « live », « fl ») : un Return qui se
    /// connecte apres coup prend le flux, un Return qui part le rend a la capture de Windows (follow_returns)
    pub want: Option<Want>,
    /// « plugin:<id> » : le Return dont vient le son ; None : la capture de Windows (ou le sinus du mode demo)
    pub source: Option<String>,
    /// la generation des Return du hub vue au depart (plugin_link::Hub::returns_generation)
    pub gen: u64,
}

/// (25 oct.) Ce que le studio a demande (« start »), garde pour rouvrir le flux ailleurs.
#[derive(Clone, Debug)]
pub struct Want {
    pub target: String,
    pub rate: u32,
    pub fmt: SampleFormat,
    pub packet_ms: f64,
    pub demo: bool,
    pub test_tone: Option<u32>,
}

/// Les cibles qui suivent les plugins Return : le DAW, pas un processus ni un plugin precis.
pub fn follows_returns(target: &str) -> bool {
    matches!(target, "auto" | "live" | "fl")
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

pub struct Opts {
    pub pid: u32,
    pub exe: String,
    pub tree: bool,
    pub rate: u32,
    pub fmt: SampleFormat,
    pub packet_ms: f64,
    pub demo: bool,
}

impl Feed {
    /// Demarrer : la capture s'ouvre dans son fil ; on attend sa reponse (3 s au plus).
    pub fn start(o: Opts) -> Result<Feed, String> {
        let (tx, rx) = mpsc::channel::<Out>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<Value, String>>();
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let label = if o.demo { "demo sine".to_string() } else { format!("{} (pid {})", o.exe, o.pid) };
        std::thread::spawn(move || {
            if o.demo {
                demo_loop(o, tx, ready_tx, st);
            } else {
                #[cfg(windows)]
                capture_loop(o, tx, ready_tx, st);
                #[cfg(not(windows))]
                {
                    let _ = (o, tx, st);
                    let _ = ready_tx.send(Err("the audio return needs Windows".into()));
                }
            }
        });
        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(info)) => Ok(Feed { rx, stop, info, sent: 0, dropped: 0, stalled: None, started: Instant::now(), label, want: None, source: None, gen: 0 }),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                stop.store(true, Ordering::SeqCst);
                Err("the capture did not start within 3 s".into())
            }
        }
    }

    /// (25 oct.) Un flux fabrique ailleurs : le plugin « Uppwind Link Return » (plugin_link), dont les paquets arrivent
    /// par `rx` ; `stop` dit a l'abonnement de finir quand ce flux disparait.
    pub fn from_parts(rx: Receiver<Out>, stop: Arc<AtomicBool>, info: Value, label: String) -> Feed {
        // (audit de publication) le meme retour borne que la capture : paquets jetes quand le socket est plein, arret apres 3 s
        Feed { rx, stop, info, sent: 0, dropped: 0, stalled: None, started: Instant::now(), label, want: None, source: None, gen: 0 }
    }

    /// Le prochain paquet ou la fin, sans attendre.
    pub fn next(&mut self) -> Option<Out> {
        match self.rx.try_recv() {
            Ok(m) => Some(m),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Out::Ended("the capture stopped".into())),
        }
    }
}

fn info(o: &Opts, frames: usize, capture: &str, sample: &str) -> Value {
    json!({
        "type": "audio", "state": "on", "pid": o.pid, "exe": o.exe, "daw": super::procs::daw_of_exe(&o.exe),
        "rate": o.rate, "channels": 2, "format": o.fmt.name(), "frames": frames,
        "ms": (frames as f64 * 1000.0 / o.rate as f64 * 100.0).round() / 100.0, "capture": capture, "windowsSample": sample,
    })
}

/// Le mode demo (et les tests) : un sinus a 440 Hz, −20 dBFS, cadence par l'horloge, sans Windows.
fn demo_loop(o: Opts, tx: mpsc::Sender<Out>, ready: mpsc::Sender<Result<Value, String>>, stop: Arc<AtomicBool>) {
    let frames = packet::frames_for(o.rate, o.packet_ms);
    let _ = ready.send(Ok(info(&o, frames, "demo", "f32")));
    let mut pk = Packetizer::new(o.rate, 2, frames, o.fmt);
    let t0 = Instant::now();
    let e0 = super::epoch_ms();
    let mut pos = 0u64;
    let a = packet::amp(-20.0) as f32;
    let mut out = Vec::new();
    while !stop.load(Ordering::SeqCst) {
        let s = packet::sine(frames, 2, o.rate, 440.0, a, pos);
        let t = e0 + pos as f64 * 1000.0 / o.rate as f64;
        pk.push(&packet::Chunk { samples: &s, frames, pos, t_ms: t, silent: false, discontinuity: false }, &mut out);
        pos += frames as u64;
        for p in out.drain(..) {
            if tx.send(Out::Packet(p)).is_err() {
                return;
            }
        }
        // le paquet suivant a son heure (pas de derive : compte depuis le debut)
        let due = Duration::from_secs_f64(pos as f64 / o.rate as f64);
        let el = t0.elapsed();
        if due > el {
            std::thread::sleep(due - el);
        }
    }
}

#[cfg(windows)]
fn capture_loop(o: Opts, tx: mpsc::Sender<Out>, ready: mpsc::Sender<Result<Value, String>>, stop: Arc<AtomicBool>) {
    use super::wasapi::{pro_audio_thread, Loopback};
    let mut lb = match Loopback::open(o.pid, o.tree, o.rate) {
        Ok(l) => l,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    if let Err(e) = lb.start() {
        let _ = ready.send(Err(e));
        return;
    }
    pro_audio_thread();
    let frames = packet::frames_for(o.rate, o.packet_ms);
    let mut v = info(&o, frames, "process-loopback", if lb.float { "f32" } else { "i16" });
    v["activateMs"] = json!((lb.activate_ms * 10.0).round() / 10.0);
    let _ = ready.send(Ok(v));
    let mut pk = Packetizer::new(o.rate, 2, frames, o.fmt);
    let mut out = Vec::new();
    let mut levels = packet::Levels::new(2);
    let mut cadence = packet::Cadence::new(25.0);
    let mut last_check = Instant::now();
    let mut last_log = Instant::now();
    let mut chunks = 0u64;
    while !stop.load(Ordering::SeqCst) {
        let mut err = None;
        let r = lb.read(Duration::from_millis(50), &mut |c| {
            cadence.tick(super::wasapi::qpc_ms());
            if c.silent {
                levels.add_silence(c.frames);
            } else {
                levels.add(c.samples);
            }
            pk.push(c, &mut out);
        });
        match r {
            Ok(n) => chunks += n as u64,
            Err(e) => err = Some(e),
        }
        for p in out.drain(..) {
            if tx.send(Out::Packet(p)).is_err() {
                return;
            }
        }
        if let Some(e) = err {
            let _ = tx.send(Out::Ended(format!("the capture broke: {e}")));
            return;
        }
        if last_check.elapsed() > Duration::from_secs(1) {
            last_check = Instant::now();
            if !super::procs::alive(o.pid) {
                let _ = tx.send(Out::Ended(format!("{} quit", o.exe)));
                return;
            }
        }
        if last_log.elapsed() > Duration::from_secs(10) {
            last_log = Instant::now();
            let d = |v: f64| packet::db(v).map(|x| format!("{x:.1}")).unwrap_or_else(|| "-inf".into());
            crate::log(&format!(
                "audio {}: {chunks} chunks from Windows (every {:.1} ms, max {:.1}), {} packets, {} gaps ({} frames lost), peak {} / {} dBFS",
                o.exe,
                cadence.mean(),
                if cadence.n > 0 { cadence.max } else { 0.0 },
                pk.packets,
                pk.gaps,
                pk.lost_frames,
                d(levels.peak(0)),
                d(levels.peak(1)),
            ));
            levels.reset();
            cadence = packet::Cadence::new(25.0);
            chunks = 0;
        }
    }
}

/// Une demande du studio → la reponse (texte) ; `feed` : le flux de cette connexion (remplace, arrete).
/// `demo` : le compagnon en mode demo (un sinus, pas de capture) ; `test_tone` : le pid de la tonalite d'essai.
pub fn on_request(feed: &mut Option<Feed>, action: &str, target: Option<&str>, rate: Option<u32>, format: Option<&str>, ms: Option<f64>, demo: bool, test_tone: Option<u32>) -> String {
    match action {
        "stop" => {
            match feed.take() {
                Some(f) => crate::log(&format!("audio return stopped ({}, {} packets sent in {:.1} s)", f.label, f.sent, f.started.elapsed().as_secs_f64())),
                // (25 oct.) une demande qui arrive est toujours dans le journal : « rien ne s'est passe » se distingue de
                // « rien n'est arrive »
                None => crate::log("audio return: stop asked, none was running"),
            }
            json!({ "type": "audio", "state": "off", "reason": "stop" }).to_string()
        }
        "start" => {
            *feed = None; // (un seul flux par connexion)
            let asked: String = target.unwrap_or("auto").chars().filter(|c| !c.is_control()).take(40).collect();
            let rate = rate.unwrap_or(48000);
            if !(8000..=192_000).contains(&rate) {
                crate::log(&format!("audio return REFUSED (target \"{asked}\"): rate {rate} Hz"));
                return off_err("rate must be between 8000 and 192000 Hz");
            }
            let Some(fmt) = SampleFormat::parse(format.unwrap_or("f32")) else {
                crate::log(&format!("audio return REFUSED (target \"{asked}\"): bad format"));
                return off_err("format is f32 or i16");
            };
            let packet_ms = ms.filter(|m| m.is_finite()).unwrap_or(packet::DEFAULT_PACKET_MS).clamp(1.0, 50.0);
            let w = Want { target: target.unwrap_or("auto").to_string(), rate, fmt, packet_ms, demo, test_tone };
            let hub = crate::plugin_link::hub();
            match open(hub.as_deref(), &w) {
                Ok(f) => {
                    let s = f.info.to_string();
                    *feed = Some(f);
                    s
                }
                Err(e) => off_err(&e),
            }
        }
        _ => off_err("audio action is start or stop"),
    }
}

/// (25 oct.) Ouvrir le flux d'une demande : « plugin » / « plugin:<id> » : le son du plugin Uppwind Link Return du DAW
/// (ASIO compris), pas la capture de Windows ; « auto » / « live » / « fl » aussi, quand un Return de ce DAW est
/// connecte ; sinon la capture de Windows (ou le sinus du mode demo). Le flux garde la demande s'il suit les Return.
pub fn open(hub: Option<&crate::plugin_link::Hub>, w: &Want) -> Result<Feed, String> {
    let gen = hub.map(|h| h.returns_generation()).unwrap_or(0);
    let pick = if w.target.starts_with("plugin") { Some(w.target.clone()) } else { hub.and_then(|h| h.preferred_return(&w.target)) };
    let mut f = match pick {
        Some(t) => {
            let r = match hub {
                Some(h) => h.start_feed(&t, w.rate, w.fmt, w.packet_ms),
                None => Err("the companion does not take plugins (--no-plugins)".to_string()),
            };
            match r {
                Ok(mut f) => {
                    f.source = f.info["plugin"]["id"].as_str().map(|id| format!("plugin:{id}"));
                    f
                }
                Err(e) => {
                    crate::log(&format!("audio return (plugin) failed: {e}"));
                    return Err(e);
                }
            }
        }
        None => start_capture(w)?,
    };
    crate::log(&format!("audio return: {} at {} Hz {} in {:.1} ms packets", f.label, w.rate, w.fmt.name(), w.packet_ms));
    if follows_returns(&w.target) {
        f.want = Some(w.clone());
        f.gen = gen;
    }
    Ok(f)
}

/// La capture de Windows d'un DAW de la liste blanche (ou de la tonalite d'essai), ou le sinus du mode demo.
fn start_capture(w: &Want) -> Result<Feed, String> {
    let (pid, exe) = if w.demo {
        (0, "Demo Synth".to_string())
    } else {
        if let Err(e) = super::available() {
            crate::log(&format!("audio return REFUSED: this Windows cannot capture a single app ({e})"));
            return Err(format!("this Windows cannot capture a single app: {e}"));
        }
        // seulement un DAW de la liste blanche (ou la tonalite d'essai que le compagnon a lancee lui-meme)
        match super::procs::pick(&w.target, &super::procs::list(), false, w.test_tone) {
            Ok(p) => (p.pid, p.exe),
            Err(e) => {
                let asked: String = w.target.chars().filter(|c| !c.is_control()).take(40).collect();
                crate::log(&format!("audio return REFUSED (target \"{asked}\"): {e}"));
                return Err(e);
            }
        }
    };
    Feed::start(Opts { pid, exe: exe.clone(), tree: true, rate: w.rate, fmt: w.fmt, packet_ms: w.packet_ms, demo: w.demo }).map_err(|e| {
        crate::log(&format!("audio return failed ({exe}): {e}"));
        e
    })
}

/// (25 oct.) Un plugin Uppwind Link Return s'est connecte ou est parti (la generation du hub a change), ou `force` :
/// le flux d'une demande « auto » / « live » / « fl » passe au Return de ce DAW, ou revient a la capture de Windows
/// quand son Return est parti. → le message pour le studio (« audio on » avec « switched », ou « audio off » si plus
/// rien ne marche), ou None (rien a changer).
pub fn follow_returns(hub: Option<&crate::plugin_link::Hub>, feed: &mut Option<Feed>, force: bool) -> Option<String> {
    let f = feed.as_mut()?;
    let w = f.want.clone()?;
    let gen = hub.map(|h| h.returns_generation()).unwrap_or(0);
    if gen == f.gen && !force {
        return None;
    }
    f.gen = gen;
    // le Return du flux est toujours la : il le garde (un deuxieme Return ne le lui prend pas)
    if !force {
        if let (Some(src), Some(h)) = (f.source.as_deref(), hub) {
            if h.find_return(src).is_ok() {
                return None;
            }
        }
    }
    let pick = hub.and_then(|h| h.preferred_return(&w.target));
    if pick.is_none() && f.source.is_none() {
        return None; // la capture de Windows, et toujours aucun Return de ce DAW
    }
    let from = f.label.clone();
    let to_plugin = pick.is_some();
    match open(hub, &w) {
        Ok(nf) => {
            let mut info = nf.info.clone();
            info["switched"] = json!(if nf.source.is_some() { "plugin" } else { "capture" });
            info["reason"] = json!(if nf.source.is_some() { "an Uppwind Link Return plugin connected" } else { "the Uppwind Link Return plugin left" });
            crate::log(&format!("audio return SWITCHED to {} (was {from}): {}", nf.label, info["reason"].as_str().unwrap_or("")));
            *feed = Some(nf);
            Some(info.to_string())
        }
        Err(e) if to_plugin => {
            // le Return n'a pas pris : la capture continue
            crate::log(&format!("audio return: the Return plugin could not take over ({e}); still {from}"));
            None
        }
        Err(e) => {
            crate::log(&format!("audio return ended: the Return plugin left and the Windows capture did not start ({e})"));
            *feed = None;
            Some(json!({ "type": "audio", "state": "off", "error": format!("the Uppwind Link Return plugin left and the capture of the DAW did not start: {e}") }).to_string())
        }
    }
}

fn off_err(e: &str) -> String {
    json!({ "type": "audio", "state": "off", "error": e }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_feed_paces_packets_like_real_audio() {
        let mut f = Feed::start(Opts { pid: 0, exe: "Demo Synth".into(), tree: true, rate: 48000, fmt: SampleFormat::F32, packet_ms: 5.0, demo: true }).unwrap();
        assert_eq!(f.info["frames"], 240);
        assert_eq!(f.info["capture"], "demo");
        let t0 = Instant::now();
        let mut got = Vec::new();
        while got.len() < 40 && t0.elapsed() < Duration::from_secs(3) {
            match f.next() {
                Some(Out::Packet(p)) => got.push(packet::decode(&p).unwrap()),
                Some(Out::Ended(e)) => panic!("{e}"),
                None => std::thread::sleep(Duration::from_millis(1)),
            }
        }
        let el = t0.elapsed().as_secs_f64();
        assert_eq!(got.len(), 40);
        // 40 paquets de 5 ms : ~0,2 s (pas d'un coup, pas en retard)
        assert!(el > 0.15 && el < 0.6, "{el} s");
        for (i, (h, s)) in got.iter().enumerate() {
            assert_eq!(h.seq as usize, i + 1);
            assert_eq!(h.pos, i as u64 * 240);
            assert_eq!(s.len(), 480);
        }
        let peak = got.iter().flat_map(|(_, s)| s.iter()).fold(0f32, |m, v| m.max(v.abs()));
        assert_eq!(packet::db(peak as f64).map(|d| d.round()), Some(-20.0));
        drop(f);
    }

    #[test]
    fn requests_are_validated() {
        let mut feed = None;
        assert!(on_request(&mut feed, "start", None, Some(1000), None, None, true, None).contains("rate must be"));
        assert!(on_request(&mut feed, "start", None, None, Some("mp3"), None, true, None).contains("f32 or i16"));
        assert!(on_request(&mut feed, "dance", None, None, None, None, true, None).contains("start or stop"));
        let on: Value = serde_json::from_str(&on_request(&mut feed, "start", Some("auto"), Some(44100), Some("i16"), Some(10.0), true, None)).unwrap();
        assert_eq!((on["state"].as_str(), on["rate"].as_u64(), on["format"].as_str(), on["frames"].as_u64()), (Some("on"), Some(44100), Some("i16"), Some(441)));
        assert!(feed.is_some());
        let off: Value = serde_json::from_str(&on_request(&mut feed, "stop", None, None, None, None, true, None)).unwrap();
        assert_eq!(off["state"], "off");
        assert!(feed.is_none());
    }

    #[cfg(windows)]
    #[test]
    fn the_studio_cannot_capture_a_non_daw_process() {
        // ce processus de test n'est pas un DAW : refuse, par son pid comme par son nom
        let mut feed = None;
        let me = std::process::id().to_string();
        let r: Value = serde_json::from_str(&on_request(&mut feed, "start", Some(&me), None, None, None, false, None)).unwrap();
        assert_eq!(r["state"], "off");
        let e = r["error"].as_str().unwrap();
        assert!(e.contains("not a DAW") || e.contains("cannot capture"), "{e}");
        assert!(!e.to_lowercase().contains(".exe") && !e.to_lowercase().contains("uppwind"), "the studio never learns a program's name: {e}");
        let r: Value = serde_json::from_str(&on_request(&mut feed, "start", Some("explorer"), None, None, None, false, None)).unwrap();
        assert_eq!(r["state"], "off");
        assert!(feed.is_none());
    }
}
