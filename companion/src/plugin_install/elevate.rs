// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (29 oct.) Windows : ecrire dans C:\Program Files\Common Files\VST3 demande les droits d'administrateur. Le compagnon
//! se relance lui-meme, eleve, pour la copie et pour elle seule :
//!
//!   - ShellExecuteExW, verbe « runas » : UNE invite de Windows (UAC). Refusee : ERROR_CANCELLED, rien ne change.
//!   - la copie elevee recoit `--plugins-elevated install|uninstall`, rien d'autre (tout autre argument : refuse). Elle
//!     ne lit ni chemin ni variable d'environnement (un processus eleve recoit celles du profil, pas les notres ; avec
//!     un compte administrateur a part, ce n'est meme pas le meme profil) : le dossier vient du dossier connu de
//!     Windows, les octets de l'exe lui-meme. Ni serveur, ni fenetre, ni journal, ni COM ; sa console est cachee.
//!   - le compagnon attend sa fin (2 minutes au plus) et lit son code de sortie (OK, HELD : un DAW tenait l'ancienne DLL,
//!     FAILED, USAGE, EMPTY : cet exe ne porte pas de plugins), puis relit lui-meme le dossier.
//!
//! main.rs appelle `main` avant tout le reste (juste apres SetDefaultDllDirectories : les DLL de System32 seulement,
//! les importees le sont deja par /DEPENDENTLOADFLAG). Les essais n'elevent jamais (`run` refuse sous cfg(test)).

pub const FLAG: &str = "--plugins-elevated";
pub const OK: i32 = 0;
pub const FAILED: i32 = 1;
pub const USAGE: i32 = 2;
/// fait, mais un DAW tient encore l'ancienne DLL (mise de cote, effacee au prochain demarrage de Windows)
pub const HELD: i32 = 3;
/// cet exe ne porte pas de plugins
pub const EMPTY: i32 = 4;

/// La copie elevee : exactement `--plugins-elevated install|uninstall`.
pub fn main(raw: &[String]) -> i32 {
    let [flag, op] = raw else { return USAGE };
    if flag != FLAG || !matches!(op.as_str(), "install" | "uninstall") {
        return USAGE;
    }
    #[cfg(windows)]
    {
        super::elevated_job(op)
    }
    #[cfg(not(windows))]
    {
        USAGE
    }
}

/// Ce qu'a donne la demande.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// la copie elevee a tourne : son code de sortie
    Exit(i32),
    /// l'invite de Windows a ete refusee
    Refused,
    Failed(String),
}

/// Relance cet exe eleve pour `op` (« install », « uninstall ») et attend sa fin.
#[cfg(windows)]
pub fn run(op: &str) -> Outcome {
    if cfg!(test) {
        return Outcome::Failed("the tests never ask for administrator rights".into());
    }
    if !matches!(op, "install" | "uninstall") {
        return Outcome::Failed(format!("unknown operation {op}"));
    }
    use windows::core::{w, HRESULT, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HWND, WAIT_OBJECT_0};
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, SW_HIDE};
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return Outcome::Failed(format!("cannot find this program's own file ({e})")),
    };
    let file = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(format!("{FLAG} {op}"));
    unsafe {
        // l'invite au premier plan : la fenetre d'installation, si c'est elle qui est devant
        let fg = GetForegroundWindow();
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        let hwnd = if pid == std::process::id() { fg } else { HWND::default() };
        // (ShellExecuteEx veut COM ; un fil deja en MTA le garde, ce qui suffit pour lancer un exe)
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            hwnd,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(params.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        if let Err(e) = ShellExecuteExW(&mut info) {
            if e.code() == HRESULT::from_win32(ERROR_CANCELLED.0) {
                return Outcome::Refused;
            }
            return Outcome::Failed(format!("cannot start the copy with administrator rights ({e})"));
        }
        let h = info.hProcess;
        if h.is_invalid() {
            return Outcome::Failed("Windows started the copy with administrator rights but gave no process to wait for".into());
        }
        let waited = WaitForSingleObject(h, 120_000);
        let mut code = 0u32;
        let got = GetExitCodeProcess(h, &mut code);
        let _ = CloseHandle(h);
        if waited != WAIT_OBJECT_0 {
            return Outcome::Failed("the copy with administrator rights did not finish within 2 minutes".into());
        }
        match got {
            Ok(()) => Outcome::Exit(code as i32),
            Err(e) => Outcome::Failed(format!("cannot read the result of the copy with administrator rights ({e})")),
        }
    }
}

#[cfg(not(windows))]
pub fn run(_op: &str) -> Outcome {
    Outcome::Failed("administrator rights are only asked for on Windows".into())
}
