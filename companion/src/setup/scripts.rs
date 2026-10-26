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
//!   - un fichier .uppwind-link-install.json dans le dossier installe dit quelle version le compagnon y a mise.

use super::Mode;
use serde::Serialize;
use std::path::{Path, PathBuf};

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded_scripts.rs"));
}

pub const MARKER: &str = ".uppwind-link-install.json";

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
            .filter(|n| n != MARKER && !n.ends_with(".pyc") && !b.files.iter().any(|(f, _)| f == n))
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
    std::fs::create_dir_all(to)?;
    let mut n = 0;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let p = e.path();
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
    let n = copy_dir(dir, backup).map_err(|e| format!("cannot copy {} to {} ({e})", dir.display(), backup.display()))?;
    if count_files(backup) != count_files(dir) {
        return Err(format!("the copy in {} is incomplete: nothing was changed", backup.display()));
    }
    if let Err(e) = std::fs::remove_dir_all(dir) {
        // a moitie efface : on remet ce qui manque depuis la copie
        let _ = copy_dir(backup, dir);
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
    match t.action {
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
                        Ok(_) => format!("; the previous folder was put back from {}", bk.display()),
                        Err(e2) => format!("; putting the previous folder back failed too ({e2}): it is in {}", bk.display()),
                    },
                    _ => String::new(),
                };
                return Err(format!("{e}{back}"));
            }
            did.push(format!("{} {} {} in {}", if t.action == Action::Install { "installed" } else { "updated" }, b.folder, b.version(), t.dir.display()));
        }
    }
    Ok(did)
}

#[cfg(test)]
mod tests {
    use super::*;

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
