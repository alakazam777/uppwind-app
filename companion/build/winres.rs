// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (24 oct., la fenetre d'installation) Les ressources Windows de l'exe, ecrites sans rc.exe : l'icone
//! (assets/uppwind-link.ico, le disque d'Uppwind Records) et le bloc de version (le Gestionnaire des taches et les
//! Proprietes du fichier disent « Uppwind Link », Uppwind, la version ; l'assistant y lit la version d'une copie
//! installee). Un fichier .res est une suite d'entrees que link.exe prend en entree comme un .obj.
//! Et, pour les pages de l'interface (src/ui/), le disque en base64 (ui_assets.rs dans OUT_DIR).

use std::path::{Path, PathBuf};

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
const LANG_EN_US: u16 = 0x0409;

fn pad4(b: &mut Vec<u8>) {
    while b.len() % 4 != 0 {
        b.push(0);
    }
}

/// Une entree du .res : l'en-tete de 32 octets (type et nom en ordinaux), puis les donnees, alignees sur 4.
fn entry(out: &mut Vec<u8>, ty: u16, id: u16, flags: u16, lang: u16, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes());
    out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    out.extend_from_slice(&ty.to_le_bytes());
    out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // DataVersion
    out.extend_from_slice(&flags.to_le_bytes()); // MemoryFlags
    out.extend_from_slice(&lang.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // Version
    out.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
    out.extend_from_slice(data);
    pad4(out);
}

/// Un bloc de VS_VERSIONINFO : wLength, wValueLength, wType, la cle en UTF-16, la valeur, les enfants.
fn vblock(key: &str, w_type: u16, value: &[u8], value_len: u16, children: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0u8; 6];
    for c in key.encode_utf16().chain([0]) {
        b.extend_from_slice(&c.to_le_bytes());
    }
    pad4(&mut b);
    b.extend_from_slice(value);
    for c in children {
        pad4(&mut b);
        b.extend_from_slice(c);
    }
    let len = b.len() as u16;
    b[0..2].copy_from_slice(&len.to_le_bytes());
    b[2..4].copy_from_slice(&value_len.to_le_bytes());
    b[4..6].copy_from_slice(&w_type.to_le_bytes());
    b
}

fn vstring(key: &str, text: &str) -> Vec<u8> {
    let w: Vec<u16> = text.encode_utf16().chain([0]).collect();
    let bytes: Vec<u8> = w.iter().flat_map(|c| c.to_le_bytes()).collect();
    vblock(key, 1, &bytes, w.len() as u16, &[])
}

fn version_info(major: u16, minor: u16, patch: u16, version: &str) -> Vec<u8> {
    let ms = ((major as u32) << 16) | minor as u32;
    let ls = (patch as u32) << 16;
    let mut fixed = Vec::new();
    for v in [0xFEEF04BDu32, 0x0001_0000, ms, ls, ms, ls, 0x3F, 0, 0x0004_0004, 1, 0, 0, 0] {
        fixed.extend_from_slice(&v.to_le_bytes());
    }
    let strings = [
        ("CompanyName", "Uppwind"),
        ("FileDescription", "Uppwind Link"),
        ("FileVersion", version),
        ("InternalName", "uppwind-companion"),
        // le titulaire du copyright : le meme partout (LICENSE-MIT, les en-tetes SPDX, --license, CompanyName ici et le
        // Publisher des Applications installees, wizard/arp.rs)
        ("LegalCopyright", "Copyright (C) 2026 Uppwind. GPL-3.0-or-later"),
        ("OriginalFilename", "uppwind-companion.exe"),
        ("ProductName", "Uppwind Link"),
        ("ProductVersion", version),
    ];
    let table = vblock("040904B0", 1, &[], 0, &strings.iter().map(|(k, v)| vstring(k, v)).collect::<Vec<_>>());
    let sfi = vblock("StringFileInfo", 1, &[], 0, &[table]);
    let translation = vblock("Translation", 0, &[0x09, 0x04, 0xB0, 0x04], 4, &[]);
    let vfi = vblock("VarFileInfo", 1, &[], 0, &[translation]);
    vblock("VS_VERSION_INFO", 0, &fixed, 52, &[sfi, vfi])
}

/// Le .res : l'icone (chaque image de l'.ico, puis le groupe, id 1) et la version (id 1).
fn resources(ico: &[u8], major: u16, minor: u16, patch: u16, version: &str) -> Vec<u8> {
    let rd16 = |o: usize| u16::from_le_bytes([ico[o], ico[o + 1]]);
    let rd32 = |o: usize| u32::from_le_bytes([ico[o], ico[o + 1], ico[o + 2], ico[o + 3]]);
    assert!(ico.len() > 6 && rd16(0) == 0 && rd16(2) == 1, "assets/uppwind-link.ico is not an icon file");
    let n = rd16(4) as usize;
    let mut out = Vec::new();
    entry(&mut out, 0, 0, 0, 0, &[]);
    let mut group = Vec::new();
    group.extend_from_slice(&0u16.to_le_bytes());
    group.extend_from_slice(&1u16.to_le_bytes());
    group.extend_from_slice(&(n as u16).to_le_bytes());
    for i in 0..n {
        let e = 6 + 16 * i;
        let (size, offset) = (rd32(e + 8) as usize, rd32(e + 12) as usize);
        assert!(offset + size <= ico.len(), "a truncated icon image");
        let id = (i + 1) as u16;
        entry(&mut out, RT_ICON, id, 0x1010, LANG_EN_US, &ico[offset..offset + size]);
        // GRPICONDIRENTRY : les 4 premiers octets de l'ICONDIRENTRY, 1 plan, les bits, la taille, l'id de l'image
        group.extend_from_slice(&ico[e..e + 4]);
        group.extend_from_slice(&1u16.to_le_bytes());
        group.extend_from_slice(&rd16(e + 6).max(32).to_le_bytes());
        group.extend_from_slice(&(size as u32).to_le_bytes());
        group.extend_from_slice(&id.to_le_bytes());
    }
    entry(&mut out, RT_GROUP_ICON, 1, 0x1030, LANG_EN_US, &group);
    entry(&mut out, RT_VERSION, 1, 0x0030, LANG_EN_US, &version_info(major, minor, patch, version));
    out
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for (i, shift) in [18, 12, 6, 0].iter().enumerate() {
            s.push(if i <= c.len() { T[((n >> shift) & 63) as usize] as char } else { '=' });
        }
    }
    s
}

pub fn emit(here: &Path) {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let assets = here.join("assets");
    println!("cargo:rerun-if-changed={}", assets.join("uppwind-link.ico").display());
    println!("cargo:rerun-if-changed={}", assets.join("mark-64.png").display());
    let mark = std::fs::read(assets.join("mark-64.png")).expect("assets/mark-64.png");
    let ui = format!("// genere par build/winres.rs : ne pas modifier\npub const MARK_PNG_BASE64: &str = {:?};\n", base64(&mark));
    std::fs::write(out_dir.join("ui_assets.rs"), ui).unwrap();
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os != "windows" || env != "msvc" {
        return;
    }
    let ico = std::fs::read(assets.join("uppwind-link.ico")).expect("assets/uppwind-link.ico");
    let num = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u16>().ok()).unwrap_or(0);
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let res = resources(&ico, num("CARGO_PKG_VERSION_MAJOR"), num("CARGO_PKG_VERSION_MINOR"), num("CARGO_PKG_VERSION_PATCH"), &version);
    let path = out_dir.join("uppwind-link.res");
    std::fs::write(&path, res).unwrap();
    // l'exe seulement (pas les essais) : link.exe convertit lui-meme le .res
    println!("cargo:rustc-link-arg-bins={}", path.display());
}
