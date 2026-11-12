// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! L'entree « Applications installees » (Parametres › Applications, et Programmes et fonctionnalites) :
//! HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\UppwindLink. HKCU : sans administrateur, pour cet
//! utilisateur seulement. « Desinstaller » y lance `"<exe installe>" --uninstall` (une fenetre de confirmation).
//!
//! Les essais et le bac a sable n'ecrivent jamais la vraie cle : UWC_UNINSTALL_KEY=<cle sous
//! HKCU\Software\UppwindLinkTest> la remplace, `--sandbox` prend SANDBOX_KEY ; compile pour les essais, l'ecrivain du
//! vrai registre refuse toute cle hors de HKCU\Software\UppwindLinkTest.

use crate::setup::env::{RegValue, Registry, TEST_KEY};

pub const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\UppwindLink";
pub const KEY_ENV: &str = "UWC_UNINSTALL_KEY";
pub const SANDBOX_KEY: &str = r"HKCU\Software\UppwindLinkTest\Sandbox\Uninstall\UppwindLink";
pub const DISPLAY_NAME: &str = "Uppwind Link";
pub const PUBLISHER: &str = "Uppwind";

/// Ce que l'entree dit.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub version: String,
    /// le dossier de l'application (%LOCALAPPDATA%\Uppwind Link)
    pub location: String,
    /// l'exe installe
    pub exe: String,
    /// des options en plus pour la desinstallation (le bac a sable : --sandbox "<dossier>")
    pub extra_args: String,
    /// la taille, en Ko
    pub size_kb: u64,
    /// AAAAMMJJ
    pub date: String,
}

fn canon(path: &str) -> String {
    path.replace('/', "\\").trim_matches('\\').to_ascii_lowercase().replacen("hkey_current_user", "hkcu", 1)
}

/// Sous la cle d'essai (et pas la cle d'essai elle-meme), sans « .. ».
pub fn under_test_key(path: &str) -> bool {
    let p = canon(path);
    !p.split('\\').any(|s| s.is_empty() || s == "." || s == "..") && p.starts_with(&format!("{}\\", canon(TEST_KEY)))
}

/// Ou l'ecrivain du vrai registre accepte d'ecrire : la vraie entree exactement, ou sous la cle d'essai ; compile pour
/// les essais (`test`), sous la cle d'essai seulement.
pub fn write_allowed(path: &str, test: bool) -> bool {
    under_test_key(path) || (!test && canon(path) == canon(KEY))
}

/// La cle a utiliser : UWC_UNINSTALL_KEY (sous la cle d'essai), sinon celle du bac a sable, sinon la vraie.
pub fn key_from(env_override: Option<&str>, sandbox: bool) -> Result<String, String> {
    if let Some(k) = env_override.map(str::trim).filter(|k| !k.is_empty()) {
        if !under_test_key(k) {
            return Err(format!("{KEY_ENV} must name a key under {TEST_KEY} (got {k})"));
        }
        return Ok(k.to_string());
    }
    Ok(if sandbox { SANDBOX_KEY } else { KEY }.to_string())
}

/// La cle de ce lancement : un essai (--sandbox, ou UWC_INSTALL_ROOT) n'ecrit jamais la vraie.
pub fn key() -> Result<String, String> {
    key_from(std::env::var(KEY_ENV).ok().as_deref(), super::trial())
}

/// Les valeurs, dans l'ordre ou on les ecrit.
pub fn values(e: &Entry) -> Vec<(&'static str, RegValue)> {
    let extra = if e.extra_args.is_empty() { String::new() } else { format!(" {}", e.extra_args) };
    vec![
        ("DisplayName", RegValue::Str(DISPLAY_NAME.into())),
        ("DisplayVersion", RegValue::Str(e.version.clone())),
        ("Publisher", RegValue::Str(PUBLISHER.into())),
        ("InstallLocation", RegValue::Str(e.location.clone())),
        ("DisplayIcon", RegValue::Str(format!("{},0", e.exe))),
        ("UninstallString", RegValue::Str(format!("\"{}\" --uninstall{extra}", e.exe))),
        ("QuietUninstallString", RegValue::Str(format!("\"{}\" --uninstall --quiet{extra}", e.exe))),
        ("InstallDate", RegValue::Str(e.date.clone())),
        ("EstimatedSize", RegValue::Num(e.size_kb.min(u32::MAX as u64) as i64)),
        ("NoModify", RegValue::Num(1)),
        ("NoRepair", RegValue::Num(1)),
    ]
}

/// Ecrit l'entree (la remplace si elle est la) → ce qui a ete fait.
pub fn write(reg: &dyn Registry, key: &str, e: &Entry) -> Result<Vec<String>, String> {
    for (name, v) in values(e) {
        reg.set_value(key, name, &v)?;
    }
    // relue : une valeur qui ne se relit pas est une erreur
    for (name, v) in values(e) {
        if reg.value(key, name).map(|x| x.text()) != Some(v.text()) {
            return Err(format!("{key}\\{name} does not read back as written"));
        }
    }
    Ok(vec![format!("Listed in Windows' Installed apps as \"{DISPLAY_NAME}\" {} ({key}).", e.version)])
}

/// Retire l'entree (ses valeurs, puis la cle) → ce qui a ete fait.
pub fn remove(reg: &dyn Registry, key: &str) -> Result<Vec<String>, String> {
    if !reg.key_exists(key) {
        return Ok(vec![]);
    }
    for name in reg.value_names(key) {
        reg.delete_value(key, &name)?;
    }
    reg.delete_key(key)?;
    Ok(vec![format!("Removed from Windows' Installed apps ({key}).")])
}

/// (12 nov.) Les valeurs de l'entree telles qu'elles sont (nom → donnee, rangees par nom) : `--self-update` les lit
/// avant d'installer, pour les remettre s'il remet la copie d'avant. Vide : pas d'entree. La donnee passe par
/// `RegValue` (texte, nombre, octets) : toutes celles qu'Uppwind Link ecrit sont du texte (REG_SZ) ou des nombres
/// (REG_DWORD) et reviennent telles quelles ; une valeur posee par un autre programme dans un autre type (REG_EXPAND_SZ,
/// REG_MULTI_SZ…) reviendrait dans le type le plus proche (relecture du 12 nov.).
pub fn snapshot(reg: &dyn Registry, key: &str) -> Vec<(String, RegValue)> {
    let mut v: Vec<(String, RegValue)> = reg.value_names(key).into_iter().filter_map(|n| reg.value(key, &n).map(|x| (n, x))).collect();
    v.sort_by_key(|(n, _)| n.to_ascii_lowercase());
    v
}

/// Deux photos de l'entree disent-elles la meme chose ? Les noms sans casse, comme Windows (relecture du 12 nov.).
fn same_values(a: &[(String, RegValue)], b: &[(String, RegValue)]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|((na, va), (nb, vb))| na.eq_ignore_ascii_case(nb) && va == vb)
}

/// (12 nov.) Remet l'entree telle que `snapshot` l'a lue : ses valeurs (celles ajoutees depuis s'en vont), ou plus
/// d'entree s'il n'y en avait pas. Sans changement depuis, rien. Sinon le premier lancement d'une version plus recente
/// lirait la version qui n'a pas pris la place, et ouvrirait la copie installee au lieu de l'installer (essai du
/// 12 nov. : une 0.4.1 qui ne demarrait pas, la 0.4.0 remise, l'entree disait 0.4.1). Les valeurs d'avant sont
/// reecrites d'abord, la version en tete, puis celles ajoutees depuis s'en vont : une ecriture qui echoue en route
/// laisse au moins la version d'avant. → ce qui a ete fait.
pub fn restore(reg: &dyn Registry, key: &str, before: &[(String, RegValue)]) -> Result<Vec<String>, String> {
    if same_values(&snapshot(reg, key), before) {
        return Ok(vec![]);
    }
    if before.is_empty() {
        return remove(reg, key);
    }
    let is_version = |n: &str| n.eq_ignore_ascii_case("DisplayVersion");
    for (name, v) in before.iter().filter(|(n, _)| is_version(n)).chain(before.iter().filter(|(n, _)| !is_version(n))) {
        reg.set_value(key, name, v)?;
    }
    for name in reg.value_names(key) {
        if !before.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name)) {
            reg.delete_value(key, &name)?;
        }
    }
    if !same_values(&snapshot(reg, key), before) {
        return Err(format!("{key} does not read back as it was before the update"));
    }
    let version = before.iter().find(|(n, _)| n.eq_ignore_ascii_case("DisplayVersion")).and_then(|(_, v)| v.as_str().map(str::to_string)).unwrap_or_default();
    Ok(vec![format!("Windows' Installed apps lists \"{DISPLAY_NAME}\" {version} again ({key}).")])
}

/// La version et le dossier que l'entree annonce (le premier lancement compare la version).
pub fn installed(reg: &dyn Registry, key: &str) -> Option<(String, String)> {
    let v = reg.str(key, "DisplayVersion")?;
    Some((v, reg.str(key, "InstallLocation").unwrap_or_default()))
}

/// Le vrai registre pour cette entree : lu comme d'habitude, ecrit seulement sous `write_allowed`.
pub struct ArpRegistry;

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::setup::winsys::RealRegistry;
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, KEY_WOW64_64KEY,
        REG_OPTION_NON_VOLATILE, REG_VALUE_TYPE,
    };

    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }

    /// « HKCU\… » → le chemin sous HKEY_CURRENT_USER, apres le controle ; jamais HKLM.
    fn sub(path: &str) -> Result<String, String> {
        if !write_allowed(path, cfg!(test)) {
            return Err(format!("refused to write {path}: only the Installed apps entry of Uppwind Link"));
        }
        let p = path.replace('/', "\\");
        let (root, rest) = p.split_once('\\').ok_or_else(|| format!("{path}: not a registry path"))?;
        if !root.eq_ignore_ascii_case("HKCU") && !root.eq_ignore_ascii_case("HKEY_CURRENT_USER") {
            return Err(format!("refused to write {path}: HKCU only"));
        }
        Ok(rest.to_string())
    }

    fn open(path: &str, create: bool) -> Result<Option<Key>, String> {
        let s = sub(path)?;
        let mut h = HKEY::default();
        let r = unsafe {
            if create {
                RegCreateKeyExW(HKEY_CURRENT_USER, &HSTRING::from(s.as_str()), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE | KEY_WOW64_64KEY, None, &mut h, None)
            } else {
                RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(s.as_str()), None, KEY_SET_VALUE | KEY_WOW64_64KEY, &mut h)
            }
        };
        if r == ERROR_SUCCESS {
            Ok(Some(Key(h)))
        } else if r == ERROR_FILE_NOT_FOUND && !create {
            Ok(None)
        } else {
            Err(format!("cannot open {path} for writing (Windows error {})", r.0))
        }
    }

    impl Registry for ArpRegistry {
        fn subkeys(&self, path: &str) -> Vec<String> {
            RealRegistry.subkeys(path)
        }
        fn value(&self, path: &str, name: &str) -> Option<RegValue> {
            RealRegistry.value(path, name)
        }
        fn value_names(&self, path: &str) -> Vec<String> {
            RealRegistry.value_names(path)
        }
        fn key_exists(&self, path: &str) -> bool {
            RealRegistry.key_exists(path)
        }
        fn set_value(&self, path: &str, name: &str, v: &RegValue) -> Result<(), String> {
            let k = open(path, true)?.ok_or_else(|| format!("cannot create {path}"))?;
            let (ty, data) = v.to_raw();
            let r = unsafe { RegSetValueExW(k.0, &HSTRING::from(name), None, REG_VALUE_TYPE(ty), Some(&data)) };
            if r == ERROR_SUCCESS {
                Ok(())
            } else {
                Err(format!("cannot write {path}\\{name} (Windows error {})", r.0))
            }
        }
        fn delete_value(&self, path: &str, name: &str) -> Result<(), String> {
            let Some(k) = open(path, false)? else { return Ok(()) };
            let r = unsafe { RegDeleteValueW(k.0, &HSTRING::from(name)) };
            if r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                Err(format!("cannot remove {path}\\{name} (Windows error {})", r.0))
            }
        }
        fn delete_key(&self, path: &str) -> Result<(), String> {
            let s = sub(path)?;
            if !self.key_exists(path) {
                return Ok(());
            }
            if !self.subkeys(path).is_empty() || !self.value_names(path).is_empty() {
                return Err(format!("{path} is not empty"));
            }
            let r = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, &HSTRING::from(s.as_str())) };
            if r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                Err(format!("cannot remove {path} (Windows error {})", r.0))
            }
        }
    }
}

#[cfg(not(windows))]
impl Registry for ArpRegistry {
    fn subkeys(&self, _path: &str) -> Vec<String> {
        vec![]
    }
    fn value(&self, _path: &str, _name: &str) -> Option<RegValue> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::env::FakeRegistry;

    fn entry() -> Entry {
        Entry {
            version: "0.2.0".into(),
            location: r"C:\Users\x\AppData\Local\Uppwind Link".into(),
            exe: r"C:\Users\x\AppData\Local\Uppwind Link\uppwind-companion.exe".into(),
            extra_args: String::new(),
            size_kb: 5123,
            date: "20261024".into(),
        }
    }

    #[test]
    fn the_entry_says_what_windows_shows() {
        let v = values(&entry());
        let get = |n: &str| v.iter().find(|(k, _)| *k == n).map(|(_, x)| x.clone()).unwrap();
        assert_eq!(get("DisplayName"), RegValue::Str("Uppwind Link".into()));
        assert_eq!(get("Publisher"), RegValue::Str("Uppwind".into()));
        assert_eq!(get("UninstallString"), RegValue::Str(r#""C:\Users\x\AppData\Local\Uppwind Link\uppwind-companion.exe" --uninstall"#.into()));
        assert_eq!(get("QuietUninstallString"), RegValue::Str(r#""C:\Users\x\AppData\Local\Uppwind Link\uppwind-companion.exe" --uninstall --quiet"#.into()));
        assert_eq!(get("DisplayIcon"), RegValue::Str(r"C:\Users\x\AppData\Local\Uppwind Link\uppwind-companion.exe,0".into()));
        assert_eq!(get("EstimatedSize"), RegValue::Num(5123));
        assert_eq!((get("NoModify"), get("NoRepair")), (RegValue::Num(1), RegValue::Num(1)));
        let mut sb = entry();
        sb.extra_args = r#"--sandbox "D:\sandbox""#.into();
        let v = values(&sb);
        assert_eq!(v.iter().find(|(k, _)| *k == "UninstallString").unwrap().1, RegValue::Str(r#""C:\Users\x\AppData\Local\Uppwind Link\uppwind-companion.exe" --uninstall --sandbox "D:\sandbox""#.into()));
    }

    #[test]
    fn only_the_entry_or_the_test_key_can_be_written() {
        assert!(write_allowed(KEY, false));
        assert!(write_allowed(r"hkey_current_user/software/microsoft/windows/currentversion/uninstall/uppwindlink", false));
        assert!(!write_allowed(KEY, true), "the tests never write the real entry");
        for no in [
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\UppwindLinkX",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Other",
            r"HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\UppwindLink",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            r"HKCU\Software\UppwindLinkTest",
            r"HKCU\Software\UppwindLinkTest\..\Microsoft",
        ] {
            assert!(!write_allowed(no, false), "{no}");
        }
        assert!(write_allowed(SANDBOX_KEY, true) && write_allowed(SANDBOX_KEY, false));
        assert_eq!(key_from(None, false).unwrap(), KEY);
        assert_eq!(key_from(None, true).unwrap(), SANDBOX_KEY);
        assert_eq!(key_from(Some(r"HKCU\Software\UppwindLinkTest\t1\UppwindLink"), false).unwrap(), r"HKCU\Software\UppwindLinkTest\t1\UppwindLink");
        assert!(key_from(Some(KEY), false).is_err(), "the override is for the test key only");
        assert_eq!(key_from(Some("  "), false).unwrap(), KEY, "an empty override is no override");
    }

    #[test]
    fn write_then_remove_on_a_fake_registry() {
        let r = FakeRegistry::default();
        let k = r"HKCU\Software\UppwindLinkTest\fake\UppwindLink";
        assert_eq!(installed(&r, k), None);
        let did = write(&r, k, &entry()).unwrap();
        assert!(did[0].contains("Installed apps"), "{did:?}");
        assert_eq!(installed(&r, k), Some(("0.2.0".into(), r"C:\Users\x\AppData\Local\Uppwind Link".into())));
        // une reinstallation remplace
        let mut e2 = entry();
        e2.version = "0.3.0".into();
        write(&r, k, &e2).unwrap();
        assert_eq!(installed(&r, k).unwrap().0, "0.3.0");
        assert!(remove(&r, k).unwrap()[0].starts_with("Removed"));
        assert!(!r.key_exists(k));
        assert!(remove(&r, k).unwrap().is_empty(), "nothing to remove");
    }

    #[cfg(windows)]
    #[test]
    fn write_then_remove_on_the_real_registry_under_the_test_key() {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let tree = format!(r"{TEST_KEY}\arp-{}-{t}", std::process::id());
        let k = format!(r"{tree}\UppwindLink");
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                crate::setup::winsys::delete_test_tree(&self.0);
            }
        }
        let _c = Cleanup(tree.clone());
        let reg = ArpRegistry;
        write(&reg, &k, &entry()).unwrap();
        assert_eq!(installed(&reg, &k).unwrap().0, "0.2.0");
        assert_eq!(reg.num(&k, "EstimatedSize"), Some(5123));
        assert_eq!(reg.str(&k, "UninstallString").unwrap(), r#""C:\Users\x\AppData\Local\Uppwind Link\uppwind-companion.exe" --uninstall"#);
        remove(&reg, &k).unwrap();
        assert!(!reg.key_exists(&k));
        // compile pour les essais : la vraie entree est refusee avant tout appel a Windows
        let e = reg.set_value(KEY, "DisplayName", &RegValue::Str("x".into())).unwrap_err();
        assert!(e.starts_with("refused"), "{e}");
        assert!(reg.delete_key(KEY).unwrap_err().starts_with("refused"));
    }

    /// (12 nov.) Une mise a jour qui remet la copie d'avant remet son entree : la version d'avant, ses valeurs telles
    /// qu'elles etaient (celles ajoutees depuis s'en vont), ou plus d'entree s'il n'y en avait pas.
    #[test]
    fn restore_puts_the_entry_back_as_it_was() {
        let r = FakeRegistry::default();
        let k = r"HKCU\Software\UppwindLinkTest\Uninstall\UppwindLink";
        let mut old = entry();
        old.version = "0.4.0".into();
        write(&r, k, &old).unwrap();
        r.set_value(k, "Comments", &RegValue::Str("written by hand".into())).unwrap();
        let before = snapshot(&r, k);
        assert_eq!(before.len(), values(&old).len() + 1);
        assert!(restore(&r, k, &before).unwrap().is_empty(), "unchanged: nothing to do");
        // (relecture) le meme nom dans une autre casse est le meme nom, comme sous Windows
        r.set_value(k, "COMMENTS", &RegValue::Str("written by hand".into())).unwrap();
        assert!(restore(&r, k, &before).unwrap().is_empty(), "only the case of a name differs");
        // la mise a jour ecrit la sienne, puis la copie d'avant revient
        let mut new = entry();
        new.version = "0.4.1".into();
        new.size_kb = 9999;
        write(&r, k, &new).unwrap();
        r.set_value(k, "Extra", &RegValue::Num(1)).unwrap();
        assert_eq!(installed(&r, k).unwrap().0, "0.4.1");
        let did = restore(&r, k, &before).unwrap();
        assert_eq!(did, vec![format!("Windows' Installed apps lists \"Uppwind Link\" 0.4.0 again ({k}).")]);
        assert_eq!(installed(&r, k).unwrap().0, "0.4.0");
        assert_eq!(snapshot(&r, k), before);
        assert_eq!(r.str(k, "Comments").as_deref(), Some("written by hand"));
        assert_eq!(r.value(k, "Extra"), None, "a value added since goes");
        // pas d'entree avant (une reparation) : plus d'entree apres
        let k2 = r"HKCU\Software\UppwindLinkTest\Uninstall\Other";
        let none = snapshot(&r, k2);
        assert!(none.is_empty());
        write(&r, k2, &new).unwrap();
        assert!(restore(&r, k2, &none).unwrap()[0].starts_with("Removed"));
        assert!(!r.key_exists(k2));
    }

    #[cfg(windows)]
    #[test]
    fn restore_on_the_real_registry_under_the_test_key() {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let tree = format!(r"{TEST_KEY}\arp-restore-{}-{t}", std::process::id());
        let k = format!(r"{tree}\UppwindLink");
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                crate::setup::winsys::delete_test_tree(&self.0);
            }
        }
        let _c = Cleanup(tree.clone());
        let reg = ArpRegistry;
        let mut old = entry();
        old.version = "0.4.0".into();
        write(&reg, &k, &old).unwrap();
        let before = snapshot(&reg, &k);
        assert!(!before.is_empty());
        let mut new = entry();
        new.version = "0.4.1".into();
        write(&reg, &k, &new).unwrap();
        restore(&reg, &k, &before).unwrap();
        assert_eq!(installed(&reg, &k).unwrap().0, "0.4.0");
        assert_eq!(snapshot(&reg, &k), before);
    }
}
