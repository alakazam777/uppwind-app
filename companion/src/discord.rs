// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (11 nov. 2026) « Show listening on Discord » : ce que la personne ecoute dans Music (le site) s'affiche sur son
//! profil Discord. Un site ne peut pas le faire seul (les permissions `activities.write` et `rpc.activities.write` ne
//! sont pas ouvertes aux applications) ; une application locale, si : par le canal local du client Discord de bureau
//! (IPC : un tube nomme sous Windows, un socket Unix ailleurs), sans authentification, avec le seul identifiant de
//! l'application Discord (« Rich Presence Without Authentication », docs.discord.com, lue le 11 nov. 2026).
//!
//! Ce que ce module fait, et rien d'autre :
//!  - il n'ecrit que vers ce canal LOCAL (jamais vers le reseau : le compagnon n'a toujours aucun client reseau) ;
//!  - il n'envoie qu'une commande, SET_ACTIVITY, de type « Listening », faite ICI a partir de champs nommes (titre,
//!    artiste, album, pochette, lien, duree, position) : la page ne choisit ni la commande ni sa forme ;
//!  - les textes sont nettoyes (pas de caracteres de controle) et coupes a 128 caracteres ; les adresses (pochette,
//!    lien) doivent etre en https et sur un hote de la liste (`IMAGE_HOSTS`, `LINK_HOSTS`), sinon elles sont omises ;
//!  - rien ne part tant que la personne ne l'a pas permis DANS le compagnon (`Presence::set_allowed` : la fenetre
//!    d'etat, la console) ; ce choix est garde d'un lancement a l'autre (le fichier d'etat, « discordPresence ») ;
//!  - la presence s'efface quand la page le demande, quand sa connexion se ferme, quand la personne retire la
//!    permission, et (par Discord lui-meme) quand le compagnon s'arrete ;
//!  - le journal ne dit jamais ce qui est ecoute.
//!
//! Un fil a part fait les echanges (une ouverture de tube peut attendre) : les connexions du studio ne font que lui
//! deposer la derniere demande. Au plus une commande toutes les `MIN_GAP` (la plus recente gagne).

use serde::Deserialize;
use serde_json::{json, Value};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// L'application Discord « Uppwind » (la meme que « Connect with Discord » du site) : un identifiant public.
pub const DEFAULT_APP_ID: &str = "1562062386075017236";
/// Les textes d'une activite Discord : 2 a 128 caracteres.
pub const MAX_TEXT: usize = 128;
pub const MAX_URL: usize = 256;
/// Au plus une commande toutes les 4 s (la plus recente gagne).
pub const MIN_GAP: Duration = Duration::from_secs(4);
/// Discord repond a chaque commande ; au-dela, la connexion est tenue pour morte.
pub const REPLY_WAIT: Duration = Duration::from_secs(5);
/// Discord absent : nouvel essai tant qu'une presence attend.
pub const RETRY: Duration = Duration::from_secs(15);
const MAX_FRAME: usize = 64 * 1024;

/// Les hotes d'ou une pochette peut venir (Discord va la chercher lui-meme) : le site, sa copie R2, SoundCloud.
pub const IMAGE_HOSTS: &[&str] = &["beta.uppwind.com", "cdn.uppwind.com", "uppwind.com", "www.uppwind.com", ".sndcdn.com"];
/// Les hotes vers lesquels le titre peut renvoyer.
pub const LINK_HOSTS: &[&str] = &["beta.uppwind.com", "uppwind.com", "www.uppwind.com", "soundcloud.com"];

/// Ce que la page envoie (« presence », protocol.rs) : des champs nommes, jamais une activite Discord toute faite.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct RawTrack {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub cover: Option<String>,
    pub url: Option<String>,
    /// secondes
    pub duration: Option<f64>,
    pub position: Option<f64>,
}

/// Un titre verifie, pret a montrer.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub cover: Option<String>,
    pub url: Option<String>,
    /// ms depuis 1970 : le debut du titre (maintenant − la position)
    pub start_ms: u64,
    /// sa fin (None : une radio, pas de duree)
    pub end_ms: Option<u64>,
}

/// Les caracteres qui changent le sens d'ecriture (U+202A-202E, U+2066-2069, U+200E, U+200F) ou ne se voient pas
/// (U+200B-200D, U+FEFF) : retires, un titre ne doit pas retourner le texte qui le suit sur le profil.
fn invisible(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

/// Un texte montre sur Discord : sans caractere de controle ni de mise en forme, sans blancs autour, et 128 unites
/// UTF-16 au plus (Discord compte comme JavaScript : un emoji en vaut deux ; un caractere n'est jamais coupe en deux).
pub fn clean(s: &str) -> String {
    let t: String = s.chars().filter(|c| !invisible(*c)).map(|c| if c.is_control() { ' ' } else { c }).collect();
    let mut out = String::new();
    let mut units = 0;
    for c in t.split_whitespace().collect::<Vec<_>>().join(" ").chars() {
        units += c.len_utf16();
        if units > MAX_TEXT {
            break;
        }
        out.push(c);
    }
    out.trim_end().to_string()
}

fn field(s: Option<&str>) -> Option<String> {
    let t = clean(s?);
    (t.chars().count() >= 2).then_some(t)
}

/// Une adresse https sur un hote permis (`hosts` : un nom exact, ou « .suffixe ») ; sans identifiants ni port.
pub fn https_url(s: &str, hosts: &[&str]) -> Option<String> {
    let s = s.trim();
    if s.len() > MAX_URL || !s.bytes().all(|b| (0x21..=0x7e).contains(&b) && !matches!(b, b'"' | b'<' | b'>' | b'\\' | b'`')) {
        return None;
    }
    let rest = s.strip_prefix("https://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = rest[..end].to_ascii_lowercase();
    if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') {
        return None; // « @ », « : » (un port, des identifiants) : refuses
    }
    let ok = hosts.iter().any(|h| match h.strip_prefix('.') {
        Some(suffix) => host.len() > h.len() && host.ends_with(h) && !suffix.is_empty(),
        None => host == *h,
    });
    ok.then(|| s.to_string())
}

impl Track {
    /// `now_ms` : l'instant ou la page l'a dit (ms depuis 1970).
    pub fn from_raw(r: &RawTrack, now_ms: u64) -> Result<Track, String> {
        let title = clean(&r.title);
        if title.is_empty() {
            return Err("presence needs a title".into());
        }
        // Discord refuse un texte d'un seul caractere
        let title = if title.chars().count() < 2 { format!("« {title} »") } else { title };
        let secs = |v: Option<f64>| v.filter(|x| x.is_finite() && *x >= 0.0 && *x <= 86_400.0);
        let pos = secs(r.position).unwrap_or(0.0);
        let start_ms = now_ms.saturating_sub((pos * 1000.0) as u64);
        let end_ms = secs(r.duration).filter(|d| *d >= 1.0 && *d >= pos).map(|d| start_ms + (d * 1000.0) as u64);
        Ok(Track {
            title,
            artist: field(r.artist.as_deref()),
            album: field(r.album.as_deref()),
            cover: r.cover.as_deref().and_then(|u| https_url(u, IMAGE_HOSTS)),
            url: r.url.as_deref().and_then(|u| https_url(u, LINK_HOSTS)),
            start_ms,
            end_ms,
        })
    }

    /// Le nom affiche a la place de celui de l'application (11 nov. : l'artiste et le titre, au lieu de « Uppwind ») :
    /// « Uppermost - Eternal Love », ou « Eternal Love » sans artiste. (12 nov.) Sans la note de musique devant.
    pub fn display_name(&self) -> String {
        match &self.artist {
            Some(a) => clean(&format!("{a} - {}", self.title)),
            None => clean(&self.title),
        }
    }

    /// L'activite Discord : « Listening », le titre, l'artiste, la pochette, le temps. `name` remplace le nom de
    /// l'application sur la premiere ligne, et status_display_type 0 (Name) le montre aussi sous le pseudo, dans la
    /// liste des membres (docs.discord.com, Activity Object, lu le 11 nov. 2026).
    pub fn activity(&self) -> Value {
        let mut a = json!({ "type": 2, "name": self.display_name(), "status_display_type": 0, "details": self.title, "timestamps": { "start": self.start_ms } });
        if let Some(e) = self.end_ms {
            a["timestamps"]["end"] = json!(e);
        }
        if let Some(s) = &self.artist {
            a["state"] = json!(s);
        }
        if let Some(u) = &self.url {
            a["details_url"] = json!(u);
        }
        if let Some(c) = &self.cover {
            a["assets"] = json!({ "large_image": c });
            if let Some(al) = &self.album {
                a["assets"]["large_text"] = json!(al);
            }
        }
        a
    }
}

// ── Le canal local de Discord ────────────────────────────────────────────────────────────────────────────────────

/// Le canal : ecrire tout, lire exactement tant d'octets avant une echeance.
pub trait Pipe: Send {
    fn write_all(&mut self, b: &[u8]) -> io::Result<()>;
    fn read_exact_by(&mut self, buf: &mut [u8], until: Instant) -> io::Result<()>;
}

pub type Opener = Box<dyn Fn() -> io::Result<Box<dyn Pipe>> + Send + Sync>;

/// Une trame : le code (u32 petit-boutiste), la longueur (u32), le JSON.
pub fn frame(op: u32, v: &Value) -> Vec<u8> {
    let body = v.to_string().into_bytes();
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn read_frame(p: &mut dyn Pipe, until: Instant) -> io::Result<(u32, Value)> {
    let mut head = [0u8; 8];
    p.read_exact_by(&mut head, until)?;
    let op = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
    let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too long"));
    }
    let mut body = vec![0u8; len];
    p.read_exact_by(&mut body, until)?;
    let v = serde_json::from_slice(&body).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not JSON"))?;
    Ok((op, v))
}

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;

fn refused(v: &Value) -> io::Error {
    let m: String = v["message"].as_str().or_else(|| v["data"]["message"].as_str()).unwrap_or("refused").chars().take(120).collect();
    io::Error::other(m)
}

/// La poignee de main : la version et l'identifiant de l'application → READY.
fn handshake(p: &mut dyn Pipe, app_id: &str) -> io::Result<()> {
    p.write_all(&frame(OP_HANDSHAKE, &json!({ "v": 1, "client_id": app_id })))?;
    let until = Instant::now() + REPLY_WAIT;
    loop {
        let (op, v) = read_frame(p, until)?;
        match op {
            OP_FRAME if v["evt"] == "READY" => return Ok(()),
            OP_FRAME if v["evt"] == "ERROR" => return Err(refused(&v)),
            OP_CLOSE => return Err(refused(&v)),
            OP_PING => p.write_all(&frame(OP_PONG, &v))?,
            _ => {}
        }
    }
}

/// SET_ACTIVITY (None : efface). Ok(Err(texte)) : Discord a refuse CETTE activite (la connexion reste bonne).
fn set_activity(p: &mut dyn Pipe, activity: Option<&Value>, nonce: u64) -> io::Result<Result<(), String>> {
    let nonce = format!("uwl-{nonce}");
    let cmd = json!({ "cmd": "SET_ACTIVITY", "nonce": nonce, "args": { "pid": std::process::id(), "activity": activity } });
    p.write_all(&frame(OP_FRAME, &cmd))?;
    let until = Instant::now() + REPLY_WAIT;
    loop {
        let (op, v) = read_frame(p, until)?;
        match op {
            OP_FRAME if v["nonce"] == nonce.as_str() => {
                return Ok(if v["evt"] == "ERROR" { Err(refused(&v).to_string()) } else { Ok(()) });
            }
            OP_CLOSE => return Err(refused(&v)),
            OP_PING => p.write_all(&frame(OP_PONG, &v))?,
            _ => {}
        }
    }
}

#[cfg(windows)]
mod sys {
    use super::Pipe;
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read, Write};
    use std::os::windows::io::AsRawHandle;
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Pipes::PeekNamedPipe;

    struct WinPipe(File);

    impl WinPipe {
        /// Les octets deja la (un tube ouvert sans attente partagee : une lecture bloquerait sans fin).
        fn available(&self) -> io::Result<usize> {
            let mut n = 0u32;
            // SAFETY: le descripteur est celui du fichier ouvert, vivant tant que self ; aucun tampon n'est donne.
            unsafe { PeekNamedPipe(HANDLE(self.0.as_raw_handle() as _), None, 0, None, Some(&mut n), None) }
                .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e.message()))?;
            Ok(n as usize)
        }
    }

    impl Pipe for WinPipe {
        fn write_all(&mut self, b: &[u8]) -> io::Result<()> {
            self.0.write_all(b)?;
            self.0.flush()
        }
        fn read_exact_by(&mut self, buf: &mut [u8], until: Instant) -> io::Result<()> {
            let mut got = 0;
            while got < buf.len() {
                let n = self.available()?.min(buf.len() - got);
                if n == 0 {
                    if Instant::now() >= until {
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "Discord did not answer"));
                    }
                    std::thread::sleep(Duration::from_millis(15));
                    continue;
                }
                self.0.read_exact(&mut buf[got..got + n])?;
                got += n;
            }
            Ok(())
        }
    }

    /// Le client Discord de bureau : discord-ipc-0 … 9 (le premier qui s'ouvre).
    pub fn open() -> io::Result<Box<dyn Pipe>> {
        for n in 0..10 {
            let path = format!(r"\\.\pipe\discord-ipc-{n}");
            if let Ok(f) = OpenOptions::new().read(true).write(true).open(&path) {
                return Ok(Box::new(WinPipe(f)));
            }
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "the Discord desktop app is not running"))
    }
}

#[cfg(unix)]
mod sys {
    use super::Pipe;
    use std::io::{self, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    struct UnixPipe(UnixStream);

    impl Pipe for UnixPipe {
        fn write_all(&mut self, b: &[u8]) -> io::Result<()> {
            self.0.write_all(b)
        }
        fn read_exact_by(&mut self, buf: &mut [u8], until: Instant) -> io::Result<()> {
            let mut got = 0;
            while got < buf.len() {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "Discord did not answer"));
                }
                self.0.set_read_timeout(Some(left.max(Duration::from_millis(1))))?;
                match self.0.read(&mut buf[got..]) {
                    Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Discord closed")),
                    Ok(n) => got += n,
                    Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        }
    }

    /// L'ordre de Discord : XDG_RUNTIME_DIR, TMPDIR, TMP, TEMP, puis /tmp ; discord-ipc-0 … 9. (11 nov., relecture L1)
    /// Sur un Mac, jamais /tmp, commun a tous les comptes : un autre compte pourrait y poser un faux Discord. A sa
    /// place, le dossier temporaire du compte (confstr DARWIN_USER_TEMP_DIR, ce que vaut TMPDIR), meme sans TMPDIR
    /// (un lancement par launchd) : c'est la que Discord pose son socket (mesure le 11 nov. : `$TMPDIR/discord-ipc-0`,
    /// rien dans /tmp).
    pub fn socket_dirs(env: impl Fn(&str) -> Option<std::ffi::OsString>, user_tmp: Option<std::path::PathBuf>) -> Vec<std::path::PathBuf> {
        let mut dirs: Vec<std::path::PathBuf> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"].iter().filter_map(|k| env(k)).filter(|v| !v.is_empty()).map(Into::into).collect();
        if cfg!(target_os = "macos") {
            dirs.extend(user_tmp);
        } else {
            dirs.push("/tmp".into());
        }
        dirs
    }

    /// macOS : le dossier temporaire de ce compte (/var/folders/…/T/), par confstr ; None ailleurs.
    pub fn user_temp_dir() -> Option<std::path::PathBuf> {
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::ffi::OsStringExt;
            extern "C" {
                fn confstr(name: i32, buf: *mut std::os::raw::c_char, len: usize) -> usize;
            }
            // <unistd.h> : _CS_DARWIN_USER_TEMP_DIR
            const CS_DARWIN_USER_TEMP_DIR: i32 = 65537;
            let mut buf = vec![0u8; 1024];
            // SAFETY : un tampon a nous, et sa taille
            let n = unsafe { confstr(CS_DARWIN_USER_TEMP_DIR, buf.as_mut_ptr() as *mut _, buf.len()) };
            if n <= 1 || n > buf.len() {
                return None;
            }
            buf.truncate(n - 1); // n compte le NUL
            Some(std::path::PathBuf::from(std::ffi::OsString::from_vec(buf)))
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    pub fn open() -> io::Result<Box<dyn Pipe>> {
        for d in socket_dirs(|k| std::env::var_os(k), user_temp_dir()) {
            for n in 0..10 {
                if let Ok(s) = UnixStream::connect(d.join(format!("discord-ipc-{n}"))) {
                    return Ok(Box::new(UnixPipe(s)));
                }
            }
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "the Discord desktop app is not running"))
    }
}

// ── La presence, vue du reste du compagnon ───────────────────────────────────────────────────────────────────────

/// Discord repond-il ? (ce que la page et la fenetre d'etat disent)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// rien n'a encore ete envoye
    Unknown,
    Ok,
    /// le client Discord de bureau ne tourne pas (ou ne repond pas)
    Absent,
}

impl Link {
    pub fn name(self) -> &'static str {
        match self {
            Link::Unknown => "unknown",
            Link::Ok => "ok",
            Link::Absent => "absent",
        }
    }
}

enum Cmd {
    Set(Box<Track>),
    Clear,
}

struct Inner {
    tx: Option<mpsc::Sender<Cmd>>,
    link: Link,
    /// une presence est affichee
    shown: bool,
    /// la connexion du studio qui a pose la presence (0 : personne)
    owner: u64,
}

struct Core {
    allowed: AtomicBool,
    generation: AtomicU64,
    inner: Mutex<Inner>,
    opener: Opener,
    app_id: String,
    gap: Duration,
    retry: Duration,
}

impl Core {
    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn note(&self, link: Link, shown: bool) {
        let mut i = self.inner();
        if i.link != link || i.shown != shown {
            i.link = link;
            i.shown = shown;
            self.generation.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// L'etat, pour « hello », « presence » et la fenetre d'etat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    pub allowed: bool,
    pub link: Link,
    pub shown: bool,
}

impl Snapshot {
    pub fn json(&self) -> Value {
        json!({ "allowed": self.allowed, "discord": self.link.name(), "shown": self.shown })
    }
}

pub struct Presence {
    core: Arc<Core>,
}

static OWNERS: AtomicU64 = AtomicU64::new(0);

/// Un numero pour une connexion du studio (qui a pose la presence l'efface en partant).
pub fn new_owner() -> u64 {
    OWNERS.fetch_add(1, Ordering::SeqCst) + 1
}

impl Presence {
    pub fn new(allowed: bool) -> Presence {
        Presence::with(allowed, Box::new(sys::open), MIN_GAP, RETRY)
    }

    pub fn with(allowed: bool, opener: Opener, gap: Duration, retry: Duration) -> Presence {
        Presence {
            core: Arc::new(Core {
                allowed: AtomicBool::new(allowed),
                generation: AtomicU64::new(0),
                inner: Mutex::new(Inner { tx: None, link: Link::Unknown, shown: false, owner: 0 }),
                opener,
                app_id: DEFAULT_APP_ID.into(),
                gap,
                retry,
            }),
        }
    }

    /// La personne l'a-t-elle permis, dans le compagnon ?
    pub fn allowed(&self) -> bool {
        self.core.allowed.load(Ordering::SeqCst)
    }

    /// Permet ou retire (la fenetre d'etat, la console ; jamais le studio). Retire : la presence s'efface. → a change
    pub fn set_allowed(&self, on: bool, by: &str) -> bool {
        if self.core.allowed.swap(on, Ordering::SeqCst) == on {
            return false;
        }
        self.core.generation.fetch_add(1, Ordering::SeqCst);
        crate::log(&format!("discord presence: {} ({by})", if on { "ALLOWED: Music can show on Discord what you listen to" } else { "off: nothing is shown on Discord" }));
        if !on {
            self.clear(0);
        }
        true
    }

    /// Avance a chaque changement (la permission, Discord la ou non, affichee ou non) : les connexions le redisent.
    pub fn generation(&self) -> u64 {
        self.core.generation.load(Ordering::SeqCst)
    }

    pub fn snapshot(&self) -> Snapshot {
        let i = self.core.inner();
        Snapshot { allowed: self.allowed(), link: i.link, shown: i.shown }
    }

    fn post(&self, i: &mut Inner, cmd: Cmd) {
        let cmd = match &i.tx {
            Some(tx) => match tx.send(cmd) {
                Ok(()) => return,
                Err(e) => e.0, // le fil est mort : un autre
            },
            None => cmd,
        };
        if matches!(cmd, Cmd::Clear) && !i.shown {
            return; // rien a effacer, pas de fil pour cela
        }
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(cmd);
        i.tx = Some(tx);
        let core = self.core.clone();
        std::thread::spawn(move || work(core, rx));
    }

    /// Montre ce titre (la connexion `owner` du studio). Err : pas permis dans le compagnon.
    pub fn show(&self, owner: u64, track: Track) -> Result<(), &'static str> {
        if !self.allowed() {
            return Err(REFUSAL);
        }
        let mut i = self.core.inner();
        i.owner = owner;
        self.post(&mut i, Cmd::Set(Box::new(track)));
        Ok(())
    }

    /// Efface la presence, si c'est `owner` qui l'a posee (0 : quoi qu'il en soit).
    pub fn clear(&self, owner: u64) {
        let mut i = self.core.inner();
        if owner != 0 && i.owner != owner {
            return;
        }
        i.owner = 0;
        self.post(&mut i, Cmd::Clear);
    }
}

pub const CODE: &str = "disabled";
#[cfg(not(target_os = "macos"))]
pub const REFUSAL: &str = "Discord presence is off in Uppwind Link: turn it on there (its window, or « discord on » in its console)";
/// (11 nov., 0.4.0) Sur un Mac, ni fenetre d'etat ni icone (comme arming::REFUSAL) : le Terminal ou il tourne.
#[cfg(target_os = "macos")]
pub const REFUSAL: &str = "Discord presence is off in Uppwind Link: type « discord on » in the Terminal where it runs";

fn work(core: Arc<Core>, rx: mpsc::Receiver<Cmd>) {
    let mut pipe: Option<Box<dyn Pipe>> = None;
    let mut want: Option<Track> = None;
    let mut dirty = false;
    let mut not_before = Instant::now();
    let mut nonce = 0u64;
    loop {
        let got = if dirty {
            match rx.recv_timeout(not_before.saturating_duration_since(Instant::now())) {
                Ok(c) => Some(c),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match rx.recv() {
                Ok(c) => Some(c),
                Err(_) => return,
            }
        };
        // la plus recente gagne
        for c in got.into_iter().chain(rx.try_iter()) {
            want = match c {
                Cmd::Set(t) => Some(*t),
                Cmd::Clear => None,
            };
            dirty = true;
        }
        // (relecture) la permission a pu etre retiree entre show() et ici : rien ne part, et l'effacement n'attend pas
        if want.is_some() && !core.allowed.load(Ordering::SeqCst) {
            want = None;
            dirty = true;
            not_before = Instant::now();
        }
        if !dirty || Instant::now() < not_before {
            continue;
        }
        if want.is_none() && pipe.is_none() {
            // rien d'affiche chez Discord (ou il est parti, et la presence avec lui)
            dirty = false;
            let link = core.inner().link;
            core.note(link, false);
            continue;
        }
        if pipe.is_none() {
            match (core.opener)().and_then(|mut p| handshake(p.as_mut(), &core.app_id).map(|()| p)) {
                Ok(p) => pipe = Some(p),
                Err(e) => {
                    if core.inner().link != Link::Absent {
                        crate::log(&format!("discord presence: Discord is not reachable ({e}); trying again while Music plays"));
                    }
                    core.note(Link::Absent, false);
                    not_before = Instant::now() + core.retry;
                    continue;
                }
            }
        }
        nonce += 1;
        let activity = want.as_ref().map(Track::activity);
        match set_activity(pipe.as_mut().expect("pipe").as_mut(), activity.as_ref(), nonce) {
            Ok(r) => {
                dirty = false;
                not_before = Instant::now() + core.gap;
                let shown = want.is_some() && r.is_ok();
                if let Err(e) = r {
                    crate::log(&format!("discord presence: Discord refused it ({e})"));
                }
                let was = core.inner().shown;
                if was != shown {
                    crate::log(if shown { "discord presence: shown" } else { "discord presence: cleared" });
                }
                core.note(Link::Ok, shown);
                if want.is_none() {
                    pipe = None; // rien a montrer : le canal se ferme
                }
            }
            Err(e) => {
                crate::log(&format!("discord presence: Discord stopped answering ({e})"));
                pipe = None;
                core.note(Link::Absent, false);
                not_before = Instant::now() + core.retry;
            }
        }
    }
}

#[cfg(test)]
pub mod fake {
    //! Un faux Discord, en memoire : il repond a la poignee de main et aux commandes, et garde ce qu'il a recu.
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    pub struct Seen {
        /// les activites recues (None : effacee), dans l'ordre
        pub activities: Vec<Option<Value>>,
        pub handshakes: Vec<Value>,
        /// le canal est ferme (le compagnon l'a lache)
        pub closed: usize,
        /// Discord tourne-t-il ?
        pub up: bool,
        /// ne repond plus aux commandes
        pub mute: bool,
        /// refuse les activites (ERROR)
        pub refuse: bool,
    }

    pub struct FakePipe {
        seen: Arc<Mutex<Seen>>,
        inbox: Vec<u8>,
        out: VecDeque<u8>,
    }

    impl Drop for FakePipe {
        fn drop(&mut self) {
            self.seen.lock().unwrap().closed += 1;
        }
    }

    impl Pipe for FakePipe {
        fn write_all(&mut self, b: &[u8]) -> io::Result<()> {
            self.inbox.extend_from_slice(b);
            while self.inbox.len() >= 8 {
                let op = u32::from_le_bytes(self.inbox[0..4].try_into().unwrap());
                let len = u32::from_le_bytes(self.inbox[4..8].try_into().unwrap()) as usize;
                if self.inbox.len() < 8 + len {
                    break;
                }
                let v: Value = serde_json::from_slice(&self.inbox[8..8 + len]).unwrap();
                self.inbox.drain(..8 + len);
                let mut s = self.seen.lock().unwrap();
                if !s.up {
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, "gone"));
                }
                let reply = match op {
                    OP_HANDSHAKE => {
                        s.handshakes.push(v);
                        Some(json!({ "cmd": "DISPATCH", "evt": "READY", "data": { "v": 1 } }))
                    }
                    OP_FRAME if s.mute => None,
                    OP_FRAME if s.refuse => Some(json!({ "cmd": "SET_ACTIVITY", "evt": "ERROR", "nonce": v["nonce"], "data": { "code": 4000, "message": "bad activity" } })),
                    OP_FRAME => {
                        let a = v["args"]["activity"].clone();
                        s.activities.push(if a.is_null() { None } else { Some(a) });
                        Some(json!({ "cmd": "SET_ACTIVITY", "evt": null, "nonce": v["nonce"], "data": {} }))
                    }
                    _ => None,
                };
                if let Some(r) = reply {
                    self.out.extend(frame(OP_FRAME, &r));
                }
            }
            Ok(())
        }
        fn read_exact_by(&mut self, buf: &mut [u8], _until: Instant) -> io::Result<()> {
            if self.out.len() < buf.len() {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "Discord did not answer"));
            }
            for b in buf.iter_mut() {
                *b = self.out.pop_front().unwrap();
            }
            Ok(())
        }
    }

    /// Une presence branchee sur le faux Discord (sans attente entre deux commandes).
    pub fn presence(allowed: bool) -> (Presence, Arc<Mutex<Seen>>) {
        let seen = Arc::new(Mutex::new(Seen { up: true, ..Default::default() }));
        let s = seen.clone();
        let opener: Opener = Box::new(move || {
            if !s.lock().unwrap().up {
                return Err(io::Error::new(io::ErrorKind::NotFound, "no Discord"));
            }
            Ok(Box::new(FakePipe { seen: s.clone(), inbox: Vec::new(), out: VecDeque::new() }) as Box<dyn Pipe>)
        });
        (Presence::with(allowed, opener, Duration::from_millis(0), Duration::from_millis(30)), seen)
    }

    /// Attend qu'une condition devienne vraie (2 s au plus).
    pub fn until(what: &str, mut f: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(2);
        while !f() {
            assert!(Instant::now() < end, "never happened: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[cfg(all(test, unix))]
mod socket_dirs_tests {
    use super::sys::{socket_dirs, user_temp_dir};
    use std::ffi::OsString;
    use std::path::PathBuf;

    /// (11 nov., relecture L1) Ou chercher le socket de Discord : jamais /tmp sur un Mac.
    #[test]
    fn never_the_shared_tmp_on_a_mac() {
        let env = |k: &str| match k {
            "TMPDIR" => Some(OsString::from("/var/folders/ab/cd/T/")),
            "TMP" => Some(OsString::new()),
            _ => None,
        };
        let mine = PathBuf::from("/var/folders/ab/cd/T");
        let dirs = socket_dirs(env, Some(mine.clone()));
        assert_eq!(dirs[0], PathBuf::from("/var/folders/ab/cd/T/"), "TMPDIR first, an empty TMP skipped");
        if cfg!(target_os = "macos") {
            assert_eq!(dirs, vec![PathBuf::from("/var/folders/ab/cd/T/"), mine]);
            assert!(!socket_dirs(|_| None, None).iter().any(|d| d.starts_with("/tmp")), "no /tmp, even with nothing else");
            // le vrai dossier du compte : sous /var/folders, et TMPDIR quand il est donne
            let t = user_temp_dir().expect("confstr gives the user's temporary folder");
            assert!(t.is_absolute() && t.starts_with("/var/folders") || t.starts_with("/private/var/folders"), "{}", t.display());
            if let Some(v) = std::env::var_os("TMPDIR") {
                assert_eq!(std::fs::canonicalize(&t).ok(), std::fs::canonicalize(PathBuf::from(v)).ok());
            }
        } else {
            assert_eq!(dirs.last(), Some(&PathBuf::from("/tmp")));
            assert_eq!(user_temp_dir(), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{presence, until};
    use super::*;

    fn raw(title: &str) -> RawTrack {
        RawTrack { title: title.into(), artist: Some("Uppermost".into()), album: Some("Origins".into()), cover: Some("https://beta.uppwind.com/covers/origins.jpg".into()), url: Some("https://beta.uppwind.com/?release=origins".into()), duration: Some(200.0), position: Some(20.0) }
    }

    #[test]
    fn texts_are_cleaned_and_cut() {
        assert_eq!(clean("  Flashback\u{0}\n\t(live)  "), "Flashback (live)");
        assert_eq!(clean(&"é".repeat(300)).chars().count(), MAX_TEXT);
        // un emoji compte pour deux (UTF-16), jamais coupe en deux
        let e = clean(&"🎵".repeat(100));
        assert_eq!((e.chars().count(), e.encode_utf16().count()), (64, 128));
        assert_eq!(clean("Flash\u{202E}back\u{200B} live"), "Flashback live");
        let t = Track::from_raw(&RawTrack { title: "X".into(), artist: Some("Y".into()), ..Default::default() }, 1_000_000).unwrap();
        assert_eq!(t.title, "« X »", "Discord refuses a one-character text");
        assert_eq!(t.artist, None);
        assert!(Track::from_raw(&RawTrack { title: " \n ".into(), ..Default::default() }, 0).is_err());
    }

    #[test]
    fn only_https_urls_on_listed_hosts_pass() {
        let ok = |u: &str| https_url(u, IMAGE_HOSTS).is_some();
        assert!(ok("https://beta.uppwind.com/covers/a.jpg"));
        assert!(ok("https://cdn.uppwind.com/v/x.webp?v=2"));
        assert!(!ok("https://uppwind.lucasmoreau.fr/covers/a.jpg"), "the old name is closed (13 Nov 2026)");
        assert!(!ok("https://cdn.lucasmoreau.fr/v/x.webp"), "the old bucket is gone");
        assert!(ok("https://i1.sndcdn.com/artworks-000-t500x500.jpg"));
        assert!(ok("https://UPPWIND.com/a.png"));
        for bad in [
            "http://beta.uppwind.com/a.jpg",
            "https://evil.example/a.jpg",
            "https://beta.uppwind.com.evil.example/a.jpg",
            "https://evil.example/beta.uppwind.com/a.jpg",
            "https://beta.uppwind.com@evil.example/a.jpg",
            "https://beta.uppwind.com:8443/a.jpg",
            "https://sndcdn.com/a.jpg",
            "https://evilsndcdn.com/a.jpg",
            "https://beta.uppwind.com/a b.jpg",
            "https://beta.uppwind.com/a\".jpg",
            "javascript:alert(1)",
            "/covers/a.jpg",
            "",
        ] {
            assert!(!ok(bad), "{bad}");
        }
        assert!(https_url(&format!("https://uppwind.com/{}", "a".repeat(MAX_URL)), IMAGE_HOSTS).is_none());
        assert!(https_url("https://soundcloud.com/uppermost/x", LINK_HOSTS).is_some());
        assert!(https_url("https://i1.sndcdn.com/x", LINK_HOSTS).is_none());
    }

    #[test]
    fn the_activity_is_built_here_from_named_fields() {
        let t = Track::from_raw(&raw("Flashback"), 1_700_000_100_000).unwrap();
        let a = t.activity();
        assert_eq!(a["type"], 2, "Listening");
        assert_eq!(a["details"], "Flashback");
        assert_eq!(a["name"], "Uppermost - Flashback");
        assert_eq!(a["status_display_type"], 0);
        assert_eq!(a["state"], "Uppermost");
        assert_eq!(a["timestamps"]["start"], 1_700_000_080_000u64);
        assert_eq!(a["timestamps"]["end"], 1_700_000_280_000u64);
        assert_eq!(a["assets"]["large_image"], "https://beta.uppwind.com/covers/origins.jpg");
        assert_eq!(a["assets"]["large_text"], "Origins");
        assert_eq!(a["details_url"], "https://beta.uppwind.com/?release=origins");
        // une radio : pas de fin ; une adresse refusee : omise, le reste passe
        let r = Track::from_raw(&RawTrack { title: "Uppwind Radio".into(), cover: Some("https://evil.example/x.png".into()), url: Some("https://evil.example/".into()), duration: Some(0.0), ..Default::default() }, 5_000).unwrap();
        let a = r.activity();
        assert!(a["timestamps"]["end"].is_null() && a["assets"].is_null() && a["details_url"].is_null() && a["state"].is_null());
        assert_eq!(a["name"], "Uppwind Radio");
        // des nombres absurdes : ignores
        let n = Track::from_raw(&RawTrack { title: "Ab".into(), duration: Some(f64::NAN), position: Some(-4.0), ..Default::default() }, 9_000).unwrap();
        assert_eq!((n.start_ms, n.end_ms), (9_000, None));
        let full = t.activity();
        for k in full.as_object().unwrap().keys() {
            let k = k.as_str();
            assert!(["type", "name", "status_display_type", "details", "state", "timestamps", "assets", "details_url"].contains(&k), "{k}");
        }
    }

    #[test]
    fn nothing_leaves_until_allowed_in_the_companion() {
        let (p, seen) = presence(false);
        assert_eq!(p.show(1, Track::from_raw(&raw("Flashback"), 0).unwrap()), Err(REFUSAL));
        std::thread::sleep(Duration::from_millis(40));
        assert!(seen.lock().unwrap().handshakes.is_empty(), "Discord was not even contacted");
        assert!(p.set_allowed(true, "test"));
        assert!(!p.set_allowed(true, "test"), "already on");
        p.show(1, Track::from_raw(&raw("Flashback"), 0).unwrap()).unwrap();
        until("shown", || p.snapshot().shown);
        let s = seen.lock().unwrap();
        assert_eq!(s.handshakes[0]["client_id"], DEFAULT_APP_ID);
        assert_eq!(s.activities[0].as_ref().unwrap()["details"], "Flashback");
    }

    #[test]
    fn it_clears_for_its_owner_when_disallowed_and_closes_the_channel() {
        let (p, seen) = presence(true);
        p.show(7, Track::from_raw(&raw("One"), 0).unwrap()).unwrap();
        until("shown", || p.snapshot().shown);
        p.clear(8); // une autre connexion : rien
        std::thread::sleep(Duration::from_millis(40));
        assert!(p.snapshot().shown);
        p.clear(7);
        until("cleared", || !p.snapshot().shown);
        until("closed", || seen.lock().unwrap().closed == 1);
        assert_eq!(seen.lock().unwrap().activities.last().unwrap(), &None);
        // retirer la permission efface, quel que soit le proprietaire
        p.show(9, Track::from_raw(&raw("Two"), 0).unwrap()).unwrap();
        until("shown again", || p.snapshot().shown);
        let g = p.generation();
        p.set_allowed(false, "test");
        until("cleared by the switch", || !p.snapshot().shown);
        assert!(p.generation() > g);
        assert_eq!(seen.lock().unwrap().activities.last().unwrap(), &None);
        assert_eq!(p.show(9, Track::from_raw(&raw("Three"), 0).unwrap()), Err(REFUSAL));
    }

    #[test]
    fn discord_absent_then_back_and_a_dead_discord() {
        let (p, seen) = presence(true);
        seen.lock().unwrap().up = false;
        p.show(1, Track::from_raw(&raw("Late"), 0).unwrap()).unwrap();
        until("absent", || p.snapshot().link == Link::Absent);
        assert!(!p.snapshot().shown);
        seen.lock().unwrap().up = true; // Discord demarre : le nouvel essai l'affiche
        until("shown after the retry", || p.snapshot() == Snapshot { allowed: true, link: Link::Ok, shown: true });
        assert_eq!(seen.lock().unwrap().activities[0].as_ref().unwrap()["details"], "Late");
        // Discord est quitte : la commande suivante echoue, la presence n'est plus dite affichee
        seen.lock().unwrap().up = false;
        p.show(1, Track::from_raw(&raw("Next"), 0).unwrap()).unwrap();
        until("absent again", || p.snapshot().link == Link::Absent && !p.snapshot().shown);
        // effacer sans Discord : rien a faire, pas de nouvel essai sans fin
        p.clear(1);
        std::thread::sleep(Duration::from_millis(80));
        assert!(!p.snapshot().shown);
    }

    #[test]
    fn a_set_posted_after_the_permission_was_withdrawn_never_reaches_discord() {
        let (p, seen) = presence(true);
        // la course de la relecture : show() a lu « permis », la permission tombe, puis le Set arrive au fil
        p.core.allowed.store(false, Ordering::SeqCst);
        {
            let mut i = p.core.inner();
            p.post(&mut i, Cmd::Set(Box::new(Track::from_raw(&raw("Late"), 0).unwrap())));
        }
        std::thread::sleep(Duration::from_millis(80));
        let s = seen.lock().unwrap();
        assert!(s.activities.iter().all(|a| a.is_none()), "nothing shown: {:?}", s.activities);
    }

    #[test]
    fn a_refused_activity_is_not_called_shown() {
        let (p, seen) = presence(true);
        seen.lock().unwrap().refuse = true;
        p.show(1, Track::from_raw(&raw("Nope"), 0).unwrap()).unwrap();
        until("answered", || p.snapshot().link == Link::Ok);
        assert!(!p.snapshot().shown);
    }

    #[test]
    fn frames_are_little_endian_op_length_json() {
        let f = frame(1, &json!({ "a": 1 }));
        assert_eq!(&f[..8], &[1, 0, 0, 0, 7, 0, 0, 0]);
        assert_eq!(&f[8..], br#"{"a":1}"#);
    }
}
