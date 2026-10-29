// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Embarque les scripts des DAW dans l'exe (src/setup/scripts.rs : `--setup` les installe sans telechargement).
//!
//! Les memes fichiers que les zips de telechargement du studio :
//!   Ableton : ableton/UppwindLink/ (la liste ABLETON) + ableton/README.md + LICENSE-MIT
//!   FL      : fl-studio/ (la liste FL) + fl-studio/README.md + LICENSE-MIT
//! (les scripts sont sous licence MIT, le compagnon sous GPL-3.0-or-later : la licence des scripts les accompagne)
//! Audit de publication : une liste EXPLICITE. Un fichier de trop dans ces dossiers (une copie de travail, un .orig,
//! des notes…) arrete la construction au lieu de partir dans l'exe puis, par --setup, chez chaque utilisateur ; un
//! fichier de la liste qui manque aussi. Seulement des fichiers ordinaires (pas de lien), tous sous ce dossier.
//!
//! (29 oct.) Et les plugins Uppwind Link (src/plugin_install/), quand UWL_PLUGIN_BUNDLE nomme un « Uppwind Link.vst3 »
//! deja construit (uwl-bundle ; sous macOS, deja signe) : chacun de ses fichiers, avec son chemin dans le bundle. La
//! meme regle : une disposition FIXE par systeme (PLUGIN_LAYOUT), un fichier de trop, un fichier qui manque, un lien ou
//! un dossier vide arretent la construction. Sans la variable : aucun plugin (une construction de developpement ; la
//! fenetre d'installation et --setup disent « not in this build »). Ni le chemin du bundle ni l'heure n'entrent dans
//! l'exe : la construction reste reproductible (dev/release_check.py).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

// (24 oct.) l'icone et la version de l'exe, le disque des pages de l'interface
#[path = "build/winres.rs"]
mod winres;

/// ableton/UppwindLink/ : le Remote Script.
const ABLETON: &[&str] = &["UppwindLink.py", "__init__.py", "editor_channel.py", "editor_core.py", "link_core.py", "notes_route.py", "uwlink_protocol.py"];
/// fl-studio/ : le script de FL (son README.md, dans le meme dossier, est ajoute a la fin).
const FL: &[&str] = &["device_UppwindLink.py", "uwlink_protocol.py"];
/// companion/assets/ : ce que build/winres.rs lit (l'icone de l'exe, le disque des pages de src/ui/). Les pages
/// elles-memes (src/ui/web/) sont des include_str! du code, nommees une par une dans src/ui/mod.rs.
const ASSETS: &[&str] = &["uppwind-link.ico", "mark-64.png"];

/// Ce que Python ou Windows posent d'eux-memes : jamais embarque, et pas une erreur.
fn ignorable(name: &str) -> bool {
    name == "__pycache__" || name.ends_with(".pyc") || name.eq_ignore_ascii_case("desktop.ini") || name.eq_ignore_ascii_case("thumbs.db") || name == ".DS_Store"
}

/// Le dossier ne contient que `want` (et des fichiers ignorables) : sinon la construction s'arrete.
fn check_folder(dir: &Path, want: &[&str]) {
    let rd = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {} ({e})", dir.display()));
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if ignorable(&name) {
            continue;
        }
        if !want.contains(&name.as_str()) {
            panic!("{} holds {name:?}, which is not in build.rs's list: add it there (it ships to every user) or move it out", dir.display());
        }
    }
}

/// Un fichier a embarquer : un fichier ordinaire (pas un lien), sous `root`.
fn checked(root: &Path, p: PathBuf) -> PathBuf {
    let meta = std::fs::symlink_metadata(&p).unwrap_or_else(|e| panic!("{} is missing ({e})", p.display()));
    if !meta.file_type().is_file() {
        panic!("{} is not a regular file (a link or a folder): refused", p.display());
    }
    let (c, r) = (std::fs::canonicalize(&p).unwrap(), std::fs::canonicalize(root).unwrap());
    if !c.starts_with(&r) {
        panic!("{} resolves outside {}: refused", p.display(), root.display());
    }
    p
}

fn emit(out: &mut String, name: &str, files: &[(String, PathBuf)]) {
    writeln!(out, "pub static {name}: &[(&str, &[u8])] = &[").unwrap();
    for (n, p) in files {
        // le chemin absolu tel quel (CARGO_MANIFEST_DIR l'est deja) : pas de canonicalize, dont le prefixe \\?\ gene
        // include_bytes! sur certaines versions et casse un chemin reseau (\\?\UNC\…)
        writeln!(out, "    ({n:?}, include_bytes!({:?})),", p.to_string_lossy()).unwrap();
        println!("cargo:rerun-if-changed={}", p.display());
    }
    writeln!(out, "];").unwrap();
}

/// La variable qui nomme le bundle des plugins a embarquer (un dossier « Uppwind Link.vst3 »).
const PLUGIN_ENV: &str = "UWL_PLUGIN_BUNDLE";
/// Le nom du dossier du bundle (celui que uwl-bundle ecrit, celui qu'on installe).
const PLUGIN_BUNDLE: &str = "Uppwind Link.vst3";
/// Plus gros que ca, ce n'est pas notre plugin (la DLL fait ~1 Mo, la dylib universelle ~3 Mo).
const PLUGIN_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Les fichiers d'un bundle VST3 tel que uwl-bundle l'ecrit, pour le systeme et l'architecture CIBLES (pas ceux de la
/// machine qui construit), tries : Windows, la DLL renommee (la regle de Steinberg, VST 3.6.10+) ; macOS, le paquet signe
/// (codesign y ajoute _CodeSignature/CodeResources : un bundle non signe est refuse).
fn plugin_layout(os: &str, arch: &str) -> Option<&'static [&'static str]> {
    match (os, arch) {
        ("windows", "x86_64") => Some(&["Contents/x86_64-win/Uppwind Link.vst3"]),
        ("macos", "aarch64" | "x86_64") => Some(&["Contents/Info.plist", "Contents/MacOS/Uppwind Link", "Contents/PkgInfo", "Contents/_CodeSignature/CodeResources"]),
        _ => None,
    }
}

/// Tout ce qui est sous `dir` : les fichiers (chemin relatif avec des « / ») ; un lien, un dossier vide ou autre chose
/// qu'un fichier ordinaire arrete la construction.
fn walk_bundle(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let rd = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{PLUGIN_ENV}: cannot read {} ({e})", dir.display()));
    let mut empty = true;
    for e in rd {
        let e = e.unwrap_or_else(|e| panic!("{PLUGIN_ENV}: cannot read {} ({e})", dir.display()));
        empty = false;
        let p = e.path();
        let m = std::fs::symlink_metadata(&p).unwrap_or_else(|e| panic!("{PLUGIN_ENV}: {} ({e})", p.display()));
        if m.file_type().is_symlink() {
            panic!("{PLUGIN_ENV}: {} is a link: refused (the bundle must hold its own files)", p.display());
        }
        if m.is_dir() {
            walk_bundle(root, &p, out);
        } else if m.is_file() {
            let rel: Vec<String> = p.strip_prefix(root).unwrap().components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
            out.push((rel.join("/"), p));
        } else {
            panic!("{PLUGIN_ENV}: {} is not a regular file: refused", p.display());
        }
    }
    if empty && dir != root {
        panic!("{PLUGIN_ENV}: {} is an empty folder, which no bundle has: refused", dir.display());
    }
}

/// Le bundle des plugins → (chemin relatif, fichier, executable), tries, verifies ; None : pas de variable.
fn plugin_files() -> Option<Vec<(String, PathBuf, bool)>> {
    println!("cargo:rerun-if-env-changed={PLUGIN_ENV}");
    let raw = std::env::var_os(PLUGIN_ENV).filter(|v| !v.is_empty())?;
    let root = PathBuf::from(raw);
    // (cargo lance build.rs depuis companion/ : un chemin relatif ne dirait pas de quel dossier il part)
    if !root.is_absolute() {
        panic!("{PLUGIN_ENV} must be an absolute path (got {})", root.display());
    }
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let want = plugin_layout(&os, &arch).unwrap_or_else(|| panic!("{PLUGIN_ENV} is set, but the companion installs the Uppwind Link plugins on Windows (x86_64) and macOS only, not on {os} {arch}: unset it"));
    if root.file_name().and_then(|n| n.to_str()) != Some(PLUGIN_BUNDLE) {
        panic!("{PLUGIN_ENV} must name a folder called \"{PLUGIN_BUNDLE}\" (got {})", root.display());
    }
    let m = std::fs::symlink_metadata(&root).unwrap_or_else(|e| panic!("{PLUGIN_ENV}: {} is missing ({e}): build the plugins and bundle them first (uwl-bundle)", root.display()));
    if !m.is_dir() || m.file_type().is_symlink() {
        panic!("{PLUGIN_ENV}: {} is not a plain folder: refused", root.display());
    }
    let mut found = Vec::new();
    walk_bundle(&root, &root, &mut found);
    found.sort();
    let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
    for n in &names {
        if !want.contains(n) {
            panic!("{PLUGIN_ENV}: the bundle holds {n:?}, which the {os} layout of build.rs does not have (expected exactly {want:?}): it would ship to every user, so it is refused");
        }
    }
    for w in want {
        if !names.contains(w) {
            panic!("{PLUGIN_ENV}: the bundle has no {w:?} (expected exactly {want:?}){}", if w.contains("_CodeSignature") { ": sign it first (codesign)" } else { "" });
        }
    }
    let mut total = 0u64;
    let mut out = Vec::new();
    for (rel, p) in found {
        let p = checked(&root, p);
        let bytes = std::fs::read(&p).unwrap_or_else(|e| panic!("{PLUGIN_ENV}: cannot read {} ({e})", p.display()));
        total += bytes.len() as u64;
        // le binaire du plugin : le bon format, pour la bonne cible (une erreur de chemin se voit ici, pas chez l'utilisateur)
        if rel.ends_with("/x86_64-win/Uppwind Link.vst3") {
            let pe = (bytes.len() > 0x40 && &bytes[..2] == b"MZ")
                .then(|| u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize)
                .filter(|o| bytes.len() > o + 24 && &bytes[*o..o + 4] == b"PE\0\0");
            let ok = pe.is_some_and(|o| u16::from_le_bytes([bytes[o + 4], bytes[o + 5]]) == 0x8664 && u16::from_le_bytes([bytes[o + 22], bytes[o + 23]]) & 0x2000 != 0);
            if !ok {
                panic!("{PLUGIN_ENV}: {} is not an x86_64 Windows DLL", p.display());
            }
        }
        let exec = rel.starts_with("Contents/MacOS/");
        if exec {
            let magic = bytes.get(..4).map(|m| [m[0], m[1], m[2], m[3]]);
            // CA FE BA BE : universel (lipo) ; CF FA ED FE : Mach-O 64 bits
            if !matches!(magic, Some([0xca, 0xfe, 0xba, 0xbe]) | Some([0xcf, 0xfa, 0xed, 0xfe])) {
                panic!("{PLUGIN_ENV}: {} is not a Mach-O binary", p.display());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if std::fs::metadata(&p).map(|m| m.permissions().mode() & 0o111 == 0).unwrap_or(true) {
                    panic!("{PLUGIN_ENV}: {} is not executable", p.display());
                }
            }
        }
        out.push((rel, p, exec));
    }
    if total > PLUGIN_MAX_BYTES {
        panic!("{PLUGIN_ENV}: the bundle weighs {total} bytes, more than {PLUGIN_MAX_BYTES}: not the Uppwind Link plugins");
    }
    println!("cargo:rerun-if-changed={}", root.display());
    Some(out)
}

fn emit_plugins(out: &mut String, link: &Path, files: Option<Vec<(String, PathBuf, bool)>>) {
    writeln!(out, "/// (chemin dans le bundle, octets, executable) ; vide : une construction sans plugins").unwrap();
    writeln!(out, "pub static PLUGIN_FILES: &[(&str, &[u8], bool)] = &[").unwrap();
    let embedded = files.is_some();
    for (rel, p, exec) in files.unwrap_or_default() {
        writeln!(out, "    ({rel:?}, include_bytes!({:?}), {exec}),", p.to_string_lossy()).unwrap();
        println!("cargo:rerun-if-changed={}", p.display());
    }
    writeln!(out, "];").unwrap();
    // les notices des crates des plugins : avec les plugins seulement (elles accompagnent leur binaire)
    if embedded {
        let n = checked(link, link.join("plugin").join("THIRD-PARTY-NOTICES.txt"));
        println!("cargo:rerun-if-changed={}", n.display());
        writeln!(out, "pub static PLUGIN_NOTICES: &str = include_str!({:?});", n.to_string_lossy()).unwrap();
    } else {
        writeln!(out, "pub static PLUGIN_NOTICES: &str = \"\";").unwrap();
    }
}

fn main() {
    let here = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let link = here.parent().unwrap().to_path_buf();
    let ab_dir = link.join("ableton").join("UppwindLink");
    let fl_dir = link.join("fl-studio");
    check_folder(&ab_dir, ABLETON);
    check_folder(&fl_dir, &[FL, &["README.md"]].concat());
    let mut ab: Vec<(String, PathBuf)> = ABLETON.iter().map(|n| (n.to_string(), checked(&link, ab_dir.join(n)))).collect();
    ab.push(("README.md".into(), checked(&link, link.join("ableton").join("README.md"))));
    let mut fl: Vec<(String, PathBuf)> = FL.iter().map(|n| (n.to_string(), checked(&link, fl_dir.join(n)))).collect();
    fl.push(("README.md".into(), checked(&link, fl_dir.join("README.md"))));
    for v in [&mut ab, &mut fl] {
        for n in ["LICENSE-MIT"] {
            v.push((n.to_string(), checked(&link, link.join(n))));
        }
    }
    let mut out = String::from("// genere par build.rs : ne pas modifier\n");
    emit(&mut out, "ABLETON_FILES", &ab);
    emit(&mut out, "FL_FILES", &fl);
    println!("cargo:rerun-if-changed={}", ab_dir.display());
    println!("cargo:rerun-if-changed={}", fl_dir.display());
    println!("cargo:rerun-if-changed=build.rs");
    let dst = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("embedded_scripts.rs");
    std::fs::write(dst, out).unwrap();
    // (29 oct.) les plugins Uppwind Link, si UWL_PLUGIN_BUNDLE est donne
    let mut plugins = String::from("// genere par build.rs : ne pas modifier\n");
    emit_plugins(&mut plugins, &link, plugin_files());
    std::fs::write(PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("embedded_plugins.rs"), plugins).unwrap();
    println!("cargo:rerun-if-changed=build/winres.rs");
    // la meme regle pour les fichiers de l'interface : la liste, des fichiers ordinaires sous ce dossier
    let assets = here.join("assets");
    check_folder(&assets, ASSETS);
    for n in ASSETS {
        checked(&here, assets.join(n));
    }
    println!("cargo:rerun-if-changed={}", assets.display());
    winres::emit(&here);
}
