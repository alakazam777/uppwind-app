// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (29 oct.) macOS : le nom d'installation de la dylib (LC_ID_DYLIB) serait son chemin dans target/, avec le dossier de
//! l'utilisateur qui l'a construite (dev/release_check.py le refuse dans une version publiee, et le compagnon embarque
//! le bundle tel quel). Aucun binaire ne se lie a un plugin (le DAW l'ouvre par son bundle) : un nom neutre suffit.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-install_name,@rpath/libuppwind_link.dylib");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
