// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (19 nov., etape 1 de « le compagnon comme hote VST3 », a-lire/hote-vst3-compagnon-19nov.md) Le compagnon heberge
//! lui-meme les plugins VST3 : Uppwind Studio les charge, les joue et les affiche SANS DAW ouvert.
//!
//! - Le scan : les dossiers VST3 standard (Common Files\VST3, %LOCALAPPDATA%\Programs\Common\VST3), un module a la fois
//!   dans un processus a part (`--plug-scan`, SCAN_TIMEOUT au plus : un plugin qui plante ou se bloque est note et
//!   saute), en fond ; le cache (%LOCALAPPDATA%\Uppwind Link\vst3-scan.json, par chemin + taille + date) : seuls les
//!   modules nouveaux ou changes sont relus. Le studio ne voit que des identifiants de classe (32 chiffres), JAMAIS
//!   un chemin, et ne peut faire charger que ce que le scan a trouve dans ces dossiers.
//! - Un plugin = un processus enfant (`--plug-host`, uwl_vst3host::child) : un plugin qui plante ne tue ni le
//!   compagnon ni les autres ; l'enfant s'arrete seul si le compagnon disparait (la fin de son entree).
//! - Les notes et le son passent par le chemin mesure le 19 nov. : nos plugins Uppwind Link charges DANS l'enfant
//!   (Notes, son « Link tag » ; Return), connectes a ce compagnon (plugin_link) comme ceux d'un DAW.
//! - L'etat : l'enfant l'ecrit dans %LOCALAPPDATA%\Uppwind Link\plugin-states\<sha256>.bin ; le projet du studio n'en
//!   garde que l'empreinte. Un etat que ce compagnon n'a pas produit n'existe pas ici : le plugin s'ouvre alors avec
//!   ses reglages par defaut (jamais des octets venus d'un projet partage dans setState).
//!
//! Studio → compagnon (arme seulement, comme la capture) : {"type":"host","action":…}
//!   status → {type:"host",action:"status",available,scanning,done,total,count}
//!   scan   → relance le scan (les modules changes) ; plugins → {action:"plugins",list:[{id,name,vendor,kind}],scanning}
//!   add {plugin, state?} → {action:"added",item} ; remove {id} → {action:"removed",id}
//!   list   → {action:"list",items:[{id,name,plugin,vendor,tag,pid,status,editor}]}
//!   save {id, n?} → {action:"saved",id,n,state,bytes} ; editor {id, show} → {action:"editor",id,shown} (sur le bureau)
//!   view {id, open} → {action:"view",id,open} : l'editeur ouvert MASQUE, pour que le studio le filme (sa fenetre entre
//!   dans la liste des fenetres : `windows`, host « host », track « #<id> »)
//!   tempo {bpm, playing?} → (tous les plugins heberges) ; tempo {bpm, id, from, offset, playing} → l'ancre du morceau de
//!   l'effet en direct `id` (rien en retour) ; mix {id, vol, pan, mute} → le volume de sa piste (rien en retour)
//! Pousse : {type:"host",event:"changed"} quand la liste change (un plugin pret, parti, plante).

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

const SCAN_TIMEOUT: Duration = Duration::from_secs(25);
const READY_TIMEOUT: Duration = Duration::from_secs(60);
const SAVE_TIMEOUT: Duration = Duration::from_secs(15);
/// Etape 1 : des plugins heberges en meme temps, au plus (la memoire : Kontakt prend 1 Go avec un instrument)
const MAX_INSTANCES: usize = 16;
/// Les « Link tag » des plugins heberges : depuis 255 vers le bas (le script de Live prend les petits numeros)
const TAG_TOP: u32 = 255;
const TAG_BOTTOM: u32 = 200;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct Cfg {
    exe: PathBuf,
    state_file: PathBuf,
    data: PathBuf,
    dirs: Vec<PathBuf>,
    link_bundle: PathBuf,
}

static CFG: OnceLock<Cfg> = OnceLock::new();
static GENERATION: AtomicU64 = AtomicU64::new(1);


// ── (20 nov., relecture de securite) ─────────────────────────────────────────────────────────────────────────────────
/// Les places prises par des « add » en cours (lancement → pret), et leurs Link tags : comptees avec les instances, sous
/// le meme verrou, pour que des demandes simultanees ne depassent jamais MAX_INSTANCES ni ne partagent un tag.
fn reserved() -> &'static Mutex<(usize, Vec<u32>)> {
    static R: OnceLock<Mutex<(usize, Vec<u32>)>> = OnceLock::new();
    R.get_or_init(|| Mutex::new((0, Vec::new())))
}
/// Une place reservee : rendue quand l'« add » se termine, quelle qu'en soit l'issue.
struct Slot(u32, bool);
impl Drop for Slot {
    fn drop(&mut self) {
        let mut r = reserved().lock().unwrap_or_else(|e| e.into_inner());
        r.0 = r.0.saturating_sub(1);
        if !self.1 {
            r.1.retain(|t| *t != self.0);
        }
    }
}

/// L'armement (main.rs le donne) : desarme, les editeurs des plugins heberges se ferment et ne sont plus filmes.
static ARMED: OnceLock<Box<dyn Fn() -> bool + Send + Sync>> = OnceLock::new();
fn armed_now() -> bool {
    ARMED.get().map(|f| f()).unwrap_or(true)
}
/// main.rs, une fois l'armement cree : le fil qui le surveille ferme les editeurs a chaque desarmement (la personne, ou
/// l'arret automatique apres 10 minutes sans studio).
pub fn watch_arming(probe: Box<dyn Fn() -> bool + Send + Sync>) {
    if ARMED.set(probe).is_err() {
        return;
    }
    std::thread::spawn(|| {
        let mut was = armed_now();
        loop {
            std::thread::sleep(Duration::from_millis(500));
            let now = armed_now();
            if was && !now {
                close_editors();
            }
            was = now;
        }
    });
}
/// Ferme l'editeur (montre sur le bureau ou ouvert cache pour le studio) de chaque plugin heberge.
fn close_editors() {
    let list: Vec<Arc<Inst>> = insts().lock().unwrap().clone();
    let mut n = 0;
    for i in list {
        if *i.status.lock().unwrap() != "ready" {
            continue;
        }
        if i.view.lock().unwrap().is_some() {
            let _ = send_cmd(&i, json!({ "cmd": "view", "open": false }));
            n += 1;
        }
        if i.editor.load(Ordering::Relaxed) {
            let _ = send_cmd(&i, json!({ "cmd": "editor", "show": false }));
            n += 1;
        }
    }
    if n > 0 {
        crate::log(&format!("plugin host: remote control is off — {n} plugin window(s) closed"));
    }
}

/// Les processus des plugins (et du scan) meurent avec le compagnon, meme s'il est tue ou plante : un Job object
/// « KILL_ON_JOB_CLOSE » (sa poignee n'est jamais fermee : elle part avec le compagnon, et ses enfants avec elle).
#[cfg(windows)]
fn tie_to_companion(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
    };
    static JOB: OnceLock<usize> = OnceLock::new();
    let j = *JOB.get_or_init(|| unsafe {
        let Ok(h) = CreateJobObjectW(None, windows::core::PCWSTR::null()) else { return 0 };
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        // (2e relecture) SILENT_BREAKAWAY_OK : ce qu'un plugin lance lui-meme (un navigateur, un gestionnaire de licences)
        // n'entre pas dans le Job object, et survit au compagnon
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK;
        let size = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
        if SetInformationJobObject(h, JobObjectExtendedLimitInformation, &info as *const _ as *const core::ffi::c_void, size).is_err() {
            return 0;
        }
        h.0 as usize
    });
    if j != 0 {
        unsafe {
            if AssignProcessToJobObject(HANDLE(j as *mut core::ffi::c_void), HANDLE(child.as_raw_handle())).is_err() {
                crate::log("plugin host: could not tie a plugin process to the companion");
            }
        }
    }
}
#[cfg(not(windows))]
fn tie_to_companion(_child: &Child) {}

/// Une erreur pour le studio, sans chemin de fichier (celui d'un module de %LOCALAPPDATA% porte le nom de la session
/// Windows) : chaque « X:\… » devient « (path) », jusqu'au « : » suivant, a une guillemet ou a la fin. Le message
/// entier reste dans le journal du compagnon.
pub fn no_paths(e: &str) -> String {
    let b = e.as_bytes();
    let mut out = String::with_capacity(e.len());
    let mut i = 0;
    while i < b.len() {
        let drive = i + 2 < b.len() && b[i].is_ascii_alphabetic() && b[i + 1] == b':' && (b[i + 2] == b'\\' || b[i + 2] == b'/')
            && (i == 0 || !b[i - 1].is_ascii_alphanumeric());
        let unc = i + 1 < b.len() && b[i] == b'\\' && b[i + 1] == b'\\';
        if drive || unc {
            let mut j = i + 2;
            while j < b.len() && !(b[j] == b'"' || b[j] == b'\'' || (b[j] == b':' && j + 1 < b.len() && b[j + 1] == b' ')) {
                j += 1;
            }
            out.push_str("(path)");
            i = j;
            continue;
        }
        let ch = e[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn changed() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// La generation de la liste (un changement : le studio relit).
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// Au demarrage du serveur (main.rs), Windows seulement : le fichier d'etat que lisent nos plugins (les enfants le
/// passent aux leurs), les dossiers.
pub fn configure(state_file: PathBuf) {
    let Ok(exe) = std::env::current_exe() else { return };
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let Some(local) = local else { return };
    let data = local.join(crate::install::APP_DIR);
    let mut dirs = Vec::new();
    if let Some(pf) = std::env::var_os("CommonProgramFiles").map(PathBuf::from) {
        dirs.push(pf.join("VST3"));
    }
    dirs.push(local.join("Programs").join("Common").join("VST3"));
    let link_bundle = dirs.first().map(|d| d.join("Uppwind Link.vst3")).unwrap_or_default();
    let _ = CFG.set(Cfg { exe, state_file, data, dirs, link_bundle });
    sweep_jobs();
}

pub fn available() -> bool {
    CFG.get().is_some()
}

fn cfg() -> Result<&'static Cfg, String> {
    CFG.get().ok_or_else(|| "this companion does not host plugins (Windows only, with the plugin listener on)".into())
}

// ── Le scan ─────────────────────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct ModuleRec {
    path: String,
    size: u64,
    mtime: u64,
    ok: bool,
    error: Option<String>,
    classes: Vec<Value>,
}

#[derive(Default)]
struct ScanState {
    loaded: bool,
    running: bool,
    done: usize,
    total: usize,
    modules: Vec<ModuleRec>,
}

fn scan_state() -> &'static Mutex<ScanState> {
    static S: OnceLock<Mutex<ScanState>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(ScanState::default()))
}

fn cache_path(c: &Cfg) -> PathBuf {
    c.data.join("vst3-scan.json")
}

fn load_cache(c: &Cfg) -> Vec<ModuleRec> {
    let Ok(t) = std::fs::read_to_string(cache_path(c)) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<Value>(&t) else { return Vec::new() };
    v["modules"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    Some(ModuleRec {
                        path: m["path"].as_str()?.to_string(),
                        size: m["size"].as_u64()?,
                        mtime: m["mtime"].as_u64()?,
                        ok: m["ok"].as_bool().unwrap_or(false),
                        error: m["error"].as_str().map(String::from),
                        classes: m["classes"].as_array().cloned().unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn save_cache(c: &Cfg, mods: &[ModuleRec]) {
    let v = json!({ "v": 1, "modules": mods.iter().map(|m| json!({ "path": m.path, "size": m.size, "mtime": m.mtime, "ok": m.ok, "error": m.error, "classes": m.classes })).collect::<Vec<_>>() });
    let _ = std::fs::create_dir_all(&c.data);
    let tmp = cache_path(c).with_extension("tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, cache_path(c));
    }
}

/// Les modules VST3 d'un dossier : un « .vst3 » (un fichier : l'ancienne forme ; un dossier : un paquet, pas plus bas),
/// 6 niveaux au plus (les dossiers des fabricants), sans nos propres plugins.
pub fn find_modules(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        let is_vst3 = name.to_ascii_lowercase().ends_with(".vst3");
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if is_vst3 {
            if !name.eq_ignore_ascii_case("Uppwind Link.vst3") {
                out.push(p);
            }
        } else if ft.is_dir() && depth > 0 {
            find_modules(&p, depth - 1, out);
        }
    }
}

/// La taille et la date d'un module (un paquet : celles du binaire qu'il contient).
fn stamp(p: &Path) -> (u64, u64) {
    let bin = uwl_vst3host::vsthost::dll_of_bundle(p);
    let m = std::fs::metadata(&bin).or_else(|_| std::fs::metadata(p));
    match m {
        Ok(m) => (m.len(), m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)),
        Err(_) => (0, 0),
    }
}

fn hidden(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Un module dans un processus a part : ses classes, ou pourquoi pas.
fn scan_one(c: &Cfg, p: &Path) -> Result<Vec<Value>, String> {
    let mut cmd = Command::new(&c.exe);
    cmd.arg("--plug-scan").arg(p).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    hidden(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("cannot start the scan ({e})"))?;
    tie_to_companion(&child);
    let stdout = child.stdout.take().ok_or("no output")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if v.get("ok").is_some() {
                    let _ = tx.send(v);
                    return;
                }
            }
        }
    });
    let r = rx.recv_timeout(SCAN_TIMEOUT);
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(3) {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    match r {
        Ok(v) if v["ok"] == true => Ok(v["classes"].as_array().cloned().unwrap_or_default()),
        Ok(v) => Err(v["error"].as_str().unwrap_or("the module could not be loaded").to_string()),
        Err(_) => Err("the module did not answer (crashed or too slow)".into()),
    }
}

/// Lance le scan en fond (s'il ne tourne pas) : seuls les modules nouveaux ou changes sont relus.
pub fn ensure_scan(force: bool) {
    let Ok(c) = cfg() else { return };
    {
        let mut s = scan_state().lock().unwrap();
        if !s.loaded {
            s.modules = load_cache(c);
            s.loaded = true;
        }
        if s.running || (!force && s.total > 0 && s.done == s.total) {
            return;
        }
        s.running = true;
    }
    std::thread::spawn(move || {
        let mut found = Vec::new();
        for d in &c.dirs {
            find_modules(d, 6, &mut found);
        }
        let old: HashMap<String, ModuleRec> = scan_state().lock().unwrap().modules.iter().map(|m| (m.path.clone(), m.clone())).collect();
        {
            let mut s = scan_state().lock().unwrap();
            s.total = found.len();
            s.done = 0;
        }
        changed();
        let mut mods = Vec::with_capacity(found.len());
        for p in &found {
            let key = p.to_string_lossy().to_string();
            let (size, mtime) = stamp(p);
            let rec = match old.get(&key) {
                Some(m) if m.size == size && m.mtime == mtime => m.clone(),
                _ => {
                    crate::log(&format!("plugin host: scanning {key}"));
                    match scan_one(c, p) {
                        Ok(classes) => ModuleRec { path: key, size, mtime, ok: true, error: None, classes },
                        Err(e) => {
                            crate::log(&format!("plugin host: {} skipped ({e})", p.display()));
                            ModuleRec { path: key, size, mtime, ok: false, error: Some(e), classes: Vec::new() }
                        }
                    }
                }
            };
            mods.push(rec);
            let mut s = scan_state().lock().unwrap();
            s.done += 1;
            // le cache est ecrit au fur et a mesure (un scan interrompu garde ce qu'il a lu)
            let mut merged = mods.clone();
            merged.extend(old.values().filter(|m| !mods.iter().any(|x| x.path == m.path) && found.iter().any(|f| f.to_string_lossy() == m.path)).cloned());
            s.modules = merged;
            save_cache(c, &s.modules);
        }
        let mut s = scan_state().lock().unwrap();
        s.modules = mods;
        save_cache(c, &s.modules);
        s.running = false;
        drop(s);
        crate::log("plugin host: scan done");
        changed();
    });
}

fn kind_of(sub: &str) -> &'static str {
    if sub.contains("Instrument") {
        "instrument"
    } else if sub.contains("Fx") {
        "effect"
    } else {
        "other"
    }
}

/// L'identifiant de classe → (le module, la classe). Seulement ce que le scan a trouve.
fn lookup(cid: &str) -> Option<(PathBuf, Value)> {
    let s = scan_state().lock().unwrap();
    for m in s.modules.iter().filter(|m| m.ok) {
        if let Some(c) = m.classes.iter().find(|c| c["cid"].as_str() == Some(cid)) {
            return Some((PathBuf::from(&m.path), c.clone()));
        }
    }
    None
}

fn plugins_json() -> Value {
    ensure_scan(false);
    let s = scan_state().lock().unwrap();
    let mut seen = std::collections::HashSet::new();
    let mut list = Vec::new();
    for m in s.modules.iter().filter(|m| m.ok) {
        for c in &m.classes {
            let Some(id) = c["cid"].as_str() else { continue };
            if !seen.insert(id.to_string()) {
                continue;
            }
            let kind = kind_of(c["sub"].as_str().unwrap_or(""));
            list.push(json!({ "id": id, "name": c["name"], "vendor": c["vendor"], "kind": kind, "version": c["version"] }));
        }
    }
    list.sort_by(|a, b| a["name"].as_str().unwrap_or("").to_lowercase().cmp(&b["name"].as_str().unwrap_or("").to_lowercase()));
    let failed = s.modules.iter().filter(|m| !m.ok).count();
    json!({ "type": "host", "action": "plugins", "list": list, "scanning": s.running, "done": s.done, "total": s.total, "failed": failed })
}

// ── Les plugins heberges ────────────────────────────────────────────────────────────────────────────────────────

struct Inst {
    id: u32,
    /// (19 nov., etape 3) un effet (rendu hors temps reel), pas un instrument
    effect: bool,
    cid: String,
    name: String,
    vendor: String,
    tag: u32,
    pid: u32,
    /// (19 nov., etape 3b) l'entree de l'enfant : un fil l'ecrit (WRITE_QUEUE lignes au plus en attente). Les ordres
    /// attendent leur place ; le son d'un effet en direct, s'il n'y en a pas, est jete et compte (fx_dropped).
    tx: SyncSender<String>,
    /// (etape 3b) ou va le son d'un effet en direct : sa generation et la session qui l'a ouvert
    fx_sink: Mutex<Option<(u32, SyncSender<FxOut>)>>,
    fx_dropped: AtomicU64,
    child: Mutex<Child>,
    status: Mutex<String>,
    error: Mutex<Option<String>>,
    editor: AtomicBool,
    /// l'editeur est ouvert (filme ou montre) ; sa taille
    view: Mutex<Option<(i32, i32)>>,
    ready: Mutex<Option<Value>>,
    waiters: Mutex<HashMap<u64, Sender<Value>>>,
}

/// (19 nov., etape 3b) Les lignes en attente vers un enfant (un effet en direct : 200 paquets par seconde ; 512 : 2,5 s).
const WRITE_QUEUE: usize = 512;
/// Un ordre attend sa place au plus (un enfant qui ne lit plus est un enfant arrete).
const WRITE_WAIT: Duration = Duration::from_secs(5);
/// (etape 3b) La reponse d'un effet qu'on met en direct (il redemarre son traitement).
const LIVE_TIMEOUT: Duration = Duration::from_secs(15);

pub use crate::session::FxOut;

static NEXT_ID: AtomicU32 = AtomicU32::new(1);
static NEXT_N: AtomicU64 = AtomicU64::new(1);

fn insts() -> &'static Mutex<Vec<Arc<Inst>>> {
    static I: OnceLock<Mutex<Vec<Arc<Inst>>>> = OnceLock::new();
    I.get_or_init(|| Mutex::new(Vec::new()))
}

fn find(id: u32) -> Result<Arc<Inst>, String> {
    insts().lock().unwrap().iter().find(|i| i.id == id).cloned().ok_or_else(|| format!("no hosted plugin {id}"))
}

fn item_json(i: &Inst) -> Value {
    let ready = i.ready.lock().unwrap().clone().unwrap_or(Value::Null);
    json!({ "id": i.id, "name": i.name, "plugin": i.cid, "vendor": i.vendor, "tag": i.tag, "pid": i.pid, "kind": if i.effect { "effect" } else { "instrument" },
            "status": *i.status.lock().unwrap(), "error": i.error.lock().unwrap().as_deref().map(no_paths), "editor": i.editor.load(Ordering::Relaxed), "view": i.view.lock().unwrap().is_some(),
            "hasEditor": ready["editor"], "w": ready["w"], "h": ready["h"], "hwnd": ready["hwnd"] })
}

fn send_cmd(i: &Inst, v: Value) -> Result<(), String> {
    let mut line = v.to_string();
    let t0 = Instant::now();
    loop {
        match i.tx.try_send(line) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Disconnected(_)) => return Err(format!("{} has stopped", i.name)),
            Err(TrySendError::Full(l)) => {
                if t0.elapsed() > WRITE_WAIT {
                    return Err(format!("{} does not answer", i.name));
                }
                line = l;
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}

/// (19 nov., etape 3b) Le fil qui ecrit l'entree d'un enfant : il s'arrete quand l'enfant est parti (l'ecriture echoue)
/// ou quand l'instance est oubliee (tous les envoyeurs lâches) : l'entree se ferme, l'enfant s'arrete.
fn writer(mut stdin: ChildStdin, rx: mpsc::Receiver<String>) {
    std::thread::spawn(move || {
        while let Ok(l) = rx.recv() {
            if writeln!(stdin, "{l}").and_then(|_| stdin.flush()).is_err() {
                break;
            }
        }
    });
}

fn reader(inst: Arc<Inst>, stdout: std::process::ChildStdout, ready_tx: Sender<Result<Value, String>>) {
    std::thread::spawn(move || {
        let mut ready_tx = Some(ready_tx);
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            match v["event"].as_str().unwrap_or("") {
                "ready" => {
                    *inst.ready.lock().unwrap() = Some(v.clone());
                    *inst.status.lock().unwrap() = "ready".into();
                    if let Some(t) = ready_tx.take() {
                        let _ = t.send(Ok(v));
                    }
                    changed();
                }
                "editor" => {
                    inst.editor.store(v["shown"].as_bool().unwrap_or(false), Ordering::Relaxed);
                    changed();
                }
                // (l'editeur s'ouvre a la demande) ouvert, avec sa taille, ou ferme
                "view" => {
                    let open = v["open"].as_bool().unwrap_or(false);
                    *inst.view.lock().unwrap() = if open { Some((v["w"].as_i64().unwrap_or(0) as i32, v["h"].as_i64().unwrap_or(0) as i32)) } else { None };
                    if let Some(r) = inst.ready.lock().unwrap().as_mut() {
                        r["w"] = v["w"].clone();
                        r["h"] = v["h"].clone();
                    }
                    changed();
                }
                // (19 nov., etape 3b) le son d'un effet en direct : a la session qui l'a ouvert (sa generation)
                "out" => {
                    let gen = v["gen"].as_u64().unwrap_or(u64::MAX) as u32;
                    let sink = inst.fx_sink.lock().unwrap().as_ref().filter(|(g, _)| *g == gen).map(|(_, s)| s.clone());
                    if let (Some(s), Some(pos), Some(d)) = (sink, v["pos"].as_i64(), v["d"].as_str()) {
                        if let Some(data) = uwl_vst3host::b64::decode_f32(d) {
                            if s.try_send(FxOut { gen, pos, data }).is_err() {
                                inst.fx_dropped.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
                "saved" | "error" | "rendered" | "live" => {
                    if v["fatal"] == true {
                        *inst.error.lock().unwrap() = v["error"].as_str().map(String::from);
                        if let Some(t) = ready_tx.take() {
                            let _ = t.send(Err(v["error"].as_str().unwrap_or("the plugin could not start").to_string()));
                        }
                    }
                    if let Some(n) = v["n"].as_u64() {
                        if let Some(w) = inst.waiters.lock().unwrap().remove(&n) {
                            let _ = w.send(v);
                        }
                    }
                }
                "closed" => *inst.status.lock().unwrap() = "closed".into(),
                // (20 nov., 2e relecture) les reglages abimes d'un plugin : il s'ouvre avec ses defauts, le journal le dit
                "notice" => crate::log(&format!("plugin host: {}", no_paths(v["notice"].as_str().unwrap_or("")))),
                _ => {}
            }
        }
        // la sortie est fermee : le processus est parti
        let code = inst.child.lock().unwrap().wait().ok().and_then(|s| s.code());
        {
            let mut st = inst.status.lock().unwrap();
            if *st != "closed" && *st != "removed" {
                *st = "crashed".into();
                crate::log(&format!("plugin host: {} stopped unexpectedly (exit {code:?})", inst.name));
            }
        }
        if let Some(t) = ready_tx.take() {
            let _ = t.send(Err(inst.error.lock().unwrap().clone().unwrap_or_else(|| "the plugin stopped while loading".into())));
        }
        inst.waiters.lock().unwrap().clear();
        changed();
    });
}

/// (20 nov., le delai de l'hote) le bloc du fil audio d'un instrument heberge : 128 trames (2,9 ms a 44,1 kHz). Une note
/// jouee au clavier attend le debut du bloc suivant : la moitie d'un bloc en moyenne. (L'enfant le prend par --block ; un
/// effet en direct a son propre bloc, child.rs LIVE_BLOCK.)
pub const INSTRUMENT_BLOCK: u32 = 128;
/// les frequences que le studio peut demander pour un instrument (celle de son contexte audio) : sans elle, 48 000 Hz, et
/// le compagnon reechantillonne le son pour le studio (lineairement : il perd des aigus, et ses paquets ne tombent plus
/// juste sur les blocs)
pub fn instrument_rate(r: Option<u64>) -> Option<u32> {
    r.and_then(|r| u32::try_from(r).ok()).filter(|r| [22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000].contains(r))
}

fn add(plugin: &str, state: Option<&str>, rate: Option<u32>) -> Result<Value, String> {
    let c = cfg()?;
    if plugin.len() != 32 || !plugin.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("not a plugin id".into());
    }
    ensure_scan(false);
    let (module, class) = lookup(&plugin.to_ascii_lowercase()).ok_or("this plugin is not in the scanned VST3 folders")?;
    // (19 nov., etape 3) un effet : rendu hors temps reel (render), sans Notes ni Return ni tag ni flux
    let effect = match kind_of(class["sub"].as_str().unwrap_or("")) {
        "instrument" => false,
        "effect" => true,
        _ => return Err("this plugin is neither an instrument nor an effect".into()),
    };
    if !effect && !c.link_bundle.join("Contents").is_dir() {
        return Err("the Uppwind Link plugins are not installed (companion setup › Uppwind Link plugins)".into());
    }
    // (20 nov., relecture) la place et le tag reserves sous le verrou, avant le lancement : des demandes simultanees ne
    // depassent jamais MAX_INSTANCES ; les instances parties (plantees, fermees) quittent la liste ici
    let (tag, _slot) = {
        let mut list = insts().lock().unwrap();
        list.retain(|i| !matches!(i.status.lock().unwrap().as_str(), "crashed" | "closed" | "removed"));
        let mut res = reserved().lock().unwrap();
        let live: Vec<&Arc<Inst>> = list.iter().filter(|i| matches!(i.status.lock().unwrap().as_str(), "starting" | "ready")).collect();
        // (20 nov., 2e relecture) un plugin qui se charge est compte par sa reservation, pas une seconde fois par la liste
        let ready = live.iter().filter(|i| i.status.lock().unwrap().as_str() == "ready").count();
        if ready + res.0 >= MAX_INSTANCES {
            return Err(format!("{MAX_INSTANCES} hosted plugins at most"));
        }
        let tag = if effect {
            0
        } else {
            (TAG_BOTTOM..=TAG_TOP).rev().find(|t| !live.iter().any(|i| i.tag == *t) && !res.1.contains(t)).ok_or("no free Link tag")?
        };
        res.0 += 1;
        if !effect {
            res.1.push(tag);
        }
        (tag, Slot(tag, effect))
    };
    let states = c.data.join("plugin-states");
    let mut cmd = Command::new(&c.exe);
    cmd.arg("--plug-host").arg("--module").arg(&module).arg("--cid").arg(plugin.to_ascii_lowercase()).arg("--states-dir").arg(&states);
    if effect {
        cmd.arg("--effect");
    } else {
        cmd.arg("--tag").arg(tag.to_string()).arg("--link-bundle").arg(&c.link_bundle).arg("--block").arg(INSTRUMENT_BLOCK.to_string());
        if let Some(r) = rate {
            cmd.arg("--rate").arg(r.to_string());
        }
    }
    // un etat : seulement un fichier de ce compagnon, nomme par son empreinte (sinon : les reglages par defaut)
    let mut state_note = None;
    if let Some(sha) = state {
        if uwl_vst3host::child::is_state_name(sha) && states.join(format!("{sha}.bin")).is_file() {
            cmd.arg("--state").arg(states.join(format!("{sha}.bin")));
        } else {
            state_note = Some("the saved settings of this plugin are not on this computer: it opens with its defaults");
        }
    }
    cmd.env("UWL_STATE_FILE", &c.state_file).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    hidden(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("cannot start the plugin host ({e})"))?;
    tie_to_companion(&child);
    let stdin = child.stdin.take().ok_or("no input")?;
    let (tx, wrx) = mpsc::sync_channel(WRITE_QUEUE);
    writer(stdin, wrx);
    let stdout = child.stdout.take().ok_or("no output")?;
    let pid = child.id();
    let name = class["name"].as_str().unwrap_or("Plugin").to_string();
    let inst = Arc::new(Inst {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        effect,
        cid: plugin.to_ascii_lowercase(),
        name: name.clone(),
        vendor: class["vendor"].as_str().unwrap_or("").to_string(),
        tag,
        pid,
        tx,
        fx_sink: Mutex::new(None),
        fx_dropped: AtomicU64::new(0),
        child: Mutex::new(child),
        status: Mutex::new("starting".into()),
        error: Mutex::new(None),
        editor: AtomicBool::new(false),
        view: Mutex::new(None),
        ready: Mutex::new(None),
        waiters: Mutex::new(HashMap::new()),
    });
    insts().lock().unwrap().push(inst.clone());
    changed();
    crate::log(&format!("plugin host: loading {name} (pid {pid}, Link tag {tag})"));
    let (tx, rx) = mpsc::channel();
    reader(inst.clone(), stdout, tx);
    match rx.recv_timeout(READY_TIMEOUT) {
        // (20 nov., relecture) desarme pendant le chargement : le plugin n'est pas garde
        Ok(Ok(_)) if !armed_now() => {
            remove_inst(&inst);
            Err("remote control was turned off while the plugin loaded".into())
        }
        Ok(Ok(_)) => {
            let mut item = item_json(&inst);
            if let Some(n) = state_note {
                item["notice"] = json!(n);
            }
            Ok(item)
        }
        Ok(Err(e)) => {
            remove_inst(&inst);
            crate::log(&format!("plugin host: {name} could not start: {e}"));
            Err(format!("{name} could not start: {}", no_paths(&e)))
        }
        Err(_) => {
            remove_inst(&inst);
            Err(format!("{name} took too long to load"))
        }
    }
}

fn remove_inst(i: &Arc<Inst>) {
    *i.status.lock().unwrap() = "removed".into();
    let _ = send_cmd(i, json!({ "cmd": "quit" }));
    let t0 = Instant::now();
    loop {
        if let Ok(Some(_)) = i.child.lock().unwrap().try_wait() {
            break;
        }
        if t0.elapsed() > Duration::from_secs(4) {
            let _ = i.child.lock().unwrap().kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    insts().lock().unwrap().retain(|x| x.id != i.id);
    changed();
}

fn save(id: u32, n: Value) -> Result<Value, String> {
    let i = find(id)?;
    let k = NEXT_N.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel();
    i.waiters.lock().unwrap().insert(k, tx);
    send_cmd(&i, json!({ "cmd": "save", "n": k }))?;
    let v = rx.recv_timeout(SAVE_TIMEOUT).map_err(|_| format!("{} did not give its settings in time", i.name))?;
    if v["event"] == "saved" {
        Ok(json!({ "type": "host", "action": "saved", "id": id, "n": n, "state": v["sha"], "bytes": v["bytes"] }))
    } else {
        Err(v["error"].as_str().unwrap_or("the settings could not be saved").to_string())
    }
}

/// Ce que le compagnon fait des plugins heberges quand il s'arrete : rien a faire (les enfants voient leur entree se
/// fermer et s'arretent seuls).
/// (19 nov.) Les fenetres des editeurs ouverts des plugins heberges, pour la liste du studio (session.rs) : seulement
/// celles des enfants que ce compagnon a lances lui-meme (leur handle vient d'eux), seulement un editeur ouvert. Le
/// titre « <plugin> (Uppwind Link) », la piste « #<id> » (le studio retrouve celle de sa liaison), host « host ».
pub fn windows() -> Vec<crate::plugins::PluginWindow> {
    // (20 nov., relecture) desarme : aucun editeur heberge dans la liste (ni filme, ni clique)
    if !armed_now() {
        return Vec::new();
    }
    let list = insts().lock().unwrap();
    list.iter()
        .filter(|i| *i.status.lock().unwrap() == "ready")
        .filter_map(|i| {
            let (w, h) = (*i.view.lock().unwrap())?;
            let hwnd = i.ready.lock().unwrap().as_ref().and_then(|r| r["hwnd"].as_u64())?;
            if hwnd == 0 || !is_window_of(hwnd, i.pid) {
                return None;
            }
            Some(crate::plugins::PluginWindow { id: hwnd, title: format!("{} (Uppwind Link)", i.name), plugin: i.name.clone(), track: format!("#{}", i.id), w, h, minimized: false, host: Some("host") })
        })
        .collect()
}

/// La fenetre `hwnd` existe-t-elle, et appartient-elle au processus `pid` (l'enfant) ? Un handle recycle par Windows
/// pour la fenetre d'un autre programme n'est jamais filme.
fn is_window_of(hwnd: u64, pid: u32) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsWindow};
    let h = HWND(hwnd as usize as *mut core::ffi::c_void);
    unsafe {
        if !IsWindow(Some(h)).as_bool() {
            return false;
        }
        let mut owner = 0u32;
        GetWindowThreadProcessId(h, Some(&mut owner));
        owner == pid
    }
}

/// (19 nov., etape 2) Le Return du plugin heberge `id` (connecte au compagnon par son enfant) → « plugin:<hex> ».
pub fn return_target(id: u32) -> Option<String> {
    let pid = {
        let list = insts().lock().unwrap();
        let i = list.iter().find(|i| i.id == id)?;
        if *i.status.lock().unwrap() != "ready" {
            return None;
        }
        i.pid
    };
    let hub = crate::plugin_link::hub()?;
    hub.list().into_iter().filter(|r| r.kind == uwl_proto::Kind::Return && r.pid == pid).last().map(|r| format!("plugin:{}", r.hex()))
}

/// (19 nov., etape 3) Les fichiers d'un rendu (l'entree ecrite par la session, la sortie ecrite par l'enfant), nommes
/// par le compagnon dans son dossier (jobs/), jamais par le studio.
pub fn job_paths(job: u32) -> Option<(PathBuf, PathBuf)> {
    let c = CFG.get()?;
    let d = c.data.join("jobs");
    std::fs::create_dir_all(&d).ok()?;
    Some((d.join(format!("job-{}-{job}.in", std::process::id())), d.join(format!("job-{}-{job}.out", std::process::id()))))
}

/// (20 nov., relecture) le numero d'un nouveau rendu, commun a toutes les connexions : leurs fichiers ne se melangent jamais.
pub fn next_job_id() -> u32 {
    static N: AtomicU32 = AtomicU32::new(1);
    N.fetch_add(1, Ordering::Relaxed).max(1)
}

/// (20 nov., relecture) la place que prennent les rendus de jobs/ (en cours, ou qu'un enfant ecrit encore).
pub fn jobs_bytes() -> u64 {
    let Some(c) = CFG.get() else { return 0 };
    std::fs::read_dir(c.data.join("jobs")).map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum()).unwrap_or(0)
}

/// Les fichiers de rendu laisses par un compagnon d'avant (arrete pendant un rendu) : effaces au demarrage.
pub fn sweep_jobs() {
    let Some(c) = CFG.get() else { return };
    if let Ok(rd) = std::fs::read_dir(c.data.join("jobs")) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("job-") && (n.ends_with(".in") || n.ends_with(".out")) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// (19 nov., etape 3) Le rendu d'un effet heberge (bloquant : appele dans un fil). → {frames, latency, tail}
#[allow(clippy::too_many_arguments)]
pub fn render(id: u32, input: &Path, output: &Path, frames: u64, rate: u32, bpm: f64, tail: f64, start: i64) -> Result<Value, String> {
    let i = find(id)?;
    if !i.effect {
        return Err("this hosted plugin is not an effect".into());
    }
    let k = NEXT_N.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel();
    i.waiters.lock().unwrap().insert(k, tx);
    send_cmd(&i, json!({ "cmd": "render", "n": k, "in": input, "out": output, "frames": frames, "rate": rate, "bpm": bpm, "tail": tail, "start": start }))?;
    // le temps reel du son, deux fois, et une minute de marge (un effet lourd, un disque lent)
    let wait = Duration::from_secs_f64(60.0 + 2.0 * (frames as f64 / rate.max(1) as f64) + tail);
    let v = rx.recv_timeout(wait).map_err(|_| format!("{} did not finish the render in time", i.name))?;
    if v["event"] == "rendered" {
        Ok(json!({ "frames": v["frames"], "latency": v["latency"], "tail": v["tail"] }))
    } else {
        Err(v["error"].as_str().unwrap_or("the render failed").to_string())
    }
}

/// (19 nov., etape 3b) Un effet en direct : `on` le fait traiter le son que la session lui envoie (fx_input) au fil de
/// l'eau, a la frequence `rate`, sous la generation `gen` ; son son revient dans `sink`. Bloquant (le plugin redemarre
/// son traitement) : appele dans un fil. → {latency, rate}
pub fn fx_live(id: u32, rate: u32, gen: u32, sink: SyncSender<FxOut>) -> Result<Value, String> {
    let i = find(id)?;
    if !i.effect {
        return Err("this hosted plugin is not an effect".into());
    }
    if *i.status.lock().unwrap() != "ready" {
        return Err(format!("{} is not ready", i.name));
    }
    *i.fx_sink.lock().unwrap() = Some((gen, sink));
    let k = NEXT_N.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel();
    i.waiters.lock().unwrap().insert(k, tx);
    send_cmd(&i, json!({ "cmd": "live", "n": k, "on": true, "rate": rate, "gen": gen }))?;
    let v = rx.recv_timeout(LIVE_TIMEOUT).map_err(|_| format!("{} did not start in time", i.name))?;
    if v["event"] == "live" && v["on"] == true {
        crate::log(&format!("plugin host: {} live at {rate} Hz (latency {})", i.name, v["latency"]));
        Ok(json!({ "latency": v["latency"], "rate": v["rate"] }))
    } else {
        Err(v["error"].as_str().unwrap_or("the effect could not go live").to_string())
    }
}

/// (etape 3b) Plus de direct pour l'effet `id`, seulement si `gen` est encore la sienne (un direct rouvert depuis reste).
pub fn fx_stop(id: u32, gen: u32) {
    let Ok(i) = find(id) else { return };
    {
        let mut s = i.fx_sink.lock().unwrap();
        if !matches!(&*s, Some((g, _)) if *g == gen) {
            return;
        }
        *s = None;
    }
    let _ = i.tx.try_send(json!({ "cmd": "live", "on": false }).to_string());
    let d = i.fx_dropped.swap(0, Ordering::Relaxed);
    crate::log(&format!("plugin host: {} no longer live ({d} packets dropped on the way back)", i.name));
}

/// (etape 3b) Un paquet du studio pour l'effet en direct `id` (sa trame `pos`, stereo entrelace) : vers l'enfant, sans
/// jamais attendre. Faux : jete (l'enfant est en retard, ou parti).
pub fn fx_input(id: u32, gen: u32, pos: u64, samples: &[f32]) -> bool {
    let Ok(i) = find(id) else { return false };
    // la forme exacte que le fil de lecture de l'enfant reconnait (« {"cmd":"in" » d'abord)
    let line = format!("{{\"cmd\":\"in\",\"gen\":{gen},\"pos\":{pos},\"d\":\"{}\"}}", uwl_vst3host::b64::encode_f32(samples));
    if i.tx.try_send(line).is_ok() {
        true
    } else {
        i.fx_dropped.fetch_add(1, Ordering::Relaxed);
        false
    }
}

/// (19 nov., la mesure en direct) L'ancre du morceau d'un effet en direct, vers l'enfant : la forme exacte que son fil de
/// lecture reconnait (« {"cmd":"song" » d'abord), pour qu'elle passe devant le son qu'elle date, jamais derriere
/// l'interface de l'enfant.
fn song_anchor(i: &Inst, from: i64, offset: i64, playing: bool) -> Result<(), String> {
    // (20 nov., relecture) un intervalle, pas abs() : i64::MIN.abs() deborde sans erreur en version publiee
    if !(-FRAME_MAX..=FRAME_MAX).contains(&from) || !(-FRAME_MAX..=FRAME_MAX).contains(&offset) {
        return Err("from and offset are frames".into());
    }
    let line = format!("{{\"cmd\":\"song\",\"from\":{from},\"offset\":{offset},\"playing\":{playing}}}");
    i.tx.try_send(line).map_err(|_| format!("{} is busy", i.name))
}

/// Une trame, au plus (2^53 : un f64 du navigateur la porte exactement).
pub const FRAME_MAX: i64 = 1 << 53;

fn list_json() -> Value {
    let list = insts().lock().unwrap();
    json!({ "type": "host", "action": "list", "items": list.iter().map(|i| item_json(i)).collect::<Vec<_>>() })
}

fn status_json() -> Value {
    let s = scan_state().lock().unwrap();
    json!({ "type": "host", "action": "status", "available": available(), "scanning": s.running, "done": s.done, "total": s.total,
            "count": insts().lock().unwrap().len() })
}

/// Une demande du studio (deja armee, verifiee par session.rs). La reponse part par `reply` (un fil a part : charger un
/// plugin prend des secondes, la connexion ne s'arrete jamais).
pub fn request(v: Value, reply: Sender<String>) {
    std::thread::spawn(move || {
        let action = v["action"].as_str().unwrap_or("").to_string();
        let r: Result<Value, String> = (|| {
            cfg()?;
            match action.as_str() {
                "status" => {
                    ensure_scan(false);
                    Ok(status_json())
                }
                "scan" => {
                    ensure_scan(true);
                    Ok(status_json())
                }
                "plugins" => Ok(plugins_json()),
                "list" => Ok(list_json()),
                "add" => {
                    let item = add(v["plugin"].as_str().unwrap_or(""), v["state"].as_str(), instrument_rate(v["rate"].as_u64()))?;
                    Ok(json!({ "type": "host", "action": "added", "item": item, "n": v["n"] }))
                }
                "remove" => {
                    let id = v["id"].as_u64().ok_or("id")? as u32;
                    let i = find(id)?;
                    remove_inst(&i);
                    Ok(json!({ "type": "host", "action": "removed", "id": id }))
                }
                "save" => save(v["id"].as_u64().ok_or("id")? as u32, v["n"].clone()),
                "editor" => {
                    let id = v["id"].as_u64().ok_or("id")? as u32;
                    let i = find(id)?;
                    let show = v["show"].as_bool().unwrap_or(false);
                    send_cmd(&i, json!({ "cmd": "editor", "show": show }))?;
                    Ok(json!({ "type": "host", "action": "editor", "id": id, "shown": show }))
                }
                "mix" => {
                    let id = v["id"].as_u64().ok_or("id")? as u32;
                    let i = find(id)?;
                    let vol = v["vol"].as_f64().filter(|x| x.is_finite()).unwrap_or(1.0).clamp(0.0, 2.0);
                    let pan = v["pan"].as_f64().filter(|x| x.is_finite()).unwrap_or(0.0).clamp(-1.0, 1.0);
                    send_cmd(&i, json!({ "cmd": "mix", "vol": vol, "pan": pan, "mute": v["mute"].as_bool().unwrap_or(false) }))?;
                    return Ok(Value::Null);
                }
                "view" => {
                    let id = v["id"].as_u64().ok_or("id")? as u32;
                    let i = find(id)?;
                    let open = v["open"].as_bool().unwrap_or(false);
                    send_cmd(&i, json!({ "cmd": "view", "open": open }))?;
                    Ok(json!({ "type": "host", "action": "view", "id": id, "open": open }))
                }
                "tempo" => {
                    let bpm = v["bpm"].as_f64().filter(|b| (20.0..=999.0).contains(b)).ok_or("bpm is 20..999")?;
                    // (19 nov., la mesure en direct) avec `id`, `from` et `offset` : l'ancre du morceau de CET effet en direct
                    // (chaque worklet du studio a son horloge) — rien en retour, elle part a chaque depart et chaque saut
                    if let (Some(id), Some(from), Some(offset)) = (v["id"].as_u64(), v["from"].as_i64(), v["offset"].as_i64()) {
                        let i = find(id as u32)?;
                        let _ = send_cmd(&i, json!({ "cmd": "tempo", "bpm": bpm }));
                        song_anchor(&i, from, offset, v["playing"].as_bool().unwrap_or(true))?;
                        return Ok(Value::Null);
                    }
                    // (20 nov., relecture) la liste copiee, puis les envois hors du verrou : un plugin fige ne bloque pas les autres
                    let list: Vec<Arc<Inst>> = insts().lock().unwrap().clone();
                    for i in &list {
                        let _ = send_cmd(i, json!({ "cmd": "tempo", "bpm": bpm, "playing": v["playing"] }));
                    }
                    Ok(json!({ "type": "host", "action": "tempo", "bpm": bpm }))
                }
                _ => Err("unknown host action".into()),
            }
        })();
        let msg = match r {
            // (mix) rien a repondre
            Ok(Value::Null) => return,
            Ok(m) => m,
            Err(e) => json!({ "type": "host", "action": action, "error": e, "n": v["n"], "id": v["id"], "plugin": v["plugin"] }),
        };
        let _ = reply.send(msg.to_string());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modules_of_a_folder() {
        let d = std::env::temp_dir().join(format!("uw-host-scan-p{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("Vendor").join("Synth.vst3").join("Contents")).unwrap();
        std::fs::write(d.join("Old.vst3"), b"x").unwrap();
        std::fs::create_dir_all(d.join("Uppwind Link.vst3")).unwrap();
        std::fs::write(d.join("readme.txt"), b"x").unwrap();
        let mut v = Vec::new();
        find_modules(&d, 6, &mut v);
        let names: Vec<String> = v.iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, vec!["Old.vst3", "Synth.vst3"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_of("Instrument|Synth"), "instrument");
        assert_eq!(kind_of("Fx|Reverb"), "effect");
        assert_eq!(kind_of(""), "other");
    }

    #[test]
    fn the_studio_rate_of_an_instrument() {
        assert_eq!(instrument_rate(Some(44100)), Some(44100));
        assert_eq!(instrument_rate(Some(48000)), Some(48000));
        assert_eq!(instrument_rate(Some(12345)), None, "only the usual sound card rates");
        assert_eq!(instrument_rate(Some(u64::MAX)), None);
        assert_eq!(instrument_rate(None), None, "an older studio: the child keeps 48 kHz");
    }

    // (20 nov., relecture de securite) les messages pour le studio ne montrent aucun chemin (point 9)
    #[test]
    fn errors_without_paths() {
        assert_eq!(no_paths(r"cannot load C:\Users\someone\AppData\Local\Programs\Common\VST3\X.vst3: bad module"), "cannot load (path): bad module");
        assert_eq!(no_paths(r"open 'D:/VST3/My Synth.vst3' failed"), "open '(path)' failed");
        assert_eq!(no_paths(r"\\nas\share\a.vst3"), "(path)");
        assert_eq!(no_paths("no plugin with this id"), "no plugin with this id");
        assert_eq!(no_paths("ratio 1:2 ok"), "ratio 1:2 ok");
    }

    // (point 3) un numero de rendu jamais donne deux fois, meme par des connexions simultanees
    #[test]
    fn job_ids_are_unique() {
        let hs: Vec<_> = (0..8).map(|_| std::thread::spawn(|| (0..500).map(|_| next_job_id()).collect::<Vec<_>>())).collect();
        let mut all: Vec<u32> = hs.into_iter().flat_map(|h| h.join().unwrap()).collect();
        let n = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), n);
        assert!(!all.contains(&0));
    }

    // (point 4) la plus petite valeur ne passe plus la borne
    #[test]
    fn position_bounds_take_the_extremes() {
        let ok = |s: i64| (-(1i64 << 53)..=(1i64 << 53)).contains(&s);
        assert!(!ok(i64::MIN));
        assert!(!ok(i64::MAX));
        assert!(ok(-(1i64 << 53)) && ok(1i64 << 53) && ok(0));
    }
}
