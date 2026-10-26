// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Uppwind Link — le compagnon (prototype, etape 1, partie 1 : « voir le plugin dans le studio »).
//!
//! Trouve les fenetres de plugins d'Ableton Live, filme celle que le studio choisit (Windows.Graphics.Capture, sinon
//! la surface DWM de la fenetre, sinon PrintWindow) et l'envoie en JPEG au studio par un WebSocket local (127.0.0.1
//! seulement). Voir README.md.
//!
//!   uppwind-companion                    le serveur (port 47410, ou le suivant libre jusqu'a 47419)
//!   uppwind-companion --list             les fenetres de plugins de Live, puis quitte
//!   uppwind-companion --probe Diva       filme 10 s la fenetre dont le titre contient « Diva » et mesure, puis quitte
//!   uppwind-companion --bench Diva       ou passe la latence (bench.rs) : le cout de chaque capture, la cadence du
//!                                        plugin ; --bench-clicks "x,y;…" clic → image ; --bench-out DIR une image
//!   options : --port N (0 = au hasard) · --new-token · --capture wgc|dwm|print · --dev-origins (localhost:5174/5747)
//!             --allow-origin URL (https:// ou local) · --studio-origin URL|none · --state-file PATH
//!             --demo (une fausse fenetre, sans Live) · --seconds S
//!   installer (install.rs) : --install · --uninstall · --autostart on|off [--background] · --background · --stop
//!             --pairing-code (le code du fichier d'etat : un compagnon en fond ne l'ecrit jamais dans son journal)
//!   --license : la licence (GPL-3.0-or-later), la permission pour WebView2, la licence MIT des scripts des DAW, les
//!             bibliotheques et leurs licences
//!   scripts des DAW (setup/) : --setup [--apply] [--remove] [--json] [--daw ableton|fl|all] [--force] [--fl-settings]
//!   le retour audio sans cable (audio/) : --capture-test PID|NOM|tone [--seconds N] · --tone · serveur : --test-tone
//!   l'installation par le compagnon (wizard/, ui/, tray/) : un double-clic → la fenetre d'installation ; --app (l'icone
//!   et la fenetre d'etat) · --uninstall [--quiet] · --sandbox DIR · --wizard · --smoke-test [wizard|status|uninstall]

mod audio;
mod bench;
mod capture;
mod encode;
mod input;
mod install;
mod link_relay;
mod live;
mod plugin_link;
mod plugins;
mod popups;
mod protocol;
mod security;
mod session;
mod setup;
mod stream;
mod testwin;
// (24 oct.) le compagnon s'installe lui-meme : la fenetre d'installation, l'icone, la fenetre d'etat
mod tray;
mod ui;
mod wizard;

use serde_json::json;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// La ligne de licence (GPL v3 « How to apply ») : la banniere, --help et --license.
pub const NOTICE: &str = "Copyright (C) 2026 Uppwind. Free software under the GNU General Public License, version 3 or (at your option) any later version, with ABSOLUTELY NO WARRANTY: run with --license for the details.";
/// --license : la licence, la permission pour le chargeur de WebView2, la licence MIT des scripts des DAW (que l'exe
/// embarque pour --setup), les bibliotheques et leurs licences.
const LICENSE_TEXT: &str = include_str!("../../LICENSE");
const EXCEPTION_TEXT: &str = include_str!("../../LICENSE-EXCEPTION.txt");
const SCRIPTS_LICENSE_TEXT: &str = include_str!("../../LICENSE-MIT");
const NOTICES_TEXT: &str = include_str!("../../THIRD-PARTY-NOTICES.txt");

pub const DEFAULT_PORT: u16 = 47410;
pub const PORT_TRIES: u16 = 10;
pub const STATE_NAME: &str = ".uppwind-companion.json";

pub struct Shared {
    pub port: u16,
    pub token: String,
    pub allow: security::Allowlist,
    pub guard: Mutex<security::PairingGuard>,
    pub capture: Option<String>,
    pub demo: bool,
    /// --test-window : la fenetre d'essai du compagnon (testwin.rs), proposee au studio en plus des plugins de Live.
    pub test_window: Option<u64>,
}

static START: OnceLock<Instant> = OnceLock::new();

pub fn log(msg: &str) {
    let t = START.get_or_init(Instant::now).elapsed().as_secs_f64();
    println!("[{t:8.1}s] {msg}");
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// Un chemin tel qu'on l'affiche : le dossier de l'utilisateur ecrit %USERPROFILE% (une console ou un journal colle
/// dans un rapport de bug ne donne pas le nom du compte Windows).
pub fn shown_path(p: &std::path::Path) -> String {
    shown_path_in(p, &home_dir())
}

pub fn shown_path_in(p: &std::path::Path, home: &std::path::Path) -> String {
    let s = p.display().to_string();
    let h = home.display().to_string();
    if h.len() > 3 && s.len() >= h.len() && s[..h.len()].eq_ignore_ascii_case(&h) && (s.len() == h.len() || s[h.len()..].starts_with(['\\', '/'])) {
        return format!("%USERPROFILE%{}", &s[h.len()..]);
    }
    s
}

/// --pairing-code : le code du fichier d'etat (celui du compagnon qui tourne, meme en fond) → « UWC-<port>-<jeton> ».
pub fn pairing_code_of(state_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(state_json).ok()?;
    let token = v["token"].as_str().filter(|t| security::token_well_formed(t))?;
    let port = v["port"].as_u64().filter(|p| *p > 0 && *p < 65536)?;
    Some(format!("UWC-{port}-{token}"))
}

#[derive(Default)]
struct Args {
    port: Option<u16>,
    new_token: bool,
    capture: Option<String>,
    dev_origins: bool,
    allow: Vec<String>,
    /// --studio-origin URL|none : remplace l'origine du studio (None : celle de l'exe ; Some(None) : aucune)
    studio: Option<Option<String>>,
    state_file: Option<PathBuf>,
    demo: bool,
    list: bool,
    probe: Option<String>,
    any: bool,
    seconds: f64,
    test_window: Option<(i32, i32)>,
    test_window_unaware: bool,
    test_window_ghost: bool,
    describe: Option<String>,
    bench: Option<String>,
    bench_clicks: Vec<(f64, f64)>,
    bench_out: Option<String>,
    install: bool,
    uninstall: bool,
    autostart: Option<bool>,
    background: bool,
    stop: bool,
    pairing_code: bool,
    /// (25 oct.) l'ecoute des plugins Uppwind Link (plugin_link) : son port (defaut : au hasard), ou pas d'ecoute
    plugin_port: Option<u16>,
    no_plugins: bool,
}

fn parse_args() -> Result<Args, String> {
    // (audit de publication) les adresses de dev ne sont plus acceptees par defaut : --dev-origins les ajoute
    let mut a = Args { dev_origins: false, seconds: 10.0, ..Default::default() };
    let mut it = std::env::args().skip(1);
    while let Some(x) = it.next() {
        let mut val = |n: &str| it.next().ok_or(format!("{n} needs a value"));
        match x.as_str() {
            "--port" => a.port = Some(val("--port")?.parse().map_err(|_| "bad --port")?),
            "--new-token" => a.new_token = true,
            "--capture" => {
                let v = val("--capture")?;
                if v != "wgc" && v != "dwm" && v != "print" {
                    return Err("--capture is wgc, dwm or print".into());
                }
                a.capture = Some(v);
            }
            "--dev-origins" => a.dev_origins = true,
            "--no-dev-origins" => a.dev_origins = false, // (le defaut ; garde pour les scripts d'avant)
            "--allow-origin" => a.allow.push(security::normalize_origin(&val("--allow-origin")?)?),
            "--studio-origin" => {
                let v = val("--studio-origin")?;
                a.studio = Some(if v.trim().eq_ignore_ascii_case("none") { None } else { Some(security::normalize_origin(&v)?) });
            }
            "--state-file" => a.state_file = Some(PathBuf::from(val("--state-file")?)),
            "--demo" => a.demo = true,
            "--list" => a.list = true,
            "--probe" => a.probe = Some(val("--probe")?),
            "--describe" => a.describe = Some(val("--describe")?),
            "--bench" => a.bench = Some(val("--bench")?),
            "--bench-out" => a.bench_out = Some(val("--bench-out")?),
            "--bench-clicks" => {
                for p in val("--bench-clicks")?.split(';').filter(|p| !p.trim().is_empty()) {
                    let (x, y) = p.split_once(',').ok_or("--bench-clicks x,y;x,y")?;
                    a.bench_clicks.push((x.trim().parse().map_err(|_| "bad x")?, y.trim().parse().map_err(|_| "bad y")?));
                }
            }
            "--any" => a.any = true,
            "--seconds" => a.seconds = val("--seconds")?.parse().map_err(|_| "bad --seconds")?,
            "--test-window" => a.test_window = Some(a.test_window.unwrap_or((40, 40))),
            "--test-window-unaware" => a.test_window_unaware = true,
            "--test-window-ghost" => a.test_window_ghost = true,
            "--test-window-at" => {
                let v = val("--test-window-at")?;
                let (x, y) = v.split_once(',').ok_or("--test-window-at X,Y")?;
                a.test_window = Some((x.trim().parse().map_err(|_| "bad X")?, y.trim().parse().map_err(|_| "bad Y")?));
            }
            "--install" => a.install = true,
            "--uninstall" => a.uninstall = true,
            "--autostart" => {
                a.autostart = Some(match val("--autostart")?.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => return Err("--autostart is on or off".into()),
                })
            }
            "--background" => a.background = true,
            "--stop" => a.stop = true,
            "--pairing-code" => a.pairing_code = true,
            "--plugin-port" => a.plugin_port = Some(val("--plugin-port")?.parse().map_err(|_| "bad --plugin-port")?),
            "--no-plugins" => a.no_plugins = true,
            // le retour audio (audio/mod.rs les lit lui-meme)
            o if audio::is_server_flag(o).is_some() => {
                if audio::is_server_flag(o) == Some(true) {
                    val(o)?;
                }
            }
            // (24 oct.) l'installation par le compagnon (wizard/mod.rs les lit lui-meme)
            o if wizard::is_flag(o).is_some() => {
                if wizard::is_flag(o) == Some(true) {
                    val(o)?;
                }
            }
            "--license" => {
                println!("Uppwind Link companion {}", env!("CARGO_PKG_VERSION"));
                println!("{NOTICE}");
                println!("Source code: {}", env!("CARGO_PKG_REPOSITORY"));
                println!();
                println!("{LICENSE_TEXT}");
                println!();
                println!("{EXCEPTION_TEXT}");
                println!();
                println!("{SCRIPTS_LICENSE_TEXT}");
                println!();
                println!("{NOTICES_TEXT}");
                std::process::exit(0);
            }
            "-h" | "--help" => {
                println!("Uppwind Link companion {}. {NOTICE}", env!("CARGO_PKG_VERSION"));
                println!("uppwind-companion [--list | --probe TITLE [--seconds S]] [--port N] [--new-token] [--capture wgc|dwm|print] [--dev-origins] [--allow-origin URL] [--studio-origin URL|none] [--state-file PATH] [--demo] [--test-window [--test-window-at X,Y] [--test-window-unaware] [--test-window-ghost]] [--bench TITLE [--bench-clicks \"x,y;x,y\"] [--bench-out DIR]] [--background] [--plugin-port N | --no-plugins]");
                println!("  Studio origin accepted by default: {} (only that one). --dev-origins adds http://localhost and http://127.0.0.1 on ports 5174 and 5747; --allow-origin adds one https:// or local origin (for your own development server only).", security::default_studio_origin());
                println!("uppwind-companion --install | --uninstall | --autostart on|off [--background] | --stop | --pairing-code [--state-file PATH]");
                println!("{}", setup::HELP);
                std::process::exit(0);
            }
            other => return Err(format!("unknown option {other} (see --help)")),
        }
    }
    Ok(a)
}

#[cfg(windows)]
fn dpi_aware() {
    // les tailles en pixels physiques (un ecran a 125 % ou 150 % : la zone client vraie, pas mise a l'echelle)
    use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}
#[cfg(not(windows))]
fn dpi_aware() {}

fn print_windows() {
    let all = plugins::enumerate();
    if !plugins::live_running(&all) {
        println!("Ableton Live is not running.");
        return;
    }
    let wins = plugins::plugin_windows(&all);
    if wins.is_empty() {
        println!("Live is running, but no plugin window is open (open one in Live: the wrench icon of the device).");
    }
    for w in wins {
        println!("{:>12}  {}×{}{}  \"{}\"  (plugin {}, track {})", w.id, w.w, w.h, if w.minimized { " minimized" } else { "" }, w.title, w.plugin, w.track);
    }
}

/// --probe : filme une fenetre de plugin pendant quelques secondes et mesure (rien n'est envoye ni ecrit).
fn probe(name: &str, seconds: f64, force: Option<&str>, any: bool) -> i32 {
    let all = plugins::enumerate();
    let mut wins = plugins::plugin_windows(&all);
    if any {
        // --any (dev) : n'importe quelle fenetre visible, pour verifier la capture elle-meme (le serveur, lui, ne
        // filme jamais que des fenetres de plugins de Live)
        wins = all
            .iter()
            .filter(|w| w.visible && !w.title.is_empty())
            .map(|w| plugins::PluginWindow { id: w.hwnd, title: w.title.clone(), plugin: String::new(), track: String::new(), w: w.w, h: w.h, minimized: w.minimized })
            .collect();
    }
    let needle = name.to_lowercase();
    let Some(w) = wins.iter().find(|w| w.title.to_lowercase().contains(&needle)) else {
        println!("No Live plugin window whose title contains \"{name}\". Windows found:");
        print_windows();
        return 2;
    };
    let mut cap = match capture::open(w.id, force) {
        Ok(c) => c,
        Err(e) => {
            println!("Cannot capture \"{}\": {e}", w.title);
            return 1;
        }
    };
    println!("Capturing \"{}\" with {} for {seconds:.0} s (30 fps asked)...", w.title, cap.kind());
    let t0 = Instant::now();
    let mut prev: Option<capture::Frame> = None;
    let (mut asked, mut got, mut changed, mut bytes) = (0u64, 0u64, 0u64, 0u64);
    let (mut grab_t, mut enc_t) = (Duration::ZERO, Duration::ZERO);
    let mut size = (0, 0);
    while t0.elapsed().as_secs_f64() < seconds {
        let tick = Instant::now();
        asked += 1;
        let g0 = Instant::now();
        match cap.grab() {
            Ok(Some(f)) => {
                grab_t += g0.elapsed();
                got += 1;
                size = (f.w, f.h);
                let same = prev.as_ref().map(|p| p.w == f.w && p.h == f.h && p.bgra == f.bgra).unwrap_or(false);
                if !same {
                    changed += 1;
                    let e0 = Instant::now();
                    if let Ok(j) = encode::jpeg(&encode::cap_size(capture::Frame { w: f.w, h: f.h, bgra: f.bgra.clone() }, encode::MAX_PIXELS), encode::DEFAULT_QUALITY) {
                        bytes += j.len() as u64;
                    }
                    enc_t += e0.elapsed();
                }
                prev = Some(f);
            }
            Ok(None) => {}
            Err(e) => {
                println!("Capture stopped: {e}");
                break;
            }
        }
        let spent = tick.elapsed();
        if spent < Duration::from_millis(33) {
            std::thread::sleep(Duration::from_millis(33) - spent);
        }
    }
    let s = t0.elapsed().as_secs_f64();
    println!("{}", json!({
        "window": w.title, "capture": cap.kind(), "size": format!("{}x{}", size.0, size.1), "seconds": (s * 10.0).round() / 10.0,
        "ticks": asked, "frames": got, "changed": changed,
        "changed_fps": ((changed as f64 / s) * 10.0).round() / 10.0,
        "grab_ms_avg": if got > 0 { ((grab_t.as_secs_f64() * 1000.0 / got as f64) * 10.0).round() / 10.0 } else { 0.0 },
        "jpeg_ms_avg": if changed > 0 { ((enc_t.as_secs_f64() * 1000.0 / changed as f64) * 10.0).round() / 10.0 } else { 0.0 },
        "jpeg_kb_avg": if changed > 0 { (bytes as f64 / 1024.0 / changed as f64).round() } else { 0.0 },
        "kb_per_s": (bytes as f64 / 1024.0 / s).round(),
    }));
    0
}

/// Le jeton : celui du fichier d'etat s'il est la (l'appairage est fait une fois), sinon un neuf.
fn load_token(path: &PathBuf, renew: bool) -> String {
    if !renew {
        if let Ok(raw) = std::fs::read_to_string(path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(t) = v["token"].as_str() {
                    if security::token_well_formed(t) {
                        return t.to_string();
                    }
                }
            }
        }
    }
    security::new_token()
}

fn bind(port: Option<u16>) -> std::io::Result<TcpListener> {
    match port {
        Some(p) => TcpListener::bind(("127.0.0.1", p)),
        None => {
            let mut last = None;
            for p in DEFAULT_PORT..DEFAULT_PORT + PORT_TRIES {
                match TcpListener::bind(("127.0.0.1", p)) {
                    Ok(l) => return Ok(l),
                    Err(e) => last = Some(e),
                }
            }
            Err(last.unwrap())
        }
    }
}

/// --install / --uninstall / --autostart (install.rs) → le code de sortie ; dit ce qui a ete fait, ligne par ligne.
fn run_installer(a: &Args) -> i32 {
    let dirs = match install::Dirs::from_env() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut lines: Vec<String> = Vec::new();
    let mut r: Result<(), String> = Ok(());
    if a.uninstall {
        // d'abord les reglages de FL que --setup --fl-settings a ecrits (FL ouvert : rien n'est retire)
        r = setup::undo_fl_settings_for_uninstall().map(|l| lines.extend(l));
        if r.is_ok() {
            r = install::uninstall(&dirs).map(|l| lines.extend(l));
        }
    } else {
        if a.install {
            r = match std::env::current_exe() {
                Ok(src) => install::install(&dirs, &src).map(|l| lines.extend(l)),
                Err(e) => Err(format!("cannot find this program's own file ({e})")),
            };
        }
        if r.is_ok() {
            if let Some(on) = a.autostart {
                r = install::autostart(&dirs, on, a.background).map(|l| lines.extend(l));
            }
        }
    }
    for l in &lines {
        println!("{l}");
    }
    let state = a.state_file.clone().unwrap_or_else(wizard::state_file);
    match r {
        Ok(()) => {
            if a.install {
                println!();
                println!("Start it from the Start Menu: \"Uppwind Link Companion\".");
                println!("The pairing code stays in {}: the studio stays paired.", shown_path(&state));
                println!("A companion that is already running keeps the old version until you close its window.");
            }
            if a.uninstall {
                println!("The pairing code file {} was left in place.", shown_path(&state));
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// (26 oct.) Les DLL chargees en cours de route (LoadLibrary par nom : uxtheme.dll de tao, celles de WebView2…)
/// viennent de System32 seulement, jamais du dossier de l'exe : un exe telecharge tourne depuis Telechargements, ou
/// n'importe quoi peut etre pose a cote. Les DLL importees le sont deja (/DEPENDENTLOADFLAG:0x800, .cargo/config.toml).
#[cfg(windows)]
fn dlls_from_system32_only() {
    use windows::Win32::System::LibraryLoader::{SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_SYSTEM32};
    // SAFETY : un appel sans pointeur, au tout debut du processus
    unsafe {
        let _ = SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32);
    }
}

fn main() {
    #[cfg(windows)]
    dlls_from_system32_only();
    START.get_or_init(Instant::now);
    // --setup (setup/mod.rs) : ses propres options, une simulation sans --apply
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.iter().any(|a| a == "--setup") {
        std::process::exit(setup::main(&raw));
    }
    // le retour audio sans cable (audio/mod.rs) : le diagnostic et la tonalite d'essai, sans serveur
    if raw.iter().any(|a| a == "--capture-test" || a == "--tone") {
        std::process::exit(audio::main(&raw));
    }
    // (24 oct.) un double-clic, --app, --uninstall, --sandbox… (wizard/mod.rs) ; sinon la console, comme avant
    let mut app = match wizard::early(&raw) {
        wizard::Early::Exit(code) => std::process::exit(code),
        wizard::Early::App(m) => Some(m),
        wizard::Early::Console => None,
    };
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    if args.install || args.uninstall || args.autostart.is_some() {
        std::process::exit(run_installer(&args));
    }
    if args.stop {
        // (24 oct.) un compagnon avec son icone s'arrete proprement (l'icone part avec lui)
        if tray::single::request_quit(Duration::from_secs(5)) {
            println!("Stopped Uppwind Link.");
            std::process::exit(0);
        }
        let state = args.state_file.clone().unwrap_or_else(wizard::state_file);
        match install::stop(&state) {
            Ok(m) => {
                println!("{m}");
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }
    if args.pairing_code {
        // le code du compagnon qui tourne (en fond ou avec son icone : son journal ne le contient pas)
        let state = args.state_file.clone().unwrap_or_else(wizard::state_file);
        match std::fs::read_to_string(&state).ok().as_deref().and_then(pairing_code_of) {
            Some(code) => {
                println!("{code}");
                std::process::exit(0);
            }
            None => {
                eprintln!("No pairing code in {} yet: start the companion once.", shown_path(&state));
                std::process::exit(1);
            }
        }
    }
    // (24 oct.) --background (l'ouverture de session) : l'icone aussi ; un seul compagnon avec icone (tray/single.rs)
    if args.background && app.is_none() {
        app = Some(tray::AppMode { window: false, replace: raw.iter().any(|a| a == "--replace") });
    }
    #[cfg(windows)]
    let guard = match &app {
        Some(m) => match tray::prepare(m) {
            Ok(g) => Some(g),
            Err(code) => std::process::exit(code),
        },
        None => None,
    };
    // sans console (en fond, ou l'appli avec son icone) : la sortie va dans companion.log, jamais le code d'appairage
    let to_log = args.background || app.is_some();
    if to_log {
        // sans console : tout ce qui suit s'ecrit dans le journal (%LOCALAPPDATA%\Uppwind Link\companion.log)
        let log = install::Dirs::from_env().map(|d| d.log()).unwrap_or_else(|_| home_dir().join("uppwind-companion.log"));
        // les codes d'appairage ecrits par un compagnon d'avant (il les mettait dans ce journal) : retires
        let scrubbed = install::scrub_log(&log);
        if let Err(e) = install::go_background(&log) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        println!();
        println!("=== started in the background (pid {}, unix time {t}); --stop ends it", std::process::id());
        if scrubbed > 0 {
            println!("(removed {scrubbed} pairing code(s) that an older companion had written to this log)");
        }
    }
    dpi_aware();
    // avant tout fil de connexion : le MTA de COM vit autant que le programme (sinon : plantage a la 2e connexion)
    capture::keep_com_alive();
    if args.list {
        print_windows();
        return;
    }
    #[cfg(windows)]
    if let Some(name) = &args.describe {
        // --describe Diva : la fenetre de plugin et ses sous-fenetres (diagnostic des gestes ; rien n'est envoye)
        let wins = plugins::plugin_windows(&plugins::enumerate());
        let needle = name.to_lowercase();
        match wins.iter().find(|w| w.title.to_lowercase().contains(&needle)) {
            Some(w) => {
                println!("\"{}\"", w.title);
                for l in input::win::describe(w.id) {
                    println!("  {l}");
                }
            }
            None => print_windows(),
        }
        return;
    }
    #[cfg(windows)]
    if let Some(name) = &args.bench {
        std::process::exit(bench::run(name, &args.bench_clicks, args.bench_out.as_deref(), args.seconds));
    }
    if let Some(name) = &args.probe {
        std::process::exit(probe(name, args.seconds, args.capture.as_deref(), args.any));
    }

    let state = args.state_file.clone().unwrap_or_else(wizard::state_file);
    let token = load_token(&state, args.new_token);
    // la machine elle-meme, et rien d'autre (un seul socket a l'ecoute, sur 127.0.0.1)
    let listener = match bind(args.port) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Cannot listen on 127.0.0.1 ({e}). Is another companion running?");
            std::process::exit(1);
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let allow = match &args.studio {
        None => security::Allowlist::new(args.dev_origins, &args.allow),
        Some(studio) => security::Allowlist::with_studio(studio.as_deref(), args.dev_origins, &args.allow),
    };
    if allow.origins().is_empty() {
        eprintln!("No studio origin is allowed (--studio-origin none without --allow-origin): nothing could pair.");
        std::process::exit(2);
    }
    // (25 oct.) les plugins Uppwind Link : une ecoute a part, un secret a part (garde comme le jeton), lus par les plugins
    let plugin_secret = plugin_link::load_secret(&state);
    let plugin_port = if args.no_plugins {
        None
    } else {
        match plugin_link::start(args.plugin_port.unwrap_or(0), plugin_secret) {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("warning: {e} (the Uppwind Link plugins cannot connect)");
                None
            }
        }
    };
    let mut st = json!({
        "port": port, "token": token, "pid": std::process::id(), "version": env!("CARGO_PKG_VERSION"),
        "protocol": protocol::PROTOCOL_VERSION, "origins": allow.origins(),
    });
    if let Some(p) = plugin_port {
        st["pluginPort"] = json!(p);
        st["pluginSecret"] = json!(uwl_proto::secret_hex(&plugin_secret));
        st["pluginProtocol"] = json!(uwl_proto::VERSION);
    }
    if let Err(e) = std::fs::write(&state, serde_json::to_string_pretty(&st).unwrap()) {
        eprintln!("warning: cannot write {} ({e})", shown_path(&state));
    }

    println!("Uppwind Link companion {}: your DAW's plugin windows and sound, in Uppwind Studio", env!("CARGO_PKG_VERSION"));
    println!("{NOTICE}");
    println!("Listening on ws://127.0.0.1:{port} (this computer only).");
    println!("Studio origins allowed: {}", allow.origins().join(", "));
    if args.dev_origins || !args.allow.is_empty() || args.studio.is_some() {
        println!("WARNING: this companion accepts more than the studio ({}). Only use --dev-origins, --allow-origin or", security::default_studio_origin());
        println!("         --studio-origin for a development server of your own, and never paste your pairing code elsewhere.");
    }
    println!();
    if to_log {
        // sans console (en fond, ou l'appli avec son icone : la fenetre d'etat montre le code), tout ceci va dans companion.log, un fichier qu'on joint volontiers a un rapport de bug : le
        // code d'appairage (le secret qui donne la main sur le compagnon) n'y est jamais ecrit
        println!("PAIRING CODE: not written to this log (it is a secret). To see it, run:");
        println!("    \"{}\" --pairing-code", std::env::current_exe().map(|p| shown_path(&p)).unwrap_or_else(|_| "uppwind-companion.exe".into()));
    } else {
        println!("PAIRING CODE (paste it once in the studio's Plugin window; keep it to yourself):");
        println!();
        println!("    UWC-{port}-{token}");
    }
    println!();
    println!("Saved in {} (the same code next time; --new-token makes a new one and unpairs the studio).", shown_path(&state));
    if args.demo {
        println!("DEMO MODE: one fake plugin window, no Live needed.");
    } else {
        print_windows();
        if live::available() {
            println!("Live script channel (UppwindLink): found");
        } else {
            println!("Live script channel (UppwindLink): not found, no {} yet (Open/Close from the studio need it; capture works without)", shown_path(&live::state_file()));
        }
    }
    audio::server_start(&raw, args.demo);
    match plugin_port {
        Some(p) => println!("Uppwind Link plugins (Return, Notes): listening on 127.0.0.1:{p} (port and secret in the state file)"),
        None => println!("Uppwind Link plugins: off"),
    }
    let test_window = match args.test_window {
        #[cfg(windows)]
        Some(at) => match testwin::open_with(at, args.test_window_unaware, args.test_window_ghost) {
            Ok(t) => {
                println!("TEST WINDOW: \"{}\" (a fake plugin of the companion itself, listed in the studio)", testwin::TITLE);
                Some(t.top)
            }
            Err(e) => {
                eprintln!("cannot open the test window: {e}");
                None
            }
        },
        _ => None,
    };
    println!("Ctrl+C stops the companion.");
    println!();

    let shared = Arc::new(Shared {
        port,
        token,
        allow,
        guard: Mutex::new(security::PairingGuard::new(5, Duration::from_secs(30))),
        capture: args.capture.clone(),
        demo: args.demo,
        test_window,
    });
    // (24 oct.) avec l'icone : l'icone et la fenetre d'etat sur un fil a elles (STA pour WebView2 ; ce fil-ci garde son
    // MTA) ; quand l'icone quitte, tout s'arrete. Le code n'y est que montre (la fenetre, le bouton Copy), jamais ecrit.
    #[cfg(windows)]
    if let (Some(mode), Some(guard)) = (app, guard) {
        let token = shared.token.clone();
        std::thread::spawn(move || std::process::exit(tray::run(mode, guard, port, token)));
    }
    // un fil par connexion, au plus session::MAX_CONNECTIONS a la fois (comptees des l'acceptation)
    session::serve(listener, shared, session::MAX_CONNECTIONS);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_pairing_code_comes_from_the_state_file() {
        let t = "0123456789abcdef".repeat(4);
        assert_eq!(pairing_code_of(&json!({"port": 47411, "token": t, "pid": 7}).to_string()), Some(format!("UWC-47411-{t}")));
        assert_eq!(pairing_code_of(&json!({"port": 47411, "token": "short"}).to_string()), None);
        assert_eq!(pairing_code_of(&json!({"token": t}).to_string()), None, "no port");
        assert_eq!(pairing_code_of("not json"), None);
    }

    #[test]
    fn shown_paths_hide_the_account_name() {
        let home = Path::new(r"C:\Users\someone");
        assert_eq!(shown_path_in(Path::new(r"C:\Users\someone\.uppwind-companion.json"), home), r"%USERPROFILE%\.uppwind-companion.json");
        assert_eq!(shown_path_in(Path::new(r"c:\users\SOMEONE\AppData\Local\Uppwind Link\x.exe"), home), r"%USERPROFILE%\AppData\Local\Uppwind Link\x.exe");
        assert_eq!(shown_path_in(Path::new(r"C:\Users\someone2\x"), home), r"C:\Users\someone2\x", "another account");
        assert_eq!(shown_path_in(Path::new(r"D:\state.json"), home), r"D:\state.json");
    }
}
