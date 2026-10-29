// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Le lien d'une instance avec le compagnon : un fil a elle, jamais le fil audio. Une connexion TCP sur 127.0.0.1
//! (uwl-proto) ; le port et le secret sont lus dans le fichier d'etat du compagnon a chaque essai (le compagnon peut
//! changer de port d'un lancement a l'autre). Le fil audio ne voit que des files sans verrou (ring.rs) et des
//! compteurs atomiques ; ce fil-ci fait tout le reste : l'heure de Windows, le reseau, les reconnexions.
//!
//! Return : fil audio → (en-tetes de blocs + echantillons) → ce fil → trames AUDIO → compagnon → studio.
//! Notes  : compagnon → trames EVENTS → ce fil (ms depuis 1970 → horloge du plugin) → file → fil audio → sortie MIDI.

use crate::clock;
use crate::ring::{Consumer, Producer};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use uwl_proto as p;

pub const STATE_NAME: &str = ".uppwind-companion.json";
/// Une trame AUDIO porte au plus ca de trames (un bloc plus grand part en plusieurs trames).
pub const AUDIO_CHUNK: usize = 4096;

/// Le fichier d'etat du compagnon : UWL_STATE_FILE (les essais), sinon %USERPROFILE%\.uppwind-companion.json.
pub fn state_path() -> PathBuf {
    if let Some(p) = std::env::var_os("UWL_STATE_FILE") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(STATE_NAME)
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkInfo {
    pub port: u16,
    pub secret: [u8; p::SECRET_LEN],
}

/// Le port et le secret des plugins, ecrits par le compagnon (« pluginPort », « pluginSecret »). Jamais d'autre
/// machine : l'adresse est toujours 127.0.0.1, quoi que dise le fichier.
pub fn read_link(path: &Path) -> Result<LinkInfo, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("no companion state file at {} ({e})", path.display()))?;
    let v: serde_json::Value = serde_json::from_str(&raw).map_err(|_| "the companion state file is not JSON".to_string())?;
    // (29 oct.) le compagnon retire pluginPort quand il s'arrete proprement : son absence veut d'abord dire « arrete »
    let port = v["pluginPort"].as_u64().filter(|p| (1..=65535).contains(p)).ok_or_else(|| "no pluginPort in the companion state file: the companion is not running, or it is an older companion, or it runs with --no-plugins".to_string())?;
    let secret = v["pluginSecret"].as_str().and_then(p::parse_secret).ok_or_else(|| "no plugin secret in the companion state file".to_string())?;
    Ok(LinkInfo { port: port as u16, secret })
}

/// Une identite d'instance au hasard (pas cryptographique : elle distingue des instances, elle ne protege rien).
pub fn new_instance_id() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.write_u32(std::process::id());
    match h.finish() {
        0 => 1,
        v => v,
    }
}

/// Le nom de l'exe qui charge le plugin (pour l'affichage dans le compagnon : « Ableton Live 12 Suite.exe »).
pub fn host_exe() -> String {
    std::env::current_exe().ok().and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned())).unwrap_or_default()
}

/// Un bloc du fil audio (Return) : sa position, l'instant (horloge du plugin) de sa premiere trame, sa taille, ses bits.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BlockHeader {
    pub pos: u64,
    pub t: f64,
    pub frames: u32,
    pub flags: u8,
}

/// Un message pour le fil audio (Notes). `len == 0` : « relache tout » (le compagnon est parti).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Wire {
    pub t: f64,
    pub n: u32,
    pub len: u8,
    pub data: [u8; 3],
}

/// Ce que le fil audio dit d'un message sorti (Notes) : la mesure.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Report {
    pub n: u32,
    pub t: f64,
    pub offset: u32,
    pub frames: u32,
    pub late_ms: f32,
}

/// L'etat partage par le fil audio, le fil reseau et le plugin. Le fil audio n'y fait que des lectures et des
/// ecritures atomiques (Relaxed).
pub struct Shared {
    pub kind: p::Kind,
    /// l'identite de l'instance : la meme cellule que le champ persistant du plugin (le DAW la garde dans le projet)
    pub instance: Arc<AtomicU64>,
    pub rate: AtomicU32,
    pub max_block: AtomicU32,
    /// (25 oct., Notes) le parametre « Link tag » : le numero que le Remote Script de Live lui a donne (0 : aucun)
    pub tag: AtomicU32,
    /// active() .. deactivate() : le lien ne se fait que pendant ce temps (un plugin juste scanne ne se connecte pas)
    pub active: AtomicBool,
    pub stop: AtomicBool,
    pub connected: AtomicBool,
    // les compteurs du fil audio
    pub blocks: AtomicU64,
    pub frames: AtomicU64,
    pub dropped: AtomicU64,
    pub resets: AtomicU64,
    pub skipped: AtomicU64,
    pub jitter_us: AtomicU64,
    pub jitter_max_us: AtomicU64,
    pub emitted: AtomicU64,
    pub late: AtomicU64,
    pub late_max_us: AtomicU64,
    pub stale: AtomicU64,
    pub overflow: AtomicU64,
    pub send_fail: AtomicU64,
    pub panics: AtomicU64,
    // le fil reseau
    pub connects: AtomicU64,
    pub status: Mutex<String>,
    /// (26 oct.) le transport du DAW, lu dans process() : le tempo en milliemes de BPM (0 : le DAW ne le dit pas) et
    /// la lecture (0 arretee, 1 en lecture, 2 inconnu). Le studio suit ainsi le tempo de FL Studio sans script.
    pub tempo_milli: AtomicU64,
    pub playing: AtomicU32,
}

/// Le tempo que dit le DAW → milliemes de BPM (0 : rien de croyable).
pub fn tempo_milli(tempo: Option<f64>) -> u64 {
    match tempo {
        Some(t) if t.is_finite() && t >= 1.0 && t <= 999.0 => (t * 1000.0).round() as u64,
        _ => 0,
    }
}

impl Shared {
    pub fn new(kind: p::Kind, instance: Arc<AtomicU64>) -> Shared {
        Shared {
            kind,
            instance,
            rate: AtomicU32::new(48000),
            max_block: AtomicU32::new(1024),
            tag: AtomicU32::new(0),
            active: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            connected: AtomicBool::new(false),
            blocks: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            resets: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
            jitter_us: AtomicU64::new(0),
            jitter_max_us: AtomicU64::new(0),
            emitted: AtomicU64::new(0),
            late: AtomicU64::new(0),
            late_max_us: AtomicU64::new(0),
            stale: AtomicU64::new(0),
            overflow: AtomicU64::new(0),
            send_fail: AtomicU64::new(0),
            panics: AtomicU64::new(0),
            connects: AtomicU64::new(0),
            status: Mutex::new("not started".into()),
            tempo_milli: AtomicU64::new(0),
            playing: AtomicU32::new(2),
        }
    }

    /// (26 oct.) Le transport du bloc (fil audio : deux ecritures atomiques, rien d'autre).
    pub fn note_transport(&self, tempo: Option<f64>, playing: bool) {
        self.tempo_milli.store(tempo_milli(tempo), Ordering::Relaxed);
        self.playing.store(playing as u32, Ordering::Relaxed);
    }

    /// Ce qui, du transport, fait partir une trame STATS tout de suite quand il change.
    pub fn transport_key(&self) -> (u64, u32) {
        (self.tempo_milli.load(Ordering::Relaxed), self.playing.load(Ordering::Relaxed))
    }

    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub fn instance_id(&self) -> u64 {
        self.instance.load(Ordering::Relaxed)
    }

    /// Le nom annonce au compagnon (le nom de la piste viendra du DAW en phase 1 : VST3 IInfoListener).
    pub fn display_name(&self) -> String {
        match self.kind {
            p::Kind::Return => "Uppwind Link Return".into(),
            p::Kind::Notes => format!("Uppwind Link Notes {:04x}", self.instance_id() & 0xffff),
        }
    }

    fn set_status(&self, s: String) {
        if let Ok(mut g) = self.status.lock() {
            *g = s;
        }
    }

    pub fn status(&self) -> String {
        self.status.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Les compteurs en JSON (trame STATS, toutes les 2 s ; (26 oct.) et des que le tempo ou la lecture du DAW change).
    pub fn stats_json(&self) -> String {
        let r = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let ms = |a: &AtomicU64| a.load(Ordering::Relaxed) as f64 / 1000.0;
        let (tm, pl) = self.transport_key();
        let tempo = if tm == 0 { "null".to_string() } else { format!("{:.3}", tm as f64 / 1000.0) };
        let playing = match pl {
            0 => "false",
            1 => "true",
            _ => "null",
        };
        format!(
            "{{\"kind\":\"{}\",\"tag\":{},\"rate\":{},\"maxBlock\":{},\"tempo\":{tempo},\"playing\":{playing},\"blocks\":{},\"frames\":{},\"dropped\":{},\"resets\":{},\"skipped\":{},\"jitterMs\":{:.3},\"jitterMaxMs\":{:.3},\"emitted\":{},\"late\":{},\"lateMaxMs\":{:.3},\"stale\":{},\"overflow\":{},\"sendFail\":{},\"panics\":{},\"connects\":{}}}",
            self.kind.name(),
            self.tag.load(Ordering::Relaxed),
            self.rate.load(Ordering::Relaxed),
            self.max_block.load(Ordering::Relaxed),
            r(&self.blocks),
            r(&self.frames),
            r(&self.dropped),
            r(&self.resets),
            r(&self.skipped),
            ms(&self.jitter_us),
            ms(&self.jitter_max_us),
            r(&self.emitted),
            r(&self.late),
            ms(&self.late_max_us),
            r(&self.stale),
            r(&self.overflow),
            r(&self.send_fail),
            r(&self.panics),
            r(&self.connects),
        )
    }
}

/// Les bouts des files que le fil reseau tient.
pub enum Io {
    Return { headers: Consumer<BlockHeader>, samples: Consumer<f32> },
    Notes { events: Producer<Wire>, reports: Consumer<Report> },
}

/// Le fil reseau d'une instance : demarre au premier active(), arrete (et attendu) quand le plugin disparait.
pub struct NetThread {
    shared: Arc<Shared>,
    handle: Option<JoinHandle<()>>,
}

impl NetThread {
    pub fn start(shared: Arc<Shared>, io: Io) -> NetThread {
        let sh = shared.clone();
        let handle = std::thread::Builder::new().name(format!("uwl-{}", shared.kind.name())).spawn(move || run(sh, io)).ok();
        NetThread { shared, handle }
    }
}

impl Drop for NetThread {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

struct Conn {
    s: TcpStream,
    reader: p::FrameReader,
}

fn connect(sh: &Shared) -> Result<Conn, (String, bool)> {
    let info = read_link(&state_path()).map_err(|e| (e, false))?;
    connect_to(sh, &info)
}

/// (29 oct., protocole v2, audit PL-1) La poignee de main a preuves (uwl-proto) : HELLO avec un nonce neuf, jamais le
/// secret ; le CHALLENGE du compagnon est verifie (temps constant) AVANT tout, et seulement ensuite part AUTH. Sans
/// preuve juste, rien ne part (pas de son, pas de preuve du plugin) et rien n'est accepte (pas de notes, pas de
/// changement d'identite) : la connexion se ferme et le fil attend (backoff), comme pour un compagnon absent.
/// Err((raison, reessayer tout de suite)).
fn connect_to(sh: &Shared, info: &LinkInfo) -> Result<Conn, (String, bool)> {
    let addr = SocketAddr::from(([127, 0, 0, 1], info.port));
    let s = TcpStream::connect_timeout(&addr, Duration::from_millis(300)).map_err(|e| (format!("no companion on 127.0.0.1:{} ({e})", info.port), false))?;
    let _ = s.set_nodelay(true);
    let _ = s.set_write_timeout(Some(Duration::from_millis(500)));
    let _ = s.set_read_timeout(Some(Duration::from_millis(200)));
    let mut nonce = [0u8; p::NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|e| (format!("no system random source ({e})"), false))?;
    let not_proved = || {
        (
            format!("the program on 127.0.0.1:{} did not prove it knows the plugin secret (not the companion, an older companion, or its state file changed): it gets no sound and no proof, and no note is taken from it", info.port),
            false,
        )
    };
    let hello = p::Hello {
        version: p::VERSION,
        kind: sh.kind,
        flags: 0,
        nonce,
        instance: sh.instance_id(),
        rate: sh.rate.load(Ordering::Relaxed),
        max_block: sh.max_block.load(Ordering::Relaxed),
        channels: 2,
        pid: std::process::id(),
        name: sh.display_name(),
        host: host_exe(),
    };
    let mut out = Vec::new();
    p::encode_hello(&hello, &mut out);
    let mut c = Conn { s, reader: p::FrameReader::new(p::MAX_HELLO) };
    c.s.write_all(&out).map_err(|e| (format!("hello: {e}"), false))?;
    // le CHALLENGE puis le verdict, dans les 2 s
    let mut proved = false;
    let t0 = Instant::now();
    let mut buf = [0u8; 2048];
    while t0.elapsed() < Duration::from_secs(2) {
        if sh.stop.load(Ordering::Relaxed) {
            return Err(("stopping".into(), false));
        }
        match c.s.read(&mut buf) {
            Ok(0) => return Err((if proved { "the companion closed the connection".into() } else { not_proved().0 }, false)),
            Ok(n) => c.reader.extend(&buf[..n]),
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => continue,
            Err(e) => return Err((format!("welcome: {e}"), false)),
        }
        loop {
            match c.reader.next() {
                Ok(Some((p::ty::CHALLENGE, body))) if !proved => {
                    let ch = p::decode_challenge(&body).map_err(|e| (format!("challenge: {e}"), false))?;
                    if !p::auth::companion_proof_ok(&info.secret, info.port, &nonce, &ch.nonce, &ch.proof) {
                        return Err(not_proved());
                    }
                    // le compagnon a prouve qu'il connait le secret : la preuve du plugin, puis le verdict
                    proved = true;
                    out.clear();
                    p::encode_auth(&p::auth::plugin_proof(&info.secret, info.port, &ch.nonce, &nonce), &mut out);
                    c.s.write_all(&out).map_err(|e| (format!("auth: {e}"), false))?;
                }
                Ok(Some((p::ty::WELCOME, body))) => {
                    let w = p::decode_welcome(&body).map_err(|e| (format!("welcome: {e}"), false))?;
                    // un verdict sans preuve verifiee ne vaut rien : ni « ok », ni « deja connectee » (I-6 : un faux
                    // compagnon changerait l'identite gardee dans le projet) ; seul un refus se dit
                    if !proved {
                        return Err(match w.status {
                            p::status::OK | p::status::DUPLICATE => not_proved(),
                            // (un compagnon d'avant prend le nonce pour un secret faux)
                            p::status::BAD_SECRET => (format!("refused by the companion (status 1: {}): an older companion? This plugin needs Uppwind Link 0.2 or later", w.message), false),
                            s => (format!("refused by the companion (status {s}: {})", w.message), false),
                        });
                    }
                    return match w.status {
                        p::status::OK => {
                            c.reader.set_max(p::MAX_FRAME);
                            // ensuite : une lecture n'attend pas plus d'1 ms (Notes lit, Return ne lit plus)
                            let _ = c.s.set_read_timeout(Some(Duration::from_millis(1)));
                            Ok(c)
                        }
                        // une copie de cette instance (piste dupliquee) est deja la : une identite neuve, et on reessaie
                        p::status::DUPLICATE => {
                            sh.instance.store(new_instance_id(), Ordering::Relaxed);
                            Err((format!("instance already connected ({}): new identity", w.message), true))
                        }
                        s => Err((format!("refused by the companion (status {s}: {})", w.message), false)),
                    };
                }
                Ok(Some((t, _))) => return Err((format!("unexpected frame 0x{t:02x} before welcome"), false)),
                Ok(None) => break,
                Err(e) => return Err((format!("welcome: {e}"), false)),
            }
        }
    }
    Err((if proved { "no welcome within 2 s".into() } else { not_proved().0 }, false))
}

fn send(c: &mut Conn, out: &mut Vec<u8>) -> bool {
    if out.is_empty() {
        return true;
    }
    let ok = c.s.write_all(out).is_ok();
    out.clear();
    ok
}

/// Lire ce qui est arrive (Notes : des EVENTS ; les deux : BYE). false : la connexion est finie.
fn pump_in(c: &mut Conn, on_frame: &mut dyn FnMut(u8, &[u8]) -> bool) -> bool {
    let mut buf = [0u8; 16 * 1024];
    loop {
        match c.s.read(&mut buf) {
            Ok(0) => return false,
            Ok(n) => {
                c.reader.extend(&buf[..n]);
                loop {
                    match c.reader.next() {
                        Ok(Some((t, body))) => {
                            if !on_frame(t, &body) {
                                return false;
                            }
                        }
                        Ok(None) => break,
                        Err(_) => return false,
                    }
                }
                if n < buf.len() {
                    return true;
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => return true,
            Err(_) => return false,
        }
    }
}

fn run(sh: Arc<Shared>, mut io: Io) {
    let mut conn: Option<Conn> = None;
    let mut next_try = Instant::now();
    let mut backoff = Duration::from_millis(250);
    let mut offset = clock::epoch_offset_ms();
    let mut last_offset = Instant::now();
    let mut last_stats = Instant::now();
    // (26 oct.) le transport annonce dans la derniere trame STATS
    let mut sent_transport: Option<(u64, u32)> = None;
    let mut sent_format = (0u32, 0u32);
    // (25 oct., Notes) le dernier numero « Link tag » annonce sur cette connexion (None : pas encore)
    let mut sent_tag: Option<u32> = None;
    let mut out: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut scratch: Vec<f32> = Vec::with_capacity(AUDIO_CHUNK * 2);
    let mut events: Vec<p::Event> = Vec::with_capacity(p::MAX_EVENTS);
    let mut emitted: Vec<p::Emitted> = Vec::with_capacity(p::MAX_EVENTS);
    sh.set_status("idle (the DAW has not activated the plugin yet)".into());
    loop {
        if sh.stop.load(Ordering::SeqCst) {
            if let Some(mut c) = conn.take() {
                p::encode_text(p::ty::BYE, "plugin removed", &mut out);
                let _ = send(&mut c, &mut out);
            }
            sh.connected.store(false, Ordering::SeqCst);
            return;
        }
        // l'ecart entre les deux horloges, remesure chaque seconde
        if last_offset.elapsed() >= Duration::from_secs(1) {
            offset = clock::epoch_offset_ms();
            last_offset = Instant::now();
        }
        let active = sh.active.load(Ordering::SeqCst);
        if !active {
            if let Some(mut c) = conn.take() {
                p::encode_text(p::ty::BYE, "deactivated", &mut out);
                let _ = send(&mut c, &mut out);
                sh.connected.store(false, Ordering::SeqCst);
                sh.set_status("deactivated by the DAW".into());
                if let Io::Notes { events: ev, .. } = &mut io {
                    let _ = ev.push(Wire { len: 0, ..Default::default() });
                }
            }
            drain(&mut io);
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        if conn.is_none() && Instant::now() >= next_try {
            match connect(&sh) {
                Ok(c) => {
                    conn = Some(c);
                    backoff = Duration::from_millis(250);
                    sent_format = (0, 0);
                    sent_tag = None;
                    sent_transport = None;
                    sh.connects.fetch_add(1, Ordering::Relaxed);
                    drain(&mut io);
                    sh.connected.store(true, Ordering::SeqCst);
                    sh.set_status(format!("connected as {}", p::instance_hex(sh.instance_id())));
                }
                Err((e, retry_now)) => {
                    sh.set_status(e);
                    next_try = Instant::now() + if retry_now { Duration::ZERO } else { backoff };
                    if !retry_now {
                        backoff = (backoff * 2).min(Duration::from_secs(2));
                    }
                }
            }
        }
        let mut alive = true;
        match (&mut io, conn.as_mut()) {
            (Io::Return { headers, samples }, Some(c)) => {
                let rate = sh.rate.load(Ordering::Relaxed);
                let fmt = (rate, sh.max_block.load(Ordering::Relaxed));
                if fmt != sent_format {
                    p::encode_format(&p::Format { rate: fmt.0, max_block: fmt.1, channels: 2 }, &mut out);
                    sent_format = fmt;
                }
                while let Some(h) = headers.pop() {
                    let n = h.frames as usize;
                    scratch.clear();
                    if samples.read_with(n * 2, |a, b| {
                        scratch.extend_from_slice(a);
                        scratch.extend_from_slice(b);
                    })
                    .is_none()
                    {
                        // ne devrait jamais arriver (l'en-tete suit ses echantillons) : on repart proprement
                        let k = samples.len();
                        samples.skip(k);
                        continue;
                    }
                    let mut done = 0usize;
                    while done < n {
                        let k = (n - done).min(AUDIO_CHUNK);
                        let ah = p::AudioHeader {
                            pos: h.pos + done as u64,
                            t_ms: h.t + offset + done as f64 * 1000.0 / rate.max(1) as f64,
                            rate,
                            frames: k as u16,
                            channels: 2,
                            flags: if done == 0 { h.flags } else { h.flags & !p::flags::DISCONTINUITY },
                        };
                        p::encode_audio(&ah, &scratch[done * 2..(done + k) * 2], &mut out);
                        done += k;
                    }
                    if out.len() > 48 * 1024 && !send(c, &mut out) {
                        alive = false;
                        break;
                    }
                }
                if alive && !send(c, &mut out) {
                    alive = false;
                }
            }
            (Io::Notes { events: ev, reports }, Some(c)) => {
                // (25 oct.) le numero « Link tag » : a la connexion, puis des que le script l'a change
                let tag = sh.tag.load(Ordering::Relaxed);
                if sent_tag != Some(tag) {
                    p::encode_tag(tag, &mut out);
                    sent_tag = Some(tag);
                }
                let sh2 = &sh;
                let off = offset;
                alive = pump_in(c, &mut |t, body| match t {
                    p::ty::EVENTS => {
                        if p::decode_events_into(body, &mut events).is_err() {
                            return false;
                        }
                        for e in &events {
                            // ms depuis 1970 → l'horloge du plugin
                            if !ev.push(Wire { t: e.t_ms - off, n: e.n, len: e.len, data: e.data }) {
                                sh2.overflow.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        true
                    }
                    p::ty::BYE => false,
                    _ => true,
                });
                emitted.clear();
                while let Some(r) = reports.pop() {
                    emitted.push(p::Emitted { n: r.n, t_ms: r.t + offset, offset: r.offset, frames: r.frames, late_ms: r.late_ms });
                    if emitted.len() == p::MAX_EVENTS {
                        p::encode_emitted(&emitted, &mut out);
                        emitted.clear();
                    }
                }
                if !emitted.is_empty() {
                    p::encode_emitted(&emitted, &mut out);
                }
                if alive && !send(c, &mut out) {
                    alive = false;
                }
            }
            (_, None) => drain(&mut io),
        }
        // toutes les 2 s ; (26 oct.) et tout de suite (100 ms au plus souvent) quand le tempo ou la lecture change : le
        // studio suit le tempo du DAW par cette trame (FL Studio sans script)
        let tp = sh.transport_key();
        let transport_moved = conn.is_some() && sent_transport != Some(tp) && last_stats.elapsed() >= Duration::from_millis(100);
        if alive && (last_stats.elapsed() >= Duration::from_secs(2) || transport_moved) {
            last_stats = Instant::now();
            sent_transport = Some(tp);
            if let Some(c) = conn.as_mut() {
                p::encode_text(p::ty::STATS, &sh.stats_json(), &mut out);
                alive = send(c, &mut out);
            }
        }
        if !alive && conn.is_some() {
            conn = None;
            out.clear();
            sh.connected.store(false, Ordering::SeqCst);
            sh.set_status("the companion went away: retrying".into());
            next_try = Instant::now() + backoff;
            if let Io::Notes { events: ev, .. } = &mut io {
                // les notes tenues sont relachees par le fil audio
                let _ = ev.push(Wire { len: 0, ..Default::default() });
            }
        }
        match (&io, conn.is_some()) {
            // Notes connecte : la lecture attend deja (delai de lecture du socket) ; un petit repos quand meme
            (Io::Notes { .. }, true) => std::thread::sleep(Duration::from_micros(500)),
            (_, true) => std::thread::sleep(Duration::from_millis(1)),
            (_, false) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// Sans connexion : on vide ce que le fil audio a laisse (Return ne remplit plus la file, mais un reste peut y etre).
fn drain(io: &mut Io) {
    match io {
        Io::Return { headers, samples } => {
            while headers.pop().is_some() {}
            let k = samples.len();
            samples.skip(k);
        }
        Io::Notes { reports, .. } => while reports.pop().is_some() {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_file_gives_port_and_secret_never_a_host() {
        let dir = std::env::temp_dir().join(format!("uwl-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("state.json");
        let secret = "ab".repeat(32);
        std::fs::write(&f, format!(r#"{{"port":47415,"token":"x","pluginPort":47416,"pluginSecret":"{secret}","pluginHost":"10.0.0.1"}}"#)).unwrap();
        let info = read_link(&f).unwrap();
        assert_eq!(info.port, 47416);
        assert_eq!(info.secret, [0xab; 32]);
        std::fs::write(&f, r#"{"port":47415,"token":"x"}"#).unwrap();
        assert!(read_link(&f).unwrap_err().contains("older companion"));
        std::fs::write(&f, r#"{"pluginPort":70000,"pluginSecret":"00"}"#).unwrap();
        assert!(read_link(&f).is_err());
        assert!(read_link(&dir.join("missing.json")).unwrap_err().contains("no companion state file"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (25 oct.) un essai reel : un Return charge AVANT le compagnon : pas de fichier d'etat, puis un port mort (un
    /// compagnon d'avant), puis le bon : le plugin se connecte tout seul, en relisant le fichier a chaque essai.
    #[test]
    fn a_plugin_loaded_before_the_companion_connects_when_it_appears() {
        use std::net::TcpListener;
        let dir = std::env::temp_dir().join(format!("uwl-late-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = dir.join("state.json");
        let _ = std::fs::remove_file(&state);
        // (le seul essai de ce crate qui passe par state_path())
        std::env::set_var("UWL_STATE_FILE", &state);
        let sh = Arc::new(Shared::new(p::Kind::Return, Arc::new(AtomicU64::new(0x1a7e))));
        let (mut hp, headers) = crate::ring::ring::<BlockHeader>(64);
        let (mut sp, samples) = crate::ring::ring::<f32>(4096);
        sh.active.store(true, Ordering::SeqCst);
        let net = NetThread::start(sh.clone(), Io::Return { headers, samples });
        let wait = |what: &dyn Fn() -> bool, secs: u64| {
            let t0 = Instant::now();
            while !what() {
                assert!(t0.elapsed() < Duration::from_secs(secs), "timed out: {}", sh.status());
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        wait(&|| sh.status().contains("no companion state file"), 3);
        // un port ou personne n'ecoute
        let dead = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let dead_port = dead.local_addr().unwrap().port();
        drop(dead);
        let secret = [0x3c; p::SECRET_LEN];
        std::fs::write(&state, format!(r#"{{"pluginPort":{dead_port},"pluginSecret":"{}"}}"#, p::secret_hex(&secret))).unwrap();
        wait(&|| sh.status().contains(&format!("127.0.0.1:{dead_port}")), 4);
        assert!(!sh.connected());
        // (29 oct., audit PL-1) un programme qui ecoute sur le port du fichier sans connaitre le secret (le faux
        // compagnon de l'audit : WELCOME 0 tout de suite) : le plugin ne s'y connecte jamais, et le son du DAW qui arrive
        // pendant ce temps ne lui est jamais envoye
        let liar = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let liar_port = liar.local_addr().unwrap().port();
        liar.set_nonblocking(true).unwrap();
        let liar_stop = Arc::new(AtomicBool::new(false));
        let ls = liar_stop.clone();
        let liar_srv = std::thread::spawn(move || {
            let (mut hellos, mut after) = (0, Vec::new());
            while !ls.load(Ordering::SeqCst) {
                let Ok((mut s, _)) = liar.accept() else {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                };
                s.set_nonblocking(false).unwrap();
                s.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
                let mut rd = p::FrameReader::new(p::MAX_FRAME);
                if next_frame(&mut s, &mut rd).map(|f| f.0) == Some(p::ty::HELLO) {
                    hellos += 1;
                    let mut out = Vec::new();
                    p::encode_welcome(&p::Welcome { version: p::VERSION, status: p::status::OK, message: "welcome".into() }, &mut out);
                    let _ = s.write_all(&out);
                    while let Some((t, _)) = next_frame(&mut s, &mut rd) {
                        after.push(t);
                    }
                }
            }
            (hellos, after)
        });
        std::fs::write(&state, format!(r#"{{"pluginPort":{liar_port},"pluginSecret":"{}"}}"#, p::secret_hex(&secret))).unwrap();
        let t0 = Instant::now();
        let mut refused_at: Option<Instant> = None;
        // jusqu'a 300 ms apres le premier refus (le plugin reessaie toutes les 2 s au plus)
        while refused_at.map(|t| t.elapsed() < Duration::from_millis(300)).unwrap_or(true) {
            assert!(t0.elapsed() < Duration::from_secs(6), "timed out: {}", sh.status());
            // le fil audio : des blocs de 128 trames, comme un DAW qui joue
            if hp.free() > 0 && sp.free() >= 256 {
                sp.push_slice(&[0.25f32; 256]);
                hp.push(BlockHeader { pos: 0, t: 0.0, frames: 128, flags: 0 });
            }
            assert!(!sh.connected(), "connected to a program that proved nothing");
            if refused_at.is_none() && sh.status().contains("did not prove") {
                refused_at = Some(Instant::now());
            }
            std::thread::sleep(Duration::from_millis(3));
        }
        liar_stop.store(true, Ordering::SeqCst);
        let (hellos, after) = liar_srv.join().unwrap();
        assert!(hellos >= 1, "the plugin did try");
        assert!(after.is_empty(), "nothing after its HELLO (no AUTH, no AUDIO): {after:x?}");
        assert!(sh.status().contains("did not prove"), "{}", sh.status());
        // le compagnon apparait (un autre port, ecrit dans le fichier)
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let srv = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut rd = p::FrameReader::new(p::MAX_HELLO);
            let hello = v2_handshake(&mut s, &mut rd, &secret, port, p::status::OK);
            std::thread::sleep(Duration::from_millis(300));
            hello
        });
        std::fs::write(&state, format!(r#"{{"pluginPort":{port},"pluginSecret":"{}"}}"#, p::secret_hex(&secret))).unwrap();
        wait(&|| sh.connected(), 5);
        let hello = srv.join().unwrap();
        assert_eq!((hello.kind, hello.instance), (p::Kind::Return, 0x1a7e));
        drop(net);

        // (25 oct.) un Notes annonce son numero « Link tag » a la connexion, puis quand le script de Live le change
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        std::fs::write(&state, format!(r#"{{"pluginPort":{port},"pluginSecret":"{}"}}"#, p::secret_hex(&secret))).unwrap();
        let ns = Arc::new(Shared::new(p::Kind::Notes, Arc::new(AtomicU64::new(0x7a6))));
        let (evp, _evc) = crate::ring::ring::<Wire>(64);
        let (_rp, rc) = crate::ring::ring::<Report>(64);
        let ns2 = ns.clone();
        let srv = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut rd = p::FrameReader::new(p::MAX_FRAME);
            v2_handshake(&mut s, &mut rd, &secret, port, p::status::OK);
            s.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
            let mut buf = [0u8; 4096];
            let mut tags = Vec::new();
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_secs(5) && tags.len() < 2 {
                while let Ok(Some((t, body))) = rd.next() {
                    if t == p::ty::TAG {
                        tags.push(p::decode_tag(&body).unwrap());
                        if tags.len() == 1 {
                            ns2.tag.store(17, Ordering::Relaxed); // le script ecrit le parametre
                        }
                    }
                }
                if let Ok(n) = s.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    rd.extend(&buf[..n]);
                }
            }
            tags
        });
        ns.active.store(true, Ordering::SeqCst);
        let net = NetThread::start(ns.clone(), Io::Notes { events: evp, reports: rc });
        let tags = srv.join().unwrap();
        assert_eq!(tags, vec![0, 17], "the tag at connect, then its change");
        drop(net);
        std::env::remove_var("UWL_STATE_FILE");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// La trame suivante (None : fermee, ou rien dans le delai de lecture du socket).
    fn next_frame(s: &mut TcpStream, rd: &mut p::FrameReader) -> Option<(u8, Vec<u8>)> {
        let mut buf = [0u8; 4096];
        loop {
            if let Ok(Some(f)) = rd.next() {
                return Some(f);
            }
            match s.read(&mut buf) {
                Ok(n) if n > 0 => rd.extend(&buf[..n]),
                _ => return None,
            }
        }
    }

    /// (29 oct.) Un faux compagnon v2 qui connait le secret : HELLO → CHALLENGE (la preuve juste pour `port`) → AUTH
    /// (verifiee ici) → WELCOME `status`. → le HELLO du plugin.
    fn v2_handshake(s: &mut TcpStream, rd: &mut p::FrameReader, secret: &[u8; 32], port: u16, status: u8) -> p::Hello {
        let (t, body) = next_frame(s, rd).expect("hello");
        assert_eq!(t, p::ty::HELLO);
        let hello = p::decode_hello(&body).unwrap();
        assert_eq!(hello.version, p::VERSION);
        assert_ne!(&hello.nonce, secret, "the secret never travels");
        let nc = [0x6b; 32];
        let mut out = Vec::new();
        p::encode_challenge(&p::Challenge { nonce: nc, proof: p::auth::companion_proof(secret, port, &hello.nonce, &nc) }, &mut out);
        s.write_all(&out).unwrap();
        let (t, body) = next_frame(s, rd).expect("auth");
        assert_eq!(t, p::ty::AUTH, "the plugin answers the challenge");
        assert!(p::auth::plugin_proof_ok(secret, port, &nc, &hello.nonce, &body), "the plugin's proof is right");
        out.clear();
        p::encode_welcome(&p::Welcome { version: p::VERSION, status, message: "welcome".into() }, &mut out);
        s.write_all(&out).unwrap();
        hello
    }

    /// Un faux compagnon sur un port au hasard : `answer` repond au HELLO comme il veut ; → le port, et (dans le fil)
    /// les types des trames recues APRES le HELLO, jusqu'a la fermeture.
    fn fake_companion(answer: impl FnOnce(&mut TcpStream, &p::Hello, u16) + Send + 'static) -> (u16, std::thread::JoinHandle<(p::Hello, Vec<u8>)>) {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut rd = p::FrameReader::new(p::MAX_FRAME);
            let (t, body) = next_frame(&mut s, &mut rd).expect("hello");
            assert_eq!(t, p::ty::HELLO);
            let hello = p::decode_hello(&body).unwrap();
            answer(&mut s, &hello, port);
            let mut after = Vec::new();
            while let Some((t, _)) = next_frame(&mut s, &mut rd) {
                after.push(t);
            }
            (hello, after)
        });
        (port, h)
    }

    fn send_welcome(s: &mut TcpStream, status: u8) {
        let mut out = Vec::new();
        p::encode_welcome(&p::Welcome { version: p::VERSION, status, message: "welcome".into() }, &mut out);
        let _ = s.write_all(&out);
    }

    fn send_challenge(s: &mut TcpStream, nonce: [u8; 32], proof: [u8; 32]) {
        let mut out = Vec::new();
        p::encode_challenge(&p::Challenge { nonce, proof }, &mut out);
        let _ = s.write_all(&out);
    }

    /// (29 oct., audit PL-1, I-6) Un programme qui ne prouve pas qu'il connait le secret n'obtient RIEN : ni la preuve du
    /// plugin, ni une trame de plus ; son « ok » ou son « deja connectee » ne valent rien (l'identite ne change pas) ;
    /// une preuve fausse, rejouee (un ancien nonce du plugin), faite pour un autre port (un relais) ou absente : refusee.
    #[test]
    fn a_companion_that_does_not_prove_itself_gets_nothing() {
        let secret = [0x41u8; 32];
        let sh = Shared::new(p::Kind::Notes, Arc::new(AtomicU64::new(0x5eed)));
        type Answer = Box<dyn FnOnce(&mut TcpStream, &p::Hello, u16) + Send>;
        let cases: Vec<(&str, Answer)> = vec![
            // le faux compagnon de l'audit : WELCOME 0 tout de suite, puis des notes
            (
                "welcome ok at once, then notes",
                Box::new(|s, _, _| {
                    send_welcome(s, p::status::OK);
                    let mut out = Vec::new();
                    p::encode_events(&[p::Event { t_ms: 1.0, n: 1, len: 3, data: [0x90, 60, 100] }], &mut out);
                    let _ = s.write_all(&out);
                }),
            ),
            ("duplicate at once", Box::new(|s, _, _| send_welcome(s, p::status::DUPLICATE))),
            ("a random proof", Box::new(|s, _, _| send_challenge(s, [1; 32], [2; 32]))),
            ("a challenge replayed from an older connection", Box::new(move |s, _, port| send_challenge(s, [3; 32], p::auth::companion_proof(&secret, port, &[0x11; 32], &[3; 32])))),
            ("the right proof for another port (a relay)", Box::new(move |s, h, port| send_challenge(s, [4; 32], p::auth::companion_proof(&secret, port.wrapping_add(1), &h.nonce, &[4; 32])))),
            ("the plugin's own proof sent back (a reflection)", Box::new(move |s, h, port| send_challenge(s, [5; 32], p::auth::plugin_proof(&secret, port, &h.nonce, &[5; 32])))),
            ("nothing at all", Box::new(|_, _, _| std::thread::sleep(Duration::from_millis(2300)))),
        ];
        for (what, answer) in cases {
            let (port, srv) = fake_companion(answer);
            let r = connect_to(&sh, &LinkInfo { port, secret });
            let e = r.err().unwrap_or_else(|| panic!("{what}: connected"));
            assert!(e.0.contains("did not prove") && !e.1, "{what}: {e:?}");
            let (hello, after) = srv.join().unwrap();
            assert!(after.is_empty(), "{what}: the plugin sent more after its HELLO: {after:x?}");
            assert_ne!(hello.nonce, secret, "{what}: the secret never travels");
            assert_eq!(sh.instance_id(), 0x5eed, "{what}: the identity did not change");
        }
    }

    /// Le bon compagnon : sa preuve, puis celle du plugin, puis le verdict ; « deja connectee » APRES la preuve donne une
    /// identite neuve (comme avant) ; un compagnon d'avant (v1 : il prend le nonce pour un secret faux) se dit.
    #[test]
    fn a_companion_that_proves_itself_gets_the_plugin_proof_then_its_verdict() {
        let secret = [0x42u8; 32];
        let sh = Shared::new(p::Kind::Return, Arc::new(AtomicU64::new(0xd0d0)));
        let (port, srv) = fake_companion(move |s, h, port| {
            let nc = [0x77; 32];
            send_challenge(s, nc, p::auth::companion_proof(&secret, port, &h.nonce, &nc));
            let mut rd = p::FrameReader::new(p::MAX_HELLO);
            let (t, body) = next_frame(s, &mut rd).expect("auth");
            assert_eq!(t, p::ty::AUTH);
            assert!(p::auth::plugin_proof_ok(&secret, port, &nc, &h.nonce, &body));
            send_welcome(s, p::status::OK);
        });
        let c = connect_to(&sh, &LinkInfo { port, secret }).expect("connected");
        drop(c);
        srv.join().unwrap();
        // deux connexions : deux nonces differents
        let (port, srv) = fake_companion(move |s, h, port| {
            let nc = [0x78; 32];
            send_challenge(s, nc, p::auth::companion_proof(&secret, port, &h.nonce, &nc));
            let mut rd = p::FrameReader::new(p::MAX_HELLO);
            assert_eq!(next_frame(s, &mut rd).map(|f| f.0), Some(p::ty::AUTH));
            send_welcome(s, p::status::DUPLICATE);
        });
        let e = connect_to(&sh, &LinkInfo { port, secret }).err().unwrap();
        assert!(e.0.contains("new identity") && e.1, "{e:?}");
        assert_ne!(sh.instance_id(), 0xd0d0, "a proven duplicate: a new identity");
        let (hello2, _) = srv.join().unwrap();
        assert_eq!(hello2.instance, 0xd0d0);
        // un compagnon d'avant (v1) : « mauvais secret » au HELLO v2
        let (port, srv) = fake_companion(|s, _, _| send_welcome(s, p::status::BAD_SECRET));
        let e = connect_to(&sh, &LinkInfo { port, secret }).err().unwrap();
        assert!(e.0.contains("older companion"), "{e:?}");
        srv.join().unwrap();
    }

    #[test]
    fn instance_ids_differ() {
        let a = new_instance_id();
        let b = new_instance_id();
        assert_ne!(a, 0);
        assert_ne!(a, b);
    }

    #[test]
    fn stats_are_json() {
        let sh = Shared::new(p::Kind::Notes, Arc::new(AtomicU64::new(0xbeef)));
        sh.jitter_us.store(1234, Ordering::Relaxed);
        let v: serde_json::Value = serde_json::from_str(&sh.stats_json()).unwrap();
        assert_eq!(v["kind"], "notes");
        assert_eq!(v["jitterMs"], 1.234);
        assert_eq!(sh.display_name(), "Uppwind Link Notes beef");
        // (26 oct.) le transport : inconnu tant que le DAW n'a rien dit, puis son tempo et sa lecture
        assert_eq!((v["tempo"].is_null(), v["playing"].is_null()), (true, true));
        sh.note_transport(Some(128.5), true);
        let v: serde_json::Value = serde_json::from_str(&sh.stats_json()).unwrap();
        assert_eq!((v["tempo"].as_f64(), v["playing"].as_bool()), (Some(128.5), Some(true)));
        sh.note_transport(None, false);
        let v: serde_json::Value = serde_json::from_str(&sh.stats_json()).unwrap();
        assert_eq!((v["tempo"].is_null(), v["playing"].as_bool()), (true, Some(false)));
        for bad in [Some(f64::NAN), Some(0.0), Some(-120.0), Some(1e9)] {
            assert_eq!(tempo_milli(bad), 0, "{bad:?}");
        }
        assert_eq!(tempo_milli(Some(97.3333)), 97333);
    }
}
