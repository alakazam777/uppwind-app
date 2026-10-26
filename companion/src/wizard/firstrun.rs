// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Le premier lancement : que faire d'un exe lance sans option ?
//!
//!   - depuis un terminal (Git Bash, cmd, PowerShell, un script) : la console du compagnon, comme avant ;
//!   - d'un double-clic (l'Explorateur, le navigateur qui vient de le telecharger, le menu Demarrer) :
//!       la copie installee elle-meme      → l'appli (l'icone de la zone de notification et la fenetre d'etat) ;
//!       pas installe, ou une version plus ancienne installee → la fenetre d'installation ;
//!       la meme version (ou plus recente) installee → on passe la main a la copie installee et on s'en va.
//!
//! « Un double-clic » : la console de ce processus est a lui seul (Windows l'a creee pour lui), sa sortie est cette
//! console, et son parent n'est pas un interprete de commandes. La version installee : l'entree « Applications
//! installees » (arp.rs), sinon le bloc de version de l'exe installe (build/winres.rs), sinon inconnue (une copie
//! d'avant l'assistant : traitee comme plus ancienne).

use crate::setup::cmp_versions;
use std::cmp::Ordering;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// la console du compagnon (un terminal, ou une option explicite)
    Console,
    /// la copie installee, d'un double-clic : l'icone et la fenetre d'etat
    App,
    /// la fenetre d'installation ; `installed` : la version deja la (une mise a jour), « » si inconnue
    Wizard { installed: Option<String> },
    /// la meme version, ou plus recente, est deja installee : la lancer, elle montre sa fenetre
    OpenInstalled,
}

/// Ce que l'on sait du lancement (rempli par `observe`, ou a la main dans les essais).
#[derive(Debug, Clone, Default)]
pub struct Seen {
    /// aucune option
    pub no_args: bool,
    /// lance d'un double-clic (voir `observe`)
    pub double_click: bool,
    /// cet exe EST la copie installee
    pub from_install: bool,
    /// la copie installee existe
    pub installed: bool,
    /// sa version, si on la connait
    pub installed_version: Option<String>,
}

/// La decision (pure).
pub fn decide(s: &Seen, this_version: &str) -> Launch {
    if !s.no_args || !s.double_click {
        return Launch::Console;
    }
    if s.from_install {
        return Launch::App;
    }
    if !s.installed {
        return Launch::Wizard { installed: None };
    }
    match s.installed_version.as_deref() {
        Some(v) if cmp_versions(v, this_version) != Ordering::Less => Launch::OpenInstalled,
        Some(v) => Launch::Wizard { installed: Some(v.to_string()) },
        None => Launch::Wizard { installed: Some(String::new()) },
    }
}

/// Les interpretes de commandes et terminaux : lance par l'un d'eux, l'exe garde sa console.
pub fn is_shell(exe: &str) -> bool {
    const SHELLS: &[&str] = &[
        "bash.exe", "sh.exe", "zsh.exe", "fish.exe", "dash.exe", "mintty.exe", "cmd.exe", "powershell.exe", "pwsh.exe", "windowsterminal.exe", "wt.exe",
        "openconsole.exe", "conhost.exe", "node.exe", "python.exe", "pythonw.exe", "py.exe", "cargo.exe", "rustc.exe", "far.exe", "conemu64.exe", "conemuc64.exe",
        "tcc.exe", "nu.exe", "wsl.exe", "winpty-agent.exe", "code.exe", "devenv.exe",
    ];
    let n = exe.rsplit(['\\', '/']).next().unwrap_or("").to_ascii_lowercase();
    SHELLS.contains(&n.as_str())
}

/// Deux chemins designent-ils le meme fichier (existant) ?
pub fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// La console est-elle a ce processus seul, et sa sortie cette console ? (un double-clic, le menu Demarrer, un
/// navigateur ; pas un terminal, qui partage la sienne, ni un programme qui lit la sortie par un tube)
#[cfg(windows)]
pub fn console_is_ours() -> bool {
    use std::io::IsTerminal;
    use windows::Win32::System::Console::GetConsoleProcessList;
    let mut pids = [0u32; 4];
    let n = unsafe { GetConsoleProcessList(&mut pids) };
    n == 1 && std::io::stdout().is_terminal()
}
#[cfg(not(windows))]
pub fn console_is_ours() -> bool {
    false
}

/// Le nom de l'exe du processus parent (Toolhelp), s'il tourne encore.
#[cfg(windows)]
pub fn parent_exe() -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    let me = std::process::id();
    let mut all: Vec<(u32, u32, String)> = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let end = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
                all.push((e.th32ProcessID, e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..end])));
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    let parent = all.iter().find(|p| p.0 == me)?.1;
    all.into_iter().find(|p| p.0 == parent).map(|p| p.2)
}
#[cfg(not(windows))]
pub fn parent_exe() -> Option<String> {
    None
}

/// Un double-clic (voir `console_is_ours`), et le parent n'est pas un terminal.
pub fn double_clicked() -> bool {
    console_is_ours() && !parent_exe().is_some_and(|p| is_shell(&p))
}

/// La version d'un exe, lue dans son bloc de version (« 0.1.0 ») ; None : pas de bloc (une copie d'avant l'assistant).
#[cfg(windows)]
pub fn file_version(exe: &Path) -> Option<String> {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO};
    let name = HSTRING::from(exe.as_os_str());
    unsafe {
        let size = GetFileVersionInfoSizeW(&name, None);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        GetFileVersionInfoW(&name, None, size, buf.as_mut_ptr() as *mut _).ok()?;
        let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        if !VerQueryValueW(buf.as_ptr() as *const _, &HSTRING::from("\\"), &mut p, &mut len).as_bool() || p.is_null() || (len as usize) < std::mem::size_of::<VS_FIXEDFILEINFO>() {
            return None;
        }
        let f = &*(p as *const VS_FIXEDFILEINFO);
        if f.dwSignature != 0xFEEF04BD {
            return None;
        }
        Some(format!("{}.{}.{}", f.dwFileVersionMS >> 16, f.dwFileVersionMS & 0xFFFF, f.dwFileVersionLS >> 16))
    }
}
#[cfg(not(windows))]
pub fn file_version(_exe: &Path) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(no_args: bool, double_click: bool, from_install: bool, installed: bool, v: Option<&str>) -> Seen {
        Seen { no_args, double_click, from_install, installed, installed_version: v.map(String::from) }
    }

    #[test]
    fn a_terminal_or_an_option_keeps_the_console() {
        assert_eq!(decide(&seen(false, true, false, false, None), "0.2.0"), Launch::Console, "an option");
        assert_eq!(decide(&seen(true, false, false, false, None), "0.2.0"), Launch::Console, "a terminal");
        assert_eq!(decide(&seen(true, false, true, true, Some("0.2.0")), "0.2.0"), Launch::Console, "the installed copy from a terminal");
    }

    #[test]
    fn a_double_click_installs_updates_or_opens() {
        assert_eq!(decide(&seen(true, true, false, false, None), "0.2.0"), Launch::Wizard { installed: None }, "not installed");
        assert_eq!(decide(&seen(true, true, false, true, Some("0.1.0")), "0.2.0"), Launch::Wizard { installed: Some("0.1.0".into()) }, "older");
        assert_eq!(decide(&seen(true, true, false, true, None), "0.2.0"), Launch::Wizard { installed: Some(String::new()) }, "unknown: a copy from before the setup window");
        assert_eq!(decide(&seen(true, true, false, true, Some("0.2.0")), "0.2.0"), Launch::OpenInstalled, "the same");
        assert_eq!(decide(&seen(true, true, false, true, Some("0.10.0")), "0.9.0"), Launch::OpenInstalled, "newer (numbers, not text)");
        assert_eq!(decide(&seen(true, true, true, true, Some("0.2.0")), "0.2.0"), Launch::App, "the installed copy itself");
    }

    #[test]
    fn shells_are_known_by_their_exe() {
        for s in ["bash.exe", r"C:\Program Files\Git\usr\bin\bash.exe", "CMD.EXE", "pwsh.exe", "WindowsTerminal.exe", "node.exe"] {
            assert!(is_shell(s), "{s}");
        }
        for s in ["explorer.exe", "chrome.exe", "msedge.exe", "firefox.exe", "SystemSettings.exe", ""] {
            assert!(!is_shell(s), "{s}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn the_version_block_of_an_exe_is_read() {
        let ping = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32").join("PING.EXE");
        let v = file_version(&ping).expect("Windows' ping.exe has a version block");
        assert!(v.starts_with("10.") || v.starts_with("6."), "{v}");
        let t = std::env::temp_dir().join(format!("uwc-nover-{}.exe", std::process::id()));
        std::fs::write(&t, b"MZ not really").unwrap();
        assert_eq!(file_version(&t), None, "no version block");
        let _ = std::fs::remove_file(&t);
    }

    #[cfg(windows)]
    #[test]
    fn a_test_run_is_not_a_double_click() {
        // cargo test lit la sortie par un tube : jamais la fenetre d'installation pendant les essais
        assert!(!double_clicked());
    }
}
