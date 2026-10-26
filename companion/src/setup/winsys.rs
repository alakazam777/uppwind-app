// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! La vraie machine : le registre (lu avec KEY_READ ; ecrit seulement pour --fl-settings, et seulement les cles de
//! `env::write_allowed` : les reglages de FL sous HKCU, jamais HKLM), les ports MIDI (WinMM, les memes noms que Chrome,
//! Live et FL), les peripheriques audio (MMDevice : nom, format partage, id), les programmes qui tournent (Toolhelp), le
//! dossier Documents (dossier connu). Tout le reste est en lecture seule.

use super::env::{write_allowed, Endpoint, RegValue, Registry};
use std::path::PathBuf;

pub struct RealRegistry;

/// Une ecriture hors des cles permises : refusee avant tout appel a Windows.
fn guard(path: &str) -> Result<(), String> {
    if write_allowed(path, cfg!(test)) {
        Ok(())
    } else {
        Err(format!("refused to write {path}: the companion only writes FL Studio's MIDI settings under HKCU"))
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegDeleteValueW, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, KEY_WOW64_64KEY, REG_OPTION_NON_VOLATILE, REG_VALUE_TYPE,
    };

    /// Une cle ouverte ; fermee a la fin.
    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }

    fn split(path: &str) -> Option<(HKEY, &str)> {
        let (root, sub) = path.split_once('\\').unwrap_or((path, ""));
        let hroot = match root.to_ascii_uppercase().as_str() {
            "HKCU" | "HKEY_CURRENT_USER" => HKEY_CURRENT_USER,
            "HKLM" | "HKEY_LOCAL_MACHINE" => HKEY_LOCAL_MACHINE,
            _ => return None,
        };
        Some((hroot, sub))
    }

    fn open(path: &str) -> Option<Key> {
        let (hroot, sub) = split(path)?;
        let mut h = HKEY::default();
        // KEY_READ : la lecture n'ouvre jamais une cle en ecriture
        let r = unsafe { RegOpenKeyExW(hroot, &HSTRING::from(sub), None, KEY_READ | KEY_WOW64_64KEY, &mut h) };
        (r == ERROR_SUCCESS).then_some(Key(h))
    }

    /// Ouverte en ecriture (creee au besoin) : seulement apres `guard`, seulement sous HKCU.
    fn open_for_write(path: &str, create: bool) -> Result<Option<Key>, String> {
        guard(path)?;
        let Some((hroot, sub)) = split(path) else { return Err(format!("{path}: not a registry path")) };
        if hroot != HKEY_CURRENT_USER {
            return Err(format!("refused to write {path}: HKCU only"));
        }
        let mut h = HKEY::default();
        let r = unsafe {
            if create {
                RegCreateKeyExW(hroot, &HSTRING::from(sub), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE | KEY_WOW64_64KEY, None, &mut h, None)
            } else {
                RegOpenKeyExW(hroot, &HSTRING::from(sub), None, KEY_SET_VALUE | KEY_WOW64_64KEY, &mut h)
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

    impl Registry for RealRegistry {
        fn subkeys(&self, path: &str) -> Vec<String> {
            let Some(k) = open(path) else { return vec![] };
            let mut out = Vec::new();
            for i in 0..100_000u32 {
                let mut buf = [0u16; 512];
                let mut len = buf.len() as u32;
                let r = unsafe { RegEnumKeyExW(k.0, i, Some(PWSTR(buf.as_mut_ptr())), &mut len, None, None, None, None) };
                if r != ERROR_SUCCESS {
                    break;
                }
                out.push(String::from_utf16_lossy(&buf[..len as usize]));
            }
            out
        }

        fn value(&self, path: &str, name: &str) -> Option<RegValue> {
            self.raw(path, name).map(|(ty, data)| RegValue::from_raw(ty, &data))
        }

        fn raw(&self, path: &str, name: &str) -> Option<(u32, Vec<u8>)> {
            let k = open(path)?;
            let n = HSTRING::from(name);
            let mut ty = REG_VALUE_TYPE::default();
            let mut size = 0u32;
            let r = unsafe { RegQueryValueExW(k.0, PCWSTR(n.as_ptr()), None, Some(&mut ty), None, Some(&mut size)) };
            if r != ERROR_SUCCESS && r != ERROR_MORE_DATA {
                return None;
            }
            let mut data = vec![0u8; size as usize + 2];
            let mut size2 = size;
            let r = unsafe { RegQueryValueExW(k.0, PCWSTR(n.as_ptr()), None, Some(&mut ty), Some(data.as_mut_ptr()), Some(&mut size2)) };
            if r != ERROR_SUCCESS {
                return None;
            }
            data.truncate(size2 as usize);
            Some((ty.0, data))
        }

        fn value_names(&self, path: &str) -> Vec<String> {
            let Some(k) = open(path) else { return vec![] };
            let mut out = Vec::new();
            let mut buf = vec![0u16; 16_384];
            for i in 0..100_000u32 {
                let mut len = buf.len() as u32;
                let r = unsafe { RegEnumValueW(k.0, i, Some(PWSTR(buf.as_mut_ptr())), &mut len, None, None, None, None) };
                if r != ERROR_SUCCESS {
                    break;
                }
                out.push(String::from_utf16_lossy(&buf[..len as usize]));
            }
            out
        }

        fn key_exists(&self, path: &str) -> bool {
            open(path).is_some()
        }

        fn set_value(&self, path: &str, name: &str, v: &RegValue) -> Result<(), String> {
            let k = open_for_write(path, true)?.ok_or_else(|| format!("cannot create {path}"))?;
            let (ty, data) = v.to_raw();
            let r = unsafe { RegSetValueExW(k.0, &HSTRING::from(name), None, REG_VALUE_TYPE(ty), Some(&data)) };
            if r == ERROR_SUCCESS {
                Ok(())
            } else {
                Err(format!("cannot write {path}\\{name} (Windows error {})", r.0))
            }
        }

        fn delete_value(&self, path: &str, name: &str) -> Result<(), String> {
            let Some(k) = open_for_write(path, false)? else { return Ok(()) };
            let r = unsafe { RegDeleteValueW(k.0, &HSTRING::from(name)) };
            if r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                Err(format!("cannot remove {path}\\{name} (Windows error {})", r.0))
            }
        }

        fn delete_key(&self, path: &str) -> Result<(), String> {
            guard(path)?;
            if !self.key_exists(path) {
                return Ok(());
            }
            if !self.subkeys(path).is_empty() || !self.value_names(path).is_empty() {
                return Err(format!("{path} is not empty"));
            }
            let Some((hroot, sub)) = split(path) else { return Err(format!("{path}: not a registry path")) };
            if hroot != HKEY_CURRENT_USER || sub.is_empty() {
                return Err(format!("refused to remove {path}"));
            }
            // RegDeleteKeyW ne retire qu'une cle sans sous-cle
            let r = unsafe { RegDeleteKeyW(hroot, &HSTRING::from(sub)) };
            if r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                Err(format!("cannot remove {path} (Windows error {})", r.0))
            }
        }
    }

    /// Les essais : efface l'arbre d'essai qu'ils ont cree (seulement sous TEST_KEY).
    #[cfg(test)]
    pub fn delete_test_tree(path: &str) {
        use windows::Win32::System::Registry::RegDeleteTreeW;
        assert!(write_allowed(path, true), "{path} is not under the test key");
        let (_, sub) = split(path).unwrap();
        unsafe {
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(sub));
            let _ = RegDeleteKeyW(HKEY_CURRENT_USER, &HSTRING::from(sub));
            // la cle d'essai elle-meme, si plus rien n'est dessous
            let (_, top) = split(super::super::env::TEST_KEY).unwrap();
            let _ = RegDeleteKeyW(HKEY_CURRENT_USER, &HSTRING::from(top));
        }
    }

    fn wide(b: &[u16]) -> String {
        String::from_utf16_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())])
    }

    pub fn midi_inputs() -> Vec<String> {
        use windows::Win32::Media::Audio::{midiInGetDevCapsW, midiInGetNumDevs, MIDIINCAPSW};
        let mut out = Vec::new();
        unsafe {
            for i in 0..midiInGetNumDevs() {
                let mut caps = MIDIINCAPSW::default();
                if midiInGetDevCapsW(i as usize, &mut caps, std::mem::size_of::<MIDIINCAPSW>() as u32) == 0 {
                    let name = caps.szPname;
                    out.push(wide(&name));
                }
            }
        }
        out
    }

    pub fn midi_outputs() -> Vec<String> {
        use windows::Win32::Media::Audio::{midiOutGetDevCapsW, midiOutGetNumDevs, MIDIOUTCAPSW};
        let mut out = Vec::new();
        unsafe {
            for i in 0..midiOutGetNumDevs() {
                let mut caps = MIDIOUTCAPSW::default();
                if midiOutGetDevCapsW(i as usize, &mut caps, std::mem::size_of::<MIDIOUTCAPSW>() as u32) == 0 {
                    let name = caps.szPname;
                    out.push(wide(&name));
                }
            }
        }
        out
    }

    /// Les peripheriques (actifs, debranches, desactives) : lecture (`render`) ou enregistrement.
    pub fn endpoints(render: bool) -> Vec<Endpoint> {
        use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
        use windows::Win32::Media::Audio::{
            eCapture, eRender, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE, DEVICE_STATE_ACTIVE, DEVICE_STATE_DISABLED, DEVICE_STATE_UNPLUGGED, PKEY_AudioEngine_DeviceFormat,
        };
        use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
        use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ};
        use windows::Win32::System::Variant::{VT_BLOB, VT_LPWSTR};
        let mut out = Vec::new();
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return out };
            let Ok(col) = en.EnumAudioEndpoints(if render { eRender } else { eCapture }, DEVICE_STATE(DEVICE_STATE_ACTIVE.0 | DEVICE_STATE_DISABLED.0 | DEVICE_STATE_UNPLUGGED.0)) else { return out };
            let n = col.GetCount().unwrap_or(0);
            for i in 0..n {
                let Ok(dev) = col.Item(i) else { continue };
                let id = match dev.GetId() {
                    Ok(p) => {
                        let s = p.to_string().unwrap_or_default();
                        CoTaskMemFree(Some(p.0 as *const _));
                        s
                    }
                    Err(_) => String::new(),
                };
                let active = dev.GetState().map(|s| s == DEVICE_STATE_ACTIVE).unwrap_or(false);
                let Ok(store) = dev.OpenPropertyStore(STGM_READ) else { continue };
                let mut name = String::new();
                if let Ok(mut v) = store.GetValue(&PKEY_Device_FriendlyName) {
                    if v.Anonymous.Anonymous.vt == VT_LPWSTR {
                        name = v.Anonymous.Anonymous.Anonymous.pwszVal.to_string().unwrap_or_default();
                    }
                    let _ = PropVariantClear(&mut v);
                }
                let mut rate = 0u32;
                if let Ok(mut v) = store.GetValue(&PKEY_AudioEngine_DeviceFormat) {
                    if v.Anonymous.Anonymous.vt == VT_BLOB {
                        let b = v.Anonymous.Anonymous.Anonymous.blob;
                        if b.cbSize >= 8 && !b.pBlobData.is_null() {
                            // WAVEFORMATEX : wFormatTag u16, nChannels u16, nSamplesPerSec u32
                            let s = std::slice::from_raw_parts(b.pBlobData, 8);
                            rate = u32::from_le_bytes([s[4], s[5], s[6], s[7]]);
                        }
                    }
                    let _ = PropVariantClear(&mut v);
                }
                out.push(Endpoint { name, rate, id, active });
            }
        }
        out
    }

    pub fn processes() -> Vec<String> {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
        let mut out = Vec::new();
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
            let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            if Process32FirstW(snap, &mut e).is_ok() {
                loop {
                    let n = wide(&e.szExeFile);
                    if !out.contains(&n) {
                        out.push(n);
                    }
                    if Process32NextW(snap, &mut e).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
        }
        out
    }

    pub fn documents_folder() -> Option<PathBuf> {
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
        unsafe {
            let p = SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None).ok()?;
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const _));
            s.filter(|s| !s.is_empty()).map(PathBuf::from)
        }
    }
}

#[cfg(all(windows, test))]
pub use imp::delete_test_tree;
#[cfg(windows)]
pub use imp::{documents_folder, endpoints, midi_inputs, midi_outputs, processes};

#[cfg(not(windows))]
impl Registry for RealRegistry {
    fn subkeys(&self, _path: &str) -> Vec<String> {
        vec![]
    }
    fn value(&self, _path: &str, _name: &str) -> Option<RegValue> {
        None
    }
}
#[cfg(not(windows))]
pub fn midi_inputs() -> Vec<String> {
    vec![]
}
#[cfg(not(windows))]
pub fn midi_outputs() -> Vec<String> {
    vec![]
}
#[cfg(not(windows))]
pub fn endpoints(_render: bool) -> Vec<Endpoint> {
    vec![]
}
#[cfg(not(windows))]
pub fn processes() -> Vec<String> {
    vec![]
}
#[cfg(not(windows))]
pub fn documents_folder() -> Option<PathBuf> {
    None
}
