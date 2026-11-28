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
    // (relecture F18) le chemin REEL de l'image qui tourne (QueryFullProcessImageNameW suit un renommage ;
    // current_exe garde l'ancien chemin, ou un autre exe a pu etre pose), tenu, puis relu : s'il a change entre les
    // deux, refuse. Une fois tenu (sans FILE_SHARE_DELETE), l'exe ne peut plus etre renomme, ni ses dossiers.
    let Some(exe) = own_image() else { return Outcome::Failed("cannot find this program's own file".into()) };
    let _held = match hold(&exe) {
        Ok(h) => h,
        Err(e) => return Outcome::Failed(format!("cannot hold this program's own file before asking for administrator rights ({e})")),
    };
    if own_image().as_deref() != Some(exe.as_path()) {
        return Outcome::Failed("this program's own file moved while it was being checked: nothing was asked".into());
    }
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

/// (F18) Ouvre `exe` en lecture, en partage de lecture seulement (ni FILE_SHARE_WRITE, ni FILE_SHARE_DELETE) : tant
/// que la poignee vit, l'exe ne peut etre ni ecrit, ni renomme, ni efface, et ses dossiers ne peuvent pas etre renommes.
/// (relecture) Le dossier n'est plus tenu : ca bloquait tout renommage dedans (un telechargement qui se termine dans
/// Telechargements), sans rien ajouter. Ce n'est pas une signature : un exe remplace avant son lancement n'est pas vu.
#[cfg(windows)]
pub fn hold(exe: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 1;
    std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(exe)
}

/// (relecture F18) Le chemin de l'image de ce processus, tel que Windows le suit (apres un renommage aussi).
#[cfg(windows)]
pub fn own_image() -> Option<std::path::PathBuf> {
    use windows::core::PWSTR;
    use windows::Win32::System::Threading::{GetCurrentProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32};
    let mut buf = vec![0u16; 32768];
    let mut n = buf.len() as u32;
    unsafe { QueryFullProcessImageNameW(GetCurrentProcess(), PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n) }.ok()?;
    Some(std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n as usize])))
}

#[cfg(all(test, windows))]
mod f18_tests {
    #[test]
    fn a_held_exe_and_its_folder_cannot_be_swapped() {
        let base = std::env::temp_dir().join(format!("uwl-f18-{}", std::process::id()));
        let dir = base.join("app");
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("uppwind-companion.exe");
        std::fs::write(&exe, b"MZ original").unwrap();
        {
            let _h = super::hold(&exe).unwrap();
            assert!(std::fs::write(&exe, b"MZ other").is_err(), "written while held");
            assert!(std::fs::rename(&exe, dir.join("old.exe")).is_err(), "renamed while held");
            assert!(std::fs::rename(&dir, base.join("moved")).is_err(), "folder moved while held");
            assert_eq!(std::fs::read(&exe).unwrap(), b"MZ original");
            // (relecture) un telechargement qui se termine a cote (Chrome renomme .crdownload) n'est pas bloque
            std::fs::write(dir.join("x.crdownload"), b"x").unwrap();
            std::fs::rename(dir.join("x.crdownload"), dir.join("x.zip")).expect("a rename next to the held exe");
        }
        std::fs::rename(&exe, dir.join("old.exe")).unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }

    /// (relecture) Un exe renomme pendant qu'il tourne : QueryFullProcessImageNameW donne le nouveau nom (current_exe
    /// garderait l'ancien chemin, ou un autre exe y aurait ete pose).
    #[test]
    fn the_image_path_follows_a_rename() {
        use std::os::windows::io::AsRawHandle;
        use windows::core::PWSTR;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Threading::{QueryFullProcessImageNameW, PROCESS_NAME_WIN32};
        let base = std::env::temp_dir().join(format!("uwl-f18r-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let exe = base.join("ping-copy.exe");
        std::fs::copy(r"C:\Windows\System32\PING.EXE", &exe).unwrap();
        let mut child = std::process::Command::new(&exe).args(["-n", "5", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let moved = base.join("moved-aside.exe");
        std::fs::rename(&exe, &moved).unwrap();
        let mut buf = vec![0u16; 1024];
        let mut n = buf.len() as u32;
        unsafe { QueryFullProcessImageNameW(HANDLE(child.as_raw_handle()), PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n) }.unwrap();
        let seen = String::from_utf16_lossy(&buf[..n as usize]).to_lowercase();
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&base);
        assert!(seen.ends_with("moved-aside.exe"), "{seen}");
        assert!(super::own_image().is_some());
    }

    #[test]
    fn the_running_exe_can_be_held_and_still_started() {
        let me = std::env::current_exe().unwrap();
        let _h = super::hold(&me).unwrap();
        let st = std::process::Command::new(&me).arg("--list").output().unwrap();
        assert!(st.status.success(), "the held exe did not start");
    }
}

#[cfg(not(windows))]
pub fn run(_op: &str) -> Outcome {
    Outcome::Failed("administrator rights are only asked for on Windows".into())
}
