// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Les menus et listes deroulantes d'un plugin (21 oct., Kontakt) : beaucoup de plugins (Qt, JUCE…) ouvrent leurs
//! menus dans des fenetres « popup » A PART, hors de la fenetre du plugin. Sans elles, le studio montrait le plugin sans
//! son menu ouvert, et un clic dans le menu tombait sur le plugin dessous.
//! Ici : les trouver (`list`), les poser sur l'image du plugin (`compose`) et y envoyer les gestes (input.rs).
//! Securite : seulement des fenetres du MEME fil que la fenetre suivie (le fil d'interface de Live, ou vit le plugin),
//! visibles, qui sont soit possedees (GW_OWNER) par la fenetre suivie, soit des popups « outil » ou « au-dessus »
//! qui la chevauchent ; jamais la fenetre principale de Live ni une autre fenetre de plugin.
//! Audit de publication : jamais non plus une vraie fenetre que le plugin ouvrirait (une boite de dialogue #32770 —
//! ouvrir ou enregistrer un preset, un message —, une fenetre a barre de titre, une fenetre qui contient une vue de
//! l'Explorateur), ni ce qui en depend (le menu contextuel d'une boite « Ouvrir ») : le studio ne doit pas pouvoir
//! parcourir des dossiers ou des fichiers par la. Le clavier ne va qu'a un menu possede par le plugin.

use crate::capture::Frame;

/// Ce qu'on sait d'une fenetre de premier niveau du fil du plugin.
#[derive(Debug, Clone, PartialEq)]
pub struct Cand {
    pub id: u64,
    pub visible: bool,
    /// la chaine des proprietaires (GW_OWNER), du plus proche au plus lointain
    pub owners: Vec<u64>,
    pub popup_style: bool,
    pub tool_or_topmost: bool,
    /// une barre de titre (WS_CAPTION) : une vraie fenetre, pas un menu
    pub caption: bool,
    pub class: String,
    pub title: String,
    /// rectangle a l'ecran (pixels physiques) : gauche, haut, droite, bas
    pub rect: (i32, i32, i32, i32),
}

/// Une fenetre popup du plugin, a l'ecran.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Popup {
    pub id: u64,
    pub rect: (i32, i32, i32, i32),
    /// possedee (GW_OWNER) par la fenetre du plugin : la seule sorte qui recoit le clavier
    pub owned: bool,
}

pub const LIVE_MAIN_CLASS: &str = "Ableton Live Window Class";
/// La classe des boites de dialogue de Windows (dont « Ouvrir » / « Enregistrer sous », les messages).
pub const DIALOG_CLASS: &str = "#32770";

/// Une vraie fenetre, jamais un menu : une boite de dialogue, ou une fenetre a barre de titre.
pub fn is_window_like(c: &Cand) -> bool {
    c.class == DIALOG_CLASS || c.caption
}
/// Au plus quelques menus (un sous-menu dans un menu…).
pub const MAX_POPUPS: usize = 6;

fn overlaps(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> bool {
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}

/// La regle : `c` est-il un menu du plugin `top` (dont la fenetre est en `top_rect`) ?
pub fn is_plugin_popup(c: &Cand, top: u64, top_rect: (i32, i32, i32, i32)) -> bool {
    if c.id == top || !c.visible || c.rect.2 <= c.rect.0 || c.rect.3 <= c.rect.1 {
        return false;
    }
    if c.class == LIVE_MAIN_CLASS || crate::plugins::split_title(&c.title).is_some() {
        return false; // Live lui-meme, ou une autre fenetre de plugin (« Diva/2-Diva »)
    }
    if is_window_like(c) {
        return false; // une boite de dialogue ou une fenetre a barre de titre : pas un menu
    }
    if c.owners.contains(&top) {
        return true;
    }
    c.popup_style && c.tool_or_topmost && c.owners.is_empty() && overlaps(c.rect, top_rect)
}

/// Les candidats retenus, dans l'ordre de la pile des fenetres (le plus haut d'abord). Une fenetre dont la chaine des
/// proprietaires passe, avant le plugin, par une vraie fenetre (le menu contextuel d'une boite « Ouvrir ») est ecartee.
pub fn select(cands: &[Cand], top: u64, top_rect: (i32, i32, i32, i32)) -> Vec<Popup> {
    let windows: Vec<u64> = cands.iter().filter(|c| is_window_like(c)).map(|c| c.id).collect();
    cands
        .iter()
        .filter(|c| is_plugin_popup(c, top, top_rect))
        .filter(|c| !c.owners.iter().take_while(|o| **o != top).any(|o| windows.contains(o)))
        .take(MAX_POPUPS)
        .map(|c| Popup { id: c.id, rect: c.rect, owned: c.owners.contains(&top) })
        .collect()
}

/// Colle `src` dans `dst` en (x, y) (pixels de `dst`), sans deborder.
pub fn paste(dst: &mut Frame, src: &Frame, x: i32, y: i32) {
    for sy in 0..src.h as i32 {
        let dy = y + sy;
        if dy < 0 || dy >= dst.h as i32 {
            continue;
        }
        let x0 = x.max(0);
        let x1 = (x + src.w as i32).min(dst.w as i32);
        if x1 <= x0 {
            return;
        }
        let s = ((sy as u32 * src.w + (x0 - x) as u32) * 4) as usize;
        let d = ((dy as u32 * dst.w + x0 as u32) * 4) as usize;
        let n = ((x1 - x0) * 4) as usize;
        dst.bgra[d..d + n].copy_from_slice(&src.bgra[s..s + n]);
    }
}

/// Redimensionne (au plus proche voisin) : un menu dessine a une autre echelle que l'image du plugin.
pub fn resize(src: &Frame, w: u32, h: u32) -> Frame {
    if (w, h) == (src.w, src.h) || w == 0 || h == 0 {
        return Frame { w: src.w, h: src.h, bgra: src.bgra.clone() };
    }
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let sy = ((y as u64 * src.h as u64) / h as u64) as u32;
        for x in 0..w {
            let sx = ((x as u64 * src.w as u64) / w as u64) as u32;
            let i = ((sy * src.w + sx) * 4) as usize;
            out.extend_from_slice(&src.bgra[i..i + 4]);
        }
    }
    Frame { w, h, bgra: out }
}

/// Ou poser un menu sur l'image : son coin a l'ecran moins l'origine de la zone client, a l'echelle de l'image
/// (`scale` = largeur de l'image / largeur physique de la zone client : 0,8 pour un plugin « DPI unaware » a 125 %).
pub fn place(popup: (i32, i32, i32, i32), client_origin: (i32, i32), scale: f64) -> (i32, i32, u32, u32) {
    let x = ((popup.0 - client_origin.0) as f64 * scale).round() as i32;
    let y = ((popup.1 - client_origin.1) as f64 * scale).round() as i32;
    let w = (((popup.2 - popup.0) as f64) * scale).round().max(1.0) as u32;
    let h = (((popup.3 - popup.1) as f64) * scale).round().max(1.0) as u32;
    (x, y, w, h)
}

#[cfg(windows)]
pub use win::{compose, list};

#[cfg(not(windows))]
pub fn list(_top: u64) -> Vec<Popup> {
    Vec::new()
}
#[cfg(not(windows))]
pub fn compose(_top: u64, f: Frame) -> Frame {
    f
}

#[cfg(windows)]
mod win {
    use super::{paste, place, resize, select, Cand, Popup};
    use crate::capture::Frame;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
    use windows::Win32::Graphics::Gdi::{
        ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
        BI_RGB, DIB_RGB_COLORS,
    };
    use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
    const PW_RENDERFULLCONTENT: u32 = 0x0000_0002;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, EnumWindows, GetClassNameW, GetClientRect, GetWindow, GetWindowLongW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
        GWL_EXSTYLE, GWL_STYLE, GW_OWNER, WS_CAPTION, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    };

    fn h(id: u64) -> HWND {
        HWND(id as isize as *mut core::ffi::c_void)
    }
    fn id(w: HWND) -> u64 {
        w.0 as isize as u64
    }
    fn rect_of(w: HWND) -> (i32, i32, i32, i32) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetWindowRect(w, &mut r);
        }
        (r.left, r.top, r.right, r.bottom)
    }

    /// Les fenetres de premier niveau du fil de `top`, dans l'ordre de la pile (EnumWindows : du haut vers le bas).
    fn candidates(top: u64) -> Vec<Cand> {
        struct Ctx {
            tid: u32,
            out: Vec<HWND>,
        }
        unsafe extern "system" fn each(w: HWND, lp: LPARAM) -> BOOL {
            let c = &mut *(lp.0 as *mut Ctx);
            if GetWindowThreadProcessId(w, None) == c.tid {
                c.out.push(w);
            }
            BOOL(1)
        }
        let tid = unsafe { GetWindowThreadProcessId(h(top), None) };
        if tid == 0 {
            return Vec::new();
        }
        let mut ctx = Ctx { tid, out: Vec::new() };
        unsafe {
            let _ = EnumWindows(Some(each), LPARAM(&mut ctx as *mut Ctx as isize));
        }
        ctx.out
            .into_iter()
            .filter(|w| unsafe { IsWindowVisible(*w).as_bool() })
            .map(|w| unsafe {
                let mut owners = Vec::new();
                let mut o = GetWindow(w, GW_OWNER).unwrap_or_default();
                while !o.is_invalid() && owners.len() < 8 {
                    owners.push(id(o));
                    o = GetWindow(o, GW_OWNER).unwrap_or_default();
                }
                let style = GetWindowLongW(w, GWL_STYLE) as u32;
                let ex = GetWindowLongW(w, GWL_EXSTYLE) as u32;
                let mut cls = [0u16; 128];
                let n = GetClassNameW(w, &mut cls).max(0) as usize;
                let mut t = [0u16; 256];
                let tn = GetWindowTextW(w, &mut t).max(0) as usize;
                Cand {
                    id: id(w),
                    visible: true,
                    owners,
                    popup_style: style & WS_POPUP.0 != 0,
                    tool_or_topmost: ex & (WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0) != 0,
                    caption: style & WS_CAPTION.0 == WS_CAPTION.0,
                    class: String::from_utf16_lossy(&cls[..n]),
                    title: String::from_utf16_lossy(&t[..tn]),
                    rect: rect_of(w),
                }
            })
            .collect()
    }

    /// Une vue de l'Explorateur dans la fenetre (une boite « Ouvrir » faite a la main, un navigateur de fichiers) ?
    fn hosts_shell_view(w: HWND) -> bool {
        unsafe extern "system" fn each(c: HWND, lp: LPARAM) -> BOOL {
            let found = &mut *(lp.0 as *mut bool);
            let mut cls = [0u16; 64];
            let n = GetClassNameW(c, &mut cls).max(0) as usize;
            let name = String::from_utf16_lossy(&cls[..n]);
            if name == "SHELLDLL_DefView" || name == "DirectUIHWND" {
                *found = true;
                return BOOL(0);
            }
            BOOL(1)
        }
        let mut found = false;
        unsafe {
            let _ = EnumChildWindows(Some(w), Some(each), LPARAM(&mut found as *mut bool as isize));
        }
        found
    }

    /// Les menus ouverts du plugin `top`, le plus haut d'abord.
    pub fn list(top: u64) -> Vec<Popup> {
        select(&candidates(top), top, rect_of(h(top))).into_iter().filter(|p| !hosts_shell_view(h(p.id))).collect()
    }

    /// L'image d'un menu (PrintWindow, toute la fenetre), recadree a sa taille logique si elle est « DPI unaware ».
    fn grab(p: &Popup) -> Option<Frame> {
        let (w, hh) = (p.rect.2 - p.rect.0, p.rect.3 - p.rect.1);
        if w <= 0 || hh <= 0 || w > 4096 || hh > 4096 {
            return None;
        }
        unsafe {
            let wnd = h(p.id);
            let hdc = GetDC(Some(wnd));
            let mem = CreateCompatibleDC(Some(hdc));
            let bmp = CreateCompatibleBitmap(hdc, w, hh);
            let old = SelectObject(mem, bmp.into());
            let ok = PrintWindow(wnd, mem, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();
            let mut bi = BITMAPINFO::default();
            bi.bmiHeader = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -hh, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() };
            let mut buf = vec![0u8; (w * hh * 4) as usize];
            let lines = GetDIBits(mem, bmp, 0, hh as u32, Some(buf.as_mut_ptr() as *mut core::ffi::c_void), &mut bi, DIB_RGB_COLORS);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem);
            ReleaseDC(Some(wnd), hdc);
            if !ok || lines == 0 {
                return None;
            }
            for px in buf.chunks_exact_mut(4) {
                px[3] = 255;
            }
            let f = Frame { w: w as u32, h: hh as u32, bgra: buf };
            let (wd, md) = crate::capture::dpis_of(p.id);
            let (lw, lh) = crate::capture::logical_size(w as u32, hh as u32, wd, md);
            Some(if (lw, lh) != (w as u32, hh as u32) { crate::stream::crop(&f, (0, 0, lw, lh)) } else { f })
        }
    }

    /// L'image du plugin, avec ses menus ouverts poses dessus (rien a faire sans menu : l'image revient telle quelle).
    pub fn compose(top: u64, mut frame: Frame) -> Frame {
        let pops = list(top);
        if pops.is_empty() {
            return frame;
        }
        let mut cr = RECT::default();
        let mut o = POINT { x: 0, y: 0 };
        unsafe {
            let _ = GetClientRect(h(top), &mut cr);
            let _ = ClientToScreen(h(top), &mut o);
        }
        let cw = (cr.right - cr.left).max(1) as f64;
        let scale = frame.w as f64 / cw;
        // du plus bas au plus haut : le plus haut finit dessus
        for p in pops.iter().rev() {
            if let Some(img) = grab(p) {
                let (x, y, w, hh) = place(p.rect, (o.x, o.y), scale);
                let img = resize(&img, w, hh);
                paste(&mut frame, &img, x, y);
            }
        }
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: u64, owners: Vec<u64>, popup: bool, tool: bool, class: &str, title: &str, rect: (i32, i32, i32, i32)) -> Cand {
        Cand { id, visible: true, owners, popup_style: popup, tool_or_topmost: tool, caption: false, class: class.into(), title: title.into(), rect }
    }

    #[test]
    fn only_the_plugin_menus_are_followed() {
        let top = 100;
        let tr = (1000, 900, 2200, 1700);
        let cands = vec![
            cand(1, vec![top], true, true, "Qt661QWindowPopupDropShadowSaveBits", "", (1100, 950, 1300, 1200)), // un menu Qt, possede : oui
            cand(2, vec![], true, true, "JUCE_18a", "", (2100, 1600, 2400, 1900)),                                // un menu JUCE qui deborde : oui
            cand(3, vec![], true, true, "JUCE_18b", "", (10, 10, 200, 200)),                                      // ailleurs a l'ecran : non
            cand(4, vec![], false, false, LIVE_MAIN_CLASS, "Untitled - Ableton Live 12 Suite", (0, 0, 1920, 1040)), // Live : non
            cand(5, vec![999], true, true, "Vst3PlugWindow", "Diva/2-Diva", tr),                                  // un autre plugin : non
            cand(6, vec![999], true, true, "Qt661QWindowToolSaveBits", "", (1100, 950, 1300, 1200)),              // possede par Live : non
            cand(top, vec![999], true, true, "Vst3PlugWindow", "Kontakt 8/1-Kontakt 8", tr),                       // lui-meme : non
            cand(7, vec![], true, false, "Plain", "", (1100, 950, 1300, 1200)),                                    // ni outil ni au-dessus : non
        ];
        let got: Vec<u64> = select(&cands, top, tr).iter().map(|p| p.id).collect();
        assert_eq!(got, vec![1, 2]);
        let owned: Vec<bool> = select(&cands, top, tr).iter().map(|p| p.owned).collect();
        assert_eq!(owned, vec![true, false], "only an owned menu gets the keyboard");
        let mut hidden = cands[0].clone();
        hidden.visible = false;
        assert!(!is_plugin_popup(&hidden, top, tr));
    }

    /// (audit de publication) une boite « Ouvrir » / « Enregistrer » que le plugin ouvre est possedee par lui, mais ce
    /// n'est pas un menu : ni filmee ni cliquable depuis le studio, pas plus que son menu contextuel ; une fenetre a
    /// barre de titre non plus. Un vrai menu possede reste suivi.
    #[test]
    fn dialogs_and_titled_windows_of_the_plugin_are_never_menus() {
        let top = 100;
        let tr = (1000, 900, 2200, 1700);
        let mut dialog = cand(10, vec![top], true, false, DIALOG_CLASS, "Open preset", (1200, 1000, 1900, 1500));
        dialog.caption = true;
        let shell_menu = cand(11, vec![10, top], true, true, "#32768", "", (1300, 1100, 1500, 1300)); // son menu contextuel
        let mut titled = cand(12, vec![top], true, true, "JUCE_1a2", "Settings", (1100, 950, 1600, 1300));
        titled.caption = true;
        let bare_dialog = cand(13, vec![top], true, true, DIALOG_CLASS, "", (1100, 950, 1600, 1300)); // sans titre : non plus
        let menu = cand(14, vec![top], true, true, "#32768", "", (1100, 950, 1300, 1200)); // un menu Windows du plugin
        let cands = vec![shell_menu, dialog, titled, bare_dialog, menu];
        let got: Vec<u64> = select(&cands, top, tr).iter().map(|p| p.id).collect();
        assert_eq!(got, vec![14]);
        assert!(is_window_like(&cands[1]) && is_window_like(&cands[2]) && is_window_like(&cands[3]) && !is_window_like(&cands[4]));
    }

    #[test]
    fn menus_land_at_their_place_on_the_image() {
        // un plugin « DPI unaware » a 125 % : l'image est a 0,8 de la zone client physique
        assert_eq!(place((1100, 1000, 1350, 1250), (1000, 900), 0.8), (80, 80, 200, 200));
        assert_eq!(place((1100, 1000, 1350, 1250), (1000, 900), 1.0), (100, 100, 250, 250));
        let mut dst = Frame { w: 10, h: 6, bgra: vec![0; 10 * 6 * 4] };
        let src = Frame { w: 4, h: 3, bgra: vec![200; 4 * 3 * 4] };
        paste(&mut dst, &src, 8, 4); // deborde a droite et en bas : coupe
        let px = |f: &Frame, x: u32, y: u32| f.bgra[((y * f.w + x) * 4) as usize];
        assert_eq!((px(&dst, 8, 4), px(&dst, 9, 5), px(&dst, 7, 4), px(&dst, 8, 3)), (200, 200, 0, 0));
        paste(&mut dst, &src, -2, -1); // deborde a gauche et en haut
        assert_eq!((px(&dst, 0, 0), px(&dst, 1, 1), px(&dst, 2, 0)), (200, 200, 0));
        paste(&mut dst, &src, 50, 50); // hors de l'image : rien
        let r = resize(&src, 2, 6);
        assert_eq!((r.w, r.h, r.bgra.len()), (2, 6, 2 * 6 * 4));
    }
}
