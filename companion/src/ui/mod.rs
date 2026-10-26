// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Les fenetres du compagnon : une fenetre WebView2 (wry) dans une fenetre tao, la page tiree de l'exe.
//!
//! Securite (la page n'a aucun pouvoir propre ; tout passe par ipc.rs, verifie) :
//!   - aucune page distante : le HTML est une chaine de l'exe (web/), chargee par NavigateToString (origine « null ») ;
//!   - CSP : default-src 'none', le script et le style seulement avec le nonce de ce lancement, les images en data: ;
//!   - aucune navigation (seulement le chargement de la page elle-meme, une fois : data:text/html), aucune nouvelle
//!     fenetre, aucun telechargement, aucune permission (micro, camera…), aucun fichier depose ;
//!   - pas d'outils de developpement dans l'exe publie, pas de menu contextuel, pas de raccourcis du navigateur ;
//!   - les messages de la page arrivent par l'IPC de wry, dans le processus : aucun port, aucun point d'entree HTTP.
//! Les donnees de WebView2 : un dossier temporaire efface a la fin (installation, desinstallation), ou
//! %LOCALAPPDATA%\Uppwind Link\webview (la fenetre d'etat ; la desinstallation l'efface).

pub mod ipc;
pub mod pages;
pub mod preview;

use serde_json::Value;
use std::path::{Path, PathBuf};

const CSS: &str = include_str!("web/app.css");
const JS: &str = include_str!("web/app.js");
const TEMPLATE: &str = include_str!("web/page.html");

mod assets {
    include!(concat!(env!("OUT_DIR"), "/ui_assets.rs"));
}

/// Un nonce neuf pour la CSP de la page.
pub fn nonce() -> String {
    let mut b = [0u8; 18];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// La page : le gabarit, le style, le script, le disque ; `preview` : un script de plus avant celui de la page (les
/// apercus de --ui-preview, qui imitent le compagnon).
pub fn html(page: ipc::Page, nonce: &str, preview: Option<&str>) -> String {
    let mark = format!("data:image/png;base64,{}", assets::MARK_PNG_BASE64);
    let pre = preview.map(|p| format!("<script nonce=\"{nonce}\">{p}</script>\n")).unwrap_or_default();
    // d'abord les petites valeurs du gabarit, puis le style et le script tels quels
    let t = TEMPLATE.replace("__NONCE__", nonce).replace("__PAGE__", page.name()).replace("__MARK__", &mark).replace("__PREVIEW__", &pre);
    let (a, rest) = t.split_once("__CSS__").expect("page.html has __CSS__");
    let (b, c) = rest.split_once("__JS__").expect("page.html has __JS__");
    let mut out = String::with_capacity(t.len() + CSS.len() + JS.len());
    out.push_str(a);
    out.push_str(CSS);
    out.push_str(b);
    out.push_str(JS);
    out.push_str(c);
    out
}

/// Le message que la page recoit (window.__uwl.recv) : du JSON, qui est du JavaScript valide.
pub fn script_for(v: &Value) -> String {
    format!("window.__uwl&&window.__uwl.recv({v})")
}

/// La version de WebView2 installee ; None : absente (la console prend le relais).
pub fn webview2_version() -> Option<String> {
    #[cfg(windows)]
    {
        wry::webview_version().ok().filter(|v| !v.is_empty())
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// La console de ce processus, s'il est seul a l'avoir (un double-clic) : cachee et lachee. Jamais celle d'un terminal.
pub fn hide_console() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Console::{FreeConsole, GetConsoleProcessList, GetConsoleWindow};
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
        let mut pids = [0u32; 4];
        if GetConsoleProcessList(&mut pids) == 1 {
            let h = GetConsoleWindow();
            if !h.is_invalid() {
                let _ = ShowWindow(h, SW_HIDE);
            }
            let _ = FreeConsole();
        }
    }
}

/// Ouvre une adresse de la liste fermee (ipc::Link) dans le navigateur par defaut.
pub fn open_url(url: &str) {
    #[cfg(windows)]
    unsafe {
        use windows::core::HSTRING;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let _ = ShellExecuteW(None, &HSTRING::from("open"), &HSTRING::from(url), None, None, SW_SHOWNORMAL);
    }
    #[cfg(not(windows))]
    let _ = url;
}

/// Met ce texte dans le presse-papiers (CF_UNICODETEXT).
pub fn copy_text(text: &str) -> Result<(), String> {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
        use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
        use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
        use windows::Win32::System::Ole::CF_UNICODETEXT;
        let w: Vec<u16> = text.encode_utf16().chain([0]).collect();
        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(None).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        if !opened {
            return Err("the clipboard is busy".into());
        }
        let r = (|| {
            EmptyClipboard().map_err(|e| e.to_string())?;
            let mem: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2).map_err(|e| e.to_string())?;
            let p = GlobalLock(mem) as *mut u16;
            if p.is_null() {
                let _ = GlobalFree(Some(mem));
                return Err("cannot lock memory".to_string());
            }
            std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
            let _ = GlobalUnlock(mem);
            // le presse-papiers garde la memoire si l'appel reussit
            if let Err(e) = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(mem.0))) {
                let _ = GlobalFree(Some(mem));
                return Err(e.to_string());
            }
            // (26 oct.) le code d'appairage n'entre ni dans l'historique du presse-papiers (Win+V) ni dans sa
            // synchronisation entre appareils : les formats que Windows lit pour cela, a 0
            for name in ["CanIncludeInClipboardHistory", "CanUploadToCloudClipboard"] {
                let f = windows::Win32::System::DataExchange::RegisterClipboardFormatW(&windows::core::HSTRING::from(name));
                if f == 0 {
                    continue;
                }
                let Ok(m) = GlobalAlloc(GMEM_MOVEABLE, 4) else { continue };
                let q = GlobalLock(m) as *mut u32;
                if q.is_null() {
                    let _ = GlobalFree(Some(m));
                    continue;
                }
                *q = 0;
                let _ = GlobalUnlock(m);
                if SetClipboardData(f, Some(HANDLE(m.0))).is_err() {
                    let _ = GlobalFree(Some(m));
                }
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        r
    }
    #[cfg(not(windows))]
    {
        let _ = text;
        Err("no clipboard".into())
    }
}

/// --smoke-test … --shot <fichier.jpg> : une image de la vraie fenetre (son cadre compris), une seconde apres qu'elle
/// s'est dessinee, par la capture du compagnon (capture.rs), dans un fil a part.
pub fn shot_later(hwnd: u64, path: PathBuf) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(1000));
        let r = (|| -> Result<(), String> {
            let mut cap = crate::capture::open(hwnd, None)?;
            for _ in 0..60 {
                if let Some(f) = cap.grab()? {
                    let j = crate::encode::jpeg(&f, 92)?;
                    return std::fs::write(&path, j).map_err(|e| format!("cannot write {} ({e})", path.display()));
                }
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            Err("no image from the capture".into())
        })();
        match r {
            Ok(()) => println!("smoke test: picture of the window in {}", path.display()),
            Err(e) => eprintln!("smoke test: no picture ({e})"),
        }
    });
}

/// L'image demandee par --shot (l'essai seulement).
pub fn shot_path() -> Option<PathBuf> {
    let raw: Vec<String> = std::env::args().collect();
    raw.iter().position(|a| a == "--shot").and_then(|i| raw.get(i + 1)).map(PathBuf::from)
}

/// Le dossier des donnees de WebView2 d'une fenetre d'installation ou de desinstallation : dans le bac a sable, sinon
/// un dossier temporaire a ce processus (efface a la fin).
pub fn setup_data_dir() -> PathBuf {
    match crate::wizard::sandbox() {
        Some(d) => d.join("setup-webview"),
        None => std::env::temp_dir().join("Uppwind Link Setup").join(format!("wv-{}", std::process::id())),
    }
}

/// Efface un dossier de donnees de WebView2 (ses processus le lachent une seconde apres la fermeture) ; s'il est encore
/// tenu, apres la sortie.
pub fn remove_data_dir(dir: &Path) {
    for _ in 0..30 {
        if !dir.exists() || std::fs::remove_dir_all(dir).is_ok() {
            if let Some(parent) = dir.parent().filter(|p| p.ends_with("Uppwind Link Setup")) {
                let _ = std::fs::remove_dir(parent);
            }
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    crate::wizard::cleanup_after_exit(&[], &[], &[dir.to_path_buf()]);
}

#[cfg(windows)]
pub use win::{apply as apply_out, event_loop, run_page, smoke_test, UiWindow, UserEvent};

/// Une boite de message (la fenetre d'etat sans WebView2).
pub fn message_box(title: &str, text: &str) {
    #[cfg(windows)]
    unsafe {
        use windows::core::HSTRING;
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};
        let _ = MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(title), MB_OK | MB_ICONINFORMATION);
    }
    #[cfg(not(windows))]
    let _ = (title, text);
}

#[cfg(windows)]
mod win {
    use super::ipc::{self, Request};
    use super::pages::{run_job, Out, View};
    use super::*;
    use std::time::{Duration, Instant};
    use tao::dpi::{LogicalSize, PhysicalPosition};
    use tao::event::{Event, StartCause, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
    use tao::platform::run_return::EventLoopExtRunReturn;
    use tao::platform::windows::{EventLoopBuilderExtWindows, IconExtWindows};

    /// La boucle des fenetres, sur un fil a elle (pas le fil principal) : tao y entre dans l'appartement STA que
    /// WebView2 demande, et le reste du compagnon garde son MTA (capture::keep_com_alive, les connexions, --setup).
    pub fn event_loop() -> EventLoop<UserEvent> {
        let mut b = EventLoopBuilder::<UserEvent>::with_user_event();
        b.with_any_thread(true);
        b.build()
    }
    use tao::window::{Icon, Theme, Window, WindowBuilder};
    use wry::{NewWindowResponse, PermissionResponse, WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows};

    /// Ce qui reveille la boucle des fenetres.
    pub enum UserEvent {
        /// un message d'une page
        Ipc(String),
        /// une ligne de progression d'un travail en cours
        Progress(String),
        /// un travail fini
        Done(super::pages::Done),
        /// l'icone : montrer la fenetre d'etat (un 2e lancement, un clic)
        Show,
        /// l'icone : quitter (le menu, une mise a jour qui remplace ce compagnon)
        Quit,
        Menu(tray_icon::menu::MenuId),
        Tray(tray_icon::TrayIconEvent),
    }

    /// Une fenetre et sa page. L'ordre des champs est l'ordre de destruction : la page, ses donnees, puis la fenetre.
    pub struct UiWindow {
        pub webview: WebView,
        _ctx: WebContext,
        pub window: Window,
        pub page: ipc::Page,
    }

    impl UiWindow {
        pub fn open(target: &EventLoopWindowTarget<UserEvent>, view: &dyn View, proxy: &EventLoopProxy<UserEvent>, data_dir: &Path) -> Result<UiWindow, String> {
            let (w, h) = view.size();
            let mut wb = WindowBuilder::new()
                .with_title(view.title())
                .with_inner_size(LogicalSize::new(w, h))
                .with_min_inner_size(LogicalSize::new(520.0, 420.0))
                .with_theme(Some(Theme::Dark))
                .with_visible(false);
            if let Ok(icon) = Icon::from_resource(1, None) {
                wb = wb.with_window_icon(Some(icon));
            }
            let window = wb.build(target).map_err(|e| format!("cannot open a window ({e})"))?;
            if let Some(m) = window.current_monitor() {
                let (ms, mp, ws) = (m.size(), m.position(), window.outer_size());
                window.set_outer_position(PhysicalPosition::new(mp.x + (ms.width as i32 - ws.width as i32) / 2, mp.y + (ms.height as i32 - ws.height as i32) / 2));
            }
            let _ = std::fs::create_dir_all(data_dir);
            let mut ctx = WebContext::new(Some(data_dir.to_path_buf()));
            let page = view.page();
            let p2 = proxy.clone();
            let trace = std::env::var_os("UWC_UI_TRACE").is_some();
            let first = std::cell::Cell::new(true);
            let webview = WebViewBuilder::new_with_web_context(&mut ctx)
                .with_html(html(page, &nonce(), None))
                .with_ipc_handler(move |req: wry::http::Request<String>| {
                    let _ = p2.send_event(UserEvent::Ipc(req.into_body()));
                })
                .with_navigation_handler(move |url: String| {
                    // seulement la page elle-meme : NavigateToString la charge comme une adresse data:text/html, une
                    // fois, avant tout script ; toute autre navigation est refusee
                    let ok = (first.replace(false) && url.starts_with("data:text/html")) || url == "about:blank";
                    if trace || !ok {
                        crate::log(&format!("ui: navigation to {} {}", url.chars().take(80).collect::<String>(), if ok { "allowed" } else { "REFUSED" }));
                    }
                    ok
                })
                .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
                .with_download_started_handler(|_, _| false)
                .with_permission_handler(|_| PermissionResponse::Deny)
                .with_drag_drop_handler(|_| true)
                .with_devtools(cfg!(debug_assertions))
                .with_hotkeys_zoom(false)
                .with_background_color((12, 8, 22, 255))
                .with_default_context_menus(false)
                .with_browser_accelerator_keys(false)
                .with_general_autofill_enabled(false)
                .build(&window)
                .map_err(|e| format!("cannot open WebView2 ({e})"))?;
            window.set_visible(true);
            window.set_focus();
            Ok(UiWindow { webview, _ctx: ctx, window, page })
        }

        pub fn send(&self, v: &Value) {
            let _ = self.webview.evaluate_script(&script_for(v));
        }

        pub fn focus(&self) {
            self.window.set_minimized(false);
            self.window.set_visible(true);
            self.window.set_focus();
        }
    }

    /// Ce que fait une liste d'effets d'une page → true : fermer la fenetre.
    pub fn apply(outs: Vec<Out>, win: &UiWindow, proxy: &EventLoopProxy<UserEvent>, exit_code: &mut i32) -> bool {
        let mut close = false;
        for o in outs {
            match o {
                Out::Send(v) => win.send(&v),
                Out::Open(l) => open_url(l.url()),
                Out::Copy(s) => {
                    if let Err(e) = copy_text(&s) {
                        win.send(&serde_json::json!({ "type": "error", "message": format!("Could not copy: {e}.") }));
                    }
                }
                Out::Close => close = true,
                Out::Exit(c) => {
                    *exit_code = c;
                    close = true;
                }
                Out::Run(job) => {
                    let p = proxy.clone();
                    std::thread::spawn(move || {
                        let p2 = p.clone();
                        let done = run_job(job, &move |line: &str| {
                            let _ = p2.send_event(UserEvent::Progress(line.to_string()));
                        });
                        let _ = p.send_event(UserEvent::Done(done));
                    });
                }
            }
        }
        close
    }

    /// Une fenetre seule (l'installation, la desinstallation), jusqu'a sa fermeture → le code de sortie. `smoke` :
    /// l'essai, qui se ferme 3 s apres que la page s'est dessinee (et echoue si elle ne le fait pas en 25 s). Sur un fil
    /// a elle (voir `event_loop`).
    pub fn run_window(view: Box<dyn View + Send>, smoke: bool) -> i32 {
        std::thread::spawn(move || run_window_here(view, smoke)).join().unwrap_or(1)
    }

    fn run_window_here(mut view: Box<dyn View + Send>, smoke: bool) -> i32 {
        let mut event_loop = event_loop();
        let proxy = event_loop.create_proxy();
        let data_dir = setup_data_dir();
        let win = match UiWindow::open(&event_loop, view.as_ref(), &proxy, &data_dir) {
            Ok(w) => w,
            Err(e) => {
                // la console est deja cachee (un double-clic) : le dire dans une boite
                eprintln!("{e}");
                if !smoke {
                    message_box(
                        "Uppwind Link",
                        &format!("This window could not open ({e}).\n\nIt needs Microsoft Edge WebView2 Runtime (free, from Microsoft):\n{}", crate::wizard::WEBVIEW2_URL),
                    );
                }
                remove_data_dir(&data_dir);
                return 1;
            }
        };
        let t0 = Instant::now();
        let mut code = 0;
        let mut deadline = smoke.then(|| t0 + Duration::from_secs(25));
        let mut rendered = false;
        event_loop.run_return(|event, _target, cf| {
            *cf = match deadline {
                Some(d) => ControlFlow::WaitUntil(d),
                None => ControlFlow::Wait,
            };
            let outs = match event {
                Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                    if !rendered {
                        eprintln!("smoke test: the page did not draw itself within 25 s");
                        code = 1;
                    }
                    *cf = ControlFlow::Exit;
                    return;
                }
                Event::UserEvent(UserEvent::Ipc(raw)) => match ipc::parse(win.page, &raw) {
                    Ok(Request::Ready) => {
                        if smoke && !rendered {
                            println!("smoke test: the {} window drew itself in {} ms (WebView2 {})", win.page.name(), t0.elapsed().as_millis(), webview2_version().unwrap_or_default());
                            deadline = Some(Instant::now() + Duration::from_secs(3));
                            *cf = ControlFlow::WaitUntil(deadline.unwrap());
                            if let Some(p) = shot_path() {
                                use tao::platform::windows::WindowExtWindows;
                                shot_later(win.window.hwnd() as u64, p);
                            }
                        }
                        rendered = true;
                        vec![]
                    }
                    Ok(req) => view.on_request(req),
                    Err(e) => {
                        crate::log(&format!("ui: refused a message from the page ({e})"));
                        vec![Out::Send(serde_json::json!({ "type": "error", "message": "That did not work." }))]
                    }
                },
                Event::UserEvent(UserEvent::Progress(l)) => view.on_progress(l),
                Event::UserEvent(UserEvent::Done(d)) => view.on_done(d),
                Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => view.on_request(Request::Close),
                _ => vec![],
            };
            if apply(outs, &win, &proxy, &mut code) {
                *cf = ControlFlow::Exit;
            }
        });
        drop(win);
        remove_data_dir(&data_dir);
        code
    }

    pub fn run_page(view: Box<dyn View + Send>) -> i32 {
        run_window(view, false)
    }

    /// --smoke-test [wizard|status|uninstall] : la vraie fenetre s'ouvre, se dessine, se ferme apres 3 s. Rien ne
    /// s'installe ni ne se desinstalle (la page refuse ces gestes).
    pub fn smoke_test(kind: &str) -> i32 {
        if webview2_version().is_none() {
            eprintln!("smoke test: WebView2 is not installed ({})", crate::wizard::WEBVIEW2_URL);
            return 2;
        }
        match kind {
            "wizard" => match crate::wizard::detect(None) {
                Ok(m) => run_window(Box::new(super::pages::WizardPage::new(crate::wizard::steps::Wizard::new(m)).smoke()), true),
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            },
            "uninstall" => run_window(Box::new(super::pages::UninstallPage::new().smoke()), true),
            "status" => crate::tray::smoke(),
            other => {
                eprintln!("--smoke-test is wizard, status or uninstall (not {other})");
                2
            }
        }
    }
}

#[cfg(not(windows))]
pub fn run_page(_view: Box<dyn pages::View + Send>) -> i32 {
    1
}
#[cfg(not(windows))]
pub fn smoke_test(_kind: &str) -> i32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_carries_its_nonce_and_no_remote_content() {
        let n = nonce();
        assert_eq!(n.len(), 36);
        assert_ne!(n, nonce());
        let h = html(ipc::Page::Wizard, &n, None);
        assert!(h.contains(&format!("script-src 'nonce-{n}'")) && h.contains("default-src 'none'"));
        assert!(h.contains(&format!("<script nonce=\"{n}\">")) && h.contains(&format!("<style nonce=\"{n}\">")));
        assert!(h.contains("data-page=\"wizard\"") && h.contains("data:image/png;base64,iVBOR"));
        // (l'espace de noms SVG n'est pas une adresse chargee)
        let h2 = h.replace("http://www.w3.org/2000/svg", "");
        for bad in ["http://", "https://", "src=\"//", "onclick=", "onload=", " style=\""] {
            let n = h2.matches(bad).count();
            assert_eq!(n, 0, "{bad} appears {n} time(s) in the page");
        }
        assert!(!h.contains("__CSS__") && !h.contains("__JS__") && !h.contains("__NONCE__") && !h.contains("__PREVIEW__"));
        assert!(h.len() < 2 * 1024 * 1024, "NavigateToString takes at most 2 MB");
        let s = html(ipc::Page::Status, &n, Some("window.x=1"));
        assert!(s.contains("data-page=\"status\"") && s.contains(&format!("<script nonce=\"{n}\">window.x=1</script>")));
    }

    #[test]
    fn messages_to_the_page_are_json() {
        let v = serde_json::json!({ "type": "progress", "line": "a </script> \"b\"\u{2028}" });
        let s = script_for(&v);
        assert!(s.starts_with("window.__uwl&&window.__uwl.recv({") && s.ends_with("})"));
        assert!(s.contains(r#"\"b\""#), "{s}");
    }
}
