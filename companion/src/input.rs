// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Les gestes du studio injectes dans la fenetre du plugin (etape 1, partie 2).
//!
//! Deux modes, choisis par le studio (et retenus par plugin) :
//! - DIRECT (par defaut) : des messages Windows envoyes a la sous-fenetre la plus profonde sous le point, sans
//!   toucher au vrai curseur ni passer Live devant. Souris : PostMessage (WM_MOUSEMOVE, WM_xBUTTONDOWN/UP/DBLCLK,
//!   WM_MOUSEWHEEL) ; pendant un glisser, la fenetre qui a recu l'appui recoit tout (comme une capture Windows).
//!   Clavier : SendMessageTimeout (WM_KEYDOWN / WM_CHAR / WM_KEYUP) directement a la fenetre qui a le focus dans Live
//!   (sinon la derniere cliquee) : le message ne passe pas par la boucle de Live, donc ses raccourcis (Espace = lecture)
//!   ne partent pas et il n'y a pas de double caractere (TranslateMessage).
//! - FOCUS (le secours, pour les plugins qui lisent le vrai curseur) : la fenetre du plugin passe un instant au premier
//!   plan (TOPMOST, sans l'activer), le vrai curseur va au point et SendInput rejoue le geste ; puis tout est remis
//!   (curseur, ordre des fenetres, Chrome au premier plan). Un clic ou un glisser est rejoue AU RELACHEMENT (tant que
//!   le bouton est tenu, Windows donne la souris a Chrome) ; la molette tout de suite ; le survol est ignore ; le
//!   clavier passe comme en direct.
//!
//! Securite : l'injecteur ne vise que la fenetre suivie (verifiee par la session : une fenetre de plugin de Live) et
//! ses sous-fenetres ; en FOCUS, il verifie avant chaque clic que la fenetre sous le point est bien celle-la, et un
//! glisser ou un relachement ne sort jamais de la zone du plugin (keep_inside).
//! Le journal ne contient jamais ce qui est tape.

use crate::protocol::{InputEvent, PointerAction, Pt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Direct,
    Focus,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "direct" => Some(Mode::Direct),
            "focus" => Some(Mode::Focus),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Mode::Direct => "direct",
            Mode::Focus => "focus",
        }
    }
}

// ── Les messages Windows (constantes recopiees : les fonctions pures se testent sans Windows) ─────────────────
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_KEYUP: u32 = 0x0101;
pub const WM_CHAR: u32 = 0x0102;
pub const WM_SYSKEYDOWN: u32 = 0x0104;
pub const WM_SYSKEYUP: u32 = 0x0105;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_LBUTTONDBLCLK: u32 = 0x0203;
pub const WM_RBUTTONDOWN: u32 = 0x0204;
pub const WM_RBUTTONUP: u32 = 0x0205;
pub const WM_RBUTTONDBLCLK: u32 = 0x0206;
pub const WM_MBUTTONDOWN: u32 = 0x0207;
pub const WM_MBUTTONUP: u32 = 0x0208;
pub const WM_MBUTTONDBLCLK: u32 = 0x0209;
pub const WM_MOUSEWHEEL: u32 = 0x020A;
pub const WM_MOUSEHWHEEL: u32 = 0x020E;
pub const MK_LBUTTON: usize = 0x0001;
pub const MK_RBUTTON: usize = 0x0002;
pub const MK_SHIFT: usize = 0x0004;
pub const MK_CONTROL: usize = 0x0008;
pub const MK_MBUTTON: usize = 0x0010;
pub const WHEEL_DELTA: f64 = 120.0;
/// Le DOM de Chrome sous Windows donne 100 pixels par cran de molette.
pub const DOM_PIXELS_PER_NOTCH: f64 = 100.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WinMsg {
    pub msg: u32,
    pub wparam: usize,
    pub lparam: isize,
}

/// (x, y) de l'image du studio (iw×ih) → pixels de la zone client (cw×ch) de la fenetre. Sans iw/ih : 1:1. Le
/// resultat peut sortir de la zone (un glisser) ; il reste dans ce que lParam sait porter (16 bits signes).
pub fn map_point(pt: &Pt, cw: i32, ch: i32) -> (i32, i32) {
    let sx = pt.iw.map(|w| cw as f64 / w).unwrap_or(1.0);
    let sy = pt.ih.map(|h| ch as f64 / h).unwrap_or(1.0);
    let x = (pt.x * sx).floor().clamp(-32768.0, 32767.0) as i32;
    let y = (pt.y * sy).floor().clamp(-32768.0, 32767.0) as i32;
    (x, y)
}

/// Une fenetre a l'echelle de Windows (« DPI unaware » ou « system aware ») recoit ses coordonnees dans SES pixels :
/// physiques × dpi de la fenetre / dpi de l'ecran. 0 : inconnu (rien ne change).
pub fn scale_point(p: (i32, i32), window_dpi: u32, monitor_dpi: u32) -> (i32, i32) {
    if window_dpi == 0 || monitor_dpi == 0 || window_dpi == monitor_dpi {
        return p;
    }
    let f = window_dpi as f64 / monitor_dpi as f64;
    ((p.0 as f64 * f).round() as i32, (p.1 as f64 * f).round() as i32)
}

pub fn make_lparam(x: i32, y: i32) -> isize {
    (((y as i16 as u16 as u32) << 16) | (x as i16 as u16 as u32)) as i32 as isize
}

/// Les boutons tenus (masque du DOM : 1 gauche, 2 droit, 4 milieu) et Maj / Ctrl → les drapeaux MK_ de wParam.
pub fn mk_flags(dom_buttons: u8, mods: u8) -> usize {
    let mut f = 0;
    if dom_buttons & 1 != 0 {
        f |= MK_LBUTTON;
    }
    if dom_buttons & 2 != 0 {
        f |= MK_RBUTTON;
    }
    if dom_buttons & 4 != 0 {
        f |= MK_MBUTTON;
    }
    if mods & 1 != 0 {
        f |= MK_SHIFT;
    }
    if mods & 2 != 0 {
        f |= MK_CONTROL;
    }
    f
}

/// Bouton du DOM (0 gauche, 1 milieu, 2 droit) → (appui, relache, double clic) et son bit dans `buttons`.
pub fn button_msgs(button: u8) -> (u32, u32, u32, u8) {
    match button {
        2 => (WM_RBUTTONDOWN, WM_RBUTTONUP, WM_RBUTTONDBLCLK, 2),
        1 => (WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MBUTTONDBLCLK, 4),
        _ => (WM_LBUTTONDOWN, WM_LBUTTONUP, WM_LBUTTONDBLCLK, 1),
    }
}

/// Les messages d'un geste de pointeur, deja en coordonnees de la fenetre visee. `buttons` : les boutons tenus APRES
/// le geste (comme PointerEvent.buttons). `class_dblclk` : la classe de la fenetre veut WM_xBUTTONDBLCLK (CS_DBLCLKS).
pub fn pointer_msgs(action: PointerAction, button: u8, buttons: u8, mods: u8, p: (i32, i32), class_dblclk: bool) -> Vec<WinMsg> {
    let lp = make_lparam(p.0, p.1);
    let (down, up, dbl, bit) = button_msgs(button);
    match action {
        PointerAction::Move => vec![WinMsg { msg: WM_MOUSEMOVE, wparam: mk_flags(buttons, mods), lparam: lp }],
        PointerAction::Down | PointerAction::DblClick => {
            let held = buttons | bit;
            let msg = if action == PointerAction::DblClick && class_dblclk { dbl } else { down };
            vec![
                // le survol d'abord (beaucoup de plugins choisissent le controle vise au mouvement)
                WinMsg { msg: WM_MOUSEMOVE, wparam: mk_flags(held & !bit, mods), lparam: lp },
                WinMsg { msg, wparam: mk_flags(held, mods), lparam: lp },
            ]
        }
        PointerAction::Up => vec![WinMsg { msg: up, wparam: mk_flags(buttons & !bit, mods), lparam: lp }],
    }
}

/// Molette : dy du DOM (positif = vers le bas) → delta Windows (positif = vers le haut), 120 par cran.
pub fn wheel_delta(dom: f64) -> i32 {
    (-dom * WHEEL_DELTA / DOM_PIXELS_PER_NOTCH).round().clamp(-32000.0, 32000.0) as i32
}

/// Les messages de molette ; `screen` : le point en coordonnees d'ecran (WM_MOUSEWHEEL les veut ainsi).
pub fn wheel_msgs(dx: f64, dy: f64, mods: u8, screen: (i32, i32)) -> Vec<WinMsg> {
    let lp = make_lparam(screen.0, screen.1);
    let mk = mk_flags(0, mods);
    let mut out = Vec::new();
    let v = wheel_delta(dy);
    if v != 0 {
        out.push(WinMsg { msg: WM_MOUSEWHEEL, wparam: ((v as i16 as u16 as usize) << 16) | mk, lparam: lp });
    }
    // horizontal : dx positif (vers la droite) = delta positif
    let h = -wheel_delta(dx);
    if h != 0 {
        out.push(WinMsg { msg: WM_MOUSEHWHEEL, wparam: ((h as i16 as u16 as usize) << 16) | mk, lparam: lp });
    }
    out
}

/// KeyboardEvent.code (la touche physique) → touche virtuelle Windows et « etendue ». Les lettres et chiffres :
/// leur VK ; la ponctuation : les VK_OEM d'un clavier US (le caractere, lui, part en WM_CHAR depuis `key`).
pub fn vk_of_code(code: &str) -> Option<(u16, bool)> {
    let b = code.as_bytes();
    if code.len() == 4 && code.starts_with("Key") && b[3].is_ascii_uppercase() {
        return Some((b[3] as u16, false));
    }
    if code.len() == 6 && code.starts_with("Digit") && b[5].is_ascii_digit() {
        return Some((b[5] as u16, false));
    }
    if code.len() == 7 && code.starts_with("Numpad") && b[6].is_ascii_digit() {
        return Some((0x60 + (b[6] - b'0') as u16, false));
    }
    if let Some(n) = code.strip_prefix('F').and_then(|s| s.parse::<u16>().ok()) {
        if (1..=24).contains(&n) {
            return Some((0x6F + n, false));
        }
    }
    Some(match code {
        "Enter" => (0x0D, false),
        "NumpadEnter" => (0x0D, true),
        "Escape" => (0x1B, false),
        "Backspace" => (0x08, false),
        "Tab" => (0x09, false),
        "Space" => (0x20, false),
        "ArrowLeft" => (0x25, true),
        "ArrowUp" => (0x26, true),
        "ArrowRight" => (0x27, true),
        "ArrowDown" => (0x28, true),
        "PageUp" => (0x21, true),
        "PageDown" => (0x22, true),
        "End" => (0x23, true),
        "Home" => (0x24, true),
        "Insert" => (0x2D, true),
        "Delete" => (0x2E, true),
        "ShiftLeft" => (0xA0, false),
        "ShiftRight" => (0xA1, false),
        "ControlLeft" => (0xA2, false),
        "ControlRight" => (0xA3, true),
        "AltLeft" => (0xA4, false),
        "AltRight" => (0xA5, true),
        "NumpadMultiply" => (0x6A, false),
        "NumpadAdd" => (0x6B, false),
        "NumpadSubtract" => (0x6D, false),
        "NumpadDecimal" => (0x6E, false),
        "NumpadDivide" => (0x6F, true),
        "Semicolon" => (0xBA, false),
        "Equal" => (0xBB, false),
        "Comma" => (0xBC, false),
        "Minus" => (0xBD, false),
        "Period" => (0xBE, false),
        "Slash" => (0xBF, false),
        "Backquote" => (0xC0, false),
        "BracketLeft" => (0xDB, false),
        "Backslash" => (0xDC, false),
        "BracketRight" => (0xDD, false),
        "Quote" => (0xDE, false),
        "IntlBackslash" => (0xE2, false),
        _ => return None,
    })
}

/// Les caracteres a envoyer en WM_CHAR pour une touche enfoncee : le caractere de `key` (deja selon la disposition
/// du clavier : AZERTY, accents) sans Ctrl ni Alt (Ctrl+Alt = AltGr sous Windows : oui), plus Entree, Retour, Tab.
pub fn chars_for_key(key: &str, mods: u8) -> Vec<u16> {
    let ctrl = mods & 2 != 0;
    let alt = mods & 4 != 0;
    match key {
        "Enter" => return if ctrl || alt { vec![] } else { vec![0x0D] },
        "Backspace" => return if ctrl || alt { vec![] } else { vec![0x08] },
        "Tab" => return if ctrl || alt { vec![] } else { vec![0x09] },
        _ => {}
    }
    if key.chars().count() != 1 || (ctrl != alt) || mods & 8 != 0 {
        return vec![];
    }
    key.encode_utf16().collect()
}

/// lParam d'un message de touche : repetition 1, code de balayage, etendue ; une relache porte les bits 30 et 31.
pub fn key_lparam(scan: u32, extended: bool, up: bool) -> isize {
    let mut v: u32 = 1 | ((scan & 0xff) << 16);
    if extended {
        v |= 1 << 24;
    }
    if up {
        v |= 0xC000_0000;
    }
    v as i32 as isize
}

/// Les messages d'une touche (Alt seul : WM_SYSKEY*).
pub fn key_msgs(down: bool, vk: u16, scan: u32, extended: bool, chars: &[u16], mods: u8) -> Vec<WinMsg> {
    let sys = mods & 4 != 0 && mods & 2 == 0;
    let lp = key_lparam(scan, extended, !down);
    if down {
        let mut v = vec![WinMsg { msg: if sys { WM_SYSKEYDOWN } else { WM_KEYDOWN }, wparam: vk as usize, lparam: lp }];
        for c in chars {
            v.push(WinMsg { msg: WM_CHAR, wparam: *c as usize, lparam: lp });
        }
        v
    } else {
        vec![WinMsg { msg: if sys { WM_SYSKEYUP } else { WM_KEYUP }, wparam: vk as usize, lparam: lp }]
    }
}

/// Un pas d'un geste rejoue en mode FOCUS (coordonnees de la zone client de la fenetre suivie, pixels physiques).
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Down { button: u8, p: (i32, i32) },
    Move { p: (i32, i32) },
    Up { button: u8, p: (i32, i32) },
    Wheel { p: (i32, i32), dx: f64, dy: f64 },
}

/// Au plus MAX_STEPS pas par geste rejoue : au-dela on garde un pas sur deux (le trajet reste le meme).
pub const MAX_STEPS: usize = 240;

/// FOCUS (audit de publication) : un point de l'ecran ou passe le vrai curseur pendant un glisser ou au relachement.
/// Dans la zone client du plugin (`client`, a l'ecran) ou dans un de ses menus : tel quel ; ailleurs (un glisser qui
/// sort) : ramene au bord de la zone client. Le curseur et le relachement ne quittent jamais le plugin : rien n'est
/// lache sur une autre fenetre (un fichier glisse hors d'un plugin, par exemple).
pub fn keep_inside(p: (i32, i32), client: (i32, i32, i32, i32), menus: &[(i32, i32, i32, i32)]) -> (i32, i32) {
    let inside = |r: &(i32, i32, i32, i32)| p.0 >= r.0 && p.0 < r.2 && p.1 >= r.1 && p.1 < r.3;
    if inside(&client) || menus.iter().any(inside) {
        return p;
    }
    (p.0.clamp(client.0, (client.2 - 1).max(client.0)), p.1.clamp(client.1, (client.3 - 1).max(client.1)))
}

pub fn thin_steps(steps: &mut Vec<Step>) {
    if steps.len() <= MAX_STEPS {
        return;
    }
    let last = steps.len() - 1;
    let mut i = 0;
    steps.retain(|s| {
        let keep = !matches!(s, Step::Move { .. }) || i % 2 == 0 || i == last;
        i += 1;
        keep
    });
}

/// Ce que la session apprend d'une injection.
#[derive(Debug, Default, Clone)]
pub struct Outcome {
    /// Le geste a ete applique (false : mis de cote, ex. un glisser en FOCUS attend le relachement).
    pub applied: bool,
    /// Ou est le pointeur dans l'image entiere (pixels de la zone client) : pour la tuile et la mesure « pres de ».
    pub pointer: Option<(i32, i32)>,
}

pub struct Injector {
    pub mode: Mode,
    /// La sous-fenetre qui a recu l'appui : elle recoit le glisser jusqu'au relachement.
    drag: Option<u64>,
    /// La derniere sous-fenetre cliquee : le clavier y va si Live ne dit pas qui a le focus.
    key_target: u64,
    /// FOCUS : le geste en cours, rejoue au relachement.
    pending: Vec<Step>,
}

impl Default for Injector {
    fn default() -> Self {
        Injector { mode: Mode::Direct, drag: None, key_target: 0, pending: Vec::new() }
    }
}

impl Injector {
    pub fn set_mode(&mut self, m: Mode) {
        if m != self.mode {
            self.mode = m;
            self.drag = None;
            self.pending.clear();
        }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some() || !self.pending.is_empty()
    }

    /// Le geste, dans la fenetre `top` (deja verifiee par la session).
    #[cfg(windows)]
    pub fn handle(&mut self, top: u64, ev: &InputEvent) -> Result<Outcome, String> {
        win::check_alive(top)?;
        match ev {
            InputEvent::Pointer { action, pt, button, buttons, mods } => {
                let (cw, ch) = win::client_size(top);
                let p = map_point(pt, cw, ch);
                if self.mode == Mode::Focus {
                    return self.focus_pointer(top, *action, *button, *buttons, p);
                }
                // 21 oct. : un menu ouvert du plugin (une fenetre a part, popups.rs) passe avant le plugin dessous
                let (target, tp) = match (self.drag, action) {
                    (Some(t), PointerAction::Move | PointerAction::Up) if win::is_inside(top, t) || win::in_popup(top, t) => (t, win::map_to(top, t, p)),
                    _ => win::hit_with_popups(top, p),
                };
                let tp = win::to_window_dpi(target, top, tp);
                let msgs = pointer_msgs(*action, *button, *buttons, *mods, tp, win::wants_dblclk(target));
                for m in &msgs {
                    win::post(target, m)?;
                }
                match action {
                    PointerAction::Down | PointerAction::DblClick => {
                        self.drag = Some(target);
                        self.key_target = target;
                    }
                    PointerAction::Up if *buttons == 0 => self.drag = None,
                    _ => {}
                }
                Ok(Outcome { applied: true, pointer: Some(p) })
            }
            InputEvent::Wheel { pt, dx, dy, mods } => {
                let (cw, ch) = win::client_size(top);
                let p = map_point(pt, cw, ch);
                if self.mode == Mode::Focus {
                    win::focus_replay(top, &[Step::Wheel { p, dx: *dx, dy: *dy }])?;
                    return Ok(Outcome { applied: true, pointer: Some(p) });
                }
                let (target, _) = win::hit_with_popups(top, p);
                let screen = win::to_window_dpi(target, top, win::client_to_screen(top, p));
                for m in wheel_msgs(*dx, *dy, *mods, screen) {
                    win::post(target, &m)?;
                }
                Ok(Outcome { applied: true, pointer: Some(p) })
            }
            InputEvent::Key { down, key, code, mods } => {
                let Some((vk, ext)) = vk_of_code(code) else { return Err("this key is not sent to plugins".into()) };
                let target = win::key_target(top, self.key_target);
                let chars = if *down { chars_for_key(key, *mods) } else { Vec::new() };
                for m in key_msgs(*down, vk, win::scan_code(vk), ext, &chars, *mods) {
                    win::send(target, &m)?;
                }
                Ok(Outcome { applied: true, pointer: None })
            }
            InputEvent::Text { text } => {
                let target = win::key_target(top, self.key_target);
                for c in text.encode_utf16() {
                    win::send(target, &WinMsg { msg: WM_CHAR, wparam: c as usize, lparam: 1 })?;
                }
                Ok(Outcome { applied: true, pointer: None })
            }
        }
    }

    #[cfg(windows)]
    fn focus_pointer(&mut self, top: u64, action: PointerAction, button: u8, buttons: u8, p: (i32, i32)) -> Result<Outcome, String> {
        match action {
            PointerAction::Down | PointerAction::DblClick => {
                if self.pending.is_empty() {
                    self.pending.push(Step::Down { button, p });
                } else {
                    self.pending.push(Step::Down { button, p }); // le 2e appui d'un double clic
                }
                Ok(Outcome { applied: false, pointer: Some(p) })
            }
            PointerAction::Move => {
                if self.pending.is_empty() {
                    return Ok(Outcome { applied: false, pointer: Some(p) }); // le survol : ignore en FOCUS
                }
                self.pending.push(Step::Move { p });
                thin_steps(&mut self.pending);
                Ok(Outcome { applied: false, pointer: Some(p) })
            }
            PointerAction::Up => {
                if self.pending.is_empty() {
                    return Ok(Outcome { applied: false, pointer: Some(p) });
                }
                self.pending.push(Step::Up { button, p });
                if buttons != 0 {
                    return Ok(Outcome { applied: false, pointer: Some(p) });
                }
                // un double clic : le 1er clic arrive, le 2e suit en < 500 ms ; on attend un instant avant de rejouer
                let steps = std::mem::take(&mut self.pending);
                win::focus_replay(top, &steps)?;
                Ok(Outcome { applied: true, pointer: Some(p) })
            }
        }
    }

    #[cfg(not(windows))]
    pub fn handle(&mut self, _top: u64, _ev: &InputEvent) -> Result<Outcome, String> {
        Err("input needs Windows".into())
    }
}

#[cfg(windows)]
pub mod win {
    use super::{scale_point, wheel_delta, Step, WinMsg};
    use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{ClientToScreen, MapWindowPoints, MonitorFromPoint, MonitorFromWindow, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL};
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_MOUSE, MAPVK_VK_TO_VSC, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, MOUSE_EVENT_FLAGS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, ChildWindowFromPointEx, GetClassLongPtrW, GetClientRect, GetCursorPos, GetForegroundWindow, GetGUIThreadInfo, GetWindowRect,
        GetWindowThreadProcessId, IsChild, IsWindow, IsWindowVisible, PostMessageW, SendMessageTimeoutW, SetCursorPos, SetForegroundWindow, SetWindowPos,
        WindowFromPoint, CS_DBLCLKS, CWP_SKIPINVISIBLE, CWP_SKIPTRANSPARENT, GCL_STYLE, GUITHREADINFO, HWND_NOTOPMOST, HWND_TOPMOST, SMTO_ABORTIFHUNG,
        SMTO_NORMAL, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    };

    /// --describe (diagnostic, lecture seule) : la fenetre et ses sous-fenetres, avec leur taille, leur DPI et ce que
    /// le point central de chacune donne au test de « hit ».
    pub fn describe(top: u64) -> Vec<String> {
        use windows::Win32::UI::HiDpi::{GetAwarenessFromDpiAwarenessContext, GetWindowDpiAwarenessContext};
        use windows::Win32::UI::WindowsAndMessaging::{EnumChildWindows, GetClassNameW, GetParent};
        unsafe extern "system" fn collect(hwnd: HWND, lp: LPARAM) -> windows::core::BOOL {
            let v = &mut *(lp.0 as *mut Vec<u64>);
            v.push(hwnd.0 as isize as u64);
            windows::core::BOOL(1)
        }
        let mut all = vec![top];
        unsafe {
            let _ = EnumChildWindows(Some(h(top)), Some(collect), LPARAM(&mut all as *mut Vec<u64> as isize));
        }
        let mon = unsafe { MonitorFromWindow(h(top), MONITOR_DEFAULTTONEAREST) };
        let (mut mx, mut my) = (0u32, 0u32);
        unsafe {
            let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut mx, &mut my);
        }
        let mut out = vec![format!("monitor DPI {mx}")];
        for w in all {
            unsafe {
                let mut c = [0u16; 128];
                let n = GetClassNameW(h(w), &mut c).max(0) as usize;
                let mut wr = RECT::default();
                let _ = GetWindowRect(h(w), &mut wr);
                let (cw, ch) = client_size(w);
                let parent = GetParent(h(w)).map(|p| id(p)).unwrap_or(0);
                let aw = GetAwarenessFromDpiAwarenessContext(GetWindowDpiAwarenessContext(h(w))).0;
                let origin = client_to_screen(w, (0, 0));
                out.push(format!(
                    "{w:>10} parent {parent:>10} class {:?} client {cw}×{ch} at screen {:?} window {}×{} dpi {} awareness {aw} visible {} dblclk {}",
                    String::from_utf16_lossy(&c[..n]),
                    origin,
                    wr.right - wr.left,
                    wr.bottom - wr.top,
                    GetDpiForWindow(h(w)),
                    IsWindowVisible(h(w)).as_bool(),
                    wants_dblclk(w)
                ));
            }
        }
        out
    }

    pub fn h(id: u64) -> HWND {
        HWND(id as isize as *mut core::ffi::c_void)
    }
    fn id(h: HWND) -> u64 {
        h.0 as isize as u64
    }

    pub fn check_alive(top: u64) -> Result<(), String> {
        unsafe {
            if !IsWindow(Some(h(top))).as_bool() {
                return Err("the plugin window is gone".into());
            }
            if !IsWindowVisible(h(top)).as_bool() {
                return Err("the plugin window is hidden".into());
            }
        }
        Ok(())
    }

    pub fn pid_of(w: u64) -> u32 {
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(h(w), Some(&mut pid));
        }
        pid
    }

    pub fn client_size(w: u64) -> (i32, i32) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(h(w), &mut r);
        }
        (r.right - r.left, r.bottom - r.top)
    }

    pub fn is_inside(top: u64, w: u64) -> bool {
        unsafe { w == top || (IsWindow(Some(h(w))).as_bool() && IsChild(h(top), h(w)).as_bool()) }
    }

    /// Un point de la zone client de `from` → zone client de `to` (pixels physiques : le compagnon est « per monitor v2 »).
    pub fn map_to(from: u64, to: u64, p: (i32, i32)) -> (i32, i32) {
        if from == to {
            return p;
        }
        let mut pts = [POINT { x: p.0, y: p.1 }];
        unsafe {
            MapWindowPoints(Some(h(from)), Some(h(to)), &mut pts);
        }
        (pts[0].x, pts[0].y)
    }

    /// `w` est-il un menu ouvert du plugin `top` (ou une sous-fenetre d'un tel menu) ?
    pub fn in_popup(top: u64, w: u64) -> bool {
        crate::popups::list(top).iter().any(|p| p.id == w || unsafe { IsWindow(Some(h(w))).as_bool() && IsChild(h(p.id), h(w)).as_bool() })
    }

    /// Comme `hit`, mais un menu ouvert du plugin (popups.rs) sous le point passe devant : la sous-fenetre du menu, et
    /// le point dans sa zone client.
    pub fn hit_with_popups(top: u64, p: (i32, i32)) -> (u64, (i32, i32)) {
        let sp = client_to_screen(top, p);
        for pop in crate::popups::list(top) {
            let r = pop.rect;
            if sp.0 >= r.0 && sp.0 < r.2 && sp.1 >= r.1 && sp.1 < r.3 {
                let mut pt = POINT { x: sp.0, y: sp.1 };
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::ScreenToClient(h(pop.id), &mut pt);
                }
                return hit(pop.id, (pt.x, pt.y));
            }
        }
        hit(top, p)
    }

    /// La sous-fenetre visible la plus profonde sous le point, et le point dans sa zone client.
    pub fn hit(top: u64, p: (i32, i32)) -> (u64, (i32, i32)) {
        let mut cur = top;
        let mut cp = p;
        for _ in 0..16 {
            let child = unsafe { ChildWindowFromPointEx(h(cur), POINT { x: cp.0, y: cp.1 }, CWP_SKIPINVISIBLE | CWP_SKIPTRANSPARENT) };
            let c = id(child);
            if c == 0 || c == cur {
                break;
            }
            cp = map_to(cur, c, cp);
            cur = c;
        }
        (cur, cp)
    }

    pub fn client_to_screen(w: u64, p: (i32, i32)) -> (i32, i32) {
        let mut pt = POINT { x: p.0, y: p.1 };
        unsafe {
            let _ = ClientToScreen(h(w), &mut pt);
        }
        (pt.x, pt.y)
    }

    /// Les pixels de la fenetre visee. Par defaut les pixels physiques, sans conversion : mesure du 20 oct. (Diva,
    /// « DPI unaware », ecran a 125 %), les coordonnees logiques (÷ 1,25) tombaient en haut a gauche du curseur,
    /// les physiques tombent juste. UWC_DPI=logical remet la conversion (diagnostic, pour un plugin qui la voudrait).
    pub fn to_window_dpi(target: u64, top: u64, p: (i32, i32)) -> (i32, i32) {
        static LOGICAL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*LOGICAL.get_or_init(|| std::env::var("UWC_DPI").map(|v| v == "logical").unwrap_or(false)) {
            return p;
        }
        unsafe {
            let wdpi = GetDpiForWindow(h(target));
            let mon = MonitorFromWindow(h(top), MONITOR_DEFAULTTONEAREST);
            let (mut mx, mut my) = (0u32, 0u32);
            if GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut mx, &mut my).is_err() {
                return p;
            }
            scale_point(p, wdpi, mx)
        }
    }

    pub fn wants_dblclk(w: u64) -> bool {
        unsafe { (GetClassLongPtrW(h(w), GCL_STYLE) as u32) & CS_DBLCLKS.0 != 0 }
    }

    pub fn post(target: u64, m: &WinMsg) -> Result<(), String> {
        // UWC_MOUSE=send (diagnostic) : la souris aussi par SendMessageTimeout (sans passer par la boucle de Live)
        static SEND: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *SEND.get_or_init(|| std::env::var("UWC_MOUSE").map(|v| v == "send").unwrap_or(false)) {
            return send(target, m);
        }
        unsafe { PostMessageW(Some(h(target)), m.msg, WPARAM(m.wparam), LPARAM(m.lparam)).map_err(|e| format!("PostMessage: {e}")) }
    }

    /// Envoye directement a la fenetre (pas par la boucle de Live), 200 ms au plus si elle est occupee.
    pub fn send(target: u64, m: &WinMsg) -> Result<(), String> {
        let mut res = 0usize;
        let r = unsafe { SendMessageTimeoutW(h(target), m.msg, WPARAM(m.wparam), LPARAM(m.lparam), SMTO_NORMAL | SMTO_ABORTIFHUNG, 200, Some(&mut res)) };
        if r.0 == 0 {
            return Err("the plugin did not take the key (busy or hung)".into());
        }
        Ok(())
    }

    pub fn scan_code(vk: u16) -> u32 {
        unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) }
    }

    /// Qui recoit le clavier : la sous-fenetre qui a le focus dans le fil de Live si elle est dans le plugin, sinon
    /// la derniere cliquee, sinon la sous-fenetre au centre.
    pub fn key_target(top: u64, last: u64) -> u64 {
        // un menu ouvert prend le clavier (fleches, Entree, Echap pour le fermer) — seulement un menu POSSEDE par le
        // plugin (audit de publication : une fenetre sans proprietaire qui le chevauche ne recoit jamais de touches)
        if let Some(p) = crate::popups::list(top).iter().find(|p| p.owned) {
            return p.id;
        }
        unsafe {
            let tid = GetWindowThreadProcessId(h(top), None);
            let mut gi = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
            if GetGUIThreadInfo(tid, &mut gi).is_ok() {
                // une sous-fenetre du plugin (un champ de texte qui a pris le focus au clic) ; pas le cadre de Live
                // lui-meme, qui pourrait passer les touches aux raccourcis de Live
                let f = id(gi.hwndFocus);
                if f != 0 && f != top && is_inside(top, f) {
                    return f;
                }
            }
        }
        if last != 0 && is_inside(top, last) {
            return last;
        }
        let (cw, ch) = client_size(top);
        hit(top, (cw / 2, ch / 2)).0
    }

    // ── FOCUS : le vrai curseur et SendInput ────────────────────────────────────────────────────────────────
    fn mouse(flags: MOUSE_EVENT_FLAGS, data: i32) -> INPUT {
        INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
    }

    fn send_input(i: INPUT) -> Result<(), String> {
        let n = unsafe { SendInput(&[i], std::mem::size_of::<INPUT>() as i32) };
        if n != 1 {
            // (jamais « lancez le compagnon en administrateur » : la page appairee piloterait alors des fenetres elevees)
            return Err("SendInput was blocked: is Live running as administrator? Run Live normally (never run Live or the companion as administrator)".into());
        }
        Ok(())
    }

    fn buttons_held() -> bool {
        unsafe { [0x01, 0x02, 0x04].iter().any(|vk| (GetAsyncKeyState(*vk) as u16) & 0x8000 != 0) }
    }

    /// Mettre `w` au premier plan pour de bon (Windows le refuse a un programme qui n'a pas la main : on emprunte
    /// un instant la file d'entree du premier plan actuel).
    pub fn force_foreground(w: u64) {
        unsafe {
            let fg = GetForegroundWindow();
            if id(fg) == w {
                return;
            }
            let cur = GetCurrentThreadId();
            let ft = GetWindowThreadProcessId(fg, None);
            let attached = ft != 0 && ft != cur && AttachThreadInput(cur, ft, true).as_bool();
            let _ = SetForegroundWindow(h(w));
            let _ = BringWindowToTop(h(w));
            if attached {
                let _ = AttachThreadInput(cur, ft, false);
            }
        }
    }

    /// Rejouer un geste avec le vrai curseur, puis tout remettre. Effets visibles : la fenetre du plugin passe devant
    /// Chrome le temps du geste (quelques dizaines de ms), Live est active puis Chrome reprend la main.
    pub fn focus_replay(top: u64, steps: &[Step]) -> Result<(), String> {
        // le vrai bouton doit etre relache (sinon Windows donne la souris a Chrome)
        let t0 = std::time::Instant::now();
        while buttons_held() && t0.elapsed() < std::time::Duration::from_millis(600) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        unsafe {
            let saved_fg = GetForegroundWindow();
            let mut saved_cur = POINT::default();
            let _ = GetCursorPos(&mut saved_cur);
            let mut wr = RECT::default();
            let _ = GetWindowRect(h(top), &mut wr);
            // le point vise doit etre a l'ecran : sinon on amene la fenetre sous le curseur, le temps du geste
            let first = steps.iter().map(|s| match s { Step::Down { p, .. } | Step::Move { p } | Step::Up { p, .. } | Step::Wheel { p, .. } => *p }).next().unwrap_or((0, 0));
            let sp = client_to_screen(top, first);
            let moved = MonitorFromPoint(POINT { x: sp.0, y: sp.1 }, MONITOR_DEFAULTTONULL).is_invalid();
            if moved {
                let _ = SetWindowPos(h(top), None, wr.left + saved_cur.x - sp.0, wr.top + saved_cur.y - sp.1, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
            let _ = SetWindowPos(h(top), Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            // la zone client du plugin et ses menus, a l'ecran (apres l'eventuel deplacement) : un glisser n'en sort pas
            let (cw, ch) = client_size(top);
            let c0 = client_to_screen(top, (0, 0));
            let client = (c0.0, c0.1, c0.0 + cw.max(1), c0.1 + ch.max(1));
            let menus: Vec<(i32, i32, i32, i32)> = crate::popups::list(top).iter().map(|p| p.rect).collect();
            let mut result = Ok(());
            for s in steps {
                let p = match s { Step::Down { p, .. } | Step::Move { p } | Step::Up { p, .. } | Step::Wheel { p, .. } => *p };
                let mut sp = client_to_screen(top, p);
                if matches!(s, Step::Move { .. } | Step::Up { .. }) {
                    sp = super::keep_inside(sp, client, &menus);
                }
                let _ = SetCursorPos(sp.0, sp.1);
                // la securite : sous le point, la fenetre suivie et rien d'autre
                let under = id(WindowFromPoint(POINT { x: sp.0, y: sp.1 }));
                if matches!(s, Step::Down { .. } | Step::Wheel { .. }) && !is_inside(top, under) && !in_popup(top, under) {
                    result = Err("another window covers the plugin at that point: nothing clicked".to_string());
                    break;
                }
                let r = match s {
                    Step::Down { button, .. } => send_input(mouse(match button { 2 => MOUSEEVENTF_RIGHTDOWN, 1 => MOUSEEVENTF_MIDDLEDOWN, _ => MOUSEEVENTF_LEFTDOWN }, 0)),
                    Step::Up { button, .. } => send_input(mouse(match button { 2 => MOUSEEVENTF_RIGHTUP, 1 => MOUSEEVENTF_MIDDLEUP, _ => MOUSEEVENTF_LEFTUP }, 0)),
                    Step::Move { .. } => Ok(()),
                    Step::Wheel { dx, dy, .. } => {
                        let v = wheel_delta(*dy);
                        let hz = -wheel_delta(*dx);
                        let mut r = Ok(());
                        if v != 0 {
                            r = send_input(mouse(MOUSEEVENTF_WHEEL, v));
                        }
                        if hz != 0 && r.is_ok() {
                            r = send_input(mouse(MOUSEEVENTF_HWHEEL, hz));
                        }
                        r
                    }
                };
                if let Err(e) = r {
                    result = Err(e);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(if matches!(s, Step::Move { .. }) { 4 } else { 12 }));
            }
            // tout remettre
            std::thread::sleep(std::time::Duration::from_millis(30));
            let _ = SetWindowPos(h(top), Some(HWND_NOTOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            if moved {
                let _ = SetWindowPos(h(top), None, wr.left, wr.top, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
            let _ = SetCursorPos(saved_cur.x, saved_cur.y);
            if !saved_fg.is_invalid() {
                force_foreground(id(saved_fg));
            }
            result
        }
    }
}

/// Le mode direct contre une VRAIE fenetre Windows creee par le test (testwin.rs, hors de l'ecran) : ce qu'elle recoit.
#[cfg(all(test, windows))]
mod window_tests {
    use super::*;
    use crate::testwin::{self, KNOB_X, KNOB_Y};
    use std::time::{Duration, Instant};

    fn at(x: f64, y: f64) -> Pt {
        Pt { x, y, iw: Some(testwin::CLIENT_W as f64), ih: Some(testwin::CLIENT_H as f64) }
    }
    fn ptr(action: PointerAction, x: f64, y: f64, button: u8, buttons: u8) -> InputEvent {
        InputEvent::Pointer { action, pt: at(x, y), button, buttons, mods: 0 }
    }
    fn wait_for(h: u64, n: usize) -> Vec<testwin::Rec> {
        let t0 = Instant::now();
        loop {
            let r = testwin::received(h);
            if r.len() >= n || t0.elapsed() > Duration::from_secs(2) {
                return r;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn xy(l: isize) -> (i32, i32) {
        ((l as u32 & 0xffff) as u16 as i16 as i32, ((l as u32 >> 16) & 0xffff) as u16 as i16 as i32)
    }

    #[test]
    fn direct_mode_delivers_clicks_drags_wheel_and_keys_to_the_right_child() {
        let w = testwin::open((-6000, -6000)).expect("test window");
        let (top, knob) = (w.top, w.knob);
        let mut inj = Injector::default();
        // un clic dans le potard (sous-fenetre en 40,60) : il le recoit a SES coordonnees, avec un survol avant
        let (x, y) = ((KNOB_X + 100) as f64, (KNOB_Y + 50) as f64);
        inj.handle(top, &ptr(PointerAction::Down, x, y, 0, 1)).unwrap();
        // un glisser qui sort du potard (et de l'image) : la sous-fenetre qui a recu l'appui recoit tout
        inj.handle(top, &ptr(PointerAction::Move, x + 40.0, 20.0, 0, 1)).unwrap();
        inj.handle(top, &ptr(PointerAction::Move, x + 40.0, -120.0, 0, 1)).unwrap();
        inj.handle(top, &ptr(PointerAction::Up, x + 40.0, -120.0, 0, 0)).unwrap();
        let r = wait_for(knob, 5);
        let got: Vec<(u32, usize, (i32, i32))> = r.iter().map(|m| (m.msg, m.wparam, xy(m.lparam))).collect();
        assert_eq!(
            got,
            vec![
                (WM_MOUSEMOVE, 0, (100, 50)),
                (WM_LBUTTONDOWN, MK_LBUTTON, (100, 50)),
                (WM_MOUSEMOVE, MK_LBUTTON, (140, -40)),
                (WM_MOUSEMOVE, MK_LBUTTON, (140, -180)),
                (WM_LBUTTONUP, 0, (140, -180)),
            ]
        );
        assert!(testwin::received(top).is_empty(), "the parent got nothing: {:?}", testwin::received(top));
        // apres le relachement, un mouvement va a la fenetre sous le point (le parent, hors du potard)
        inj.handle(top, &ptr(PointerAction::Move, 10.0, 10.0, 0, 0)).unwrap();
        let r = wait_for(top, 1);
        assert_eq!((r[0].msg, xy(r[0].lparam)), (WM_MOUSEMOVE, (10, 10)));

        // double clic : la classe a CS_DBLCLKS → WM_LBUTTONDBLCLK
        inj.handle(top, &ptr(PointerAction::Down, x, y, 0, 1)).unwrap();
        inj.handle(top, &ptr(PointerAction::Up, x, y, 0, 0)).unwrap();
        inj.handle(top, &ptr(PointerAction::DblClick, x, y, 0, 1)).unwrap();
        inj.handle(top, &ptr(PointerAction::Up, x, y, 0, 0)).unwrap();
        // clic droit
        inj.handle(top, &ptr(PointerAction::Down, x, y, 2, 2)).unwrap();
        inj.handle(top, &ptr(PointerAction::Up, x, y, 2, 0)).unwrap();
        let r = wait_for(knob, 5 + 9);
        let msgs: Vec<u32> = r[5..].iter().map(|m| m.msg).collect();
        assert_eq!(
            msgs,
            vec![WM_MOUSEMOVE, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP]
        );
        assert_eq!(r[12].wparam, MK_RBUTTON);
        // (le clic droit a ouvert le « menu » de la fenetre d'essai : on le referme, la suite vise le potard)
        let t0 = Instant::now();
        while crate::popups::list(top).is_empty() && t0.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(5));
        }
        win::post(w.popup, &WinMsg { msg: WM_LBUTTONUP, wparam: 0, lparam: 0 }).unwrap();
        while !crate::popups::list(top).is_empty() && t0.elapsed() < Duration::from_secs(4) {
            std::thread::sleep(Duration::from_millis(5));
        }

        // molette : au potard, un cran vers le bas = -120, le point en coordonnees d'ecran
        let before = testwin::received(knob).len();
        inj.handle(top, &InputEvent::Wheel { pt: at(x, y), dx: 0.0, dy: 100.0, mods: 0 }).unwrap();
        let r = wait_for(knob, before + 1);
        let wm = r[before];
        assert_eq!(wm.msg, WM_MOUSEWHEEL);
        assert_eq!((wm.wparam >> 16) as u16 as i16, -120);
        assert_eq!(xy(wm.lparam), win::client_to_screen(top, (x as i32, y as i32)));

        // clavier : a la sous-fenetre cliquee en dernier (le potard), touche + caractere, puis une saisie composee
        let before = testwin::received(knob).len();
        inj.handle(top, &InputEvent::Key { down: true, key: "a".into(), code: "KeyA".into(), mods: 0 }).unwrap();
        inj.handle(top, &InputEvent::Key { down: false, key: "a".into(), code: "KeyA".into(), mods: 0 }).unwrap();
        inj.handle(top, &InputEvent::Text { text: "é!".into() }).unwrap();
        inj.handle(top, &InputEvent::Key { down: true, key: "Enter".into(), code: "Enter".into(), mods: 0 }).unwrap();
        let r = wait_for(knob, before + 6);
        let keys: Vec<(u32, usize)> = r[before..].iter().map(|m| (m.msg, m.wparam)).collect();
        assert_eq!(keys, vec![(WM_KEYDOWN, 0x41), (WM_CHAR, 'a' as usize), (WM_KEYUP, 0x41), (WM_CHAR, 0xE9), (WM_CHAR, '!' as usize), (WM_KEYDOWN, 0x0D), (WM_CHAR, 0x0D)]);
        assert!(inj.handle(top, &InputEvent::Key { down: true, key: "Fn".into(), code: "Fn".into(), mods: 0 }).is_err());

        // la mesure : recu → injecte, par geste (PostMessage / SendMessageTimeout)
        let n = 300;
        let t0 = Instant::now();
        let mut worst = Duration::ZERO;
        inj.handle(top, &ptr(PointerAction::Down, x, y, 0, 1)).unwrap();
        for i in 0..n {
            let t = Instant::now();
            inj.handle(top, &ptr(PointerAction::Move, x, y - (i % 50) as f64, 0, 1)).unwrap();
            worst = worst.max(t.elapsed());
        }
        inj.handle(top, &ptr(PointerAction::Up, x, y, 0, 0)).unwrap();
        let avg = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
        let t = Instant::now();
        inj.handle(top, &InputEvent::Key { down: true, key: "b".into(), code: "KeyB".into(), mods: 0 }).unwrap();
        let key_ms = t.elapsed().as_secs_f64() * 1000.0;
        println!("MEASURE injection: drag move avg {avg:.3} ms, worst {:.3} ms; key (SendMessageTimeout) {key_ms:.3} ms", worst.as_secs_f64() * 1000.0);
        assert!(avg < 5.0 && key_ms < 50.0);
        assert!(win::check_alive(top).is_ok());
        assert!(win::check_alive(0).is_err(), "no window, no input");
    }

    /// 21 oct. (Kontakt) : un menu dans sa propre fenetre (popup possedee par le plugin) est trouve, pose sur l'image
    /// du plugin, et un clic a sa place lui arrive (a SES coordonnees), pas au plugin dessous.
    #[test]
    fn a_plugin_menu_in_its_own_window_is_listed_captured_and_clicked() {
        // a l'ecran (Windows ne dessine pas une fenetre hors ecran) mais au fond, derriere les autres fenetres
        let w = testwin::open_with((80, 300), false, true).expect("test window");
        let top = w.top;
        let mut inj = Injector::default();
        assert!(crate::popups::list(top).is_empty(), "no menu yet");
        let (x, y) = ((KNOB_X + 200) as f64, (KNOB_Y + 100) as f64);
        inj.handle(top, &ptr(PointerAction::Down, x, y, 2, 2)).unwrap();
        inj.handle(top, &ptr(PointerAction::Up, x, y, 2, 0)).unwrap();
        let t0 = Instant::now();
        let mut pops = Vec::new();
        while pops.is_empty() && t0.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(5));
            pops = crate::popups::list(top);
        }
        assert_eq!(pops.iter().map(|p| p.id).collect::<Vec<_>>(), vec![w.popup], "the menu is found");
        std::thread::sleep(Duration::from_millis(100)); // dessine
        // l'image : le menu (vert) a sa place, le reste tel quel
        let f = crate::capture::Frame { w: testwin::CLIENT_W as u32, h: testwin::CLIENT_H as u32, bgra: vec![0; (testwin::CLIENT_W * testwin::CLIENT_H * 4) as usize] };
        let c = crate::popups::compose(top, f);
        let px = |x: i32, y: i32| {
            let i = ((y * testwin::CLIENT_W + x) * 4) as usize;
            (c.bgra[i], c.bgra[i + 1], c.bgra[i + 2])
        };
        assert_eq!(px(x as i32 + 20, y as i32 + 20), (0, 0xCC, 0), "the menu is drawn on the plugin image");
        assert_eq!(px(x as i32 - 20, y as i32 - 20), (0, 0, 0), "outside the menu: unchanged");
        // un clic dans le menu : au menu, a ses coordonnees ; le potard dessous ne recoit rien
        let knob_before = testwin::received(w.knob).len();
        inj.handle(top, &ptr(PointerAction::Down, x + 20.0, y + 30.0, 0, 1)).unwrap();
        inj.handle(top, &ptr(PointerAction::Up, x + 20.0, y + 30.0, 0, 0)).unwrap();
        let r = wait_for(w.popup, 3);
        let got: Vec<(u32, (i32, i32))> = r.iter().map(|m| (m.msg, xy(m.lparam))).collect();
        assert_eq!(got, vec![(WM_MOUSEMOVE, (20, 30)), (WM_LBUTTONDOWN, (20, 30)), (WM_LBUTTONUP, (20, 30))]);
        assert_eq!(testwin::received(w.knob).len(), knob_before, "nothing reached the plugin under the menu");
        let t0 = Instant::now();
        while !crate::popups::list(top).is_empty() && t0.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(crate::popups::list(top).is_empty(), "the menu closed itself");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64, iw: Option<f64>, ih: Option<f64>) -> Pt {
        Pt { x, y, iw, ih }
    }

    #[test]
    fn maps_image_pixels_to_client_pixels() {
        // 1:1 (le studio envoie la taille de l'image = la zone client)
        assert_eq!(map_point(&pt(10.0, 20.0, Some(1500.0), Some(837.0)), 1500, 837), (10, 20));
        assert_eq!(map_point(&pt(10.7, 20.2, None, None), 1500, 837), (10, 20));
        // une image reduite de moitie par le compagnon (trop grande) : le point est remis a l'echelle
        assert_eq!(map_point(&pt(100.0, 50.0, Some(1280.0), Some(800.0)), 2560, 1600), (200, 100));
        // la fenetre a change de taille depuis l'image : la proportion compte
        assert_eq!(map_point(&pt(400.0, 250.0, Some(800.0), Some(500.0)), 1000, 600), (500, 300));
        // un glisser qui sort de l'image
        assert_eq!(map_point(&pt(-50.0, 900.0, Some(800.0), Some(500.0)), 800, 500), (-50, 900));
        assert_eq!(map_point(&pt(-1e6, 1e6, None, None), 800, 500), (-32768, 32767));
    }

    #[test]
    fn focus_keeps_the_pointer_inside_the_plugin() {
        let client = (1000, 500, 1400, 800);
        let menu = [(1350, 700, 1600, 900)]; // un menu qui deborde a droite et en bas
        assert_eq!(keep_inside((1200, 600), client, &menu), (1200, 600));
        assert_eq!(keep_inside((1500, 850), client, &menu), (1500, 850), "inside an open menu");
        assert_eq!(keep_inside((2500, 600), client, &menu), (1399, 600), "a drag that leaves to the right stops at the edge");
        assert_eq!(keep_inside((900, 100), client, &[]), (1000, 500));
        assert_eq!(keep_inside((1200, 5000), client, &[]), (1200, 799));
        assert_eq!(keep_inside((5, 5), (0, 0, 0, 0), &[]), (0, 0), "an empty client area: its corner");
    }

    #[test]
    fn scales_for_dpi_virtualised_windows() {
        assert_eq!(scale_point((150, 300), 96, 144), (100, 200));
        assert_eq!(scale_point((150, 300), 144, 144), (150, 300));
        assert_eq!(scale_point((150, 300), 0, 144), (150, 300));
    }

    #[test]
    fn lparam_packs_signed_16_bits() {
        assert_eq!(make_lparam(10, 20), (20 << 16 | 10) as isize);
        let lp = make_lparam(-5, -7) as u32;
        assert_eq!((lp & 0xffff) as i16, -5);
        assert_eq!((lp >> 16) as i16, -7);
    }

    #[test]
    fn builds_click_drag_and_double_click_messages() {
        // appui gauche : survol puis WM_LBUTTONDOWN avec MK_LBUTTON
        let d = pointer_msgs(PointerAction::Down, 0, 1, 0, (5, 6), true);
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].msg, d[0].wparam), (WM_MOUSEMOVE, 0));
        assert_eq!((d[1].msg, d[1].wparam, d[1].lparam), (WM_LBUTTONDOWN, MK_LBUTTON, make_lparam(5, 6)));
        // glisser avec Maj : WM_MOUSEMOVE avec MK_LBUTTON | MK_SHIFT
        let m = pointer_msgs(PointerAction::Move, 0, 1, 1, (9, 9), true);
        assert_eq!((m[0].msg, m[0].wparam), (WM_MOUSEMOVE, MK_LBUTTON | MK_SHIFT));
        // relache : plus de bouton
        let u = pointer_msgs(PointerAction::Up, 0, 0, 0, (9, 9), true);
        assert_eq!((u[0].msg, u[0].wparam), (WM_LBUTTONUP, 0));
        // clic droit, avec Ctrl
        let r = pointer_msgs(PointerAction::Down, 2, 2, 2, (1, 1), true);
        assert_eq!((r[1].msg, r[1].wparam), (WM_RBUTTONDOWN, MK_RBUTTON | MK_CONTROL));
        assert_eq!(pointer_msgs(PointerAction::Up, 2, 0, 0, (1, 1), true)[0].msg, WM_RBUTTONUP);
        // milieu
        assert_eq!(pointer_msgs(PointerAction::Down, 1, 4, 0, (1, 1), true)[1].wparam, MK_MBUTTON);
        // double clic : WM_LBUTTONDBLCLK si la classe le demande, sinon un appui
        assert_eq!(pointer_msgs(PointerAction::DblClick, 0, 1, 0, (1, 1), true)[1].msg, WM_LBUTTONDBLCLK);
        assert_eq!(pointer_msgs(PointerAction::DblClick, 0, 1, 0, (1, 1), false)[1].msg, WM_LBUTTONDOWN);
        // un 2e bouton pendant un glisser garde le 1er
        assert_eq!(pointer_msgs(PointerAction::Down, 2, 3, 0, (1, 1), true)[1].wparam, MK_LBUTTON | MK_RBUTTON);
    }

    #[test]
    fn builds_wheel_messages() {
        let w = wheel_msgs(0.0, 100.0, 0, (300, 400));
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].msg, WM_MOUSEWHEEL);
        assert_eq!((w[0].wparam >> 16) as u16 as i16, -120, "down = negative");
        assert_eq!(w[0].lparam, make_lparam(300, 400));
        let up = wheel_msgs(0.0, -300.0, 2, (0, 0));
        assert_eq!((up[0].wparam >> 16) as u16 as i16, 360);
        assert_eq!(up[0].wparam & 0xffff, MK_CONTROL);
        let h = wheel_msgs(100.0, 0.0, 0, (0, 0));
        assert_eq!((h[0].msg, (h[0].wparam >> 16) as u16 as i16), (WM_MOUSEHWHEEL, 120));
        assert!(wheel_msgs(0.0, 0.0, 0, (0, 0)).is_empty());
        assert_eq!(wheel_delta(33.3), -40, "a trackpad's small steps keep their size");
    }

    #[test]
    fn maps_keys() {
        assert_eq!(vk_of_code("KeyA"), Some((0x41, false)));
        assert_eq!(vk_of_code("Digit7"), Some((0x37, false)));
        assert_eq!(vk_of_code("Numpad3"), Some((0x63, false)));
        assert_eq!(vk_of_code("F5"), Some((0x74, false)));
        assert_eq!(vk_of_code("ArrowDown"), Some((0x28, true)));
        assert_eq!(vk_of_code("Enter"), Some((0x0D, false)));
        assert_eq!(vk_of_code("NumpadEnter"), Some((0x0D, true)));
        assert_eq!(vk_of_code("Fn"), None);
        assert_eq!(vk_of_code("F99"), None);
        assert_eq!(vk_of_code(""), None);
        // les caracteres
        assert_eq!(chars_for_key("a", 0), vec![b'a' as u16]);
        assert_eq!(chars_for_key("A", 1), vec![b'A' as u16]);
        assert_eq!(chars_for_key("é", 0), vec![0xE9]);
        assert_eq!(chars_for_key("@", 2 | 4), vec![b'@' as u16], "AltGr = Ctrl+Alt");
        assert!(chars_for_key("a", 2).is_empty(), "Ctrl+A is a shortcut, not a letter");
        assert_eq!(chars_for_key("Enter", 0), vec![0x0D]);
        assert_eq!(chars_for_key("Backspace", 0), vec![0x08]);
        assert!(chars_for_key("ArrowLeft", 0).is_empty());
        assert!(chars_for_key("Shift", 1).is_empty());
    }

    #[test]
    fn builds_key_messages() {
        let d = key_msgs(true, 0x41, 0x1E, false, &[b'a' as u16], 0);
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].msg, d[0].wparam), (WM_KEYDOWN, 0x41));
        assert_eq!(d[0].lparam as u32, 1 | (0x1E << 16));
        assert_eq!((d[1].msg, d[1].wparam), (WM_CHAR, b'a' as usize));
        let u = key_msgs(false, 0x41, 0x1E, false, &[], 0);
        assert_eq!(u[0].msg, WM_KEYUP);
        assert_eq!(u[0].lparam as u32 & 0xC000_0000, 0xC000_0000);
        let e = key_msgs(true, 0x28, 0x50, true, &[], 0);
        assert_eq!(e[0].lparam as u32 & (1 << 24), 1 << 24, "extended key");
        assert_eq!(key_msgs(true, 0x46, 0x21, false, &[], 4)[0].msg, WM_SYSKEYDOWN, "Alt+F");
        assert_eq!(key_msgs(false, 0x46, 0x21, false, &[], 4)[0].msg, WM_SYSKEYUP);
    }

    #[test]
    fn modes_and_long_gestures() {
        assert_eq!(Mode::parse("direct"), Some(Mode::Direct));
        assert_eq!(Mode::parse("focus").map(|m| m.name()), Some("focus"));
        assert_eq!(Mode::parse("god"), None);
        let mut s = vec![Step::Down { button: 0, p: (0, 0) }];
        for i in 0..400 {
            s.push(Step::Move { p: (i, i) });
        }
        s.push(Step::Up { button: 0, p: (399, 399) });
        thin_steps(&mut s);
        assert!(s.len() <= MAX_STEPS + 2, "{}", s.len());
        assert_eq!(s[0], Step::Down { button: 0, p: (0, 0) });
        assert_eq!(s.last(), Some(&Step::Up { button: 0, p: (399, 399) }));
    }
}
