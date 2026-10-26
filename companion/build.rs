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
