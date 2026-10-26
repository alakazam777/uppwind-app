// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Ableton Live : les versions installees, leurs dossiers de preferences, la User Library (Library.cfg), et les
//! reglages que Live a vraiment appliques, lus dans son Log.txt (le seul endroit lisible : Preferences.cfg est un
//! binaire non documente, `--setup` n'y touche pas).
//!
//! Log.txt ecrit a chaque demarrage (et a chaque changement de reglage MIDI) :
//!   … info: AMidiIO: Midi Remote Scripts:
//!     MidiRemoteScript 1 [Control Surface="UppwindLink" Input="Uppwind Link" Output="Uppwind Link Back"]
//!     MidiInDevice [Name="Uppwind Link", Track=true, Sync=false, Remote=true, …]
//!     MidiOutDevice [Name="Uppwind Link Back", Track=false, Sync=false, Remote=true, …]
//!   … info: Audio In Out: Driver Type: MME/DirectX · Output Device: … · Sample Rate: 44100

use super::env::{programs, Probe, Program};
use super::{is_back_port, is_link_port, Check};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Le dossier du script dans la User Library.
pub const FOLDER: &str = "UppwindLink";

#[derive(Debug, Clone, Serialize)]
pub struct Prefs {
    /// « 12.4.3 » (le nom du dossier « Live 12.4.3 »)
    pub version: String,
    pub folder: PathBuf,
    /// la User Library de cette version (Library.cfg), sinon celle par defaut
    pub user_library: PathBuf,
    /// « Library.cfg » ou « default »
    pub user_library_from: &'static str,
    /// secondes depuis 1970 de la derniere ecriture de Log.txt (la version utilisee en dernier a la plus recente)
    pub log_mtime: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Port {
    pub name: String,
    pub track: bool,
    pub remote: bool,
    pub sync: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Surface {
    pub slot: u32,
    pub name: String,
    pub input: String,
    pub output: String,
}

/// Ce que Live a ecrit dans Log.txt la derniere fois.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Settings {
    pub log: PathBuf,
    /// l'horodatage de Live du dernier bloc MIDI (« 2026-10-23T20:09:24 »)
    pub midi_at: String,
    pub surfaces: Vec<Surface>,
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
    pub driver: Option<String>,
    pub output_device: Option<String>,
    pub sample_rate: Option<u32>,
    pub buffer: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Ableton {
    pub programs: Vec<Program>,
    /// les exe trouves sous %ProgramData%\Ableton\*\Program
    pub exes: Vec<PathBuf>,
    pub prefs: Vec<Prefs>,
    /// la version utilisee en dernier (Log.txt le plus recent)
    pub last_used: Option<String>,
    /// les User Library distinctes (plusieurs versions de Live partagent souvent la meme)
    pub user_libraries: Vec<PathBuf>,
    pub settings: Option<Settings>,
    pub running: Option<String>,
}

impl Ableton {
    pub fn found(&self) -> bool {
        !self.programs.is_empty() || !self.exes.is_empty() || !self.prefs.is_empty()
    }
}

fn mtime(p: &Path) -> u64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

fn attr_value(xml: &str, tag: &str) -> Option<String> {
    let at = xml.find(&format!("<{tag} "))?;
    let rest = &xml[at..];
    let v = rest.find("Value=\"")? + 7;
    let end = rest[v..].find('"')?;
    Some(unescape(&rest[v..v + end]))
}

/// La User Library d'un Library.cfg : <UserLibrary> … <ProjectPath Value="D:/Music/Ableton" /> et
/// <ProjectName Value="User Library" /> → D:/Music/Ableton/User Library.
pub fn user_library_from_cfg(xml: &str) -> Option<PathBuf> {
    let at = xml.find("<UserLibrary>")?;
    let end = xml[at..].find("</UserLibrary>").map(|e| at + e).unwrap_or(xml.len());
    let block = &xml[at..end];
    let path = attr_value(block, "ProjectPath").filter(|p| !p.trim().is_empty())?;
    let name = attr_value(block, "ProjectName").filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "User Library".into());
    // Live ecrit « D:/Music/Ableton » meme sous Windows
    let path = if cfg!(windows) { path.replace('/', "\\") } else { path };
    Some(PathBuf::from(path).join(name))
}

pub fn default_user_library(probe: &Probe) -> PathBuf {
    if cfg!(target_os = "macos") {
        crate::home_dir().join("Music").join("Ableton").join("User Library")
    } else {
        probe.paths.documents.join("Ableton").join("User Library")
    }
}

fn parse_bool(fields: &str, key: &str) -> bool {
    fields.split(", ").any(|f| f.trim() == format!("{key}=true"))
}

fn quoted(fields: &str, key: &str) -> String {
    let k = format!("{key}=\"");
    fields.find(&k).and_then(|i| fields[i + k.len()..].find('"').map(|e| fields[i + k.len()..i + k.len() + e].to_string())).unwrap_or_default()
}

/// Les reglages du dernier bloc MIDI et des dernieres lignes audio de Log.txt.
pub fn parse_log(text: &str) -> Settings {
    let mut s = Settings::default();
    if let Some(at) = text.rfind("Midi Remote Scripts:") {
        let line_start = text[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
        s.midi_at = text[line_start..at].split(": ").next().unwrap_or("").split('.').next().unwrap_or("").to_string();
        for line in text[at..].lines().skip(1) {
            let l = line.trim();
            // le bloc s'arrete a la premiere ligne horodatee qui ne le prolonge pas
            if !l.starts_with("Midi") && l.contains(": info: ") && !l.contains("AMidiIO") {
                break;
            }
            if let Some(rest) = l.strip_prefix("MidiRemoteScript ") {
                let slot = rest.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0);
                s.surfaces.push(Surface { slot, name: quoted(rest, "Control Surface"), input: quoted(rest, "Input"), output: quoted(rest, "Output") });
            } else if let Some(rest) = l.strip_prefix("MidiInDevice [") {
                s.inputs.push(Port { name: quoted(rest, "Name"), track: parse_bool(rest, "Track"), remote: parse_bool(rest, "Remote"), sync: parse_bool(rest, "Sync") });
            } else if let Some(rest) = l.strip_prefix("MidiOutDevice [") {
                s.outputs.push(Port { name: quoted(rest, "Name"), track: parse_bool(rest, "Track"), remote: parse_bool(rest, "Remote"), sync: parse_bool(rest, "Sync") });
            }
        }
    }
    let last = |key: &str| text.rfind(key).map(|i| text[i + key.len()..].lines().next().unwrap_or("").trim().to_string());
    s.driver = last("Audio In Out: Driver Type: ");
    s.output_device = last("Audio In Out: Output Device: ");
    s.sample_rate = last("Audio In Out: Sample Rate: ").and_then(|v| v.parse().ok());
    s.buffer = last("Audio In Out: Output Buffer Size: ").and_then(|v| v.split_whitespace().next().and_then(|n| n.parse().ok()));
    s
}

/// Ce qui est installe, sans rien ecrire.
pub fn detect(probe: &Probe) -> Ableton {
    let mut a = Ableton { programs: programs(probe.reg.as_ref(), |n| n.starts_with("Ableton Live ")), ..Default::default() };
    if let Ok(rd) = std::fs::read_dir(probe.paths.program_data.join("Ableton")) {
        for e in rd.flatten() {
            if let Ok(prog) = std::fs::read_dir(e.path().join("Program")) {
                for f in prog.flatten() {
                    let n = f.file_name().to_string_lossy().to_string();
                    if n.starts_with("Ableton Live") && n.to_ascii_lowercase().ends_with(".exe") {
                        a.exes.push(f.path());
                    }
                }
            }
        }
    }
    a.exes.sort();
    let prefs_root = if cfg!(target_os = "macos") { crate::home_dir().join("Library").join("Preferences").join("Ableton") } else { probe.paths.roaming.join("Ableton") };
    if let Ok(rd) = std::fs::read_dir(&prefs_root) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            let Some(v) = n.strip_prefix("Live ") else { continue };
            if !v.starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            // Windows : Live x.y\Preferences ; macOS : Live x.y directement
            let folder = if e.path().join("Preferences").is_dir() { e.path().join("Preferences") } else { e.path() };
            let cfg = std::fs::read_to_string(folder.join("Library.cfg")).ok();
            let (ul, from) = match cfg.as_deref().and_then(user_library_from_cfg) {
                Some(p) => (p, "Library.cfg"),
                None => (default_user_library(probe), "default"),
            };
            a.prefs.push(Prefs { version: v.to_string(), log_mtime: mtime(&folder.join("Log.txt")), folder, user_library: ul, user_library_from: from });
        }
    }
    a.prefs.sort_by(|x, y| super::cmp_versions(&x.version, &y.version));
    if let Some(p) = a.prefs.iter().filter(|p| p.log_mtime > 0).max_by_key(|p| p.log_mtime) {
        a.last_used = Some(p.version.clone());
        if let Ok(bytes) = std::fs::read(p.folder.join("Log.txt")) {
            let mut s = parse_log(&String::from_utf8_lossy(&bytes));
            s.log = p.folder.join("Log.txt");
            a.settings = Some(s);
        }
    }
    for p in &a.prefs {
        if !a.user_libraries.iter().any(|u| super::same_path(u, &p.user_library)) {
            a.user_libraries.push(p.user_library.clone());
        }
    }
    if a.user_libraries.is_empty() && a.found() {
        a.user_libraries.push(default_user_library(probe));
    }
    a.running = probe.running_like(|p| p.starts_with("ableton live") && p.ends_with(".exe"));
    a
}

/// Les reglages de Live compares a ceux qu'il faut (Settings › Tempo & MIDI, Settings › Audio).
pub fn checks(a: &Ableton) -> Vec<Check> {
    let mut c = Vec::new();
    let Some(s) = &a.settings else {
        if a.found() {
            c.push(Check::info("ableton.settings", "Live's settings are unknown (no Log.txt yet: start Live once)."));
        }
        return c;
    };
    let when = format!("as Live {} reported on {}", a.last_used.as_deref().unwrap_or("?"), if s.midi_at.is_empty() { "?" } else { &s.midi_at });
    match s.surfaces.iter().find(|x| x.name == FOLDER) {
        None => c.push(Check::todo("ableton.surface", &format!("Settings › Tempo & MIDI: no Control Surface is UppwindLink ({when}). Pick UppwindLink, Input \"Uppwind Link\", Output \"Uppwind Link Back\"."))),
        Some(x) if is_link_port(&x.input) && is_back_port(&x.output) => {
            c.push(Check::ok("ableton.surface", &format!("Control Surface {} is UppwindLink, Input \"{}\", Output \"{}\" ({when}).", x.slot, x.input, x.output)))
        }
        Some(x) => c.push(Check::warn(
            "ableton.surface",
            &format!("Control Surface {} is UppwindLink but Input is \"{}\" and Output \"{}\": they must be \"Uppwind Link\" and \"Uppwind Link Back\" ({when}).", x.slot, x.input, x.output),
        )),
    }
    match s.inputs.iter().find(|p| is_link_port(&p.name)) {
        None => c.push(Check::todo("ableton.input", "Live did not list an input port \"Uppwind Link\" (create the MIDI ports, then restart Live).")),
        Some(p) if p.track && p.remote => c.push(Check::ok("ableton.input", &format!("Input port \"{}\": Track and Remote on.", p.name))),
        Some(p) => c.push(Check::todo(
            "ableton.input",
            &format!("Input port \"{}\": turn on{}{} (Settings › Tempo & MIDI › Input Ports).", p.name, if p.track { "" } else { " Track" }, if p.remote { "" } else { " Remote" }),
        )),
    }
    match s.outputs.iter().find(|p| is_back_port(&p.name)) {
        None => c.push(Check::todo("ableton.output", "Live did not list an output port \"Uppwind Link Back\".")),
        Some(p) if p.remote => c.push(Check::ok("ableton.output", &format!("Output port \"{}\": Remote on.", p.name))),
        Some(p) => c.push(Check::todo("ableton.output", &format!("Output port \"{}\": turn Remote on.", p.name))),
    }
    if let Some(p) = s.inputs.iter().find(|p| is_back_port(&p.name) && p.track) {
        c.push(Check::warn("ableton.back_input", &format!("Input port \"{}\" has Track on: turn it off (the studio's replies are not notes).", p.name)));
    }
    let dev = s.output_device.clone().unwrap_or_default();
    let cable = dev.to_ascii_lowercase().contains("cable input") || dev.to_ascii_lowercase().contains("blackhole");
    let driver = s.driver.clone().unwrap_or_default();
    // (24 oct.) sous Windows le son revient par le compagnon (sans cable) : Live doit jouer par Windows (pas ASIO), vers
    // une sortie qu'on n'ecoute pas ; VB-CABLE reste un secours. Sur macOS : BlackHole.
    if !cfg!(windows) {
        if cable {
            c.push(Check::ok("ableton.audio", &format!("Audio output: {dev} ({}).", if driver.is_empty() { "?" } else { &driver })));
        } else {
            c.push(Check::todo("ableton.audio", &format!("Settings › Audio: Live plays to \"{}\". Set Audio Output Device \"BlackHole 2ch\".", if dev.is_empty() { "?" } else { &dev })));
        }
    } else if cable {
        c.push(Check::ok("ableton.audio", &format!("Audio output: {dev} ({}): VB-CABLE, the fallback way back to the studio.", if driver.is_empty() { "?" } else { &driver })));
    } else if driver.to_ascii_lowercase().contains("asio") {
        c.push(Check::todo(
            "ableton.audio",
            &format!("Settings › Audio: Live uses ASIO (\"{}\"): the companion cannot hear an ASIO driver. Set Driver Type MME/DirectX, then an Audio Output Device you don't listen to: the companion brings Live's sound back to the studio.", if dev.is_empty() { "?" } else { &dev }),
        ));
    } else if dev.is_empty() {
        c.push(Check::info("ableton.audio", "Settings › Audio: Live's output is unknown (start Live once). Use Driver Type MME/DirectX and an output you don't listen to: the companion brings Live's sound back to the studio."));
    } else {
        c.push(Check::ok(
            "ableton.audio",
            &format!("Audio output: {dev} ({}): the companion captures Live's sound from there. Pick an output you don't listen to, or you hear it twice.", if driver.is_empty() { "?" } else { &driver }),
        ));
    }
    match s.sample_rate {
        Some(48000) => c.push(Check::ok("ableton.rate", "Sample rate 48000 Hz.")),
        Some(r) if cable || !cfg!(windows) => c.push(Check::todo("ableton.rate", &format!("Sample rate {r} Hz: use 48000 Hz, the cable's rate."))),
        Some(r) => c.push(Check::info("ableton.rate", &format!("Sample rate {r} Hz: fine for the companion's audio return (use 48000 Hz if you ever switch to VB-CABLE)."))),
        None => {}
    }
    if let Some(b) = s.buffer {
        if b > 1024 {
            c.push(Check::info("ableton.buffer", &format!("Output buffer {b} samples: 256 (or 512) keeps the round trip short.")));
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "2026-10-16T23:38:18.267365: info: Audio In Out: Driver Type: ASIO\n\
2026-10-16T23:38:18.267396: info: Audio In Out: Output Device: Speakers\n\
2026-10-16T23:38:18.267421: info: Audio In Out: Sample Rate: 44100\n\
2026-10-16T23:38:19.000000: info: AMidiIO: Midi Remote Scripts: \n\
  MidiRemoteScript 1 [Control Surface=\"None\" Input=\"None\" Output=\"None\"]\n\
  MidiInDevice [Name=\"Uppwind Link\", Track=true, Sync=false, Remote=false, MPE=false]\n\
2026-10-22T21:08:37.575594: info: Audio In Out: Driver Type: MME/DirectX\n\
2026-10-22T21:08:37.575640: info: Audio In Out: Output Device: CABLE Input (VB-Audio Virtual Cable) DX\n\
2026-10-22T21:08:37.575676: info: Audio In Out: Sample Rate: 48000\n\
2026-10-22T21:08:37.575685: info: Audio In Out: Output Buffer Size: 256 Samples\n\
2026-10-23T20:09:24.431768: info: AMidiIO: Midi Remote Scripts: \n\
  MidiRemoteScript 1 [Control Surface=\"UppwindLink\" Input=\"Uppwind Link\" Output=\"Uppwind Link Back\"]\n\
  MidiRemoteScript 2 [Control Surface=\"UppwindLinkDev\" Input=\"None\" Output=\"None\"]\n\
2026-10-23T20:09:24.431800: info: AMidiIO: Midi In Devices:\n\
  MidiInDevice [Name=\"Uppwind Link\", Track=true, Sync=false, Remote=true, MPE=false, MIDI Clock Sync Delay=0]\n\
  MidiInDevice [Name=\"Uppwind Link Back\", Track=false, Sync=false, Remote=false, MPE=false]\n\
  MidiOutDevice [Name=\"Uppwind Link Back\", Track=false, Sync=false, Remote=true, MIDI Clock Sync Delay=0]\n\
2026-10-23T20:09:25.000000: info: Python: something else\n\
  MidiInDevice [Name=\"Uppwind Link\", Track=false, Sync=false, Remote=false]\n";

    #[test]
    fn the_last_midi_block_and_audio_lines_of_log_txt_are_read() {
        let s = parse_log(LOG);
        assert_eq!(s.midi_at, "2026-10-23T20:09:24");
        assert_eq!(s.surfaces.len(), 2);
        assert_eq!(s.surfaces[0].name, "UppwindLink");
        assert_eq!(s.surfaces[0].output, "Uppwind Link Back");
        assert_eq!(s.inputs.len(), 2, "the stray line after the block is not part of it: {:?}", s.inputs);
        assert!(s.inputs[0].track && s.inputs[0].remote);
        assert!(s.outputs[0].remote);
        assert_eq!(s.driver.as_deref(), Some("MME/DirectX"));
        assert_eq!(s.output_device.as_deref(), Some("CABLE Input (VB-Audio Virtual Cable) DX"));
        assert_eq!(s.sample_rate, Some(48000));
        assert_eq!(s.buffer, Some(256));
        let a = Ableton { settings: Some(s), last_used: Some("12.4.3".into()), prefs: vec![], ..Default::default() };
        let c = checks(&a);
        assert!(c.iter().all(|x| x.state == super::super::State::Ok), "{c:#?}");
    }

    #[test]
    fn missing_settings_become_todos() {
        let s = parse_log(&LOG[..LOG.find("2026-10-22").unwrap()]);
        let a = Ableton { settings: Some(s), ..Default::default() };
        let c = checks(&a);
        let state = |id: &str| c.iter().find(|x| x.id == id).map(|x| x.state);
        assert_eq!(state("ableton.surface"), Some(super::super::State::Todo));
        assert_eq!(state("ableton.input"), Some(super::super::State::Todo), "Remote off");
        assert_eq!(state("ableton.output"), Some(super::super::State::Todo));
        assert_eq!(state("ableton.audio"), Some(super::super::State::Todo), "ASIO: the companion cannot hear it");
        let want_rate = if cfg!(windows) { super::super::State::Info } else { super::super::State::Todo };
        assert_eq!(state("ableton.rate"), Some(want_rate), "44.1 kHz only matters with the cable");
        // MME/DirectX vers des haut-parleurs : le compagnon capte Live, sans cable
        let mut s = parse_log(&LOG[..LOG.find("2026-10-22").unwrap()]);
        s.driver = Some("MME/DirectX".into());
        let c = checks(&Ableton { settings: Some(s), ..Default::default() });
        let audio = c.iter().find(|x| x.id == "ableton.audio").unwrap();
        if cfg!(windows) {
            assert_eq!(audio.state, super::super::State::Ok, "{}", audio.text);
            assert!(audio.text.contains("companion") && !audio.text.contains("CABLE Input"), "{}", audio.text);
        }
    }

    #[test]
    fn the_user_library_comes_from_library_cfg() {
        let xml = r#"<Ableton><ContentLibrary><UserLibrary><LibraryProject Id="0"><ProjectLocation /><ProjectName Value="User Library" /><ProjectPath Value="D:/Music/Ableton" /></LibraryProject></UserLibrary></ContentLibrary></Ableton>"#;
        let sep = std::path::MAIN_SEPARATOR;
        let s = |p: Option<PathBuf>| p.map(|p| p.to_string_lossy().to_string());
        assert_eq!(s(user_library_from_cfg(xml)), Some(format!("D:{sep}Music{sep}Ableton{sep}User Library")));
        assert_eq!(user_library_from_cfg("<Ableton><UserLibrary><ProjectPath Value=\"\" /></UserLibrary></Ableton>"), None);
        assert_eq!(s(user_library_from_cfg(r#"<UserLibrary><ProjectPath Value="D:/A &amp; B" /></UserLibrary>"#)), Some(format!("D:{sep}A & B{sep}User Library")));
    }
}
