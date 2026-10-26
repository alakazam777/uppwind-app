// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Les fenetres de plugins d'Ableton Live.
//!
//! Mesure du 20 oct. (Live 12.4.6, Windows 10) : une fenetre de plugin est une fenetre de premier niveau du
//! processus de Live, POSSEDEE par sa fenetre principale, de style WS_EX_TOOLWINDOW, titree « <Plugin>/<Piste> »
//! (« Diva/2-Diva »). `plugin_windows` fait le tri sur une liste de fenetres brutes (testee avec une fausse liste) ;
//! `enumerate` (Windows seulement) remplit cette liste avec EnumWindows.

use serde::Serialize;

pub const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;

/// Une fenetre de premier niveau telle que Windows la decrit.
#[derive(Debug, Clone, Default)]
pub struct RawWindow {
    pub hwnd: u64,
    pub pid: u32,
    /// Le nom du fichier du processus (« Ableton Live 12 Suite.exe »).
    pub exe: String,
    pub title: String,
    /// La classe de fenetre (AbletonVstPlugClass, Vst3PlugWindow…) : pas encore un critere, gardee pour la partie 2.
    #[allow(dead_code)]
    pub class: String,
    /// La fenetre proprietaire (0 : aucune).
    pub owner: u64,
    pub ex_style: u32,
    pub visible: bool,
    pub minimized: bool,
    /// La zone client, en pixels physiques (macOS : la fenetre entiere, en points).
    pub w: i32,
    pub h: i32,
    /// macOS : le niveau de la fenetre (kCGWindowLayer : 0 normal, 3 flottant…) ; 0 sous Windows
    pub layer: i32,
}

/// Une fenetre de plugin, telle que le studio la voit (`list`).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PluginWindow {
    pub id: u64,
    pub title: String,
    pub plugin: String,
    pub track: String,
    pub w: i32,
    pub h: i32,
    pub minimized: bool,
}

/// Un processus d'Ableton Live ? (« Ableton Live 12 Suite.exe », « Ableton Live 12 Beta.exe », « Ableton Live 11 … »)
pub fn is_live_exe(exe: &str) -> bool {
    let e = exe.to_ascii_lowercase();
    let name = e.rsplit(['\\', '/']).next().unwrap_or(&e);
    name.starts_with("ableton live") && name.ends_with(".exe")
}

/// « Diva/2-Diva » → (« Diva », « 2-Diva »). Le nom du plugin ne contient pas de « / » d'habitude ; la piste, si :
/// on coupe au premier.
pub fn split_title(title: &str) -> Option<(String, String)> {
    let (p, t) = title.split_once('/')?;
    let p = p.trim();
    let t = t.trim();
    if p.is_empty() || t.is_empty() {
        return None;
    }
    Some((p.to_string(), t.to_string()))
}

/// macOS : l'appli de Live telle que CGWindowList la nomme (kCGWindowOwnerName) : « Live » (le nom de son
/// executable), ou « Ableton Live 12 Suite » selon les versions *(a tester)*.
pub fn is_live_app(owner: &str) -> bool {
    let o = owner.trim();
    o == "Live" || (o.starts_with("Ableton Live") && !o.to_ascii_lowercase().ends_with(".exe"))
}

/// Le tri : les fenetres d'un processus de Live, visibles, d'outil, possedees par une fenetre du meme processus,
/// titrees « plugin/piste ». Triees par titre (l'ordre de la liste ne bouge pas d'un appel a l'autre).
pub fn plugin_windows(all: &[RawWindow]) -> Vec<PluginWindow> {
    if cfg!(target_os = "macos") {
        plugin_windows_mac(all)
    } else {
        plugin_windows_win(all)
    }
}

/// Windows : le tri de `plugin_windows` (fenetres d'outil possedees).
pub fn plugin_windows_win(all: &[RawWindow]) -> Vec<PluginWindow> {
    let mut out: Vec<PluginWindow> = all
        .iter()
        .filter(|w| is_live_exe(&w.exe))
        .filter(|w| w.visible && w.owner != 0 && w.ex_style & WS_EX_TOOLWINDOW != 0)
        .filter(|w| all.iter().any(|o| o.hwnd == w.owner && o.pid == w.pid))
        .filter_map(|w| {
            let (plugin, track) = split_title(&w.title)?;
            Some(PluginWindow { id: w.hwnd, title: w.title.clone(), plugin, track, w: w.w, h: w.h, minimized: w.minimized })
        })
        .collect();
    out.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()).then(a.id.cmp(&b.id)));
    out
}

/// macOS (26 oct.) : ni proprietaire ni style d'outil dans CGWindowList. Les fenetres de l'appli Live, a l'ecran, de
/// niveau normal ou flottant (0 a 8), titrees « plugin/piste » ; pas la fenetre principale (« Set … - Ableton Live 12
/// Suite », dont le nom du set peut contenir un « / »). *(a tester sur Live pour Mac : le titre « Diva/2-Diva »)*
pub fn plugin_windows_mac(all: &[RawWindow]) -> Vec<PluginWindow> {
    let mut out: Vec<PluginWindow> = all
        .iter()
        .filter(|w| is_live_app(&w.exe) && w.visible && (0..=8).contains(&w.layer))
        .filter(|w| !w.title.contains(" - Ableton Live"))
        .filter_map(|w| {
            let (plugin, track) = split_title(&w.title)?;
            Some(PluginWindow { id: w.hwnd, title: w.title.clone(), plugin, track, w: w.w, h: w.h, minimized: w.minimized })
        })
        .collect();
    out.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()).then(a.id.cmp(&b.id)));
    out
}

/// Live tourne-t-il (une fenetre de son processus, quelle qu'elle soit) ?
pub fn live_running(all: &[RawWindow]) -> bool {
    all.iter().any(|w| is_live_exe(&w.exe) || (cfg!(target_os = "macos") && is_live_app(&w.exe)))
}

/// macOS : les fenetres du systeme (CGWindowList) → la liste brute. Sans « Enregistrement de l'ecran », les titres
/// des autres applis sont vides : aucune fenetre de plugin ne sort (--list le dit).
#[cfg(target_os = "macos")]
pub fn enumerate() -> Vec<RawWindow> {
    crate::macos::all_windows()
        .into_iter()
        .map(|w| RawWindow {
            hwnd: w.id as u64,
            pid: w.pid.max(0) as u32,
            exe: w.owner,
            title: w.title,
            class: String::new(),
            owner: 0,
            ex_style: 0,
            visible: w.onscreen,
            minimized: false,
            w: w.bounds.size.width.round() as i32,
            h: w.bounds.size.height.round() as i32,
            layer: w.layer,
        })
        .collect()
}

/// Le titre que Live donne a la fenetre d'un device : « <device>/<piste> ».
pub fn expected_title(device: &str, track: &str) -> String {
    format!("{device}/{track}")
}

#[cfg(windows)]
pub use win::enumerate;

#[cfg(not(any(windows, target_os = "macos")))]
pub fn enumerate() -> Vec<RawWindow> {
    Vec::new()
}

#[cfg(windows)]
mod win {
    use super::RawWindow;
    use std::collections::HashMap;
    use windows::core::{BOOL, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetClientRect, GetWindow, GetWindowLongPtrW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        GWL_EXSTYLE, GW_OWNER,
    };

    unsafe extern "system" fn collect(hwnd: HWND, lp: LPARAM) -> BOOL {
        let v = &mut *(lp.0 as *mut Vec<isize>);
        v.push(hwnd.0 as isize);
        BOOL(1)
    }

    pub fn exe_of(pid: u32) -> String {
        unsafe {
            let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
            let mut buf = [0u16; 1024];
            let mut n = buf.len() as u32;
            let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n);
            let _ = CloseHandle(h);
            if r.is_err() {
                return String::new();
            }
            let full = String::from_utf16_lossy(&buf[..n as usize]);
            full.rsplit('\\').next().unwrap_or("").to_string()
        }
    }

    pub fn describe(h: HWND, exes: &mut HashMap<u32, String>) -> RawWindow {
        unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid));
            let exe = exes.entry(pid).or_insert_with(|| exe_of(pid)).clone();
            let mut t = [0u16; 512];
            let n = GetWindowTextW(h, &mut t).max(0) as usize;
            let mut c = [0u16; 256];
            let cn = GetClassNameW(h, &mut c).max(0) as usize;
            let owner = GetWindow(h, GW_OWNER).map(|o| o.0 as u64).unwrap_or(0);
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
            let mut r = RECT::default();
            let _ = GetClientRect(h, &mut r);
            RawWindow {
                hwnd: h.0 as u64,
                pid,
                exe,
                title: String::from_utf16_lossy(&t[..n]),
                class: String::from_utf16_lossy(&c[..cn]),
                owner,
                ex_style: ex,
                visible: IsWindowVisible(h).as_bool(),
                minimized: IsIconic(h).as_bool(),
                w: r.right - r.left,
                h: r.bottom - r.top,
                layer: 0,
            }
        }
    }

    /// Toutes les fenetres de premier niveau. Le nom du processus n'est lu que pour les fenetres visibles et celles
    /// qui possedent une fenetre (assez pour reconnaitre Live), une fois par processus.
    pub fn enumerate() -> Vec<RawWindow> {
        let mut handles: Vec<isize> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(collect), LPARAM(&mut handles as *mut Vec<isize> as isize));
        }
        let mut exes: HashMap<u32, String> = HashMap::new();
        handles.into_iter().map(|h| describe(HWND(h as *mut core::ffi::c_void), &mut exes)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE: &str = "Ableton Live 12 Suite.exe";

    fn win(hwnd: u64, pid: u32, exe: &str, title: &str, owner: u64, tool: bool) -> RawWindow {
        RawWindow {
            hwnd,
            pid,
            exe: exe.into(),
            title: title.into(),
            class: "AbletonVstPlugClass".into(),
            owner,
            ex_style: if tool { WS_EX_TOOLWINDOW } else { 0 },
            visible: true,
            minimized: false,
            w: 800,
            h: 500,
            layer: 0,
        }
    }

    /// (26 oct.) macOS : les fenetres de l'appli « Live », titrees « plugin/piste », pas la fenetre principale.
    #[test]
    fn finds_live_plugin_windows_on_macos() {
        let mac = |id: u64, owner: &str, title: &str, layer: i32, on: bool| RawWindow { hwnd: id, pid: 100, exe: owner.into(), title: title.into(), visible: on, w: 600, h: 400, layer, ..Default::default() };
        let d = vec![
            mac(1, "Live", "My/Set [My/Set] - Ableton Live 12 Suite", 0, true),
            mac(2, "Live", "Diva/2-Diva", 0, true),
            mac(3, "Live", "Serum 2/3-Lead / Pad", 3, true),
            mac(4, "Live", "Pro-Q 4/5-Master", 0, false),
            mac(5, "Google Chrome", "Diva/2-Diva", 0, true),
            mac(6, "Live", "Diva/2-Diva", 25, true), // un menu, une bulle : pas une fenetre de plugin
            mac(7, "Live", "", 0, true),            // sans « Enregistrement de l'ecran » : pas de titre
        ];
        let ids: Vec<u64> = plugin_windows_mac(&d).iter().map(|w| w.id).collect();
        assert_eq!(ids, vec![2, 3]);
        assert!(is_live_app("Live") && is_live_app("Ableton Live 12 Suite") && !is_live_app("Ableton Live 12 Suite.exe") && !is_live_app("Livestream"));
    }

    fn fake_desktop() -> Vec<RawWindow> {
        vec![
            win(1, 100, LIVE, "Set 20 oct [Set 20 oct] - Ableton Live 12 Suite", 0, false), // la fenetre principale
            win(2, 100, LIVE, "Diva/2-Diva", 1, true),
            win(3, 100, LIVE, "Kontakt 8/1-Kontakt 8", 1, true),
            win(4, 100, LIVE, "Serum 2/3-Lead / Pad", 1, true), // une piste avec un « / »
            win(5, 100, LIVE, "Preferences", 1, true),          // pas « plugin/piste »
            win(6, 100, LIVE, "Diva/2-Diva", 1, false),         // pas une fenetre d'outil
            win(7, 200, "chrome.exe", "Diva/2-Diva - Google Chrome", 0, false),
            win(8, 200, "chrome.exe", "Diva/2-Diva", 7, true), // un autre processus qui imite le titre
            win(9, 100, LIVE, "Valhalla/4-Vox", 7, true),      // possedee par une fenetre d'un autre processus
            {
                let mut w = win(10, 100, LIVE, "Pro-Q 4/5-Master", 1, true);
                w.visible = false; // fermee (cachee) par Live
                w
            },
            win(11, 100, LIVE, "Pigments/", 1, true), // titre incomplet
        ]
    }

    #[test]
    fn finds_live_plugin_windows_only() {
        let got = plugin_windows_win(&fake_desktop());
        let ids: Vec<u64> = got.iter().map(|w| w.id).collect();
        assert_eq!(ids, vec![2, 3, 4], "{got:?}");
        assert_eq!(got[0].plugin, "Diva");
        assert_eq!(got[0].track, "2-Diva");
        assert_eq!(got[2].plugin, "Serum 2");
        assert_eq!(got[2].track, "3-Lead / Pad", "split at the first slash");
        assert_eq!((got[0].w, got[0].h), (800, 500));
    }

    #[test]
    fn no_live_no_windows() {
        let only_chrome: Vec<RawWindow> = fake_desktop().into_iter().filter(|w| w.pid == 200).collect();
        assert!(plugin_windows_win(&only_chrome).is_empty());
        assert!(!live_running(&only_chrome));
        assert!(live_running(&fake_desktop()));
    }

    #[test]
    fn live_exe_names() {
        assert!(is_live_exe("Ableton Live 12 Suite.exe"));
        assert!(is_live_exe("C:\\ProgramData\\Ableton\\Live 12 Suite\\Program\\Ableton Live 12 Suite.exe"));
        assert!(is_live_exe("ableton live 11 standard.EXE"));
        assert!(!is_live_exe("Ableton Index.exe"));
        assert!(!is_live_exe("chrome.exe"));
        assert!(!is_live_exe(""));
    }

    #[test]
    fn titles() {
        assert_eq!(split_title("Diva/2-Diva"), Some(("Diva".into(), "2-Diva".into())));
        assert_eq!(split_title("Diva"), None);
        assert_eq!(split_title("/2-Diva"), None);
        assert_eq!(expected_title("Diva", "2-Diva"), "Diva/2-Diva");
    }

    #[test]
    fn order_is_stable() {
        let mut d = fake_desktop();
        d.reverse();
        let ids: Vec<u64> = plugin_windows_win(&d).iter().map(|w| w.id).collect();
        assert_eq!(ids, vec![2, 3, 4]);
    }
}
