// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (24 oct.) Le compagnon s'installe lui-meme, plutot qu'un setup.exe NSIS, et le moins possible a faire pour
//! l'utilisateur.
//!
//!   un double-clic sur l'exe telecharge  → la fenetre d'installation (steps.rs : la machine a etats ; flow.rs : ce que
//!                                          fait « Install », c'est-a-dire install.rs et setup/ dans l'ordre)
//!   le menu Demarrer, l'icone, --app     → l'appli : l'icone de la zone de notification et la fenetre d'etat (src/tray)
//!   --uninstall (Applications installees)→ une fenetre de confirmation, puis tout est remis (flow.rs ; --quiet : sans)
//!   --sandbox <dossier>                  → tout cela dans un dossier d'essai : le programme, ses raccourcis, son
//!                                          fichier d'appairage ; l'entree « Applications installees » sous
//!                                          HKCU\Software\UppwindLinkTest ; Live et FL lus mais jamais changes
//!   --wizard · --smoke-test [wizard|status|uninstall] · --ui-preview <dossier> · --version
//!
//! Dans main.rs : `wizard::early()` en tete, et ses options sautees par parse_args. Rien d'autre ne change pour la
//! console du compagnon (un terminal, ou une option).

pub mod arp;
pub mod firstrun;
pub mod flow;
pub mod steps;

use crate::install::{self, Dirs};
use crate::setup::env::Probe;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// La page du studio : l'origine que cet exe accepte par defaut (security.rs, UPPWIND_STUDIO_ORIGIN a la compilation),
/// pas une adresse ecrite une seconde fois ici (une copie pour un autre studio ouvre le sien).
pub fn studio_url() -> &'static str {
    static URL: OnceLock<String> = OnceLock::new();
    URL.get_or_init(|| format!("{}/atelier", crate::security::default_studio_origin().trim_end_matches('/')))
}
pub const LOOPMIDI_URL: &str = "https://www.tobias-erichsen.de/software/loopmidi.html";
pub const WEBVIEW2_URL: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";
pub const VBCABLE_URL: &str = "https://vb-audio.com/Cable/";

static SANDBOX: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Le dossier du bac a sable (--sandbox), s'il y en a un.
pub fn sandbox() -> Option<&'static Path> {
    SANDBOX.get().and_then(|s| s.as_deref())
}

/// Un essai de l'installeur : --sandbox, ou UWC_INSTALL_ROOT a la main. Live, FL et la vraie entree « Uninstall »
/// ne sont alors jamais touches (sauf la fausse machine de UWC_SETUP_ROOT).
pub fn trial() -> bool {
    sandbox().is_some() || std::env::var_os(install::ROOT_ENV).is_some_and(|v| !v.is_empty())
}

/// Les options lues ici : main.rs (parse_args) les saute ; Some(true) : suivie d'une valeur.
pub fn is_flag(a: &str) -> Option<bool> {
    match a {
        "--sandbox" | "--ui-preview" | "--shot" | "--download" => Some(true),
        "--app" | "--replace" | "--quiet" | "--wizard" | "--version" | "--smoke-test" | "--self-update" | "--show" | "--check-update" => Some(false),
        _ => None,
    }
}

/// Ce que main.rs fait ensuite.
pub enum Early {
    /// fini : ce code de sortie
    Exit(i32),
    /// le compagnon avec son icone : le serveur dans un fil, l'icone et ses fenetres sur le fil principal
    App(crate::tray::AppMode),
    /// la console du compagnon, comme avant
    Console,
}

/// Les options d'un essai pour les raccourcis et les lancements : --sandbox "<dossier>".
pub fn extra_args() -> Vec<String> {
    sandbox().map(|d| vec!["--sandbox".into(), d.display().to_string()]).unwrap_or_default()
}

/// Le fichier d'etat du compagnon (port, jeton, pid) : celui de l'utilisateur, ou celui du bac a sable.
pub fn state_file() -> PathBuf {
    match sandbox() {
        Some(d) => d.join(crate::STATE_NAME),
        None => crate::home_dir().join(crate::STATE_NAME),
    }
}

fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf())
    }
}

/// En tete de main() : le bac a sable, puis la fenetre qu'il faut (ou la console).
pub fn early(raw: &[String]) -> Early {
    let mut sb: Option<PathBuf> = None;
    let mut rest: Vec<&str> = Vec::new();
    let mut it = raw.iter();
    while let Some(a) = it.next() {
        if a == "--sandbox" {
            match it.next() {
                Some(d) if !d.starts_with("--") => sb = Some(absolute(Path::new(&if cfg!(windows) { d.replace('/', "\\") } else { d.clone() }))),
                _ => {
                    eprintln!("--sandbox needs a folder");
                    return Early::Exit(2);
                }
            }
        } else {
            rest.push(a.as_str());
        }
    }
    if let Some(d) = &sb {
        if let Err(e) = std::fs::create_dir_all(d) {
            eprintln!("cannot create the sandbox {} ({e})", d.display());
            return Early::Exit(1);
        }
        // avant tout fil : install.rs lit UWC_INSTALL_ROOT (le programme, les raccourcis, le journal vont dans le bac)
        std::env::set_var(install::ROOT_ENV, d);
    }
    let _ = SANDBOX.set(sb.clone());
    let has = |f: &str| rest.contains(&f);
    let value_of = |f: &str| rest.iter().position(|a| *a == f).and_then(|i| rest.get(i + 1)).map(|s| s.to_string());
    if has("--version") {
        println!("{VERSION}");
        return Early::Exit(0);
    }
    if has("--ui-preview") {
        let Some(dir) = value_of("--ui-preview") else {
            eprintln!("--ui-preview needs a folder");
            return Early::Exit(2);
        };
        return Early::Exit(crate::ui::preview::write_all(Path::new(&dir)));
    }
    if has("--smoke-test") {
        let kind = value_of("--smoke-test").filter(|k| !k.starts_with("--")).unwrap_or_else(|| "wizard".into());
        return Early::Exit(crate::ui::smoke_test(&kind));
    }
    if has("--uninstall") {
        return Early::Exit(uninstall_main(has("--quiet")));
    }
    // (11 nov., update.rs) l'exe telecharge et verifie par la fenetre d'etat d'une version d'avant : il s'installe
    if has("--check-update") {
        return Early::Exit(crate::update::console(value_of("--download").as_deref().map(Path::new)));
    }
    if has(crate::update::SELF_UPDATE_FLAG) {
        return Early::Exit(self_update(has("--show")));
    }
    if has("--wizard") {
        if has("--quiet") {
            return Early::Exit(console_install(installed_version_now(), false));
        }
        return Early::Exit(run_wizard(installed_version_now()));
    }
    if has("--app") {
        return Early::App(crate::tray::AppMode { window: true, replace: has("--replace") });
    }
    if rest.is_empty() {
        // sans option (ou seulement --sandbox, pour essayer le premier lancement depuis un terminal)
        let seen = observe(sb.is_some());
        return match firstrun::decide(&seen, VERSION) {
            firstrun::Launch::Console => Early::Console,
            firstrun::Launch::App => Early::App(crate::tray::AppMode { window: true, replace: false }),
            firstrun::Launch::Wizard { installed } => Early::Exit(run_wizard(installed)),
            firstrun::Launch::OpenInstalled => Early::Exit(open_installed()),
        };
    }
    Early::Console
}

/// La version de la copie installee : l'entree « Applications installees » si elle nomme ce dossier, sinon le bloc de
/// version de l'exe installe ; None : inconnue (une copie d'avant l'assistant).
pub fn installed_version(dirs: &Dirs) -> Option<String> {
    if let Ok(key) = arp::key() {
        if let Some((v, loc)) = arp::installed(&arp::ArpRegistry, &key) {
            if crate::setup::same_path(Path::new(&loc), &dirs.app_dir()) {
                return Some(v);
            }
        }
    }
    firstrun::file_version(&dirs.exe())
}

fn installed_version_now() -> Option<String> {
    let d = Dirs::from_env().ok()?;
    d.exe().is_file().then(|| installed_version(&d).unwrap_or_default())
}

fn observe(force_double_click: bool) -> firstrun::Seen {
    let dirs = Dirs::from_env().ok();
    let exe = std::env::current_exe().ok();
    let installed_exe = dirs.as_ref().map(|d| d.exe());
    let from_install = matches!((&exe, &installed_exe), (Some(a), Some(b)) if firstrun::same_file(a, b));
    let installed = installed_exe.as_ref().is_some_and(|p| p.is_file());
    firstrun::Seen {
        no_args: true,
        double_click: force_double_click || firstrun::double_clicked(),
        from_install,
        installed,
        installed_version: if installed { dirs.as_ref().and_then(installed_version) } else { None },
    }
}

/// Lance un programme detache (sans console, sans nos poignees) → son pid. Nos entree et sorties standard ne passent
/// pas au fils : un programme qui lit notre sortie par un tube (un script, un test) attendrait sinon la fin du
/// compagnon lance, qui tient le tube ouvert.
pub fn spawn_detached(exe: &Path, args: &[String]) -> Result<u32, String> {
    let mut c = std::process::Command::new(exe);
    c.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        keep_std_handles();
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
        c.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    c.spawn().map(|ch| ch.id()).map_err(|e| format!("cannot start {} ({e})", exe.display()))
}

/// Nos entree et sorties standard ne passent plus aux processus que l'on lance (CreateProcess passe toutes les poignees
/// heritables, pas seulement celles qu'on lui donne).
#[cfg(windows)]
fn keep_std_handles() {
    use windows::Win32::Foundation::{SetHandleInformation, HANDLE_FLAGS, HANDLE_FLAG_INHERIT};
    use windows::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
    unsafe {
        for s in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            if let Ok(h) = GetStdHandle(s) {
                if !h.is_invalid() && !h.0.is_null() {
                    let _ = SetHandleInformation(h, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0));
                }
            }
        }
    }
}

/// La meme version (ou plus recente) est installee : on lui passe la main (elle montre sa fenetre).
fn open_installed() -> i32 {
    let Ok(dirs) = Dirs::from_env() else { return 1 };
    let mut args = vec!["--app".to_string()];
    args.extend(extra_args());
    match spawn_detached(&dirs.exe(), &args) {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// Les morceaux de l'installation : les dossiers, la machine, la cle de l'entree, le fichier d'etat.
pub struct Parts {
    pub dirs: Dirs,
    pub probe: Probe,
    pub simulate_daws: bool,
    pub arp_key: String,
    pub state_file: PathBuf,
    pub extra_args: Vec<String>,
    /// (29 oct.) ou vont les plugins Uppwind Link (simules dans le bac a sable, comme Live et FL)
    pub plugins: crate::plugin_install::Target,
}

pub fn parts() -> Result<Parts, String> {
    let dirs = Dirs::from_env()?;
    let probe = Probe::from_env()?;
    let simulate_daws = trial() && !probe.fake;
    let plugins = crate::plugin_install::target_for(&probe.paths, probe.fake, simulate_daws)?;
    Ok(Parts { dirs, simulate_daws, probe, arp_key: arp::key()?, state_file: state_file(), extra_args: extra_args(), plugins })
}

impl Parts {
    pub fn env<'a>(&'a self, reg: &'a dyn crate::setup::env::Registry) -> flow::Env<'a> {
        flow::Env {
            dirs: self.dirs.clone(),
            probe: &self.probe,
            simulate_daws: self.simulate_daws,
            arp_reg: reg,
            arp_key: self.arp_key.clone(),
            state_file: self.state_file.clone(),
            extra_args: self.extra_args.clone(),
            stamp: crate::setup::scripts::now_stamp(),
            plugins: self.plugins.clone(),
            plugin_bundle: crate::plugin_install::Bundle::embedded(),
            component_bundle: crate::plugin_install::Bundle::component(),
        }
    }
}

/// Le modele de la fenetre d'installation : une simulation de `--setup` (avec les reglages de FL), rien n'est ecrit.
pub fn detect(update_from: Option<String>) -> Result<steps::Model, String> {
    let p = parts()?;
    let opts = crate::setup::Opts { fl_settings: Some(true), ..Default::default() };
    let r = crate::setup::gather(&p.probe, &opts, &crate::setup::scripts::now_stamp());
    Ok(steps::Model::from_report(&r, update_from, sandbox().map(|d| d.display().to_string()), p.simulate_daws, p.dirs.app_dir().display().to_string()))
}

/// Le code d'appairage du compagnon qui tourne (son fichier d'etat) : « UWC-<port>-<jeton> ».
/// (Montre dans la fenetre ou la console de l'installation, jamais ecrit dans companion.log.)
pub fn pairing_code(state: &Path) -> Option<String> {
    crate::pairing_code_of(&std::fs::read_to_string(state).ok()?)
}

/// Les effets reels de flow.rs : la progression (vers la fenetre), le compagnon qui tourne ferme proprement, le
/// nouveau lance, le nettoyage apres coup.
pub struct RealHooks<F: Fn(&str)> {
    pub progress: F,
    pub state_file: PathBuf,
}

impl<F: Fn(&str)> flow::Hooks for RealHooks<F> {
    fn progress(&self, line: &str) {
        (self.progress)(line)
    }
    fn stop_running(&self) -> Option<String> {
        crate::tray::single::request_quit(Duration::from_secs(6)).then(|| "Closed the Uppwind Link that was running (it starts again at the end).".to_string())
    }
    /// (29 oct.) macOS : le gardien des ports MIDI, dont le fichier est a cote du fichier d'etat (midi_ports.rs)
    fn stop_midi_ports(&self) -> Result<Option<String>, String> {
        #[cfg(target_os = "macos")]
        let r = crate::midi_ports::stop_keeper(&crate::midi_ports::dir_of(&self.state_file), &crate::midi_ports::default_names());
        #[cfg(not(target_os = "macos"))]
        let r = Ok(None);
        r
    }
    fn start(&self, exe: &Path, args: &[String]) -> Result<Option<u16>, String> {
        let pid = spawn_detached(exe, args)?;
        // il ecrit son fichier d'etat des qu'il ecoute
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(8) {
            if let Ok(raw) = std::fs::read_to_string(&self.state_file) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                    if v["pid"].as_u64() == Some(pid as u64) {
                        return Ok(v["port"].as_u64().map(|p| p as u16));
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err("Uppwind Link was installed but did not start: open it from the Start menu.".into())
    }
    fn cleanup_later(&self, files: &[PathBuf], dirs: &[PathBuf], trees: &[PathBuf]) {
        cleanup_after_exit(files, dirs, trees);
    }
}

/// (26 oct.) cmd.exe de System32, par son chemin complet : Rust cherche un nom nu d'abord dans le dossier de l'exe, et
/// l'exe telecharge tourne depuis Telechargements (un « cmd.exe » pose a cote serait lance).
pub fn system_cmd() -> PathBuf {
    let root = ["SystemRoot", "windir"]
        .iter()
        .filter_map(|v| std::env::var_os(v))
        .map(PathBuf::from)
        .find(|p| p.is_absolute() && p.join("System32").join("cmd.exe").is_file())
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    root.join("System32").join("cmd.exe")
}

/// Apres la sortie de ce processus (3 s) : efface ces fichiers, ces arbres (les donnees d'une fenetre), puis ces
/// dossiers s'ils sont vides (cmd.exe, sans fenetre). Pour l'exe qui se desinstalle lui-meme.
pub fn cleanup_after_exit(files: &[PathBuf], empty_dirs: &[PathBuf], trees: &[PathBuf]) {
    if files.is_empty() && empty_dirs.is_empty() && trees.is_empty() {
        return;
    }
    let mut cmd = String::from("ping -n 4 127.0.0.1 >nul");
    for f in files {
        cmd.push_str(&format!(" & del /f /q \"{}\"", f.display()));
    }
    for t in trees {
        cmd.push_str(&format!(" & rd /s /q \"{}\" 2>nul", t.display()));
    }
    for d in empty_dirs {
        cmd.push_str(&format!(" & rd \"{}\" 2>nul", d.display()));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        keep_std_handles();
        let _ = std::process::Command::new(system_cmd())
            .raw_arg(format!("/d /s /c \"{cmd}\""))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn();
    }
}

/// La fenetre d'installation (WebView2 present), sinon l'installation dans la console.
fn run_wizard(installed: Option<String>) -> i32 {
    if crate::ui::webview2_version().is_none() {
        return console_install(installed, true);
    }
    // la console d'un double-clic disparait tout de suite (la machine se lit en ~0,2 s, avant la fenetre)
    crate::ui::hide_console();
    let model = match detect(installed) {
        Ok(m) => m,
        Err(e) => {
            crate::ui::message_box("Uppwind Link", &format!("Uppwind Link cannot be installed: {e}"));
            return 1;
        }
    };
    crate::ui::run_page(Box::new(crate::ui::pages::WizardPage::new(steps::Wizard::new(model))))
}

/// (11 nov.) Les choix d'une mise a jour faite depuis la fenetre d'etat : seulement ce qui est deja pose (les scripts
/// des DAW a mettre a jour, les plugins deja installes), le demarrage avec Windows comme il est, rien de nouveau (ni
/// les reglages de FL, ni un DAW que la personne n'avait pas coche). `admin_ok` : l'invite d'administrateur des
/// plugins est permise (un clic sur « Update » ; jamais la mise a jour automatique).
pub fn update_choices(m: &steps::Model, autostart: bool, admin_ok: bool) -> steps::Choices {
    let mut daws: Vec<String> = m.daws.iter().filter(|d| d.selectable && d.action == "update").map(|d| d.id.clone()).collect();
    let au_update = m.plugins.au.as_ref().is_some_and(|a| a.action == "update");
    if m.plugins.selectable && (m.plugins.action == "update" || au_update) && (admin_ok || !m.plugins.admin) {
        daws.push(steps::PLUGINS.into());
    }
    steps::Choices { daws, fl_settings: false, autostart, open_studio: false }
}

/// (11 nov.) `--self-update [--show]` : cet exe vient d'etre telecharge et verifie (update.rs) ; il s'installe a la
/// place de la copie installee, comme l'installation, avec les choix de `update_choices`, arrete celui qui tourne et se
/// lance (`--background --replace`). `show` : sa fenetre d'etat s'ouvre ensuite. Le compte rendu va dans
/// `updates\last-update.json`, que la fenetre d'etat montre. Puis l'exe telecharge s'efface (apres la sortie).
fn self_update(show: bool) -> i32 {
    // (11 nov., relecture du Mac) seulement sous Windows : ailleurs le fichier n'a pas de version lisible (file_version),
    // donc rien ne refuserait une version plus ancienne, et il n'y a pas de mise a jour depuis l'app
    if !cfg!(windows) {
        eprintln!("--self-update is for Windows. On this computer, run --install from the new version instead.");
        return 1;
    }
    let p = match parts() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let updates = crate::update::updates_dir(&p.dirs.app_dir());
    let from = installed_version_now().filter(|v| !v.is_empty());
    let dst = p.dirs.exe();
    // (11 nov.) `failed` : cette version n'a pas pris la place (la mise a jour automatique ne la reessaie pas)
    let report = |ok: bool, failed: bool, errors: &[String], done: &[String]| {
        let _ = crate::update::write_last(&updates, VERSION, from.as_deref(), ok, failed, errors, done);
    };
    // (11 nov.) seulement plus recente que la copie installee (la version de son fichier ; pas de copie, ou pas de
    // version lisible : permis, c'est une reparation)
    let installed_file = if dst.is_file() { firstrun::file_version(&dst) } else { None };
    if let Err(e) = self_update_allowed(VERSION, installed_file.as_deref()) {
        report(false, true, std::slice::from_ref(&e), &[]);
        return 1;
    }
    let m = match detect(from.clone()) {
        Ok(m) => m,
        Err(e) => {
            report(false, true, std::slice::from_ref(&e), &[]);
            return 1;
        }
    };
    let c = update_choices(&m, p.dirs.startup_link().exists(), show);
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            report(false, true, &[format!("cannot find this program's own file ({e})")], &[]);
            return 1;
        }
    };
    // (11 nov.) la copie d'avant, gardee le temps de la mise a jour : elle revient si la nouvelle ne demarre pas
    let backup = updates.join("uppwind-companion-previous.exe");
    let had_previous = dst.is_file() && !firstrun::same_file(&exe, &dst);
    let have_backup = had_previous && std::fs::create_dir_all(&updates).is_ok() && std::fs::copy(&dst, &backup).is_ok();
    let reg = arp::ArpRegistry;
    let env = p.env(&reg);
    let hooks = RealHooks { progress: |l: &str| println!("... {l}"), state_file: p.state_file.clone() };
    let o = flow::install(&env, &exe, &c, &hooks);
    let mut done = o.done.clone();
    done.extend(o.scripts.iter().cloned());
    done.extend(o.plugin_lines.iter().cloned());
    let mut errors = o.errors.clone();
    // (11 nov.) l'installation ou le lancement a echoue : la copie d'avant est relancee comme son raccourci de
    // demarrage (`--background`), remise d'abord si l'installation l'avait deja remplacee
    // (ailleurs que sous Windows, flow::install ne lance rien : seul compte ce qui a echoue)
    let new_running = if cfg!(windows) { o.port.is_some() || crate::tray::single::running() } else { o.ok };
    if cfg!(windows) {
        let installed_is_previous = dst.is_file() && std::fs::read(&dst).ok() != std::fs::read(&exe).ok();
        match restore_plan(new_running, had_previous, installed_is_previous, have_backup) {
            Restore::Nothing => {}
            Restore::Lost => errors.push("The previous Uppwind Link could not be put back: install it again from the release page.".into()),
            plan => {
                let back = if plan == Restore::PutBackThenStart { install::replace_exe(&backup, &dst).map(|_| ()) } else { Ok(()) };
                let mut args = vec!["--background".to_string()];
                args.extend(p.extra_args.iter().cloned());
                match back.and_then(|_| spawn_detached(&dst, &args)) {
                    Ok(_) => done.push(format!("Started the previous Uppwind Link{} again.", from.as_deref().map(|v| format!(" ({v})")).unwrap_or_default())),
                    Err(e) => errors.push(format!("The previous Uppwind Link could not be started again ({e}).")),
                }
            }
        }
    }
    let _ = std::fs::remove_file(&backup);
    report(errors.is_empty(), !new_running, &errors, &done);
    #[cfg(windows)]
    if show && o.port.is_some() {
        // le nouveau compagnon a pris la place : sa fenetre
        for _ in 0..20 {
            if crate::tray::single::signal_show() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    // l'exe telecharge (dans updates\) n'est plus utile : efface apres la sortie, s'il n'est pas la copie installee
    if !firstrun::same_file(&exe, &p.dirs.exe()) && exe.starts_with(&updates) {
        cleanup_after_exit(std::slice::from_ref(&exe), &[], &[]);
    }
    if o.ok {
        0
    } else {
        1
    }
}

/// (11 nov., pure) `--self-update` : cette version (`this`) peut-elle remplacer la copie installee, dont `installed` est
/// la version lue dans son fichier ? Seulement si elle est plus recente ; pas de copie, ou pas de version lisible :
/// permis (une reparation).
pub fn self_update_allowed(this: &str, installed: Option<&str>) -> Result<(), String> {
    match installed {
        Some(v) if crate::update::parse_version(v).is_some() && !crate::update::is_newer(this, v) => Err(format!("Uppwind Link {v} is installed: {this} is not newer, nothing was changed.")),
        _ => Ok(()),
    }
}

/// (11 nov.) Apres un `--self-update` : que faire de la copie d'avant ?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restore {
    /// la nouvelle tourne, ou il n'y avait rien avant
    Nothing,
    /// la copie installee est encore celle d'avant : la relancer
    Start,
    /// l'installation l'a remplacee : la remettre (depuis sa copie de cote), puis la relancer
    PutBackThenStart,
    /// remplacee, et pas de copie de cote : on le dit
    Lost,
}

/// (pure) `new_running` : la nouvelle version tourne ; `had_previous` : une copie etait installee ; `installed_is_previous`
/// : le fichier installe est encore celui d'avant ; `have_backup` : sa copie de cote existe.
pub fn restore_plan(new_running: bool, had_previous: bool, installed_is_previous: bool, have_backup: bool) -> Restore {
    if new_running || !had_previous {
        Restore::Nothing
    } else if installed_is_previous {
        Restore::Start
    } else if have_backup {
        Restore::PutBackThenStart
    } else {
        Restore::Lost
    }
}

fn ask(q: &str, default: bool) -> bool {
    use std::io::Write;
    print!("{q} [{}] ", if default { "Y/n" } else { "y/N" });
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    if std::io::stdin().read_line(&mut s).is_err() {
        return default;
    }
    match s.trim().to_ascii_lowercase().as_str() {
        "" => default,
        a => matches!(a, "y" | "yes" | "o" | "oui"),
    }
}

fn pause() {
    use std::io::Write;
    print!("Press Enter to close this window.");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
}

/// (10 nov.) Ce que dit la console avant la premiere question : sous Windows, il manque WebView2 a la fenetre ; sous
/// macOS, il n'y a pas encore de fenetre d'installation.
fn console_intro() -> Vec<String> {
    if cfg!(target_os = "macos") {
        vec!["There is no setup window on macOS yet: the setup goes on here, in Terminal.".into()]
    } else {
        vec![
            "The setup window needs Microsoft Edge WebView2 Runtime, which is missing on this computer.".into(),
            format!("It is free, from Microsoft: {WEBVIEW2_URL}"),
            "Without it, the setup goes on here, in this window.".into(),
        ]
    }
}

/// (10 nov.) La question du demarrage automatique. Windows : le raccourci du dossier Demarrage (l'icone). macOS : un
/// LaunchAgent, une copie sans Terminal, donc sans `arm` (la reponse par defaut y est non : `Choices::for_console`).
const AUTOSTART_QUESTION: &str = if cfg!(target_os = "macos") {
    "Start Uppwind Link when you log in? That copy runs without Terminal, so remote control stays off in it."
} else {
    "Start Uppwind Link with Windows?"
};

/// Sans WebView2 (et sous macOS) : l'installation dans la console (les memes etapes, deux ou trois questions).
/// `--wizard --quiet` : la meme, sans question, avec les choix de `Choices::for_console` (tout ce qui a un sens, et
/// Start with Windows ; sous macOS, ni les reglages de FL ni le LaunchAgent) — une installation silencieuse, et
/// l'essai de bout en bout dans un bac a sable.
fn console_install(installed: Option<String>, interactive: bool) -> i32 {
    println!("Uppwind Link {VERSION}{}", if installed.is_some() { " (update)" } else { "" });
    if interactive {
        println!();
        for l in console_intro() {
            println!("{l}");
        }
        println!();
        if !ask("Install Uppwind Link now?", true) {
            return 1;
        }
    }
    let p = match parts() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let m = match detect(installed) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut c = steps::Choices::for_console(&m);
    if interactive {
        if m.daws.iter().any(|d| d.daw == "fl" && d.fl_settings.as_ref().is_some_and(|f| f.changes > 0)) {
            c.fl_settings = ask("Apply FL Studio's MIDI settings for you? (FL Studio closed; a backup is kept; uninstalling puts them back)", true);
        }
        // (29 oct.) les plugins Uppwind Link (VST3) ; (10 nov.) sous macOS, leurs Audio Units (Logic Pro) avec eux : la
        // question dit ce qui serait pose
        if m.plugins.changes() {
            let ch = |a: &str| matches!(a, "install" | "update");
            let q = match m.plugins.au.as_ref() {
                Some(au) => {
                    let mut what = Vec::new();
                    if ch(&m.plugins.action) {
                        what.push(format!("VST3, for FL Studio, in {}", m.plugins.folder));
                    }
                    if ch(&au.action) {
                        what.push(format!("Audio Units, for Logic Pro, in {}", au.folder));
                    }
                    format!("Install the Uppwind Link plugins ({})?", what.join("; "))
                }
                None => format!(
                    "Install the Uppwind Link plugins (VST3, needed for FL Studio) in {}?{}",
                    m.plugins.folder,
                    if m.plugins.admin { " Windows asks for administrator rights once." } else { "" }
                ),
            };
            if !ask(&q, true) {
                c.daws.retain(|d| d != steps::PLUGINS);
            }
        }
        c.autostart = ask(AUTOSTART_QUESTION, c.autostart);
    }
    // (29 oct.) --wizard --quiet promet « sans question » : pas d'invite d'administrateur de Windows non plus — des
    // plugins qui la demanderaient attendent `--install-plugins`
    if !interactive && m.plugins.admin && c.wants_plugins() {
        c.daws.retain(|d| d != steps::PLUGINS);
        println!("The Uppwind Link plugins need administrator rights, which a quiet setup does not ask for: not installed. Run: uppwind-companion --install-plugins");
    }
    let reg = arp::ArpRegistry;
    let env = p.env(&reg);
    let hooks = RealHooks { progress: |l: &str| println!("... {l}"), state_file: p.state_file.clone() };
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("cannot find this program's own file ({e})");
            return 1;
        }
    };
    let o = flow::install(&env, &exe, &c, &hooks);
    println!();
    for l in o.done.iter().chain(o.scripts.iter()).chain(o.fl_lines.iter()).chain(o.plugin_lines.iter()) {
        println!("  {l}");
    }
    for e in &o.errors {
        println!("  could not: {e}");
    }
    println!();
    for (i, t) in o.todo.iter().enumerate() {
        println!("{}. {}", i + 1, t.title);
        for l in &t.lines {
            println!("   {l}");
        }
    }
    if let Some(code) = o.port.and_then(|_| pairing_code(&p.state_file)) {
        println!();
        println!("PAIRING CODE (paste it once in the studio: Settings > Uppwind Link > Plugin windows):");
        println!();
        println!("    {code}");
    }
    // (10 nov.) Windows : la console d'un double-clic se ferme a la fin, d'ou la pause ; sous macOS, on est dans Terminal
    if interactive && !cfg!(target_os = "macos") {
        println!();
        pause();
    }
    if o.ok {
        0
    } else {
        1
    }
}

/// --uninstall : une fenetre de confirmation (lance depuis « Applications installees »), une question dans un
/// terminal, rien avec --quiet ; puis flow::uninstall.
fn uninstall_main(quiet: bool) -> i32 {
    let p = match parts() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    if !quiet && firstrun::console_is_ours() && crate::ui::webview2_version().is_some() {
        crate::ui::hide_console();
        return crate::ui::run_page(Box::new(crate::ui::pages::UninstallPage::new()));
    }
    if !quiet && !ask("Uninstall Uppwind Link? It puts back what it changed in FL Studio and Ableton Live.", false) {
        return 1;
    }
    let reg = arp::ArpRegistry;
    let env = p.env(&reg);
    let hooks = RealHooks { progress: |l: &str| println!("... {l}"), state_file: p.state_file.clone() };
    let r = flow::uninstall(&env, &hooks);
    if let Some(b) = &r.blocked {
        eprintln!("{b}");
        return 1;
    }
    for l in &r.done {
        println!("{l}");
    }
    for e in &r.errors {
        eprintln!("could not: {e}");
    }
    for k in &r.kept {
        println!("Kept: {k}");
    }
    for t in &r.todo {
        println!("Left for you: {t}");
    }
    if !quiet && firstrun::console_is_ours() {
        pause();
    }
    if r.ok {
        0
    } else {
        1
    }
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn the_cleanup_runs_the_cmd_of_system32_never_a_bare_name() {
        let c = super::system_cmd();
        assert!(c.is_absolute() && c.is_file(), "{}", c.display());
        assert!(c.ends_with(r"System32\cmd.exe"), "{}", c.display());
    }
}

#[cfg(test)]
mod self_update_tests {
    use super::*;

    #[test]
    fn self_update_only_replaces_an_older_copy() {
        assert!(self_update_allowed("0.4.0", Some("0.3.0")).is_ok());
        assert!(self_update_allowed("0.10.0", Some("0.9.9")).is_ok(), "numbers, not text");
        let e = self_update_allowed("0.4.0", Some("0.4.0")).unwrap_err();
        assert!(e.contains("0.4.0 is installed") && e.contains("not newer"), "{e}");
        assert!(self_update_allowed("0.4.0", Some("0.5.0")).is_err(), "a downgrade");
        assert!(self_update_allowed("0.4.0", Some("0.10.0")).is_err(), "a downgrade (numbers, not text)");
        // pas de copie installee, ou pas de version lisible : une reparation reste permise
        assert!(self_update_allowed("0.4.0", None).is_ok());
        assert!(self_update_allowed("0.4.0", Some("")).is_ok());
        assert!(self_update_allowed("0.4.0", Some("junk")).is_ok());
    }

    #[test]
    fn a_failed_self_update_starts_the_previous_copy_again() {
        assert_eq!(restore_plan(true, true, false, true), Restore::Nothing, "the new one runs");
        assert_eq!(restore_plan(true, true, true, true), Restore::Nothing);
        assert_eq!(restore_plan(false, false, false, false), Restore::Nothing, "nothing was installed before");
        assert_eq!(restore_plan(false, true, true, true), Restore::Start, "the install failed: the old file is still there");
        assert_eq!(restore_plan(false, true, true, false), Restore::Start);
        assert_eq!(restore_plan(false, true, false, true), Restore::PutBackThenStart, "installed but did not start");
        assert_eq!(restore_plan(false, true, false, false), Restore::Lost);
    }
}
