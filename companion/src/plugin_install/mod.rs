// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (29 oct.) Les plugins Uppwind Link (« Uppwind Link.vst3 » : Uppwind Link Return et Uppwind Link Notes), installes
//! par le compagnon lui-meme.
//!
//! Le bundle est DANS l'exe (build.rs, UWL_PLUGIN_BUNDLE a la construction) : le compagnon ne telecharge jamais rien.
//!   Windows : %CommonProgramFiles%\VST3\Uppwind Link.vst3, le seul dossier VST3 que FL Studio parcourt (Live aussi le
//!             lit). Y ecrire demande l'administrateur : le compagnon se relance lui-meme, eleve, pour la copie seulement
//!             (elevate.rs : ShellExecuteExW « runas », UNE invite, un drapeau cache, le code de sortie). Rien d'autre ne
//!             tourne eleve. Le dossier vient du dossier connu de Windows (FOLDERID_ProgramFilesCommon), jamais d'une
//!             variable d'environnement.
//!   macOS   : ~/Library/Audio/Plug-Ins/VST3/Uppwind Link.vst3, sans administrateur. Les fichiers sont ceux du bundle
//!             signe, octet pour octet (sa signature reste valide : `codesign --verify` sur le resultat), le binaire en
//!             0755, le reste en 0644. Sur un Mac, Return est le seul chemin du son du DAW vers le studio sans cable.
//!   Linux   : pas de plugins (le compagnon s'y construit, c'est tout).
//!
//! Un DAW garde la DLL chargee : sous Windows, elle ne peut ni etre ecrasee ni effacee, mais elle peut etre renommee.
//! Le fichier tenu est mis de cote (« … .old-N »), le neuf ecrit a sa place, et les restes effaces des qu'ils sont
//! libres (au prochain passage, et au prochain demarrage de Windows quand la copie est elevee) ; il faut relancer le
//! DAW. Sous macOS, chaque fichier est ecrit a cote puis renomme par-dessus : un DAW qui tourne garde l'ancien.
//!
//! L'etat compare les octets embarques a ceux du dossier : missing, up-to-date, outdated, different, newer. Le releve
//! « plugins-installed.json » (dans le dossier du compagnon, PAS dans le bundle : un fichier de plus y casserait la
//! signature macOS) dit quelle version du compagnon les a mis et l'empreinte de chaque fichier : un bundle intact d'une
//! version plus ancienne est « outdated », d'une plus recente « newer » (laisse, sauf --force), le reste « different ».
//! La desinstallation ne retire que ce que le compagnon a mis (le releve), ou un bundle identique au sien.
//!
//! Securite : jamais de lien suivi dans le bundle (un lien ou une jonction y est refuse, ou retire lui-meme, jamais sa
//! cible) ; on n'ecrit et n'efface que sous « Uppwind Link.vst3 » ; la copie elevee ne lit ni chemin ni variable.
//!
//!   uppwind-companion --install-plugins [--dry-run] [--remove] [--force]
//!   --setup (setup/mod.rs) : le rapport, et --apply (--skip-plugins les laisse)
//!   la fenetre d'installation (wizard/) : « Uppwind Link plugins (VST3) », coche par defaut ; --uninstall les retire
//!   --plugins-dir <dossier> (cache, pour les essais) : ce dossier au lieu de celui du systeme, jamais d'elevation

pub mod elevate;
pub mod sha256;

use crate::setup::env::Paths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded_plugins.rs"));
}

/// Le dossier du bundle, dans le dossier VST3.
pub const BUNDLE_NAME: &str = "Uppwind Link.vst3";
/// Le releve de ce que le compagnon a installe (dans son dossier).
pub const MARKER_NAME: &str = "plugins-installed.json";
pub const INSTALL_FLAG: &str = "--install-plugins";
/// Cache, pour les essais : le dossier VST3 a utiliser (jamais celui du systeme, jamais d'elevation).
pub const DIR_FLAG: &str = "--plugins-dir";
/// --setup : sans les plugins.
pub const SKIP_FLAG: &str = "--skip-plugins";
/// L'identifiant des plugins dans les choix de la fenetre d'installation (Choices::daws : les DAW coches, et lui).
pub const CHOICE_ID: &str = "plugins";

pub const HELP: &str = "uppwind-companion --install-plugins [--dry-run] [--remove] [--force]\n\
  the Uppwind Link plugins (VST3: Uppwind Link Return, Uppwind Link Notes) that this program carries. Windows: into\n\
  C:\\Program Files\\Common Files\\VST3, where FL Studio and Live look for them (Windows asks for administrator rights\n\
  once, for the copy only); macOS: into ~/Library/Audio/Plug-Ins/VST3. --dry-run says what it would do; --remove\n\
  takes them out again (only plugins the companion installed); --force replaces plugins of a newer companion.";

/// Le bundle a installer : (chemin dans le bundle avec des « / », octets, executable sous macOS), tries.
#[derive(Clone, Copy)]
pub struct Bundle {
    pub files: &'static [(&'static str, &'static [u8], bool)],
}

impl Bundle {
    /// Celui de cet exe (vide : une construction sans UWL_PLUGIN_BUNDLE).
    pub fn embedded() -> Bundle {
        Bundle { files: embedded::PLUGIN_FILES }
    }
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
    fn has(&self, rel: &str) -> bool {
        self.files.iter().any(|(n, _, _)| *n == rel)
    }
    pub fn size(&self) -> u64 {
        self.files.iter().map(|(_, b, _)| b.len() as u64).sum()
    }
    /// Un dossier qui mene a un fichier du bundle (« Contents », « Contents/MacOS ») : il reste.
    fn needs_dir(&self, rel: &str) -> bool {
        self.files.iter().any(|(n, _, _)| n.len() > rel.len() && n.starts_with(rel) && n.as_bytes()[rel.len()] == b'/')
    }
}

/// Les notices des crates des plugins (THIRD-PARTY-NOTICES.txt de plugin/), embarquees avec eux ; vide sans plugins.
pub fn notices() -> &'static str {
    embedded::PLUGIN_NOTICES
}

/// Les plugins ont un sens ici (Windows, macOS).
pub fn supported() -> bool {
    cfg!(any(windows, target_os = "macos"))
}

static DIR_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// --plugins-dir, s'il a ete donne (lu une fois par `early`).
pub fn dir_override() -> Option<&'static Path> {
    DIR_OVERRIDE.get().map(|p| p.as_path())
}

/// Ou vont les plugins.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Target {
    /// le dossier VST3 : le bundle y va, sous le nom « Uppwind Link.vst3 »
    pub vst3: PathBuf,
    /// le releve de ce que le compagnon y a installe
    pub marker: PathBuf,
    /// Windows : le vrai dossier du systeme (y ecrire demande l'administrateur)
    pub system: bool,
    /// un essai de l'installeur (--sandbox) : rien n'est ecrit, les etapes sont dites
    pub simulate: bool,
}

impl Target {
    pub fn bundle(&self) -> PathBuf {
        self.vst3.join(BUNDLE_NAME)
    }
    /// Un dossier d'essai (--plugins-dir, les tests) : ni le systeme, ni une simulation ; le releve a cote du bundle.
    pub fn in_dir(vst3: &Path) -> Target {
        Target { vst3: vst3.to_path_buf(), marker: vst3.join(MARKER_NAME), system: false, simulate: false }
    }
}

fn mac_vst3(home: &Path) -> PathBuf {
    home.join("Library").join("Audio").join("Plug-Ins").join("VST3")
}

/// Le dossier VST3 du systeme : Windows, le dossier connu « Common Files » (C:\Program Files\Common Files) + VST3 ;
/// macOS, ~/Library/Audio/Plug-Ins/VST3 ; ailleurs, ~/.vst3 (jamais utilise : `supported()`).
pub fn system_vst3(home: &Path) -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        let _ = home;
        Ok(winfs::common_files()?.join("VST3"))
    }
    #[cfg(not(windows))]
    {
        Ok(if cfg!(target_os = "macos") { mac_vst3(home) } else { home.join(".vst3") })
    }
}

/// Ou vont les plugins : --plugins-dir (les essais), le dossier de la fausse machine (UWC_SETUP_ROOT), sinon celui du
/// systeme. `simulate` : un essai de l'installeur (--sandbox), rien n'est ecrit.
pub fn target_for(paths: &Paths, fake: bool, simulate: bool) -> Result<Target, String> {
    if let Some(d) = dir_override() {
        return Ok(Target::in_dir(d));
    }
    let marker = paths.local.join(crate::install::APP_DIR).join(MARKER_NAME);
    if fake {
        let vst3 = if cfg!(target_os = "macos") { mac_vst3(&paths.home) } else { paths.home.join("Program Files").join("Common Files").join("VST3") };
        return Ok(Target { vst3, marker, system: false, simulate: false });
    }
    Ok(Target { vst3: system_vst3(&paths.home)?, marker, system: cfg!(windows), simulate })
}

/// Les dossiers de cette machine, ou ceux de la fausse de UWC_SETUP_ROOT (true), sans lire le reste de la machine.
pub fn env_paths() -> Result<(Paths, bool), String> {
    if let Some(root) = std::env::var_os(crate::setup::env::ROOT_ENV).filter(|v| !v.is_empty()) {
        return Ok((Paths::in_root(Path::new(&root)), true));
    }
    let p = if cfg!(target_os = "macos") { Paths::from_lookup_macos(|k| std::env::var_os(k))? } else { Paths::from_lookup(|k| std::env::var_os(k), || None)? };
    Ok((p, false))
}

// ── l'etat ────────────────────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// pas de bundle
    Missing,
    /// les fichiers de ce compagnon, octet pour octet, et rien d'autre
    UpToDate,
    /// ceux, intacts, qu'une version plus ancienne du compagnon a mis
    Outdated,
    /// d'autres octets (une construction a la main, une copie modifiee, ou rien a quoi les comparer)
    Different,
    /// ceux, intacts, qu'une version plus recente du compagnon a mis
    Newer,
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

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileState {
    /// le chemin dans le bundle
    pub name: String,
    /// same, differs, missing, extra
    pub state: &'static str,
}

/// Les plugins : ce qui est la, ce que l'on y ferait.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    /// Windows ou macOS
    pub supported: bool,
    /// cet exe porte les plugins (UWL_PLUGIN_BUNDLE a la construction)
    pub embedded: bool,
    /// le dossier du bundle (…\VST3\Uppwind Link.vst3)
    pub folder: PathBuf,
    pub status: Status,
    pub files: Vec<FileState>,
    /// la version du compagnon qui les a installes (le releve) ; None : pas par le compagnon
    pub installed_by: Option<String>,
    /// d'anciens fichiers mis de cote parce qu'un DAW les tenait (effaces des qu'ils sont libres)
    pub held: Vec<PathBuf>,
    pub action: Action,
    pub reason: String,
    /// Windows, le dossier du systeme : Windows demande les droits d'administrateur (une invite)
    pub admin: bool,
    /// un essai : rien n'est ecrit
    pub simulated: bool,
    /// apres --apply : l'etat relu
    pub after: Option<Status>,
}

/// Le releve, dans le dossier du compagnon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Marker {
    bundle: String,
    version: String,
    installed_at_unix: u64,
    /// chemin dans le bundle → SHA-256
    files: BTreeMap<String, String>,
}

fn read_marker(t: &Target) -> Option<Marker> {
    let raw = std::fs::read_to_string(&t.marker).ok()?;
    let m: Marker = serde_json::from_str(&raw).ok()?;
    crate::setup::same_path(Path::new(&m.bundle), &t.bundle()).then_some(m)
}

fn write_marker(t: &Target, b: &Bundle) -> Result<(), String> {
    let m = Marker {
        bundle: t.bundle().display().to_string(),
        version: env!("CARGO_PKG_VERSION").into(),
        installed_at_unix: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        files: b.files.iter().map(|(n, bytes, _)| (n.to_string(), sha256::hex(bytes))).collect(),
    };
    if let Some(d) = t.marker.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("cannot create {} ({e})", d.display()))?;
    }
    std::fs::write(&t.marker, serde_json::to_string_pretty(&m).unwrap()).map_err(|e| format!("cannot write {} ({e})", t.marker.display()))
}

fn remove_marker(t: &Target) {
    if read_marker(t).is_some() {
        let _ = std::fs::remove_file(&t.marker);
    }
}

/// Un lien, une jonction ou tout autre point d'analyse (Windows) : jamais suivi.
fn is_link(m: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        m.file_type().is_symlink() || m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        m.file_type().is_symlink()
    }
}

/// Le chemin d'un fichier du bundle (« Contents/MacOS/Uppwind Link ») sous `dir`.
fn path_in(dir: &Path, rel: &str) -> PathBuf {
    rel.split('/').filter(|c| !c.is_empty() && *c != "." && *c != "..").fold(dir.to_path_buf(), |p, c| p.join(c))
}

fn rel_of(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("/")).unwrap_or_default()
}

/// Les restes d'une installation : « … .old-3 » (un fichier qu'un DAW tenait), « … .uwl-new-1234 » (une ecriture
/// interrompue).
fn is_leftover(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    [".old-", ".uwl-new-"].iter().any(|m| {
        name.rfind(m).is_some_and(|i| {
            let tail = &name[i + m.len()..];
            i > 0 && !tail.is_empty() && tail.bytes().all(|c| c.is_ascii_digit())
        })
    })
}

/// Les fichiers (et les liens) sous `dir`, sans jamais suivre un lien : (chemin dans le bundle, chemin).
fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    v.sort();
    for p in v {
        let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
        if m.is_dir() && !is_link(&m) {
            walk(root, &p, out);
        } else {
            out.push((rel_of(root, &p), p));
        }
    }
}

/// Les dossiers (vrais) sous `dir`, les plus profonds d'abord.
fn dirs_deepest_first(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir() && !is_link(&m)) {
            dirs_deepest_first(&p, out);
            out.push(p);
        }
    }
}

/// macOS : le binaire doit etre executable ; le reste, et Windows : toujours bon.
fn exec_ok(p: &Path, exec: bool) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        !exec || std::fs::metadata(p).is_ok_and(|m| m.permissions().mode() & 0o100 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = (p, exec);
        true
    }
}

/// Ce qu'il y a dans le dossier, compare au bundle `b` (rien n'est ecrit).
pub fn inspect(b: &Bundle, t: &Target) -> Report {
    let dir = t.bundle();
    let mut r = Report {
        supported: supported(),
        embedded: !b.is_empty(),
        folder: dir.clone(),
        status: Status::Missing,
        files: vec![],
        installed_by: None,
        held: vec![],
        action: Action::None,
        reason: String::new(),
        admin: cfg!(windows) && t.system,
        simulated: t.simulate,
        after: None,
    };
    if !r.supported {
        return r;
    }
    let marker = read_marker(t);
    r.installed_by = marker.as_ref().map(|m| m.version.clone());
    let Ok(meta) = std::fs::symlink_metadata(&dir) else { return r };
    if !meta.is_dir() || is_link(&meta) {
        r.status = Status::Different;
        r.files.push(FileState { name: String::new(), state: "not-a-folder" });
        return r;
    }
    let mut all_same = !b.is_empty();
    for (rel, bytes, exec) in b.files {
        let p = path_in(&dir, rel);
        let state = match std::fs::read(&p) {
            Ok(have) if have == *bytes && exec_ok(&p, *exec) => "same",
            Ok(_) => "differs",
            Err(_) => "missing",
        };
        all_same &= state == "same";
        r.files.push(FileState { name: rel.to_string(), state });
    }
    let mut found = Vec::new();
    walk(&dir, &dir, &mut found);
    for (rel, p) in &found {
        if b.has(rel) {
            continue;
        }
        if is_leftover(rel) {
            r.held.push(p.clone());
            continue;
        }
        all_same = false;
        r.files.push(FileState { name: rel.clone(), state: "extra" });
    }
    r.status = if all_same {
        Status::UpToDate
    } else {
        // le releve dit-il ce qui est la ? (les empreintes, lues seulement ici)
        match &marker {
            Some(m) if !b.is_empty() => {
                let installed: BTreeMap<String, String> =
                    found.iter().filter(|(rel, _)| !is_leftover(rel)).filter_map(|(rel, p)| std::fs::read(p).ok().map(|x| (rel.clone(), sha256::hex(&x)))).collect();
                if installed == m.files {
                    match crate::setup::cmp_versions(&m.version, env!("CARGO_PKG_VERSION")) {
                        std::cmp::Ordering::Less => Status::Outdated,
                        std::cmp::Ordering::Greater => Status::Newer,
                        std::cmp::Ordering::Equal => Status::Different,
                    }
                } else {
                    Status::Different
                }
            }
            _ => Status::Different,
        }
    };
    r
}

/// Pas de dossier VST3 (Windows n'a pas donne le sien) : rien a regarder, rien a faire, et pourquoi.
pub fn unavailable(b: &Bundle, why: String) -> Report {
    Report {
        supported: false,
        embedded: !b.is_empty(),
        folder: PathBuf::new(),
        status: Status::Missing,
        files: vec![],
        installed_by: None,
        held: vec![],
        action: Action::Skip,
        reason: why,
        admin: false,
        simulated: false,
        after: None,
    }
}

/// Ce que l'on ferait : installer (`remove` false) ou retirer.
pub fn plan(r: &mut Report, remove: bool, force: bool) {
    let v = env!("CARGO_PKG_VERSION");
    let by = r.installed_by.clone().unwrap_or_else(|| "?".into());
    let (a, why): (Action, String) = if !r.supported {
        (Action::Skip, "the Uppwind Link plugins are for Windows and macOS".into())
    } else if remove {
        match r.status {
            Status::Missing => (Action::None, "not installed".into()),
            _ if r.installed_by.is_some() || r.status == Status::UpToDate => (Action::Remove, "remove them".into()),
            _ => (Action::Skip, "not installed by Uppwind Link: left as they are".into()),
        }
    } else if !r.embedded {
        (Action::Skip, "not in this build of the companion (a development build: the released one carries them)".into())
    } else {
        match r.status {
            Status::Missing => (Action::Install, "install them".into()),
            Status::UpToDate => (Action::None, "up to date".into()),
            Status::Outdated => (Action::Update, format!("update them (installed by companion {by}, this is {v})")),
            Status::Different => (Action::Update, "replace them with this companion's (the installed files differ)".into()),
            Status::Newer if force => (Action::Update, format!("--force: replace the plugins of companion {by} with those of {v}")),
            Status::Newer => (Action::Skip, format!("installed by a newer companion ({by}): left as they are (--force replaces them)")),
        }
    };
    r.action = a;
    r.reason = why;
}

/// Ce que `apply` a fait.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Applied {
    pub done: Vec<String>,
    pub errors: Vec<String>,
    /// Windows : l'invite d'administrateur refusee ; rien n'a change
    pub refused: bool,
    /// un DAW garde les anciens plugins charges : le relancer
    pub restart: bool,
    /// la copie s'est faite avec les droits d'administrateur (une invite)
    pub elevated: bool,
}

impl Applied {
    /// Quelque chose a ete ecrit ou retire, sans erreur ni invite refusee.
    pub fn changed(&self) -> bool {
        !self.done.is_empty() && self.errors.is_empty() && !self.refused
    }
}

/// Fait ce que `plan` a decide : dans ce processus si l'on peut ecrire la, sinon (Windows, le dossier du systeme) par
/// la copie elevee. Puis relit le dossier et tient le releve a jour. `progress` : une ligne avant l'invite de Windows.
pub fn apply(r: &mut Report, b: &Bundle, t: &Target, progress: &dyn Fn(&str)) -> Applied {
    apply_with(r, b, t, progress, &elevate::run)
}

/// `apply`, avec la copie elevee donnee (les essais en donnent une fausse : jamais d'invite).
pub(crate) fn apply_with(r: &mut Report, b: &Bundle, t: &Target, progress: &dyn Fn(&str), elevated: &dyn Fn(&str) -> elevate::Outcome) -> Applied {
    let mut a = Applied::default();
    let dir = t.bundle();
    let shown = crate::shown_path(&dir);
    let remove = match r.action {
        Action::None if !r.held.is_empty() && !t.simulate => {
            // a jour, avec les restes d'avant qu'un DAW tenait : effaces s'ils sont libres, sans invite (la copie elevee
            // les a deja confies au prochain demarrage de Windows)
            let n = sweep(&dir);
            r.held = inspect(b, t).held;
            if n > 0 {
                a.done.push(format!("Removed {n} older plugin file{} that a DAW no longer holds.", if n == 1 { "" } else { "s" }));
            }
            return a;
        }
        Action::None | Action::Skip => return a,
        Action::Remove => true,
        Action::Install | Action::Update => false,
    };
    if t.simulate {
        a.done.push(format!("Uppwind Link plugins: {} in {shown} (simulated: nothing written).", r.reason));
        return a;
    }
    let local = if remove { remove_bundle(&dir).map(|x| x.held) } else { write_bundle(b, &dir).map(|x| x.held) };
    let res: Result<bool, String> = match local {
        Ok(held) => Ok(!held.is_empty()),
        Err(WriteError::Denied(why)) if t.system && cfg!(windows) => {
            progress(if remove {
                "Removing the Uppwind Link plugins: Windows asks for administrator rights"
            } else {
                "Installing the Uppwind Link plugins: Windows asks for administrator rights"
            });
            a.elevated = true;
            match elevated(if remove { "uninstall" } else { "install" }) {
                elevate::Outcome::Exit(elevate::OK) => Ok(false),
                elevate::Outcome::Exit(elevate::HELD) => Ok(true),
                elevate::Outcome::Exit(code) => Err(format!("the copy with administrator rights did not work (exit code {code}); without them: {why}")),
                elevate::Outcome::Refused => {
                    a.refused = true;
                    Err(String::new())
                }
                elevate::Outcome::Failed(e) => Err(e),
            }
        }
        Err(e) => Err(e.text().to_string()),
    };
    let again = inspect(b, t);
    r.after = Some(again.status);
    r.held = again.held.clone();
    match res {
        Err(_) if a.refused => a.done.push(if remove {
            format!("The administrator prompt was refused: the Uppwind Link plugins stay in {shown}.")
        } else {
            format!("The administrator prompt was refused: the Uppwind Link plugins were not installed ({shown}).")
        }),
        Err(e) => a.errors.push(format!("Uppwind Link plugins: {e}")),
        Ok(held) if !remove => {
            if again.status != Status::UpToDate {
                a.errors.push(format!("Uppwind Link plugins: {shown} does not hold this companion's plugins after the copy ({:?})", again.status));
            } else {
                let verb = if r.action == Action::Install { "installed" } else { "updated" };
                a.done.push(format!("Uppwind Link plugins (Uppwind Link Return, Uppwind Link Notes) {verb} in {shown}{}.", if a.elevated { " (with administrator rights)" } else { "" }));
                if let Err(e) = write_marker(t, b) {
                    a.errors.push(format!("Uppwind Link plugins: {e}"));
                }
                if held {
                    a.restart = true;
                    a.done.push("A DAW still has the previous Uppwind Link plugins loaded: restart it to use the new ones (the old file goes once it is free).".into());
                }
            }
        }
        Ok(held) => {
            let gone = std::fs::symlink_metadata(&dir).is_err();
            if gone {
                remove_marker(t);
                a.done.push(format!("Removed the Uppwind Link plugins from {shown}{}.", if a.elevated { " (with administrator rights)" } else { "" }));
            } else if held || !again.held.is_empty() {
                a.restart = true;
                a.done.push(format!(
                    "Removed the Uppwind Link plugins from {shown}, except a file a DAW still has loaded: close the DAW{}.",
                    if a.elevated { "; it goes at the next restart of Windows" } else { ", then delete the folder" }
                ));
            } else {
                a.errors.push(format!("Uppwind Link plugins: {shown} is still there"));
            }
        }
    }
    a
}

// ── l'ecriture ───────────────────────────────────────────────────────────────────────────────────────────

/// Ce que l'ecriture du bundle a fait.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Written {
    pub written: usize,
    pub same: usize,
    /// des fichiers en trop retires
    pub removed: usize,
    /// d'anciens fichiers mis de cote, qu'un DAW tient encore
    pub held: Vec<PathBuf>,
}

/// Ce que le retrait du bundle a fait.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Removed {
    pub gone: bool,
    pub held: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WriteError {
    /// pas le droit d'ecrire la (Windows : le dossier du systeme sans administrateur)
    Denied(String),
    Failed(String),
}

impl WriteError {
    pub fn text(&self) -> &str {
        match self {
            WriteError::Denied(s) | WriteError::Failed(s) => s,
        }
    }
}

fn io_err(what: &str, p: &Path, e: std::io::Error) -> WriteError {
    let s = format!("cannot {what} {} ({e})", p.display());
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        WriteError::Denied(s)
    } else {
        WriteError::Failed(s)
    }
}

/// Un dossier du bundle : un vrai dossier (jamais un lien ni une jonction), cree s'il manque.
fn ensure_dir(p: &Path) -> Result<(), WriteError> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() && !is_link(&m) => Ok(()),
        Ok(_) => Err(WriteError::Failed(format!("{} is not a plain folder (a link, a junction or a file): refused", p.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(p).map_err(|e| io_err("create", p, e)),
        Err(e) => Err(io_err("read", p, e)),
    }
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut name = p.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(suffix);
    p.with_file_name(name)
}

/// Le premier « <nom>.old-N » libre a cote de `p`.
fn free_aside(p: &Path) -> Option<PathBuf> {
    (1..=99).map(|n| with_suffix(p, &format!(".old-{n}"))).find(|q| std::fs::symlink_metadata(q).is_err())
}

/// Un fichier neuf (jamais un existant : create_new), ecrit et pousse sur le disque ; macOS : 0755 ou 0644.
fn write_new(p: &Path, bytes: &[u8], exec: bool) -> Result<(), WriteError> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(if exec { 0o755 } else { 0o644 });
    }
    let mut f = o.open(p).map_err(|e| io_err("write", p, e))?;
    if let Err(e) = f.write_all(bytes).and_then(|_| f.sync_all()) {
        drop(f);
        let _ = std::fs::remove_file(p);
        return Err(io_err("write", p, e));
    }
    drop(f);
    #[cfg(unix)]
    {
        // (le umask a pu retirer des bits a la creation)
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(if exec { 0o755 } else { 0o644 })).map_err(|e| io_err("set the mode of", p, e))?;
    }
    let _ = exec;
    Ok(())
}

/// Met `bytes` a la place de `dest` : ecrit a cote, puis renomme par-dessus ; si un DAW tient `dest` (Windows : une DLL
/// chargee ne se remplace pas mais se renomme), `dest` passe d'abord de cote (« .old-N », dans `held`).
fn replace(dest: &Path, bytes: &[u8], exec: bool, held: &mut Vec<PathBuf>) -> Result<(), WriteError> {
    let tmp = with_suffix(dest, &format!(".uwl-new-{}", std::process::id()));
    if std::fs::symlink_metadata(&tmp).is_ok() {
        let _ = std::fs::remove_file(&tmp);
    }
    write_new(&tmp, bytes, exec)?;
    let first = match std::fs::rename(&tmp, dest) {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    if std::fs::symlink_metadata(dest).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return Err(io_err("write", dest, first));
    }
    let Some(aside) = free_aside(dest) else {
        let _ = std::fs::remove_file(&tmp);
        return Err(WriteError::Failed(format!("cannot replace {} ({first}), and 99 older copies are already set aside next to it", dest.display())));
    };
    if let Err(e) = std::fs::rename(dest, &aside) {
        let _ = std::fs::remove_file(&tmp);
        return Err(io_err("replace", dest, e));
    }
    if let Err(e) = std::fs::rename(&tmp, dest) {
        // l'ancien reprend sa place
        let _ = std::fs::rename(&aside, dest);
        let _ = std::fs::remove_file(&tmp);
        return Err(io_err("write", dest, e));
    }
    held.push(aside);
    Ok(())
}

/// Retire un fichier ou un lien (un lien seulement, jamais ce qu'il designe).
fn remove_entry(p: &Path) -> std::io::Result<()> {
    let m = std::fs::symlink_metadata(p)?;
    if is_link(&m) && m.file_type().is_dir() {
        return std::fs::remove_dir(p);
    }
    #[cfg(windows)]
    {
        // une jonction ou un lien vers un dossier : FILE_ATTRIBUTE_DIRECTORY
        use std::os::windows::fs::MetadataExt;
        if is_link(&m) && m.file_attributes() & 0x10 != 0 {
            return std::fs::remove_dir(p);
        }
    }
    std::fs::remove_file(p)
}

/// Ecrit le bundle `b` dans `dir` (« …\VST3\Uppwind Link.vst3 ») : les fichiers qui different, ce qui est en trop retire
/// (macOS : un fichier de plus casserait la signature), les restes d'avant effaces s'ils sont libres, puis relu.
pub fn write_bundle(b: &Bundle, dir: &Path) -> Result<Written, WriteError> {
    if b.is_empty() {
        return Err(WriteError::Failed("this companion carries no plugins (a build without UWL_PLUGIN_BUNDLE)".into()));
    }
    let vst3 = dir.parent().ok_or_else(|| WriteError::Failed(format!("{}: no parent folder", dir.display())))?;
    std::fs::create_dir_all(vst3).map_err(|e| io_err("create", vst3, e))?;
    ensure_dir(dir)?;
    let mut w = Written::default();
    for (rel, bytes, exec) in b.files {
        let parts: Vec<&str> = rel.split('/').collect();
        let (name, sub) = parts.split_last().expect("a path in the bundle");
        let mut folder = dir.to_path_buf();
        for c in sub {
            folder.push(c);
            ensure_dir(&folder)?;
        }
        let dest = folder.join(name);
        match std::fs::symlink_metadata(&dest) {
            Ok(m) if is_link(&m) || !m.is_file() => return Err(WriteError::Failed(format!("{} is not a regular file (a link or a folder): refused", dest.display()))),
            Ok(_) => {
                if std::fs::read(&dest).ok().as_deref() == Some(*bytes) && exec_ok(&dest, *exec) {
                    w.same += 1;
                    continue;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_err("read", &dest, e)),
        }
        replace(&dest, bytes, *exec, &mut w.held)?;
        w.written += 1;
    }
    w.removed = prune(b, dir, &mut w.held)?;
    for (rel, bytes, _) in b.files {
        let p = path_in(dir, rel);
        if std::fs::read(&p).ok().as_deref() != Some(*bytes) {
            return Err(WriteError::Failed(format!("{} does not read back as written", p.display())));
        }
    }
    Ok(w)
}

/// Retire de `dir` tout ce qui n'est pas dans `b` : les fichiers en trop, les liens (eux seuls), les dossiers vides en
/// trop ; les restes d'avant s'ils sont libres (sinon : `held`). → combien de fichiers en trop sont partis.
fn prune(b: &Bundle, dir: &Path, held: &mut Vec<PathBuf>) -> Result<usize, WriteError> {
    let mut found = Vec::new();
    walk(dir, dir, &mut found);
    let mut n = 0;
    for (rel, p) in found {
        if b.has(&rel) {
            continue;
        }
        let leftover = is_leftover(&rel);
        match remove_entry(&p) {
            Ok(()) => {
                held.retain(|h| h != &p);
                if !leftover {
                    n += 1;
                }
            }
            Err(_) if leftover => {
                if !held.contains(&p) {
                    held.push(p);
                }
            }
            Err(e) => match free_aside(&p) {
                // un fichier en trop qu'un DAW tient (une autre disposition d'avant) : de cote, comme le binaire
                Some(aside) if std::fs::rename(&p, &aside).is_ok() => held.push(aside),
                _ => return Err(io_err("remove", &p, e)),
            },
        }
    }
    let mut dirs = Vec::new();
    dirs_deepest_first(dir, &mut dirs);
    for d in dirs {
        if !b.needs_dir(&rel_of(dir, &d)) {
            let _ = std::fs::remove_dir(&d);
        }
    }
    Ok(n)
}

/// Les restes d'avant (« .old-N », « .uwl-new-N ») qui sont libres : effaces → combien.
fn sweep(dir: &Path) -> usize {
    let mut found = Vec::new();
    walk(dir, dir, &mut found);
    found.iter().filter(|(rel, p)| is_leftover(rel) && remove_entry(p).is_ok()).count()
}

/// Retire le bundle `dir` ; un fichier qu'un DAW tient passe de cote (« .old-N ») et reste (`held`).
pub fn remove_bundle(dir: &Path) -> Result<Removed, WriteError> {
    let m = match std::fs::symlink_metadata(dir) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Removed { gone: true, held: vec![] }),
        Err(e) => return Err(io_err("read", dir, e)),
    };
    if is_link(&m) {
        remove_entry(dir).map_err(|e| io_err("remove", dir, e))?;
        return Ok(Removed { gone: true, held: vec![] });
    }
    if !m.is_dir() {
        return Err(WriteError::Failed(format!("{} is not a folder: left as it is", dir.display())));
    }
    if std::fs::remove_dir_all(dir).is_ok() {
        return Ok(Removed { gone: true, held: vec![] });
    }
    let mut found = Vec::new();
    walk(dir, dir, &mut found);
    let mut held = Vec::new();
    for (rel, p) in found {
        let Err(e) = remove_entry(&p) else { continue };
        if is_leftover(&rel) {
            held.push(p);
            continue;
        }
        match free_aside(&p) {
            Some(aside) if std::fs::rename(&p, &aside).is_ok() => held.push(aside),
            _ => return Err(io_err("remove", &p, e)),
        }
    }
    let mut dirs = Vec::new();
    dirs_deepest_first(dir, &mut dirs);
    dirs.push(dir.to_path_buf());
    for d in dirs {
        let _ = std::fs::remove_dir(&d);
    }
    Ok(Removed { gone: std::fs::symlink_metadata(dir).is_err(), held })
}

// ── la copie elevee (Windows) ────────────────────────────────────────────────────────────────────────────

/// Ce que fait la copie elevee (elevate.rs) : le bundle de cet exe dans le dossier du systeme, rien d'autre. Les
/// fichiers tenus par un DAW partent au prochain demarrage de Windows (MoveFileEx, possible en administrateur).
#[cfg(windows)]
pub(crate) fn elevated_job(op: &str) -> i32 {
    // (les essais n'ecrivent jamais dans le dossier du systeme)
    if cfg!(test) {
        return elevate::FAILED;
    }
    let dir = match winfs::common_files() {
        Ok(c) => c.join("VST3").join(BUNDLE_NAME),
        Err(_) => return elevate::FAILED,
    };
    match op {
        "install" => {
            let b = Bundle::embedded();
            if b.is_empty() {
                return elevate::EMPTY;
            }
            match write_bundle(&b, &dir) {
                Ok(w) if w.held.is_empty() => elevate::OK,
                Ok(w) => {
                    for h in &w.held {
                        winfs::delete_at_reboot(h);
                    }
                    elevate::HELD
                }
                Err(_) => elevate::FAILED,
            }
        }
        "uninstall" => match remove_bundle(&dir) {
            Ok(r) if r.gone => elevate::OK,
            Ok(r) => {
                // les fichiers d'abord, puis les dossiers, les plus profonds d'abord (Windows les prend dans l'ordre)
                for h in &r.held {
                    winfs::delete_at_reboot(h);
                }
                let mut dirs = Vec::new();
                dirs_deepest_first(&dir, &mut dirs);
                dirs.push(dir.clone());
                for d in &dirs {
                    winfs::delete_at_reboot(d);
                }
                elevate::HELD
            }
            Err(_) => elevate::FAILED,
        },
        _ => elevate::USAGE,
    }
}

#[cfg(windows)]
mod winfs {
    use std::path::{Path, PathBuf};

    /// Le dossier connu « Common Files » (C:\Program Files\Common Files pour un programme 64 bits).
    pub fn common_files() -> Result<PathBuf, String> {
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::{FOLDERID_ProgramFilesCommon, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
        unsafe {
            let p = SHGetKnownFolderPath(&FOLDERID_ProgramFilesCommon, KF_FLAG_DEFAULT, None).map_err(|e| format!("cannot find the Common Files folder ({e})"))?;
            let s = p.to_string();
            CoTaskMemFree(Some(p.0 as *const _));
            let s = s.map_err(|e| format!("cannot read the Common Files folder ({e})"))?;
            if s.is_empty() {
                return Err("Windows gave no Common Files folder".into());
            }
            Ok(PathBuf::from(s))
        }
    }

    /// Efface `p` au prochain demarrage de Windows (seulement en administrateur ; sinon, rien).
    pub fn delete_at_reboot(p: &Path) -> bool {
        use windows::core::{HSTRING, PCWSTR};
        use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_DELAY_UNTIL_REBOOT};
        unsafe { MoveFileExW(&HSTRING::from(p.as_os_str()), PCWSTR::null(), MOVEFILE_DELAY_UNTIL_REBOOT).is_ok() }
    }
}

// ── la ligne de commande ─────────────────────────────────────────────────────────────────────────────────

fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf())
    }
}

/// En tete de main() : la copie elevee (`--plugins-elevated`, seule), le dossier d'essai (`--plugins-dir`, garde pour
/// --setup, --wizard et --uninstall), et `--install-plugins`. Some(code) : fini.
pub fn early(raw: &[String]) -> Option<i32> {
    if raw.iter().any(|a| a == elevate::FLAG) {
        return Some(elevate::main(raw));
    }
    if let Some(i) = raw.iter().position(|a| a == DIR_FLAG) {
        match raw.get(i + 1).filter(|v| !v.starts_with("--") && !v.trim().is_empty()) {
            Some(d) => {
                let d = if cfg!(windows) { d.replace('/', "\\") } else { d.clone() };
                let _ = DIR_OVERRIDE.set(absolute(Path::new(&d)));
            }
            None => {
                eprintln!("{DIR_FLAG} needs a folder");
                return Some(2);
            }
        }
    }
    if raw.iter().any(|a| a == INSTALL_FLAG) {
        return Some(cli(raw));
    }
    None
}

fn status_text(s: Status) -> &'static str {
    match s {
        Status::Missing => "not installed",
        Status::UpToDate => "up to date",
        Status::Outdated => "an older version",
        Status::Different => "different files",
        Status::Newer => "a newer version",
    }
}

/// `--install-plugins [--dry-run] [--remove] [--force]` → le code de sortie (0 ; 1 si rien n'a pu se faire ; 2 : les
/// options).
fn cli(raw: &[String]) -> i32 {
    let (mut remove, mut dry, mut force) = (false, false, false);
    let mut it = raw.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            INSTALL_FLAG => {}
            "--remove" => remove = true,
            "--dry-run" => dry = true,
            "--force" => force = true,
            DIR_FLAG => {
                it.next();
            }
            "-h" | "--help" => {
                println!("{HELP}");
                return 0;
            }
            o => {
                eprintln!("unknown {INSTALL_FLAG} option {o}\n{HELP}");
                return 2;
            }
        }
    }
    let t = match env_paths().and_then(|(p, fake)| target_for(&p, fake, crate::wizard::trial())) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let b = Bundle::embedded();
    let mut r = inspect(&b, &t);
    plan(&mut r, remove, force);
    println!("Uppwind Link plugins (VST3: Uppwind Link Return, Uppwind Link Notes)");
    if !r.supported {
        println!("  {}.", r.reason);
        return 0;
    }
    println!(
        "  this companion: {}",
        if b.is_empty() { "does not carry them (a development build)".to_string() } else { format!("{} file{}, {} bytes", b.files.len(), if b.files.len() == 1 { "" } else { "s" }, b.size()) }
    );
    println!("  folder: {}{}", crate::shown_path(&r.folder), if dir_override().is_some() { " (--plugins-dir)" } else { "" });
    println!(
        "  now: {}{}",
        status_text(r.status),
        r.installed_by.as_ref().map(|v| format!(" (installed by companion {v})")).unwrap_or_default()
    );
    for f in r.files.iter().filter(|f| f.state != "same") {
        println!("    {} {}", f.name, f.state);
    }
    if !r.held.is_empty() {
        println!("  older files set aside while a DAW held them: {}", r.held.iter().map(|h| crate::shown_path(h)).collect::<Vec<_>>().join(", "));
    }
    if dry {
        for (rel, bytes, _) in b.files {
            println!("  embedded: {rel}  {} bytes  sha256 {}", bytes.len(), sha256::hex(bytes));
        }
        println!("  would: {:?} — {}{}", r.action, r.reason, if r.admin && matches!(r.action, Action::Install | Action::Update | Action::Remove) { " (Windows asks for administrator rights)" } else { "" });
        return 0;
    }
    println!("  {:?} — {}", r.action, r.reason);
    let a = apply(&mut r, &b, &t, &|l| println!("  {l}"));
    for l in &a.done {
        println!("{l}");
    }
    for e in &a.errors {
        eprintln!("{e}");
    }
    if a.changed() && matches!(r.action, Action::Install | Action::Update) {
        println!("Restart your DAW, or rescan its plugins: FL Studio › Options › Manage plugins › Find installed plugins; Ableton Live › Settings › Plug-Ins › Rescan.");
    }
    if a.errors.is_empty() && !a.refused {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests;
