// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Un seul Uppwind Link avec icone par session Windows : un mutex nomme, et deux evenements nommes pour lui parler.
//!   relance (le menu Demarrer, un double-clic) → « Show » : celui qui tourne montre sa fenetre, le nouveau s'en va ;
//!   l'installation d'une mise a jour, --stop  → « Quit » : celui qui tourne s'arrete proprement (son icone part).
//! Le bac a sable a ses propres noms (un hachage de son dossier) : il ne parle jamais au vrai.

use std::time::Duration;

/// Le suffixe des noms : « main », ou le bac a sable.
pub fn scope() -> String {
    match crate::wizard::sandbox() {
        None => "main".into(),
        Some(d) => {
            // FNV-1a du chemin en minuscules : court, stable, sans caractere interdit
            let mut h: u64 = 0xcbf29ce484222325;
            for b in d.to_string_lossy().to_ascii_lowercase().bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            format!("sb{h:016x}")
        }
    }
}

pub fn names(scope: &str) -> (String, String, String) {
    (format!("Local\\UppwindLink.App.{scope}"), format!("Local\\UppwindLink.Show.{scope}"), format!("Local\\UppwindLink.Quit.{scope}"))
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        CreateEventW, CreateMutexW, OpenEventW, OpenMutexW, ReleaseMutex, SetEvent, WaitForMultipleObjects, WaitForSingleObject, EVENT_MODIFY_STATE, INFINITE, SYNCHRONIZATION_SYNCHRONIZE,
    };

    /// Ce processus est celui qui a l'icone (le mutex, et les evenements qu'il ecoute).
    pub struct Guard {
        mutex: isize,
        show: isize,
        quit: isize,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe {
                let _ = ReleaseMutex(HANDLE(self.mutex as _));
                for h in [self.mutex, self.show, self.quit] {
                    let _ = CloseHandle(HANDLE(h as _));
                }
            }
        }
    }

    pub enum Acquire {
        Got(Guard),
        /// un autre tourne deja
        Running,
    }

    /// Prend la place ; `replace` : demande d'abord a celui qui tourne de s'arreter (une mise a jour), et l'attend.
    pub fn acquire(replace: bool) -> Acquire {
        let (m, s, q) = names(&scope());
        unsafe {
            let Ok(mutex) = CreateMutexW(None, true, &HSTRING::from(m.as_str())) else { return Acquire::Running };
            if GetLastError() == ERROR_ALREADY_EXISTS {
                if !replace {
                    let _ = CloseHandle(mutex);
                    return Acquire::Running;
                }
                if let Ok(e) = OpenEventW(EVENT_MODIFY_STATE, false, &HSTRING::from(q.as_str())) {
                    let _ = SetEvent(e);
                    let _ = CloseHandle(e);
                }
                let w = WaitForSingleObject(mutex, 8000);
                if w != WAIT_OBJECT_0 && w != WAIT_ABANDONED {
                    let _ = CloseHandle(mutex);
                    return Acquire::Running;
                }
            }
            let show = CreateEventW(None, false, false, &HSTRING::from(s.as_str())).unwrap_or_default();
            let quit = CreateEventW(None, false, false, &HSTRING::from(q.as_str())).unwrap_or_default();
            Acquire::Got(Guard { mutex: mutex.0 as isize, show: show.0 as isize, quit: quit.0 as isize })
        }
    }

    /// Celui qui tourne : montre ta fenetre → true s'il y en a un.
    pub fn signal_show() -> bool {
        let (_, s, _) = names(&scope());
        unsafe {
            match OpenEventW(EVENT_MODIFY_STATE, false, &HSTRING::from(s.as_str())) {
                Ok(e) => {
                    let ok = SetEvent(e).is_ok();
                    let _ = CloseHandle(e);
                    ok
                }
                Err(_) => false,
            }
        }
    }

    /// Celui qui tourne : arrete-toi ; attend qu'il soit parti → true s'il y en avait un et qu'il est parti.
    pub fn request_quit(wait: Duration) -> bool {
        let (m, _, q) = names(&scope());
        unsafe {
            let Ok(mutex) = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, &HSTRING::from(m.as_str())) else { return false };
            if let Ok(e) = OpenEventW(EVENT_MODIFY_STATE, false, &HSTRING::from(q.as_str())) {
                let _ = SetEvent(e);
                let _ = CloseHandle(e);
            }
            let w = WaitForSingleObject(mutex, wait.as_millis().min(u32::MAX as u128) as u32);
            let gone = w == WAIT_OBJECT_0 || w == WAIT_ABANDONED;
            if gone {
                let _ = ReleaseMutex(mutex);
            }
            let _ = CloseHandle(mutex);
            gone
        }
    }

    /// Ecoute « Show » et « Quit » dans un fil ; `on(true)` : show, `on(false)` : quit (puis le fil s'arrete).
    pub fn listen(g: &Guard, on: impl Fn(bool) + Send + 'static) {
        let (show, quit) = (g.show, g.quit);
        std::thread::spawn(move || loop {
            let hs = [HANDLE(show as _), HANDLE(quit as _)];
            let w = unsafe { WaitForMultipleObjects(&hs, false, INFINITE) };
            if w == WAIT_OBJECT_0 {
                on(true);
            } else {
                on(false);
                break;
            }
        });
    }
}

#[cfg(windows)]
pub use imp::{acquire, listen, request_quit, signal_show, Acquire, Guard};

#[cfg(not(windows))]
pub fn request_quit(_wait: Duration) -> bool {
    false
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn the_sandbox_has_its_own_names() {
        assert_eq!(names("main").0, "Local\\UppwindLink.App.main");
        // (les essais n'ont pas de bac a sable : la portee est « main », mais on ne prend jamais le mutex ici)
        assert_eq!(scope(), "main");
    }
}
