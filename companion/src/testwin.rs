// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Une fenetre d'essai (Windows) : un faux plugin avec un « potard » a tourner, qui note chaque message recu.
//! - Les tests Rust y injectent des gestes (mode direct) et lisent ce qu'elle a recu (`received`).
//! - `uppwind-companion --test-window` l'ouvre et la propose au studio comme une fenetre de plugin (« Uppwind
//!   Test/0-Test ») : la mesure de bout en bout (geste → image) sans toucher a Live.
//! Une fenetre principale 640×400 (zone client) et une sous-fenetre « potard » 560×300 en (40, 60) : le potard
//! monte quand on le tire vers le haut, la molette le tourne, un trait suit le survol, les touches s'affichent.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rec {
    pub hwnd: u64,
    pub msg: u32,
    pub wparam: usize,
    pub lparam: isize,
}

pub const TITLE: &str = "Uppwind Test/0-Test";
pub const CLIENT_W: i32 = 640;
pub const CLIENT_H: i32 = 400;
pub const KNOB_X: i32 = 40;
pub const KNOB_Y: i32 = 60;
pub const KNOB_W: i32 = 560;
pub const KNOB_H: i32 = 300;

pub struct TestWindow {
    pub top: u64,
    #[allow(dead_code)] // les tests
    pub knob: u64,
    /// 21 oct. : un « menu » a part (une fenetre popup possedee par `top`, comme les menus de Kontakt ou de JUCE) :
    /// un clic droit sur le potard l'ouvre sous le pointeur, un clic dedans le referme.
    #[allow(dead_code)] // les tests
    pub popup: u64,
}

pub const POPUP_W: i32 = 180;
pub const POPUP_H: i32 = 120;

#[cfg(windows)]
pub use win::open_with;
#[cfg(all(windows, test))]
pub use win::open;
#[cfg(all(windows, test))]
pub use win::received;

#[cfg(windows)]
mod win {
    use super::*;
    use std::sync::{mpsc, Mutex, OnceLock};
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, ClientToScreen, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect, SetBkMode, SetTextColor, TextOutW, PAINTSTRUCT, TRANSPARENT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetMessageW, GetParent, RegisterClassW, SetWindowPos, ShowWindow,
        TranslateMessage, HWND_BOTTOM, SWP_NOSIZE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, CS_DBLCLKS,
        CS_HREDRAW, CS_VREDRAW, MSG, SW_HIDE, SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
        WS_VISIBLE,
    };

    struct State {
        value: i32,
        drag_y: Option<i32>,
        hover: (i32, i32),
        keys: String,
        clicks: u32,
    }

    fn log() -> &'static Mutex<Vec<Rec>> {
        static L: OnceLock<Mutex<Vec<Rec>>> = OnceLock::new();
        L.get_or_init(|| Mutex::new(Vec::new()))
    }
    fn state() -> &'static Mutex<State> {
        static S: OnceLock<Mutex<State>> = OnceLock::new();
        S.get_or_init(|| Mutex::new(State { value: 50, drag_y: None, hover: (-1, -1), keys: String::new(), clicks: 0 }))
    }

    /// Ce que la fenetre `hwnd` a recu (souris, molette, clavier), dans l'ordre.
    #[allow(dead_code)] // les tests
    pub fn received(hwnd: u64) -> Vec<Rec> {
        log().lock().unwrap().iter().filter(|r| r.hwnd == hwnd).copied().collect()
    }

    /// fenetre d'essai → son menu
    fn popups() -> &'static Mutex<std::collections::HashMap<u64, u64>> {
        static P: OnceLock<Mutex<std::collections::HashMap<u64, u64>>> = OnceLock::new();
        P.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
    }

    /// Le « menu » : vert, il note ce qu'il recoit, un clic gauche le referme.
    unsafe extern "system" fn popup_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if (0x0100..=0x0109).contains(&msg) || (0x0200..=0x020E).contains(&msg) {
            log().lock().unwrap().push(Rec { hwnd: h.0 as isize as u64, msg, wparam: wp.0, lparam: lp.0 });
        }
        if msg == 0x0202 || (msg == 0x0100 && wp.0 == 0x1B) {
            let _ = ShowWindow(h, SW_HIDE);
            return LRESULT(0);
        }
        if msg == 0x000F {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(h, &mut ps);
            let g = CreateSolidBrush(COLORREF(0x0000_CC00));
            FillRect(dc, &RECT { left: 0, top: 0, right: POPUP_W, bottom: POPUP_H }, g);
            let _ = DeleteObject(g.into());
            let _ = EndPaint(h, &ps);
            return LRESULT(0);
        }
        DefWindowProcW(h, msg, wp, lp)
    }

    fn lo(l: isize) -> i32 {
        (l as u32 & 0xffff) as u16 as i16 as i32
    }
    fn hi(l: isize) -> i32 {
        ((l as u32 >> 16) & 0xffff) as u16 as i16 as i32
    }

    unsafe extern "system" fn proc_(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        let interesting = (0x0100..=0x0109).contains(&msg) || (0x0200..=0x020E).contains(&msg);
        if interesting {
            log().lock().unwrap().push(Rec { hwnd: h.0 as isize as u64, msg, wparam: wp.0, lparam: lp.0 });
        }
        let is_knob = GetParent(h).map(|p| !p.is_invalid()).unwrap_or(false);
        let mut repaint = false;
        {
            let mut s = state().lock().unwrap();
            match msg {
                0x0200 => {
                    s.hover = (lo(lp.0), hi(lp.0));
                    if let (Some(y0), true) = (s.drag_y, wp.0 & 1 != 0) {
                        s.value = (s.value + (y0 - hi(lp.0))).clamp(0, 100);
                        s.drag_y = Some(hi(lp.0));
                    }
                    repaint = true;
                }
                0x0201 | 0x0203 => {
                    s.drag_y = Some(hi(lp.0));
                    s.clicks += 1;
                    repaint = true;
                }
                0x0202 => {
                    s.drag_y = None;
                    repaint = true;
                }
                0x020A => {
                    let d = ((wp.0 >> 16) as u16 as i16) as i32;
                    s.value = (s.value + d / 24).clamp(0, 100);
                    repaint = true;
                }
                0x0102 => {
                    if let Some(c) = char::from_u32(wp.0 as u32).filter(|c| !c.is_control()) {
                        s.keys.push(c);
                        if s.keys.chars().count() > 24 {
                            s.keys.remove(0);
                        }
                        repaint = true;
                    }
                }
                _ => {}
            }
        }
        if repaint && is_knob {
            let _ = InvalidateRect(Some(h), None, false);
        }
        // un clic droit sur le potard : le « menu » s'ouvre sous le pointeur (une fenetre a part, comme Kontakt)
        if msg == 0x0205 && is_knob {
            let top = GetParent(h).map(|p| p.0 as isize as u64).unwrap_or(0);
            let pop = popups().lock().unwrap().get(&top).copied();
            if let Some(pop) = pop {
                let mut pt = POINT { x: lo(lp.0), y: hi(lp.0) };
                let _ = ClientToScreen(h, &mut pt);
                let ph = HWND(pop as isize as *mut core::ffi::c_void);
                let _ = SetWindowPos(ph, None, pt.x, pt.y, POPUP_W, POPUP_H, SWP_NOACTIVATE | SWP_NOZORDER);
                let _ = ShowWindow(ph, SW_SHOWNOACTIVATE);
            }
        }
        if msg == 0x000F {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(h, &mut ps);
            let s = state().lock().unwrap();
            let (w, hh) = if is_knob { (KNOB_W, KNOB_H) } else { (CLIENT_W, CLIENT_H) };
            let bg = CreateSolidBrush(COLORREF(if is_knob { 0x0030_1A1A } else { 0x0020_1010 }));
            FillRect(dc, &RECT { left: 0, top: 0, right: w, bottom: hh }, bg);
            let _ = DeleteObject(bg.into());
            if is_knob {
                // le « potard » : une barre de 0 a 100 %
                let bar = CreateSolidBrush(COLORREF(0x0000_88FF));
                FillRect(dc, &RECT { left: 20, top: 120, right: 20 + (s.value * (w - 40)) / 100, bottom: 180 }, bar);
                let _ = DeleteObject(bar.into());
                // le survol : une croix ; l'appui : un carre plein sous le pointeur (comme un bouton enfonce)
                let cross = CreateSolidBrush(COLORREF(0x00FF_FFFF));
                let (hx, hy) = s.hover;
                if hx >= 0 {
                    FillRect(dc, &RECT { left: hx - 8, top: hy, right: hx + 9, bottom: hy + 1 }, cross);
                    FillRect(dc, &RECT { left: hx, top: hy - 8, right: hx + 1, bottom: hy + 9 }, cross);
                    if s.drag_y.is_some() {
                        FillRect(dc, &RECT { left: hx - 6, top: hy - 6, right: hx + 7, bottom: hy + 7 }, cross);
                    }
                }
                let _ = DeleteObject(cross.into());
                SetBkMode(dc, TRANSPARENT);
                SetTextColor(dc, COLORREF(0x00FF_FFFF));
                if hx >= 0 {
                    // la valeur pres du pointeur (comme l'infobulle d'un potard)
                    let v: Vec<u16> = format!("{}", s.value).encode_utf16().collect();
                    let _ = TextOutW(dc, hx + 12, hy + 6, &v);
                }
                let t: Vec<u16> = format!("value {}  clicks {}  keys {}", s.value, s.clicks, s.keys).encode_utf16().collect();
                let _ = TextOutW(dc, 20, 20, &t);
            } else {
                SetBkMode(dc, TRANSPARENT);
                SetTextColor(dc, COLORREF(0x00CC_CCCC));
                let t: Vec<u16> = "Uppwind companion test window (fake plugin)".encode_utf16().collect();
                let _ = TextOutW(dc, 40, 20, &t);
            }
            drop(s);
            let _ = EndPaint(h, &ps);
            return LRESULT(0);
        }
        DefWindowProcW(h, msg, wp, lp)
    }

    /// Ouvre la fenetre (un fil a elle, avec sa boucle de messages). `at` : sa place a l'ecran (coin de la zone client).
    #[allow(dead_code)] // les tests
    pub fn open(at: (i32, i32)) -> Result<TestWindow, String> {
        open_with(at, false, false)
    }

    /// `unaware` : la fenetre est « DPI unaware », comme Diva : sur un ecran a 125 %, Windows l'agrandit (640×400
    /// dessines, 800×500 a l'ecran) et la capture la rend a sa taille logique. `ghost` : mise tout au fond, derriere
    /// les autres fenetres — a l'ecran pour etre dessinee, cachee a celui qui travaille (derriere Live agrandi) ; la
    /// capture la voit quand meme : c'est aussi l'essai d'une fenetre couverte, comme Diva derriere Chrome. (Une fenetre
    /// « layered » presque transparente ne se redessine plus du tout : essaye le 21 oct.)
    pub fn open_with(at: (i32, i32), unaware: bool, ghost: bool) -> Result<TestWindow, String> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || unsafe {
            if unaware {
                use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_UNAWARE};
                let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_UNAWARE);
            }
            let inst = match GetModuleHandleW(None) {
                Ok(m) => m,
                Err(e) => {
                    let _ = tx.send(Err(e.to_string()));
                    return;
                }
            };
            static REG: OnceLock<()> = OnceLock::new();
            REG.get_or_init(|| {
                let wc = WNDCLASSW {
                    style: CS_DBLCLKS | CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some(proc_),
                    hInstance: inst.into(),
                    lpszClassName: w!("UppwindCompanionTestWindow"),
                    ..Default::default()
                };
                RegisterClassW(&wc);
            });
            let style = WS_POPUP | WS_CAPTION | WS_VISIBLE;
            let ex = WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
            let mut r = RECT { left: at.0, top: at.1, right: at.0 + CLIENT_W, bottom: at.1 + CLIENT_H };
            let _ = AdjustWindowRectEx(&mut r, style, false, ex);
            let title: Vec<u16> = TITLE.encode_utf16().chain(Some(0)).collect();
            let top = CreateWindowExW(
                ex,
                w!("UppwindCompanionTestWindow"),
                PCWSTR(title.as_ptr()),
                style,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                None,
                None,
                Some(inst.into()),
                None,
            );
            let top = match top {
                Ok(h) => h,
                Err(e) => {
                    let _ = tx.send(Err(format!("CreateWindowEx: {e}")));
                    return;
                }
            };
            let knob = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("UppwindCompanionTestWindow"),
                w!("knob"),
                WS_CHILD | WS_VISIBLE,
                KNOB_X,
                KNOB_Y,
                KNOB_W,
                KNOB_H,
                Some(top),
                None,
                Some(inst.into()),
                None,
            );
            let knob = match knob {
                Ok(h) => h,
                Err(e) => {
                    let _ = tx.send(Err(format!("CreateWindowEx (child): {e}")));
                    return;
                }
            };
            // le « menu » : une fenetre popup possedee par la fenetre d'essai, cachee jusqu'au clic droit
            static REG_POPUP: OnceLock<()> = OnceLock::new();
            REG_POPUP.get_or_init(|| {
                let wc = WNDCLASSW { lpfnWndProc: Some(popup_proc), hInstance: inst.into(), lpszClassName: w!("UppwindCompanionTestPopup"), ..Default::default() };
                RegisterClassW(&wc);
            });
            let popup = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("UppwindCompanionTestPopup"),
                w!(""),
                WS_POPUP,
                0,
                0,
                POPUP_W,
                POPUP_H,
                Some(top),
                None,
                Some(inst.into()),
                None,
            );
            let popup = match popup {
                Ok(h) => h,
                Err(e) => {
                    let _ = tx.send(Err(format!("CreateWindowEx (popup): {e}")));
                    return;
                }
            };
            popups().lock().unwrap().insert(top.0 as isize as u64, popup.0 as isize as u64);
            // la zone client exactement CLIENT_W × CLIENT_H (le cadre reel peut differer d'AdjustWindowRectEx)
            let mut cr = RECT::default();
            let _ = GetClientRect(top, &mut cr);
            let (dw, dh) = (CLIENT_W - (cr.right - cr.left), CLIENT_H - (cr.bottom - cr.top));
            if dw != 0 || dh != 0 {
                let _ = SetWindowPos(top, None, 0, 0, r.right - r.left + dw, r.bottom - r.top + dh, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
            let _ = ShowWindow(top, SW_SHOWNOACTIVATE);
            if ghost {
                let _ = SetWindowPos(top, Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
            let _ = tx.send(Ok(TestWindow { top: top.0 as isize as u64, knob: knob.0 as isize as u64, popup: popup.0 as isize as u64 }));
            let mut m = MSG::default();
            while GetMessageW(&mut m, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&m);
                DispatchMessageW(&m);
            }
        });
        rx.recv_timeout(std::time::Duration::from_secs(5)).map_err(|_| "the test window did not open".to_string())?
    }
}
