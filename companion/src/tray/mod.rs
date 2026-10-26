// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! L'icone de la zone de notification du compagnon installe (lance a l'ouverture de session avec --background, ou par le
//! menu Demarrer : --app) : « Open Uppwind Link » (la fenetre d'etat), « Open the studio », « Start with Windows »
//! (coche), « Quit ». Un clic sur l'icone ouvre la fenetre d'etat ; un deuxieme lancement la montre (single.rs).
//! Le serveur du compagnon tourne dans un fil (main.rs) ; l'icone et la fenetre sur le fil principal.

pub mod single;
pub mod status;

use std::sync::OnceLock;

/// Le compagnon avec son icone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppMode {
    /// ouvrir la fenetre d'etat tout de suite (--app, le menu Demarrer) ; sinon l'icone seule (--background)
    pub window: bool,
    /// une mise a jour : arreter d'abord celui qui tourne
    pub replace: bool,
}

/// Le serveur de ce processus : son port, son jeton (la fenetre d'etat montre le code).
static SERVER: OnceLock<(u16, String)> = OnceLock::new();

/// Le modele de la fenetre d'etat, maintenant (un travail de fond : la machine est relue).
pub fn status_model() -> Result<status::StatusModel, String> {
    let p = crate::wizard::parts()?;
    let r = status::report(&p.probe);
    let (port, token) = SERVER.get().cloned().unwrap_or((0, String::new()));
    Ok(status::model(&r, port, &token, status::live(), crate::audio::available(), p.dirs.startup_link().exists(), crate::wizard::sandbox().map(|d| d.display().to_string())))
}

#[cfg(windows)]
pub use win::{prepare, run, smoke};

#[cfg(windows)]
mod win {
    use super::*;
    use crate::ui::ipc::{self, Request};
    use crate::ui::pages::{Out, StatusPage, View};
    use crate::ui::{apply_out as apply, UiWindow, UserEvent};
    use std::time::{Duration, Instant};
    use tao::event::{Event, StartCause, WindowEvent};
    use tao::event_loop::ControlFlow;
    use tao::platform::run_return::EventLoopExtRunReturn;
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    /// Avant le serveur : la place (single.rs). Err(code) : un autre tourne (il a ete prevenu), sortir avec ce code.
    pub fn prepare(mode: &AppMode) -> Result<single::Guard, i32> {
        match single::acquire(mode.replace) {
            single::Acquire::Got(g) => Ok(g),
            single::Acquire::Running => {
                if mode.window {
                    single::signal_show();
                }
                Err(0)
            }
        }
    }

    fn autostart_on() -> bool {
        crate::install::Dirs::from_env().map(|d| d.startup_link().exists()).unwrap_or(false)
    }

    /// La boucle de l'icone (et de la fenetre d'etat) → le code de sortie. `smoke` : l'essai (pas de serveur, la
    /// fenetre se ferme 3 s apres s'etre dessinee).
    fn run_loop(mode: AppMode, guard: Option<&single::Guard>, port: u16, smoke: bool) -> i32 {
        let mut event_loop = crate::ui::event_loop();
        let proxy = event_loop.create_proxy();
        if let Some(g) = guard {
            let p = proxy.clone();
            single::listen(g, move |show| {
                let _ = p.send_event(if show { UserEvent::Show } else { UserEvent::Quit });
            });
        }
        let open = MenuItem::new("Open Uppwind Link", true, None);
        let studio = MenuItem::new("Open the studio", true, None);
        let auto = CheckMenuItem::new("Start with Windows", true, autostart_on(), None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        let sep1 = PredefinedMenuItem::separator();
        let sep2 = PredefinedMenuItem::separator();
        if let Err(e) = menu.append_items(&[&open, &studio, &sep1, &auto, &sep2, &quit]) {
            crate::log(&format!("tray: menu ({e})"));
        }
        let p = proxy.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = p.send_event(UserEvent::Menu(e.id));
        }));
        let p = proxy.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            let _ = p.send_event(UserEvent::Tray(e));
        }));
        let mut tb = TrayIconBuilder::new()
            .with_tooltip(if port > 0 { format!("Uppwind Link · port {port}") } else { "Uppwind Link".to_string() })
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false);
        if let Ok(icon) = tray_icon::Icon::from_resource(1, Some((32, 32))) {
            tb = tb.with_icon(icon);
        }
        let tray = match tb.build() {
            Ok(t) => Some(t),
            Err(e) => {
                crate::log(&format!("tray: no icon in the notification area ({e})"));
                None
            }
        };
        let data_dir = crate::install::Dirs::from_env().map(|d| crate::wizard::flow::webview_dir(&d)).unwrap_or_else(|_| crate::ui::setup_data_dir());
        let webview_ok = crate::ui::webview2_version().is_some();
        let mut win: Option<(UiWindow, StatusPage)> = None;
        let mut code = 0;
        let mut next_tick: Option<Instant> = None;
        let t0 = Instant::now();
        let mut smoke_end: Option<Instant> = smoke.then(|| t0 + Duration::from_secs(25));
        let mut rendered = false;
        let mut want_window = mode.window;
        event_loop.run_return(|event, target, cf| {
            let mut show = std::mem::take(&mut want_window);
            let mut outs: Vec<Out> = Vec::new();
            match event {
                Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                    if smoke_end.is_some_and(|d| Instant::now() >= d) {
                        if !rendered {
                            eprintln!("smoke test: the status window did not draw itself within 25 s");
                            code = 1;
                        }
                        *cf = ControlFlow::Exit;
                        return;
                    }
                    if let Some((_, page)) = win.as_mut() {
                        outs = page.tick();
                    }
                    next_tick = win.as_ref().map(|_| Instant::now() + Duration::from_secs(3));
                }
                Event::UserEvent(UserEvent::Show) => show = true,
                Event::UserEvent(UserEvent::Quit) => {
                    *cf = ControlFlow::Exit;
                    return;
                }
                Event::UserEvent(UserEvent::Menu(id)) => {
                    if id == *open.id() {
                        show = true;
                    } else if id == *studio.id() {
                        crate::ui::open_url(crate::wizard::studio_url());
                    } else if id == *auto.id() {
                        // la case s'est deja cochee (ou decochee) au clic : on fait comme elle dit
                        let on = auto.is_checked();
                        outs.push(Out::Run(crate::ui::pages::Job::Autostart(on)));
                    } else if id == *quit.id() {
                        *cf = ControlFlow::Exit;
                        return;
                    }
                }
                Event::UserEvent(UserEvent::Tray(TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. })) => show = true,
                Event::UserEvent(UserEvent::Tray(TrayIconEvent::DoubleClick { button: MouseButton::Left, .. })) => show = true,
                Event::UserEvent(UserEvent::Ipc(raw)) => {
                    if let Some((w, page)) = win.as_mut() {
                        match ipc::parse(w.page, &raw) {
                            Ok(Request::Ready) => {
                                if smoke && !rendered {
                                    println!("smoke test: the status window drew itself in {} ms, with its icon in the notification area: {}", t0.elapsed().as_millis(), tray.is_some());
                                    smoke_end = Some(Instant::now() + Duration::from_secs(3));
                                    if let Some(p) = crate::ui::shot_path() {
                                        use tao::platform::windows::WindowExtWindows;
                                        crate::ui::shot_later(w.window.hwnd() as u64, p);
                                    }
                                }
                                rendered = true;
                            }
                            Ok(r) => outs = page.on_request(r),
                            Err(e) => crate::log(&format!("ui: refused a message from the page ({e})")),
                        }
                    }
                }
                Event::UserEvent(UserEvent::Progress(_)) => {}
                Event::UserEvent(UserEvent::Done(d)) => {
                    if let crate::ui::pages::Done::Status(Ok(m)) = &d {
                        auto.set_checked(m.autostart);
                    }
                    if let crate::ui::pages::Done::Autostart(r) = &d {
                        auto.set_checked(if r.is_ok() { autostart_on() } else { !auto.is_checked() });
                    }
                    if let Some((_, page)) = win.as_mut() {
                        outs = page.on_done(d);
                    }
                }
                Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                    // la fenetre se ferme (sa memoire WebView2 avec) ; l'icone reste
                    if smoke {
                        *cf = ControlFlow::Exit;
                        return;
                    }
                    win = None;
                }
                _ => {}
            }
            if show {
                match win.as_ref() {
                    Some((w, _)) => w.focus(),
                    None if webview_ok => {
                        let page = StatusPage::new();
                        match UiWindow::open(target, &page, &proxy, &data_dir) {
                            Ok(w) => win = Some((w, page)),
                            Err(e) => crate::log(&format!("ui: {e}")),
                        }
                    }
                    None => crate::ui::message_box("Uppwind Link", &format!("Uppwind Link is running.\n\nIts window needs Microsoft Edge WebView2 Runtime (free, from Microsoft):\n{}", crate::wizard::WEBVIEW2_URL)),
                }
                if win.is_some() && next_tick.is_none() {
                    next_tick = Some(Instant::now() + Duration::from_secs(3));
                }
            }
            if let Some((w, _)) = win.as_ref() {
                let mut dummy = 0;
                if apply(outs, w, &proxy, &mut dummy) {
                    win = None;
                }
            } else {
                // sans fenetre : seulement les travaux (la case « Start with Windows » du menu)
                for o in outs {
                    if let Out::Run(job) = o {
                        let p = proxy.clone();
                        std::thread::spawn(move || {
                            let done = crate::ui::pages::run_job(job, &|_| {});
                            let _ = p.send_event(UserEvent::Done(done));
                        });
                    }
                }
            }
            if win.is_none() {
                next_tick = None;
            }
            let wake = [next_tick, smoke_end].into_iter().flatten().min();
            *cf = match wake {
                Some(t) => ControlFlow::WaitUntil(t),
                None => ControlFlow::Wait,
            };
        });
        drop(win);
        drop(tray);
        code
    }

    /// Le compagnon avec son icone → le code de sortie. Sur un fil a lui (main.rs) : le serveur reste sur le fil
    /// principal, comme dans la console ; quand l'icone quitte, le processus s'arrete.
    pub fn run(mode: AppMode, guard: single::Guard, port: u16, token: String) -> i32 {
        let _ = SERVER.set((port, token));
        let c = run_loop(mode, Some(&guard), port, false);
        drop(guard);
        c
    }

    /// --smoke-test status : l'icone et la fenetre d'etat, sans serveur, 3 s.
    pub fn smoke() -> i32 {
        std::thread::spawn(|| run_loop(AppMode { window: true, replace: false }, None, 0, true)).join().unwrap_or(1)
    }
}

#[cfg(not(windows))]
pub fn smoke() -> i32 {
    1
}
