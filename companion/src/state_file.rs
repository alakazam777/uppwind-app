// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (29 oct., audit MAC-1) Le fichier d'etat du compagnon (%USERPROFILE%\.uppwind-companion.json, ~/.uppwind-companion.json
//! sous macOS) : le jeton d'appairage, le secret des plugins, leur port. Il donne la main sur le compagnon : seul son
//! proprietaire doit pouvoir le lire.
//! - macOS, Linux : ecrit en 0600, par un fichier temporaire cree 0600 a cote puis renomme par-dessus (jamais un instant
//!   lisible par les autres comptes, jamais lu a moitie ecrit), et remis a 0600 a chaque demarrage (un fichier ecrit par
//!   un compagnon d'avant garderait sinon son mode : -rw-r--r-- sur le Mac, que tout le groupe staff lisait).
//! - Windows : rien a faire pour les droits ; le profil n'est lisible que par son proprietaire, SYSTEM et les
//!   administrateurs (ses ACL, que le fichier herite). Le renommage sert aussi (un plugin ne lit jamais un fichier a
//!   moitie ecrit) ; s'il echoue (un programme tient le fichier ouvert sans partage), l'ecriture directe d'avant.
//! - « pairingV2 » (session.rs) : un studio a fait la poignee de main v2 avec ce jeton ; la v1 (le jeton en clair) est
//!   alors refusee, d'un lancement a l'autre, tant que le jeton ne change pas (--new-token l'oublie).
//! - (defense en profondeur) a la sortie propre, pluginPort est retire : un plugin ne frappe plus a un port qu'un autre
//!   programme pourrait reprendre ; le compagnon le reecrit au demarrage. Sortie propre : l'icone qui quitte (tray), Ctrl+C
//!   ou la console fermee (Windows) ; SIGINT, SIGTERM (--stop), SIGHUP (macOS, Linux).
//! Toutes les ecritures du processus passent par ici, l'une apres l'autre (un verrou).

use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static LOCK: Mutex<()> = Mutex::new(());

/// Le mode du fichier sous macOS et Linux : lu et ecrit par son proprietaire, rien pour les autres.
#[cfg(unix)]
pub const MODE: u32 = 0o600;

/// Remettre le fichier a 0600 s'il existe et s'il n'y est pas (un fichier d'un compagnon d'avant). Windows : rien.
pub fn secure(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // (un lien symbolique n'est pas suivi : l'ecriture le remplacera par un vrai fichier)
        if let Ok(m) = std::fs::symlink_metadata(path) {
            if m.file_type().is_file() && m.permissions().mode() & 0o777 != MODE {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(MODE))?;
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Le contenu du fichier (None : absent ou pas du JSON).
pub fn read(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Ecrire tout le fichier, d'un coup (0600 sous macOS et Linux).
pub fn write(path: &Path, v: &Value) -> std::io::Result<()> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    write_locked(path, v)
}

/// Le fichier temporaire, a cote : toujours le meme nom (un reste d'un compagnon arrete au mauvais moment est retire
/// a l'ecriture suivante, il ne s'accumule pas).
fn tmp_path(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "state".into());
    path.with_file_name(format!("{name}.tmp"))
}

fn write_locked(path: &Path, v: &Value) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(v).map_err(std::io::Error::other)?;
    let tmp = tmp_path(path);
    let _ = std::fs::remove_file(&tmp);
    let r = (|| {
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(MODE);
        }
        let mut f = o.open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        // Windows : un programme qui tient le fichier ouvert sans partage empeche le renommage ; l'ecriture directe
        if cfg!(windows) {
            return std::fs::write(path, text.as_bytes());
        }
        return Err(e);
    }
    Ok(())
}

/// Lire, changer, reecrire, sous le verrou. `f` → false : rien a ecrire. → true si le fichier a ete reecrit.
pub fn update(path: &Path, f: impl FnOnce(&mut Value) -> bool) -> std::io::Result<bool> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let raw = std::fs::read_to_string(path)?;
    let mut v: Value = serde_json::from_str(&raw).map_err(std::io::Error::other)?;
    if !f(&mut v) {
        return Ok(false);
    }
    write_locked(path, &v)?;
    Ok(true)
}

/// (29 oct.) Le fichier (celui d'avant ce lancement) dit-il qu'un studio a deja fait la v2 avec ce jeton ?
pub fn pairing_v2(state: &Value, token: &str) -> bool {
    state["token"].as_str() == Some(token) && state["pairingV2"].as_bool() == Some(true)
}

/// La premiere poignee de main v2 reussie avec ce jeton : « pairingV2 » dans le fichier (s'il porte encore ce jeton).
pub fn mark_pairing_v2(path: &Path, token: &str) -> std::io::Result<bool> {
    update(path, |v| {
        if v["token"].as_str() != Some(token) || v["pairingV2"].as_bool() == Some(true) {
            return false;
        }
        v["pairingV2"] = Value::Bool(true);
        true
    })
}

/// Retirer pluginPort, si le fichier est encore celui du compagnon `pid` (un compagnon lance apres a pu le reecrire).
/// Le secret reste (garde d'un lancement a l'autre, comme le jeton).
pub fn forget_plugin_port(path: &Path, pid: u32) -> std::io::Result<bool> {
    update(path, |v| {
        if v["pid"].as_u64() != Some(pid as u64) {
            return false;
        }
        v.as_object_mut().map(|o| o.remove("pluginPort").is_some()).unwrap_or(false)
    })
}

static EXIT: OnceLock<PathBuf> = OnceLock::new();

/// A la sortie propre de ce compagnon : pluginPort retire (s'il est toujours le notre). Sans effet avant `at_exit`.
pub fn on_exit() {
    if let Some(path) = EXIT.get() {
        let _ = forget_plugin_port(path, std::process::id());
    }
}

/// Retenir le fichier d'etat de ce lancement et attraper les sorties propres (voir en tete). Une seule fois.
pub fn at_exit(path: &Path) {
    if EXIT.set(path.to_path_buf()).is_err() {
        return;
    }
    #[cfg(windows)]
    console::install();
    #[cfg(unix)]
    signals::install();
}

/// Windows : Ctrl+C, Ctrl+Break, la console fermee. Le gestionnaire tourne sur un fil a lui ; il range le fichier, puis
/// rend FALSE : le gestionnaire suivant (celui de Windows) termine le processus, comme avant.
#[cfg(windows)]
mod console {
    use windows::core::BOOL;
    use windows::Win32::System::Console::{SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT};

    unsafe extern "system" fn on_ctrl(kind: u32) -> BOOL {
        if matches!(kind, CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT) {
            super::on_exit();
        }
        BOOL(0)
    }

    pub fn install() {
        // SAFETY : une fonction `extern "system"` statique, valable tant que le processus vit
        unsafe {
            let _ = SetConsoleCtrlHandler(Some(on_ctrl), true);
        }
    }
}

/// macOS, Linux : SIGINT, SIGTERM, SIGHUP. Le gestionnaire ne fait qu'ecrire un octet dans un tube (permis dans un
/// gestionnaire de signal) ; un fil le lit, range le fichier et termine le processus. Si ce fil n'est plus la (le tube
/// est ferme : l'ecriture echoue), le gestionnaire termine le processus lui-meme, comme l'action par defaut. Les
/// gestionnaires ne sont poses qu'une fois ce fil parti.
#[cfg(unix)]
mod signals {
    use std::ffi::c_void;
    use std::io::Read;
    use std::os::unix::io::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicI32, Ordering};

    const SIGHUP: i32 = 1;
    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;
    static WAKE: AtomicI32 = AtomicI32::new(-1);

    // (les memes signatures que midi_ports.rs : une seule declaration par fonction C dans la crate)
    extern "C" {
        fn signal(sig: i32, handler: extern "C" fn(i32)) -> usize;
        fn write(fd: i32, buf: *const c_void, n: usize) -> isize;
        fn _exit(status: i32) -> !;
    }

    extern "C" fn on_signal(sig: i32) {
        let fd = WAKE.load(Ordering::Relaxed);
        let b = sig as u8;
        // SAFETY : write(2) et _exit(2) sont permis dans un gestionnaire de signal ; un octet de la pile
        unsafe {
            if fd < 0 || write(fd, &b as *const u8 as *const c_void, 1) != 1 {
                _exit(128 + sig);
            }
        }
    }

    pub fn install() {
        let Ok((mut r, w)) = UnixStream::pair() else { return };
        let _ = w.set_nonblocking(true);
        let spawned = std::thread::Builder::new()
            .name("uwc-signals".into())
            .spawn(move || {
                let mut b = [0u8; 1];
                loop {
                    match r.read(&mut b) {
                        Ok(1) => break,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        // (ne devrait pas arriver : ce processus tient l'autre bout) ; le tube se ferme avec ce fil, et
                        // un signal termine alors le processus directement (on_signal)
                        _ => return,
                    }
                }
                super::on_exit();
                crate::log(&format!("stopped (signal {})", b[0]));
                std::process::exit(0);
            })
            .is_ok();
        if !spawned {
            return;
        }
        WAKE.store(w.as_raw_fd(), Ordering::Relaxed);
        // l'autre bout du tube vit autant que le processus
        std::mem::forget(w);
        for s in [SIGHUP, SIGINT, SIGTERM] {
            // SAFETY : un gestionnaire `extern "C"` statique qui ne fait qu'un write(2) (ou _exit)
            unsafe { signal(s, on_signal) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("uwc-state-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn writes_are_whole_and_updates_keep_the_rest() {
        let d = dir("write");
        let f = d.join(".uppwind-companion.json");
        write(&f, &json!({"port": 47411, "token": "t", "pid": 7, "pluginPort": 50000, "pluginSecret": "s"})).unwrap();
        assert_eq!(read(&f).unwrap()["pluginPort"], 50000);
        assert!(!tmp_path(&f).exists(), "no temporary file left");
        // pluginPort retire seulement par le compagnon qui l'a ecrit (son pid) ; le secret et le reste restent
        assert_eq!(forget_plugin_port(&f, 8).unwrap(), false, "another companion's file: left alone");
        assert_eq!(read(&f).unwrap()["pluginPort"], 50000);
        assert_eq!(forget_plugin_port(&f, 7).unwrap(), true);
        let v = read(&f).unwrap();
        assert!(v.get("pluginPort").is_none() && v["pluginSecret"] == "s" && v["token"] == "t", "{v}");
        assert_eq!(forget_plugin_port(&f, 7).unwrap(), false, "already gone");
        assert!(forget_plugin_port(&d.join("missing.json"), 7).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn pairing_v2_belongs_to_one_token() {
        let d = dir("v2");
        let f = d.join("state.json");
        write(&f, &json!({"token": "aaa", "pid": 1})).unwrap();
        assert!(!pairing_v2(&read(&f).unwrap(), "aaa"));
        assert_eq!(mark_pairing_v2(&f, "bbb").unwrap(), false, "not this file's token (another companion rewrote it)");
        assert_eq!(mark_pairing_v2(&f, "aaa").unwrap(), true);
        assert_eq!(mark_pairing_v2(&f, "aaa").unwrap(), false, "once");
        let v = read(&f).unwrap();
        assert!(pairing_v2(&v, "aaa"));
        assert!(!pairing_v2(&v, "bbb"), "a new token (--new-token) starts over: v1 is allowed again until a v2 handshake");
        assert!(!pairing_v2(&json!({"token": "aaa", "pairingV2": "yes"}), "aaa"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// (audit MAC-1) macOS, Linux : le fichier est cree 0600, et un fichier d'avant en 0644 y est remis au demarrage.
    #[cfg(unix)]
    #[test]
    fn the_state_file_is_0600_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("mode");
        let f = d.join(".uppwind-companion.json");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        // un fichier d'un compagnon d'avant, lisible par tous
        std::fs::write(&f, "{\"token\":\"t\"}").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        secure(&f).unwrap();
        assert_eq!(mode(&f), 0o600, "re-set at start");
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        write(&f, &json!({"token": "t", "pid": 3})).unwrap();
        assert_eq!(mode(&f), 0o600, "a write makes a 0600 file");
        update(&f, |v| {
            v["x"] = json!(1);
            true
        })
        .unwrap();
        assert_eq!(mode(&f), 0o600, "an update too");
        let g = d.join("fresh.json");
        write(&g, &json!({})).unwrap();
        assert_eq!(mode(&g), 0o600, "a new file: 0600 from the start");
        secure(&d.join("missing.json")).unwrap();
        let _ = std::fs::remove_dir_all(&d);
    }
}
