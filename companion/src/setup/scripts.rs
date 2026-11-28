// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Les scripts des DAW, embarques dans l'exe (build.rs), compares a ce qui est installe, et installes, mis a jour ou
//! retires avec `--setup --apply`.
//!
//! Regles :
//!   - un dossier existant n'est jamais ecrase sans copie : il est d'abord copie dans
//!     %LOCALAPPDATA%\Uppwind Link\backups\<daw>-<horodatage>\<dossier> (hors des dossiers de Live et de FL, qui
//!     prendraient la copie pour un autre script), la copie est verifiee, puis seulement le dossier est remplace ;
//!   - si l'ecriture echoue en route, le dossier d'avant est remis depuis la copie ;
//!   - une version installee plus recente que celle de l'exe n'est pas remplacee (sauf --force) ;
//!   - la User Library (Live) et le dossier de donnees (FL) doivent deja exister : `--setup` ne cree pas de
//!     bibliotheque sur un disque debranche ;
//!   - un fichier .uppwind-link-install.json dans le dossier installe dit quelle version le compagnon y a mise ;
//!   - (24 nov., audit M3) uwl_secret.py : le code du chemin MIDI de ce script (15 octets au hasard, 24 caracteres en
//!     base 32), ecrit a l'installation et GARDE d'une mise a jour a l'autre ; le studio appaire le demande au compagnon
//!     (midicodes). Le script l'importe : FL ne lui laisse ouvrir aucun fichier, mais il importe les modules voisins.

use super::Mode;
use serde::Serialize;
use std::path::{Path, PathBuf};

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded_scripts.rs"));
}

pub const MARKER: &str = ".uppwind-link-install.json";
/// (24 nov.) le code du chemin MIDI, a cote du script (uwlink_protocol.py : secret_of)
pub const SECRET_FILE: &str = "uwl_secret.py";
/// (24 nov., relecture) le temporaire de write_secret : cree prive, puis renomme a la place de SECRET_FILE
pub const SECRET_TMP: &str = "uwl_secret.py.tmp";
pub const SECRET_BYTES: usize = 15;
const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// 15 octets → « ABCD-EFGH-IJKL-MNOP-QRST-UVWX » (le code_of de uwlink_protocol.py, octet pour octet)
pub fn code_of(secret: &[u8; SECRET_BYTES]) -> String {
    let mut acc: u32 = 0;
    let mut bits = 0;
    let mut chars = String::with_capacity(24);
    for &x in secret {
        acc = (acc << 8) | x as u32;
        bits += 8;
        while bits >= 5 {
            chars.push(B32[((acc >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
        acc &= (1 << bits) - 1;
    }
    chars.as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join("-")
}

/// Un code (tirets, espaces, casse indifferents) → sa forme ecrite, ou None
pub fn normalize_code(code: &str) -> Option<String> {
    let t: String = code.chars().filter(|c| !matches!(c, '-' | ' ' | '\t')).map(|c| c.to_ascii_uppercase()).collect();
    if t.len() != 24 || !t.bytes().all(|b| B32.contains(&b)) {
        return None;
    }
    Some(t.as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join("-"))
}

/// Le code d'un uwl_secret.py (la ligne SECRET = "…"), ou None
pub fn parse_secret(src: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(src).ok()?;
    let line = s.lines().find(|l| l.trim_start().starts_with("SECRET = \""))?;
    let v = line.trim_start().strip_prefix("SECRET = \"")?;
    normalize_code(&v[..v.find('"')?])
}

pub fn read_secret(dir: &Path) -> Option<String> {
    std::fs::read(dir.join(SECRET_FILE)).ok().and_then(|b| parse_secret(&b))
}

pub fn new_code() -> Result<String, String> {
    let mut b = [0u8; SECRET_BYTES];
    getrandom::fill(&mut b).map_err(|e| format!("no system random source ({e})"))?;
    Ok(code_of(&b))
}

/// Ecrit uwl_secret.py avec ce code. (24 nov., relecture) Par un temporaire cree neuf et deja prive (0600 sur un Mac :
/// jamais un instant lisible par les autres comptes), puis renomme a la place de l'ancien (d'un coup : un echec ne perd
/// pas le code d'avant). Sous Windows, aucun controle d'acces n'est pose : le fichier a ceux de son dossier (SECURITY.md).
pub fn write_secret(dir: &Path, code: &str) -> Result<(), String> {
    use std::io::Write;
    let p = dir.join(SECRET_FILE);
    let tmp = dir.join(SECRET_TMP);
    let text = format!(
        "# -*- coding: utf-8 -*-\n# Uppwind Link: the MIDI link code of this script, written by the Uppwind Link companion when it installed\n\
         # the script. Uppwind Studio gets it from the companion it is paired with. Keep it private: whoever has it can\n\
         # drive this script through the MIDI port. Delete this file to get a new code at the next install.\nSECRET = \"{code}\"\n"
    );
    let _ = std::fs::remove_file(&tmp);
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let written = o.open(&tmp).and_then(|mut f| {
        f.write_all(text.as_bytes())?;
        f.sync_all()
    });
    if let Err(e) = written.and_then(|_| std::fs::rename(&tmp, &p)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("cannot write {} ({e})", p.display()));
    }
    if read_secret(dir).as_deref() != Some(code) {
        return Err(format!("{} does not read back as written", p.display()));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Daw {
    Ableton,
    Fl,
}

impl Daw {
    pub fn label(self) -> &'static str {
        match self {
            Daw::Ableton => "Ableton Live",
            Daw::Fl => "FL Studio",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Daw::Ableton => "ableton",
            Daw::Fl => "fl",
        }
    }
}

/// Un script tel qu'il part : le nom du dossier, ses fichiers, le fichier qui porte SCRIPT_VERSION.
pub struct Bundle {
    pub daw: Daw,
    pub folder: &'static str,
    pub files: &'static [(&'static str, &'static [u8])],
    pub version_file: &'static str,
}

pub fn bundle(daw: Daw) -> Bundle {
    match daw {
        Daw::Ableton => Bundle { daw, folder: super::ableton::FOLDER, files: embedded::ABLETON_FILES, version_file: "UppwindLink.py" },
        Daw::Fl => Bundle { daw, folder: super::fl::FOLDER, files: embedded::FL_FILES, version_file: "device_UppwindLink.py" },
    }
}

/// SCRIPT_VERSION = "1.0.0" dans un fichier Python.
pub fn version_in(src: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(src);
    let i = s.find("SCRIPT_VERSION = \"")? + 18;
    let end = s[i..].find('"')?;
    Some(s[i..i + end].to_string())
}

impl Bundle {
    pub fn version(&self) -> String {
        self.files.iter().find(|(n, _)| *n == self.version_file).and_then(|(_, b)| version_in(b)).unwrap_or_else(|| "?".into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// pas de dossier
    Missing,
    /// les memes fichiers, octet pour octet
    UpToDate,
    /// une version plus ancienne
    Outdated,
    /// la meme version (ou une inconnue), d'autres octets : modifie a la main, ou une autre branche
    Different,
    /// une version plus recente que celle de l'exe
    Newer,
    /// le dossier parent manque (User Library, dossier de donnees de FL) : rien a faire ici
    NoLibrary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    None,
    Install,
    Update,
    Remove,
    Skip,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileState {
    pub name: String,
    /// same, differs, missing, extra
    pub state: &'static str,
}

/// Un dossier de script : ce qui y est, ce que `--apply` y ferait.
#[derive(Debug, Clone, Serialize)]
pub struct Target {
    pub daw: Daw,
    /// le dossier du script (…\Remote Scripts\UppwindLink, …\Settings\Hardware\Uppwind Link)
    pub dir: PathBuf,
    /// le dossier qui doit exister (la User Library, le dossier de donnees de FL)
    pub library: PathBuf,
    pub status: Status,
    pub installed_version: Option<String>,
    pub bundled_version: String,
    pub files: Vec<FileState>,
    /// le fichier .uppwind-link-install.json est la : le compagnon l'a installe
    pub by_companion: bool,
    pub action: Action,
    pub reason: String,
    pub backup: Option<PathBuf>,
    /// d'autres dossiers « Uppwind… » a cote (UppwindLinkDev) : laisses tels quels
    pub neighbours: Vec<String>,
    /// apres --apply : l'etat relu
    pub after: Option<Status>,
}

pub fn cmp_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let p = |s: &str| s.split(|c: char| !c.is_ascii_digit()).filter(|x| !x.is_empty()).map(|x| x.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    p(a).cmp(&p(b))
}

/// Ce qu'il y a dans `dir` compare a `b`.
pub fn inspect(b: &Bundle, dir: &Path, library: &Path) -> Target {
    let mut t = Target {
        daw: b.daw,
        dir: dir.to_path_buf(),
        library: library.to_path_buf(),
        status: Status::Missing,
        installed_version: None,
        bundled_version: b.version(),
        files: vec![],
        by_companion: dir.join(MARKER).is_file(),
        action: Action::None,
        reason: String::new(),
        backup: None,
        neighbours: vec![],
        after: None,
    };
    if let Some(parent) = dir.parent() {
        if let Ok(rd) = std::fs::read_dir(parent) {
            let mut n: Vec<String> = rd
                .flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.to_ascii_lowercase().starts_with("uppwind") && n != b.folder)
                .collect();
            n.sort();
            t.neighbours = n;
        }
    }
    if !library.is_dir() {
        t.status = Status::NoLibrary;
        return t;
    }
    if !dir.is_dir() {
        return t;
    }
    let mut all_same = true;
    for (name, bytes) in b.files {
        let state = match std::fs::read(dir.join(name)) {
            Ok(have) if have == *bytes => "same",
            Ok(_) => "differs",
            Err(_) => "missing",
        };
        all_same &= state == "same";
        t.files.push(FileState { name: name.to_string(), state });
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut extra: Vec<String> = rd
            .flatten()
            .filter(|e| e.path().is_file())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != MARKER && n != SECRET_FILE && n != SECRET_TMP && !n.ends_with(".pyc") && !b.files.iter().any(|(f, _)| f == n))
            .collect();
        extra.sort();
        for n in extra {
            t.files.push(FileState { name: n, state: "extra" });
        }
    }
    t.installed_version = std::fs::read(dir.join(b.version_file)).ok().and_then(|x| version_in(&x));
    t.status = if all_same {
        Status::UpToDate
    } else {
        match t.installed_version.as_deref() {
            Some(v) if t.bundled_version != "?" => match cmp_versions(v, &t.bundled_version) {
                std::cmp::Ordering::Less => Status::Outdated,
                std::cmp::Ordering::Greater => Status::Newer,
                std::cmp::Ordering::Equal => Status::Different,
            },
            _ => Status::Different,
        }
    };
    t
}

/// Ce que `--apply` ferait, et la copie de sauvegarde qu'il prendrait.
pub fn plan(t: &mut Target, mode: Mode, force: bool, backups: &Path, stamp: &str) {
    let backup = || backups.join(format!("{}-{stamp}", t.daw.key())).join(t.dir.file_name().unwrap_or_default());
    let (action, reason) = match (mode, t.status) {
        (_, Status::NoLibrary) => (
            Action::Skip,
            match t.daw {
                Daw::Ableton => format!("the User Library {} does not exist (start Live once, or check Settings › Library)", t.library.display()),
                Daw::Fl => format!("FL's user data folder {} does not exist (start FL once)", t.library.display()),
            },
        ),
        (Mode::Remove, Status::Missing) => (Action::None, "not installed".into()),
        (Mode::Remove, _) => (Action::Remove, "remove the folder (a copy is kept)".into()),
        (Mode::Install, Status::Missing) => (Action::Install, format!("install {}", t.bundled_version)),
        (Mode::Install, Status::UpToDate) => (Action::None, "up to date".into()),
        (Mode::Install, Status::Outdated) => (Action::Update, format!("update {} → {}", t.installed_version.as_deref().unwrap_or("?"), t.bundled_version)),
        (Mode::Install, Status::Different) => (Action::Update, format!("replace with the bundled {} (the installed files differ)", t.bundled_version)),
        (Mode::Install, Status::Newer) if force => (Action::Update, format!("--force: replace the newer {} with {}", t.installed_version.as_deref().unwrap_or("?"), t.bundled_version)),
        (Mode::Install, Status::Newer) => (Action::Skip, format!("the installed {} is newer than this companion's {} (--force replaces it)", t.installed_version.as_deref().unwrap_or("?"), t.bundled_version)),
    };
    t.action = action;
    t.reason = reason;
    t.backup = matches!(action, Action::Update | Action::Remove).then(backup);
}

pub(crate) fn copy_dir(from: &Path, to: &Path) -> std::io::Result<u64> {
    copy_dir_but(from, to, &[])
}

/// copy_dir sans ces fichiers du premier niveau. (24 nov., relecture) Les sauvegardes ne gardent pas uwl_secret.py : un
/// code ne doit pas s'accumuler hors du dossier du script (apply le relit avant, et le remet).
fn copy_dir_but(from: &Path, to: &Path, skip: &[&str]) -> std::io::Result<u64> {
    std::fs::create_dir_all(to)?;
    let mut n = 0;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let p = e.path();
        if skip.iter().any(|s| e.file_name().to_str() == Some(*s)) {
            continue;
        }
        if e.file_type()?.is_dir() {
            n += copy_dir(&p, &to.join(e.file_name()))?;
        } else {
            std::fs::copy(&p, to.join(e.file_name()))?;
            n += 1;
        }
    }
    Ok(n)
}

fn count_files(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten().map(|e| if e.path().is_dir() { count_files(&e.path()) } else { 1 }).sum()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// « 20261024-093012 » (UTC) : le nom des copies de sauvegarde.
pub fn stamp(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    // jours depuis 1970 → date civile (algorithme de Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

pub fn now_stamp() -> String {
    stamp(now_secs())
}

/// Copie le dossier dans la sauvegarde, verifie la copie, puis retire le dossier.
fn back_up_and_clear(dir: &Path, backup: &Path) -> Result<String, String> {
    if backup.exists() {
        return Err(format!("the backup folder {} already exists", backup.display()));
    }
    let skip = [SECRET_FILE, SECRET_TMP];
    let n = copy_dir_but(dir, backup, &skip).map_err(|e| format!("cannot copy {} to {} ({e})", dir.display(), backup.display()))?;
    let skipped = skip.iter().filter(|s| dir.join(s).is_file()).count() as u64;
    if count_files(backup) + skipped != count_files(dir) {
        return Err(format!("the copy in {} is incomplete: nothing was changed", backup.display()));
    }
    let code = read_secret(dir);
    if let Err(e) = std::fs::remove_dir_all(dir) {
        // a moitie efface : on remet ce qui manque depuis la copie (et le code, que la copie n'a pas)
        let _ = copy_dir(backup, dir);
        if let Some(c) = &code {
            if read_secret(dir).as_deref() != Some(c.as_str()) {
                let _ = write_secret(dir, c);
            }
        }
        return Err(format!("cannot remove {} ({e}); its files were put back from {}", dir.display(), backup.display()));
    }
    Ok(format!("copied {} ({n} file{}) to {}", dir.display(), if n == 1 { "" } else { "s" }, backup.display()))
}

fn write_bundle(b: &Bundle, dir: &Path, backup: Option<&Path>) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {} ({e})", dir.display()))?;
    for (name, bytes) in b.files {
        let p = dir.join(name);
        std::fs::write(&p, bytes).map_err(|e| format!("cannot write {} ({e})", p.display()))?;
    }
    let marker = serde_json::json!({
        "script": b.folder, "version": b.version(), "installed_by": format!("uppwind-companion {}", env!("CARGO_PKG_VERSION")),
        "installed_at_unix": now_secs(), "files": b.files.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        "backup": backup.map(|p| p.display().to_string()),
    });
    std::fs::write(dir.join(MARKER), serde_json::to_string_pretty(&marker).unwrap()).map_err(|e| format!("cannot write {} ({e})", dir.join(MARKER).display()))?;
    for (name, bytes) in b.files {
        if std::fs::read(dir.join(name)).ok().as_deref() != Some(*bytes) {
            return Err(format!("{} does not read back as written", dir.join(name).display()));
        }
    }
    Ok(())
}

/// Fait ce que `plan` a decide → les lignes de ce qui a ete fait.
pub fn apply(t: &Target) -> Result<Vec<String>, String> {
    apply_with(t, &bundle(t.daw))
}

/// `apply` avec un autre jeu de fichiers (les essais : une ecriture qui echoue).
pub fn apply_with(t: &Target, b: &Bundle) -> Result<Vec<String>, String> {
    let mut did = Vec::new();
    // (24 nov.) le code du chemin MIDI d'avant : garde (le studio le connait deja), jamais remplace par une mise a jour
    let kept = read_secret(&t.dir);
    match t.action {
        Action::None if t.status == Status::UpToDate && kept.is_none() => {
            // un script a jour sans code (installe a la main, ou par un compagnon d'avant le 24 nov.) : on lui en donne un
            write_secret(&t.dir, &new_code()?)?;
            did.push(format!("gave {} its MIDI link code ({SECRET_FILE})", t.dir.display()));
        }
        Action::None | Action::Skip => {}
        Action::Remove => {
            let bk = t.backup.as_ref().ok_or("no backup folder planned")?;
            did.push(back_up_and_clear(&t.dir, bk)?);
            did.push(format!("removed {}", t.dir.display()));
        }
        Action::Install | Action::Update => {
            if let Some(bk) = &t.backup {
                if t.dir.exists() {
                    did.push(back_up_and_clear(&t.dir, bk)?);
                }
            }
            if let Err(e) = write_bundle(b, &t.dir, t.backup.as_deref()) {
                // retour a l'etat d'avant
                let _ = std::fs::remove_dir_all(&t.dir);
                let back = match &t.backup {
                    Some(bk) if bk.exists() => match copy_dir(bk, &t.dir) {
                        Ok(_) => {
                            // (relecture) la sauvegarde n'a pas le code : on le remet
                            if let Some(c) = &kept {
                                let _ = write_secret(&t.dir, c);
                            }
                            format!("; the previous folder was put back from {}", bk.display())
                        }
                        Err(e2) => format!("; putting the previous folder back failed too ({e2}): it is in {}", bk.display()),
                    },
                    _ => String::new(),
                };
                return Err(format!("{e}{back}"));
            }
            let code = match &kept {
                Some(c) => c.clone(),
                None => new_code()?,
            };
            write_secret(&t.dir, &code)?;
            did.push(format!("{} {} {} in {}", if t.action == Action::Install { "installed" } else { "updated" }, b.folder, b.version(), t.dir.display()));
        }
    }
    Ok(did)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_midi_code_is_the_scripts_own_byte_for_byte() {
        // les memes codes que link-protocol.js et uwlink_protocol.py (fixtures/auth.json)
        let fix: serde_json::Value = serde_json::from_str(include_str!("../../../fixtures/auth.json")).unwrap();
        let mut n = 0;
        for c in fix["cases"].as_array().unwrap() {
            match c["kind"].as_str().unwrap() {
                "code" => {
                    let hex = c["secret"].as_str().unwrap();
                    let mut b = [0u8; SECRET_BYTES];
                    for (i, x) in b.iter_mut().enumerate() {
                        *x = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
                    }
                    assert_eq!(code_of(&b), c["code"].as_str().unwrap());
                    assert_eq!(normalize_code(&c["code"].as_str().unwrap().to_lowercase()).as_deref(), c["code"].as_str());
                    n += 1;
                }
                "bad-code" => {
                    if let Some(s) = c["code"].as_str() {
                        assert_eq!(normalize_code(s), None, "{s}");
                    }
                }
                _ => {}
            }
        }
        assert!(n >= 4);
        let a = new_code().unwrap();
        assert_ne!(a, new_code().unwrap());
        assert_eq!(normalize_code(&a).as_deref(), Some(a.as_str()));
        assert_eq!(parse_secret(b"# x\nSECRET = \"mzxw 6ytb-oi2d-emrt-gq3d-knzw\"\n").as_deref(), Some("MZXW-6YTB-OI2D-EMRT-GQ3D-KNZW"));
        assert_eq!(parse_secret(b"SECRET = \"nope\"\n"), None);
        assert_eq!(parse_secret(b"\xff\xfe"), None);
    }

    #[test]
    fn the_scripts_are_embedded_with_their_version() {
        let a = bundle(Daw::Ableton);
        for f in ["__init__.py", "UppwindLink.py", "link_core.py", "notes_route.py", "uwlink_protocol.py", "README.md"] {
            assert!(a.files.iter().any(|(n, b)| *n == f && !b.is_empty()), "{f} is embedded");
        }
        assert!(!a.files.iter().any(|(n, _)| n.ends_with(".pyc")));
        let fl = bundle(Daw::Fl);
        for f in ["device_UppwindLink.py", "uwlink_protocol.py", "README.md"] {
            assert!(fl.files.iter().any(|(n, _)| *n == f), "{f} is embedded");
        }
        assert!(a.version().chars().next().unwrap().is_ascii_digit(), "{}", a.version());
        assert!(fl.version().chars().next().unwrap().is_ascii_digit(), "{}", fl.version());
        // les deux copies du protocole sont identiques (build-link-zip.mjs le verifie aussi)
        let p = |b: &Bundle| b.files.iter().find(|(n, _)| *n == "uwlink_protocol.py").unwrap().1;
        assert_eq!(p(&a), p(&fl));
    }

    #[test]
    fn versions_and_stamps() {
        assert_eq!(version_in(b"x = 1\nSCRIPT_VERSION = \"1.2.3\"\n").as_deref(), Some("1.2.3"));
        assert_eq!(version_in(b"nothing"), None);
        assert_eq!(cmp_versions("1.0.10", "1.0.9"), std::cmp::Ordering::Greater);
        assert_eq!(cmp_versions("1.0", "1.0.0"), std::cmp::Ordering::Less);
        assert_eq!(cmp_versions("12.4.3", "12.4.3"), std::cmp::Ordering::Equal);
        assert_eq!(stamp(0), "19700101-000000");
        assert_eq!(stamp(1_792_834_212), "20261024-093012");
        assert_eq!(stamp(951_782_400), "20000229-000000");
    }
}
