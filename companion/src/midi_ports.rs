// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! macOS (26 oct. 2026) : les deux ports MIDI d'Uppwind Link crees par le compagnon lui-meme (CoreMIDI), a la place de
//! loopMIDI (Windows) ou du bus IAC a regler a la main.
//!
//! Chaque port est une boucle, comme un port de loopMIDI : une DESTINATION virtuelle (les autres applis y envoient :
//! pour elles, c'est une sortie MIDI « Uppwind Link ») et une SOURCE virtuelle du meme nom (une entree « Uppwind
//! Link ») ; ce qui arrive dans la destination ressort tel quel par la source (MIDIReceived, sur le fil de CoreMIDI,
//! sans rien lire ni garder). Les deux ports : « Uppwind Link » (le studio → le DAW) et « Uppwind Link Back » (le DAW →
//! le studio). Un identifiant unique fixe (kMIDIPropertyUniqueID) par port. Rien n'est ecrit dans la configuration MIDI
//! de la machine.
//!
//! (29 oct.) Le gardien des ports. macOS retire les ports d'un programme a sa sortie, et FL Studio garde alors des
//! references mortes : muet jusqu'a « Refresh device list » dans ses reglages MIDI (vu le 28 oct. ; l'identifiant fixe
//! n'y change rien, FL ne se reconnecte pas seul). Les ports n'ont besoin de rien du compagnon : ils vivent donc dans un
//! petit processus a part, le meme programme lance avec `--midi-ports-keeper`, qui survit au compagnon (un
//! redemarrage, une mise a jour, un plantage) :
//!   - detache : une session a lui (setsid, des avant exec) ; ni la fin du groupe de processus du compagnon, ni l'arret
//!     de son LaunchAgent ne l'emportent ; entree et sorties sur /dev/null ; il tourne depuis /.
//!   - un seul par jeu de noms : un fichier a cote du fichier d'etat du compagnon (~/.uppwind-companion-midi-ports.pid,
//!     ou dans le bac a sable de --sandbox), verrouille (fcntl) tant qu'il vit, son pid ecrit une fois les ports faits.
//!     Un gardien vivant dont les ports existent : le nouveau s'en va aussitot. Vivant mais sans ses ports (le serveur
//!     MIDI a redemarre ?) : il est remplace.
//!   - il attend SIGTERM (ou SIGINT, SIGHUP), retire son fichier et sort ; macOS retire alors les ports.
//! Au demarrage du compagnon (`open`) : les deux points de chaque port (source et destination) deja la → rien n'est
//! cree ; sinon le gardien est lance et attendu 2 s ; s'il ne vient pas, le compagnon cree les ports lui-meme, comme
//! avant (ils vivent alors autant que lui). `--stop` laisse le gardien (install.rs le reconnait a son option) ;
//! `--stop-midi-ports` et `--uninstall` l'arretent. `--no-midi-ports` : ni ports ni gardien (un bus IAC du meme nom,
//! par exemple). Les essais donnent d'autres noms (`--names "A,B"`, une option du gardien et des essais seulement) :
//! jamais les vrais ports.
//!
//! Vu sur macOS 26 : la liste des ports d'un processus reste celle de son premier appel a CoreMIDI, sauf s'il a un
//! client et fait tourner la boucle du fil de ce client (CFRunLoop) : `open` le fait pendant son attente (Watch).
//! *(a tester dans un vrai DAW)* : FL Studio pour Mac garde-t-il les ports apres un redemarrage du compagnon, sans Refresh.

#![allow(non_upper_case_globals)]

use std::ffi::{c_char, c_void};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

type MIDIObjectRef = u32;
type OSStatus = i32;
type CFStringRef = *const c_void;
type NotifyProc = extern "C" fn(*const c_void, *mut c_void);

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithCString(alloc: *const c_void, s: *const c_char, enc: u32) -> CFStringRef;
    fn CFRelease(o: *const c_void);
    static kCFRunLoopDefaultMode: CFStringRef;
    fn CFRunLoopRunInMode(mode: CFStringRef, seconds: f64, return_after_source_handled: u8) -> i32;
}

#[link(name = "CoreMIDI", kind = "framework")]
extern "C" {
    static kMIDIPropertyUniqueID: CFStringRef;
    fn MIDIClientCreate(name: CFStringRef, notify: Option<NotifyProc>, refcon: *mut c_void, out: *mut MIDIObjectRef) -> OSStatus;
    fn MIDIClientDispose(client: MIDIObjectRef) -> OSStatus;
    fn MIDISourceCreate(client: MIDIObjectRef, name: CFStringRef, out: *mut MIDIObjectRef) -> OSStatus;
    fn MIDIDestinationCreate(client: MIDIObjectRef, name: CFStringRef, read: extern "C" fn(*const c_void, *mut c_void, *mut c_void), refcon: *mut c_void, out: *mut MIDIObjectRef) -> OSStatus;
    fn MIDIReceived(src: MIDIObjectRef, list: *const c_void) -> OSStatus;
    fn MIDIObjectSetIntegerProperty(obj: MIDIObjectRef, prop: CFStringRef, v: i32) -> OSStatus;
    static kMIDIPropertyDisplayName: CFStringRef;
    static kMIDIPropertyName: CFStringRef;
    fn MIDIObjectGetStringProperty(obj: MIDIObjectRef, prop: CFStringRef, out: *mut CFStringRef) -> OSStatus;
    fn MIDIGetNumberOfSources() -> usize;
    fn MIDIGetSource(i: usize) -> MIDIObjectRef;
    fn MIDIGetNumberOfDestinations() -> usize;
    fn MIDIGetDestination(i: usize) -> MIDIObjectRef;
}

// la libc (libSystem) : ce que le gardien fait de son processus
extern "C" {
    fn setsid() -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    fn sysctl(name: *mut i32, namelen: u32, oldp: *mut c_void, oldlenp: *mut usize, newp: *mut c_void, newlen: usize) -> i32;
    fn signal(sig: i32, handler: extern "C" fn(i32)) -> usize;
    fn write(fd: i32, buf: *const c_void, n: usize) -> isize;
    fn dup2(from: i32, to: i32) -> i32;
}

const SIGHUP: i32 = 1;
const SIGINT: i32 = 2;
const SIGKILL: i32 = 9;
const SIGTERM: i32 = 15;

fn name_of(obj: MIDIObjectRef) -> String {
    for prop in unsafe { [kMIDIPropertyDisplayName, kMIDIPropertyName] } {
        let mut s: CFStringRef = std::ptr::null();
        if unsafe { MIDIObjectGetStringProperty(obj, prop, &mut s) } == 0 && !s.is_null() {
            let n = crate::macos::cf_string(s);
            unsafe { CFRelease(s) };
            if !n.is_empty() {
                return n;
            }
        }
    }
    String::new()
}

/// Les ports MIDI de la machine, en lecture seule (--setup) : (les entrees = les sources que les applis lisent, les
/// sorties = les destinations ou elles ecrivent), par leur nom affiche (« IAC Driver Bus 1 », « Uppwind Link »).
pub fn endpoints() -> (Vec<String>, Vec<String>) {
    let ins = (0..unsafe { MIDIGetNumberOfSources() }).map(|i| name_of(unsafe { MIDIGetSource(i) })).filter(|n| !n.is_empty()).collect();
    let outs = (0..unsafe { MIDIGetNumberOfDestinations() }).map(|i| name_of(unsafe { MIDIGetDestination(i) })).filter(|n| !n.is_empty()).collect();
    (ins, outs)
}

/// Chacun de ces ports a-t-il sa source ET sa destination (par le nom affiche) ?
pub fn all_there(names: &[String]) -> bool {
    let (ins, outs) = endpoints();
    names.iter().all(|n| ins.contains(n) && outs.contains(n))
}

const UTF8: u32 = 0x0800_0100;

/// Les deux ports (le meme nom que les ports de loopMIDI sous Windows : les scripts des DAW les cherchent par ce nom).
pub const PORTS: [&str; 2] = ["Uppwind Link", "Uppwind Link Back"];

/// L'option du gardien (cachee : le compagnon le lance lui-meme) ; --stop la reconnait pour le laisser.
pub const KEEPER_FLAG: &str = "--midi-ports-keeper";
/// Arrete le gardien : les ports partent (--uninstall le fait aussi).
pub const STOP_FLAG: &str = "--stop-midi-ports";
/// (essais) le demarrage du compagnon (`open`) avec d'autres noms, puis la sortie, comme un compagnon qui s'arrete.
const TEST_FLAG: &str = "--midi-ports-test";
/// (essais) d'autres noms que PORTS, pour le gardien, son arret et l'essai seulement : « A,B ».
const NAMES_FLAG: &str = "--names";

/// Les codes de sortie du gardien qui s'en va aussitot (0 : un autre gardien les tient deja).
const EXIT_MADE_ELSEWHERE: i32 = 3;
const EXIT_COREMIDI: i32 = 4;

/// Le temps laisse au gardien pour faire ses ports (le compagnon l'attend, puis les fait lui-meme).
const WAIT_KEEPER: Duration = Duration::from_secs(2);

pub fn default_names() -> Vec<String> {
    PORTS.iter().map(|s| s.to_string()).collect()
}

fn is_default(names: &[String]) -> bool {
    names.len() == PORTS.len() && names.iter().zip(PORTS).all(|(a, b)| a == b)
}

/// L'identifiant unique d'un port : fixe pour un nom (un hachage FNV-1a, positif, jamais 0), pour que les DAW
/// retrouvent le port d'un lancement a l'autre.
pub fn unique_id(name: &str, end: &str) -> i32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in name.bytes().chain(b"/".iter().copied()).chain(end.bytes()) {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    ((h & 0x3fff_ffff) | 0x1000_0000) as i32
}

fn cf(s: &str) -> CFStringRef {
    let c = std::ffi::CString::new(s).unwrap_or_default();
    unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) }
}

/// Le rappel de la destination : refcon porte le numero de la source du meme port.
extern "C" fn forward(list: *const c_void, refcon: *mut c_void, _src: *mut c_void) {
    let src = refcon as usize as MIDIObjectRef;
    if src != 0 && !list.is_null() {
        unsafe {
            MIDIReceived(src, list);
        }
    }
}

/// Cree ces ports dans ce processus → leurs noms. Erreur : le code de CoreMIDI (par exemple un serveur MIDI absent).
/// Le client et les ports vivent autant que le processus (macOS les retire a sa sortie).
fn create(names: &[String]) -> Result<Vec<String>, String> {
    let mut client: MIDIObjectRef = 0;
    let name = cf("Uppwind Link companion");
    let st = unsafe { MIDIClientCreate(name, None, std::ptr::null_mut(), &mut client) };
    unsafe { CFRelease(name) };
    if st != 0 {
        return Err(format!("MIDIClientCreate failed ({st})"));
    }
    let mut made = Vec::new();
    for p in names {
        let n = cf(p);
        let (mut src, mut dst) = (0, 0);
        let s1 = unsafe { MIDISourceCreate(client, n, &mut src) };
        let s2 = if s1 == 0 { unsafe { MIDIDestinationCreate(client, n, forward, src as usize as *mut c_void, &mut dst) } } else { s1 };
        unsafe { CFRelease(n) };
        if s1 != 0 || s2 != 0 {
            return Err(format!("cannot create the MIDI port \"{p}\" ({})", if s1 != 0 { s1 } else { s2 }));
        }
        unsafe {
            // deja pris (une autre copie du compagnon) : CoreMIDI garde l'identifiant qu'il avait donne
            let _ = MIDIObjectSetIntegerProperty(src, kMIDIPropertyUniqueID, unique_id(p, "source"));
            let _ = MIDIObjectSetIntegerProperty(dst, kMIDIPropertyUniqueID, unique_id(p, "destination"));
        }
        made.push(p.clone());
    }
    Ok(made)
}

/// Un client CoreMIDI qui ne sert qu'a voir les ports des autres processus : sans client, ou sans tourner la boucle de
/// ce fil, la liste de ce processus reste celle de son premier appel a CoreMIDI. Defait a la fin de `open`.
struct Watch(MIDIObjectRef);

extern "C" fn ignore_notification(_msg: *const c_void, _refcon: *mut c_void) {}

impl Watch {
    fn new() -> Watch {
        let mut c: MIDIObjectRef = 0;
        let n = cf("Uppwind Link companion (watch)");
        let st = unsafe { MIDIClientCreate(n, Some(ignore_notification), std::ptr::null_mut(), &mut c) };
        unsafe { CFRelease(n) };
        Watch(if st == 0 { c } else { 0 })
    }
    /// Laisse CoreMIDI mettre la liste a jour pendant `d` (la boucle de ce fil ; une boucle sans source rend la main
    /// tout de suite : le reste en sommeil).
    fn refresh(&self, d: Duration) {
        let t = Instant::now();
        unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, d.as_secs_f64(), 0) };
        if let Some(rest) = d.checked_sub(t.elapsed()) {
            std::thread::sleep(rest);
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe { MIDIClientDispose(self.0) };
        }
    }
}

/// --names "A,B" (le gardien, son arret, l'essai) → ces noms ; sans --names : None (les vrais ports).
fn names_from(raw: &[String]) -> Result<Option<Vec<String>>, String> {
    let Some(i) = raw.iter().position(|a| a == NAMES_FLAG) else { return Ok(None) };
    let v = raw.get(i + 1).filter(|v| !v.starts_with("--")).ok_or("--names needs a value: \"Name A,Name B\"")?;
    let names: Vec<String> = v.split(',').map(|s| s.trim().to_string()).collect();
    if names.len() > 8 || names.iter().any(|n| n.is_empty() || n.contains('\0')) {
        return Err("--names: 1 to 8 port names, separated by commas, none empty".into());
    }
    let mut d = names.clone();
    d.sort();
    d.dedup();
    if d.len() != names.len() {
        return Err("--names: the same name twice".into());
    }
    Ok(Some(names))
}

/// Le dossier du fichier du gardien : celui du fichier d'etat du compagnon (~, ou le bac a sable de --sandbox ;
/// --state-file n'y change rien : les ports sont ceux de la session, pas d'un compagnon).
pub fn keeper_dir() -> PathBuf {
    dir_of(&crate::wizard::state_file())
}

pub fn dir_of(state: &Path) -> PathBuf {
    state.parent().filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf).unwrap_or_else(crate::home_dir)
}

/// Le fichier du gardien de ces ports : ~/.uppwind-companion-midi-ports.pid pour les vrais, un autre (un hachage des
/// noms) pour les noms d'essai.
pub fn pid_file(dir: &Path, names: &[String]) -> PathBuf {
    const BASE: &str = ".uppwind-companion-midi-ports";
    if is_default(names) {
        return dir.join(format!("{BASE}.pid"));
    }
    let mut h: u32 = 0x811c_9dc5;
    for b in names.join("\n").bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    dir.join(format!("{BASE}-{h:08x}.pid"))
}

/// struct flock de macOS (sys/fcntl.h).
#[repr(C)]
struct Flock {
    l_start: i64,
    l_len: i64,
    l_pid: i32,
    l_type: i16,
    l_whence: i16,
}
const F_GETLK: i32 = 7;
const F_SETLK: i32 = 8;
const F_RDLCK: i16 = 1;
const F_UNLCK: i16 = 2;
const F_WRLCK: i16 = 3;

/// Le verrou du gardien (fcntl, en ecriture, tout le fichier ; il part avec le processus, meme tue) : pris → true ;
/// tenu par un autre processus → false.
fn try_lock(f: &File) -> std::io::Result<bool> {
    let mut l = Flock { l_start: 0, l_len: 0, l_pid: 0, l_type: F_WRLCK, l_whence: 0 };
    if unsafe { fcntl(f.as_raw_fd(), F_SETLK, &mut l as *mut Flock) } == 0 {
        return Ok(true);
    }
    let e = std::io::Error::last_os_error();
    // EAGAIN (35, macOS) ou EACCES (13) : un autre processus le tient
    if matches!(e.raw_os_error(), Some(35) | Some(13)) {
        Ok(false)
    } else {
        Err(e)
    }
}

/// Qui tient le verrou (F_GETLK : sans rien prendre, donc sans gener un gardien qui demarre) ; None : personne.
fn lock_owner(f: &File) -> std::io::Result<Option<u32>> {
    let mut l = Flock { l_start: 0, l_len: 0, l_pid: 0, l_type: F_RDLCK, l_whence: 0 };
    if unsafe { fcntl(f.as_raw_fd(), F_GETLK, &mut l as *mut Flock) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(if l.l_type == F_UNLCK || l.l_pid <= 0 { None } else { Some(l.l_pid as u32) })
}

fn pid_in(text: &str) -> Option<u32> {
    text.lines().next()?.trim().parse().ok().filter(|p| *p > 0)
}

fn read_pid(mut f: &File) -> Option<u32> {
    let mut s = String::new();
    f.seek(SeekFrom::Start(0)).ok()?;
    f.read_to_string(&mut s).ok()?;
    pid_in(&s)
}

/// Le gardien vivant de ce fichier : (le processus qui tient le verrou, le pid ecrit une fois ses ports faits) ;
/// None : aucun (pas de fichier, ou un fichier laisse par un gardien tue).
fn holder(path: &Path) -> Option<(u32, Option<u32>)> {
    let f = File::open(path).ok()?;
    let owner = lock_owner(&f).ok().flatten()?;
    Some((owner, read_pid(&f)))
}

fn same_inode(path: &Path, f: &File) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(path), f.metadata()) {
        (Ok(a), Ok(b)) => a.ino() == b.ino() && a.dev() == b.dev(),
        _ => false,
    }
}

/// Les arguments d'un processus de l'utilisateur (sysctl KERN_PROCARGS2) ; None : parti, ou pas a nous.
pub fn process_args(pid: u32) -> Option<Vec<String>> {
    const CTL_KERN: i32 = 1;
    const KERN_ARGMAX: i32 = 8;
    const KERN_PROCARGS2: i32 = 49;
    let mut argmax: i32 = 0;
    let mut len = std::mem::size_of::<i32>();
    let mut mib = [CTL_KERN, KERN_ARGMAX];
    if unsafe { sysctl(mib.as_mut_ptr(), 2, &mut argmax as *mut i32 as *mut c_void, &mut len, std::ptr::null_mut(), 0) } != 0 || argmax <= 0 {
        return None;
    }
    let mut buf = vec![0u8; argmax as usize];
    let mut len = buf.len();
    let mut mib = [CTL_KERN, KERN_PROCARGS2, i32::try_from(pid).ok()?];
    if unsafe { sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr() as *mut c_void, &mut len, std::ptr::null_mut(), 0) } != 0 {
        return None;
    }
    buf.truncate(len);
    parse_procargs(&buf)
}

/// KERN_PROCARGS2 : argc (un i32), le chemin de l'exe, des zeros, puis argv (puis l'environnement, ignore).
fn parse_procargs(buf: &[u8]) -> Option<Vec<String>> {
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?);
    if argc < 0 {
        return None;
    }
    if argc == 0 {
        return Some(Vec::new());
    }
    let mut rest = &buf[4..];
    let end = rest.iter().position(|b| *b == 0)?;
    rest = &rest[end..];
    let start = rest.iter().position(|b| *b != 0)?;
    rest = &rest[start..];
    let mut out = Vec::new();
    for _ in 0..argc {
        if rest.is_empty() {
            break;
        }
        let end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
        out.push(String::from_utf8_lossy(&rest[..end]).into_owned());
        rest = rest.get(end + 1..).unwrap_or(&[]);
    }
    Some(out)
}

/// Ce processus est-il un gardien des ports (le meme programme que le compagnon : son option le dit) ?
pub fn is_keeper(pid: u32) -> bool {
    process_args(pid).is_some_and(|a| a.iter().skip(1).any(|x| x == KEEPER_FLAG))
}

/// En tete de main() (apres wizard::early, pour le bac a sable) : le gardien, son arret, l'essai du demarrage → le code
/// de sortie ; None : rien de tout cela.
pub fn early(raw: &[String]) -> Option<i32> {
    let has = |f: &str| raw.iter().any(|a| a == f);
    if !has(KEEPER_FLAG) && !has(STOP_FLAG) && !has(TEST_FLAG) {
        return None;
    }
    let names = match names_from(raw) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("{e}");
            return Some(2);
        }
    };
    if has(KEEPER_FLAG) {
        return Some(keeper_main(names.unwrap_or_else(default_names)));
    }
    if has(STOP_FLAG) {
        return Some(stop_main(&names.unwrap_or_else(default_names)));
    }
    Some(test_main(names, raw))
}

/// Ce que dit le gardien tant qu'il a une sortie (lance a la main) ; lance par le compagnon, c'est /dev/null.
fn say(line: &str) {
    let _ = writeln!(std::io::stdout(), "{line}");
}

/// L'extremite du tube que le rappel des signaux ecrit (un octet : il faut sortir).
static WAKE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(_sig: i32) {
    let fd = WAKE.load(Ordering::Relaxed);
    if fd >= 0 {
        let b = 1u8;
        // write : permis dans un rappel de signal (et non bloquant : le tube plein ne bloque rien)
        unsafe { write(fd, &b as *const u8 as *const c_void, 1) };
    }
}

enum Acquired {
    /// le fichier, verrouille par ce processus
    Mine(File),
    /// un autre gardien tient deja ces ports (son pid)
    Kept(u32),
    Failed(String),
}

/// Attend que le gardien qui tient `f` ecrive son pid (il le fait une fois ses ports faits).
fn wait_pid(f: &File, d: Duration) -> Option<u32> {
    let t = Instant::now();
    loop {
        if let Some(p) = read_pid(f) {
            return Some(p);
        }
        if t.elapsed() >= d {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Attend que plus personne ne tienne le verrou de `f`.
fn wait_free(f: &File, d: Duration) -> bool {
    let t = Instant::now();
    loop {
        if matches!(lock_owner(f), Ok(None)) {
            return true;
        }
        if t.elapsed() >= d {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Le fichier du gardien, verrouille par ce processus ; sinon le gardien qui le tient deja.
fn acquire(path: &Path, names: &[String]) -> Acquired {
    let shown = crate::shown_path(path);
    // la liste des ports de ce processus n'est fraiche qu'a son premier appel a CoreMIDI : seul celui-la decide de
    // remplacer un gardien
    let mut looked = false;
    for _ in 0..3 {
        let f = match OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path) {
            Ok(f) => f,
            Err(e) => return Acquired::Failed(format!("cannot open {shown} ({e})")),
        };
        match try_lock(&f) {
            Ok(true) => {
                // remplace entre l'ouverture et le verrou (un gardien qui partait) : on recommence
                if !same_inode(path, &f) {
                    continue;
                }
                // (le pid d'un gardien tue, s'il en reste un, n'est plus vrai)
                let _ = f.set_len(0);
                return Acquired::Mine(f);
            }
            Ok(false) => {
                // un autre gardien vit : il ecrit son pid une fois ses ports faits (2 s au plus)
                let _ = wait_pid(&f, WAIT_KEEPER);
                let Some(owner) = lock_owner(&f).ok().flatten() else {
                    // parti pendant l'attente (arrete) : le compagnon fera les ports lui-meme
                    return Acquired::Failed("the keeper that held the ports has just stopped".into());
                };
                if looked {
                    // un troisieme, arrive pendant le remplacement : il fait ses ports (sa vue a lui est fraiche)
                    return Acquired::Kept(owner);
                }
                looked = true;
                if all_there(names) {
                    return Acquired::Kept(owner);
                }
                // vivant mais sans ses ports (le serveur MIDI a redemarre ?) : il est remplace
                if !is_keeper(owner) {
                    return Acquired::Failed(format!("{shown} is held by pid {owner}, which is not a MIDI ports keeper"));
                }
                let old = owner;
                unsafe { kill(old as i32, SIGTERM) };
                if !wait_free(&f, Duration::from_secs(3)) {
                    unsafe { kill(old as i32, SIGKILL) };
                    if !wait_free(&f, Duration::from_secs(1)) {
                        return Acquired::Failed(format!("the keeper pid {old} lost its ports and does not stop"));
                    }
                }
                // (il a retire son fichier en partant : le tour suivant en ouvre un neuf)
            }
            Err(e) => return Acquired::Failed(format!("cannot lock {shown} ({e})")),
        }
    }
    Acquired::Failed(format!("cannot take {shown}"))
}

fn write_pid(mut f: &File, names: &[String]) -> std::io::Result<()> {
    f.set_len(0)?;
    f.seek(SeekFrom::Start(0))?;
    f.write_all(format!("{}\n{}\n", std::process::id(), names.join("\n")).as_bytes())
}

/// Retire le fichier s'il est toujours le notre (un gardien qui le remplace en ouvre un neuf).
fn remove_if_ours(path: &Path, f: &File) {
    if same_inode(path, f) {
        let _ = std::fs::remove_file(path);
    }
}

/// Entree et sorties sur /dev/null (le gardien ne tient plus la console, le tube ou le journal de qui l'a lance).
fn detach_stdio() {
    if let Ok(null) = OpenOptions::new().read(true).write(true).open("/dev/null") {
        let fd = null.as_raw_fd();
        unsafe {
            dup2(fd, 0);
            dup2(fd, 1);
            dup2(fd, 2);
        }
    }
}

/// --midi-ports-keeper [--names "A,B"] : le gardien. Cree les ports, puis attend un signal, sans rien d'autre.
fn keeper_main(names: Vec<String>) -> i32 {
    // lance par le compagnon, c'est deja fait (avant exec) ; lance a la main dans un terminal, il garde sa session
    unsafe { setsid() };
    let list = names.join(", ");
    let path = pid_file(&keeper_dir(), &names);
    let file = match acquire(&path, &names) {
        Acquired::Mine(f) => f,
        Acquired::Kept(pid) => {
            say(&format!("The MIDI ports {list} are already kept by the keeper (pid {pid}): nothing to do."));
            return 0;
        }
        Acquired::Failed(e) => {
            say(&format!("MIDI ports keeper: {e}"));
            return 1;
        }
    };
    // la, mais pas d'un gardien (un compagnon d'avant, qui les fait lui-meme) : pas de second jeu du meme nom (le
    // premier appel a CoreMIDI, sauf apres un remplacement, ou la vue d'acquire les disait absents : toujours vrai)
    if all_there(&names) {
        remove_if_ours(&path, &file);
        say(&format!("The MIDI ports {list} already exist, made by another program: the keeper is not needed."));
        return EXIT_MADE_ELSEWHERE;
    }
    // un signal → un octet dans le tube → la sortie, proprement (le fichier retire) ; avant, l'action par defaut
    // suffit (rien a defaire : un fichier vide, que plus rien ne verrouille)
    let (mut wake_r, wake_w) = match std::os::unix::net::UnixStream::pair() {
        Ok(p) => p,
        Err(e) => {
            remove_if_ours(&path, &file);
            say(&format!("MIDI ports keeper: {e}"));
            return 1;
        }
    };
    let _ = wake_w.set_nonblocking(true);
    WAKE.store(wake_w.as_raw_fd(), Ordering::Relaxed);
    for s in [SIGTERM, SIGINT, SIGHUP] {
        unsafe { signal(s, on_signal) };
    }
    if let Err(e) = create(&names) {
        remove_if_ours(&path, &file);
        say(&format!("MIDI ports keeper: {e}"));
        return EXIT_COREMIDI;
    }
    if let Err(e) = write_pid(&file, &names) {
        remove_if_ours(&path, &file);
        say(&format!("MIDI ports keeper: cannot write {} ({e})", crate::shown_path(&path)));
        return 1;
    }
    say(&format!("MIDI ports keeper (pid {}): {list}. They stay until {STOP_FLAG} or --uninstall.", std::process::id()));
    detach_stdio();
    let mut b = [0u8; 1];
    loop {
        match wake_r.read(&mut b) {
            Ok(n) if n > 0 => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            // (ne devrait pas arriver : ce processus tient l'autre bout) ; jamais une boucle folle
            _ => std::thread::sleep(Duration::from_secs(1)),
        }
    }
    remove_if_ours(&path, &file);
    drop(wake_w);
    0
}

/// Ce que le demarrage du compagnon dit des ports : des lignes (la sortie) et des avertissements (la sortie d'erreur).
#[derive(Debug, Default)]
pub struct Opened {
    pub lines: Vec<String>,
    pub warnings: Vec<String>,
}

enum Ready {
    /// le gardien lance a fait ses ports (son pid)
    Keeper(u32),
    /// il est parti aussitot : les ports sont la quand meme (ce qu'on en dit)
    Elsewhere(String),
    Failed(String),
}

fn kept_line(list: &str, pid: u32) -> String {
    format!("MIDI ports (CoreMIDI): {list}, kept by the MIDI ports keeper (pid {pid}): they stay when the companion stops or restarts ({STOP_FLAG} removes them).")
}

fn elsewhere_line(list: &str) -> String {
    format!("MIDI ports: {list} already exist, made by another program (an older companion still running?): none made.")
}

/// Lance le gardien de ces ports, detache.
fn spawn(names: &[String]) -> Result<Child, String> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this program's own file: {e}"))?;
    // (le chemin d'exec peut etre relatif, « ./uppwind-companion » : le gardien tourne depuis /)
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let mut c = Command::new(&exe);
    c.arg(KEEPER_FLAG);
    if !is_default(names) {
        c.arg(NAMES_FLAG).arg(names.join(","));
    }
    c.args(crate::wizard::extra_args());
    c.current_dir("/").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY : setsid est permis entre fork et exec. Une session a lui des avant exec : la fin du groupe de processus
    // du compagnon (Ctrl+C, un terminal ferme, l'arret de son LaunchAgent) ne l'atteint jamais.
    unsafe {
        c.pre_exec(|| {
            setsid();
            Ok(())
        });
    }
    c.spawn().map_err(|e| format!("cannot start {}: {e}", crate::shown_path(&exe)))
}

/// Attend (2 s au plus) que le gardien lance ait fait ses ports : son pid ecrit, et ses quatre points visibles ici.
fn wait_ready(child: &mut Child, names: &[String], path: &Path, watch: &Watch) -> Ready {
    let list = names.join(", ");
    let me = child.id();
    let t0 = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                return match st.code() {
                    // un autre gardien les tient deja (lance au meme moment)
                    Some(0) => match holder(path) {
                        Some((pid, _)) => Ready::Elsewhere(kept_line(&list, pid)),
                        None => Ready::Failed("it stopped at once".into()),
                    },
                    Some(EXIT_MADE_ELSEWHERE) => Ready::Elsewhere(elsewhere_line(&list)),
                    Some(c) => Ready::Failed(format!("it stopped at once, exit code {c}")),
                    None => Ready::Failed("it was killed".into()),
                }
            }
            Ok(None) => {}
            Err(e) => return Ready::Failed(e.to_string()),
        }
        watch.refresh(Duration::from_millis(50));
        let written = holder(path).and_then(|(_, p)| p) == Some(me);
        if written && all_there(names) {
            return Ready::Keeper(me);
        }
        if t0.elapsed() >= WAIT_KEEPER {
            return Ready::Failed(if written { "its ports did not show up within 2 s".into() } else { "no answer within 2 s".into() });
        }
    }
}

/// Un gardien lance qui n'a pas repondu : arrete (puis tue), pour que les ports du compagnon ne fassent pas doublon.
fn stop_child(child: &mut Child) {
    unsafe { kill(child.id() as i32, SIGTERM) };
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(1) {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Au demarrage du compagnon : les ports `names`. Deja la → rien ; sinon le gardien (`dir` : le dossier de son
/// fichier), et s'il ne vient pas, les ports dans ce processus, comme avant.
pub fn open(names: &[String], dir: &Path) -> Opened {
    let mut o = Opened::default();
    let list = names.join(", ");
    let path = pid_file(dir, names);
    // avant tout autre appel a CoreMIDI : un client, pour voir arriver les ports du gardien
    let watch = Watch::new();
    if all_there(names) {
        o.lines.push(match holder(&path) {
            Some((pid, _)) => kept_line(&list, pid),
            None => elsewhere_line(&list),
        });
        return o;
    }
    let why = match spawn(names) {
        Ok(mut child) => match wait_ready(&mut child, names, &path, &watch) {
            Ready::Keeper(pid) => {
                // le gardien reste notre fils tant que le compagnon vit : recueilli s'il s'arrete avant lui
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                o.lines.push(kept_line(&list, pid));
                return o;
            }
            Ready::Elsewhere(line) => {
                o.lines.push(line);
                return o;
            }
            Ready::Failed(why) => {
                stop_child(&mut child);
                why
            }
        },
        Err(e) => e,
    };
    drop(watch);
    o.warnings.push(format!("the MIDI ports keeper did not start ({why}): the companion makes the ports itself, for as long as it runs"));
    match create(names) {
        Ok(p) => o.lines.push(format!("MIDI ports (CoreMIDI, while the companion runs): {}", p.join(", "))),
        Err(e) => o.warnings.push(format!("{e} (make the ports in Audio MIDI Setup › IAC Driver instead)")),
    }
    o
}

/// Attend que `pid` ne tienne plus le fichier (parti : il l'a retire, ou tue : le verrou est parti avec lui).
fn wait_gone(path: &Path, pid: u32, d: Duration) -> bool {
    let t = Instant::now();
    loop {
        if holder(path).map(|(o, _)| o) != Some(pid) {
            return true;
        }
        if t.elapsed() >= d {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// --stop-midi-ports, --uninstall : le gardien de ces ports (son fichier dans `dir`, puis le processus) arrete → ce qu'on
/// en dit ; Ok(None) : aucun gardien.
pub fn stop_keeper(dir: &Path, names: &[String]) -> Result<Option<String>, String> {
    let path = pid_file(dir, names);
    if !path.exists() {
        return Ok(None);
    }
    let shown = crate::shown_path(&path);
    let Some((pid, _)) = holder(&path) else {
        // personne ne le tient : un gardien tue (SIGKILL) ou une machine eteinte d'un coup
        let _ = std::fs::remove_file(&path);
        return Ok(Some(format!("Removed {shown}, left by a MIDI ports keeper that is gone.")));
    };
    if !is_keeper(pid) {
        return Err(format!("{shown} is held by pid {pid}, which is not a MIDI ports keeper: left alone."));
    }
    unsafe { kill(pid as i32, SIGTERM) };
    if !wait_gone(&path, pid, Duration::from_secs(3)) {
        unsafe { kill(pid as i32, SIGKILL) };
        if !wait_gone(&path, pid, Duration::from_secs(2)) {
            return Err(format!("cannot stop the MIDI ports keeper (pid {pid})."));
        }
    }
    // (il le retire en partant ; tue, il le laisse) ; jamais celui d'un gardien neuf
    if holder(&path).is_none() {
        let _ = std::fs::remove_file(&path);
    }
    Ok(Some(format!("Stopped the MIDI ports keeper (pid {pid}): the MIDI ports {} are gone.", names.join(", "))))
}

fn stop_main(names: &[String]) -> i32 {
    match stop_keeper(&keeper_dir(), names) {
        Ok(Some(l)) => {
            println!("{l}");
            0
        }
        Ok(None) => {
            println!("No MIDI ports keeper is running.");
            1
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// --midi-ports-test --names "A,B" [--seconds S] (essais seulement) : ce que fait le demarrage du compagnon avec ces
/// noms, puis S secondes (0 par defaut) et la sortie, comme un compagnon qui s'arrete. Jamais les vrais ports.
fn test_main(names: Option<Vec<String>>, raw: &[String]) -> i32 {
    let Some(names) = names else {
        eprintln!("{TEST_FLAG} needs {NAMES_FLAG} \"Name A,Name B\" (test names: never the real ports)");
        return 2;
    };
    if names.iter().any(|n| PORTS.contains(&n.as_str())) {
        eprintln!("{TEST_FLAG}: test names only, never \"{}\"", PORTS.join("\" or \""));
        return 2;
    }
    let secs = raw.iter().position(|a| a == "--seconds").and_then(|i| raw.get(i + 1)).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    let t0 = Instant::now();
    let o = open(&names, &keeper_dir());
    for l in &o.lines {
        say(l);
    }
    for w in &o.warnings {
        let _ = writeln!(std::io::stderr(), "warning: {w}");
    }
    say(&format!("(pid {}, {} ms; exits in {secs} s)", std::process::id(), t0.elapsed().as_millis()));
    std::thread::sleep(Duration::from_secs_f64(secs.clamp(0.0, 3600.0)));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn unique_ids_are_fixed_distinct_and_positive() {
        let all: Vec<i32> = PORTS.iter().flat_map(|p| [unique_id(p, "source"), unique_id(p, "destination")]).collect();
        assert!(all.iter().all(|i| *i > 0));
        let mut d = all.clone();
        d.sort();
        d.dedup();
        assert_eq!(d.len(), 4);
        assert_eq!(unique_id("Uppwind Link", "source"), unique_id("Uppwind Link", "source"));
    }

    #[test]
    fn test_names_come_only_from_the_keepers_own_option() {
        assert_eq!(names_from(&s(&[KEEPER_FLAG])).unwrap(), None, "the real ports by default");
        assert_eq!(names_from(&s(&[KEEPER_FLAG, "--names", "UWL Test A, UWL Test A Back"])).unwrap(), Some(s(&["UWL Test A", "UWL Test A Back"])));
        assert!(names_from(&s(&["--names"])).is_err());
        assert!(names_from(&s(&["--names", "--sandbox"])).is_err());
        assert!(names_from(&s(&["--names", "A,,B"])).is_err());
        assert!(names_from(&s(&["--names", "A,A"])).is_err());
        assert!(is_default(&default_names()) && !is_default(&s(&["UWL Test A", "UWL Test A Back"])));
    }

    #[test]
    fn a_test_keeper_never_shares_the_real_keepers_file() {
        let d = Path::new("/Users/x");
        assert_eq!(pid_file(d, &default_names()), PathBuf::from("/Users/x/.uppwind-companion-midi-ports.pid"));
        let t = pid_file(d, &s(&["UWL Test A", "UWL Test A Back"]));
        assert_ne!(t, pid_file(d, &default_names()));
        assert_eq!(t, pid_file(d, &s(&["UWL Test A", "UWL Test A Back"])));
        assert_ne!(t, pid_file(d, &s(&["UWL Test B", "UWL Test B Back"])));
        assert!(t.file_name().unwrap().to_string_lossy().starts_with(".uppwind-companion-midi-ports-"));
        assert_eq!(dir_of(Path::new("/Users/x/.uppwind-companion.json")), PathBuf::from("/Users/x"));
    }

    #[test]
    fn the_keeper_is_told_by_its_arguments() {
        let mut b = 3i32.to_ne_bytes().to_vec();
        b.extend_from_slice(b"/x/uppwind-companion\0\0\0\0uppwind-companion\0--midi-ports-keeper\0--names\0HOME=/x\0");
        assert_eq!(parse_procargs(&b).unwrap(), s(&["uppwind-companion", "--midi-ports-keeper", "--names"]));
        assert_eq!(parse_procargs(&0i32.to_ne_bytes()).unwrap(), Vec::<String>::new());
        assert!(parse_procargs(&[1, 0]).is_none());
        // ce processus-ci (le programme des essais) : ses arguments se lisent, et ce n'est pas un gardien
        assert!(process_args(std::process::id()).is_some_and(|a| !a.is_empty()));
        assert!(!is_keeper(std::process::id()));
        assert!(!is_keeper(0) && !is_keeper(u32::MAX));
    }

    #[test]
    fn the_lock_tells_a_live_keeper_from_a_file_left_behind() {
        extern "C" {
            fn fork() -> i32;
            fn _exit(code: i32) -> !;
            fn sleep(s: u32) -> u32;
            fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
        }
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("uwc-keeper-lock-{}-{t}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("k.pid");
        std::fs::write(&path, "4242\n").unwrap();
        assert!(holder(&path).is_none(), "a file nobody holds: no keeper");
        let f = OpenOptions::new().read(true).write(true).open(&path).unwrap();
        let (mut ready_r, ready_w) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut l = Flock { l_start: 0, l_len: 0, l_pid: 0, l_type: F_WRLCK, l_whence: 0 };
        let fd = f.as_raw_fd();
        let wfd = ready_w.as_raw_fd();
        // un autre processus prend le verrou (le fils ne fait que des appels permis apres fork)
        let child = unsafe { fork() };
        if child == 0 {
            unsafe {
                let ok = fcntl(fd, F_SETLK, &mut l as *mut Flock) == 0;
                let b: u8 = if ok { 1 } else { 0 };
                write(wfd, &b as *const u8 as *const c_void, 1);
                for _ in 0..30 {
                    sleep(1);
                }
                _exit(0);
            }
        }
        assert!(child > 0);
        let mut b = [0u8; 1];
        ready_r.read_exact(&mut b).unwrap();
        assert_eq!(b[0], 1, "the child took the lock");
        assert_eq!(holder(&path), Some((child as u32, Some(4242))), "held: alive, and its pid");
        assert!(!try_lock(&f).unwrap(), "a second keeper cannot take it");
        unsafe { kill(child, SIGKILL) };
        let mut st = 0;
        unsafe { waitpid(child, &mut st, 0) };
        assert!(holder(&path).is_none(), "killed: the lock went with it");
        assert!(try_lock(&f).unwrap());
        assert_eq!(stop_keeper(&dir, &s(&["k"])).unwrap(), None, "another name: another file");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
