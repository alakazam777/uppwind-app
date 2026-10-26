// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! --fl-settings : les reglages MIDI et audio de FL Studio, ecrits dans le registre (24 oct.). Le principe : ils
//! s'appliquent a l'installation, l'utilisateur est prevenu qu'il peut revenir en arriere, et la desinstallation les
//! retire.
//!
//! Ce qui est ecrit, sous HKCU\Software\Image-Line\FL Studio <NN>, seulement les valeurs qui ne sont pas deja bonnes :
//!   Devices\MIDI input\Uppwind Link        Enabled 1 · Type -3 (un script de l'utilisateur) · ScriptFolder
//!                                          « Uppwind Link » · Port 77 (FL l'affiche « Uppwind Link (user) »)
//!   Devices\MIDI output\Uppwind Link Back  Port 77 (le meme numero : FL y envoie les reponses du script)
//!   Devices\MIDI output\Uppwind Link       Port -1 (avec un numero, FL renvoie dans le port qu'il lit)
//!   Devices\MIDI input\Uppwind Link Back   Enabled 0
//!   General\MIDIForm                       CloseDSBox 0 (la case « Auto close » des reglages audio, voir fl.rs)
//!   et, a part, avec --fl-free-midi all|<nom> : Devices\MIDI input\<peripherique USB> Enabled 0 (Windows ne laisse
//!   qu'une appli ouvrir un peripherique USB : tenu par FL, le navigateur ne peut pas l'ouvrir).
//! Le port : celui que l'entree a deja s'il est libre, sinon 77, sinon le suivant libre. Le type de chaque valeur est
//! garde (FL range tout en texte, REG_SZ) ; une cle que FL n'a pas encore (il n'a jamais vu le port) est creee.
//!
//! Regles :
//!   - FL ferme seulement : il ecrit ces cles en quittant, il ecraserait tout. Verifie juste avant d'ecrire ;
//!   - avant d'ecrire : la cle Devices entiere (et General\MIDIForm si elle change) exportee au format de regedit dans
//!     %LOCALAPPDATA%\Uppwind Link\backups\fl-registry-<horodatage>.reg, la copie relue ; puis ce qui va changer
//!     (avant / apres) dans fl-registry-<horodatage>.json a cote, ecrit AVANT la premiere valeur ;
//!   - chaque valeur est relue ; si une ecriture echoue, celles deja faites sont remises ;
//!   - --remove --fl-settings (et --uninstall) remet les valeurs d'avant, seulement celles qui ont encore la valeur du
//!     compagnon : une valeur changee depuis dans FL est laissee, et dite. Une cle creee est retiree si FL n'y a rien
//!     mis. Le .reg reste : `reg import <fichier>` remet toute la cle Devices a la main.

use super::env::{same_value, Probe, RegValue, Registry, REG_BINARY, REG_DWORD, REG_SZ};
use super::fl::{AudioOut, Fl, Settings, AUTO_CLOSE_KEY, AUTO_CLOSE_VALUE, FOLDER, USER_SCRIPT_TYPE};
use super::{is_back_port, is_link_port, Mode, Opts};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_PORT: i64 = 77;
pub const KIND: &str = "uppwind-link-fl-settings";
const PREFIX: &str = "fl-registry-";
pub const OPT_SETTINGS: &str = "--fl-settings";
pub const OPT_FREE_MIDI: &str = "--fl-free-midi";

/// Une valeur a ecrire (ou ecrite).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    /// « midi », « audio », ou « usb » (un peripherique USB rendu au navigateur)
    pub group: String,
    /// la cle complete (HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI output\Uppwind Link)
    pub key: String,
    pub name: String,
    /// None : la valeur n'existait pas
    pub before: Option<RegValue>,
    pub after: RegValue,
    /// en clair
    pub text: String,
    /// l'option qui l'ecrit : --fl-settings ou --fl-free-midi
    #[serde(default)]
    pub opt_in: String,
    /// l'option est donnee : --apply l'ecrit
    #[serde(default)]
    pub chosen: bool,
}

/// Un peripherique USB active dans FL (le navigateur ne peut pas l'ouvrir tant que FL le tient).
#[derive(Debug, Clone, Serialize)]
pub struct UsbInput {
    pub name: String,
    /// branche en ce moment (dans les ports MIDI de Windows)
    pub plugged: bool,
    pub chosen: bool,
}

/// Ce que le compagnon a ecrit une fois : le .json a cote du .reg.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub kind: String,
    pub companion: String,
    /// l'horodatage de l'ecriture (UTC, 20261024-093012)
    pub written_at: String,
    /// HKCU\Software\Image-Line\FL Studio 26
    pub fl_key: String,
    /// la cle Devices entiere (et General\MIDIForm) avant l'ecriture
    pub backup_reg: PathBuf,
    /// writing (interrompu en route), written, rolled-back (rien n'a change), restored
    pub state: String,
    /// les cles que FL n'avait pas (retirees au retour si FL n'y a rien mis), les parentes d'abord
    #[serde(default)]
    pub created_keys: Vec<String>,
    pub changes: Vec<Change>,
    #[serde(default)]
    pub restored_at: Option<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Une ecriture precedente (un .json du dossier des sauvegardes).
#[derive(Debug, Clone, Serialize)]
pub struct Previous {
    pub manifest: PathBuf,
    pub backup_reg: PathBuf,
    pub written_at: String,
    pub state: String,
    pub changes: usize,
    pub restored_at: Option<String>,
}

impl Previous {
    fn pending(&self) -> bool {
        self.restored_at.is_none() && (self.state == "written" || self.state == "writing")
    }
}

/// Ce que --remove --fl-settings fait d'une valeur ecrite.
#[derive(Debug, Clone, Serialize)]
pub struct RestoreLine {
    pub manifest: PathBuf,
    pub key: String,
    pub name: String,
    pub now: Option<RegValue>,
    pub back_to: Option<RegValue>,
    /// restore (remise), already (deja comme avant), left (changee depuis : laissee), key (une cle creee, a retirer)
    pub action: String,
    pub text: String,
}

/// Le rapport : ce que --fl-settings ecrirait (ou remettrait), et ce qu'il a fait.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FlSettings {
    /// il y a de quoi faire : FL et ses reglages (installer), une ecriture a defaire (retirer)
    pub available: bool,
    /// HKCU\Software\Image-Line\FL Studio 26
    pub key: Option<String>,
    pub port: Option<i64>,
    /// --fl-settings (ou oui a la question)
    pub requested: bool,
    /// none, offer (l'option manque), blocked (FL tourne), write, restore
    pub action: String,
    pub reason: String,
    /// FL tourne (son exe) : rien n'est ecrit
    pub blocked_by: Option<String>,
    pub changes: Vec<Change>,
    pub usb_inputs: Vec<UsbInput>,
    /// des noms de --fl-free-midi qui ne sont pas des peripheriques USB actives dans FL
    pub usb_unknown: Vec<String>,
    pub backup_reg: Option<PathBuf>,
    pub manifest: Option<PathBuf>,
    pub previous: Vec<Previous>,
    pub restore: Vec<RestoreLine>,
    /// ce qui reste a regler dans FL, a la main (pas ecrit)
    pub manual: Vec<String>,
    /// apres --apply : fait
    pub done: bool,
    /// ce qu'on dit a l'utilisateur apres coup (ce qui a change, ou est la copie, comment revenir en arriere)
    pub message: Vec<String>,
}

/// Une valeur en texte, pour les gens : « "77" », « (none) ».
pub fn show(v: Option<&RegValue>) -> String {
    match v {
        None => "(none)".into(),
        Some(v) => format!("\"{}\"", v.text()),
    }
}

/// La cle sans la racine de FL : « Devices\MIDI output\Uppwind Link ».
pub fn rel<'a>(key: &'a str, root: &str) -> &'a str {
    match (key.get(..root.len()), key.get(root.len()..)) {
        (Some(head), Some(tail)) if head.eq_ignore_ascii_case(root) && tail.len() > 1 && tail.starts_with('\\') => &tail[1..],
        _ => key,
    }
}

/// « 20261024-093012 » → « 2026-10-24 09:30 UTC ».
pub fn pretty_stamp(s: &str) -> String {
    if s.len() >= 13 && s.as_bytes()[8] == b'-' {
        format!("{}-{}-{} {}:{} UTC", &s[0..4], &s[4..6], &s[6..8], &s[9..11], &s[11..13])
    } else {
        s.to_string()
    }
}

struct Planner<'a> {
    reg: &'a dyn Registry,
    out: Vec<Change>,
    opt_in: &'static str,
    chosen: bool,
}

impl Planner<'_> {
    /// Un nombre, du meme type que la valeur en place (FL : du texte).
    fn num(&mut self, group: &str, key: &str, name: &str, n: i64, text: String) {
        let before = self.reg.value(key, name);
        let after = match before {
            Some(RegValue::Num(_)) => RegValue::Num(n),
            _ => RegValue::Str(n.to_string()),
        };
        self.push(group, key, name, before, after, text);
    }
    fn text(&mut self, group: &str, key: &str, name: &str, s: &str, text: String) {
        let before = self.reg.value(key, name);
        self.push(group, key, name, before, RegValue::Str(s.into()), text);
    }
    fn push(&mut self, group: &str, key: &str, name: &str, before: Option<RegValue>, after: RegValue, text: String) {
        if !same_value(before.as_ref(), Some(&after)) {
            self.out.push(Change { group: group.into(), key: key.into(), name: name.into(), before, after, text, opt_in: self.opt_in.into(), chosen: self.chosen });
        }
    }
}

pub struct Plan {
    pub port: i64,
    pub changes: Vec<Change>,
    pub usb: Vec<UsbInput>,
    pub usb_unknown: Vec<String>,
}

/// Les valeurs a ecrire sous `root` (…\FL Studio 26) pour que FL parle au studio ; `base_chosen` : --fl-settings ;
/// `free_midi` : les peripheriques USB a rendre au navigateur (« all » ou des noms).
pub fn plan_install(reg: &dyn Registry, root: &str, s: &Settings, probe: &Probe, base_chosen: bool, free_midi: &[String]) -> Plan {
    let dev = format!(r"{root}\Devices");
    let link_in = s.inputs.iter().find(|i| i.name == "Uppwind Link").or_else(|| s.inputs.iter().find(|i| is_link_port(&i.name)));
    let back_in = s.inputs.iter().find(|i| i.name == "Uppwind Link Back").or_else(|| s.inputs.iter().find(|i| is_back_port(&i.name)));
    let back_out = s.outputs.iter().find(|o| o.name == "Uppwind Link Back").or_else(|| s.outputs.iter().find(|o| is_back_port(&o.name)));
    let link_out = s.outputs.iter().find(|o| o.name == "Uppwind Link").or_else(|| s.outputs.iter().find(|o| is_link_port(&o.name)));
    // les noms des cles : ceux que FL a deja, sinon ceux des ports de Windows, sinon ceux du guide (FL ne les a pas
    // encore vus : il trouvera ses reglages quand loopMIDI les creera)
    let in_name = link_in.map(|i| i.name.clone()).or_else(|| probe.midi_in.iter().find(|n| is_link_port(n)).cloned()).unwrap_or_else(|| "Uppwind Link".into());
    let out_name = back_out.map(|o| o.name.clone()).or_else(|| probe.midi_out.iter().find(|n| is_back_port(n)).cloned()).unwrap_or_else(|| "Uppwind Link Back".into());
    let ours_in = |n: &str| n == in_name || back_in.is_some_and(|b| b.name == n);
    let ours_out = |n: &str| n == out_name || link_out.is_some_and(|l| l.name == n);
    let taken = |p: i64| s.inputs.iter().any(|i| !ours_in(&i.name) && i.port == p) || s.outputs.iter().any(|o| !ours_out(&o.name) && o.port == p);
    let port = [link_in.map(|i| i.port), back_out.map(|o| o.port)]
        .into_iter()
        .flatten()
        .find(|p| *p >= 0 && !taken(*p))
        .or_else(|| (DEFAULT_PORT..=255).chain(0..DEFAULT_PORT).find(|p| !taken(*p)))
        .unwrap_or(DEFAULT_PORT);

    let mut p = Planner { reg, out: Vec::new(), opt_in: OPT_SETTINGS, chosen: base_chosen };
    let ik = format!(r"{dev}\MIDI input\{in_name}");
    let i = format!("Input \"{in_name}\"");
    p.num("midi", &ik, "Enabled", 1, format!("{i}: enabled"));
    p.num("midi", &ik, "Type", USER_SCRIPT_TYPE, format!("{i}: controller type \"Uppwind Link (user)\" (a user script)"));
    p.text("midi", &ik, "ScriptFolder", FOLDER, format!("{i}: its script is the folder \"{FOLDER}\""));
    p.num("midi", &ik, "Port", port, format!("{i}: port {port}"));
    p.num("midi", &format!(r"{dev}\MIDI output\{out_name}"), "Port", port, format!("Output \"{out_name}\": port {port}, the input's number (FL sends the script's replies there)"));
    if let Some(o) = link_out.filter(|o| o.port >= 0 && o.name != out_name) {
        p.num("midi", &format!(r"{dev}\MIDI output\{}", o.name), "Port", -1, format!("Output \"{}\": no port number (with {}, FL sends back into the port it reads and loopMIDI turns red)", o.name, o.port));
    }
    if let Some(b) = back_in.filter(|b| b.enabled && b.name != in_name) {
        p.num("midi", &format!(r"{dev}\MIDI input\{}", b.name), "Enabled", 0, format!("Input \"{}\": disabled (the studio listens to it, not FL)", b.name));
    }
    if reg.key_exists(&format!(r"{root}\General")) {
        p.num(
            "audio",
            &format!(r"{root}\{AUTO_CLOSE_KEY}"),
            AUTO_CLOSE_VALUE,
            0,
            "Audio settings: \"Auto close\" unticked (ticked, FL lets go of the audio device when the browser is in front, and the studio hears nothing)".into(),
        );
    }

    let wanted = |n: &str| free_midi.iter().any(|f| f.eq_ignore_ascii_case("all") || f.eq_ignore_ascii_case(n));
    let usb: Vec<UsbInput> = s
        .inputs
        .iter()
        .filter(|i| i.hardware && i.enabled && !is_link_port(&i.name) && !is_back_port(&i.name))
        .map(|i| UsbInput { name: i.name.clone(), plugged: probe.midi_in.iter().any(|n| n == &i.name), chosen: wanted(&i.name) })
        .collect();
    p.opt_in = OPT_FREE_MIDI;
    for u in &usb {
        p.chosen = u.chosen;
        p.num(
            "usb",
            &format!(r"{dev}\MIDI input\{}", u.name),
            "Enabled",
            0,
            format!("Input \"{}\": disabled in FL, so the browser can open it (Windows lets one app at a time open a USB MIDI device; the studio forwards what you play to FL)", u.name),
        );
    }
    let usb_unknown = free_midi.iter().filter(|f| !f.eq_ignore_ascii_case("all") && !usb.iter().any(|u| u.name.eq_ignore_ascii_case(f))).cloned().collect();
    Plan { port, changes: p.out, usb, usb_unknown }
}

/// fl-registry-<horodatage>[-n].reg et .json, des noms libres.
fn backup_paths(dir: &Path, stamp: &str) -> (PathBuf, PathBuf) {
    let mut n = 1;
    loop {
        let base = if n == 1 { format!("{PREFIX}{stamp}") } else { format!("{PREFIX}{stamp}-{n}") };
        let (r, m) = (dir.join(format!("{base}.reg")), dir.join(format!("{base}.json")));
        if !r.exists() && !m.exists() {
            return (r, m);
        }
        n += 1;
    }
}

/// L'ordre des ecritures : l'horodatage, puis le -n.
fn order_key(p: &Previous) -> (String, u32) {
    let stem = p.manifest.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let n = stem.trim_start_matches(PREFIX).splitn(3, '-').nth(2).and_then(|x| x.parse().ok()).unwrap_or(1);
    (p.written_at.clone(), n)
}

pub fn load(path: &Path) -> Result<Manifest, String> {
    let s = std::fs::read_to_string(path).map_err(|e| format!("cannot read {} ({e})", path.display()))?;
    serde_json::from_str(&s).map_err(|e| format!("{} is not a companion record ({e})", path.display()))
}

fn save(path: &Path, m: &Manifest) -> Result<(), String> {
    std::fs::write(path, serde_json::to_string_pretty(m).unwrap()).map_err(|e| format!("cannot write {} ({e})", path.display()))
}

/// Les ecritures precedentes du compagnon, la plus recente d'abord.
pub fn previous(dir: &Path) -> Vec<Previous> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<Previous> = rd
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n.starts_with(PREFIX) && n.ends_with(".json")
        })
        .filter_map(|e| load(&e.path()).ok().filter(|m| m.kind == KIND).map(|m| (e.path(), m)))
        .map(|(path, m)| Previous { manifest: path, backup_reg: m.backup_reg, written_at: m.written_at, state: m.state, changes: m.changes.len(), restored_at: m.restored_at })
        .collect();
    v.sort_by_key(|p| std::cmp::Reverse(order_key(p)));
    v
}

/// Le rapport de --setup : le plan d'ecriture (installer) ou de retour (retirer), rien n'est ecrit.
pub fn prepare(reg: &dyn Registry, base: &str, fl: &Fl, probe: &Probe, o: &Opts, backups: &Path, stamp: &str) -> FlSettings {
    let mut x = FlSettings { requested: o.fl_settings == Some(true), blocked_by: fl.running.clone(), previous: previous(backups), ..Default::default() };
    match o.mode {
        Mode::Install => prepare_install(&mut x, reg, base, fl, probe, o, backups, stamp),
        Mode::Remove => prepare_restore(&mut x, reg),
    }
    x
}

#[allow(clippy::too_many_arguments)]
fn prepare_install(x: &mut FlSettings, reg: &dyn Registry, base: &str, fl: &Fl, probe: &Probe, o: &Opts, backups: &Path, stamp: &str) {
    let Some(s) = fl.settings.as_ref().filter(|_| fl.found()) else {
        x.action = "none".into();
        x.reason = if fl.found() { "FL Studio has not saved its MIDI settings yet: start FL once, close it, then run the setup again" } else { "FL Studio is not installed" }.into();
        return;
    };
    let root = format!(r"{base}\{}", s.key);
    let plan = plan_install(reg, &root, s, probe, o.fl_settings == Some(true), &o.free_midi);
    x.available = true;
    x.key = Some(root);
    x.port = Some(plan.port);
    x.changes = plan.changes;
    x.usb_inputs = plan.usb;
    x.usb_unknown = plan.usb_unknown;
    if !x.changes.is_empty() {
        let (r, m) = backup_paths(backups, stamp);
        x.backup_reg = Some(r);
        x.manifest = Some(m);
    }
    let chosen = x.changes.iter().filter(|c| c.chosen).count();
    let (action, reason) = if x.changes.is_empty() {
        ("none", "FL Studio's MIDI and audio settings are already right".to_string())
    } else if chosen == 0 {
        ("offer", format!("{OPT_SETTINGS} writes them, FL Studio closed; a backup is made first"))
    } else if let Some(p) = &x.blocked_by {
        ("blocked", format!("FL Studio is running ({p}): close FL Studio first (FL saves its settings when it closes and would overwrite these)"))
    } else {
        ("write", format!("{chosen} value{} to write", if chosen == 1 { "" } else { "s" }))
    };
    x.action = action.into();
    x.reason = reason;
    // (24 oct.) le son revient par le compagnon : FL joue par Windows vers une sortie qu'on n'ecoute pas ; le cable n'est
    // qu'un secours (sa frequence ne compte qu'avec lui)
    let out = super::fl::audio_out(s);
    if !matches!(out, AudioOut::Cable | AudioOut::Windows) {
        x.manual.push("Options › Audio settings: FL Studio ASIO (Show ASIO panel › Output) or Primary Sound Driver, to an output you don't listen to — the companion brings FL's sound back to the studio (not written: it changes where you hear FL)".into());
    }
    if let Some(r) = s.sample_rate.filter(|r| *r != 48000 && out == AudioOut::Cable) {
        x.manual.push(format!("Options › Audio settings: sample rate 48000 Hz, the cable's (it is {r} Hz; not written: your audio device has to support it)"));
    }
}

fn restore_plan(reg: &dyn Registry, m: &Manifest, path: &Path) -> Vec<RestoreLine> {
    let mut out = Vec::new();
    for c in m.changes.iter().rev() {
        let now = reg.value(&c.key, &c.name);
        let what = format!("{} · {}", rel(&c.key, &m.fl_key), c.name);
        let (action, text) = if same_value(now.as_ref(), Some(&c.after)) {
            ("restore", format!("{what}: back to {} (the companion wrote {})", show(c.before.as_ref()), show(Some(&c.after))))
        } else if same_value(now.as_ref(), c.before.as_ref()) {
            ("already", format!("{what}: already {}, as before", show(now.as_ref())))
        } else {
            ("left", format!("{what}: now {}, changed in FL since the companion wrote {}: left as it is", show(now.as_ref()), show(Some(&c.after))))
        };
        out.push(RestoreLine { manifest: path.to_path_buf(), key: c.key.clone(), name: c.name.clone(), now, back_to: c.before.clone(), action: action.into(), text });
    }
    for k in m.created_keys.iter().rev().filter(|k| reg.key_exists(k)) {
        out.push(RestoreLine {
            manifest: path.to_path_buf(),
            key: k.clone(),
            name: String::new(),
            now: None,
            back_to: None,
            action: "key".into(),
            text: format!("{}: the companion created this key; removed if FL has stored nothing in it", rel(k, &m.fl_key)),
        });
    }
    out
}

fn prepare_restore(x: &mut FlSettings, reg: &dyn Registry) {
    let pending: Vec<Previous> = x.previous.iter().filter(|p| p.pending()).cloned().collect();
    for p in &pending {
        match load(&p.manifest) {
            Ok(m) => {
                x.key.get_or_insert(m.fl_key.clone());
                x.restore.extend(restore_plan(reg, &m, &p.manifest));
            }
            Err(e) => x.manual.push(e),
        }
    }
    x.available = !pending.is_empty();
    let (action, reason) = if pending.is_empty() {
        ("none", "the companion has not changed FL Studio's settings: nothing to put back".to_string())
    } else if !x.requested {
        ("offer", format!("--remove {OPT_SETTINGS} puts back the values the companion wrote"))
    } else if let Some(p) = &x.blocked_by {
        ("blocked", format!("FL Studio is running ({p}): close FL Studio first (FL saves its settings when it closes and would overwrite them)"))
    } else {
        let n = x.restore.iter().filter(|l| l.action == "restore").count();
        ("restore", format!("{n} value{} to put back", if n == 1 { "" } else { "s" }))
    };
    x.action = action.into();
    x.reason = reason;
}

// --- la sauvegarde .reg ---------------------------------------------------------------------------------------------

fn long_root(path: &str) -> String {
    let (root, rest) = path.split_once('\\').unwrap_or((path, ""));
    let r = match root.to_ascii_uppercase().as_str() {
        "HKCU" => "HKEY_CURRENT_USER",
        "HKLM" => "HKEY_LOCAL_MACHINE",
        other => return format!("{other}\\{rest}"),
    };
    if rest.is_empty() {
        r.to_string()
    } else {
        format!("{r}\\{rest}")
    }
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn value_line(name: &str, ty: u32, data: &[u8]) -> String {
    let n = if name.is_empty() { "@".to_string() } else { format!("\"{}\"", esc(name)) };
    if ty == REG_SZ {
        if let RegValue::Str(s) = RegValue::from_raw(ty, data) {
            // en clair seulement si le texte redonne exactement les memes octets
            if !s.contains(['\r', '\n', '\0']) && RegValue::Str(s.clone()).to_raw().1 == data {
                return format!("{n}=\"{}\"", esc(&s));
            }
        }
    }
    if ty == REG_DWORD && data.len() == 4 {
        return format!("{n}=dword:{:08x}", u32::from_le_bytes([data[0], data[1], data[2], data[3]]));
    }
    let mut line = if ty == REG_BINARY { format!("{n}=hex:") } else { format!("{n}=hex({ty:x}):") };
    let mut col = line.len();
    for (i, b) in data.iter().enumerate() {
        if i > 0 {
            line.push(',');
            col += 1;
            if col > 76 {
                line.push_str("\\\r\n  ");
                col = 2;
            }
        }
        line.push_str(&format!("{b:02x}"));
        col += 2;
    }
    line
}

fn export_key(reg: &dyn Registry, path: &str, out: &mut String) {
    if !reg.key_exists(path) {
        return;
    }
    out.push_str(&format!("\r\n[{}]\r\n", long_root(path)));
    for n in reg.value_names(path) {
        if let Some((ty, data)) = reg.raw(path, &n) {
            out.push_str(&value_line(&n, ty, &data));
            out.push_str("\r\n");
        }
    }
    for s in reg.subkeys(path) {
        export_key(reg, &format!(r"{path}\{s}"), out);
    }
}

/// Les cles `roots`, leurs valeurs et leurs sous-cles, au format de regedit (`reg import` le relit).
pub fn export_reg(reg: &dyn Registry, roots: &[String]) -> String {
    let mut out = String::from("Windows Registry Editor Version 5.00\r\n");
    for r in roots {
        export_key(reg, r, &mut out);
    }
    out
}

/// En UTF-16 avec sa marque, comme regedit.
pub fn reg_file_bytes(text: &str) -> Vec<u8> {
    let mut b = vec![0xFF, 0xFE];
    b.extend(text.encode_utf16().flat_map(|c| c.to_le_bytes()));
    b
}

/// Ce que la sauvegarde contient : toute la cle Devices, et les autres cles touchees (General\MIDIForm).
fn areas(root: &str, changes: &[Change]) -> Vec<String> {
    let dev = format!(r"{root}\Devices");
    let mut a = vec![dev.clone()];
    for c in changes {
        if rel(&c.key, &dev) == c.key && !c.key.eq_ignore_ascii_case(&dev) && !a.iter().any(|x| x.eq_ignore_ascii_case(&c.key)) {
            a.push(c.key.clone());
        }
    }
    a
}

fn parent(key: &str) -> &str {
    key.rsplit_once('\\').map(|(p, _)| p).unwrap_or("")
}

/// Remet `changes` (dans l'ordre inverse) et retire les cles creees restees vides → ce qui n'a pas pu l'etre.
fn roll_back(reg: &dyn Registry, changes: &[Change], created: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    for c in changes.iter().rev() {
        let r = match &c.before {
            None => reg.delete_value(&c.key, &c.name),
            Some(b) => reg.set_value(&c.key, &c.name, b),
        };
        if let Err(e) = r {
            problems.push(e);
        }
    }
    for k in created.iter().rev() {
        if reg.key_exists(k) && reg.subkeys(k).is_empty() && reg.value_names(k).is_empty() {
            if let Err(e) = reg.delete_key(k) {
                problems.push(e);
            }
        }
    }
    problems
}

/// --apply --fl-settings : FL ferme, la sauvegarde, le .json, puis les valeurs choisies, chacune relue → ce qui a ete fait.
/// `running` regarde (juste avant d'ecrire) si FL tourne.
pub fn write(reg: &dyn Registry, x: &mut FlSettings, running: &dyn Fn() -> Option<String>, stamp: &str) -> Result<Vec<String>, String> {
    let chosen: Vec<Change> = x.changes.iter().filter(|c| c.chosen).cloned().collect();
    if chosen.is_empty() {
        return Ok(vec![]);
    }
    if let Some(p) = running() {
        x.blocked_by = Some(p.clone());
        x.action = "blocked".into();
        return Err(format!("FL Studio is running ({p}): close FL Studio first, then run the setup again. Nothing was written (FL saves its settings when it closes and would overwrite them)."));
    }
    let root = x.key.clone().ok_or("FL Studio's registry key is unknown")?;
    let (reg_path, man_path) = match (&x.backup_reg, &x.manifest) {
        (Some(r), Some(m)) => (r.clone(), m.clone()),
        _ => return Err("no backup file planned".into()),
    };
    for c in &chosen {
        let now = reg.value(&c.key, &c.name);
        if !same_value(now.as_ref(), c.before.as_ref()) {
            return Err(format!("{} · {} changed since it was read ({} → {}): run the setup again. Nothing was written.", rel(&c.key, &root), c.name, show(c.before.as_ref()), show(now.as_ref())));
        }
    }
    // 1. la copie de toute la cle Devices (et des autres cles touchees), relue
    if reg_path.exists() || man_path.exists() {
        return Err(format!("{} already exists: run the setup again. Nothing was written.", reg_path.display()));
    }
    if let Some(d) = reg_path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("cannot create {} ({e}). Nothing was written.", d.display()))?;
    }
    let bytes = reg_file_bytes(&export_reg(reg, &areas(&root, &chosen)));
    std::fs::write(&reg_path, &bytes).map_err(|e| format!("cannot write the backup {} ({e}). Nothing was written.", reg_path.display()))?;
    if std::fs::read(&reg_path).ok().as_deref() != Some(bytes.as_slice()) {
        return Err(format!("the backup {} does not read back as written. Nothing was written.", reg_path.display()));
    }
    // 2. ce qui va changer, avant la premiere valeur (interrompu en route, --remove sait quoi remettre)
    let mut m = Manifest {
        kind: KIND.into(),
        companion: env!("CARGO_PKG_VERSION").into(),
        written_at: stamp.into(),
        fl_key: root.clone(),
        backup_reg: reg_path.clone(),
        state: "writing".into(),
        created_keys: vec![],
        changes: chosen.clone(),
        restored_at: None,
        notes: vec![],
    };
    for c in &chosen {
        let mut missing = Vec::new();
        let mut k = c.key.as_str();
        while k.len() > root.len() && !reg.key_exists(k) {
            missing.push(k.to_string());
            k = parent(k);
        }
        for mk in missing.into_iter().rev() {
            if !m.created_keys.contains(&mk) {
                m.created_keys.push(mk);
            }
        }
    }
    save(&man_path, &m).map_err(|e| format!("{e}. Nothing was written."))?;
    // 3. les valeurs, chacune relue
    for (i, c) in chosen.iter().enumerate() {
        let r = reg.set_value(&c.key, &c.name, &c.after).and_then(|_| {
            if same_value(reg.value(&c.key, &c.name).as_ref(), Some(&c.after)) {
                Ok(())
            } else {
                Err(format!("{} · {} does not read back as written", rel(&c.key, &root), c.name))
            }
        });
        if let Err(e) = r {
            let problems = roll_back(reg, &chosen[..=i], &m.created_keys);
            m.state = "rolled-back".into();
            m.notes = problems.clone();
            let _ = save(&man_path, &m);
            return Err(if problems.is_empty() {
                format!("{e}. The values already written were put back: FL's settings are as before (backup {}).", reg_path.display())
            } else {
                format!("{e}. Putting the values back failed too ({}): close FL Studio and run reg import \"{}\".", problems.join("; "), reg_path.display())
            });
        }
    }
    m.state = "written".into();
    save(&man_path, &m)?;
    x.done = true;
    x.reason = format!("{} value{} written, FL's settings from before are in {}", chosen.len(), if chosen.len() == 1 { "" } else { "s" }, reg_path.display());
    let mut lines = vec![format!("FL Studio settings written in {root} (FL reads them when it starts):")];
    lines.extend(chosen.iter().map(|c| format!("  {} ({} · {}: {} → {})", c.text, rel(&c.key, &root), c.name, show(c.before.as_ref()), show(Some(&c.after)))));
    lines.push(format!("  backup of the settings from before: {}", reg_path.display()));
    lines.push(format!("  what was changed: {}", man_path.display()));
    x.message = message_after_write(&chosen, &reg_path);
    Ok(lines)
}

fn message_after_write(chosen: &[Change], reg_path: &Path) -> Vec<String> {
    let mut v = vec!["The companion changed these FL Studio settings, so FL talks to Uppwind Studio:".to_string()];
    v.extend(chosen.iter().map(|c| format!("  - {}", c.text)));
    v.push(format!("A copy of FL's MIDI settings from before is in {}.", reg_path.display()));
    v.push("Uninstalling the companion (uppwind-companion --uninstall) puts these settings back as they were, and only these: what you change in FL in the meantime stays yours.".into());
    v.push("To put them back now: uppwind-companion --setup --remove --fl-settings --apply.".into());
    v.push(format!("To restore all of FL's MIDI settings exactly as they were (this also undoes what you changed since): close FL Studio, then run  reg import \"{}\"", reg_path.display()));
    v
}

/// --remove --fl-settings --apply (et --uninstall) : remet les valeurs ecrites par le compagnon, la plus recente ecriture
/// d'abord, seulement celles qui ont encore sa valeur → (ce qui a ete fait, les erreurs).
pub fn restore(reg: &dyn Registry, x: &mut FlSettings, running: &dyn Fn() -> Option<String>, stamp: &str) -> (Vec<String>, Vec<String>) {
    let (mut lines, mut errors) = (Vec::new(), Vec::new());
    let pending: Vec<Previous> = x.previous.iter().filter(|p| p.pending()).cloned().collect();
    if pending.is_empty() {
        return (lines, errors);
    }
    if let Some(p) = running() {
        x.blocked_by = Some(p.clone());
        x.action = "blocked".into();
        errors.push(format!("FL Studio is running ({p}): close FL Studio first. Its settings were not put back (FL saves them when it closes and would overwrite them)."));
        return (lines, errors);
    }
    let mut backups = Vec::new();
    for p in &pending {
        let mut m = match load(&p.manifest) {
            Ok(m) => m,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let mut notes = Vec::new();
        let mut failed = false;
        for c in m.changes.iter().rev() {
            let now = reg.value(&c.key, &c.name);
            let what = format!("{} · {}", rel(&c.key, &m.fl_key), c.name);
            if same_value(now.as_ref(), Some(&c.after)) {
                let r = match &c.before {
                    None => reg.delete_value(&c.key, &c.name),
                    Some(b) => reg.set_value(&c.key, &c.name, b),
                }
                .and_then(|_| {
                    if same_value(reg.value(&c.key, &c.name).as_ref(), c.before.as_ref()) {
                        Ok(())
                    } else {
                        Err(format!("{what} does not read back as put back"))
                    }
                });
                match r {
                    Ok(()) => notes.push(format!("put back {what}: {}", show(c.before.as_ref()))),
                    Err(e) => {
                        failed = true;
                        errors.push(e);
                    }
                }
            } else if same_value(now.as_ref(), c.before.as_ref()) {
                notes.push(format!("{what}: already {}, as before", show(now.as_ref())));
            } else {
                notes.push(format!("left {what}: it is {} now, changed in FL since the companion wrote {}", show(now.as_ref()), show(Some(&c.after))));
            }
        }
        for k in m.created_keys.iter().rev().filter(|k| reg.key_exists(k)) {
            if reg.subkeys(k).is_empty() && reg.value_names(k).is_empty() {
                match reg.delete_key(k) {
                    Ok(()) => notes.push(format!("removed the key {} (the companion had created it)", rel(k, &m.fl_key))),
                    Err(e) => errors.push(e),
                }
            } else {
                notes.push(format!("kept the key {}: FL has stored its own values in it", rel(k, &m.fl_key)));
            }
        }
        lines.extend(notes.iter().cloned());
        if !failed {
            m.state = "restored".into();
            m.restored_at = Some(stamp.into());
            m.notes = notes;
            if let Err(e) = save(&p.manifest, &m) {
                errors.push(e);
            }
        }
        backups.push(m.backup_reg.clone());
    }
    x.done = errors.is_empty();
    let n = lines.iter().filter(|l| l.starts_with("put back")).count();
    x.reason = format!("{n} value{} put back", if n == 1 { "" } else { "s" });
    x.message = vec!["FL Studio's settings are back as they were before the companion changed them (only the values it wrote; what you changed in FL since stays).".into()];
    for b in backups {
        x.message.push(format!("The full copy from before stays in {}: close FL Studio and run  reg import \"{}\"  to put back all of FL's MIDI settings as they were then.", b.display(), b.display()));
    }
    (lines, errors)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::env::{Endpoint, FakeRegistry, Paths};
    use super::super::fl;
    use super::*;
    use std::collections::BTreeMap;

    const FL26: &str = r"HKCU\Software\Image-Line\FL Studio 26";

    /// Un registre de FL typique (synthetique, tel que `reg query` le lit) : tout en texte, la sortie « Uppwind Link » a
    /// le port 77, un controleur USB est actif (« Pad Controller »), un clavier USB ne l'est plus (« Keys 49 »), un
    /// autre script de l'utilisateur a ses ports (« Other Script »), « Auto close » decoche.
    pub fn sample_fl_registry(base: &str) -> Vec<(String, &'static str, &'static str)> {
        let d = format!(r"{base}\FL Studio 26\Devices");
        let mut v: Vec<(String, &str, &str)> = Vec::new();
        let mut add = |k: String, vals: &[(&'static str, &'static str)]| {
            for (n, x) in vals {
                v.push((k.clone(), n, x));
            }
        };
        add(format!(r"{d}\Audio output"), &[("Device name", "1FL Studio ASIO"), ("Sample rate", "48000"), ("Buffer length (DS)", "6144")]);
        add(format!(r"{d}\MIDI input\Pad Controller"), &[("Port", "-1"), ("Type", "28"), ("Enabled", "1"), ("ScriptFolder", ""), ("IDString", "00 11 22 33 44 55 66 77 88 99"), ("LinkedOutName", "Pad Controller")]);
        add(format!(r"{d}\MIDI input\Other Script RX"), &[("Type", "-3"), ("ScriptFolder", "Other Script"), ("IDString", "-"), ("Port", "40"), ("Enabled", "1")]);
        add(format!(r"{d}\MIDI input\Keys 49"), &[("Type", "51"), ("ScriptFolder", ""), ("IDString", "00 01 02 03 04 05 06 07"), ("Port", "5"), ("Enabled", "0")]);
        add(format!(r"{d}\MIDI input\Uppwind Link"), &[("Type", "-3"), ("ScriptFolder", "Uppwind Link"), ("IDString", "-"), ("ConnectionCounter", "16"), ("Port", "77"), ("Enabled", "1")]);
        add(format!(r"{d}\MIDI input\Uppwind Link Back"), &[("Type", "0"), ("ScriptFolder", ""), ("IDString", "-"), ("Port", "-1"), ("Enabled", "0")]);
        add(format!(r"{d}\MIDI output\Other Script TX"), &[("Port", "40"), ("Sync", "0")]);
        add(format!(r"{d}\MIDI output\Keys 49"), &[("Port", "5"), ("Sync", "1")]);
        add(format!(r"{d}\MIDI output\Microsoft MIDI Mapper"), &[("Port", "200")]);
        add(format!(r"{d}\MIDI output\Uppwind Link"), &[("Port", "77"), ("Sync", "0"), ("SendAllNotesOff", "1"), ("IDString", "-")]);
        add(format!(r"{d}\MIDI output\Uppwind Link Back"), &[("Port", "77"), ("Sync", "0"), ("SendAllNotesOff", "1"), ("IDString", "-")]);
        add(format!(r"{base}\FL Studio 26\General"), &[("Run counter", "12")]);
        add(format!(r"{base}\FL Studio 26\{AUTO_CLOSE_KEY}"), &[("CloseDSBox", "0"), ("HardBufBox", "1")]);
        add(format!(r"{base}\ASIO"), &[("outputEndPoint", "{0.0.0.00000000}.{00000001}"), ("bufferSize", "256")]);
        v
    }

    pub fn fake(rows: &[(String, &str, &str)]) -> FakeRegistry {
        let mut r = FakeRegistry::default();
        for (k, n, v) in rows {
            r.set(k, n, RegValue::Str(v.to_string()));
        }
        r.set(r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\FL Studio 2026", "DisplayName", RegValue::Str("FL Studio 2026".into()));
        r.set(r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\FL Studio 2026", "DisplayVersion", RegValue::Str("26.1.0.1000".into()));
        r
    }

    pub fn probe(root: &Path, reg: FakeRegistry) -> Probe {
        let mut p = Probe::empty(Paths::in_root(root));
        p.reg = Box::new(reg);
        p.midi_in = vec!["Pad Controller".into(), "Uppwind Link".into(), "Uppwind Link Back".into()];
        p.midi_out = vec!["Uppwind Link".into(), "Uppwind Link Back".into()];
        p.render = vec![Endpoint::new("CABLE Input (VB-Audio Virtual Cable)", 48000, "{0.0.0.00000000}.{cable}")];
        p
    }

    fn opts(fl_settings: Option<bool>, free: &[&str], mode: Mode) -> Opts {
        Opts { fl_settings, free_midi: free.iter().map(|s| s.to_string()).collect(), mode, ..Default::default() }
    }

    fn plan_of(p: &Probe, o: &Opts, stamp: &str) -> FlSettings {
        let f = fl::detect(p);
        prepare(p.reg.as_ref(), fl::IL, &f, p, o, &p.paths.backups(), stamp)
    }

    fn get(p: &Probe, key: &str, name: &str) -> Option<String> {
        p.reg.value(key, name).map(|v| v.text())
    }

    fn not_running() -> Option<String> {
        None
    }

    /// Un .reg relu (les essais) : cle (en minuscules) → valeur → texte.
    pub fn parse_reg(bytes: &[u8]) -> BTreeMap<String, BTreeMap<String, String>> {
        assert_eq!(&bytes[..2], &[0xFF, 0xFE], "UTF-16 with its byte order mark");
        let w: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let text = String::from_utf16(&w).unwrap().replace("\\\r\n  ", "");
        let mut lines = text.split("\r\n");
        assert_eq!(lines.next(), Some("Windows Registry Editor Version 5.00"));
        let mut out: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        let mut cur = String::new();
        for l in lines.filter(|l| !l.is_empty()) {
            if l.starts_with('[') && l.ends_with(']') {
                cur = l[1..l.len() - 1].to_ascii_lowercase();
                out.entry(cur.clone()).or_default();
                continue;
            }
            // "nom"=… (ou @=… pour la valeur par defaut) : le nom entre guillemets, \\ et \" echappes
            let mut chars = l.chars();
            let (mut name, mut esc_on) = (String::new(), false);
            match chars.next() {
                Some('@') => {}
                Some('"') => {
                    for c in chars.by_ref() {
                        match (esc_on, c) {
                            (false, '\\') => esc_on = true,
                            (false, '"') => break,
                            (_, c) => {
                                name.push(c);
                                esc_on = false;
                            }
                        }
                    }
                }
                other => panic!("not a value line ({other:?}): {l}"),
            }
            let rest: String = chars.collect();
            let v = rest.strip_prefix('=').unwrap();
            let val = if let Some(s) = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                s.replace("\\\"", "\"").replace("\\\\", "\\")
            } else if let Some(h) = v.strip_prefix("dword:") {
                (u32::from_str_radix(h, 16).unwrap() as i32).to_string()
            } else {
                v.split_once(':').unwrap().1.replace(',', " ")
            };
            out.get_mut(&cur).unwrap().insert(name, val);
        }
        out
    }

    #[test]
    fn on_a_typical_machine_one_value_and_one_option() {
        let root = super::super::tests::temp_root("fls-typical");
        let p = probe(&root, fake(&sample_fl_registry(fl::IL)));
        let x = plan_of(&p, &opts(None, &[], Mode::Install), "20261024-101500");
        assert!(x.available);
        assert_eq!(x.key.as_deref(), Some(FL26));
        assert_eq!(x.port, Some(77), "the input already has 77, nobody else uses it");
        let base: Vec<&Change> = x.changes.iter().filter(|c| c.group != "usb").collect();
        assert_eq!(base.len(), 1, "{base:#?}");
        assert_eq!(base[0].key, format!(r"{FL26}\Devices\MIDI output\Uppwind Link"));
        assert_eq!((base[0].name.as_str(), base[0].before.clone(), base[0].after.clone()), ("Port", Some(RegValue::Str("77".into())), RegValue::Str("-1".into())), "kept as text, like FL");
        assert!(!base[0].chosen && base[0].opt_in == OPT_SETTINGS);
        assert_eq!(x.action, "offer");
        // le controleur USB : une option a part ; le clavier (Keys 49) est deja desactive
        assert_eq!(x.usb_inputs.iter().map(|u| u.name.as_str()).collect::<Vec<_>>(), vec!["Pad Controller"]);
        let usb: Vec<&Change> = x.changes.iter().filter(|c| c.group == "usb").collect();
        assert_eq!(usb.len(), 1);
        assert!(!usb[0].chosen && usb[0].opt_in == OPT_FREE_MIDI && usb[0].name == "Enabled");
        assert!(x.backup_reg.as_ref().unwrap().starts_with(p.paths.backups()));
        assert!(x.backup_reg.as_ref().unwrap().ends_with("fl-registry-20261024-101500.reg"));
        assert!(x.manual.is_empty() || x.manual.iter().all(|m| m.contains("ASIO")), "{:?}", x.manual);
        // --fl-settings : choisi ; FL ouvert : bloque
        assert_eq!(plan_of(&p, &opts(Some(true), &[], Mode::Install), "s").action, "write");
        let mut p2 = probe(&root, fake(&sample_fl_registry(fl::IL)));
        p2.processes = vec!["FL64.exe".into()];
        let x2 = plan_of(&p2, &opts(Some(true), &[], Mode::Install), "s");
        assert_eq!((x2.action.as_str(), x2.blocked_by.as_deref()), ("blocked", Some("FL64.exe")));
        assert!(x2.reason.contains("close FL Studio first"), "{}", x2.reason);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_fl_that_never_saw_the_ports_gets_everything() {
        let root = super::super::tests::temp_root("fls-fresh");
        let d = format!(r"{FL26}\Devices");
        let rows = vec![
            (format!(r"{d}\MIDI input\Some Synth"), "Port", "77"),
            (format!(r"{d}\MIDI input\Some Synth"), "Enabled", "1"),
            (format!(r"{d}\MIDI input\Some Synth"), "IDString", "-"),
            (format!(r"{d}\MIDI input\Uppwind Link Back"), "Enabled", "1"),
            (format!(r"{d}\MIDI output\Some Synth"), "Port", "78"),
            (format!(r"{FL26}\General\MIDIForm"), "CloseDSBox", "1"),
        ];
        let p = probe(&root, fake(&rows));
        let x = plan_of(&p, &opts(Some(true), &[], Mode::Install), "20261024-101500");
        assert_eq!(x.port, Some(79), "77 and 78 are taken by another device");
        let find = |k: &str, n: &str| x.changes.iter().find(|c| c.key == format!(r"{d}\{k}") && c.name == n).map(|c| (c.before.clone(), c.after.text()));
        assert_eq!(find(r"MIDI input\Uppwind Link", "Enabled"), Some((None, "1".into())));
        assert_eq!(find(r"MIDI input\Uppwind Link", "Type"), Some((None, "-3".into())));
        assert_eq!(find(r"MIDI input\Uppwind Link", "ScriptFolder"), Some((None, "Uppwind Link".into())));
        assert_eq!(find(r"MIDI input\Uppwind Link", "Port"), Some((None, "79".into())));
        assert_eq!(find(r"MIDI output\Uppwind Link Back", "Port"), Some((None, "79".into())));
        assert_eq!(find(r"MIDI input\Uppwind Link Back", "Enabled").map(|v| v.1), Some("0".into()));
        let ac = x.changes.iter().find(|c| c.name == AUTO_CLOSE_VALUE).unwrap();
        assert_eq!((ac.group.as_str(), ac.after.text()), ("audio", "0".into()));
        assert!(x.changes.iter().all(|c| c.chosen && matches!(c.after, RegValue::Str(_))));
        assert!(x.usb_inputs.is_empty(), "a loopMIDI-like port is not a USB device");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn write_backs_up_then_restore_puts_back_only_our_values() {
        let root = super::super::tests::temp_root("fls-write");
        let rows = sample_fl_registry(fl::IL);
        let p = probe(&root, fake(&rows));
        let mut x = plan_of(&p, &opts(Some(true), &["Pad Controller"], Mode::Install), "20261024-101500");
        assert_eq!(x.changes.iter().filter(|c| c.chosen).count(), 2);
        let lines = write(p.reg.as_ref(), &mut x, &not_running, "20261024-101500").unwrap();
        assert!(x.done);
        assert_eq!(get(&p, &format!(r"{FL26}\Devices\MIDI output\Uppwind Link"), "Port").as_deref(), Some("-1"));
        assert_eq!(get(&p, &format!(r"{FL26}\Devices\MIDI input\Pad Controller"), "Enabled").as_deref(), Some("0"));
        assert_eq!(p.reg.raw(&format!(r"{FL26}\Devices\MIDI output\Uppwind Link"), "Port").unwrap().0, REG_SZ, "the type is kept");
        // la copie : toute la cle Devices telle qu'elle etait, et rien d'autre (General\MIDIForm n'a pas change)
        let reg_path = x.backup_reg.clone().unwrap();
        let back = parse_reg(&std::fs::read(&reg_path).unwrap());
        let mut want: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        for (k, n, v) in rows.iter().filter(|r| r.0.contains(r"\Devices")) {
            want.entry(long_root(k).to_ascii_lowercase()).or_default().insert(n.to_string(), v.to_string());
        }
        for k in [r"HKEY_CURRENT_USER\Software\Image-Line\FL Studio 26\Devices", r"HKEY_CURRENT_USER\Software\Image-Line\FL Studio 26\Devices\MIDI input", r"HKEY_CURRENT_USER\Software\Image-Line\FL Studio 26\Devices\MIDI output"] {
            want.entry(k.to_ascii_lowercase()).or_default();
        }
        assert_eq!(back, want);
        // ce qui a change, et le message
        let m = load(x.manifest.as_ref().unwrap()).unwrap();
        assert_eq!((m.state.as_str(), m.changes.len(), m.backup_reg.clone()), ("written", 2, reg_path.clone()));
        assert!(m.created_keys.is_empty());
        assert!(lines.iter().any(|l| l.contains("backup") && l.contains("fl-registry-20261024-101500.reg")), "{lines:#?}");
        let msg = x.message.join("\n");
        assert!(msg.contains("Uninstalling the companion") && msg.contains("reg import") && msg.contains(&reg_path.display().to_string()), "{msg}");
        // relu : rien a refaire, l'ecriture est listee
        let again = plan_of(&p, &opts(Some(true), &["Pad Controller"], Mode::Install), "20261024-101600");
        assert_eq!(again.action, "none", "{:#?}", again.changes);
        assert_eq!(again.previous.len(), 1);
        // l'utilisateur change la sortie dans FL entre-temps : elle est laissee
        p.reg.set_value(&format!(r"{FL26}\Devices\MIDI output\Uppwind Link"), "Port", &RegValue::Str("12".into())).unwrap();
        let mut r = plan_of(&p, &opts(Some(true), &[], Mode::Remove), "20261024-110000");
        assert_eq!(r.action, "restore");
        let acts: Vec<(&str, &str)> = r.restore.iter().map(|l| (l.name.as_str(), l.action.as_str())).collect();
        assert_eq!(acts, vec![("Enabled", "restore"), ("Port", "left")]);
        let (done, errors) = restore(p.reg.as_ref(), &mut r, &not_running, "20261024-110000");
        assert!(errors.is_empty(), "{errors:?}");
        assert!(done.iter().any(|l| l.starts_with("left") && l.contains("\"12\"")), "{done:#?}");
        assert_eq!(get(&p, &format!(r"{FL26}\Devices\MIDI input\Pad Controller"), "Enabled").as_deref(), Some("1"));
        assert_eq!(get(&p, &format!(r"{FL26}\Devices\MIDI output\Uppwind Link"), "Port").as_deref(), Some("12"), "the user's value stays");
        assert!(r.message.iter().any(|l| l.contains("reg import")));
        let m = load(x.manifest.as_ref().unwrap()).unwrap();
        assert_eq!((m.state.as_str(), m.restored_at.as_deref()), ("restored", Some("20261024-110000")));
        // une fois remis : plus rien a faire
        assert_eq!(plan_of(&p, &opts(Some(true), &[], Mode::Remove), "s").action, "none");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fl_running_means_nothing_is_written_not_even_a_backup() {
        let root = super::super::tests::temp_root("fls-running");
        let p = probe(&root, fake(&sample_fl_registry(fl::IL)));
        let mut x = plan_of(&p, &opts(Some(true), &[], Mode::Install), "20261024-101500");
        assert_eq!(x.action, "write", "FL was closed when it looked");
        let e = write(p.reg.as_ref(), &mut x, &|| Some("FL64.exe".to_string()), "20261024-101500").unwrap_err();
        assert!(e.contains("close FL Studio first"), "{e}");
        assert_eq!(x.action, "blocked");
        assert_eq!(get(&p, &format!(r"{FL26}\Devices\MIDI output\Uppwind Link"), "Port").as_deref(), Some("77"));
        assert!(!p.paths.backups().exists(), "no backup either");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failed_write_puts_everything_back_and_removes_the_new_keys() {
        let root = super::super::tests::temp_root("fls-fail");
        let d = format!(r"{FL26}\Devices");
        let reg = fake(&[(format!(r"{d}\MIDI input\Other"), "Port", "1"), (format!(r"{d}\MIDI output\Other"), "Port", "1")]);
        *reg.fail_on.borrow_mut() = Some("Port".into());
        let p = probe(&root, reg);
        let before = count_values(&p);
        let mut x = plan_of(&p, &opts(Some(true), &[], Mode::Install), "20261024-101500");
        let e = write(p.reg.as_ref(), &mut x, &not_running, "20261024-101500").unwrap_err();
        assert!(e.contains("put back"), "{e}");
        assert!(!p.reg.key_exists(&format!(r"{d}\MIDI input\Uppwind Link")), "the key the companion created is gone");
        assert_eq!(count_values(&p), before);
        let m = load(x.manifest.as_ref().unwrap()).unwrap();
        assert_eq!(m.state, "rolled-back");
        assert_eq!(m.created_keys, vec![format!(r"{d}\MIDI input\Uppwind Link"), format!(r"{d}\MIDI output\Uppwind Link Back")]);
        assert!(!p.reg.key_exists(&format!(r"{d}\MIDI output\Uppwind Link Back")));
        // rien a remettre ensuite
        assert_eq!(plan_of(&p, &opts(Some(true), &[], Mode::Remove), "s").action, "none");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn count_values(p: &Probe) -> usize {
        fn walk(r: &dyn Registry, k: &str) -> usize {
            r.value_names(k).len() + r.subkeys(k).iter().map(|s| walk(r, &format!(r"{k}\{s}"))).sum::<usize>()
        }
        walk(p.reg.as_ref(), r"HKCU\Software")
    }

    #[test]
    fn keys_the_companion_created_go_unless_fl_filled_them() {
        let root = super::super::tests::temp_root("fls-keys");
        let d = format!(r"{FL26}\Devices");
        let p = probe(&root, fake(&[(format!(r"{d}\MIDI input\Other"), "Port", "1")]));
        let mut x = plan_of(&p, &opts(Some(true), &[], Mode::Install), "20261024-101500");
        write(p.reg.as_ref(), &mut x, &not_running, "20261024-101500").unwrap();
        let m = load(x.manifest.as_ref().unwrap()).unwrap();
        assert_eq!(m.created_keys, vec![format!(r"{d}\MIDI input\Uppwind Link"), format!(r"{d}\MIDI output"), format!(r"{d}\MIDI output\Uppwind Link Back")]);
        // FL a vu le port depuis : il a range ses propres valeurs dans l'entree
        p.reg.set_value(&format!(r"{d}\MIDI input\Uppwind Link"), "IDString", &RegValue::Str("-".into())).unwrap();
        let mut r = plan_of(&p, &opts(Some(true), &[], Mode::Remove), "20261024-110000");
        let (done, errors) = restore(p.reg.as_ref(), &mut r, &not_running, "20261024-110000");
        assert!(errors.is_empty(), "{errors:?}");
        assert!(p.reg.key_exists(&format!(r"{d}\MIDI input\Uppwind Link")), "kept: FL's IDString is in it");
        assert_eq!(p.reg.value_names(&format!(r"{d}\MIDI input\Uppwind Link")), vec!["IDString".to_string()], "only FL's own value is left");
        assert!(!p.reg.key_exists(&format!(r"{d}\MIDI output\Uppwind Link Back")) && !p.reg.key_exists(&format!(r"{d}\MIDI output")), "{done:#?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_reg_file_is_what_regedit_writes() {
        let mut r = FakeRegistry::default();
        r.set(r"HKCU\Software\T\A", "Plain", RegValue::Str(r#"C:\x "y""#.into()));
        r.set(r"HKCU\Software\T\A", "Dw", RegValue::Num(-1));
        r.set(r"HKCU\Software\T\A", "", RegValue::Str("default".into()));
        r.set(r"HKCU\Software\T\A\B", "Bin", RegValue::Bin((0..40).collect()));
        let text = export_reg(&r, &[r"HKCU\Software\T".to_string()]);
        assert!(text.starts_with("Windows Registry Editor Version 5.00\r\n\r\n[HKEY_CURRENT_USER\\Software\\T]\r\n"), "{text}");
        assert!(text.contains("\"Plain\"=\"C:\\\\x \\\"y\\\"\"\r\n"), "{text}");
        assert!(text.contains("\"Dw\"=dword:ffffffff\r\n") && text.contains("@=\"default\"\r\n"), "{text}");
        assert!(text.contains("\"Bin\"=hex:00,01,02") && text.contains(",\\\r\n  "), "long binary values wrap like regedit: {text}");
        let back = parse_reg(&reg_file_bytes(&text));
        let a = &back[r"hkey_current_user\software\t\a"];
        assert_eq!((a["Plain"].as_str(), a["Dw"].as_str()), (r#"C:\x "y""#, "-1"));
        assert_eq!(back[r"hkey_current_user\software\t\a\b"]["Bin"].split(' ').count(), 40);
    }

    /// Le vrai registre de Windows, dans une cle d'essai (HKCU\Software\UppwindLinkTest\…, effacee a la fin) : une copie
    /// d'un registre de FL typique (sample_fl_registry), l'ecriture, la copie .reg, le retour. Jamais la vraie cle de FL.
    #[cfg(windows)]
    #[test]
    fn the_real_registry_in_a_test_key() {
        use super::super::env::{write_allowed, TEST_KEY};
        use super::super::winsys::{delete_test_tree, RealRegistry};
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let top = format!(r"{TEST_KEY}\fls-{}-{t}", std::process::id());
        let base = format!(r"{top}\Image-Line");
        assert!(!write_allowed(&format!(r"{FL26}\Devices\MIDI output\Uppwind Link"), true), "the tests can never write FL's real key");
        let reg = RealRegistry;
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                delete_test_tree(&self.0);
            }
        }
        let _cleanup = Cleanup(top.clone());
        for (k, n, v) in sample_fl_registry(&base) {
            reg.set_value(&k, n, &RegValue::Str(v.into())).unwrap();
        }
        let root = super::super::tests::temp_root("fls-real");
        let p = probe(&root, FakeRegistry::default());
        let s = fl::read_settings_at(&reg, &base, 26, &p).unwrap();
        assert_eq!(s.auto_close, Some(false));
        assert!(s.inputs.iter().any(|i| i.name == "Pad Controller" && i.hardware && i.enabled));
        let mut x = FlSettings { key: Some(format!(r"{base}\FL Studio 26")), ..Default::default() };
        let plan = plan_install(&reg, x.key.as_deref().unwrap(), &s, &p, true, &["all".into()]);
        x.changes = plan.changes;
        assert_eq!(x.changes.len(), 2, "{:#?}", x.changes);
        let (rp, mp) = backup_paths(&p.paths.backups(), "20261024-120000");
        x.backup_reg = Some(rp.clone());
        x.manifest = Some(mp.clone());
        write(&reg, &mut x, &not_running, "20261024-120000").unwrap();
        let out = format!(r"{base}\FL Studio 26\Devices\MIDI output\Uppwind Link");
        assert_eq!(reg.raw(&out, "Port"), Some(RegValue::Str("-1".into()).to_raw()), "REG_SZ \"-1\", like FL writes it");
        let back = parse_reg(&std::fs::read(&rp).unwrap());
        assert_eq!(back[&long_root(&out).to_ascii_lowercase()]["Port"], "77", "the backup has the value from before");
        assert_eq!(back.len(), 14, "Devices, Audio output, MIDI input and output, and their 10 device keys: {:?}", back.keys().collect::<Vec<_>>());
        x.previous = previous(&p.paths.backups());
        x.requested = true;
        let (_, errors) = restore(&reg, &mut x, &not_running, "20261024-130000");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(reg.str(&out, "Port").as_deref(), Some("77"));
        assert_eq!(reg.str(&format!(r"{base}\FL Studio 26\Devices\MIDI input\Pad Controller"), "Enabled").as_deref(), Some("1"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
