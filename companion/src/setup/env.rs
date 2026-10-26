// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Ce que `--setup` lit de la machine : les dossiers de l'utilisateur, le registre (lecture seule), les ports MIDI, les
//! peripheriques audio, les programmes qui tournent.
//!
//! Tout passe par `Probe` : la vraie machine (winsys.rs), ou une fausse pour les essais. UWC_SETUP_ROOT=<dossier>
//! remplace la machine entiere :
//!   <dossier>\AppData\Roaming, <dossier>\AppData\Local, <dossier>\Documents, <dossier>\ProgramData
//!   <dossier>\registry.json  { "HKCU\\Software\\…\\Cle": { "Valeur": "texte" | nombre }, … }   (absent : vide)
//!   <dossier>\system.json    { "midi_in": [..], "midi_out": [..], "render": [{"name","rate","id"}], "capture": [..],
//!                              "processes": [..] }                                               (absent : vide)
//! Les essais n'ecrivent donc jamais dans les vrais dossiers de Live ou de FL, et ne lisent pas le vrai registre.
//!
//! Le registre ne s'ecrit que par `Registry::set_value` / `delete_value` / `delete_key` (refuses par defaut), et
//! seulement pour --fl-settings (fl_settings.rs). Le vrai registre (winsys.rs) n'accepte que les cles de `write_allowed`
//! (les reglages de FL sous HKCU) ; compile pour les essais, seulement HKCU\Software\UppwindLinkTest. Le faux registre
//! de UWC_SETUP_ROOT reecrit son registry.json.

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const ROOT_ENV: &str = "UWC_SETUP_ROOT";
/// La seule cle ou les essais ecrivent dans le vrai registre (effacee a la fin de chaque essai).
pub const TEST_KEY: &str = r"HKCU\Software\UppwindLinkTest";

/// « HKEY_CURRENT_USER\… » ou « hkcu/… » → « hkcu\… » (pour comparer des chemins).
fn canon(path: &str) -> String {
    let p = path.replace('/', "\\").trim_matches('\\').to_ascii_lowercase();
    for (long, short) in [("hkey_current_user", "hkcu"), ("hkey_local_machine", "hklm")] {
        if let Some(rest) = p.strip_prefix(long) {
            return format!("{short}{rest}");
        }
    }
    p
}

/// Ou le vrai registre peut ecrire. Hors essais : les reglages MIDI de FL (HKCU\Software\Image-Line\FL Studio <NN>\Devices
/// et ses sous-cles) et la case « Auto close » (…\FL Studio <NN>\General\MIDIForm), rien d'autre, jamais HKLM. Pour les
/// essais (`test`) : seulement sous TEST_KEY.
pub fn write_allowed(path: &str, test: bool) -> bool {
    let p = canon(path);
    if p.split('\\').any(|s| s.is_empty() || s == "." || s == "..") {
        return false;
    }
    if test {
        return p.starts_with(&format!("{}\\", canon(TEST_KEY)));
    }
    let Some(rest) = p.strip_prefix("hkcu\\software\\image-line\\fl studio ") else { return false };
    let (num, tail) = rest.split_once('\\').unwrap_or((rest, ""));
    if num.is_empty() || !num.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    tail == "devices" || tail.starts_with("devices\\") || tail == "general\\midiform"
}

/// Les dossiers de l'utilisateur.
#[derive(Debug, Clone, PartialEq)]
pub struct Paths {
    /// %APPDATA% (Roaming) : les preferences de Live
    pub roaming: PathBuf,
    /// %LOCALAPPDATA% : le dossier du compagnon (les sauvegardes)
    pub local: PathBuf,
    /// Documents (dossier connu, il peut etre deplace sur un autre disque)
    pub documents: PathBuf,
    /// %ProgramData% : Live s'installe dessous
    pub program_data: PathBuf,
}

impl Paths {
    pub fn in_root(root: &Path) -> Paths {
        Paths {
            roaming: root.join("AppData").join("Roaming"),
            local: root.join("AppData").join("Local"),
            documents: root.join("Documents"),
            program_data: root.join("ProgramData"),
        }
    }
    /// `get` lit une variable d'environnement ; `documents` donne le dossier Documents connu de Windows.
    pub fn from_lookup(get: impl Fn(&str) -> Option<OsString>, documents: impl Fn() -> Option<PathBuf>) -> Result<Paths, String> {
        if let Some(root) = get(ROOT_ENV).filter(|v| !v.is_empty()) {
            return Ok(Paths::in_root(Path::new(&root)));
        }
        let var = |k: &str| get(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let roaming = var("APPDATA").ok_or("APPDATA is not set")?;
        let local = var("LOCALAPPDATA").ok_or("LOCALAPPDATA is not set")?;
        let home = var("USERPROFILE").or_else(|| var("HOME")).ok_or("USERPROFILE is not set")?;
        let documents = documents().unwrap_or_else(|| home.join("Documents"));
        let program_data = var("ProgramData").or_else(|| var("PROGRAMDATA")).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        Ok(Paths { roaming, local, documents, program_data })
    }
    /// Les sauvegardes des dossiers de scripts remplaces : hors des dossiers de Live et de FL (sinon Live listerait la
    /// copie comme une autre surface de controle, et FL comme un autre type de controleur).
    pub fn backups(&self) -> PathBuf {
        self.local.join(crate::install::APP_DIR).join("backups")
    }
}

/// Une valeur du registre.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RegValue {
    Str(String),
    Num(i64),
    Bin(Vec<u8>),
}

pub const REG_SZ: u32 = 1;
pub const REG_EXPAND_SZ: u32 = 2;
pub const REG_BINARY: u32 = 3;
pub const REG_DWORD: u32 = 4;
pub const REG_QWORD: u32 = 11;

impl RegValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            RegValue::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_num(&self) -> Option<i64> {
        match self {
            RegValue::Num(n) => Some(*n),
            RegValue::Str(s) => s.trim().parse().ok(),
            _ => None,
        }
    }
    /// La valeur en texte : « 77 » pour le texte "77" comme pour le nombre 77 (FL range ses nombres en texte).
    pub fn text(&self) -> String {
        match self {
            RegValue::Str(s) => s.clone(),
            RegValue::Num(n) => n.to_string(),
            RegValue::Bin(b) => b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" "),
        }
    }
    /// Le type Windows et les octets (REG_SZ en UTF-16 termine par un zero, REG_DWORD signe comme FL le lit).
    pub fn to_raw(&self) -> (u32, Vec<u8>) {
        match self {
            RegValue::Str(s) => (REG_SZ, s.encode_utf16().chain([0]).flat_map(|c| c.to_le_bytes()).collect()),
            RegValue::Num(n) if *n >= i32::MIN as i64 && *n <= u32::MAX as i64 => (REG_DWORD, (*n as u32).to_le_bytes().to_vec()),
            RegValue::Num(n) => (REG_QWORD, n.to_le_bytes().to_vec()),
            RegValue::Bin(b) => (REG_BINARY, b.clone()),
        }
    }
    pub fn from_raw(ty: u32, data: &[u8]) -> RegValue {
        if ty == REG_SZ || ty == REG_EXPAND_SZ {
            let w: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let end = w.iter().position(|c| *c == 0).unwrap_or(w.len());
            RegValue::Str(String::from_utf16_lossy(&w[..end]))
        } else if ty == REG_DWORD && data.len() >= 4 {
            // FL range « Port = -1 » en DWORD 0xFFFFFFFF : signe
            RegValue::Num(i32::from_le_bytes([data[0], data[1], data[2], data[3]]) as i64)
        } else if ty == REG_QWORD && data.len() >= 8 {
            RegValue::Num(i64::from_le_bytes(data[..8].try_into().unwrap()))
        } else {
            RegValue::Bin(data.to_vec())
        }
    }
}

/// Deux valeurs (ou deux absences) egales, au texte pres : "77" (REG_SZ) vaut 77 (REG_DWORD).
pub fn same_value(a: Option<&RegValue>, b: Option<&RegValue>) -> bool {
    a.map(|v| v.text()) == b.map(|v| v.text())
}

/// Le registre. Les chemins : "HKCU\\Software\\Image-Line\\…" (HKCU ou HKLM). Lecture seule par defaut : les ecritures
/// sont refusees, sauf dans le vrai registre pour les cles de `write_allowed` et dans le faux.
pub trait Registry {
    /// Les noms des sous-cles directes (vide si la cle n'existe pas).
    fn subkeys(&self, path: &str) -> Vec<String>;
    fn value(&self, path: &str, name: &str) -> Option<RegValue>;
    /// Les noms des valeurs de la cle (la sauvegarde .reg).
    fn value_names(&self, _path: &str) -> Vec<String> {
        vec![]
    }
    fn key_exists(&self, path: &str) -> bool {
        !self.subkeys(path).is_empty() || !self.value_names(path).is_empty()
    }
    /// Le type Windows et les octets tels quels (la sauvegarde .reg les reprend sans rien perdre).
    fn raw(&self, path: &str, name: &str) -> Option<(u32, Vec<u8>)> {
        self.value(path, name).map(|v| v.to_raw())
    }
    fn str(&self, path: &str, name: &str) -> Option<String> {
        self.value(path, name).and_then(|v| v.as_str().map(|s| s.to_string()))
    }
    fn num(&self, path: &str, name: &str) -> Option<i64> {
        self.value(path, name).and_then(|v| v.as_num())
    }
    /// Ecrit la valeur (Str → REG_SZ, Num → REG_DWORD, Bin → REG_BINARY), la cle creee au besoin.
    fn set_value(&self, path: &str, _name: &str, _v: &RegValue) -> Result<(), String> {
        Err(format!("{path}: this registry is read-only"))
    }
    /// Retire la valeur (absente : rien a faire).
    fn delete_value(&self, path: &str, _name: &str) -> Result<(), String> {
        Err(format!("{path}: this registry is read-only"))
    }
    /// Retire la cle, seulement si elle est vide (ni valeur ni sous-cle).
    fn delete_key(&self, path: &str) -> Result<(), String> {
        Err(format!("{path}: this registry is read-only"))
    }
}

type FakeKeys = BTreeMap<String, (String, BTreeMap<String, (String, RegValue)>)>;

/// Un faux registre (les essais, UWC_SETUP_ROOT) : cle → (valeur → donnee), sans casse comme Windows, les noms gardes
/// tels qu'ecrits. Il s'ecrit (en memoire, et dans `file` s'il en a un).
#[derive(Default, Debug)]
pub struct FakeRegistry {
    keys: RefCell<FakeKeys>,
    /// UWC_SETUP_ROOT\registry.json : reecrit apres chaque ecriture
    file: Option<PathBuf>,
    /// les essais : l'ecriture de cette valeur echoue (le retour en arriere)
    pub fail_on: RefCell<Option<String>>,
}

fn norm(path: &str) -> String {
    path.trim_matches('\\').to_ascii_lowercase()
}

fn make_key(keys: &mut FakeKeys, path: &str) {
    let parts: Vec<&str> = path.trim_matches('\\').split('\\').collect();
    for i in 1..=parts.len() {
        let p = parts[..i].join("\\");
        keys.entry(norm(&p)).or_insert_with(|| (p.clone(), BTreeMap::new()));
    }
}

impl FakeRegistry {
    /// La cle et ses parentes (comme dans le vrai registre).
    pub fn key(&mut self, path: &str) {
        make_key(self.keys.get_mut(), path);
    }
    pub fn set(&mut self, path: &str, name: &str, v: RegValue) {
        let keys = self.keys.get_mut();
        make_key(keys, path);
        keys.get_mut(&norm(path)).unwrap().1.insert(name.to_ascii_lowercase(), (name.to_string(), v));
    }
    /// { "HKCU\\…": { "nom": "texte" | nombre | [octets] } }
    pub fn from_json(v: &serde_json::Value) -> FakeRegistry {
        let mut r = FakeRegistry::default();
        if let Some(o) = v.as_object() {
            for (k, vals) in o {
                r.key(k);
                if let Some(vo) = vals.as_object() {
                    for (n, x) in vo {
                        let val = if let Some(i) = x.as_i64() {
                            RegValue::Num(i)
                        } else if let Some(a) = x.as_array() {
                            RegValue::Bin(a.iter().filter_map(|b| b.as_u64()).map(|b| b as u8).collect())
                        } else {
                            RegValue::Str(x.as_str().unwrap_or("").to_string())
                        };
                        r.set(k, n, val);
                    }
                }
            }
        }
        r
    }
    /// Reecrit `file` apres chaque ecriture.
    pub fn with_file(mut self, file: PathBuf) -> FakeRegistry {
        self.file = Some(file);
        self
    }
    pub fn to_json(&self) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        for (orig, vals) in self.keys.borrow().values() {
            let mut m = serde_json::Map::new();
            for (n, v) in vals.values() {
                m.insert(n.clone(), serde_json::to_value(v).unwrap());
            }
            o.insert(orig.clone(), serde_json::Value::Object(m));
        }
        serde_json::Value::Object(o)
    }
    fn save(&self) -> Result<(), String> {
        match &self.file {
            Some(f) => std::fs::write(f, serde_json::to_string_pretty(&self.to_json()).unwrap()).map_err(|e| format!("cannot write {} ({e})", f.display())),
            None => Ok(()),
        }
    }
}

impl Registry for FakeRegistry {
    fn subkeys(&self, path: &str) -> Vec<String> {
        let p = norm(path);
        let prefix = format!("{p}\\");
        self.keys
            .borrow()
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix) && !k[prefix.len()..].contains('\\'))
            .map(|(_, (orig, _))| orig.rsplit('\\').next().unwrap_or("").to_string())
            .collect()
    }
    fn value(&self, path: &str, name: &str) -> Option<RegValue> {
        self.keys.borrow().get(&norm(path)).and_then(|(_, m)| m.get(&name.to_ascii_lowercase()).map(|(_, v)| v.clone()))
    }
    fn value_names(&self, path: &str) -> Vec<String> {
        self.keys.borrow().get(&norm(path)).map(|(_, m)| m.values().map(|(n, _)| n.clone()).collect()).unwrap_or_default()
    }
    fn key_exists(&self, path: &str) -> bool {
        self.keys.borrow().contains_key(&norm(path))
    }
    fn set_value(&self, path: &str, name: &str, v: &RegValue) -> Result<(), String> {
        if self.fail_on.borrow().as_deref().is_some_and(|f| f.eq_ignore_ascii_case(name)) {
            return Err(format!("cannot write {path}\\{name} (a test failure)"));
        }
        {
            let mut keys = self.keys.borrow_mut();
            make_key(&mut keys, path);
            keys.get_mut(&norm(path)).unwrap().1.insert(name.to_ascii_lowercase(), (name.to_string(), v.clone()));
        }
        self.save()
    }
    fn delete_value(&self, path: &str, name: &str) -> Result<(), String> {
        if let Some((_, m)) = self.keys.borrow_mut().get_mut(&norm(path)) {
            m.remove(&name.to_ascii_lowercase());
        }
        self.save()
    }
    fn delete_key(&self, path: &str) -> Result<(), String> {
        if !self.key_exists(path) {
            return Ok(());
        }
        if !self.subkeys(path).is_empty() || !self.value_names(path).is_empty() {
            return Err(format!("{path} is not empty"));
        }
        self.keys.borrow_mut().remove(&norm(path));
        self.save()
    }
}

/// Un peripherique audio (Windows : un « endpoint » MMDevice).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Endpoint {
    pub name: String,
    /// le format partage de Windows (Proprietes › Avance), en Hz ; 0 = inconnu
    pub rate: u32,
    /// l'identifiant MMDevice (FL Studio ASIO range sa sortie sous cet identifiant)
    pub id: String,
    /// false : debranche ou desactive (liste quand meme : la sortie de FL Studio ASIO peut etre un casque debranche)
    pub active: bool,
}

#[cfg(test)]
impl Endpoint {
    pub fn new(name: &str, rate: u32, id: &str) -> Endpoint {
        Endpoint { name: name.into(), rate, id: id.into(), active: true }
    }
}

/// La machine telle que `--setup` la voit.
pub struct Probe {
    pub paths: Paths,
    pub reg: Box<dyn Registry>,
    pub midi_in: Vec<String>,
    pub midi_out: Vec<String>,
    pub render: Vec<Endpoint>,
    pub capture: Vec<Endpoint>,
    /// les noms des exe qui tournent (« Ableton Live 12 Suite.exe », « FL64.exe », « loopMIDI.exe »)
    pub processes: Vec<String>,
    /// true : la fausse machine de UWC_SETUP_ROOT
    pub fake: bool,
}

impl Probe {
    /// Une fausse machine, vide : les essais la remplissent.
    pub fn empty(paths: Paths) -> Probe {
        Probe { paths, reg: Box::new(FakeRegistry::default()), midi_in: vec![], midi_out: vec![], render: vec![], capture: vec![], processes: vec![], fake: true }
    }

    /// La fausse machine d'un dossier (UWC_SETUP_ROOT) : registry.json et system.json s'ils sont la.
    pub fn from_root(root: &Path) -> Probe {
        let mut p = Probe::empty(Paths::in_root(root));
        let read = |n: &str| std::fs::read_to_string(root.join(n)).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        let reg = read("registry.json").map(|r| FakeRegistry::from_json(&r)).unwrap_or_default();
        p.reg = Box::new(reg.with_file(root.join("registry.json")));
        if let Some(s) = read("system.json") {
            let names = |k: &str| s[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            let eps = |k: &str| {
                s[k].as_array()
                    .map(|a| {
                        a.iter()
                            .map(|x| Endpoint {
                                name: x["name"].as_str().unwrap_or("").into(),
                                rate: x["rate"].as_u64().unwrap_or(0) as u32,
                                id: x["id"].as_str().unwrap_or("").into(),
                                active: x["active"].as_bool().unwrap_or(true),
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            };
            p.midi_in = names("midi_in");
            p.midi_out = names("midi_out");
            p.render = eps("render");
            p.capture = eps("capture");
            p.processes = names("processes");
        }
        p
    }

    /// La vraie machine (ou la fausse si UWC_SETUP_ROOT est donne).
    pub fn from_env() -> Result<Probe, String> {
        if let Some(root) = std::env::var_os(ROOT_ENV).filter(|v| !v.is_empty()) {
            return Ok(Probe::from_root(Path::new(&root)));
        }
        let paths = Paths::from_lookup(|k| std::env::var_os(k), super::winsys::documents_folder)?;
        Ok(Probe {
            paths,
            reg: Box::new(super::winsys::RealRegistry),
            midi_in: super::winsys::midi_inputs(),
            midi_out: super::winsys::midi_outputs(),
            render: super::winsys::endpoints(true),
            capture: super::winsys::endpoints(false),
            processes: super::winsys::processes(),
            fake: false,
        })
    }

    pub fn running(&self, exe: &str) -> bool {
        self.processes.iter().any(|p| p.eq_ignore_ascii_case(exe))
    }
    pub fn running_like(&self, pred: impl Fn(&str) -> bool) -> Option<String> {
        self.processes.iter().find(|p| pred(&p.to_ascii_lowercase())).cloned()
    }
}

/// Une entree « Programmes et fonctionnalites » (HKLM/HKCU …\Uninstall).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Program {
    pub name: String,
    pub version: String,
    pub location: String,
}

pub const UNINSTALL_KEYS: [&str; 3] = [
    r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
    r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall",
];

/// Les programmes installes dont le nom passe `pred` (sans doublons de nom + version).
pub fn programs(reg: &dyn Registry, pred: impl Fn(&str) -> bool) -> Vec<Program> {
    let mut out: Vec<Program> = Vec::new();
    for root in UNINSTALL_KEYS {
        for k in reg.subkeys(root) {
            let path = format!("{root}\\{k}");
            let Some(name) = reg.str(&path, "DisplayName") else { continue };
            if !pred(&name) {
                continue;
            }
            let p = Program { name, version: reg.str(&path, "DisplayVersion").unwrap_or_default(), location: reg.str(&path, "InstallLocation").unwrap_or_default() };
            if !out.iter().any(|o| o.name == p.name && o.version == p.version) {
                out.push(p);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn paths_come_from_the_environment_or_the_override() {
        let env: HashMap<&str, OsString> = HashMap::from([
            ("APPDATA", OsString::from(r"C:\Users\x\AppData\Roaming")),
            ("LOCALAPPDATA", OsString::from(r"C:\Users\x\AppData\Local")),
            ("USERPROFILE", OsString::from(r"C:\Users\x")),
        ]);
        let p = Paths::from_lookup(|k| env.get(k).cloned(), || None).unwrap();
        assert_eq!(p.documents, PathBuf::from(r"C:\Users\x\Documents"));
        assert_eq!(p.program_data, PathBuf::from(r"C:\ProgramData"));
        assert_eq!(p.backups(), PathBuf::from(r"C:\Users\x\AppData\Local\Uppwind Link\backups"));
        // le dossier Documents connu de Windows passe devant (il peut etre sur E:)
        let p2 = Paths::from_lookup(|k| env.get(k).cloned(), || Some(PathBuf::from(r"D:\docs"))).unwrap();
        assert_eq!(p2.documents, PathBuf::from(r"D:\docs"));
        let env3: HashMap<&str, OsString> = HashMap::from([(ROOT_ENV, OsString::from(r"D:\test")), ("APPDATA", OsString::from(r"C:\real"))]);
        let p3 = Paths::from_lookup(|k| env3.get(k).cloned(), || Some(PathBuf::from(r"C:\real\docs"))).unwrap();
        assert_eq!(p3.roaming, PathBuf::from(r"D:\test\AppData\Roaming"));
        assert_eq!(p3.documents, PathBuf::from(r"D:\test\Documents"));
        assert!(Paths::from_lookup(|_| None, || None).is_err());
    }

    #[test]
    fn the_fake_registry_behaves_like_the_real_one() {
        let r = FakeRegistry::from_json(&serde_json::json!({
            r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI input\Uppwind Link": { "Port": 77, "Type": -3, "ScriptFolder": "Uppwind Link" },
            r"HKCU\Software\Image-Line\FL Studio 21": {},
        }));
        let mut ks = r.subkeys(r"hkcu\software\image-line");
        ks.sort();
        assert_eq!(ks, vec!["FL Studio 21".to_string(), "FL Studio 26".to_string()]);
        assert_eq!(r.subkeys(r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI input"), vec!["Uppwind Link".to_string()]);
        assert_eq!(r.num(r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI input\Uppwind Link", "port"), Some(77));
        assert_eq!(r.str(r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI input\Uppwind Link", "ScriptFolder").as_deref(), Some("Uppwind Link"));
        assert_eq!(r.value(r"HKCU\Nope", "x"), None);
        assert!(r.subkeys(r"HKCU\Nope").is_empty());
    }

    #[test]
    fn only_fl_settings_can_be_written() {
        for ok in [
            r"HKCU\Software\Image-Line\FL Studio 26\Devices",
            r"HKCU\Software\Image-Line\FL Studio 26\Devices\MIDI input\Uppwind Link",
            r"HKEY_CURRENT_USER\Software\Image-Line\FL Studio 21\General\MIDIForm",
            r"hkcu/software/image-line/fl studio 26/devices/midi output/x",
        ] {
            assert!(write_allowed(ok, false), "{ok}");
            assert!(!write_allowed(ok, true), "the tests never write FL's real key: {ok}");
        }
        for no in [
            r"HKLM\SOFTWARE\Image-Line\FL Studio 26\Devices",
            r"HKCU\Software\Image-Line\FL Studio 26",
            r"HKCU\Software\Image-Line\FL Studio 26\General",
            r"HKCU\Software\Image-Line\FL Studio 26\General\MIDIForm\x",
            r"HKCU\Software\Image-Line\FL Studio X\Devices",
            r"HKCU\Software\Image-Line\ASIO",
            r"HKCU\Software\Image-Line\FL Studio 26\Devices\..\..",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            r"HKCU\Software\UppwindLinkTest\x",
        ] {
            assert!(!write_allowed(no, false), "{no}");
        }
        assert!(write_allowed(r"HKCU\Software\UppwindLinkTest\abc\Image-Line", true));
        assert!(!write_allowed(r"HKCU\Software\UppwindLinkTest", true));
        assert!(!write_allowed(r"HKCU\Software\UppwindLinkTestX\a", true));
    }

    #[test]
    fn the_fake_registry_writes_and_keeps_names() {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("uwc-fakereg-{}-{t}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let f = root.join("registry.json");
        let r = FakeRegistry::from_json(&serde_json::json!({ r"HKCU\A\B": { "Port": "77" } })).with_file(f.clone());
        r.set_value(r"HKCU\A\C", "IDString", &RegValue::Str("-".into())).unwrap();
        assert_eq!(r.value_names(r"hkcu\a\c"), vec!["IDString".to_string()]);
        let saved = FakeRegistry::from_json(&serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap());
        assert_eq!(saved.str(r"HKCU\A\C", "idstring").as_deref(), Some("-"));
        assert_eq!(saved.str(r"HKCU\A\B", "Port").as_deref(), Some("77"));
        assert!(r.delete_key(r"HKCU\A\C").is_err(), "not empty");
        r.delete_value(r"HKCU\A\C", "IDString").unwrap();
        r.delete_key(r"HKCU\A\C").unwrap();
        assert!(!r.key_exists(r"HKCU\A\C"));
        assert_eq!(RegValue::from_raw(REG_SZ, &RegValue::Str("-1".into()).to_raw().1), RegValue::Str("-1".into()));
        assert_eq!(RegValue::Num(-1).to_raw(), (REG_DWORD, vec![0xff; 4]));
        assert!(same_value(Some(&RegValue::Str("77".into())), Some(&RegValue::Num(77))));
        assert!(!same_value(None, Some(&RegValue::Str(String::new()))));
        assert!(FakeRegistry::default().set_value(r"HKCU\x", "y", &RegValue::Num(1)).is_ok());
        struct ReadOnly;
        impl Registry for ReadOnly {
            fn subkeys(&self, _: &str) -> Vec<String> {
                vec![]
            }
            fn value(&self, _: &str, _: &str) -> Option<RegValue> {
                None
            }
        }
        assert!(ReadOnly.set_value(r"HKCU\x", "y", &RegValue::Num(1)).is_err(), "a registry is read-only unless it says otherwise");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn programs_are_read_from_the_uninstall_keys() {
        let r = FakeRegistry::from_json(&serde_json::json!({
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\FL Studio 2026": { "DisplayName": "FL Studio 2026", "DisplayVersion": "26.1.0.1000" },
            r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\{x}": { "DisplayName": "loopMIDI", "DisplayVersion": "1.0.16.0" },
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\{y}": { "DisplayName": "loopMIDI", "DisplayVersion": "1.0.16.0" },
        }));
        assert_eq!(programs(&r, |n| n == "loopMIDI").len(), 1, "the same program twice is listed once");
        assert_eq!(programs(&r, |n| n.starts_with("FL Studio"))[0].version, "26.1.0.1000");
    }
}
