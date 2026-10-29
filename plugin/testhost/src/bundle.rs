// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! uwl-bundle : la DLL construite → le paquet VST3 (et le fichier CLAP), et l'installation pour l'utilisateur seul.
//!
//!   uwl-bundle [--dll CHEMIN] [--out DOSSIER] [--sign IDENTITE] [--install]
//!
//! - macOS (26 oct.) : <out>/Uppwind Link.vst3/Contents/{Info.plist, PkgInfo, MacOS/Uppwind Link} et le meme paquet
//!   en .clap ; --sign - les signe ad hoc (codesign), --install copie le .vst3 dans ~/Library/Audio/Plug-Ins/VST3.
//! - Le paquet VST3 (la regle de Steinberg pour Windows, VST 3.6.10+) :
//!     <out>\Uppwind Link.vst3\Contents\x86_64-win\Uppwind Link.vst3      (la DLL renommee)
//!   et a cote : <out>\Uppwind Link.clap (la meme DLL : nice-plug exporte les deux formats).
//! - --install (un chemin de developpement) : copie le paquet VST3 dans %LOCALAPPDATA%\Programs\Common\VST3 (le
//!   dossier VST3 de l'utilisateur : rien dans Program Files, pas d'administrateur ; Live le lit en dossier
//!   personnalise, FL Studio jamais). Une version deja la est remplacee ; si un DAW la tient ouverte, la copie echoue
//!   et le dit (fermer le DAW, recommencer). Pour les utilisateurs, c'est le compagnon qui installe les plugins
//!   (--install-plugins, --setup, sa fenetre d'installation : companion/src/plugin_install/).
//! Par defaut, --dll est uppwind_link.dll a cote de cet exe (le meme dossier de construction), --out <ce dossier>\bundled.

use std::path::{Path, PathBuf};

pub const BUNDLE: &str = "Uppwind Link";

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn copy_file(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {} ({e})", dir.display()))?;
    }
    std::fs::copy(from, to).map(|_| ()).map_err(|e| format!("cannot copy {} to {} ({e})", from.display(), to.display()))
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("cannot create {} ({e})", to.display()))?;
    for e in std::fs::read_dir(from).map_err(|e| format!("cannot read {} ({e})", from.display()))? {
        let e = e.map_err(|e| e.to_string())?;
        let p = e.path();
        let q = to.join(e.file_name());
        if p.is_dir() {
            copy_dir(&p, &q)?;
        } else {
            copy_file(&p, &q)?;
        }
    }
    Ok(())
}

/// L'identifiant des paquets macOS (CFBundleIdentifier) : « .vst3 » / « .clap » ajoute.
pub const BUNDLE_ID: &str = "com.uppwind.link";

/// macOS : le Info.plist d'un paquet de plugin (CFBundlePackageType BNDL ; l'executable est Contents/MacOS/<BUNDLE>).
pub fn info_plist(kind: &str, version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>English</string>
	<key>CFBundleExecutable</key>
	<string>{BUNDLE}</string>
	<key>CFBundleIdentifier</key>
	<string>{BUNDLE_ID}.{kind}</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>{BUNDLE}</string>
	<key>CFBundlePackageType</key>
	<string>BNDL</string>
	<key>CFBundleSignature</key>
	<string>????</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>CFBundleVersion</key>
	<string>{version}</string>
	<key>LSMinimumSystemVersion</key>
	<string>11.0</string>
	<key>NSHumanReadableCopyright</key>
	<string>Copyright (C) 2026 Uppwind. GPL-3.0-or-later.</string>
</dict>
</plist>
"#
    )
}

/// Un paquet macOS : <root>/Contents/{Info.plist, PkgInfo, MacOS/<BUNDLE>}.
fn mac_bundle(dylib: &Path, root: &Path, kind: &str) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(root);
    let contents = root.join("Contents");
    copy_file(dylib, &contents.join("MacOS").join(BUNDLE))?;
    let w = |p: PathBuf, s: &str| std::fs::write(&p, s).map_err(|e| format!("cannot write {} ({e})", p.display()));
    w(contents.join("Info.plist"), &info_plist(kind, env!("CARGO_PKG_VERSION")))?;
    w(contents.join("PkgInfo"), "BNDL????")
}

/// Le paquet : rend son chemin. Windows : la DLL renommee dans Contents\x86_64-win ; macOS : deux paquets (.vst3 et
/// .clap), la meme dylib dans Contents/MacOS (une dylib « universelle » faite par lipo convient aussi).
pub fn bundle(dll: &Path, out: &Path) -> Result<PathBuf, String> {
    if !dll.is_file() {
        return Err(format!("no plugin library at {} (cargo build --release first)", dll.display()));
    }
    let root = out.join(format!("{BUNDLE}.vst3"));
    if cfg!(target_os = "macos") {
        mac_bundle(dll, &root, "vst3")?;
        mac_bundle(dll, &out.join(format!("{BUNDLE}.clap")), "clap")?;
        return Ok(root);
    }
    let _ = std::fs::remove_dir_all(&root);
    copy_file(dll, &root.join("Contents").join("x86_64-win").join(format!("{BUNDLE}.vst3")))?;
    copy_file(dll, &out.join(format!("{BUNDLE}.clap")))?;
    Ok(root)
}

/// macOS : codesign --force --sign <identite> (« - » : ad hoc, pour charger le plugin sur cette machine ; une identite
/// « Developer ID Application » avec --options runtime --timestamp pour une version publiee : dev/build-macos.sh
/// (UWL_PLUGIN_SIGN_IDENTITY) et .github/workflows/macos-build.yml, voir RELEASING.md).
pub fn codesign(path: &Path, identity: &str) -> Result<(), String> {
    let mut c = std::process::Command::new("codesign");
    c.arg("--force").arg("--sign").arg(identity);
    if identity != "-" {
        c.args(["--options", "runtime", "--timestamp"]);
    }
    let st = c.arg(path).status().map_err(|e| format!("cannot run codesign ({e})"))?;
    if !st.success() {
        return Err(format!("codesign failed for {}", path.display()));
    }
    Ok(())
}

/// Le dossier VST3 de l'utilisateur (UWL_VST3_DIR le remplace pour les essais). Windows :
/// %LOCALAPPDATA%\Programs\Common\VST3 ; macOS : ~/Library/Audio/Plug-Ins/VST3.
pub fn user_vst3_dir() -> Result<PathBuf, String> {
    if let Some(d) = std::env::var_os("UWL_VST3_DIR") {
        return Ok(PathBuf::from(d));
    }
    if cfg!(target_os = "macos") {
        let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
        return Ok(PathBuf::from(home).join("Library").join("Audio").join("Plug-Ins").join("VST3"));
    }
    let local = std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is not set")?;
    Ok(PathBuf::from(local).join("Programs").join("Common").join("VST3"))
}

/// Le nom de la bibliotheque que cargo construit.
pub fn default_lib_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "libuppwind_link.dylib"
    } else if cfg!(windows) {
        "uppwind_link.dll"
    } else {
        "libuppwind_link.so"
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let here = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(|| PathBuf::from("."));
    let dll = arg(&args, "--dll").map(PathBuf::from).unwrap_or_else(|| here.join(default_lib_name()));
    let out = arg(&args, "--out").map(PathBuf::from).unwrap_or_else(|| here.join("bundled"));
    let root = match bundle(&dll, &out) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    println!("bundle: {}", root.display());
    println!("clap:   {}", out.join(format!("{BUNDLE}.clap")).display());
    // macOS : --sign - (ad hoc) ou --sign "Developer ID Application: …"
    if let Some(id) = arg(&args, "--sign") {
        for p in [root.clone(), out.join(format!("{BUNDLE}.clap"))] {
            if let Err(e) = codesign(&p, &id) {
                eprintln!("{e}");
                std::process::exit(1);
            }
            println!("signed: {} ({})", p.display(), if id == "-" { "ad hoc" } else { &id });
        }
    }
    if args.iter().any(|a| a == "--install") {
        let dir = match user_vst3_dir() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        };
        let dest = dir.join(format!("{BUNDLE}.vst3"));
        if dest.exists() {
            if let Err(e) = std::fs::remove_dir_all(&dest) {
                eprintln!("cannot replace {} ({e}): is a DAW using it? Close it and try again.", dest.display());
                std::process::exit(1);
            }
        }
        if let Err(e) = copy_dir(&root, &dest) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        println!("installed: {}", dest.display());
    }
}
