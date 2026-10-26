// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! FL Studio : les versions installees, le dossier de donnees de l'utilisateur (ou vont les scripts), et les reglages
//! MIDI et audio tels que FL les a ranges dans le registre (lecture seule).
//!
//! FL ecrit ses reglages MIDI dans HKCU\Software\Image-Line\FL Studio <NN>\Devices en quittant seulement :
//!   MIDI input\<port>   Enabled (1/0), Type (-3 = un script de l'utilisateur), ScriptFolder, Port (-1 = aucun)
//!   MIDI output\<port>  Port (le meme numero que l'entree : c'est ainsi que FL relie la sortie au script)
//!   Audio output        Device name, Sample rate
//! et FL Studio ASIO sa sortie dans HKCU\Software\Image-Line\ASIO\outputEndPoint (un id MMDevice).
//! Tout est en texte (REG_SZ), les nombres aussi. IDString : « - » pour un port virtuel (loopMIDI), l'identifiant USB
//! pour un vrai peripherique. La case « Auto close » (Options › Audio settings) est CloseDSBox dans
//! …\FL Studio <NN>\General\MIDIForm : c'est le nom de la case dans le formulaire des reglages de FLEngine_x64.dll
//! (« forms.options.audiopage.audiooutpanel.closedsbox », bulle « Free the audio device for other applications »).
//! Ce que `--setup` en dit vaut pour la derniere fermeture de FL. Les ecritures : fl_settings.rs.

use super::env::{programs, Probe, Program, Registry};
use super::{is_back_port, is_link_port, Check};
use serde::Serialize;
use std::path::PathBuf;

/// Le dossier du script sous Settings\Hardware (FL l'affiche « Uppwind Link (user) »).
pub const FOLDER: &str = "Uppwind Link";
pub const USER_SCRIPT_TYPE: i64 = -3;

#[derive(Debug, Clone, Default, Serialize)]
pub struct MidiIn {
    pub name: String,
    pub enabled: bool,
    pub kind: i64,
    pub script: String,
    pub port: i64,
    /// un vrai peripherique (IDString USB), pas un port virtuel : Windows ne le laisse ouvrir qu'a une appli a la fois
    pub hardware: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct MidiOut {
    pub name: String,
    pub port: i64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Settings {
    /// « FL Studio 26 »
    pub key: String,
    pub inputs: Vec<MidiIn>,
    pub outputs: Vec<MidiOut>,
    pub audio_device: Option<String>,
    pub sample_rate: Option<i64>,
    /// la sortie de FL Studio ASIO, en nom (l'id MMDevice retrouve parmi les peripheriques)
    pub asio_output: Option<String>,
    pub asio_buffer: Option<i64>,
    /// la case « Auto close » des reglages audio (CloseDSBox) ; None : pas encore enregistree
    pub auto_close: Option<bool>,
}

pub const AUTO_CLOSE_KEY: &str = r"General\MIDIForm";
pub const AUTO_CLOSE_VALUE: &str = "CloseDSBox";

#[derive(Debug, Clone, Default, Serialize)]
pub struct Fl {
    pub programs: Vec<Program>,
    pub exe: Option<PathBuf>,
    /// les numeros des cles HKCU\Software\Image-Line\FL Studio <NN> (21, 24, 26…)
    pub registry_versions: Vec<u32>,
    /// le dossier de donnees (« Shared data », sinon Documents\Image-Line)
    pub user_data: Option<PathBuf>,
    pub hardware: Option<PathBuf>,
    pub settings: Option<Settings>,
    pub running: Option<String>,
}

impl Fl {
    pub fn found(&self) -> bool {
        !self.programs.is_empty() || self.exe.is_some()
    }
}

pub const IL: &str = r"HKCU\Software\Image-Line";
const IL_LM: &str = r"HKLM\SOFTWARE\Image-Line";

/// FL tourne-t-il ? (le nom de son exe) — FL64.exe, FL.exe, « FL Studio … ».
pub fn fl_process(processes: &[String]) -> Option<String> {
    processes
        .iter()
        .find(|p| {
            let p = p.to_ascii_lowercase();
            p == "fl64.exe" || p == "fl.exe" || p.starts_with("fl studio")
        })
        .cloned()
}

/// « FL Studio 2026 » / 26.1.7 → 26 ; « FL Studio 21 » → 21.
fn major_of(p: &Program) -> Option<u32> {
    if let Some(m) = p.version.split('.').next().and_then(|v| v.parse::<u32>().ok()).filter(|m| *m > 0) {
        return Some(m);
    }
    let n: u32 = p.name.trim_start_matches("FL Studio").trim().parse().ok()?;
    Some(if n >= 2000 { n - 2000 } else { n })
}

fn is_fl_program(name: &str) -> bool {
    name.strip_prefix("FL Studio ").map(|r| r.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)).unwrap_or(false)
}

pub fn read_settings(reg: &dyn Registry, major: u32, probe: &Probe) -> Option<Settings> {
    read_settings_at(reg, IL, major, probe)
}

/// `base` : HKCU\Software\Image-Line (les essais sur le vrai registre : une copie sous HKCU\Software\UppwindLinkTest).
pub fn read_settings_at(reg: &dyn Registry, base: &str, major: u32, probe: &Probe) -> Option<Settings> {
    let key = format!("FL Studio {major}");
    let dev = format!(r"{base}\{key}\Devices");
    if reg.subkeys(&dev).is_empty() {
        return None;
    }
    let mut s = Settings { key: key.clone(), ..Default::default() };
    let ins = format!(r"{dev}\MIDI input");
    for n in reg.subkeys(&ins) {
        let k = format!(r"{ins}\{n}");
        let id = reg.str(&k, "IDString").unwrap_or_default();
        s.inputs.push(MidiIn {
            enabled: reg.num(&k, "Enabled").unwrap_or(0) == 1,
            kind: reg.num(&k, "Type").unwrap_or(0),
            script: reg.str(&k, "ScriptFolder").unwrap_or_default(),
            port: reg.num(&k, "Port").unwrap_or(-1),
            hardware: !id.trim().is_empty() && id.trim() != "-",
            name: n,
        });
    }
    let outs = format!(r"{dev}\MIDI output");
    for n in reg.subkeys(&outs) {
        let k = format!(r"{outs}\{n}");
        s.outputs.push(MidiOut { port: reg.num(&k, "Port").unwrap_or(-1), name: n });
    }
    let au = format!(r"{dev}\Audio output");
    // « 1FL Studio ASIO » : FL met un chiffre devant le nom (le type de pilote, semble-t-il)
    s.audio_device = reg.str(&au, "Device name").map(|d| d.trim_start_matches(|c: char| c.is_ascii_digit()).to_string());
    s.sample_rate = reg.num(&au, "Sample rate");
    s.auto_close = reg.num(&format!(r"{base}\{key}\{AUTO_CLOSE_KEY}"), AUTO_CLOSE_VALUE).map(|n| n != 0);
    let asio = format!(r"{base}\ASIO");
    if let Some(id) = reg.str(&asio, "outputEndPoint") {
        s.asio_output = Some(
            probe
                .render
                .iter()
                .find(|e| e.id.eq_ignore_ascii_case(&id))
                .map(|e| if e.active { e.name.clone() } else { format!("{} (unplugged or disabled)", e.name) })
                .unwrap_or(format!("unknown device {id}")),
        );
    }
    s.asio_buffer = reg.num(&asio, "bufferSize");
    Some(s)
}

pub fn detect(probe: &Probe) -> Fl {
    let reg = probe.reg.as_ref();
    let mut f = Fl { programs: programs(reg, is_fl_program), ..Default::default() };
    f.exe = reg.str(&format!(r"{IL}\Shared\Paths"), "FL Studio").or_else(|| reg.str(&format!(r"{IL_LM}\Shared\Paths"), "FL Studio")).map(PathBuf::from);
    f.registry_versions = reg.subkeys(IL).iter().filter_map(|k| k.strip_prefix("FL Studio ").and_then(|v| v.parse().ok())).collect();
    f.registry_versions.sort();
    // le dossier de donnees : celui de l'utilisateur d'abord, et seulement s'il existe (HKLM peut nommer un vieux disque)
    let candidates = [reg.str(&format!(r"{IL}\Shared\Paths"), "Shared data"), reg.str(&format!(r"{IL_LM}\Shared\Paths"), "Shared data")];
    f.user_data = candidates.iter().flatten().map(PathBuf::from).find(|p| p.is_dir());
    if f.user_data.is_none() && (f.found() || !f.registry_versions.is_empty()) {
        f.user_data = Some(probe.paths.documents.join("Image-Line"));
    }
    f.hardware = f.user_data.as_ref().map(|d| d.join("FL Studio").join("Settings").join("Hardware"));
    // les reglages de la version installee la plus recente (sinon de la cle la plus recente)
    let major = f.programs.iter().filter_map(major_of).max().or_else(|| f.registry_versions.last().copied());
    f.settings = major.and_then(|m| read_settings(reg, m, probe));
    f.running = fl_process(&probe.processes);
    f
}

/// Ou FL joue, pour le retour du son (24 oct. : par le compagnon, sans cable, sous Windows ; VB-CABLE en secours).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioOut {
    /// dans le cable (VB-CABLE, BlackHole)
    Cable,
    /// par Windows (FL Studio ASIO, Primary Sound Driver…) vers une sortie branchee : le compagnon l'entend
    Windows,
    /// vers une sortie debranchee ou desactivee
    Unplugged,
    /// un autre pilote ASIO (une carte son) : le compagnon ne l'entend pas
    OtherAsio,
    /// pas encore enregistre par FL
    Unknown,
}

pub fn audio_out(s: &Settings) -> AudioOut {
    let dev = s.audio_device.clone().unwrap_or_default();
    let fl_asio = dev.eq_ignore_ascii_case("FL Studio ASIO");
    let out = if fl_asio { s.asio_output.clone().unwrap_or_default() } else { dev.clone() };
    let lo = out.to_ascii_lowercase();
    if lo.contains("cable input") || lo.contains("blackhole") {
        return AudioOut::Cable;
    }
    if dev.is_empty() {
        return AudioOut::Unknown;
    }
    if lo.contains("(unplugged or disabled)") {
        return AudioOut::Unplugged;
    }
    if !fl_asio && dev.to_ascii_lowercase().contains("asio") {
        return AudioOut::OtherAsio;
    }
    AudioOut::Windows
}

pub fn checks(f: &Fl) -> Vec<Check> {
    let mut c = Vec::new();
    let Some(s) = &f.settings else {
        if f.found() {
            c.push(Check::info("fl.settings", "FL Studio's MIDI settings are unknown (FL saves them when it closes)."));
        }
        return c;
    };
    let when = format!("{}, as saved when FL last closed", s.key);
    let link_in = s.inputs.iter().find(|i| i.name == "Uppwind Link").or_else(|| s.inputs.iter().find(|i| is_link_port(&i.name)));
    let port = match link_in {
        None => {
            c.push(Check::todo("fl.input", &format!("Options › MIDI settings: FL has not seen the \"Uppwind Link\" input yet ({when}).")));
            None
        }
        Some(i) if i.enabled && i.kind == USER_SCRIPT_TYPE && i.script == FOLDER && i.port >= 0 => {
            c.push(Check::ok("fl.input", &format!("Input \"{}\": enabled, controller type Uppwind Link (user), port {} ({when}).", i.name, i.port)));
            Some(i.port)
        }
        Some(i) => {
            let mut miss = Vec::new();
            if !i.enabled {
                miss.push("enable it".to_string());
            }
            if i.kind != USER_SCRIPT_TYPE || i.script != FOLDER {
                miss.push("set Controller type to \"Uppwind Link (user)\"".into());
            }
            if i.port < 0 {
                miss.push("give it a Port number (77)".into());
            }
            c.push(Check::todo("fl.input", &format!("Input \"{}\": {} ({when}).", i.name, miss.join(", "))));
            (i.port >= 0).then_some(i.port)
        }
    };
    let back_out = s.outputs.iter().find(|o| o.name == "Uppwind Link Back").or_else(|| s.outputs.iter().find(|o| is_back_port(&o.name)));
    match (back_out, port) {
        (None, _) => c.push(Check::todo("fl.output", "FL has not seen the \"Uppwind Link Back\" output yet.")),
        (Some(o), Some(p)) if o.port == p => c.push(Check::ok("fl.output", &format!("Output \"{}\": port {p}, the same as the input.", o.name))),
        (Some(o), Some(p)) => c.push(Check::todo("fl.output", &format!("Output \"{}\": set Port to {p}, the input's number (it is {}).", o.name, o.port))),
        (Some(o), None) => c.push(Check::todo("fl.output", &format!("Output \"{}\": give it the same Port number as the input.", o.name))),
    }
    if let Some(o) = s.outputs.iter().find(|o| o.name == "Uppwind Link" && o.port >= 0) {
        c.push(Check::warn("fl.loop", &format!("Output \"Uppwind Link\" has port {}: clear it (FL then sends into the port it reads; loopMIDI turns red).", o.port)));
    }
    if let Some(i) = s.inputs.iter().find(|i| is_back_port(&i.name) && i.enabled) {
        c.push(Check::warn("fl.back_input", &format!("Input \"{}\" is enabled: disable it.", i.name)));
    }
    let dev = s.audio_device.clone().unwrap_or_default();
    let out_name = if dev.eq_ignore_ascii_case("FL Studio ASIO") { s.asio_output.clone().unwrap_or_default() } else { dev.clone() };
    let shown = format!("\"{}\"{}", if dev.is_empty() { "?" } else { &dev }, if out_name != dev && !out_name.is_empty() { format!(" › \"{out_name}\"") } else { String::new() });
    let out = audio_out(s);
    match out {
        AudioOut::Cable => c.push(Check::ok("fl.audio", &format!("Audio output: {shown}: the virtual cable{}.", if cfg!(windows) { ", the fallback way back to the studio" } else { "" }))),
        _ if !cfg!(windows) => c.push(Check::todo("fl.audio", &format!("Options › Audio settings: FL plays to {shown}. Set the output device to \"BlackHole 2ch\"."))),
        AudioOut::Windows => c.push(Check::ok(
            "fl.audio",
            &format!("Audio output: {shown}: the companion captures FL's sound from there (check with uppwind-companion --capture-test fl). Pick an output you don't listen to, or you hear it twice."),
        )),
        AudioOut::Unplugged => c.push(Check::todo(
            "fl.audio",
            &format!("Options › Audio settings: FL plays to {shown}: pick an output that is connected and that you don't listen to (FL Studio ASIO › Output, or Primary Sound Driver); the companion brings FL's sound back to the studio."),
        )),
        AudioOut::OtherAsio => c.push(Check::todo(
            "fl.audio",
            &format!("Options › Audio settings: FL uses the ASIO driver {shown}, which the companion cannot hear: pick FL Studio ASIO (Show ASIO panel › an output you don't listen to) or Primary Sound Driver."),
        )),
        AudioOut::Unknown => c.push(Check::info("fl.audio", "Options › Audio settings: FL's output is unknown (FL saves it when it closes). Use FL Studio ASIO or Primary Sound Driver, to an output you don't listen to.")),
    }
    match s.sample_rate {
        Some(48000) => c.push(Check::ok("fl.rate", "Sample rate 48000 Hz.")),
        Some(r) if out == AudioOut::Cable || !cfg!(windows) => c.push(Check::todo("fl.rate", &format!("Sample rate {r} Hz: use 48000 Hz, the cable's rate."))),
        Some(r) => c.push(Check::info("fl.rate", &format!("Sample rate {r} Hz: fine for the companion's audio return (use 48000 Hz if you ever switch to VB-CABLE)."))),
        None => {}
    }
    match s.auto_close {
        Some(false) => c.push(Check::ok("fl.autoclose", "Audio settings: \"Auto close\" is unticked.")),
        Some(true) => c.push(Check::todo("fl.autoclose", "Options › Audio settings: untick \"Auto close\" (ticked, FL lets go of the audio device when the browser is in front).")),
        None => c.push(Check::info("fl.autoclose", "Options › Audio settings: untick \"Auto close\" (FL has not saved it yet).")),
    }
    c
}

#[cfg(test)]
mod tests {
    use super::super::env::{Endpoint, FakeRegistry, Paths};
    use super::super::State;
    use super::*;

    fn probe_with(reg: serde_json::Value, root: &std::path::Path) -> Probe {
        let mut p = Probe::empty(Paths::in_root(root));
        p.reg = Box::new(FakeRegistry::from_json(&reg));
        p.render = vec![Endpoint::new("CABLE Input (VB-Audio Virtual Cable)", 48000, "{0.0.0.00000000}.{cable}")];
        p
    }

    fn sample_registry(data: &std::path::Path) -> serde_json::Value {
        // un registre de FL typique (synthetique) : la sortie « Uppwind Link » a aussi le port 77
        serde_json::json!({
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\FL Studio 2026": { "DisplayName": "FL Studio 2026", "DisplayVersion": "26.1.0.1000" },
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\FL Studio ASIO": { "DisplayName": "FL Studio ASIO" },
            r"HKCU\Software\Image-Line\Shared\Paths": { "Shared data": data.to_string_lossy(), "FL Studio": r"C:\Program Files (x86)\Image-Line\FL Studio 2026\FL64.exe" },
            r"HKLM\SOFTWARE\Image-Line\Shared\Paths": { "Shared data": r"Q:\gone\Image-Line\" },
            r"HKCU\Software\Image-Line\FL Studio 21": {},
            r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI input\Uppwind Link": { "Type": -3, "ScriptFolder": "Uppwind Link", "Port": 77, "Enabled": 1 },
            r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI input\Uppwind Link Back": { "Type": 0, "ScriptFolder": "", "Port": -1, "Enabled": 0 },
            r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI output\Uppwind Link": { "Port": 77 },
            r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI output\Uppwind Link Back": { "Port": 77 },
            r"HKCU\Software\Image-Line\FL Studio 26\Devices\Audio output": { "Device name": "1FL Studio ASIO", "Sample rate": 48000 },
            r"HKCU\Software\Image-Line\ASIO": { "outputEndPoint": "{0.0.0.00000000}.{00000001}", "bufferSize": 256 },
        })
    }

    #[test]
    fn fl_is_found_with_its_user_folder_and_settings() {
        let root = super::super::tests::temp_root("fl");
        let data = root.join("Docs").join("Image-Line");
        std::fs::create_dir_all(&data).unwrap();
        let mut p = probe_with(sample_registry(&data), &root);
        // la sortie de FL Studio ASIO : un casque Bluetooth debranche
        p.render.push(Endpoint { name: "Headphones (Bluetooth)".into(), rate: 44100, id: "{0.0.0.00000000}.{00000001}".into(), active: false });
        let f = detect(&p);
        assert!(f.found());
        assert_eq!(f.registry_versions, vec![21, 26]);
        assert_eq!(f.user_data.as_deref(), Some(data.as_path()), "the HKLM folder does not exist: skipped");
        assert_eq!(f.hardware, Some(data.join("FL Studio").join("Settings").join("Hardware")));
        let s = f.settings.as_ref().unwrap();
        assert_eq!(s.key, "FL Studio 26");
        assert_eq!(s.audio_device.as_deref(), Some("FL Studio ASIO"));
        assert_eq!(s.asio_output.as_deref(), Some("Headphones (Bluetooth) (unplugged or disabled)"));
        let c = checks(&f);
        let state = |id: &str| c.iter().find(|x| x.id == id).map(|x| x.state);
        assert_eq!(state("fl.input"), Some(State::Ok));
        assert_eq!(state("fl.output"), Some(State::Ok));
        assert_eq!(state("fl.loop"), Some(State::Warn), "{c:#?}");
        assert_eq!(state("fl.audio"), Some(State::Todo), "an unplugged output");
        assert_eq!(state("fl.rate"), Some(State::Ok));
        assert_eq!(state("fl.back_input"), None);
        assert_eq!(audio_out(s), AudioOut::Unplugged);
        // une sortie branchee par FL Studio ASIO : le compagnon l'entend, rien a faire (et jamais « CABLE Input »)
        let mut s2 = s.clone();
        s2.asio_output = Some("Speakers (Realtek(R) Audio)".into());
        assert_eq!(audio_out(&s2), AudioOut::Windows);
        let f2 = Fl { settings: Some(s2.clone()), ..f.clone() };
        let c2 = checks(&f2);
        let a = c2.iter().find(|x| x.id == "fl.audio").unwrap();
        assert!(a.state == State::Ok && !a.text.contains("CABLE Input"), "{a:?}");
        s2.audio_device = Some("Focusrite USB ASIO".into());
        assert_eq!(audio_out(&s2), AudioOut::OtherAsio, "a sound card's own ASIO driver");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_asio_output_is_named_and_wrong_ports_are_todos() {
        let root = super::super::tests::temp_root("fl2");
        let mut reg = sample_registry(&root);
        reg[r"HKCU\Software\Image-Line\ASIO"]["outputEndPoint"] = "{0.0.0.00000000}.{CABLE}".into();
        reg[r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI output\Uppwind Link"]["Port"] = (-1).into();
        reg[r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI output\Uppwind Link Back"]["Port"] = 12.into();
        reg[r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI input\Uppwind Link"]["Type"] = 0.into();
        let p = probe_with(reg, &root);
        let f = detect(&p);
        assert_eq!(f.settings.as_ref().unwrap().asio_output.as_deref(), Some("CABLE Input (VB-Audio Virtual Cable)"));
        let c = checks(&f);
        let state = |id: &str| c.iter().find(|x| x.id == id).map(|x| (x.state, x.text.clone()));
        assert_eq!(state("fl.audio").unwrap().0, State::Ok);
        assert_eq!(state("fl.loop"), None);
        let (st, text) = state("fl.input").unwrap();
        assert_eq!(st, State::Todo);
        assert!(text.contains("Controller type"), "{text}");
        let (st, text) = state("fl.output").unwrap();
        assert_eq!(st, State::Todo);
        assert!(text.contains("set Port to 77"), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_fl_means_nothing_to_say() {
        let root = super::super::tests::temp_root("fl3");
        let p = probe_with(serde_json::json!({}), &root);
        let f = detect(&p);
        assert!(!f.found());
        assert!(f.hardware.is_none());
        assert!(checks(&f).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
