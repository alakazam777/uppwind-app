// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (19 nov., etape 1 de « le compagnon comme hote VST3 », a-lire/hote-vst3-compagnon-19nov.md) Le processus enfant du
//! compagnon : UN plugin par processus (un plugin qui plante ne tue ni le compagnon ni les autres).
//!
//!   uppwind-companion --plug-scan <module.vst3>
//!       charge le module (sans creer d'instance), ecrit ses classes en JSON sur la sortie, s'arrete
//!   uppwind-companion --plug-host --module <module.vst3> --cid <32 hex> --tag <1..255> --link-bundle <Uppwind Link.vst3>
//!                     --states-dir <dossier> [--state <fichier>] [--rate 48000] [--block 256]
//!       joue le plugin. Le chemin mesure par l'essai (2,2 ms en lecture, 7,2 ms au clavier) : nos plugins Uppwind Link,
//!       charges ici comme un DAW le ferait — Notes (son « Link tag » = --tag : le studio envoie a « tag:<n> ») →
//!       l'instrument → Return (le son vers le compagnon). UWL_STATE_FILE (pose par le compagnon) leur dit ou il ecoute.
//!       L'editeur : une fenetre normale, POSSEDEE par une fenetre cachee (pas de bouton dans la barre des taches),
//!       MASQUEE par DWM (invisible, mais WGC la filme et les gestes passent : l'essai du 19 nov.). Hors de l'ecran,
//!       jamais (le plugin ne se dessine plus). Pas de carte son : un fil cadence par l'horloge, comme uwl-host.
//!
//! Le dialogue avec le compagnon : des lignes JSON. Entree : {"cmd":"save","n":1} · {"cmd":"editor","show":true|false}
//! · {"cmd":"view","open":true|false} (l'editeur ouvert, MASQUE : le studio le filme) · {"cmd":"tempo","bpm":120,"playing":true} · {"cmd":"mix","vol":1,"pan":0,"mute":false} · {"cmd":"quit"} ; la fin de l'entree (le compagnon est parti) = quit.
//! (20 nov.) macOS : {"cmd":"mouse"|"wheel"|"key",…} (les gestes du studio, poses dans la file d'AppKit : macui::gesture).
//! Sortie : {"event":"ready",…} · {"event":"saved","n","sha","bytes"} · {"event":"editor","shown"} · {"event":"error",…}
//! · {"event":"closed"}. L'etat d'un plugin (composant + controleur) est ecrit par CE processus dans --states-dir, sous
//! son empreinte SHA-256 : seul un etat que le compagnon a produit lui-meme est jamais relu (--state), jamais des octets
//! venus d'un projet partage.

use crate::realhost::{classes, Engine, Frame, Instance};
use crate::vsthost::{Block, Module};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{ComPtr, ComWrapper};

#[cfg(windows)]
use self::win as ui;
#[cfg(target_os = "macos")]
use crate::macui as ui;

/// (20 nov., l'hote macOS) Ce que la boucle de l'interface recoit en plus des evenements du systeme : un ordre du
/// compagnon en attente, la fin, la croix de la fenetre (montree sur le bureau).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiEv {
    Cmd,
    Quit,
    HideEditor,
}

pub(crate) fn out(v: Value) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

/// Le texte hexadecimal d'un identifiant de classe (16 octets → 32 chiffres minuscules).
pub fn cid_hex(cid: &TUID) -> String {
    cid.iter().map(|b| format!("{:02x}", *b as u8)).collect()
}

fn parse_cid(s: &str) -> Option<TUID> {
    let s = s.trim();
    if s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut t: TUID = [0; 16];
    for i in 0..16 {
        t[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()? as _;
    }
    Some(t)
}

// ── L'etat sur disque : « UWS1 », le composant, le controleur ───────────────────────────────────────────────────

pub fn pack_state(comp: &[u8], ctrl: Option<&[u8]>) -> Vec<u8> {
    let mut v = Vec::with_capacity(12 + comp.len() + ctrl.map(|c| c.len()).unwrap_or(0));
    v.extend_from_slice(b"UWS1");
    v.extend_from_slice(&(comp.len() as u32).to_le_bytes());
    v.extend_from_slice(comp);
    match ctrl {
        Some(c) => {
            v.extend_from_slice(&(c.len() as u32).to_le_bytes());
            v.extend_from_slice(c);
        }
        None => v.extend_from_slice(&u32::MAX.to_le_bytes()),
    }
    v
}

pub fn unpack_state(b: &[u8]) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
    if b.len() < 12 || &b[..4] != b"UWS1" {
        return None;
    }
    let n = u32::from_le_bytes(b[4..8].try_into().ok()?) as usize;
    let comp = b.get(8..8 + n)?.to_vec();
    let at = 8 + n;
    let m = u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?);
    let ctrl = if m == u32::MAX { None } else { Some(b.get(at + 4..at + 4 + m as usize)?.to_vec()) };
    Some((comp, ctrl))
}

pub fn sha256_hex(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// Un nom d'etat valide : 64 chiffres hexadecimaux (rien d'autre n'est jamais lu dans --states-dir).
pub fn is_state_name(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

// ── --plug-scan ─────────────────────────────────────────────────────────────────────────────────────────────────

/// (23 nov., 0.4.14, relecture macOS « un scan bloque survit au compagnon ») Le scan s'arrete de lui-meme passe ce delai
/// (le compagnon l'abandonne a 25 s) : sur Mac il n'y a pas de Job object, un module bloque ne tourne jamais plus.
pub const SCAN_SELF_LIMIT: Duration = Duration::from_secs(40);

/// Un arret immediat, sans destructeurs ni gestionnaires de sortie du plugin (Windows : TerminateProcess ; Mac : _exit).
fn hard_exit(code: i32) -> ! {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        TerminateProcess(GetCurrentProcess(), code as u32);
    }
    #[cfg(unix)]
    unsafe {
        extern "C" {
            fn _exit(code: i32) -> !;
        }
        _exit(code);
    }
    #[allow(unreachable_code)]
    std::process::abort()
}

pub fn scan_main(module: &Path) -> i32 {
    std::thread::spawn(|| {
        std::thread::sleep(SCAN_SELF_LIMIT);
        hard_exit(3);
    });
    safe_dll_search();
    match Module::load(module) {
        Ok(m) => {
            let list: Vec<Value> = classes(&m)
                .iter()
                .filter(|c| c.category == "Audio Module Class")
                .map(|c| json!({ "cid": cid_hex(&c.cid), "name": c.name, "vendor": c.vendor, "version": c.version, "sub": c.sub }))
                .collect();
            out(json!({ "ok": true, "classes": list }));
            // le module n'est pas decharge : le processus s'arrete (certains plugins plantent en ExitDll)
            std::process::exit(0);
        }
        Err(e) => {
            out(json!({ "ok": false, "error": e }));
            1
        }
    }
}

/// (23 nov., 0.4.14, relecture « l'ordre de recherche des DLL ») Windows : le dossier courant retire de la recherche des
/// DLL (SetDllDirectoryW("")) avant de charger un module ; ses propres DLL restent cherchees d'abord dans son dossier.
pub fn safe_dll_search() {
    #[cfg(windows)]
    unsafe {
        let empty = [0u16];
        windows_sys::Win32::System::LibraryLoader::SetDllDirectoryW(empty.as_ptr());
    }
}

/// (23 nov., 0.4.14) La plus longue ligne du compagnon (un paquet de son en direct : 512 trames stereo en base64 en font
/// 5 500 octets environ).
pub const LINE_MAX: usize = 16384;

/// (23 nov., 0.4.14, relecture « l'enfant lit une ligne entiere avant de verifier sa taille ») La ligne suivante dans
/// `buf`, sans sa fin (LF ou CR LF), en ne gardant jamais plus de `max` octets : Some(true) une ligne, Some(false) une
/// ligne trop longue (jetee jusqu'a sa fin, `buf` vide), None la fin de l'entree.
pub fn next_line<R: BufRead>(r: &mut R, max: usize, buf: &mut Vec<u8>) -> std::io::Result<Option<bool>> {
    buf.clear();
    let mut over = false;
    let mut any = false;
    loop {
        let avail = match r.fill_buf() {
            Ok(a) => a,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if avail.is_empty() {
            // la fin : une derniere ligne sans LF compte encore
            return Ok(if !any { None } else { Some(fin(buf, over, max)) });
        }
        any = true;
        let (chunk, done) = match avail.iter().position(|b| *b == b'\n') {
            Some(i) => (&avail[..i], Some(i + 1)),
            None => (avail, None),
        };
        if !over {
            if buf.len() + chunk.len() > max + 1 {
                over = true;
                buf.clear();
            } else {
                buf.extend_from_slice(chunk);
            }
        }
        let used = done.unwrap_or(avail.len());
        r.consume(used);
        if done.is_some() {
            return Ok(Some(fin(buf, over, max)));
        }
    }
}

/// la fin d'une ligne : sans son CR ; plus longue que `max`, jetee
fn fin(buf: &mut Vec<u8>, over: bool, max: usize) -> bool {
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    if over || buf.len() > max {
        buf.clear();
        return false;
    }
    true
}

/// (20 nov., relecture) le delai apres la fermeture de l'entree, avant que l'enfant ne s'arrete de lui-meme
const QUIT_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

pub(crate) static VIEW: std::sync::atomic::AtomicPtr<IPlugView> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

/// (19 nov., etape 3b) Un EFFET EN DIRECT : le son du studio arrive date (sa trame dans le contexte du studio), il ressort
/// avec la MEME date (moins la latence du plugin) : le studio le replace au bon endroit, avec un retard fixe (il n'y a
/// pas d'horloge ici, le studio la donne). Les paquets ne passent jamais par la boucle de l'interface : le fil qui lit
/// l'entree les pose dans `q` (bornee : au-dela, ils sont jetes et comptes), le fil du direct les traite.
pub struct Live {
    q: Mutex<VecDeque<(u64, Vec<f32>)>>,
    cv: Condvar,
    on: AtomicBool,
    quit: AtomicBool,
    gen: AtomicU32,
    rate_bits: AtomicU64,
    latency: AtomicU32,
    /// le processeur : le fil du direct ou le rendu (et ses arrets / demarrages), jamais les deux a la fois
    busy: Mutex<()>,
    dropped: AtomicU64,
    done: AtomicU64,
    /// les bus du plugin (connus a son demarrage) et le numero de ce demarrage : le fil du direct refait ses tampons
    buses: Mutex<(Vec<i32>, Vec<i32>)>,
    starts: AtomicU32,
    /// (19 nov., la mesure en direct) les ancres du morceau envoyees par le studio (« song »), rangees par `from`
    anchors: Mutex<Anchors>,
}

/// (19 nov., la mesure en direct) Une ancre du morceau : a partir de la trame `from` du worklet du studio (celle dont
/// datent les paquets), la trame du morceau est trame + `offset`, en lecture ; a l'arret, elle reste a from + offset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub from: i64,
    pub offset: i64,
    pub playing: bool,
}

/// Les ancres a venir et celle en cours. Avant la premiere : la trame du worklet, en lecture (le direct d'avant).
#[derive(Default, Debug)]
pub struct Anchors(Vec<Anchor>);

/// Au plus tant d'ancres gardees (une par saut ; le studio n'en programme que quelques-unes d'avance).
const ANCHORS_MAX: usize = 64;

impl Anchors {
    /// Une nouvelle ancre : celles programmees a partir de sa trame ou apres sont remplacees (un saut reprogramme).
    pub fn push(&mut self, a: Anchor) {
        self.0.retain(|x| x.from < a.from);
        self.0.push(a);
        if self.0.len() > ANCHORS_MAX {
            let k = self.0.len() - ANCHORS_MAX;
            self.0.drain(..k);
        }
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Le bloc [pos, pos + n) du worklet, coupe aux ancres qui y tombent : (debut dans le bloc, longueur, trame du
    /// morceau au debut de ce morceau-la ou None (pas d'ancre : la trame du worklet), en lecture). Les ancres passees
    /// (une suivante deja atteinte) sont oubliees.
    pub fn spans(&mut self, pos: i64, n: usize) -> Vec<(usize, usize, Option<i64>, bool)> {
        while self.0.len() >= 2 && self.0[1].from <= pos {
            self.0.remove(0);
        }
        let mut v = Vec::with_capacity(2);
        let end = pos + n as i64;
        let mut at = pos;
        let mut i = 0usize;
        // l'ancre en cours a `at` : la derniere dont from <= at
        let mut cur: Option<Anchor> = None;
        while i < self.0.len() && self.0[i].from <= at {
            cur = Some(self.0[i]);
            i += 1;
        }
        while at < end {
            let next = if i < self.0.len() { self.0[i].from.min(end) } else { end };
            let song = cur.map(|a| if a.playing { at + a.offset } else { a.from + a.offset });
            let playing = cur.map(|a| a.playing).unwrap_or(true);
            if next > at {
                v.push(((at - pos) as usize, (next - at) as usize, song, playing));
            }
            at = next;
            if i < self.0.len() && self.0[i].from <= at {
                cur = Some(self.0[i]);
                i += 1;
            }
        }
        v
    }
}

/// Les paquets en attente, au plus (5 ms chacun : 2 s).
const LIVE_QUEUE: usize = 400;
/// Le plus grand bloc du direct (un paquet du studio : 240 trames).
const LIVE_BLOCK: usize = 512;

struct SendBox<T>(T);
unsafe impl<T> Send for SendBox<T> {}

/// Ce que le fil audio lit a chaque bloc (le fil de l'interface l'ecrit).
struct Transport {
    tempo_bits: AtomicU64,
    playing: AtomicBool,
    /// (le studio, SET) les gains gauche et droite de la piste (volume et pan ; 0 : coupee), appliques avant le Return
    gain_l: AtomicU64,
    gain_r: AtomicU64,
}

/// Le volume (gain lineaire, 1 = 0 dB) et le pan (-1..1, une balance) d'une piste du studio → (gauche, droite).
pub fn gains(vol: f64, pan: f64, mute: bool) -> (f64, f64) {
    if mute {
        return (0.0, 0.0);
    }
    let v = vol.clamp(0.0, 2.0);
    let p = pan.clamp(-1.0, 1.0);
    (v * (1.0 - p).min(1.0), v * (1.0 + p).min(1.0))
}

pub struct HostArgs {
    pub module: PathBuf,
    pub cid: String,
    /// (19 nov., etape 3) un EFFET rendu hors temps reel (« render ») : ni Notes, ni Return, ni fil audio, ni tag
    pub effect: bool,
    pub tag: u32,
    pub link_bundle: PathBuf,
    pub states_dir: PathBuf,
    pub state: Option<PathBuf>,
    pub rate: f64,
    pub block: usize,
}

impl HostArgs {
    /// Les arguments apres « --plug-host ».
    pub fn parse(a: &[String]) -> Result<HostArgs, String> {
        let get = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned();
        let need = |k: &str| get(k).ok_or(format!("{k} is missing"));
        let effect = a.iter().any(|x| x == "--effect");
        Ok(HostArgs {
            module: PathBuf::from(need("--module")?),
            cid: need("--cid")?,
            effect,
            tag: if effect { 0 } else { need("--tag")?.parse().ok().filter(|t| (1..=255).contains(t)).ok_or("--tag is 1..255")? },
            link_bundle: if effect { PathBuf::new() } else { PathBuf::from(need("--link-bundle")?) },
            states_dir: PathBuf::from(need("--states-dir")?),
            state: get("--state").map(PathBuf::from),
            rate: get("--rate").and_then(|s| s.parse().ok()).filter(|r: &f64| (8000.0..=192000.0).contains(r)).unwrap_or(48000.0),
            block: get("--block").and_then(|s| s.parse().ok()).filter(|b: &usize| (16..=4096).contains(b)).unwrap_or(256),
        })
    }
}

/// (20 nov., relecture) les reglages ecrits d'un coup : un fichier temporaire du dossier, puis renomme a son nom (une
/// ecriture coupee ne laisse jamais un fichier tronque sous le nom d'une empreinte).
fn write_state(dir: &Path, path: &Path, blob: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".tmp-{}-{}", std::process::id(), path.file_name().and_then(|n| n.to_str()).unwrap_or("state")));
    std::fs::write(&tmp, blob)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

pub fn host_main(a: HostArgs) -> i32 {
    safe_dll_search();
    // (20 nov., relecture de securite) une panique arrete l'enfant aussitot : derouler le fil de l'interface liberait le
    // plugin pendant que le fil audio ou celui du direct l'appelle encore (ses pointeurs bruts, SendBox). Seulement dans
    // l'enfant : les plugins Uppwind Link charges par un DAW gardent leur comportement.
    std::panic::set_hook(Box::new(|info| {
        eprintln!("plugin host: {info}");
        std::process::abort();
    }));
    match run(a) {
        Ok(()) => {
            out(json!({ "event": "closed" }));
            0
        }
        Err(e) => {
            out(json!({ "event": "error", "error": e, "fatal": true }));
            1
        }
    }
}

fn run(a: HostArgs) -> Result<(), String> {
    ui::init();
    let cid = parse_cid(&a.cid).ok_or("--cid is 32 hex digits")?;
    let (rate, block) = (a.rate, a.block);

    // ── le plugin ──
    let m = Module::load(&a.module)?;
    let info = classes(&m).into_iter().find(|c| c.cid == cid && c.category == "Audio Module Class").ok_or("this class is not in the module")?;
    let mut inst = Instance::create(&m, &cid)?;
    if let Some(p) = &a.state {
        // seulement un fichier de --states-dir, nomme par son empreinte, et dont le contenu a cette empreinte
        let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let inside = p.parent().map(|d| d == a.states_dir).unwrap_or(false);
        if !inside || !is_state_name(name) {
            return Err("--state must be a state file of --states-dir".into());
        }
        // (20 nov., relecture) un fichier abime (une ecriture coupee d'avant) : le plugin s'ouvre avec ses reglages par
        // defaut, et le dit ; jamais des octets qui ne sont pas les siens
        let ok = std::fs::read(p).ok().filter(|b| sha256_hex(b) == name).and_then(|b| unpack_state(&b));
        match ok {
            Some((c, k)) => inst.set_state(&c, k.as_deref())?,
            None => out(json!({ "event": "notice", "notice": "the saved settings of this plugin were damaged: it opens with its defaults" })),
        }
    }
    if !a.effect {
        inst.start(rate, block as i32)?;
    }

    // ── la fenetre et l'editeur (masques) ──
    let hwnd = ui::make_window(&format!("{}/Uppwind", info.name))?;
    let frame = ComWrapper::new(Frame { hwnd: AtomicIsize::new(hwnd as isize), resizes: AtomicU32::new(0) });
    // (19 nov.) l'editeur ne s'ouvre qu'a la demande (« editor » show) et se ferme quand on le cache : un editeur masque
    // continue de se dessiner (Vital : la moitie d'un coeur, l'essai du 19 nov.)
    let mut view: Option<ComPtr<IPlugView>> = None;
    // l'editeur est voulu par le studio (« view », pour le filmer) et/ou montre sur le bureau (« editor ») ; ferme quand
    // ni l'un ni l'autre
    let (mut film, mut shown) = (false, false);
    ui::park(hwnd);

    // ── le fil audio (un instrument) : nos plugins Notes (le Link tag) et Return autour de lui ──
    let lm = if a.effect { None } else { Some(Module::load(&a.link_bundle).map_err(|e| format!("the Uppwind Link plugins: {e}"))?) };
    let stop = Arc::new(AtomicBool::new(false));
    let transport = Arc::new(Transport { tempo_bits: AtomicU64::new(120f64.to_bits()), playing: AtomicBool::new(true), gain_l: AtomicU64::new(1f64.to_bits()), gain_r: AtomicU64::new(1f64.to_bits()) });
    let proc_ptr = SendBox(inst.processor.as_ptr());
    let handler_ptr = SendBox(&*inst.handler as *const crate::realhost::Handler);
    let (ins, outs) = (inst.ins.clone(), inst.outs.clone());
    let mut audio = None;
    if let Some(lm) = &lm {
    let mut notes = lm.create("Uppwind Link Notes")?;
    let (tag_id, _, steps) = notes.params().into_iter().find(|(_, t, _)| t == "Link tag").ok_or("Uppwind Link Notes has no Link tag")?;
    let tag_v = a.tag as f64 / steps.max(1) as f64;
    notes.set_param(tag_id, tag_v)?;
    notes.start(rate, block as i32, false)?;
    let mut ret = lm.create("Uppwind Link Return")?;
    ret.start(rate, block as i32, true)?;
    let link = SendBox((notes, ret));
    let (stop2, tr2) = (stop.clone(), transport.clone());
    audio = Some(std::thread::spawn(move || {
        let proc_ptr = proc_ptr;
        let handler_ptr = handler_ptr;
        let mut link = link;
        let h = unsafe { &*handler_ptr.0 };
        let mut eng = Engine::new(&ins, &outs, block, rate);
        let (mut nb, mut rb) = (Block::new(block, rate), Block::new(block, rate));
        nb.queue_param(tag_id, tag_v);
        ui::raise_priority();
        let start = Instant::now() + Duration::from_millis(50);
        let mut k = 0u64;
        let mut params: Vec<(u32, f64)> = Vec::new();
        while !stop2.load(Ordering::Relaxed) {
            let pos = k * block as u64;
            wait_until(start + Duration::from_secs_f64(pos as f64 / rate));
            let tempo = f64::from_bits(tr2.tempo_bits.load(Ordering::Relaxed));
            eng.ctx.tempo = tempo;
            nb.ctx.tempo = tempo;
            rb.ctx.tempo = tempo;
            nb.process(&link.0 .0, block, false, pos as i64, 0);
            {
                let mut evs = eng.in_events.events.lock().unwrap();
                for e in nb.out_events.events.lock().unwrap().iter() {
                    let mut e2 = *e;
                    e2.busIndex = 0;
                    evs.push(e2);
                }
            }
            params.clear();
            params.append(&mut h.pending.lock().unwrap());
            eng.process(proc_ptr.0, block, pos as i64, 0, &params);
            let (l, r) = eng.main_out();
            if l.len() >= block {
                let gl = f64::from_bits(tr2.gain_l.load(Ordering::Relaxed)) as f32;
                let gr = f64::from_bits(tr2.gain_r.load(Ordering::Relaxed)) as f32;
                for i in 0..block {
                    rb.inl[i] = l[i] * gl;
                    rb.inr[i] = r[i] * gr;
                }
            }
            rb.process(&link.0 .1, block, true, pos as i64, 0);
            k += 1;
        }
        link.0 .0.stop();
        link.0 .1.stop();
        drop(link);
    }));
    }

    out(json!({ "event": "ready", "pid": std::process::id(), "hwnd": ui::window_id(hwnd), "titlebar": ui::titlebar(hwnd), "name": info.name, "vendor": info.vendor, "sub": info.sub,
                "editor": inst.ctrl.is_some(), "params": inst.param_count(), "outs": inst.outs, "ins": inst.ins, "latency": inst.latency, "tag": a.tag, "effect": a.effect,
                // (20 nov., un projet recharge) la frequence du fil audio d'un instrument (--rate, sinon 48 kHz) : le studio
                // rouvre celui qui ne tourne pas a celle de son contexte (link-host.js needsReopen)
                "rate": if a.effect { Value::Null } else { json!(a.rate.round() as u32) } }));

    // ── (19 nov., etape 3b) le fil d'un effet en direct ──
    let live = Arc::new(Live {
        q: Mutex::new(VecDeque::new()),
        cv: Condvar::new(),
        on: AtomicBool::new(false),
        quit: AtomicBool::new(false),
        gen: AtomicU32::new(0),
        rate_bits: AtomicU64::new(rate.to_bits()),
        latency: AtomicU32::new(0),
        busy: Mutex::new(()),
        dropped: AtomicU64::new(0),
        done: AtomicU64::new(0),
        buses: Mutex::new((Vec::new(), Vec::new())),
        starts: AtomicU32::new(0),
        anchors: Mutex::new(Anchors::default()),
    });
    let live_thread = if a.effect {
        let lv = live.clone();
        let tr = transport.clone();
        let proc_ptr = SendBox(inst.processor.as_ptr());
        let handler_ptr = SendBox(&*inst.handler as *const crate::realhost::Handler);
        Some(std::thread::spawn(move || live_loop(lv, tr, proc_ptr, handler_ptr)))
    } else {
        None
    };

    // ── les ordres du compagnon (un fil lit l'entree, la boucle de l'interface les traite) ──
    let cmds: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
    {
        let cmds = cmds.clone();
        let hw = hwnd;
        let lv = live.clone();
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            let mut rd = stdin.lock();
            let mut buf = Vec::new();
            // (23 nov., 0.4.14) une ligne trop longue n'est jamais gardee en memoire : jetee au fil de la lecture
            while let Ok(Some(ok)) = next_line(&mut rd, LINE_MAX, &mut buf) {
                if !ok {
                    continue;
                }
                let Ok(l) = std::str::from_utf8(&buf) else { continue };
                let l = l.to_string();
                // (19 nov., etape 3b) le son d'un effet en direct : tout droit au fil du direct, jamais par l'interface
                if l.starts_with("{\"cmd\":\"in\"") {
                    live_in(&lv, &l);
                    continue;
                }
                // (20 nov., l'hote macOS) un geste du studio pour l'editeur : tout droit dans la file d'AppKit (macui::gesture),
                // jamais par la boucle de l'interface (un bouton qui suit la souris la tient dans sa propre boucle)
                #[cfg(target_os = "macos")]
                if l.starts_with("{\"cmd\":\"mouse\"") || l.starts_with("{\"cmd\":\"wheel\"") || l.starts_with("{\"cmd\":\"key\"") {
                    if let Ok(v) = serde_json::from_str::<Value>(&l) {
                        ui::gesture(&v);
                    }
                    continue;
                }
                #[cfg(target_os = "macos")]
                if l.starts_with("{\"cmd\":\"probe\"") {
                    ui::probe_menus();
                    continue;
                }
                #[cfg(target_os = "macos")]
                if l.starts_with("{\"cmd\":\"snap\"") {
                    ui::snap();
                    continue;
                }
                // (19 nov., la mesure en direct) une ancre du morceau : au fil du direct, avant les paquets qu'elle date
                if l.starts_with("{\"cmd\":\"song\"") {
                    live_song(&lv, &l);
                    continue;
                }
                if l.len() > 4096 {
                    continue;
                }
                cmds.lock().unwrap().push_back(l);
                ui::post(hw, UiEv::Cmd);
            }
            // le compagnon est parti (ou a ferme l'entree) : on s'arrete
            ui::post(hw, UiEv::Quit);
            // (20 nov., relecture) une boite de dialogue du plugin a sa propre boucle de messages et avale le notre : apres
            // QUIT_GRACE, l'enfant s'arrete de lui-meme (le Job object du compagnon le fait aussi quand celui-ci meurt ;
            // sur Mac, un menu du plugin tient de meme la boucle d'AppKit)
            std::thread::sleep(QUIT_GRACE);
            std::process::exit(0);
        });
    }
    // le dernier etat ecrit (empreinte, taille) et la marque des changements a ce moment-la
    let mut last_saved: Option<(String, usize)> = None;
    let mut last_mark = u64::MAX;
    // (20 nov., Link to controller) l'id → l'indice des parametres (rempli par « params », au fil de l'interface) ; le fil
    // du suivi envoie les gestes de l'editeur au compagnon (« param »), toutes les 80 ms, tant que le studio suit
    let pmap: Arc<Mutex<std::collections::HashMap<u32, u32>>> = Arc::new(Mutex::new(std::collections::HashMap::new()));
    {
        let (watch, touched, pm, st) = (inst.handler.watch.clone(), inst.handler.touched.clone(), pmap.clone(), stop.clone());
        std::thread::spawn(move || {
            while !st.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(80));
                if !watch.load(Ordering::Relaxed) {
                    continue;
                }
                let got: Vec<(u32, f64)> = std::mem::take(&mut *touched.lock().unwrap());
                if got.is_empty() {
                    continue;
                }
                let map = pm.lock().unwrap();
                let mut last: Vec<(u32, f64)> = Vec::new();
                for (id, v) in got {
                    let Some(&p) = map.get(&id) else { continue };
                    if let Some(e) = last.iter_mut().find(|e| e.0 == p) {
                        e.1 = v;
                    } else if last.len() < 128 {
                        last.push((p, v));
                    }
                }
                drop(map);
                if !last.is_empty() {
                    let items: Vec<Value> = last.iter().map(|(p, v)| json!({ "p": p, "v": if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 } })).collect();
                    out(json!({ "event": "param", "items": items }));
                }
            }
        });
    }
    'ui: loop {
        // (20 nov., l'hote macOS) pendant un menu du plugin (macui : la boucle de suivi du menu tient celle-ci) : ce qui ne
        // touche pas a la fenetre est traite tout de suite (« save » toutes les 15 s ne ferme pas le menu qu'on lit) ; au
        // premier autre ordre, le menu se ferme et la boucle reprend avec lui et ceux qui le suivent (leur ordre garde)
        let ev = {
            let mut in_menu = || loop {
                let mut q = cmds.lock().unwrap();
                let Some(line) = q.pop_front() else { break };
                let v: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                match v["cmd"].as_str().unwrap_or("") {
                    "save" => {
                        drop(q);
                        save_cmd(&inst, &a.states_dir, &mut last_saved, &mut last_mark, v["n"].clone());
                    }
                    "mix" | "tempo" => {
                        drop(q);
                        transport_cmd(&transport, &v);
                    }
                    "params" | "param-set" | "param-watch" => {
                        drop(q);
                        param_cmd(&inst, &v, &pmap);
                    }
                    _ => {
                        q.push_front(line);
                        drop(q);
                        ui::cancel_menu();
                        break;
                    }
                }
            };
            ui::next(&mut in_menu)
        };
        let Some(ev) = ev else { break };
        if ev == UiEv::Quit {
            break;
        }
        if ev == UiEv::HideEditor {
            // la croix (fenetre montree sur le bureau) : masquee, le plugin reste. (20 nov., relecture macOS) sur Mac, la
            // fenetre est rangee ici (should_close ne fait que prevenir) : sinon elle restait devant, filmee et cliquable
            #[cfg(target_os = "macos")]
            ui::park(hwnd);
            out(json!({ "event": "editor", "shown": false }));
            shown = false;
            if !film && view.is_some() {
                close_view(&mut view);
                out(json!({ "event": "view", "open": false }));
            }
            continue;
        }
        if ev == UiEv::Cmd {
            loop {
                let Some(line) = cmds.lock().unwrap().pop_front() else { break };
                let v: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                match v["cmd"].as_str().unwrap_or("") {
                    "quit" => break 'ui,
                    "save" => save_cmd(&inst, &a.states_dir, &mut last_saved, &mut last_mark, v["n"].clone()),
                    "editor" | "view" => {
                        let on = if v["cmd"] == "editor" { v["show"].as_bool().unwrap_or(false) } else { v["open"].as_bool().unwrap_or(false) };
                        if v["cmd"] == "editor" {
                            shown = on;
                        } else {
                            film = on;
                        }
                        if (film || shown) && view.is_none() {
                            match ui::open_view(&inst, hwnd, &frame) {
                                Ok((v, w, h)) => {
                                    view = Some(v);
                                    out(json!({ "event": "view", "open": true, "w": w, "h": h }));
                                }
                                Err(e) => {
                                    shown = false;
                                    film = false;
                                    out(json!({ "event": "error", "error": e }));
                                    continue;
                                }
                            }
                        }
                        if v["cmd"] == "editor" {
                            ui::show_on_desktop(hwnd, shown);
                            out(json!({ "event": "editor", "shown": shown }));
                        }
                        if !film && !shown && view.is_some() {
                            close_view(&mut view);
                            out(json!({ "event": "view", "open": false }));
                        }
                    }
                    // (19 nov., etape 3) un EFFET applique hors temps reel a un son (fichier f32 entrelace stereo ecrit par le
                    // compagnon) : le resultat dans « out », la latence retiree, la queue ajoutee
                    "render" if a.effect => {
                        let n = v["n"].clone();
                        let job = RenderJob {
                            input: PathBuf::from(v["in"].as_str().unwrap_or("")),
                            output: PathBuf::from(v["out"].as_str().unwrap_or("")),
                            frames: v["frames"].as_u64().unwrap_or(0),
                            rate: v["rate"].as_f64().unwrap_or(48000.0),
                            bpm: v["bpm"].as_f64().filter(|b| (20.0..=999.0).contains(b)).unwrap_or(120.0),
                            tail_s: v["tail"].as_f64().filter(|t| t.is_finite()).unwrap_or(0.0).clamp(0.0, 30.0),
                            start: v["start"].as_i64().unwrap_or(0),
                        };
                        // le direct s'arrete le temps du rendu (le meme processeur), et reprend
                        let _g = live.busy.lock().unwrap();
                        // (23 nov., 0.4.14, relecture) une defense de plus : seulement les fichiers de rendu du compagnon
                        let r = if job_paths_ok(&job.input, &job.output, &a.states_dir) { render(&mut inst, &job) } else { Err("these are not the companion's render files".into()) };
                        if live.on.load(Ordering::Relaxed) {
                            let lr = f64::from_bits(live.rate_bits.load(Ordering::Relaxed));
                            if inst.start_mode(lr, LIVE_BLOCK as i32, false).is_ok() {
                                *live.buses.lock().unwrap() = (inst.ins.clone(), inst.outs.clone());
                                live.starts.fetch_add(1, Ordering::Relaxed);
                                let all = inst.param_values();
                                inst.handler.pending.lock().unwrap().extend(all);
                            }
                        }
                        drop(_g);
                        match r {
                            Ok((frames, latency, tail)) => out(json!({ "event": "rendered", "n": n, "frames": frames, "latency": latency, "tail": tail })),
                            Err(e) => out(json!({ "event": "error", "n": n, "error": e })),
                        }
                    }
                    // (19 nov., etape 3b) l'effet en direct : on {rate, gen} → {event:"live", on, latency, rate} ; off
                    "live" if a.effect => {
                        let n = v["n"].clone();
                        let on = v["on"].as_bool().unwrap_or(false);
                        let _g = live.busy.lock().unwrap();
                        live.on.store(false, Ordering::Relaxed);
                        live.q.lock().unwrap().clear();
                        live.anchors.lock().unwrap().clear();
                        inst.stop();
                        if on {
                            let lr = v["rate"].as_f64().filter(|r| (8000.0..=192_000.0).contains(r)).unwrap_or(48000.0);
                            match inst.start_mode(lr, LIVE_BLOCK as i32, false) {
                                Ok(()) => {
                                    live.rate_bits.store(lr.to_bits(), Ordering::Relaxed);
                                    *live.buses.lock().unwrap() = (inst.ins.clone(), inst.outs.clone());
                                    live.starts.fetch_add(1, Ordering::Relaxed);
                                    live.latency.store(inst.latency, Ordering::Relaxed);
                                    live.gen.store(v["gen"].as_u64().unwrap_or(0) as u32, Ordering::Relaxed);
                                    // chaque parametre tel que l'editeur le montre, au premier bloc
                                    let all = inst.param_values();
                                    inst.handler.pending.lock().unwrap().extend(all);
                                    live.on.store(true, Ordering::Relaxed);
                                    out(json!({ "event": "live", "n": n, "on": true, "latency": inst.latency, "rate": lr }));
                                }
                                Err(e) => out(json!({ "event": "error", "n": n, "error": e })),
                            }
                        } else {
                            out(json!({ "event": "live", "n": n, "on": false, "dropped": live.dropped.load(Ordering::Relaxed), "done": live.done.load(Ordering::Relaxed) }));
                        }
                    }
                    "mix" | "tempo" => transport_cmd(&transport, &v),
                    // (20 nov., Link to controller) les parametres du plugin pour le studio
                    "params" | "param-set" | "param-watch" => param_cmd(&inst, &v, &pmap),
                    // (20 nov.) le menu d'essai (seulement avec UWL_HOST_MENU_LOG=1 : macui::test_menu)
                    #[cfg(target_os = "macos")]
                    "testmenu" => ui::test_menu(hwnd, v["x"].as_f64().unwrap_or(100.0), v["y"].as_f64().unwrap_or(100.0)),
                    _ => out(json!({ "event": "error", "error": "unknown command" })),
                }
            }
            continue;
        }
    }

    stop.store(true, Ordering::Relaxed);
    if let Some(h) = audio.take() {
        let _ = h.join();
    }
    live.on.store(false, Ordering::Relaxed);
    live.quit.store(true, Ordering::Relaxed);
    live.cv.notify_all();
    if let Some(h) = live_thread {
        let _ = h.join();
    }
    close_view(&mut view);
    inst.close();
    ui::destroy(hwnd);
    drop(inst);
    drop(lm);
    drop(m);
    Ok(())
}

/// « save » : l'etat du plugin ecrit dans le dossier des etats (son empreinte pour nom), → {"event":"saved"}.
fn save_cmd(inst: &Instance, states_dir: &Path, last_saved: &mut Option<(String, usize)>, last_mark: &mut u64, n: Value) {
    // (19 nov.) l'etat de certains plugins change de quelques octets sans qu'on touche a rien (Diva) :
    // il n'est relu que si le plugin a dit un changement (un geste dans l'editeur, restartComponent)
    // depuis le dernier enregistrement — sinon une empreinte de plus toutes les 15 s (Kontakt : 160 Ko)
    let mark = changes(inst);
    if let (Some((sha, bytes)), true) = (&*last_saved, mark == *last_mark) {
        out(json!({ "event": "saved", "n": n, "sha": sha, "bytes": bytes, "unchanged": true }));
        return;
    }
    match inst.get_state() {
        Ok((c, k)) => {
            let blob = pack_state(&c, k.as_deref());
            let sha = sha256_hex(&blob);
            let path = states_dir.join(format!("{sha}.bin"));
            let r = if std::fs::read(&path).map(|b| sha256_hex(&b) == sha).unwrap_or(false) { Ok(()) } else { write_state(states_dir, &path, &blob) };
            if r.is_ok() {
                *last_saved = Some((sha.clone(), blob.len()));
                *last_mark = mark;
            }
            match r {
                Ok(()) => out(json!({ "event": "saved", "n": n, "sha": sha, "bytes": blob.len() })),
                Err(e) => out(json!({ "event": "error", "n": n, "error": format!("the state could not be written ({e})") })),
            }
        }
        Err(e) => out(json!({ "event": "error", "n": n, "error": e })),
    }
}

/// « mix » (volume, panoramique, muet) et « tempo » : des valeurs que le fil audio lit.
fn transport_cmd(transport: &Transport, v: &Value) {
    if v["cmd"] == "mix" {
        let (gl, gr) = gains(v["vol"].as_f64().unwrap_or(1.0), v["pan"].as_f64().unwrap_or(0.0), v["mute"].as_bool().unwrap_or(false));
        transport.gain_l.store(gl.to_bits(), Ordering::Relaxed);
        transport.gain_r.store(gr.to_bits(), Ordering::Relaxed);
        return;
    }
    if let Some(b) = v["bpm"].as_f64().filter(|b| (20.0..=999.0).contains(b)) {
        transport.tempo_bits.store(b.to_bits(), Ordering::Relaxed);
    }
    if let Some(p) = v["playing"].as_bool() {
        transport.playing.store(p, Ordering::Relaxed);
    }
}

/// Ferme l'editeur (removed : le plugin cesse de le dessiner) ; la fenetre reste, masquee.
fn close_view(view: &mut Option<ComPtr<IPlugView>>) {
    VIEW.store(std::ptr::null_mut(), Ordering::Relaxed);
    if let Some(v) = view.take() {
        unsafe {
            v.removed();
            v.setFrame(std::ptr::null_mut());
        }
    }
}

/// (19 nov., etape 3) Un rendu d'effet : l'entree (frames trames stereo f32 entrelacees, a `rate`), `bpm` (les effets
/// synchronises), `tail_s` (la queue voulue par le studio, en plus de celle que le plugin annonce).
pub struct RenderJob {
    pub input: PathBuf,
    pub output: PathBuf,
    pub frames: u64,
    pub rate: f64,
    pub bpm: f64,
    pub tail_s: f64,
    /// (19 nov., la mesure en direct) la trame du morceau de la premiere trame de l'entree (0 : son debut)
    pub start: i64,
}

/// Plafonds d'un rendu : 10 minutes d'entree (FREEZE_MAX_SECONDS du studio), 30 s de queue.
pub const RENDER_MAX_SECONDS: f64 = 600.0;
pub const RENDER_TAIL_MAX_SECONDS: f64 = 30.0;

/// (23 nov., 0.4.14, relecture « les chemins in et out du rendu ne sont pas controles cote enfant ») Les fichiers d'un
/// rendu sont ceux que le compagnon nomme (host.rs, job_paths) : dans le dossier jobs/ voisin de --states-dir,
/// « job-<pid>-<n>.in » et le « .out » du meme nom. Rien d'autre n'est lu ni ecrit.
pub fn job_paths_ok(input: &Path, output: &Path, states_dir: &Path) -> bool {
    let Some(jobs) = states_dir.parent().map(|d| d.join("jobs")) else { return false };
    let name_ok = |p: &Path, ext: &str| -> Option<String> {
        if p.parent()? != jobs {
            return None;
        }
        let n = p.file_name()?.to_str()?;
        let stem = n.strip_prefix("job-")?.strip_suffix(ext)?;
        let (pid, k) = stem.split_once('-')?;
        let digits = |x: &str| !x.is_empty() && x.len() <= 10 && x.bytes().all(|b| b.is_ascii_digit());
        (digits(pid) && digits(k)).then(|| stem.to_string())
    };
    matches!((name_ok(input, ".in"), name_ok(output, ".out")), (Some(a), Some(b)) if a == b)
}

/// Le rendu, dans le fil de l'interface (l'editeur ne se dessine pas pendant, quelques secondes au plus) : le plugin
/// passe en kOffline a la frequence du son, recoit d'abord la valeur de chaque parametre telle que son editeur la
/// montre (un controleur a part ne l'a dite au processeur que par des gestes : il n'y a pas de fil audio ici), puis
/// le son, bloc par bloc ; la latence annoncee (getLatencySamples) est retiree du debut, la queue (getTailSamples,
/// plafonnee, ou celle voulue) ajoutee a la fin. → (trames ecrites, latence, queue)
pub fn render(inst: &mut Instance, job: &RenderJob) -> Result<(u64, u32, u64), String> {
    const BLOCK: usize = 1024;
    if !(8000.0..=192_000.0).contains(&job.rate) {
        return Err("rate".into());
    }
    if job.frames == 0 || job.frames as f64 > RENDER_MAX_SECONDS * job.rate {
        return Err("the sound is empty or longer than 10 minutes".into());
    }
    let need = job.frames as usize * 2 * 4;
    let bytes = std::fs::read(&job.input).map_err(|e| format!("input: {e}"))?;
    if bytes.len() != need {
        return Err(format!("input: {} bytes, {need} expected", bytes.len()));
    }
    let input: Vec<f32> = bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    drop(bytes);
    inst.stop();
    inst.start_mode(job.rate, BLOCK as i32, true)?;
    let latency = inst.latency as u64;
    let plug_tail = unsafe { inst.processor.getTailSamples() } as u64;
    let max_tail = (RENDER_TAIL_MAX_SECONDS * job.rate) as u64;
    // kInfiniteTail (u32::MAX) : la queue maximale ; sinon celle du plugin, ou celle voulue par le studio si plus longue
    let tail = if plug_tail >= u32::MAX as u64 { max_tail } else { plug_tail.min(max_tail).max((job.tail_s * job.rate) as u64) };
    let total_out = job.frames + tail;
    let total_in = total_out + latency;
    let mut eng = Engine::new(&inst.ins, &inst.outs, BLOCK, job.rate);
    eng.mode = ProcessModes_::kOffline as i32;
    eng.ctx.tempo = job.bpm;
    let mut out_buf: Vec<f32> = Vec::with_capacity(total_out as usize * 2);
    let mut first: Vec<(u32, f64)> = inst.param_values();
    let mut pos = 0u64;
    let mono_in = inst.ins.first().map(|c| *c == 1).unwrap_or(false);
    let mut skip = latency;
    while pos < total_in {
        let n = (total_in - pos).min(BLOCK as u64) as usize;
        if let Some(b) = eng.main_in() {
            for i in 0..n {
                let f = pos as usize + i;
                let (l, r) = if f < job.frames as usize { (input[2 * f], input[2 * f + 1]) } else { (0.0, 0.0) };
                if mono_in {
                    if let Some(c) = b.get_mut(0) {
                        c[i] = 0.5 * (l + r);
                    }
                } else {
                    if let Some(c) = b.get_mut(0) {
                        c[i] = l;
                    }
                    if let Some(c) = b.get_mut(1) {
                        c[i] = r;
                    }
                }
            }
        }
        let params = std::mem::take(&mut first);
        eng.song = Some(job.start + pos as i64);
        let r = eng.process(inst.processor.as_ptr(), n, pos as i64, 0, &params);
        if r != kResultOk {
            inst.stop();
            return Err(format!("the effect refused to process ({r})"));
        }
        let (l, rr) = eng.main_out();
        for i in 0..n {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            if (out_buf.len() / 2) as u64 >= total_out {
                break;
            }
            out_buf.push(l.get(i).copied().unwrap_or(0.0));
            out_buf.push(rr.get(i).copied().unwrap_or(0.0));
        }
        pos += n as u64;
    }
    inst.stop();
    // la queue : jusqu'au dernier echantillon audible (-100 dBFS), jamais avant la fin de l'entree (un effet qui annonce une
    // queue infinie, kHs Gain par exemple, ne laisse pas 30 s de silence)
    let last = out_buf.chunks_exact(2).rposition(|c| c[0].abs() > 1e-5 || c[1].abs() > 1e-5).map(|i| i as u64 + 1).unwrap_or(0);
    let keep = last.max(job.frames).min((out_buf.len() / 2) as u64) as usize;
    out_buf.truncate(keep * 2);
    let mut raw = Vec::with_capacity(out_buf.len() * 4);
    for x in &out_buf {
        raw.extend_from_slice(&x.to_le_bytes());
    }
    std::fs::write(&job.output, raw).map_err(|e| format!("output: {e}"))?;
    Ok(((out_buf.len() / 2) as u64, latency as u32, tail))
}

/// (19 nov., etape 3b) Une ligne « in » : {"cmd":"in","gen":G,"pos":P,"d":"<base64 f32 stereo>"} → la file du direct
/// (seulement si le direct est allume, pour la generation en cours ; la file pleine : jete, compte).
fn live_in(lv: &Live, line: &str) {
    if !lv.on.load(Ordering::Relaxed) {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    if v["gen"].as_u64().unwrap_or(u64::MAX) as u32 != lv.gen.load(Ordering::Relaxed) {
        return;
    }
    let (Some(pos), Some(d)) = (v["pos"].as_u64(), v["d"].as_str()) else { return };
    let Some(x) = crate::b64::decode_f32(d) else { return };
    if x.is_empty() || x.len() % 2 != 0 || x.len() / 2 > LIVE_BLOCK {
        return;
    }
    let mut q = lv.q.lock().unwrap();
    if q.len() >= LIVE_QUEUE {
        lv.dropped.fetch_add(1, Ordering::Relaxed);
        return;
    }
    q.push_back((pos, x));
    drop(q);
    lv.cv.notify_one();
}

/// (19 nov., la mesure en direct) Une ligne « song » : {"cmd":"song","from":F,"offset":O,"playing":P} → les ancres du
/// direct (ecrite par le compagnon, host.rs : la forme exacte reconnue par le fil de lecture).
fn live_song(lv: &Live, line: &str) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    let (Some(from), Some(offset)) = (v["from"].as_i64(), v["offset"].as_i64()) else { return };
    let playing = v["playing"].as_bool().unwrap_or(true);
    lv.anchors.lock().unwrap().push(Anchor { from, offset, playing });
}

/// (19 nov., etape 3b) Le fil du direct : chaque paquet du studio, des qu'il arrive, traverse l'effet ; le resultat part
/// date de la meme trame, moins la latence du plugin : {"event":"out","gen","pos","d"}.
fn live_loop(lv: Arc<Live>, tr: Arc<Transport>, proc_ptr: SendBox<*mut IAudioProcessor>, handler_ptr: SendBox<*const crate::realhost::Handler>) {
    let h = unsafe { &*handler_ptr.0 };
    let mut eng: Option<(u32, Engine)> = None;
    let mut ins: Vec<i32> = Vec::new();
    let mut params: Vec<(u32, f64)> = Vec::new();
    ui::raise_priority();
    loop {
        let item = {
            let mut q = lv.q.lock().unwrap();
            loop {
                if lv.quit.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(it) = q.pop_front() {
                    break it;
                }
                q = lv.cv.wait_timeout(q, Duration::from_millis(200)).unwrap().0;
            }
        };
        if !lv.on.load(Ordering::Relaxed) {
            continue;
        }
        let _g = lv.busy.lock().unwrap();
        if !lv.on.load(Ordering::Relaxed) {
            continue;
        }
        let starts = lv.starts.load(Ordering::Relaxed);
        if eng.as_ref().map(|(s, _)| *s != starts).unwrap_or(true) {
            let rate = f64::from_bits(lv.rate_bits.load(Ordering::Relaxed));
            let (i, o) = lv.buses.lock().unwrap().clone();
            eng = Some((starts, Engine::new(&i, &o, LIVE_BLOCK, rate)));
            ins = i;
        }
        let e = &mut eng.as_mut().unwrap().1;
        // le tempo du projet (« tempo »), comme au rendu : un delay cale sur le tempo sonne pareil en direct et gele
        e.ctx.tempo = f64::from_bits(tr.tempo_bits.load(Ordering::Relaxed));
        let (pos, x) = item;
        let n = x.len() / 2;
        let mono = ins.first().map(|c| *c == 1).unwrap_or(false);
        // (19 nov., la mesure en direct) la position du morceau (les ancres du studio) : un saut au milieu du bloc le
        // coupe en deux traitements, chacun a sa position ; la sortie reste datee de la trame du worklet (la compensation)
        let spans = lv.anchors.lock().unwrap().spans(pos as i64, n);
        let mut y = Vec::with_capacity(n * 2);
        for (s0, len, song, playing) in spans {
            if let Some(b) = e.main_in() {
                for i in 0..len {
                    let (l, r) = (x[2 * (s0 + i)], x[2 * (s0 + i) + 1]);
                    if mono {
                        if let Some(c) = b.get_mut(0) {
                            c[i] = 0.5 * (l + r);
                        }
                    } else {
                        if let Some(c) = b.get_mut(0) {
                            c[i] = l;
                        }
                        if let Some(c) = b.get_mut(1) {
                            c[i] = r;
                        }
                    }
                }
            }
            params.clear();
            params.append(&mut h.pending.lock().unwrap());
            e.song = song;
            e.playing = playing;
            e.process(proc_ptr.0, len, pos as i64 + s0 as i64, 0, &params);
            let (l, r) = e.main_out();
            for i in 0..len {
                y.push(l.get(i).copied().unwrap_or(0.0));
                y.push(r.get(i).copied().unwrap_or(0.0));
            }
        }
        let lat = lv.latency.load(Ordering::Relaxed) as i64;
        let gen = lv.gen.load(Ordering::Relaxed);
        lv.done.fetch_add(1, Ordering::Relaxed);
        // (23 nov., 0.4.14, relecture) le processeur rendu AVANT d'ecrire : un compagnon qui lit lentement ne bloque
        // jamais un rendu ni l'arret du direct
        drop(_g);
        out(json!({ "event": "out", "gen": gen, "pos": pos as i64 - lat, "d": crate::b64::encode_f32(&y) }));
    }
}

/// (20 nov., Link to controller) « params » {start, n} → {event:"params", n, items:[{p, name, v}], next} (64 au plus par
/// page) ; « param-set » {p, v} ; « param-watch » {on}. Au fil de l'interface (le controleur n'est appele que la).
fn param_cmd(inst: &Instance, v: &Value, pmap: &Arc<Mutex<std::collections::HashMap<u32, u32>>>) {
    match v["cmd"].as_str().unwrap_or("") {
        "params" => {
            let start = v["start"].as_u64().unwrap_or(0).min(4096) as usize;
            let (page, next) = inst.param_page(start, 64);
            {
                let mut m = pmap.lock().unwrap();
                for (p, id, _, _) in &page {
                    m.insert(*id, *p);
                }
            }
            let items: Vec<Value> = page.iter().map(|(p, _, name, val)| json!({ "p": p, "name": name, "v": val })).collect();
            out(json!({ "event": "params", "n": v["n"], "items": items, "next": next }));
        }
        "param-set" => {
            let (Some(p), Some(val)) = (v["p"].as_u64(), v["v"].as_f64()) else { return };
            if let Some(id) = inst.param_id(p.min(4095) as u32) {
                inst.set_param(id, val);
            }
        }
        "param-watch" => {
            let on = v["on"].as_bool().unwrap_or(false);
            // (20 nov., l'essai de Lucas) le studio redemande le suivi a CHAQUE appui dans l'image, en meme temps que le
            // geste : deja suivi, rien n'est efface (ni ce que le geste vient de faire bouger) ni relu
            if on && inst.handler.watch.load(Ordering::Relaxed) && !pmap.lock().unwrap().is_empty() {
                return;
            }
            if on {
                // l'indice de CHAQUE parametre (le studio n'en lit que les 512 premiers ; un geste peut en toucher un autre)
                let mut start = Some(0);
                let mut m = pmap.lock().unwrap();
                while let Some(s) = start {
                    let (page, next) = inst.param_page(s, 512);
                    for (p, id, _, _) in &page {
                        m.insert(*id, *p);
                    }
                    start = next;
                }
            }
            inst.handler.touched.lock().unwrap().clear();
            inst.handler.watch.store(on, Ordering::Relaxed);
        }
        _ => {}
    }
}

/// Ce que le plugin a dit changer depuis son ouverture : les gestes de son editeur (performEdit) et ses restartComponent.
fn changes(inst: &Instance) -> u64 {
    let h = &*inst.handler;
    h.edits.load(Ordering::Relaxed) as u64 + h.restarts.lock().unwrap().len() as u64
}

/// Attendre l'instant du prochain bloc : dormir, puis ceder le processeur les dernieres centaines de microsecondes.
fn wait_until(target: Instant) {
    loop {
        let now = Instant::now();
        if now >= target {
            return;
        }
        let left = target - now;
        if left > Duration::from_micros(700) {
            std::thread::sleep(left - Duration::from_micros(500));
        } else {
            std::thread::yield_now();
        }
    }
}

// ── (Windows) la fenetre, la boucle de l'interface ──────────────────────────────────────────────────────────────
// (20 nov.) le code de l'etape 1, tel quel, sorti de run() : les memes appels, dans le meme ordre

#[cfg(windows)]
mod win {
    use super::{UiEv, VIEW};
    use crate::realhost::{Frame, Instance};
    use std::ffi::c_void;
    use std::sync::atomic::Ordering;
    use vst3::Steinberg::Vst::*;
    use vst3::Steinberg::*;
    use vst3::{ComPtr, ComWrapper};
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    const WM_CMD: u32 = WM_APP + 2;
    const WM_QUIT_HOST: u32 = WM_APP + 1;
    const WM_HIDE_EDITOR: u32 = WM_APP + 3;

    pub type Win = isize;

    pub fn init() {
        use windows_sys::Win32::System::Ole::OleInitialize;
        use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            OleInitialize(std::ptr::null());
        }
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg == WM_SIZE {
            if let Some(v) = vst3::ComRef::from_raw(VIEW.load(Ordering::Relaxed)) {
                let (w, h) = ((lp & 0xffff) as i32, ((lp >> 16) & 0xffff) as i32);
                let mut cur: ViewRect = std::mem::zeroed();
                v.getSize(&mut cur);
                if w > 0 && h > 0 && (cur.right - cur.left != w || cur.bottom - cur.top != h) {
                    let mut r = ViewRect { left: 0, top: 0, right: w, bottom: h };
                    v.onSize(&mut r);
                }
            }
        }
        if msg == WM_CLOSE {
            // la croix (fenetre montree sur le bureau) : on la masque, le plugin reste
            cloak(hwnd, true);
            PostMessageW(hwnd, WM_HIDE_EDITOR, 0, 0);
            return 0;
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn cloak(hwnd: HWND, on: bool) {
        use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
        let v: i32 = on as i32;
        unsafe { DwmSetWindowAttribute(hwnd, DWMWA_CLOAK as u32, &v as *const i32 as *const c_void, 4) };
    }

    pub fn make_window(title: &str) -> Result<Win, String> {
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        unsafe {
            let inst = GetModuleHandleW(std::ptr::null());
            let class = wide("UppwindPlugHost");
            let wc = WNDCLASSW {
                style: CS_DBLCLKS,
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: inst,
                hIcon: std::ptr::null_mut(),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: class.as_ptr(),
            };
            RegisterClassW(&wc);
            let o = wide("Uppwind plugin host");
            let owner = CreateWindowExW(0, class.as_ptr(), o.as_ptr(), WS_OVERLAPPED, 0, 0, 10, 10, std::ptr::null_mut(), std::ptr::null_mut(), inst, std::ptr::null());
            let t = wide(title);
            // pas WS_EX_TOOLWINDOW : Windows.Graphics.Capture refuse les fenetres d'outil (mesure du 19 nov.)
            let hwnd = CreateWindowExW(0, class.as_ptr(), t.as_ptr(), WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN, 120, 120, 800, 500, owner, std::ptr::null_mut(), inst, std::ptr::null());
            if hwnd.is_null() {
                return Err("CreateWindowExW failed".into());
            }
            Ok(hwnd as Win)
        }
    }

    /// La place de depart : masquee par DWM, en bas de la pile.
    pub fn park(w: Win) {
        cloak(w as HWND, true);
        unsafe { SetWindowPos(w as HWND, HWND_BOTTOM, 120, 120, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW) };
    }

    /// L'identifiant que le compagnon filme (le handle).
    pub fn window_id(w: Win) -> u64 {
        w as u64
    }

    /// (macOS seulement : la barre de titre retiree de l'image) ; sous Windows, la capture filme deja le contenu seul
    pub fn titlebar(_w: Win) -> f64 {
        0.0
    }

    pub fn show_on_desktop(w: Win, show: bool) {
        let hwnd = w as HWND;
        unsafe {
            if show {
                cloak(hwnd, false);
                SetWindowPos(hwnd, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
                SetForegroundWindow(hwnd);
            } else {
                cloak(hwnd, true);
                SetWindowPos(hwnd, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }

    /// Depuis un autre fil : reveiller la boucle (un ordre en attente, ou la fin).
    pub fn post(w: Win, ev: UiEv) {
        let m = match ev {
            UiEv::Cmd => WM_CMD,
            UiEv::Quit => WM_QUIT_HOST,
            UiEv::HideEditor => WM_HIDE_EDITOR,
        };
        unsafe { PostMessageW(w as HWND, m, 0, 0) };
    }

    /// (macOS seulement : un menu du plugin tient la boucle d'AppKit) ; sous Windows, rien a faire
    pub fn cancel_menu() {}

    /// La boucle de l'interface : les messages du systeme passent, les notres reviennent (None : la boucle est finie).
    pub fn next(_in_menu: &mut dyn FnMut()) -> Option<UiEv> {
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        loop {
            if unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } <= 0 {
                return None;
            }
            match msg.message {
                WM_QUIT_HOST => return Some(UiEv::Quit),
                WM_HIDE_EDITOR => return Some(UiEv::HideEditor),
                WM_CMD => return Some(UiEv::Cmd),
                _ => unsafe {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                },
            }
        }
    }

    pub fn destroy(w: Win) {
        unsafe { DestroyWindow(w as HWND) };
    }

    /// Ouvre l'editeur dans la fenetre (masquee) : createView, le cadre, la mise a l'echelle, attached, la taille.
    pub fn open_view(inst: &Instance, w: Win, frame: &ComWrapper<Frame>) -> Result<(ComPtr<IPlugView>, i32, i32), String> {
        use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
        let hwnd = w as HWND;
        let c = inst.ctrl.as_ref().ok_or("the plugin has no editor")?;
        unsafe {
            let v = ComPtr::from_raw(c.createView(ViewType::kEditor)).ok_or("the plugin has no editor")?;
            if v.isPlatformTypeSupported(kPlatformTypeHWND) != kResultTrue {
                return Err("the editor does not support HWND".into());
            }
            let fp = frame.to_com_ptr::<IPlugFrame>().ok_or("frame")?;
            v.setFrame(fp.as_ptr());
            if let Some(s) = v.cast::<IPlugViewContentScaleSupport>() {
                s.setContentScaleFactor(GetDpiForWindow(hwnd).max(96) as f32 / 96.0);
            }
            let mut r: ViewRect = std::mem::zeroed();
            v.getSize(&mut r);
            crate::realhost::size_window(w, (r.right - r.left).max(200), (r.bottom - r.top).max(100));
            if v.attached(hwnd as *mut c_void, kPlatformTypeHWND) != kResultOk {
                v.setFrame(std::ptr::null_mut());
                return Err("the editor did not attach".into());
            }
            VIEW.store(v.as_ptr(), Ordering::Relaxed);
            let mut r: ViewRect = std::mem::zeroed();
            v.getSize(&mut r);
            let (w2, h2) = (r.right - r.left, r.bottom - r.top);
            if w2 > 0 && h2 > 0 {
                crate::realhost::size_window(w, w2, h2);
                let mut r2 = ViewRect { left: 0, top: 0, right: w2, bottom: h2 };
                v.onSize(&mut r2);
            }
            Ok((v, w2, h2))
        }
    }

    pub fn raise_priority() {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST};
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trip() {
        let b = pack_state(b"abc", Some(b"xy"));
        assert_eq!(unpack_state(&b), Some((b"abc".to_vec(), Some(b"xy".to_vec()))));
        let b = pack_state(b"", None);
        assert_eq!(unpack_state(&b), Some((Vec::new(), None)));
        assert_eq!(unpack_state(b"UWS1\x05\0\0\0ab"), None);
        assert_eq!(unpack_state(b"nope"), None);
    }

    #[test]
    fn song_anchors() {
        let mut a = Anchors::default();
        // pas d'ancre : la trame du worklet, en lecture (le direct d'avant)
        assert_eq!(a.spans(1000, 256), vec![(0, 256, None, true)]);
        // un depart a la trame 1100 du worklet, le morceau a 48000 : le bloc coupe en deux
        a.push(Anchor { from: 1100, offset: 48000 - 1100, playing: true });
        assert_eq!(a.spans(1000, 256), vec![(0, 100, None, true), (100, 156, Some(48000), true)]);
        assert_eq!(a.spans(1256, 256), vec![(0, 256, Some(48156), true)]);
        // une boucle programmee d'avance (trame 2000 → morceau 0) ; l'arret a 3000 garde la position
        a.push(Anchor { from: 2000, offset: -2000, playing: true });
        a.push(Anchor { from: 3000, offset: -2000, playing: false });
        assert_eq!(a.spans(1900, 256), vec![(0, 100, Some(48800), true), (100, 156, Some(0), true)]);
        assert_eq!(a.spans(2900, 256), vec![(0, 100, Some(900), true), (100, 156, Some(1000), false)]);
        assert_eq!(a.spans(3156, 256), vec![(0, 256, Some(1000), false)]);
        // un saut reprogramme remplace les ancres d'apres lui
        a.push(Anchor { from: 4000, offset: 10, playing: true });
        a.push(Anchor { from: 5000, offset: 20, playing: true });
        a.push(Anchor { from: 4500, offset: 30, playing: true });
        assert_eq!(a.spans(4400, 256), vec![(0, 100, Some(4410), true), (100, 156, Some(4530), true)]);
        assert_eq!(a.spans(5100, 10), vec![(0, 10, Some(5130), true)]);
        // une ancre exactement au debut du bloc
        a.push(Anchor { from: 6000, offset: 1, playing: true });
        assert_eq!(a.spans(6000, 10), vec![(0, 10, Some(6001), true)]);
    }

    #[test]
    fn track_gains() {
        assert_eq!(gains(1.0, 0.0, false), (1.0, 1.0));
        assert_eq!(gains(0.5, -1.0, false), (0.5, 0.0));
        assert_eq!(gains(0.5, 0.5, false), (0.25, 0.5));
        assert_eq!(gains(1.0, 0.0, true), (0.0, 0.0));
    }

    #[test]
    fn names_and_cids() {
        assert!(is_state_name(&sha256_hex(b"x")));
        assert!(!is_state_name("../x"));
        assert!(!is_state_name(&"A".repeat(64)));
        let t = parse_cid("000102030405060708090a0b0c0d0e0f").unwrap();
        assert_eq!(cid_hex(&t), "000102030405060708090a0b0c0d0e0f");
        assert!(parse_cid("zz").is_none());
    }

    // (20 nov., relecture de securite, point 7) l'ecriture remplace un fichier abime et ne laisse aucun fichier temporaire
    #[test]
    fn state_written_whole() {
        let d = std::env::temp_dir().join(format!("uw-states-p{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let blob = pack_state(b"settings", None);
        let path = d.join(sha256_hex(&blob));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(&path, &blob[..3]).unwrap();
        write_state(&d, &path, &blob).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), blob);
        let left: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 1, "no temporary file left: {left:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    // (23 nov., 0.4.14) une ligne trop longue est jetee sans etre gardee, les suivantes passent
    #[test]
    fn long_lines_are_dropped_as_they_come() {
        let mut long = vec![b'x'; 100_000];
        long.push(b'\n');
        let mut data = b"{\"cmd\":\"a\"}\r\n".to_vec();
        data.extend_from_slice(&long);
        data.extend_from_slice(b"\n{\"cmd\":\"b\"}");
        // un petit tampon : la longue ligne arrive en beaucoup de morceaux
        let mut r = std::io::BufReader::with_capacity(64, &data[..]);
        let mut buf = Vec::new();
        assert_eq!(next_line(&mut r, LINE_MAX, &mut buf).unwrap(), Some(true));
        assert_eq!(buf, b"{\"cmd\":\"a\"}");
        assert_eq!(next_line(&mut r, LINE_MAX, &mut buf).unwrap(), Some(false));
        // la ligne fait 100 000 octets ; le tampon ne passe jamais deux fois LINE_MAX (sa croissance par doublement)
        assert!(buf.capacity() <= 2 * (LINE_MAX + 64), "never kept: {}", buf.capacity());
        assert_eq!(next_line(&mut r, LINE_MAX, &mut buf).unwrap(), Some(true));
        assert!(buf.is_empty(), "the empty line");
        assert_eq!(next_line(&mut r, LINE_MAX, &mut buf).unwrap(), Some(true));
        assert_eq!(buf, b"{\"cmd\":\"b\"}", "the last line, without its newline");
        assert_eq!(next_line(&mut r, LINE_MAX, &mut buf).unwrap(), None);
        // exactement LINE_MAX octets : gardee ; un de plus : jetee
        let mut edge = vec![b'y'; LINE_MAX];
        edge.extend_from_slice(b"\r\n");
        edge.extend_from_slice(&vec![b'z'; LINE_MAX + 1]);
        edge.push(b'\n');
        let mut r = std::io::BufReader::with_capacity(100, &edge[..]);
        assert_eq!(next_line(&mut r, LINE_MAX, &mut buf).unwrap(), Some(true));
        assert_eq!(buf.len(), LINE_MAX);
        assert_eq!(next_line(&mut r, LINE_MAX, &mut buf).unwrap(), Some(false));
        assert_eq!(next_line(&mut r, LINE_MAX, &mut buf).unwrap(), None);
    }

    // (23 nov., 0.4.14) seulement les fichiers de rendu que le compagnon nomme
    #[test]
    fn only_the_companion_render_files() {
        let data = std::env::temp_dir().join("uw-data");
        let states = data.join("plugin-states");
        let jobs = data.join("jobs");
        assert!(job_paths_ok(&jobs.join("job-1234-7.in"), &jobs.join("job-1234-7.out"), &states));
        assert!(!job_paths_ok(&jobs.join("job-1234-7.in"), &jobs.join("job-1234-8.out"), &states), "the same job");
        assert!(!job_paths_ok(&jobs.join("job-1234-7.in"), &data.join("job-1234-7.out"), &states), "in jobs/");
        assert!(!job_paths_ok(&jobs.join("job-1234-7.in"), &jobs.join("..").join("x.out"), &states));
        assert!(!job_paths_ok(&jobs.join("job-1234-7.in"), &jobs.join("evil.dll"), &states));
        assert!(!job_paths_ok(&jobs.join("job-a-7.in"), &jobs.join("job-a-7.out"), &states));
        assert!(!job_paths_ok(Path::new(""), Path::new(""), &states));
        assert!(!job_paths_ok(&jobs.join("job-1-1.in"), &jobs.join("job-1-1.out"), Path::new("")), "no data folder");
    }
}
