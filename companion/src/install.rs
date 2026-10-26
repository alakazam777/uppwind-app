// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! L'installation (22 oct.) : `--install`, `--uninstall`, `--autostart on|off`, `--background`, `--stop`.
//!
//! Avant, on lancait le compagnon depuis `target\release`, la ou il etait construit. Desormais :
//!   --install          copie l'exe dans %LOCALAPPDATA%\Uppwind Link\uppwind-companion.exe (une copie plus ancienne est
//!                      remplacee ; si elle tourne, elle est d'abord renommee a cote, ce que Windows permet) et cree le
//!                      raccourci « Uppwind Link Companion » du menu Demarrer (la console, le code d'appairage).
//!   --uninstall        retire l'exe, le raccourci, le demarrage automatique et le journal.
//!   --autostart on|off un raccourci dans le dossier Demarrage de l'utilisateur (fenetre reduite ; avec --background :
//!                      sans fenetre, le journal dans %LOCALAPPDATA%\Uppwind Link\companion.log).
//!   --background       sans console : la sortie va au journal (companion.log, remis a zero au-dela de 1 Mo).
//!   --stop             arrete le compagnon nomme dans le fichier d'etat (son pid), seulement si c'est bien un
//!                      uppwind-companion*.exe.
//! Jamais HKLM ni rien qui demande l'administrateur : le profil de l'utilisateur seulement. Le jeton d'appairage reste
//! dans %USERPROFILE%\.uppwind-companion.json (l'appairage survit). UWC_INSTALL_ROOT=<dossier> remplace LOCALAPPDATA
//! (<dossier>\LocalAppData) et APPDATA (<dossier>\AppData) : les essais n'ecrivent jamais dans le vrai menu Demarrer.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const APP_DIR: &str = "Uppwind Link";
pub const EXE_NAME: &str = "uppwind-companion.exe";
pub const SHORTCUT_NAME: &str = "Uppwind Link Companion.lnk";
pub const LOG_NAME: &str = "companion.log";
pub const ROOT_ENV: &str = "UWC_INSTALL_ROOT";
pub const LOG_MAX: u64 = 1024 * 1024;
const ASIDE_PREFIX: &str = "uppwind-companion.old-";

/// Les dossiers de l'utilisateur ou tout se range.
#[derive(Debug, Clone, PartialEq)]
pub struct Dirs {
    /// %LOCALAPPDATA%
    pub local: PathBuf,
    /// %APPDATA% (le menu Demarrer de l'utilisateur est dessous)
    pub roaming: PathBuf,
}

impl Dirs {
    /// `get` lit une variable d'environnement (les essais en donnent une fausse).
    pub fn from_lookup(get: impl Fn(&str) -> Option<OsString>) -> Result<Dirs, String> {
        if let Some(root) = get(ROOT_ENV).filter(|v| !v.is_empty()) {
            let r = PathBuf::from(root);
            return Ok(Dirs { local: r.join("LocalAppData"), roaming: r.join("AppData") });
        }
        let local = get("LOCALAPPDATA").filter(|v| !v.is_empty()).ok_or("LOCALAPPDATA is not set")?;
        let roaming = get("APPDATA").filter(|v| !v.is_empty()).ok_or("APPDATA is not set")?;
        Ok(Dirs { local: PathBuf::from(local), roaming: PathBuf::from(roaming) })
    }
    pub fn from_env() -> Result<Dirs, String> {
        Self::from_lookup(|k| std::env::var_os(k))
    }
    pub fn app_dir(&self) -> PathBuf {
        self.local.join(APP_DIR)
    }
    pub fn exe(&self) -> PathBuf {
        self.app_dir().join(EXE_NAME)
    }
    pub fn log(&self) -> PathBuf {
        self.app_dir().join(LOG_NAME)
    }
    pub fn programs(&self) -> PathBuf {
        self.roaming.join("Microsoft").join("Windows").join("Start Menu").join("Programs")
    }
    pub fn start_menu_link(&self) -> PathBuf {
        self.programs().join(SHORTCUT_NAME)
    }
    pub fn startup_link(&self) -> PathBuf {
        self.programs().join("Startup").join(SHORTCUT_NAME)
    }
}

/// Un raccourci : la cible, ses arguments, reduit ou non.
#[derive(Debug, Clone, PartialEq)]
pub struct Shortcut {
    pub target: PathBuf,
    pub args: String,
    pub minimized: bool,
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn aside_name(n: u32) -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{ASIDE_PREFIX}{t}-{n}.exe")
}

/// Retire un fichier ; s'il est tenu (un exe qui tourne), le renomme a cote (Windows le permet) → ce qui a ete fait.
fn remove_or_aside(p: &Path) -> Result<Option<String>, String> {
    if !p.exists() {
        return Ok(None);
    }
    if std::fs::remove_file(p).is_ok() {
        return Ok(Some(format!("removed {}", p.display())));
    }
    let dir = p.parent().ok_or("no parent folder")?;
    for n in 0..20 {
        let to = dir.join(aside_name(n));
        if to.exists() {
            continue;
        }
        return match std::fs::rename(p, &to) {
            Ok(()) => Ok(Some(format!("{} is in use (running?): renamed it to {} (removed next time)", p.display(), to.display()))),
            Err(e) => Err(format!("cannot remove or rename {} ({e}): stop the companion that runs from there, then try again", p.display())),
        };
    }
    Err(format!("cannot rename {} aside", p.display()))
}

/// Les copies mises de cote par un --install precedent (plus tenues : on peut les effacer) → combien.
fn sweep_asides(dir: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let mut n = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with(ASIDE_PREFIX) && name.ends_with(".exe") && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}

/// --install : `src` (l'exe qui tourne) copie dans le dossier de l'application, le raccourci du menu Demarrer (et celui
/// du demarrage automatique remis a jour s'il existe) → les lignes de ce qui a ete fait.
pub fn install(dirs: &Dirs, src: &Path) -> Result<Vec<String>, String> {
    let mut did = Vec::new();
    let app = dirs.app_dir();
    std::fs::create_dir_all(&app).map_err(|e| format!("cannot create {} ({e})", app.display()))?;
    let dst = dirs.exe();
    if same_file(src, &dst) {
        did.push(format!("Already running from {}: nothing to copy.", dst.display()));
    } else {
        if let Some(l) = remove_or_aside(&dst)? {
            if l.contains("renamed") {
                did.push(format!("The installed copy {l}."));
            }
        }
        std::fs::copy(src, &dst).map_err(|e| format!("cannot copy to {} ({e})", dst.display()))?;
        did.push(format!("Copied {} to {}", src.display(), dst.display()));
    }
    let swept = sweep_asides(&app);
    if swept > 0 {
        did.push(format!("Removed {swept} old cop{} set aside by a previous install.", if swept == 1 { "y" } else { "ies" }));
    }
    let sm = dirs.start_menu_link();
    shortcut::write(&sm, &Shortcut { target: dst.clone(), args: String::new(), minimized: false })?;
    did.push(format!("Start Menu shortcut \"Uppwind Link Companion\": {}", sm.display()));
    // le demarrage automatique, s'il est la, suit le nouvel exe (ses arguments gardes)
    let su = dirs.startup_link();
    if su.exists() {
        let old = shortcut::read(&su).ok();
        let args = old.as_ref().map(|s| s.args.clone()).unwrap_or_default();
        shortcut::write(&su, &Shortcut { target: dst.clone(), args, minimized: true })?;
        did.push(format!("Startup shortcut updated: {}", su.display()));
    }
    Ok(did)
}

/// --uninstall : l'exe (mis de cote s'il tourne), les deux raccourcis, le journal, le dossier s'il est vide. Le fichier
/// d'appairage (%USERPROFILE%\.uppwind-companion.json) n'est pas touche.
pub fn uninstall(dirs: &Dirs) -> Result<Vec<String>, String> {
    let mut did = Vec::new();
    for (what, p) in [("Start Menu shortcut", dirs.start_menu_link()), ("Startup shortcut", dirs.startup_link())] {
        if p.exists() {
            std::fs::remove_file(&p).map_err(|e| format!("cannot remove {} ({e})", p.display()))?;
            did.push(format!("Removed the {what}: {}", p.display()));
        }
    }
    if let Some(l) = remove_or_aside(&dirs.exe())? {
        did.push(format!("{}{}", l[..1].to_uppercase(), &l[1..]));
    }
    let log = dirs.log();
    if log.exists() && std::fs::remove_file(&log).is_ok() {
        did.push(format!("Removed the log {}", log.display()));
    }
    sweep_asides(&dirs.app_dir());
    let app = dirs.app_dir();
    if app.exists() {
        if std::fs::remove_dir(&app).is_ok() {
            did.push(format!("Removed the folder {}", app.display()));
        } else {
            did.push(format!("{} still holds a running copy: delete the folder once it has stopped.", app.display()));
        }
    }
    if did.is_empty() {
        did.push("Nothing was installed.".into());
    }
    Ok(did)
}

/// --autostart on|off : le raccourci du dossier Demarrage (reduit ; `background` : sans fenetre). Il lance la copie
/// installee : --install d'abord.
pub fn autostart(dirs: &Dirs, on: bool, background: bool) -> Result<Vec<String>, String> {
    let su = dirs.startup_link();
    if !on {
        if su.exists() {
            std::fs::remove_file(&su).map_err(|e| format!("cannot remove {} ({e})", su.display()))?;
            return Ok(vec![format!("Autostart off: removed {}", su.display())]);
        }
        return Ok(vec!["Autostart was already off.".into()]);
    }
    let exe = dirs.exe();
    if !exe.exists() {
        return Err(format!("Not installed yet ({} is missing): run uppwind-companion.exe --install first.", exe.display()));
    }
    let args = if background { "--background".to_string() } else { String::new() };
    shortcut::write(&su, &Shortcut { target: exe, args, minimized: true })?;
    Ok(vec![format!(
        "Autostart on: {} ({}).",
        su.display(),
        if background { "no window; the log is in companion.log" } else { "starts minimized in the taskbar" }
    )])
}

/// Le journal de --background : ouvert en ajout, remis a zero au-dela de LOG_MAX.
pub fn open_log(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let big = std::fs::metadata(path).map(|m| m.len() > LOG_MAX).unwrap_or(false);
    let mut o = std::fs::OpenOptions::new();
    o.create(true);
    if big {
        o.write(true).truncate(true);
    } else {
        o.append(true);
    }
    o.open(path)
}

/// Les codes d'appairage qu'un compagnon d'avant ecrivait dans le journal de --background (main.rs ne l'ecrit plus) :
/// retires du fichier → combien. Rien a faire (ou pas de journal) : 0.
pub fn scrub_log(path: &Path) -> usize {
    let Ok(bytes) = std::fs::read(path) else { return 0 };
    let text = String::from_utf8_lossy(&bytes);
    let (clean, n) = crate::security::scrub_pairing_codes(&text);
    if n > 0 && std::fs::write(path, clean.as_bytes()).is_err() {
        return 0;
    }
    n
}

/// --background : la sortie (stdout, stderr) va au journal, puis la console est lachee (sa fenetre se ferme si elle
/// n'etait qu'a nous). Rust relit le handle standard a chaque ecriture : println! ecrit dans le fichier.
#[cfg(windows)]
pub fn go_background(log: &Path) -> Result<(), String> {
    use std::os::windows::io::IntoRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{FreeConsole, SetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};
    let f = open_log(log).map_err(|e| format!("cannot open the log {} ({e})", log.display()))?;
    let h = HANDLE(f.into_raw_handle());
    unsafe {
        SetStdHandle(STD_OUTPUT_HANDLE, h).map_err(|e| format!("SetStdHandle: {e}"))?;
        SetStdHandle(STD_ERROR_HANDLE, h).map_err(|e| format!("SetStdHandle: {e}"))?;
        let _ = FreeConsole();
    }
    Ok(())
}
#[cfg(not(windows))]
pub fn go_background(_log: &Path) -> Result<(), String> {
    Err("--background is for Windows".into())
}

/// Le pid du fichier d'etat (ecrit par le compagnon au demarrage).
pub fn state_pid(state: &Path) -> Option<u32> {
    let raw = std::fs::read_to_string(state).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v["pid"].as_u64().and_then(|p| u32::try_from(p).ok()).filter(|p| *p != 0)
}

/// Un nom d'exe a nous : uppwind-companion.exe, ou une copie (uppwind-companion.part4.exe…).
pub fn is_companion_image(path: &str) -> bool {
    let name = path.rsplit(['\\', '/']).next().unwrap_or("").to_ascii_lowercase();
    name.starts_with("uppwind-companion") && name.ends_with(".exe")
}

/// --stop : le compagnon du fichier d'etat, s'il tourne et si c'est bien un uppwind-companion*.exe.
#[cfg(windows)]
pub fn stop(state: &Path) -> Result<String, String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE};
    let pid = state_pid(state).ok_or(format!("No companion recorded in {}.", state.display()))?;
    if pid == std::process::id() {
        return Err("That is this very process.".into());
    }
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, false, pid).map_err(|_| format!("No companion running (pid {pid} is gone)."))?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let named = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let image = String::from_utf16_lossy(&buf[..len as usize]);
        let r = if !named || !is_companion_image(&image) {
            Err(format!("pid {pid} is not a companion ({image}): left alone."))
        } else {
            TerminateProcess(h, 0).map(|_| format!("Stopped the companion (pid {pid}, {image}).")).map_err(|e| format!("cannot stop pid {pid}: {e}"))
        };
        let _ = CloseHandle(h);
        r
    }
}
#[cfg(not(windows))]
pub fn stop(_state: &Path) -> Result<String, String> {
    Err("--stop is for Windows".into())
}

/// Les raccourcis .lnk (IShellLinkW, dans le profil de l'utilisateur).
pub mod shortcut {
    use super::Shortcut;
    use std::path::Path;

    #[cfg(windows)]
    fn com() {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
    }

    #[cfg(windows)]
    pub fn write(lnk: &Path, s: &Shortcut) -> Result<(), String> {
        use windows::core::{Interface, HSTRING};
        use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER};
        use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
        use windows::Win32::UI::WindowsAndMessaging::{SW_SHOWMINNOACTIVE, SW_SHOWNORMAL};
        com();
        if let Some(d) = lnk.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("cannot create {} ({e})", d.display()))?;
        }
        let err = |e: windows::core::Error| format!("cannot write the shortcut {} ({e})", lnk.display());
        unsafe {
            let l: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(err)?;
            l.SetPath(&HSTRING::from(s.target.as_os_str())).map_err(err)?;
            l.SetArguments(&HSTRING::from(s.args.as_str())).map_err(err)?;
            if let Some(d) = s.target.parent() {
                l.SetWorkingDirectory(&HSTRING::from(d.as_os_str())).map_err(err)?;
            }
            l.SetDescription(&HSTRING::from("Uppwind Link companion: shows your DAW's plugin windows in Uppwind Studio")).map_err(err)?;
            l.SetIconLocation(&HSTRING::from(s.target.as_os_str()), 0).map_err(err)?;
            l.SetShowCmd(if s.minimized { SW_SHOWMINNOACTIVE } else { SW_SHOWNORMAL }).map_err(err)?;
            let pf: IPersistFile = l.cast().map_err(err)?;
            pf.Save(&HSTRING::from(lnk.as_os_str()), true).map_err(err)?;
        }
        Ok(())
    }

    #[cfg(windows)]
    pub fn read(lnk: &Path) -> Result<Shortcut, String> {
        use windows::core::{Interface, HSTRING};
        use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ};
        use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWMINNOACTIVE;
        com();
        let err = |e: windows::core::Error| format!("cannot read the shortcut {} ({e})", lnk.display());
        let z = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())]);
        unsafe {
            let l: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(err)?;
            let pf: IPersistFile = l.cast().map_err(err)?;
            pf.Load(&HSTRING::from(lnk.as_os_str()), STGM_READ).map_err(err)?;
            let mut p = [0u16; 1024];
            l.GetPath(&mut p, std::ptr::null_mut(), 0).map_err(err)?;
            let mut a = [0u16; 1024];
            l.GetArguments(&mut a).map_err(err)?;
            let show = l.GetShowCmd().map_err(err)?;
            Ok(Shortcut { target: z(&p).into(), args: z(&a), minimized: show == SW_SHOWMINNOACTIVE })
        }
    }

    #[cfg(not(windows))]
    pub fn write(_lnk: &Path, _s: &Shortcut) -> Result<(), String> {
        Err("shortcuts are for Windows".into())
    }
    #[cfg(not(windows))]
    pub fn read(_lnk: &Path) -> Result<Shortcut, String> {
        Err("shortcuts are for Windows".into())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Un dossier neuf sous le dossier temporaire (jamais le vrai profil).
    fn temp_root(tag: &str) -> PathBuf {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let d = std::env::temp_dir().join(format!("uwc-install-test-{tag}-{}-{t}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn dirs_in(root: &Path) -> Dirs {
        let env: HashMap<&str, OsString> = HashMap::from([(ROOT_ENV, root.as_os_str().to_owned())]);
        Dirs::from_lookup(|k| env.get(k).cloned()).unwrap()
    }
    fn fake_exe(root: &Path, bytes: &[u8]) -> PathBuf {
        let p = root.join("build").join("uppwind-companion.exe");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn dirs_come_from_the_environment_or_the_override() {
        let env: HashMap<&str, OsString> = HashMap::from([("LOCALAPPDATA", OsString::from(r"C:\Users\x\AppData\Local")), ("APPDATA", OsString::from(r"C:\Users\x\AppData\Roaming"))]);
        let d = Dirs::from_lookup(|k| env.get(k).cloned()).unwrap();
        assert_eq!(d.exe(), PathBuf::from(r"C:\Users\x\AppData\Local\Uppwind Link\uppwind-companion.exe"));
        assert_eq!(d.log(), PathBuf::from(r"C:\Users\x\AppData\Local\Uppwind Link\companion.log"));
        assert_eq!(d.start_menu_link(), PathBuf::from(r"C:\Users\x\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Uppwind Link Companion.lnk"));
        assert_eq!(d.startup_link(), PathBuf::from(r"C:\Users\x\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup\Uppwind Link Companion.lnk"));
        // l'override passe devant
        let env2: HashMap<&str, OsString> = HashMap::from([(ROOT_ENV, OsString::from(r"D:\test")), ("LOCALAPPDATA", OsString::from(r"C:\real"))]);
        let d2 = Dirs::from_lookup(|k| env2.get(k).cloned()).unwrap();
        assert_eq!(d2.local, PathBuf::from(r"D:\test\LocalAppData"));
        assert_eq!(d2.roaming, PathBuf::from(r"D:\test\AppData"));
        // rien : une erreur, pas un chemin au hasard
        assert!(Dirs::from_lookup(|_| None).is_err());
        assert!(Dirs::from_lookup(|k| if k == "LOCALAPPDATA" { Some("x".into()) } else { None }).is_err());
        assert!(Dirs::from_lookup(|k| if k == ROOT_ENV { Some(OsString::new()) } else { None }).is_err(), "an empty override is no override");
    }

    #[test]
    fn install_copies_and_makes_the_start_menu_shortcut_then_uninstall_removes_everything() {
        let root = temp_root("install");
        let d = dirs_in(&root);
        let keep = root.join("keep.json");
        std::fs::write(&keep, "{\"token\":\"x\"}").unwrap();
        let src = fake_exe(&root, b"v1");
        let did = install(&d, &src).unwrap();
        assert!(did.iter().any(|l| l.starts_with("Copied")), "{did:?}");
        assert_eq!(std::fs::read(d.exe()).unwrap(), b"v1");
        let sm = shortcut::read(&d.start_menu_link()).unwrap();
        assert_eq!(sm, Shortcut { target: d.exe(), args: String::new(), minimized: false });
        assert!(!d.startup_link().exists(), "install alone does not autostart");
        // une version plus recente remplace l'ancienne
        std::fs::write(&src, b"v2").unwrap();
        install(&d, &src).unwrap();
        assert_eq!(std::fs::read(d.exe()).unwrap(), b"v2");
        // l'exe installe qui se reinstalle : rien a copier
        let again = install(&d, &d.exe()).unwrap();
        assert!(again[0].starts_with("Already running from"), "{again:?}");
        // autostart : reduit, sur la copie installee ; --background passe en argument ; off le retire
        let a = autostart(&d, true, false).unwrap();
        assert!(a[0].starts_with("Autostart on"), "{a:?}");
        assert_eq!(shortcut::read(&d.startup_link()).unwrap(), Shortcut { target: d.exe(), args: String::new(), minimized: true });
        autostart(&d, true, true).unwrap();
        assert_eq!(shortcut::read(&d.startup_link()).unwrap().args, "--background");
        // une reinstallation garde l'autostart et ses arguments
        install(&d, &src).unwrap();
        assert_eq!(shortcut::read(&d.startup_link()).unwrap(), Shortcut { target: d.exe(), args: "--background".into(), minimized: true });
        assert_eq!(autostart(&d, false, false).unwrap()[0], format!("Autostart off: removed {}", d.startup_link().display()));
        assert_eq!(autostart(&d, false, false).unwrap()[0], "Autostart was already off.");
        autostart(&d, true, false).unwrap();
        // tout retirer : l'exe, les raccourcis, le dossier ; le reste (le fichier d'appairage) intact
        std::fs::write(d.log(), "log").unwrap();
        let un = uninstall(&d).unwrap();
        assert!(!d.exe().exists() && !d.start_menu_link().exists() && !d.startup_link().exists() && !d.app_dir().exists(), "{un:?}");
        assert_eq!(std::fs::read_to_string(&keep).unwrap(), "{\"token\":\"x\"}");
        assert_eq!(uninstall(&d).unwrap(), vec!["Nothing was installed.".to_string()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn autostart_needs_an_installed_copy() {
        let root = temp_root("autostart");
        let d = dirs_in(&root);
        let e = autostart(&d, true, false).unwrap_err();
        assert!(e.contains("--install first"), "{e}");
        assert!(!d.startup_link().exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_running_installed_copy_is_renamed_aside_then_swept_next_time() {
        // une vraie copie qui tourne : ping.exe de Windows, copie sous le nom de l'exe installe (Windows refuse de
        // l'effacer, accepte de le renommer — comme un compagnon qui tourne)
        let root = temp_root("running");
        let d = dirs_in(&root);
        std::fs::create_dir_all(d.app_dir()).unwrap();
        let ping = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32").join("PING.EXE");
        std::fs::copy(&ping, d.exe()).unwrap();
        let mut child = std::process::Command::new(d.exe()).args(["-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(std::fs::remove_file(d.exe()).is_err(), "Windows keeps a running exe");
        let src = fake_exe(&root, b"new");
        let did = install(&d, &src).unwrap();
        assert!(did.iter().any(|l| l.contains("renamed it to")), "{did:?}");
        assert_eq!(std::fs::read(d.exe()).unwrap(), b"new");
        let asides = || std::fs::read_dir(d.app_dir()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with(ASIDE_PREFIX)).count();
        assert_eq!(asides(), 1);
        child.kill().unwrap();
        let _ = child.wait();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let did2 = install(&d, &src).unwrap();
        assert!(did2.iter().any(|l| l.starts_with("Removed 1 old copy")), "{did2:?}");
        assert_eq!(asides(), 0);
        uninstall(&d).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_log_starts_over_past_one_megabyte() {
        use std::io::Write;
        let root = temp_root("log");
        let p = root.join("sub").join("companion.log");
        open_log(&p).unwrap().write_all(b"one\n").unwrap();
        open_log(&p).unwrap().write_all(b"two\n").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "one\ntwo\n");
        std::fs::write(&p, vec![b'x'; (LOG_MAX + 1) as usize]).unwrap();
        open_log(&p).unwrap().write_all(b"fresh\n").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "fresh\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn old_pairing_codes_are_scrubbed_from_the_log() {
        let root = temp_root("scrub");
        let p = root.join("companion.log");
        let t = crate::security::new_token();
        std::fs::write(&p, format!("start\n    UWC-47410-{t}\nend\n")).unwrap();
        assert_eq!(scrub_log(&p), 1);
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(!s.contains(&t) && s.contains("UWC-47410-<removed>") && s.ends_with("end\n"), "{s}");
        assert_eq!(scrub_log(&p), 0, "nothing left to remove");
        assert_eq!(scrub_log(&root.join("missing.log")), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stop_reads_the_pid_and_only_touches_a_companion() {
        let root = temp_root("stop");
        let st = root.join("state.json");
        assert_eq!(state_pid(&st), None);
        std::fs::write(&st, "{\"pid\": 1234, \"port\": 47411}").unwrap();
        assert_eq!(state_pid(&st), Some(1234));
        std::fs::write(&st, "{\"pid\": 0}").unwrap();
        assert_eq!(state_pid(&st), None);
        assert!(is_companion_image(r"C:\build\target\release\uppwind-companion.exe"));
        assert!(is_companion_image(r"C:\x\Uppwind-Companion.part4.exe"));
        assert!(!is_companion_image(r"C:\Windows\System32\PING.EXE"));
        assert!(!is_companion_image(r"C:\x\uppwind-companion.exe.bak"));
        // un autre programme sous ce pid : laisse tranquille
        let mut child = std::process::Command::new("PING.EXE").args(["-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        std::fs::write(&st, format!("{{\"pid\": {}}}", child.id())).unwrap();
        let e = stop(&st).unwrap_err();
        assert!(e.contains("is not a companion"), "{e}");
        assert!(child.try_wait().unwrap().is_none(), "ping still runs");
        child.kill().unwrap();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&root);
    }
}
