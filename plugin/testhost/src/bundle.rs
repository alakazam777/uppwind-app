// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! uwl-bundle : la DLL construite → le paquet VST3 (et le fichier CLAP), et l'installation pour l'utilisateur seul.
//!
//!   uwl-bundle [--dll CHEMIN] [--out DOSSIER] [--auv2] [--sign IDENTITE] [--install]
//!
//! - macOS (26 oct.) : <out>/Uppwind Link.vst3/Contents/{Info.plist, PkgInfo, MacOS/Uppwind Link} et le meme paquet
//!   en .clap ; --sign - les signe ad hoc (codesign), --install copie le .vst3 dans ~/Library/Audio/Plug-Ins/VST3.
//! - (9 nov.) --auv2 : aussi <out>/Uppwind Link.component, les Audio Units de Logic Pro (le meme dylib, construit avec
//!   l'option « auv2 » du crate uppwind-link ; son Info.plist liste les composants, AU_COMPONENTS), signe avec les
//!   autres. (10 nov.) Le compagnon l'embarque (UWL_PLUGIN_COMPONENT) et l'installe dans ~/Library/Audio/Plug-Ins/Components.
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

/// (9 nov., en essai) Les Audio Units (Logic Pro). Le code du fabricant, puis ceux de chaque composant : DEFINITIFS des
/// qu'un projet Logic en garde un (Apple veut une majuscule au moins dans le code du fabricant ; tout en minuscules lui
/// est reserve).
pub const AU_MANUFACTURER: &str = "Uppw";
/// Le nom du fabricant que Logic affiche (« Uppwind: Uppwind Link Return »), le VENDOR des plugins.
pub const AU_VENDOR: &str = "Uppwind";

/// Un composant du .component : son type et son sous-type (4 caracteres), son nom, et l'indice du plugin dans l'export
/// CLAP (nice_export_clap!(LinkReturn, LinkNotes) : 0 Return, 1 Notes), d'ou la fonction GetPluginFactoryAUV2_<indice>
/// que clap-wrapper exporte.
pub struct AuComponent {
    pub ty: &'static str,
    pub subtype: &'static str,
    pub name: &'static str,
    pub index: u8,
    pub description: &'static str,
}

/// Return en effet (la tranche Stereo Out de Logic). Notes deux fois : en instrument a sortie MIDI (une piste a lui,
/// lue par « Internal MIDI In › Instrument Output » de la piste a jouer, comme « MIDI From » dans Live) et en effet MIDI
/// (la case MIDI FX de la piste a jouer) ; clap-wrapper choisit le role d'apres le type, a l'ouverture.
pub const AU_COMPONENTS: [AuComponent; 3] = [
    AuComponent { ty: "aufx", subtype: "UwRt", name: "Uppwind Link Return", index: 0, description: "Sends the DAW's sound to Uppwind Studio through the Uppwind Link companion" },
    AuComponent { ty: "aumu", subtype: "UwNt", name: "Uppwind Link Notes", index: 1, description: "Plays the notes of an Uppwind Studio track as this plugin's MIDI output" },
    AuComponent { ty: "aumi", subtype: "UwNm", name: "Uppwind Link Notes (MIDI FX)", index: 1, description: "Plays the notes of an Uppwind Studio track into this track's instrument" },
];

/// La version d'un AU (AudioComponents/version) : 0xMMMMmmpp.
pub fn au_version(v: &str) -> u32 {
    let mut p = v.split('.').map(|x| x.trim().parse::<u32>().unwrap_or(0));
    let (a, b, c) = (p.next().unwrap_or(0), p.next().unwrap_or(0), p.next().unwrap_or(0));
    (a.min(0xffff) << 16) | (b.min(0xff) << 8) | c.min(0xff)
}

/// Le Info.plist du .component : celui des autres paquets, plus la liste AudioComponents (un dict par composant ;
/// factoryFunction est le symbole C que l'hote cherche dans le dylib).
pub fn component_plist(version: &str) -> String {
    let mut list = String::from("\t<key>AudioComponents</key>\n\t<array>\n");
    for c in &AU_COMPONENTS {
        list.push_str(&format!(
            "\t\t<dict>\n\
             \t\t\t<key>name</key>\n\t\t\t<string>{AU_VENDOR}: {}</string>\n\
             \t\t\t<key>description</key>\n\t\t\t<string>{}</string>\n\
             \t\t\t<key>factoryFunction</key>\n\t\t\t<string>GetPluginFactoryAUV2_{}</string>\n\
             \t\t\t<key>manufacturer</key>\n\t\t\t<string>{AU_MANUFACTURER}</string>\n\
             \t\t\t<key>subtype</key>\n\t\t\t<string>{}</string>\n\
             \t\t\t<key>type</key>\n\t\t\t<string>{}</string>\n\
             \t\t\t<key>version</key>\n\t\t\t<integer>{}</integer>\n\
             \t\t</dict>\n",
            c.name,
            c.description,
            c.index,
            c.subtype,
            c.ty,
            au_version(version)
        ));
    }
    list.push_str("\t</array>\n");
    info_plist("component", version).replacen("</dict>\n</plist>", &format!("{list}</dict>\n</plist>"), 1)
}

/// Un paquet macOS : <root>/Contents/{Info.plist, PkgInfo, MacOS/<BUNDLE>}.
fn mac_bundle(dylib: &Path, root: &Path, plist: &str) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(root);
    let contents = root.join("Contents");
    copy_file(dylib, &contents.join("MacOS").join(BUNDLE))?;
    let w = |p: PathBuf, s: &str| std::fs::write(&p, s).map_err(|e| format!("cannot write {} ({e})", p.display()));
    w(contents.join("Info.plist"), plist)?;
    w(contents.join("PkgInfo"), "BNDL????")
}

/// (9 nov., en essai) macOS : <out>/Uppwind Link.component, le meme dylib que le .vst3 (il doit avoir ete construit avec
/// l'option « auv2 », sinon l'hote ne trouve pas GetPluginFactoryAUV2_<n> : auval le dit).
pub fn component(dll: &Path, out: &Path) -> Result<PathBuf, String> {
    if !cfg!(target_os = "macos") {
        return Err("--auv2: Audio Units are macOS only".into());
    }
    if !dll.is_file() {
        return Err(format!("no plugin library at {} (cargo build --release first)", dll.display()));
    }
    let root = out.join(format!("{BUNDLE}.component"));
    mac_bundle(dll, &root, &component_plist(env!("CARGO_PKG_VERSION")))?;
    Ok(root)
}

/// Le paquet : rend son chemin. Windows : la DLL renommee dans Contents\x86_64-win ; macOS : deux paquets (.vst3 et
/// .clap), la meme dylib dans Contents/MacOS (une dylib « universelle » faite par lipo convient aussi).
pub fn bundle(dll: &Path, out: &Path) -> Result<PathBuf, String> {
    if !dll.is_file() {
        return Err(format!("no plugin library at {} (cargo build --release first)", dll.display()));
    }
    let root = out.join(format!("{BUNDLE}.vst3"));
    if cfg!(target_os = "macos") {
        let version = env!("CARGO_PKG_VERSION");
        mac_bundle(dll, &root, &info_plist("vst3", version))?;
        mac_bundle(dll, &out.join(format!("{BUNDLE}.clap")), &info_plist("clap", version))?;
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
    let mut signed = vec![root.clone(), out.join(format!("{BUNDLE}.clap"))];
    // (9 nov., en essai) --auv2 : le .component (Logic Pro)
    if args.iter().any(|a| a == "--auv2") {
        match component(&dll, &out) {
            Ok(c) => {
                println!("auv2:   {}", c.display());
                signed.push(c);
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }
    // macOS : --sign - (ad hoc) ou --sign "Developer ID Application: …"
    if let Some(id) = arg(&args, "--sign") {
        for p in signed {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// (9 nov.) Les codes AU : 4 caracteres ASCII imprimables, le fabricant avec une majuscule, aucun composant en double
    /// (le meme type et le meme sous-type se cacheraient l'un l'autre dans Logic).
    #[test]
    fn au_codes_are_four_printable_characters_and_never_twice() {
        let ok = |s: &str| s.len() == 4 && s.bytes().all(|b| (0x20..0x7f).contains(&b));
        assert!(ok(AU_MANUFACTURER) && AU_MANUFACTURER.bytes().any(|b| b.is_ascii_uppercase()));
        for c in &AU_COMPONENTS {
            assert!(ok(c.ty) && ok(c.subtype), "{} {}", c.ty, c.subtype);
            assert!(["aufx", "aumu", "aumi"].contains(&c.ty), "a type clap-wrapper knows: {}", c.ty);
        }
        for (i, a) in AU_COMPONENTS.iter().enumerate() {
            for b in &AU_COMPONENTS[i + 1..] {
                assert!((a.ty, a.subtype) != (b.ty, b.subtype) && a.subtype != b.subtype, "{} {}", a.subtype, b.subtype);
            }
        }
    }

    #[test]
    fn au_version_packs_major_minor_patch() {
        assert_eq!(au_version("0.1.0"), 0x0000_0100);
        assert_eq!(au_version("1.2.3"), 0x0001_0203);
        assert_eq!(au_version("2"), 0x0002_0000);
    }

    /// Le Info.plist du .component : un dict par composant, avec sa fonction d'usine, apres les cles du paquet.
    #[test]
    fn the_component_plist_lists_each_au_with_its_factory() {
        let p = component_plist("0.1.0");
        assert!(p.contains("<string>com.uppwind.link.component</string>") && p.contains("<key>AudioComponents</key>"));
        assert_eq!(p.matches("<key>factoryFunction</key>").count(), AU_COMPONENTS.len());
        assert!(p.contains("<string>Uppwind: Uppwind Link Return</string>"));
        for c in &AU_COMPONENTS {
            assert!(p.contains(&format!("<string>{}</string>\n\t\t\t<key>type</key>\n\t\t\t<string>{}</string>", c.subtype, c.ty)), "{}", c.subtype);
        }
        assert_eq!(p.matches("<string>GetPluginFactoryAUV2_0</string>").count(), 1, "Return");
        assert_eq!(p.matches("<string>GetPluginFactoryAUV2_1</string>").count(), 2, "Notes, as an instrument and as a MIDI effect");
        assert!(p.contains("<integer>256</integer>") && p.trim_end().ends_with("</dict>\n</plist>"));
    }

    /// macOS : plutil (celui d'Apple) lit le Info.plist du .component sans erreur.
    #[cfg(target_os = "macos")]
    #[test]
    fn plutil_accepts_the_component_plist() {
        let dir = std::env::temp_dir().join(format!("uwl-bundle-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("Info.plist");
        std::fs::write(&f, component_plist("0.1.0")).unwrap();
        let out = std::process::Command::new("plutil").arg("-lint").arg(&f).output().expect("plutil");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    }
}
