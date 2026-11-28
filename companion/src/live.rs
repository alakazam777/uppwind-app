// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Parler au script de Live : des datagrammes UDP en JSON sur 127.0.0.1, avec le jeton que le script ecrit dans
//! ~/.uppwind-link-dev.json. Depuis le 24 oct., c'est le vrai script UppwindLink (v1.1.0, un seul Control Surface :
//! ableton/UppwindLink/editor_channel.py) ; le script de dev de l'etape 0 (UppwindLinkDev) parle
//! le meme canal, et le fichier d'etat dit lequel sert (« script »).
//! Le compagnon n'ouvre ici qu'un socket CLIENT ephemere par requete (rien n'ecoute en dehors de la reponse attendue,
//! et seule une reponse de 127.0.0.1:<port du script> est lue).

use serde_json::{json, Value};
use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const STATE_NAME: &str = ".uppwind-link-dev.json";
pub const ENV_FILE: &str = "UWLINK_DEV_FILE";

pub fn state_file() -> PathBuf {
    if let Ok(p) = std::env::var(ENV_FILE) {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    crate::home_dir().join(STATE_NAME)
}

/// Le canal est-il la (le fichier d'etat existe et se lit) ?
pub fn available() -> bool {
    read_state().is_ok()
}

fn read_state() -> Result<(u16, String), String> {
    read_state_at(&state_file())
}

fn read_state_at(p: &Path) -> Result<(u16, String), String> {
    read_state_full(p).map(|(port, token, _)| (port, token))
}

/// (24 oct.) Qui sert le canal (« UppwindLink », « UppwindLinkDev ») et la version de Live, d'apres le fichier d'etat.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Who {
    pub script: Option<String>,
    pub live: Option<String>,
    /// (25 oct.) la version du script (« 1.3.0 ») et s'il sert le lien du studio (LINK, ROUTES : « link » dans le fichier)
    pub version: Option<String>,
    pub link: bool,
}

/// (audit de publication) les erreurs partent au studio (« live », « devices », « opened ») : jamais le chemin du
/// fichier d'etat, qui contient le nom du compte Windows. La console du compagnon le donne (main.rs).
fn read_state_full(p: &Path) -> Result<(u16, String, Who), String> {
    let raw = std::fs::read_to_string(p).map_err(|_| "Live's UppwindLink script is not running (is Live open with UppwindLink as a Control Surface?)".to_string())?;
    let v: Value = serde_json::from_str(&raw).map_err(|_| "the state file of Live's UppwindLink script is not JSON".to_string())?;
    let port = v["port"].as_u64().filter(|p| *p > 0 && *p < 65536).ok_or("no port in the state file")? as u16;
    let token = v["token"].as_str().filter(|t| !t.is_empty()).ok_or("no token in the state file")?.to_string();
    // (audit F6) si Live a plante, le fichier reste et un autre programme peut tenir le port : le jeton ne part que si
    // le processus qui a ecrit le fichier (son pid) vit encore et a demarre avant le fichier. Un script d'avant la
    // 0.4.16 n'ecrit pas de pid : rien a verifier (le studio propose deja la mise a jour du script).
    if let Some(pid) = v["pid"].as_u64() {
        let started = v["started"].as_u64().unwrap_or(u64::MAX);
        if !writer_alive(pid, started) {
            return Err(GONE.to_string());
        }
    }
    let txt = |k: &str| v[k].as_str().map(|s| s.chars().filter(|c| !c.is_control()).take(40).collect::<String>()).filter(|s| !s.is_empty());
    let link = v["link"].as_u64().map(|n| n >= 1).unwrap_or(false);
    Ok((port, token, Who { script: txt("script"), live: txt("live"), version: txt("version"), link }))
}

/// (relecture F6) La marge entre l'heure de demarrage du processus et `started` : une horloge remise a l'heure apres le
/// lancement de Live ne bloque pas le canal ; un pid repris apres un plantage demarre bien plus tard.
pub const START_MARGIN_S: u64 = 60;

/// (relecture F6) Le processus de Live, tenu pendant que le relais du lien parle au script : sous Windows une poignee
/// ouverte (son pid ne peut pas etre repris tant qu'elle vit) ; ailleurs son heure de demarrage, relue a chaque fois.
pub struct Writer {
    #[cfg_attr(windows, allow(dead_code))]
    pid: u32,
    start: u64,
    #[cfg(windows)]
    h: windows::Win32::Foundation::HANDLE,
}

unsafe impl Send for Writer {}

impl Writer {
    /// Le processus `pid`, s'il vit et a demarre au plus tard a `started` (+ la marge).
    pub fn open(pid: u64, started: u64) -> Option<Writer> {
        let pid = u32::try_from(pid).ok().filter(|p| *p != 0)?;
        #[cfg(windows)]
        let h = unsafe { windows::Win32::System::Threading::OpenProcess(windows::Win32::System::Threading::PROCESS_SYNCHRONIZE, false, pid) }.ok()?;
        let w = Writer {
            pid,
            start: process_start_secs(pid).unwrap_or(u64::MAX),
            #[cfg(windows)]
            h,
        };
        (w.start <= started.saturating_add(START_MARGIN_S) && w.alive()).then_some(w)
    }

    /// Vit-il encore (le meme processus) ?
    pub fn alive(&self) -> bool {
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::WAIT_TIMEOUT;
            unsafe { windows::Win32::System::Threading::WaitForSingleObject(self.h, 0) == WAIT_TIMEOUT }
        }
        #[cfg(not(windows))]
        {
            process_start_secs(self.pid) == Some(self.start)
        }
    }
}

#[cfg(windows)]
impl Drop for Writer {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.h);
        }
    }
}

/// (audit F6) Le processus `pid` vit-il, et a-t-il demarre au plus tard a `started` (secondes Unix, + la marge) ?
/// Un pid repris par un autre programme apres le plantage de Live a demarre apres le fichier : il est ecarte.
pub fn writer_alive(pid: u64, started: u64) -> bool {
    let Ok(pid) = u32::try_from(pid) else { return false };
    if pid == 0 {
        return false;
    }
    match process_start_secs(pid) {
        Some(t) => t <= started.saturating_add(START_MARGIN_S),
        None => false,
    }
}

/// L'heure de demarrage du processus (secondes Unix), None s'il n'existe pas.
#[cfg(windows)]
fn process_start_secs(pid: u32) -> Option<u64> {
    use windows::Win32::Foundation::{CloseHandle, FILETIME, STILL_ACTIVE};
    use windows::Win32::System::Threading::{GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let mut code = 0u32;
        let ok = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u).is_ok() && GetExitCodeProcess(h, &mut code).is_ok() && code == STILL_ACTIVE.0 as u32;
        let _ = CloseHandle(h);
        if !ok {
            return None;
        }
        // FILETIME : des centaines de ns depuis 1601
        let t = ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64;
        Some((t / 10_000_000).saturating_sub(11_644_473_600))
    }
}

#[cfg(target_os = "macos")]
fn process_start_secs(pid: u32) -> Option<u64> {
    // proc_pidinfo(PROC_PIDTBSDINFO) : struct proc_bsdinfo, 136 octets, pbi_start_tvsec (u64) a l'octet 120
    extern "C" {
        fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut u8, size: i32) -> i32;
    }
    let mut b = [0u8; 136];
    let n = unsafe { proc_pidinfo(pid as i32, 3, 0, b.as_mut_ptr(), b.len() as i32) };
    if n as usize != b.len() {
        return None;
    }
    Some(u64::from_ne_bytes(b[120..128].try_into().ok()?))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn process_start_secs(pid: u32) -> Option<u64> {
    std::path::Path::new(&format!("/proc/{pid}")).exists().then_some(0)
}

/// Une requete → la reponse (un objet JSON avec « ok »). Live repond depuis update_display (~100 ms).
pub fn request(msg: Value, timeout: Duration) -> Result<Value, String> {
    request_at(&state_file(), msg, timeout)
}

/// Le script de dev repond-il, LA, maintenant ? (23 oct.) Le studio le redemande toutes les quelques secondes : Live
/// demarre souvent APRES le compagnon (le « liveChannel » du hello est alors perime). EVENTS est la requete la plus
/// legere du script : ni ecriture dans Log.txt, ni parcours du set.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Probe {
    /// Le fichier d'etat du script est la (Live l'a ecrit au demarrage, l'efface en quittant).
    pub channel: bool,
    /// Une reponse est arrivee (ms : l'aller-retour).
    pub answers: bool,
    pub ms: Option<f64>,
    pub error: Option<String>,
    /// (24 oct.) Le script qui sert le canal (« UppwindLink » ; « UppwindLinkDev » : celui de l'etape 0) et la version
    /// de Live, lus dans le fichier d'etat (absents d'un script d'avant : None).
    pub script: Option<String>,
    pub live: Option<String>,
    /// (25 oct.) la version du script, et s'il sert le lien du studio (LINK) : le studio s'en passe alors de loopMIDI
    pub version: Option<String>,
    pub link: bool,
}

pub const PROBE_TIMEOUT: Duration = Duration::from_millis(800);

pub fn probe() -> Probe {
    probe_at(&state_file(), PROBE_TIMEOUT)
}

pub fn probe_at(path: &Path, timeout: Duration) -> Probe {
    let who = match read_state_full(path) {
        Ok((_, _, w)) => w,
        Err(e) => return Probe { channel: false, answers: false, ms: None, error: Some(e), script: None, live: None, version: None, link: false },
    };
    let t0 = Instant::now();
    let (answers, ms, error) = match request_at(path, json!({ "cmd": "EVENTS" }), timeout) {
        Ok(_) => (true, Some((t0.elapsed().as_secs_f64() * 10000.0).round() / 10.0), None),
        Err(e) => (false, None, Some(e)),
    };
    Probe { channel: true, answers, ms, error, script: who.script, live: who.live, version: who.version, link: who.link }
}

/// (26 oct.) Pour le relais du lien (link_relay.rs, un socket qui reste ouvert ; avant : une requete et son socket par
/// echange, link_exchange) : le port du script, son jeton et ce
/// qu'il sert du lien (« link » : 1 = LINK ; 2 = un releve vide peut attendre, « wait »). Les erreurs : comme read_state.
/// (relecture F6) Et le processus de Live tenu (None : un script d'avant la 0.4.16, sans pid), que le relais revoit
/// avant chaque envoi.
pub fn link_target_at(path: &Path) -> Result<(u16, String, u64, Option<Writer>), String> {
    let (port, token) = read_state_at(path)?;
    let v = std::fs::read_to_string(path).ok().and_then(|raw| serde_json::from_str::<Value>(&raw).ok()).unwrap_or(Value::Null);
    let level = v["link"].as_u64().unwrap_or(0);
    let writer = match v["pid"].as_u64() {
        Some(pid) => Some(Writer::open(pid, v["started"].as_u64().unwrap_or(u64::MAX)).ok_or_else(|| GONE.to_string())?),
        None => None,
    };
    Ok((port, token, level, writer))
}

pub const GONE: &str = "Live's UppwindLink script is gone (Live closed or crashed): open Live again";

/// Le numero d'une requete (link_relay.rs aussi).
pub fn new_request_id() -> u64 {
    request_id()
}

/// (25 oct.) ROUTES : l'entree de chaque piste de Live et ses Notes (le diagnostic du chemin sans loopMIDI).
pub fn routes() -> Result<Value, String> {
    let rep = request(json!({ "cmd": "ROUTES" }), Duration::from_millis(1500))?;
    if rep["ok"].as_bool() != Some(true) {
        return Err(rep["error"].as_str().unwrap_or("Live's script refused ROUTES (update it)").chars().take(200).collect());
    }
    Ok(json!({ "type": "routes", "ok": true, "tracks": rep["tracks"], "notesInBrowser": rep["notesInBrowser"] }))
}

/// Le numero d'une requete : au hasard (une reponse forgee par un autre programme local devrait le deviner), 32 bits.
fn request_id() -> u64 {
    let mut b = [0u8; 4];
    match getrandom::fill(&mut b) {
        Ok(()) => (u32::from_le_bytes(b) | 1) as u64,
        Err(_) => (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(1) % 4_000_000_000) as u64 | 1,
    }
}

fn request_at(path: &Path, mut msg: Value, timeout: Duration) -> Result<Value, String> {
    let (port, token) = read_state_at(path)?;
    let id = request_id();
    msg["token"] = json!(token);
    msg["id"] = json!(id);
    let sock = UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    sock.set_read_timeout(Some(Duration::from_millis(100))).map_err(|e| e.to_string())?;
    let target = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    sock.send_to(msg.to_string().as_bytes(), target).map_err(|e| e.to_string())?;
    let t0 = Instant::now();
    let mut buf = vec![0u8; 65536];
    while t0.elapsed() < timeout {
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                if from != target {
                    continue;
                }
                if let Ok(v) = serde_json::from_slice::<Value>(&buf[..n]) {
                    if v["id"].as_u64() == Some(id) {
                        return Ok(v);
                    }
                }
            }
            Err(_) => continue, // le delai (ou 10054 sous Windows si le port ne repond pas encore)
        }
    }
    Err(format!("Live did not answer on udp://127.0.0.1:{port} within {:.1} s (Live closed?)", timeout.as_secs_f64()))
}

/// Un plugin du set, tel que le studio le liste.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Device {
    pub path: String,
    pub name: String,
    pub track: String,
    #[serde(rename = "editorApi")]
    pub editor_api: bool,
}

/// La reponse de LIST → les plugins (les devices marques « plugin »), dans l'ordre du set.
pub fn plugins_of_list(rep: &Value) -> Vec<Device> {
    let mut out = Vec::new();
    for t in rep["tracks"].as_array().into_iter().flatten() {
        let track = t["name"].as_str().unwrap_or("").to_string();
        for d in t["devices"].as_array().into_iter().flatten() {
            if d["plugin"].as_bool() != Some(true) {
                continue;
            }
            let Some(path) = d["path"].as_str() else { continue };
            out.push(Device { path: path.to_string(), name: d["name"].as_str().unwrap_or("").to_string(), track: track.clone(), editor_api: d["editor"].as_bool() == Some(true) });
        }
    }
    out
}

/// La reponse d'EDITOR_OPEN / EDITOR_CLOSE → la fenetre etait-elle ouverte avant (« before ») ? None : on ne sait pas.
/// Le studio (Follow track) ne referme ensuite que les fenetres qu'il a ouvertes lui-meme.
pub fn was_open(rep: &Value) -> Option<bool> {
    rep["before"].as_bool()
}

/// Un chemin de device bien forme : « 2.0 », « 2.0.1.0 » (des entiers separes par des points, 8 au plus).
pub fn path_ok(p: &str) -> bool {
    let parts: Vec<&str> = p.split('.').collect();
    (2..=8).contains(&parts.len()) && parts.iter().all(|x| !x.is_empty() && x.len() <= 4 && x.bytes().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugins_from_a_list_reply() {
        let rep = json!({"ok": true, "tracks": [
            {"index": 0, "name": "1-Kontakt 8", "devices": [{"path": "0.0", "name": "Kontakt 8", "class_name": "PluginDevice", "plugin": true, "editor": true}]},
            {"index": 1, "name": "2-Diva", "devices": [
                {"path": "1.0", "name": "Diva", "class_name": "PluginDevice", "plugin": true, "editor": true},
                {"path": "1.1", "name": "EQ Eight", "class_name": "Eq8", "plugin": false, "editor": false}]},
            {"index": 2, "name": "Audio", "devices": []}]});
        let p = plugins_of_list(&rep);
        assert_eq!(p.len(), 2);
        assert_eq!(p[1], Device { path: "1.0".into(), name: "Diva".into(), track: "2-Diva".into(), editor_api: true });
        assert_eq!(crate::plugins::expected_title(&p[1].name, &p[1].track), "Diva/2-Diva");
        assert!(plugins_of_list(&json!({"ok": false})).is_empty());
    }

    #[test]
    fn editor_was_open_before() {
        assert_eq!(was_open(&json!({"ok": true, "before": false, "after": true})), Some(false));
        assert_eq!(was_open(&json!({"ok": true, "before": true, "after": true})), Some(true));
        assert_eq!(was_open(&json!({"ok": true})), None);
        assert_eq!(was_open(&json!({"ok": true, "before": "x"})), None);
    }

    /// Un dossier a part pour un fichier d'etat de test (jamais ~/.uppwind-link-dev.json).
    fn temp_state(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("uwc-probe-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(STATE_NAME)
    }

    #[test]
    fn probe_says_whether_the_dev_script_answers() {
        // pas de fichier d'etat : Live n'a pas (encore) demarre le script
        let none = temp_state("none");
        let p = probe_at(&none, Duration::from_millis(300));
        assert_eq!((p.channel, p.answers), (false, false));
        let e = p.error.unwrap();
        assert!(e.contains("UppwindLink as a Control Surface"), "{e}");
        assert!(!e.contains(&none.display().to_string()) && !e.contains(STATE_NAME), "no path goes to the studio: {e}");
        std::fs::write(&none, "{not json").unwrap();
        let e = probe_at(&none, Duration::from_millis(300)).error.unwrap();
        assert!(e.contains("not JSON") && !e.contains(&none.display().to_string()), "{e}");
        std::fs::remove_file(&none).unwrap();

        // un faux script : il lit le jeton et repond a EVENTS avec le meme id
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = sock.local_addr().unwrap().port();
        let path = temp_state("ok");
        std::fs::write(&path, json!({"port": port, "token": "t0k", "script": "UppwindLink", "version": "1.1.0", "live": "12.4.6"}).to_string()).unwrap();
        let fake = std::thread::spawn(move || {
            let mut buf = [0u8; 2048];
            let (n, from) = sock.recv_from(&mut buf).unwrap();
            let req: Value = serde_json::from_slice(&buf[..n]).unwrap();
            assert_eq!((req["cmd"].as_str(), req["token"].as_str()), (Some("EVENTS"), Some("t0k")));
            sock.send_to(json!({"id": req["id"], "ok": true, "events": []}).to_string().as_bytes(), from).unwrap();
        });
        let p = probe_at(&path, Duration::from_millis(1500));
        fake.join().unwrap();
        assert_eq!((p.channel, p.answers, p.error.is_none()), (true, true, true), "{p:?}");
        assert!(p.ms.unwrap() >= 0.0);
        // (24 oct.) qui sert le canal, et quel Live
        assert_eq!((p.script.as_deref(), p.live.as_deref()), (Some("UppwindLink"), Some("12.4.6")));
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!((v["script"].as_str(), v["live"].as_str()), (Some("UppwindLink"), Some("12.4.6")));

        // le fichier est la mais Live s'est tu (plante, ferme sans effacer le fichier) : pas de reponse, vite
        let mute = UdpSocket::bind("127.0.0.1:0").unwrap();
        let stale = temp_state("mute");
        std::fs::write(&stale, json!({"port": mute.local_addr().unwrap().port(), "token": "t0k"}).to_string()).unwrap();
        let t0 = Instant::now();
        let p = probe_at(&stale, Duration::from_millis(300));
        assert_eq!((p.channel, p.answers), (true, false));
        assert_eq!(p.script, None, "a state file from before 24 Oct: no script name");
        assert!(p.error.unwrap().contains("did not answer"));
        assert!(t0.elapsed() < Duration::from_millis(1000));
        drop(mute);
        for f in [none, path, stale] {
            let _ = std::fs::remove_dir_all(f.parent().unwrap());
        }
    }

    #[test]
    fn the_token_stays_home_when_the_script_is_gone() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let me = std::process::id() as u64;
        assert!(super::writer_alive(me, now), "this process is alive and started before now");
        assert!(!super::writer_alive(me, now - 3600), "a pid that started after the file is someone else");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap()).arg("--list").spawn().unwrap();
        let gone = child.id() as u64;
        child.wait().unwrap();
        assert!(!super::writer_alive(gone, now + 60), "a finished process");
        // un fichier d'etat avec le pid d'un processus fini : le jeton ne part pas, aucun paquet n'est envoye
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
        let path = temp_state("gone");
        std::fs::write(&path, json!({"port": sock.local_addr().unwrap().port(), "token": "t0k", "pid": gone, "started": now}).to_string()).unwrap();
        let e = super::request_at(&path, json!({"cmd": "EVENTS"}), Duration::from_millis(200)).unwrap_err();
        assert!(e.contains("gone"), "{e}");
        let mut buf = [0u8; 512];
        assert!(sock.recv_from(&mut buf).is_err(), "the token was sent");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn request_ids_are_random() {
        let ids: std::collections::HashSet<u64> = (0..50).map(|_| request_id()).collect();
        assert!(ids.len() >= 49, "random ids, not a clock: {}", ids.len());
        assert!(ids.iter().all(|i| *i > 0 && *i <= u32::MAX as u64));
    }

    #[test]
    fn device_paths() {
        assert!(path_ok("2.0"));
        assert!(path_ok("2.0.1.0"));
        assert!(!path_ok("2"));
        assert!(!path_ok("2..0"));
        assert!(!path_ok("a.0"));
        assert!(!path_ok("1.2.3.4.5.6.7.8.9"));
    }
}
