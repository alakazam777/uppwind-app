// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Uppwind Link — le compagnon (prototype, etape 1, partie 1 : « voir le plugin dans le studio »).
//!
//! Trouve les fenetres de plugins d'Ableton Live, filme celle que le studio choisit (Windows.Graphics.Capture, sinon
//! la surface DWM de la fenetre, sinon PrintWindow) et l'envoie en JPEG au studio par un WebSocket local (127.0.0.1
//! seulement). Voir README.md.
//!
//!   uppwind-companion                    le serveur (port 47410, ou le suivant libre jusqu'a 47419)
//!   uppwind-companion --list             les fenetres de plugins de Live (et sous macOS de Logic Pro), puis quitte
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
//!
//! (26 oct.) macOS : la meme console (fenetres, capture, gestes, --setup, --install, --autostart, --background,
//! --stop, plugins) ; pas encore la fenetre d'installation ni l'icone (voir README, « macOS »).
//! (29 oct.) macOS : les ports MIDI vivent dans le gardien (midi_ports.rs, --midi-ports-keeper, lance par le
//! compagnon), qui lui survit ; --stop-midi-ports l'arrete.

// hors Windows, une bonne part du code (la capture WGC/DWM, la fenetre d'installation, l'icone) n'est pas appelee
#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

mod arming;
mod audio;
mod bench;
mod capture;
mod discord;
mod encode;
mod input;
mod install;
mod link_relay;
mod live;
// (12 nov.) Logic Pro sans script : la surface Mackie Control (le decodage, l'etat, les pressions ; mcu_port.rs : ses ports)
mod mcu;
// (26 oct.) macOS : fenetres, capture, gestes, autorisations (CoreGraphics) ; ports MIDI virtuels (CoreMIDI)
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod midi_ports;
// (12 nov.) macOS : les deux points de la surface Mackie Control (Logic Pro), ouverts seulement permise et armee
#[cfg(target_os = "macos")]
mod mcu_port;
// (29 oct.) les plugins Uppwind Link (VST3) embarques, et leur installation (--install-plugins, --setup, l'assistant)
mod plugin_install;
mod plugin_link;
mod plugins;
mod popups;
mod protocol;
mod security;
mod session;
mod setup;
// (29 oct.) le fichier d'etat : 0600 sous macOS et Linux, pairingV2, pluginPort retire a la sortie propre
mod state_file;
mod stream;
mod testwin;
// (24 oct.) le compagnon s'installe lui-meme : la fenetre d'installation, l'icone, la fenetre d'etat
mod tray;
mod update;
mod ui;
mod wizard;

use serde_json::json;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

/// La ligne de licence (GPL v3 « How to apply ») : la banniere, --help et --license.
pub const NOTICE: &str = "Copyright (C) 2026 Uppwind. Free software under the GNU General Public License, version 3 or (at your option) any later version, with ABSOLUTELY NO WARRANTY: run with --license for the details.";
/// --license : la licence, la permission pour le chargeur de WebView2, la licence MIT des scripts des DAW (que l'exe
/// embarque pour --setup), les bibliotheques et leurs licences.
const LICENSE_TEXT: &str = include_str!("../../LICENSE");
const EXCEPTION_TEXT: &str = include_str!("../../LICENSE-EXCEPTION.txt");
const SCRIPTS_LICENSE_TEXT: &str = include_str!("../../LICENSE-MIT");
const NOTICES_TEXT: &str = include_str!("../../THIRD-PARTY-NOTICES.txt");
/// (29 oct.) la permission des plugins (n'importe quel hote VST 3 ou CLAP peut les charger), quand l'exe les embarque
const PLUGINS_EXCEPTION_TEXT: &str = include_str!("../../LICENSE-EXCEPTION-PLUGINS.txt");

pub const DEFAULT_PORT: u16 = 47410;
pub const PORT_TRIES: u16 = 10;
pub const STATE_NAME: &str = ".uppwind-companion.json";

pub struct Shared {
    pub port: u16,
    /// (9 nov., audit F2) le jeton : il change quand le studio se desappaire (« unpair ») ou quand la personne demande un
    /// nouveau code (la fenetre d'etat, la console) ; lu par `token()`
    pub token: RwLock<String>,
    pub allow: security::Allowlist,
    pub guard: Mutex<security::PairingGuard>,
    pub capture: Option<String>,
    pub demo: bool,
    /// --test-window : la fenetre d'essai du compagnon (testwin.rs), proposee au studio en plus des plugins de Live.
    pub test_window: Option<u64>,
    /// (29 oct.) un studio a fait la poignee de main v2 avec ce jeton : un hello v1 (le jeton en clair) est refuse
    pub pairing_v2: std::sync::atomic::AtomicBool,
    /// le fichier d'etat ou le noter (« pairingV2 ») ; None : les essais
    pub state: Option<PathBuf>,
    /// (9 nov., audit F1) la capture, les gestes et le son du DAW : seulement si la personne devant l'ordinateur l'a permis
    pub arming: arming::Arming,
    /// (9 nov., audit F2) avance a chaque nouveau code : les connexions appairees avec l'ancien se ferment (4403)
    pub revoked: AtomicU64,
    /// (11 nov.) ce qu'on ecoute dans Music, sur le profil Discord : seulement si la personne l'a permis ici
    pub presence: discord::Presence,
    /// (12 nov.) Logic Pro : la surface Mackie Control (mcu.rs) ; ses ports seulement si permise (« mackie on ») et armee
    pub mcu: mcu::Hub,
}

impl Shared {
    /// Le jeton en cours.
    pub fn token(&self) -> String {
        self.token.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// (11 nov.) Permettre ou retirer la presence Discord (la fenetre d'etat, la console ; jamais le studio) ; le fichier
    /// d'etat le garde (« discordPresence »). Err : le choix vaut maintenant, mais n'a pas pu etre garde.
    pub fn set_presence(&self, on: bool, by: &str) -> Result<bool, String> {
        self.presence.set_allowed(on, by);
        let Some(path) = &self.state else { return Ok(on) };
        state_file::update(path, |v| {
            v["discordPresence"] = json!(on);
            true
        })
        .map(|_| on)
        .map_err(|e| format!("Discord presence is {} now, but {} could not keep it ({e})", if on { "on" } else { "off" }, shown_path(path)))
    }

    /// (12 nov.) Permettre ou retirer la surface Mackie Control de Logic Pro (la console ; jamais le studio) ; le fichier
    /// d'etat le garde (« mackieSurface »). Ses ports s'ouvrent seulement une fois arme (mcu_port.rs). Err : le choix
    /// vaut maintenant, mais n'a pas pu etre garde.
    pub fn set_mackie(&self, on: bool, by: &str) -> Result<bool, String> {
        self.mcu.set_allowed(on, by);
        let Some(path) = &self.state else { return Ok(on) };
        state_file::update(path, |v| {
            v[mcu::STATE_KEY] = json!(on);
            true
        })
        .map(|_| on)
        .map_err(|e| format!("the Logic Pro surface is {} now, but {} could not keep it ({e})", if on { "allowed" } else { "off" }, shown_path(path)))
    }

    /// (9 nov., audit F2) Un nouveau code : un jeton neuf, tout de suite le seul accepte ; les studios appaires avec
    /// l'ancien sont deconnectes (4403) et doivent coller le nouveau ; le compagnon est desarme. Le fichier d'etat le
    /// garde pour les lancements suivants. Err : le nouveau code sert, mais le fichier n'a pas pu le garder.
    pub fn new_code(&self, by: &str) -> Result<(), String> {
        let token = security::new_token();
        *self.token.write().unwrap_or_else(|e| e.into_inner()) = token.clone();
        self.pairing_v2.store(false, Ordering::SeqCst);
        self.revoked.fetch_add(1, Ordering::SeqCst);
        self.arming.set(false, "new pairing code");
        log(&format!("pairing: NEW pairing code ({by}): the old one no longer works; every studio paired with it is disconnected"));
        let Some(path) = &self.state else { return Ok(()) };
        let saved = state_file::update(path, |v| {
            v["token"] = json!(token);
            if let Some(o) = v.as_object_mut() {
                o.remove("pairingV2");
            }
            true
        });
        saved.map(|_| ()).map_err(|e| {
            let msg = format!("the new code works now, but it could not be saved in {} ({e}): the old code would come back at the next start", shown_path(path));
            log(&format!("warning: {msg}"));
            msg
        })
    }
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

/// Ce qui remplace le dossier de l'utilisateur dans un chemin affiche : %USERPROFILE% (Windows), ~ (macOS, Linux).
pub const HOME_SHOWN: &str = if cfg!(windows) { "%USERPROFILE%" } else { "~" };

pub fn shown_path_in(p: &std::path::Path, home: &std::path::Path) -> String {
    let s = p.display().to_string();
    let h = home.display().to_string();
    if h.len() > 3 && s.len() >= h.len() && s[..h.len()].eq_ignore_ascii_case(&h) && (s.len() == h.len() || s[h.len()..].starts_with(['\\', '/'])) {
        return format!("{HOME_SHOWN}{}", &s[h.len()..]);
    }
    s
}

/// (10 nov.) Un programme a lancer en collant la ligne dans Terminal (macOS) : `~/"Library/Application Support/…"`.
/// Le ~ reste hors des guillemets, sinon le shell ne le remplace pas (`"~/…"` ne se trouve pas) ; hors du dossier de
/// l'utilisateur, le chemin entier entre guillemets.
pub fn terminal_path(p: &std::path::Path) -> String {
    terminal_path_in(p, &home_dir())
}

pub fn terminal_path_in(p: &std::path::Path, home: &std::path::Path) -> String {
    match p.strip_prefix(home) {
        Ok(rest) if home.as_os_str().len() > 1 && !rest.as_os_str().is_empty() => format!("~/\"{}\"", rest.display()),
        _ => format!("\"{}\"", p.display()),
    }
}

/// --pairing-code : le code du fichier d'etat (celui du compagnon qui tourne, meme en fond) → (29 oct.)
/// « UWC2-<port>-<jeton> » (security::pairing_code).
pub fn pairing_code_of(state_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(state_json).ok()?;
    let token = v["token"].as_str().filter(|t| security::token_well_formed(t))?;
    let port = v["port"].as_u64().filter(|p| *p > 0 && *p < 65536)?;
    Some(security::pairing_code(port as u16, token))
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
    /// (11 nov.) --logic-view show|controls|editor (macOS, diagnostic) : l'affichage des fenetres de plugins de Logic
    logic_view: Option<String>,
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
    #[allow(dead_code)]
    no_midi_ports: bool,
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
            "--logic-view" => a.logic_view = Some(val("--logic-view")?),
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
            // (26 oct.) macOS : pas de ports MIDI virtuels (midi_ports.rs) ; sans effet ailleurs
            "--no-midi-ports" => a.no_midi_ports = true,
            // (29 oct.) macOS : lue avant parse_args (midi_ports::early) ; ailleurs, pas de gardien a arreter
            #[cfg(not(target_os = "macos"))]
            "--stop-midi-ports" => {
                println!("No MIDI ports keeper on this system (macOS only).");
                std::process::exit(0);
            }
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
                // (29 oct.) ecrit sans println! : une sortie fermee en route (`--license | head`) n'est pas une panique
                use std::io::Write as _;
                let mut t = format!(
                    "Uppwind Link companion {}\n{NOTICE}\nSource code: {}\n\n{LICENSE_TEXT}\n\n{EXCEPTION_TEXT}\n\n{SCRIPTS_LICENSE_TEXT}\n\n{NOTICES_TEXT}\n",
                    env!("CARGO_PKG_VERSION"),
                    env!("CARGO_PKG_REPOSITORY")
                );
                // (29 oct.) les plugins que cet exe embarque et installe : leurs notices voyagent avec eux
                if !plugin_install::notices().is_empty() {
                    t.push_str(&format!("\n{PLUGINS_EXCEPTION_TEXT}\n\n{}\n", plugin_install::notices()));
                }
                let mut out = std::io::stdout().lock();
                let _ = out.write_all(t.as_bytes());
                let _ = out.flush();
                std::process::exit(0);
            }
            "-h" | "--help" => {
                println!("Uppwind Link companion {}. {NOTICE}", env!("CARGO_PKG_VERSION"));
                println!("uppwind-companion [--list | --probe TITLE [--seconds S]] [--port N] [--new-token] [--capture wgc|dwm|print] [--dev-origins] [--allow-origin URL] [--studio-origin URL|none] [--state-file PATH] [--demo] [--test-window [--test-window-at X,Y] [--test-window-unaware] [--test-window-ghost]] [--bench TITLE [--bench-clicks \"x,y;x,y\"] [--bench-out DIR]] [--background] [--plugin-port N | --no-plugins] [--no-midi-ports (macOS)] [--logic-view show|controls|editor (macOS)]");
                println!("  Studio origin accepted by default: {} (only that one). --dev-origins adds http://localhost and http://127.0.0.1 on ports 5174 and 5747; --allow-origin adds one https:// or local origin (for your own development server only).", security::default_studio_origin());
                println!("uppwind-companion --install | --uninstall | --autostart on|off [--background] | --stop | --pairing-code [--state-file PATH] | --stop-midi-ports (macOS) | --check-update [--download DIR]");
                if cfg!(target_os = "macos") {
                    println!("  macOS: the MIDI ports live in a small helper process that outlives the companion (DAWs keep them across restarts). --stop leaves it running; --stop-midi-ports and --uninstall stop it (the ports go).");
                }
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
    // sans « Enregistrement de l'ecran » (macOS), les titres des fenetres des autres applis sont vides
    #[cfg(not(target_os = "macos"))]
    let (titles, access) = (true, true);
    #[cfg(target_os = "macos")]
    let (titles, access) = {
        // lues sans rien demander (aucune fenetre de macOS ne s'ouvre)
        let (screen, access) = macos::permissions();
        let yn = |b: bool| if b { "allowed" } else { "NOT allowed yet" };
        println!(
            "macOS permissions for this program: Screen Recording {} (window titles and pictures), Accessibility {} (clicks and keys, and the titles of Logic Pro's plugin windows).",
            yn(screen),
            yn(access)
        );
        if !screen || !access {
            println!("  System Settings › Privacy & Security › Screen Recording / Accessibility: add the program that runs the companion (Terminal, or uppwind-companion itself).");
        }
        (screen, access)
    };
    let all = plugins::enumerate();
    // (10 nov.) sous macOS, Logic Pro aussi : il ouvre ses fenetres de plugins lui-meme (un clic sur le plugin, dans sa tranche)
    let (live, logic) = (plugins::live_running(&all), plugins::logic_running(&all));
    if !live && !logic {
        println!("Ableton Live is not running{}.", if cfg!(target_os = "macos") { ", nor Logic Pro" } else { "" });
        return;
    }
    let wins = plugins::plugin_windows(&all);
    if wins.is_empty() && !titles {
        println!("Without Screen Recording, this program cannot read the windows' titles: no plugin window can be recognized.");
    } else if wins.is_empty() {
        if live {
            println!("Live is running, but no plugin window is open (open one in Live: the wrench icon of the device).");
        }
        // (11 nov.) Logic ne donne les titres de ses fenetres de plugins qu'a l'Accessibilite
        if logic && !access {
            println!("Logic Pro names its plugin windows only for Accessibility: without it, none of them can be recognized.");
        } else if logic {
            println!("Logic Pro is running, but no plugin window is open (open one in Logic Pro: click a plugin in a channel strip).");
        }
    }
    for w in wins {
        println!("{:>12}  {}×{}{}  \"{}\"  (plugin {}, track {})", w.id, w.w, w.h, if w.minimized { " minimized" } else { "" }, w.title, w.plugin, w.track);
    }
}

/// (11 nov.) --logic-view : l'affichage de chaque fenetre de plugin de Logic Pro (« show »), ou le passer a « controls »
/// ou « editor », par l'Accessibilite, comme le message « view » du studio. Un diagnostic lance depuis le Terminal : rien
/// d'autre n'est touche.
fn logic_view_cli(what: &str) -> i32 {
    #[cfg(target_os = "macos")]
    {
        let set = match what {
            "show" => None,
            "controls" => Some(true),
            "editor" => Some(false),
            _ => {
                eprintln!("--logic-view show|controls|editor");
                return 2;
            }
        };
        let wins = plugins::logic_plugin_windows(&plugins::enumerate());
        if wins.is_empty() {
            println!("No plugin window of Logic Pro is open (or Screen Recording / Accessibility is missing: --list says).");
            return 1;
        }
        let mut code = 0;
        for w in wins {
            let Some(mw) = macos::window(w.id) else { continue };
            let b = mw.bounds;
            let frame = [b.origin.x, b.origin.y, b.size.width, b.size.height];
            let r = match set {
                None => macos::ax_logic_view(mw.pid, frame).ok_or_else(|| "no Editor / Controls menu found".to_string()),
                Some(c) => macos::ax_set_logic_view(mw.pid, frame, c),
            };
            match r {
                Ok(v) => println!(
                    "{:>12}  \"{}\"  {}{}{}  [{}]",
                    w.id,
                    w.title,
                    if v.controls { "Controls" } else { "the plugin's own view" },
                    if v.opaque { " (opaque to Accessibility: the studio's clicks do not reach it)" } else { "" },
                    // (12 nov., option a)
                    if v.covered { " (partly covered by another Logic Pro window: the studio's clicks do not reach what is covered)" } else { "" },
                    view_cost(&v.cost)
                ),
                Err(e) => {
                    println!("{:>12}  \"{}\"  {e}", w.id, w.title);
                    code = 1;
                }
            }
        }
        code
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = what;
        eprintln!("--logic-view needs macOS (Logic Pro)");
        2
    }
}

/// (12 nov.) Ce qu'ont coute les deux sondes d'une fenetre (--logic-view) : la recherche du bouton de menu de
/// l'affichage, puis le test « opaque », qui s'arrete au premier bouton trouve ou a son delai.
#[cfg(target_os = "macos")]
fn view_cost(c: &macos::ViewCost) -> String {
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
    let stopped = if c.late { ", stopped by its time limit" } else { "" };
    format!(
        "view menu found in {:.1} ms; opaque test: {} of {} points ({} in this window) in {:.1} ms{stopped}",
        ms(c.button),
        c.probed,
        c.points,
        c.in_window,
        ms(c.test)
    )
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

/// (9 nov., audit F1/F2) Les commandes de la console, une ligne puis Entree : « arm » (le studio peut filmer et piloter
/// les fenetres de plugins, et prendre le son du DAW), « disarm », « newcode » (un nouveau code d'appairage : l'ancien ne
/// marche plus). Sans console (en fond), l'entree est vide : le fil s'arrete aussitot.
fn console_commands(sh: Arc<Shared>) {
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            match line.trim().to_ascii_lowercase().as_str() {
                "" => {}
                "arm" => {
                    if !sh.arming.set(true, "the console") {
                        println!("Remote control is already on.");
                    }
                }
                "disarm" => {
                    if !sh.arming.set(false, "the console") {
                        println!("Remote control is already off.");
                    }
                }
                "newcode" | "new code" => {
                    let saved = sh.new_code("the console");
                    println!();
                    println!("NEW PAIRING CODE (paste it in the studio's Plugin window; the old one no longer works):");
                    println!();
                    println!("    {}", security::pairing_code(sh.port, &sh.token()));
                    println!();
                    if let Err(e) = saved {
                        eprintln!("warning: {e}");
                    }
                }
                "discord on" | "discord off" => {
                    let on = line.trim().to_ascii_lowercase().ends_with("on");
                    match sh.set_presence(on, "the console") {
                        Ok(true) => println!("Discord presence is on: what you play in Music shows on your Discord profile (the Discord desktop app must be open)."),
                        Ok(false) => println!("Discord presence is off."),
                        Err(e) => eprintln!("warning: {e}"),
                    }
                }
                // (12 nov.) la surface Mackie Control de Logic Pro (mcu.rs) : permise ici, jamais par le studio
                "mackie on" | "mackie off" => {
                    let on = line.trim().to_ascii_lowercase().ends_with("on");
                    if !sh.mcu.available() {
                        println!("The Logic Pro surface needs the companion on a Mac.");
                    } else {
                        match sh.set_mackie(on, "the console") {
                            Ok(true) => println!("Logic Pro surface allowed. Once armed (arm), the companion opens the MIDI port \"{}\" and Logic Pro adds it by itself as a Mackie Control surface: the studio then shows Logic Pro's mixer, 8 tracks at a time, and can press Play and Stop.", mcu::PORT_NAME),
                            Ok(false) => println!("Logic Pro surface off: the MIDI port \"{}\" closes. Logic Pro keeps the surface in Control Surface Setup until you delete it there.", mcu::PORT_NAME),
                            Err(e) => eprintln!("warning: {e}"),
                        }
                    }
                }
                "help" | "?" => println!("Commands: arm (allow remote control), disarm (stop it), newcode (a new pairing code), discord on / discord off (show what you play in Music on Discord), mackie on / mackie off (Logic Pro's mixer, Play and Stop in the studio, through a Mackie Control surface)."),
                other => println!("Unknown command {:?}. Commands: arm, disarm, newcode, discord on, discord off, mackie on, mackie off, help.", other.chars().take(20).collect::<String>()),
            }
        }
    });
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
                if cfg!(windows) {
                    println!("Start it from the Start Menu: \"Uppwind Link Companion\".");
                } else {
                    println!("Start it from a Terminal: {} (or --autostart on, for every login).", terminal_path(&dirs.exe()));
                }
                println!("The pairing code stays in {}: the studio stays paired.", shown_path(&state));
                if cfg!(windows) {
                    println!("A companion that is already running keeps the old version until you close its window.");
                } else {
                    println!("A companion that is already running keeps the old version until you stop it (Ctrl+C in its Terminal).");
                }
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
    // (29 oct.) les plugins (plugin_install/) : la copie elevee de Windows (seule, avant tout le reste),
    // --install-plugins, et le dossier d'essai --plugins-dir (garde pour --setup, --wizard, --uninstall)
    if let Some(code) = plugin_install::early(&raw) {
        std::process::exit(code);
    }
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
    // (29 oct.) macOS : le gardien des ports MIDI (lance par le compagnon), son arret, son essai (midi_ports.rs) ; apres
    // wizard::early pour le bac a sable, avant tout le reste
    #[cfg(target_os = "macos")]
    if let Some(code) = midi_ports::early(&raw) {
        std::process::exit(code);
    }
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
        let mut stopped = false;
        if tray::single::request_quit(Duration::from_secs(5)) {
            println!("Stopped Uppwind Link.");
            stopped = true;
        }
        let state = args.state_file.clone().unwrap_or_else(wizard::state_file);
        if !stopped {
            match install::stop(&state) {
                Ok(m) => {
                    println!("{m}");
                    stopped = true;
                }
                Err(e) => eprintln!("{e}"),
            }
        }
        // (27 oct.) et ceux lances en console, qui ne prennent pas le verrou de l'icone
        // (12 nov.) sauf en bac a sable (stops_other_companions)
        if stops_other_companions(wizard::sandbox()) {
            for m in install::stop_others() {
                stopped |= m.starts_with("Stopped");
                println!("{m}");
            }
        } else {
            println!("Sandbox: the other companions of this Windows session are left running.");
        }
        std::process::exit(if stopped { 0 } else { 1 });
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
    if let Some(what) = &args.logic_view {
        std::process::exit(logic_view_cli(what));
    }

    let state = args.state_file.clone().unwrap_or_else(wizard::state_file);
    // (29 oct., audit MAC-1) 0600 sous macOS et Linux, avant toute lecture ; ce que le lancement d'avant y avait note
    if let Err(e) = state_file::secure(&state) {
        eprintln!("warning: cannot make {} private ({e})", shown_path(&state));
    }
    let token = load_token(&state, args.new_token);
    let pairing_v2 = state_file::read(&state).map(|v| state_file::pairing_v2(&v, &token)).unwrap_or(false);
    // (11 nov.) la presence Discord : off tant que la personne ne l'a pas permise ici ; son choix d'avant
    let discord_presence = state_file::read(&state).map(|v| v["discordPresence"] == json!(true)).unwrap_or(false);
    // (12 nov.) la surface Mackie Control pour Logic Pro : off tant que la personne ne l'a pas permise ici (« mackie on »)
    let mackie_surface = state_file::read(&state).map(|v| v[mcu::STATE_KEY] == json!(true)).unwrap_or(false);
    // (11 nov., update.rs) la case « Update automatically » : off tant que la personne ne l'a pas cochee ici
    let auto_update = state_file::read(&state).map(|v| v[update::STATE_KEY] == json!(true)).unwrap_or(false);
    // la machine elle-meme, et rien d'autre (un seul socket a l'ecoute, sur 127.0.0.1)
    let listener = match bind(args.port) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Cannot listen on 127.0.0.1 ({e}). Is another companion running?");
            std::process::exit(1);
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    // (27 oct.) un autre compagnon tourne deja : le studio essaie les ports dans l'ordre et peut lui parler a lui
    for (pid, path) in install::other_companions() {
        println!("Another companion is running (pid {pid}, {path}): the studio may connect to it instead of this one (port {port}). Stop them all with: uppwind-companion --stop");
    }
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
    if pairing_v2 {
        st["pairingV2"] = json!(true);
    }
    if discord_presence {
        st["discordPresence"] = json!(true);
    }
    if mackie_surface {
        st[mcu::STATE_KEY] = json!(true);
    }
    if auto_update {
        st[update::STATE_KEY] = json!(true);
    }
    if let Err(e) = state_file::write(&state, &st) {
        eprintln!("warning: cannot write {} ({e})", shown_path(&state));
    }
    // (29 oct.) a la sortie propre, pluginPort est retire du fichier (state_file.rs)
    state_file::at_exit(&state);

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
        println!("    {}", security::pairing_code(port, &token));
    }
    println!();
    println!("Saved in {} (the same code next time; --new-token makes a new one and unpairs the studio).", shown_path(&state));
    println!();
    if args.demo {
        println!("REMOTE CONTROL: on (demo mode: nothing real is captured or controlled).");
    } else {
        // (9 nov., audit F1) le studio ne filme ni ne pilote rien tant que la personne devant l'ordinateur ne l'a pas permis
        println!("REMOTE CONTROL: off. The studio cannot capture plugin windows, send them gestures or take your DAW's sound");
        println!("until you allow it here:");
        let how = if !to_log {
            "type  arm  and Enter (disarm stops it; newcode makes a new pairing code)"
        } else if cfg!(windows) {
            "the status window or the icon's menu (Remote control)"
        } else {
            "run the companion in a Terminal and type  arm  (in the background it stays off)"
        };
        println!("    {how}");
        println!("It turns itself off after {} minutes without the studio, and at every start.", arming::IDLE_DISARM.as_secs() / 60);
    }
    println!();
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
    #[cfg(target_os = "macos")]
    if !args.no_midi_ports && !args.demo {
        // (29 oct.) les ports vivent dans le gardien, qui survit au compagnon (FL Studio les garde d'un redemarrage a
        // l'autre) ; deja la : rien n'est cree ; le gardien ne vient pas : ici, comme avant (midi_ports.rs)
        let o = midi_ports::open(&midi_ports::default_names(), &midi_ports::keeper_dir());
        for l in &o.lines {
            println!("{l}");
        }
        for w in &o.warnings {
            eprintln!("warning: {w}");
        }
    }
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
        token: RwLock::new(token),
        allow,
        guard: Mutex::new(security::PairingGuard::new(5, Duration::from_secs(30))),
        capture: args.capture.clone(),
        demo: args.demo,
        test_window,
        pairing_v2: std::sync::atomic::AtomicBool::new(pairing_v2),
        state: Some(state.clone()),
        // (9 nov., audit F1) desarme a chaque lancement ; --demo : arme (rien de reel n'est filme ni pilote)
        arming: arming::Arming::new(args.demo, !args.demo),
        revoked: AtomicU64::new(0),
        presence: discord::Presence::new(discord_presence),
        // (12 nov.) sur un Mac (et en --demo, une fausse table, sans port MIDI) ; permise si la personne l'a permise ici
        mcu: mcu::Hub::new(cfg!(target_os = "macos") || args.demo, mackie_surface || args.demo),
    });
    println!(
        "Discord presence (what you play in Music, on your Discord profile): {}",
        // (11 nov., 0.4.0) un Mac n'a pas de fenetre d'etat : seulement la console
        if discord_presence {
            "ON (type « discord off » to stop it)"
        } else if cfg!(target_os = "macos") {
            "off (type « discord on » to allow it)"
        } else {
            "off (type « discord on » to allow it, or use the status window)"
        }
    );
    // (12 nov.) Logic Pro : la surface Mackie Control ; ses ports seulement permise (« mackie on ») et armee
    #[cfg(target_os = "macos")]
    if !args.demo {
        println!(
            "Logic Pro surface (Mackie Control, MIDI port \"{}\"): {}",
            mcu::PORT_NAME,
            if mackie_surface { "allowed: its port opens once you type arm" } else { "off (type « mackie on » to allow it)" }
        );
        mcu_port::start(shared.clone());
    }
    if args.demo {
        mcu::demo::start(shared.clone());
    }
    // (9 nov., audit F1/F2) dans une console : « arm », « disarm », « newcode » (sans console, la fenetre d'etat et l'icone)
    if !to_log {
        console_commands(shared.clone());
    }
    // (24 oct.) avec l'icone : l'icone et la fenetre d'etat sur un fil a elles (STA pour WebView2 ; ce fil-ci garde son
    // MTA) ; quand l'icone quitte, tout s'arrete. Le code n'y est que montre (la fenetre, le bouton Copy), jamais ecrit.
    #[cfg(windows)]
    if let (Some(mode), Some(guard)) = (app, guard) {
        let sh = shared.clone();
        std::thread::spawn(move || {
            let code = tray::run(mode, guard, sh);
            state_file::on_exit();
            std::process::exit(code)
        });
    }
    // un fil par connexion, au plus session::MAX_CONNECTIONS a la fois (comptees des l'acceptation)
    session::serve(listener, shared, session::MAX_CONNECTIONS);
}

/// (12 nov.) `--stop` arrete aussi les compagnons lances en console (install::stop_others), sauf en bac a sable :
/// `--stop --sandbox DIR` n'arrete que le compagnon de ce dossier (son icone, son fichier d'etat). Avant, il arretait
/// tous les autres de la session Windows, le compagnon installe de l'utilisateur compris (le 11 nov., pendant un essai).
fn stops_other_companions(sandbox: Option<&std::path::Path>) -> bool {
    sandbox.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn stop_in_a_sandbox_leaves_the_other_companions_running() {
        assert!(stops_other_companions(None), "outside a sandbox, --stop also stops the companions started in a console");
        assert!(!stops_other_companions(Some(Path::new(r"E:\tmp\bac"))), "--stop --sandbox DIR stops only the companion of DIR");
    }

    #[test]
    fn the_pairing_code_comes_from_the_state_file() {
        let t = "0123456789abcdef".repeat(4);
        assert_eq!(pairing_code_of(&json!({"port": 47411, "token": t, "pid": 7}).to_string()), Some(format!("UWC2-47411-{t}")));
        assert_eq!(pairing_code_of(&json!({"port": 47411, "token": "short"}).to_string()), None);
        assert_eq!(pairing_code_of(&json!({"token": t}).to_string()), None, "no port");
        assert_eq!(pairing_code_of("not json"), None);
    }

    #[test]
    fn shown_paths_hide_the_account_name() {
        let home = Path::new(r"C:\Users\someone");
        assert_eq!(shown_path_in(Path::new(r"C:\Users\someone\.uppwind-companion.json"), home), format!(r"{HOME_SHOWN}\.uppwind-companion.json"));
        assert_eq!(shown_path_in(Path::new(r"c:\users\SOMEONE\AppData\Local\Uppwind Link\x.exe"), home), format!(r"{HOME_SHOWN}\AppData\Local\Uppwind Link\x.exe"));
        assert_eq!(shown_path_in(Path::new("/Users/someone/Library/x"), Path::new("/Users/someone")), format!("{HOME_SHOWN}/Library/x"));
        assert_eq!(shown_path_in(Path::new(r"C:\Users\someone2\x"), home), r"C:\Users\someone2\x", "another account");
        assert_eq!(shown_path_in(Path::new(r"D:\state.json"), home), r"D:\state.json");
    }

    /// (10 nov.) macOS : la ligne a coller dans Terminal garde le ~ hors des guillemets (zsh et bash ne remplacent pas
    /// un ~ entre guillemets : `"~/Library/…"` ne se trouve pas)
    #[cfg(not(windows))]
    #[test]
    fn a_path_to_paste_in_terminal_keeps_the_tilde_outside_the_quotes() {
        let home = Path::new("/Users/someone");
        let exe = Path::new("/Users/someone/Library/Application Support/Uppwind Link/uppwind-companion");
        assert_eq!(terminal_path_in(exe, home), r#"~/"Library/Application Support/Uppwind Link/uppwind-companion""#);
        assert_eq!(terminal_path_in(Path::new("/private/tmp/sb/uppwind-companion"), home), r#""/private/tmp/sb/uppwind-companion""#);
        assert_eq!(terminal_path_in(Path::new("/Users/someone2/x"), home), r#""/Users/someone2/x""#, "another account");
        assert_eq!(terminal_path_in(home, home), r#""/Users/someone""#);
    }
}
