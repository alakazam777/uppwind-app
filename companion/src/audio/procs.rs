// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Quel processus le retour audio a le droit d'ecouter (24 oct.) : depuis le studio, seulement un DAW de la liste
//! blanche (Ableton Live*.exe, FL64.exe, FL.exe) ; depuis la console (--capture-test), n'importe lequel.

/// Le DAW d'un nom d'exe (« Ableton Live 12 Suite.exe » → live, « FL64.exe » → fl), sinon None.
pub fn daw_of_exe(exe: &str) -> Option<&'static str> {
    let e = exe.to_ascii_lowercase();
    let name = e.rsplit(['\\', '/']).next().unwrap_or(&e);
    if name.starts_with("ableton live") && name.ends_with(".exe") {
        return Some("live");
    }
    if name == "fl64.exe" || name == "fl.exe" {
        return Some("fl");
    }
    // (28 oct.) macOS : « …/FL Studio 2026.app/Contents/MacOS/OsxFL », « …/Ableton Live 12 Suite.app/Contents/MacOS/Live »
    // (les plugins ne donnent que ce dernier nom, link::host_exe)
    if name == "osxfl" {
        return Some("fl");
    }
    if name == "live" {
        return Some("live");
    }
    None
}

#[derive(Debug, Clone, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub parent: u32,
    pub exe: String,
}

/// (audit de publication) La reponse au studio pour un pid refuse : toujours la meme, que le processus existe ou non et
/// quel que soit son programme — sinon une page appairee listerait les programmes de la machine, pid par pid.
pub const NOT_A_DAW: &str = "not a DAW (Ableton Live or FL Studio)";

/// La cible demandee : « auto » (le DAW qui tourne), « live », « fl », un pid, ou un nom d'exe (ou un bout). `any` :
/// la console (--capture-test) — n'importe quel processus ; sinon seulement un DAW de la liste blanche. `extra` : un
/// pid ajoute a la main (la tonalite d'essai, --test-tone, lancee par le compagnon lui-meme).
pub fn pick(spec: &str, all: &[Proc], any: bool, extra: Option<u32>) -> Result<Proc, String> {
    let s = spec.trim();
    if s.is_empty() {
        return Err("no process named".into());
    }
    let allowed = |p: &Proc| any || daw_of_exe(&p.exe).is_some() || Some(p.pid) == extra;
    if let Ok(pid) = s.parse::<u32>() {
        return match all.iter().find(|p| p.pid == pid) {
            Some(p) if allowed(p) => Ok(p.clone()),
            _ if any => Err(format!("no process {pid}")),
            _ => Err(NOT_A_DAW.into()),
        };
    }
    let low = s.to_ascii_lowercase();
    if low == "test-tone" {
        return match extra.and_then(|x| all.iter().find(|p| p.pid == x)) {
            Some(p) => Ok(p.clone()),
            None => Err("the test tone is not running (start the companion with --test-tone)".into()),
        };
    }
    let want = match low.as_str() {
        "auto" | "daw" => None,
        "live" | "ableton" => Some("live"),
        "fl" | "fl studio" | "flstudio" => Some("fl"),
        _ => {
            // un nom d'exe : le studio n'en donne pas (seule la console le peut, avec n'importe quel processus)
            let hit = all.iter().filter(|p| p.exe.to_ascii_lowercase().contains(&low)).find(|p| allowed(p));
            return hit.cloned().ok_or_else(|| if any { format!("no process matching \"{s}\"") } else { format!("no DAW matching \"{s}\"") });
        }
    };
    // le DAW : FL64 avant FL (FL.exe peut n'etre que le lanceur), et le processus le plus haut de son arbre
    let mut found: Vec<&Proc> = all.iter().filter(|p| match (want, daw_of_exe(&p.exe)) {
        (None, Some(_)) => true,
        (Some(w), Some(d)) => w == d,
        _ => false,
    }).collect();
    found.sort_by_key(|p| (p.exe.to_ascii_lowercase() == "fl.exe", p.pid));
    // un DAW lance par un autre DAW du meme arbre (FL.exe → FL64.exe) : l'arbre du parent le contient deja
    let top = found.iter().find(|p| !found.iter().any(|q| q.pid == p.parent)).or(found.first());
    match top {
        Some(p) => Ok((*p).clone()),
        None => Err(match want {
            Some("live") => "Ableton Live is not running".into(),
            Some(_) => "FL Studio is not running".into(),
            None => "no DAW is running (Ableton Live or FL Studio)".into(),
        }),
    }
}

#[cfg(windows)]
pub fn list() -> Vec<Proc> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let n = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
                out.push(Proc { pid: e.th32ProcessID, parent: e.th32ParentProcessID, exe: String::from_utf16_lossy(&e.szExeFile[..n]) });
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

#[cfg(not(windows))]
pub fn list() -> Vec<Proc> {
    Vec::new()
}

/// Le processus tourne-t-il encore ?
pub fn alive(pid: u32) -> bool {
    list().iter().any(|p| p.pid == pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, parent: u32, exe: &str) -> Proc {
        Proc { pid, parent, exe: exe.into() }
    }

    fn desk() -> Vec<Proc> {
        vec![p(4, 0, "System"), p(100, 4, "explorer.exe"), p(200, 100, "chrome.exe"), p(300, 100, "Ableton Live 12 Suite.exe"), p(400, 100, "Discord.exe"), p(500, 100, "FL64.exe")]
    }

    #[test]
    fn daw_names() {
        assert_eq!(daw_of_exe("Ableton Live 12 Suite.exe"), Some("live"));
        assert_eq!(daw_of_exe("C:\\ProgramData\\Ableton\\Live 11 Lite\\Program\\Ableton Live 11 Lite.exe"), Some("live"));
        assert_eq!(daw_of_exe("FL64.exe"), Some("fl"));
        assert_eq!(daw_of_exe("fl.EXE"), Some("fl"));
        assert_eq!(daw_of_exe("FL Cloud Plugins.exe"), None);
        assert_eq!(daw_of_exe("Ableton Index.exe"), None);
        assert_eq!(daw_of_exe("chrome.exe"), None);
        assert_eq!(daw_of_exe("ilbridge.exe"), None);
        assert_eq!(daw_of_exe("/Applications/FL Studio 2026.app/Contents/MacOS/OsxFL"), Some("fl"));
        assert_eq!(daw_of_exe("OsxFL"), Some("fl"));
        assert_eq!(daw_of_exe("/Applications/Ableton Live 12 Suite.app/Contents/MacOS/Live"), Some("live"));
        assert_eq!(daw_of_exe("Live"), Some("live"));
        assert_eq!(daw_of_exe("Live Helper"), None);
    }

    #[test]
    fn the_studio_only_gets_a_daw() {
        let d = desk();
        assert_eq!(pick("auto", &d, false, None).unwrap().pid, 300, "Live first when both run (lowest pid)");
        assert_eq!(pick("live", &d, false, None).unwrap().pid, 300);
        assert_eq!(pick("fl", &d, false, None).unwrap().pid, 500);
        assert_eq!(pick("300", &d, false, None).unwrap().exe, "Ableton Live 12 Suite.exe");
        // jamais un autre processus depuis le studio
        assert!(pick("200", &d, false, None).unwrap_err().contains("not a DAW"));
        // (audit) ni le nom du programme, ni la difference entre un pid refuse et un pid qui n'existe pas
        assert_eq!(pick("400", &d, false, None).unwrap_err(), NOT_A_DAW);
        assert_eq!(pick("999", &d, false, None).unwrap_err(), NOT_A_DAW);
        assert!(!pick("400", &d, false, None).unwrap_err().to_lowercase().contains("discord"));
        assert_eq!(pick("999", &d, true, None).unwrap_err(), "no process 999", "the console says it");
        assert!(pick("discord", &d, false, None).is_err());
        assert!(pick("chrome.exe", &d, false, None).is_err());
        assert!(pick("", &d, false, None).is_err());
        assert!(pick("999", &d, false, None).is_err());
        // la console, elle, peut tout ecouter
        assert_eq!(pick("discord", &d, true, None).unwrap().pid, 400);
        assert_eq!(pick("200", &d, true, None).unwrap().pid, 200);
    }

    #[test]
    fn missing_daws_say_so() {
        let d = vec![p(4, 0, "System"), p(200, 4, "chrome.exe")];
        assert_eq!(pick("live", &d, false, None).unwrap_err(), "Ableton Live is not running");
        assert_eq!(pick("fl", &d, false, None).unwrap_err(), "FL Studio is not running");
        assert!(pick("auto", &d, false, None).unwrap_err().contains("no DAW"));
    }

    #[test]
    fn fl_launcher_and_child() {
        // FL.exe (le lanceur) → FL64.exe : l'arbre du lanceur contient deja FL64
        let d = vec![p(100, 4, "explorer.exe"), p(600, 100, "FL.exe"), p(610, 600, "FL64.exe")];
        assert_eq!(pick("fl", &d, false, None).unwrap().pid, 600);
        // FL64 seul
        let d = vec![p(100, 4, "explorer.exe"), p(610, 100, "FL64.exe"), p(700, 100, "FL.exe")];
        assert_eq!(pick("fl", &d, false, None).unwrap().pid, 610, "FL64 before an unrelated FL.exe");
    }

    #[test]
    fn the_test_tone_is_allowed_only_when_the_companion_started_it() {
        let mut d = desk();
        d.push(p(800, 1, "uppwind-companion.exe"));
        assert_eq!(pick("test-tone", &d, false, Some(800)).unwrap().pid, 800);
        assert_eq!(pick("800", &d, false, Some(800)).unwrap().pid, 800);
        assert!(pick("test-tone", &d, false, None).is_err());
        assert!(pick("800", &d, false, None).is_err());
    }
}
