// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! macOS (26 oct. 2026) : ce que le compagnon demande au systeme, par les API C d'Apple (CoreGraphics,
//! CoreFoundation, ApplicationServices), sans crate de plus.
//!
//! - La liste des fenetres : CGWindowListCopyWindowInfo. Le TITRE d'une fenetre d'une autre appli n'est donne qu'avec
//!   l'autorisation « Enregistrement de l'ecran » (Reglages Systeme › Confidentialite et securite) : sans elle, la
//!   liste des fenetres de Live est vide (--list le dit).
//! - La capture d'UNE fenetre : CGWindowListCreateImage(kCGWindowListOptionIncludingWindow, id), cherchee a
//!   l'execution (dlsym) : Apple l'a declaree obsolete (macOS 14) au profit de ScreenCaptureKit ; si un macOS la
//!   retire, l'ouverture echoue proprement. Meme autorisation. L'image est en pixels (Retina : 2× les points).
//!   TODO : ScreenCaptureKit (SCStream sur un SCContentFilter d'une fenetre) — du Objective-C asynchrone (blocs) :
//!   images a la cadence du systeme, sans copie par appel ; a faire quand CGWindowListCreateImage disparaitra.
//! - Les gestes : CGEventPostToPid (10.11+), l'evenement souris en coordonnees globales de l'ecran (points, origine
//!   en haut a gauche de l'ecran principal, comme kCGWindowBounds). Il faut l'autorisation « Accessibilite ».
//!   (a tester) : un plugin dans une fenetre en arriere-plan accepte-t-il les clics postes a Live sans que Live passe
//!   devant ? AppKit route le clic a la fenetre sous le point ; certains plugins lisent la position globale du curseur
//!   (comme le mode FOCUS de Windows) : pas encore de mode FOCUS sur Mac.
//! - Les autorisations : CGPreflightScreenCaptureAccess et AXIsProcessTrusted les LISENT sans rien demander (pas de
//!   fenetre). Le compagnon ne les demande jamais lui-meme : la premiere capture fait apparaitre la demande de macOS.

#![allow(non_upper_case_globals, non_snake_case)]

use std::ffi::{c_char, c_void};

pub type CFTypeRef = *const c_void;
pub type CFArrayRef = *const c_void;
pub type CFDictionaryRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFNumberRef = *const c_void;
pub type CFDataRef = *const c_void;
pub type CGImageRef = *mut c_void;
pub type CGContextRef = *mut c_void;
pub type CGColorSpaceRef = *mut c_void;
pub type CGEventRef = *mut c_void;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}

const kCFNumberSInt64Type: i32 = 4;
const kCFStringEncodingUTF8: u32 = 0x0800_0100;

pub const kCGWindowListOptionAll: u32 = 0;
pub const kCGWindowListOptionIncludingWindow: u32 = 1 << 3;
pub const kCGWindowListExcludeDesktopElements: u32 = 1 << 4;
pub const kCGWindowImageBoundsIgnoreFraming: u32 = 1 << 0;
pub const kCGWindowImageBestResolution: u32 = 1 << 3;
const kCGImageAlphaPremultipliedFirst: u32 = 2;
const kCGBitmapByteOrder32Little: u32 = 2 << 12;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayGetCount(a: CFArrayRef) -> isize;
    fn CFArrayGetValueAtIndex(a: CFArrayRef, i: isize) -> *const c_void;
    fn CFDictionaryGetValue(d: CFDictionaryRef, key: *const c_void) -> *const c_void;
    fn CFNumberGetValue(n: CFNumberRef, ty: i32, out: *mut c_void) -> bool;
    fn CFStringGetLength(s: CFStringRef) -> isize;
    fn CFStringGetCString(s: CFStringRef, buf: *mut c_char, size: isize, enc: u32) -> bool;
    fn CFBooleanGetValue(b: *const c_void) -> bool;
    fn CFRelease(o: CFTypeRef);
    fn CFStringCreateWithCString(alloc: *const c_void, s: *const c_char, enc: u32) -> CFStringRef;
    fn CFGetTypeID(o: CFTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFNumberCreate(alloc: *const c_void, ty: i32, v: *const c_void) -> CFNumberRef;
    fn CFRetain(o: CFTypeRef) -> CFTypeRef;
}
const kCFNumberFloat64Type: i32 = 6;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    static kCGWindowNumber: CFStringRef;
    static kCGWindowOwnerPID: CFStringRef;
    static kCGWindowOwnerName: CFStringRef;
    static kCGWindowName: CFStringRef;
    static kCGWindowBounds: CFStringRef;
    static kCGWindowLayer: CFStringRef;
    static kCGWindowIsOnscreen: CFStringRef;
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFArrayRef;
    fn CGRectMakeWithDictionaryRepresentation(d: CFDictionaryRef, rect: *mut CGRect) -> bool;
    fn CGImageGetWidth(i: CGImageRef) -> usize;
    fn CGImageGetHeight(i: CGImageRef) -> usize;
    fn CGImageRelease(i: CGImageRef);
    fn CGColorSpaceCreateDeviceRGB() -> CGColorSpaceRef;
    fn CGColorSpaceRelease(c: CGColorSpaceRef);
    fn CGBitmapContextCreate(data: *mut c_void, w: usize, h: usize, bpc: usize, bpr: usize, cs: CGColorSpaceRef, info: u32) -> CGContextRef;
    fn CGContextDrawImage(c: CGContextRef, r: CGRect, i: CGImageRef);
    fn CGContextRelease(c: CGContextRef);
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGEventCreateMouseEvent(source: *const c_void, ty: u32, at: CGPoint, button: u32) -> CGEventRef;
    fn CGEventCreateScrollWheelEvent2(source: *const c_void, units: u32, count: u32, w1: i32, w2: i32, w3: i32) -> CGEventRef;
    fn CGEventCreateKeyboardEvent(source: *const c_void, key: u16, down: bool) -> CGEventRef;
    fn CGEventKeyboardSetUnicodeString(e: CGEventRef, len: usize, s: *const u16);
    fn CGEventSetFlags(e: CGEventRef, flags: u64);
    fn CGEventSetIntegerValueField(e: CGEventRef, field: u32, v: i64);
    fn CGEventSetLocation(e: CGEventRef, at: CGPoint);
    fn CGEventPostToPid(pid: i32, e: CGEventRef);
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    // (11 nov.) les fenetres d'une appli par l'Accessibilite : Logic Pro n'en donne les titres qu'a elle (ax_windows)
    fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    fn AXUIElementCreateSystemWide() -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(e: CFTypeRef, attr: CFStringRef, out: *mut CFTypeRef) -> i32;
    fn AXUIElementSetMessagingTimeout(e: CFTypeRef, seconds: f32) -> i32;
    fn AXValueGetValue(v: CFTypeRef, ty: u32, out: *mut c_void) -> bool;
    // (11 nov.) les gestes de Logic par l'Accessibilite (input.rs) : un clic poste a Logic en arriere-plan est ignore
    fn AXUIElementCopyElementAtPosition(app: CFTypeRef, x: f32, y: f32, out: *mut CFTypeRef) -> i32;
    fn AXUIElementPerformAction(e: CFTypeRef, action: CFStringRef) -> i32;
    fn AXUIElementSetAttributeValue(e: CFTypeRef, attr: CFStringRef, value: CFTypeRef) -> i32;
    fn AXUIElementIsAttributeSettable(e: CFTypeRef, attr: CFStringRef, out: *mut u8) -> i32;
}
const kAXValueCGPointType: u32 = 1;
const kAXValueCGSizeType: u32 = 2;

extern "C" {
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    // libproc (libSystem) : le chemin de l'executable d'un processus (la meme signature que dans install.rs)
    fn proc_pidpath(pid: i32, buf: *mut u8, size: u32) -> i32;
}

/// (9 nov.) L'appli « responsable » d'un processus, et le nom de son executable (« Logic Pro ») : pour un service XPC
/// lance pour une appli (les Audio Units que Logic Pro 12 fait tourner dans AUHostingServiceXPC_arrow), l'appli
/// elle-meme. responsibility_get_pid_responsible_for_pid n'est pas une API publique d'Apple : cherchee a l'execution
/// (dlsym), comme CGWindowListCreateImage ; absente, ou le processus responsable de lui-meme : None. Lit un nom, rien
/// d'autre.
pub fn responsible_exe(pid: u32) -> Option<String> {
    type RespFn = unsafe extern "C" fn(i32) -> i32;
    if pid == 0 || pid > i32::MAX as u32 {
        return None;
    }
    let p = unsafe { dlsym(RTLD_DEFAULT, c"responsibility_get_pid_responsible_for_pid".as_ptr()) };
    if p.is_null() {
        return None;
    }
    let f = unsafe { std::mem::transmute::<*mut c_void, RespFn>(p) };
    let r = unsafe { f(pid as i32) };
    if r <= 0 || r as u32 == pid {
        return None;
    }
    // PROC_PIDPATHINFO_MAXSIZE : 4 × MAXPATHLEN
    let mut buf = vec![0u8; 4096];
    let n = unsafe { proc_pidpath(r, buf.as_mut_ptr(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    let path = String::from_utf8_lossy(&buf[..(n as usize).min(buf.len())]).into_owned();
    path.rsplit('/').next().filter(|s| !s.is_empty()).map(|s| s.to_string())
}
/// RTLD_DEFAULT sous macOS
const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;

pub fn cf_string(s: CFStringRef) -> String {
    if s.is_null() {
        return String::new();
    }
    unsafe {
        let n = CFStringGetLength(s).max(0) as usize;
        let mut buf = vec![0u8; n * 4 + 1];
        if CFStringGetCString(s, buf.as_mut_ptr() as *mut c_char, buf.len() as isize, kCFStringEncodingUTF8) {
            let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
            return String::from_utf8_lossy(&buf[..end]).into_owned();
        }
    }
    String::new()
}

fn cf_i64(n: CFNumberRef) -> i64 {
    let mut v: i64 = 0;
    if !n.is_null() {
        unsafe {
            CFNumberGetValue(n, kCFNumberSInt64Type, &mut v as *mut i64 as *mut c_void);
        }
    }
    v
}

/// Une fenetre telle que CGWindowListCopyWindowInfo la decrit.
#[derive(Debug, Clone, Default)]
pub struct MacWindow {
    pub id: u32,
    pub pid: i32,
    /// le nom de l'appli (« Live », « Google Chrome »)
    pub owner: String,
    /// vide sans l'autorisation « Enregistrement de l'ecran »
    pub title: String,
    pub bounds: CGRect,
    pub layer: i32,
    pub onscreen: bool,
}

fn windows_with(option: u32, relative_to: u32) -> Vec<MacWindow> {
    let mut out = Vec::new();
    unsafe {
        let arr = CGWindowListCopyWindowInfo(option, relative_to);
        if arr.is_null() {
            return out;
        }
        for i in 0..CFArrayGetCount(arr) {
            let d = CFArrayGetValueAtIndex(arr, i) as CFDictionaryRef;
            let get = |k: CFStringRef| CFDictionaryGetValue(d, k);
            let mut bounds = CGRect::default();
            let b = get(kCGWindowBounds);
            if !b.is_null() {
                CGRectMakeWithDictionaryRepresentation(b as CFDictionaryRef, &mut bounds);
            }
            let on = get(kCGWindowIsOnscreen);
            out.push(MacWindow {
                id: cf_i64(get(kCGWindowNumber)) as u32,
                pid: cf_i64(get(kCGWindowOwnerPID)) as i32,
                owner: cf_string(get(kCGWindowOwnerName)),
                title: cf_string(get(kCGWindowName)),
                bounds,
                layer: cf_i64(get(kCGWindowLayer)) as i32,
                onscreen: !on.is_null() && CFBooleanGetValue(on),
            });
        }
        CFRelease(arr);
    }
    out
}

/// Toutes les fenetres (ecran ou non, sans le bureau).
pub fn all_windows() -> Vec<MacWindow> {
    windows_with(kCGWindowListOptionAll | kCGWindowListExcludeDesktopElements, 0)
}

/// Une fenetre par son numero (None : fermee).
pub fn window(id: u64) -> Option<MacWindow> {
    windows_with(kCGWindowListOptionIncludingWindow, id as u32).into_iter().find(|w| w.id as u64 == id)
}

/// Le pid de l'appli d'une fenetre (0 : fermee).
pub fn window_pid(id: u64) -> u32 {
    window(id).map(|w| w.pid.max(0) as u32).unwrap_or(0)
}

/// Les autorisations, lues sans rien demander : (Enregistrement de l'ecran, Accessibilite).
pub fn permissions() -> (bool, bool) {
    unsafe { (CGPreflightScreenCaptureAccess(), AXIsProcessTrusted()) }
}

/// (11 nov.) Une fenetre d'une appli vue par l'Accessibilite : son titre, son cadre ([x, y, largeur, hauteur] en
/// points, origine en haut a gauche de l'ecran principal, comme kCGWindowBounds), et l'element, pour chercher plus tard
/// ce qu'elle porte (`view_button`).
pub struct AxWin {
    pub title: String,
    pub frame: [f64; 4],
    el: AxElement,
}

impl AxWin {
    /// (11 nov., relecture M1) Porte-t-elle le bouton de menu de l'affichage (AXMenuButton decrit « view ») que Logic Pro
    /// met en haut de chacune de ses fenetres de plugins ? (`ax_view_button` : trois niveaux, 400 elements au plus.)
    pub fn view_button(&self) -> bool {
        unsafe { ax_view_button(&self.el).is_some() }
    }
}

/// (11 nov.) Les fenetres d'une appli vues par l'Accessibilite. Logic Pro 12.3.1 ne donne pas a CGWindowList le titre
/// de ses fenetres de plugins, meme avec « Enregistrement de l'ecran » ; l'Accessibilite le donne (« Deluxe Classic »).
/// Lit, ne change rien. Vide sans l'autorisation « Accessibilite » (rien n'est demande), ou si l'appli ne repond pas
/// dans le quart de seconde.
pub fn ax_windows(pid: i32) -> Vec<AxWin> {
    let mut out = Vec::new();
    if pid <= 0 || !unsafe { AXIsProcessTrusted() } {
        return out;
    }
    ax_timeout();
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return out;
        }
        let wins = ax_attr(app, c"AXWindows");
        if !wins.is_null() && CFGetTypeID(wins) == CFArrayGetTypeID() {
            for i in 0..CFArrayGetCount(wins) {
                let w = CFArrayGetValueAtIndex(wins, i);
                let (title, pos, size) = (ax_attr(w, c"AXTitle"), ax_attr(w, c"AXPosition"), ax_attr(w, c"AXSize"));
                let (mut p, mut s) = (CGPoint::default(), CGSize::default());
                let framed = !pos.is_null()
                    && !size.is_null()
                    && AXValueGetValue(pos, kAXValueCGPointType, &mut p as *mut CGPoint as *mut c_void)
                    && AXValueGetValue(size, kAXValueCGSizeType, &mut s as *mut CGSize as *mut c_void);
                let t = if !title.is_null() && CFGetTypeID(title) == CFStringGetTypeID() { cf_string(title) } else { String::new() };
                if framed && !t.is_empty() {
                    out.push(AxWin { title: t, frame: [p.x, p.y, s.width, s.height], el: AxElement(CFRetain(w)) });
                }
                for o in [title, pos, size] {
                    if !o.is_null() {
                        CFRelease(o);
                    }
                }
            }
        }
        if !wins.is_null() {
            CFRelease(wins);
        }
        CFRelease(app);
    }
    out
}

/// Pour toutes les demandes de l'Accessibilite de ce processus : une appli occupee ne retient pas le compagnon plus
/// d'un quart de seconde (6 s par defaut).
fn ax_timeout() {
    static TIMEOUT: std::sync::Once = std::sync::Once::new();
    TIMEOUT.call_once(|| unsafe {
        let sys = AXUIElementCreateSystemWide();
        if !sys.is_null() {
            AXUIElementSetMessagingTimeout(sys, 0.25);
            CFRelease(sys);
        }
    });
}

/// (11 nov.) Un element de l'Accessibilite garde d'un evenement a l'autre (l'appui, puis son relachement) ; libere a
/// la fin.
pub struct AxElement(CFTypeRef);
// les demandes de l'Accessibilite passent par le systeme : un element se garde et s'utilise depuis n'importe quel fil
unsafe impl Send for AxElement {}
impl Drop for AxElement {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

/// Ce que l'Accessibilite trouve sous un point : un bouton a appuyer, ou un bouton rotatif (sa valeur, ses bornes si
/// elle les donne).
pub enum AxTarget {
    Press(AxElement),
    /// `horizontal` : un curseur couche (l'affichage « Controls » de Logic), `width` sa largeur en points
    Knob { el: AxElement, value: f64, range: Option<(f64, f64)>, horizontal: bool, width: f64 },
}

/// (11 nov.) L'element d'une fenetre de plugin sous un point de l'ecran (points, origine en haut a gauche, comme
/// kCGWindowBounds), par l'Accessibilite de l'appli `pid`. Mesure (Logic Pro 12.3.1, l'E-Piano a 50 %) : le test de
/// Logic trouve l'element la ou il est DESSINE (l'onglet DETAILS, le bouton VOLUME), meme quand les cadres annonces
/// sont decales ; sous un bouton rotatif, c'est un AXSlider sans valeur dont le PARENT est le vrai (valeur reglable,
/// AXIncrement/AXDecrement) : on remonte de quelques parents (input::ax_step_of dit quoi faire de chacun).
/// None : rien a faire la (du decor, un menu), ou l'element d'une autre fenetre que `frame` (une fenetre de Logic
/// couvre celle du plugin a cet endroit). Err : pas d'« Accessibilite ».
pub fn ax_target_at(pid: i32, x: f64, y: f64, frame: [f64; 4]) -> Result<Option<AxTarget>, String> {
    if pid <= 0 {
        return Ok(None);
    }
    if !unsafe { AXIsProcessTrusted() } {
        return Err("allow Accessibility for the program that runs the companion (System Settings › Privacy & Security › Accessibility)".into());
    }
    ax_timeout();
    unsafe {
        let app = AxElement(AXUIElementCreateApplication(pid));
        if app.0.is_null() {
            return Ok(None);
        }
        // l'element est-il dans la fenetre du plugin (son cadre) ?
        let Some((cur, win)) = hit_in_window(app.0, x, y, frame) else { return Ok(None) };
        // (11 nov., relecture M1) et cette fenetre est-elle une fenetre de plugin (le bouton de menu de l'affichage) ?
        // Sinon, rien : un studio ne presse rien dans une autre fenetre de Logic, meme si on la lui montrait
        if !plugin_window_sign(pid, frame, &win) {
            return Ok(None);
        }
        Ok(gesture_target(cur))
    }
}

/// L'element sous le point (x, y) de l'ecran, s'il est dans la fenetre au cadre `frame` (a 2 points pres), et cette
/// fenetre. None : rien, ou l'element d'une autre fenetre (une fenetre de Logic couvre celle du plugin a cet endroit).
unsafe fn hit_in_window(app: CFTypeRef, x: f64, y: f64, frame: [f64; 4]) -> Option<(AxElement, AxElement)> {
    let mut hit: CFTypeRef = std::ptr::null();
    if AXUIElementCopyElementAtPosition(app, x as f32, y as f32, &mut hit) != 0 || hit.is_null() {
        return None;
    }
    let cur = AxElement(hit);
    let win = AxElement(ax_attr(cur.0, c"AXWindow"));
    match ax_frame(win.0) {
        Some(f) if f.iter().zip(frame).all(|(a, b)| (a - b).abs() <= 2.0) => Some((cur, win)),
        _ => None,
    }
}

/// Ce qu'un geste du studio ferait de l'element `cur` (ou de l'un de ses cinq premiers parents) : un bouton a
/// appuyer, un bouton rotatif a tourner, ou rien (input::ax_step_of dit quoi faire de chacun).
unsafe fn gesture_target(mut cur: AxElement) -> Option<AxTarget> {
    for _ in 0..5 {
        let role = AxElement(ax_attr(cur.0, c"AXRole"));
        let role = if !role.0.is_null() && CFGetTypeID(role.0) == CFStringGetTypeID() { cf_string(role.0) } else { String::new() };
        let value = ax_number(cur.0, c"AXValue");
        let settable = value.is_some() && ax_settable(cur.0, c"AXValue");
        match crate::input::ax_step_of(&role, settable) {
            crate::input::AxStep::Press => return Some(AxTarget::Press(cur)),
            crate::input::AxStep::Knob => {
                let range = match (ax_number(cur.0, c"AXMinValue"), ax_number(cur.0, c"AXMaxValue")) {
                    (Some(lo), Some(hi)) if hi > lo => Some((lo, hi)),
                    _ => None,
                };
                let orient = AxElement(ax_attr(cur.0, c"AXOrientation"));
                let horizontal = !orient.0.is_null() && CFGetTypeID(orient.0) == CFStringGetTypeID() && cf_string(orient.0) == "AXHorizontalOrientation";
                let width = ax_frame(cur.0).map(|f| f[2]).unwrap_or(0.0);
                return Some(AxTarget::Knob { value: value.unwrap_or(0.0), range, horizontal, width, el: cur });
            }
            crate::input::AxStep::Stop => return None,
            crate::input::AxStep::Up => {}
        }
        let parent = ax_attr(cur.0, c"AXParent");
        if parent.is_null() {
            break;
        }
        cur = AxElement(parent);
    }
    None
}

/// L'appui (AXPress) d'un bouton.
pub fn ax_press(el: &AxElement) -> Result<(), String> {
    ax_action(el, c"AXPress")
}

/// Un cran de plus (AXIncrement) ou de moins (AXDecrement) d'un bouton rotatif.
pub fn ax_step(el: &AxElement, up: bool) -> Result<(), String> {
    ax_action(el, if up { c"AXIncrement" } else { c"AXDecrement" })
}

/// La valeur d'un bouton rotatif.
pub fn ax_set_value(el: &AxElement, v: f64) -> Result<(), String> {
    unsafe {
        let key = CFStringCreateWithCString(std::ptr::null(), c"AXValue".as_ptr(), kCFStringEncodingUTF8);
        let num = CFNumberCreate(std::ptr::null(), kCFNumberFloat64Type, &v as *const f64 as *const c_void);
        let err = if key.is_null() || num.is_null() { -1 } else { AXUIElementSetAttributeValue(el.0, key, num) };
        if !key.is_null() {
            CFRelease(key);
        }
        if !num.is_null() {
            CFRelease(num);
        }
        if err == 0 {
            Ok(())
        } else {
            Err(format!("the plugin refused the value (Accessibility error {err})"))
        }
    }
}

/// (11 nov.) La valeur d'un bouton, verifiee : mesure dans l'affichage « Controls » de Logic, la case de valeur
/// numerique prend un AXValue pour UN cran (75 → 74 en demandant 50), la glissiere voisine le prend tel quel. On relit
/// donc la valeur, et on finit par crans (AXIncrement / AXDecrement, 32 au plus) jusqu'a `tol` pres. → la valeur atteinte.
pub fn ax_reach_value(el: &AxElement, target: f64, tol: f64) -> Result<f64, String> {
    ax_set_value(el, target)?;
    let read = || unsafe { ax_number(el.0, c"AXValue") };
    let Some(mut now) = read() else { return Ok(target) };
    for _ in 0..32 {
        let gap = target - now;
        if gap.abs() <= tol {
            break;
        }
        ax_step(el, gap > 0.0)?;
        let Some(next) = read() else { break };
        // plus rien ne bouge, ou la cible est depassee : on s'arrete la
        let stuck = (next - now).abs() < 1e-9 || (target - next).signum() != gap.signum();
        now = next;
        if stuck {
            break;
        }
    }
    Ok(now)
}

fn ax_action(el: &AxElement, name: &std::ffi::CStr) -> Result<(), String> {
    unsafe {
        let key = CFStringCreateWithCString(std::ptr::null(), name.as_ptr(), kCFStringEncodingUTF8);
        if key.is_null() {
            return Err("CFString failed".into());
        }
        let err = AXUIElementPerformAction(el.0, key);
        CFRelease(key);
        if err == 0 {
            Ok(())
        } else {
            Err(format!("the plugin refused the gesture (Accessibility error {err})"))
        }
    }
}

/// (11 nov.) L'affichage d'une fenetre de plugin de Logic Pro : « Controls » (Logic y dessine tous les reglages en
/// curseurs, accessibles) ou le dessin du plugin ; `opaque` : ce dessin ne montre rien a l'Accessibilite (mesure : PSP
/// VintageWarmer2, sous un bouton Logic ne trouve que la fenetre), les gestes du studio n'y passent donc pas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogicView {
    pub controls: bool,
    pub opaque: bool,
}

/// La fenetre de l'appli dont le cadre est `frame` (a 2 points pres), par l'Accessibilite.
unsafe fn ax_window_by_frame(app: CFTypeRef, frame: [f64; 4]) -> Option<AxElement> {
    let wins = AxElement(ax_attr(app, c"AXWindows"));
    if wins.0.is_null() || CFGetTypeID(wins.0) != CFArrayGetTypeID() {
        return None;
    }
    for i in 0..CFArrayGetCount(wins.0) {
        let w = CFArrayGetValueAtIndex(wins.0, i);
        if ax_frame(w).is_some_and(|f| f.iter().zip(frame).all(|(a, b)| (a - b).abs() <= 2.0)) {
            return Some(AxElement(CFRetain(w)));
        }
    }
    None
}

/// (11 nov., relecture M1) La derniere fenetre reconnue comme fenetre de plugin par son bouton de menu de l'affichage
/// (pid, cadre, quand) : les gestes d'une rafale (la molette, plusieurs dizaines par seconde) ne refont pas la recherche.
static PLUGIN_SIGN: std::sync::Mutex<Option<(i32, [f64; 4], std::time::Instant)>> = std::sync::Mutex::new(None);
const PLUGIN_SIGN_FRESH: std::time::Duration = std::time::Duration::from_secs(1);

fn plugin_window_sign(pid: i32, frame: [f64; 4], win: &AxElement) -> bool {
    let same = |f: [f64; 4]| f.iter().zip(frame).all(|(a, b)| (a - b).abs() <= 2.0);
    if let Ok(g) = PLUGIN_SIGN.lock() {
        if g.as_ref().is_some_and(|&(p, f, t)| p == pid && same(f) && t.elapsed() < PLUGIN_SIGN_FRESH) {
            return true;
        }
    }
    let ok = unsafe { ax_view_button(win).is_some() };
    if let Ok(mut g) = PLUGIN_SIGN.lock() {
        *g = ok.then(|| (pid, frame, std::time::Instant::now()));
    }
    ok
}

/// (11 nov., relecture M1) La fenetre de Logic `frame` porte-t-elle le bouton de menu de l'affichage, que seules ses
/// fenetres de plugins portent ? Relu a chaque fois (pour `view`, rare). false sans « Accessibilite ».
pub fn ax_plugin_window(pid: i32, frame: [f64; 4]) -> bool {
    if pid <= 0 || !unsafe { AXIsProcessTrusted() } {
        return false;
    }
    ax_timeout();
    unsafe {
        let app = AxElement(AXUIElementCreateApplication(pid));
        if app.0.is_null() {
            return false;
        }
        ax_window_by_frame(app.0, frame).is_some_and(|w| ax_view_button(&w).is_some())
    }
}

/// Le bouton de menu de l'affichage dans la barre de Logic, au-dessus du plugin (AXMenuButton, AXDescription
/// « view », titre « Editor » ou « Controls » : mesure du 11 nov.) ; cherche sur trois niveaux, 400 elements au plus.
unsafe fn ax_view_button(win: &AxElement) -> Option<AxElement> {
    let mut level = vec![AxElement(CFRetain(win.0))];
    let mut seen = 0;
    for _ in 0..3 {
        let mut next = Vec::new();
        for e in &level {
            let kids = AxElement(ax_attr(e.0, c"AXChildren"));
            if kids.0.is_null() || CFGetTypeID(kids.0) != CFArrayGetTypeID() {
                continue;
            }
            for i in 0..CFArrayGetCount(kids.0) {
                seen += 1;
                if seen > 400 {
                    return None;
                }
                let k = CFArrayGetValueAtIndex(kids.0, i);
                if ax_text(k, c"AXRole") == "AXMenuButton" && ax_text(k, c"AXDescription") == "view" {
                    return Some(AxElement(CFRetain(k)));
                }
                next.push(AxElement(CFRetain(k)));
            }
        }
        level = next;
    }
    None
}

/// Le texte d'un attribut (AXRole, AXTitle…), ou vide.
unsafe fn ax_text(e: CFTypeRef, name: &std::ffi::CStr) -> String {
    let v = AxElement(ax_attr(e, name));
    if !v.0.is_null() && CFGetTypeID(v.0) == CFStringGetTypeID() {
        cf_string(v.0)
    } else {
        String::new()
    }
}

/// L'affichage de la fenetre `win` : son bouton de menu, et si le dessin du plugin est opaque. (11 nov., 23 h) Avant,
/// un seul point, le centre sous la barre de Logic : sur l'E-Piano a 50 %, il tombe sur un decor sans element, et le
/// studio disait a tort que ses clics n'y passaient pas. Maintenant, douze points sous la barre
/// (`plugins::opaque_probe_points`) : opaque seulement si aucun ne trouve un bouton a appuyer ou a tourner, comme un
/// geste du studio le chercherait (`gesture_target`), parmi ceux qui tombent bien dans cette fenetre
/// (`plugins::opaque_of`).
unsafe fn ax_logic_view_of(app: CFTypeRef, win: &AxElement) -> Option<LogicView> {
    let button = ax_view_button(win)?;
    let controls = ax_text(button.0, c"AXTitle") == "Controls";
    let f = ax_frame(win.0)?;
    let found: Vec<Option<bool>> = crate::plugins::opaque_probe_points(f, LOGIC_HEADER)
        .into_iter()
        .map(|(x, y)| hit_in_window(app, x, y, f).map(|(cur, _)| gesture_target(cur).is_some()))
        .collect();
    Some(LogicView { controls, opaque: crate::plugins::opaque_of(&found) })
}

/// La hauteur de la barre de Logic au-dessus du dessin d'un plugin (points), que le test « opaque » laisse de cote.
const LOGIC_HEADER: f64 = 100.0;

/// (11 nov.) L'affichage de la fenetre de plugin de Logic Pro au cadre `frame`. None : pas d'Accessibilite, ou pas de
/// fenetre de plugin de Logic la.
pub fn ax_logic_view(pid: i32, frame: [f64; 4]) -> Option<LogicView> {
    if pid <= 0 || !unsafe { AXIsProcessTrusted() } {
        return None;
    }
    ax_timeout();
    unsafe {
        let app = AxElement(AXUIElementCreateApplication(pid));
        if app.0.is_null() {
            return None;
        }
        let win = ax_window_by_frame(app.0, frame)?;
        ax_logic_view_of(app.0, &win)
    }
}

/// (11 nov.) Passe la fenetre de plugin de Logic Pro au cadre `frame` a « Controls » (ou au dessin du plugin), par son
/// menu d'affichage : AXPress ouvre le menu, puis AXPress sur l'element choisi (plugins::logic_view_item ; mesure :
/// « Controls », puis le nom du plugin). Le menu s'ouvre un instant ; sans l'element voulu, il est referme (AXCancel).
/// → l'affichage obtenu (la fenetre change de taille).
pub fn ax_set_logic_view(pid: i32, frame: [f64; 4], controls: bool) -> Result<LogicView, String> {
    if pid <= 0 {
        return Err("no application".into());
    }
    if !unsafe { AXIsProcessTrusted() } {
        return Err("allow Accessibility for the program that runs the companion (System Settings › Privacy & Security › Accessibility)".into());
    }
    ax_timeout();
    unsafe {
        let app = AxElement(AXUIElementCreateApplication(pid));
        if app.0.is_null() {
            return Err("no application".into());
        }
        let win = ax_window_by_frame(app.0, frame).ok_or("the plugin window is not found by Accessibility")?;
        let button = ax_view_button(&win).ok_or("this window has no Editor / Controls menu")?;
        if (ax_text(button.0, c"AXTitle") == "Controls") == controls {
            return ax_logic_view_of(app.0, &win).ok_or_else(|| "the view cannot be read".to_string());
        }
        ax_press(&button)?;
        std::thread::sleep(std::time::Duration::from_millis(250));
        // le menu ouvert : ses elements (AXMenu > AXMenuItem)
        let mut items: Vec<AxElement> = Vec::new();
        let menus = AxElement(ax_attr(button.0, c"AXChildren"));
        if !menus.0.is_null() && CFGetTypeID(menus.0) == CFArrayGetTypeID() {
            for i in 0..CFArrayGetCount(menus.0) {
                let menu = CFArrayGetValueAtIndex(menus.0, i);
                let kids = AxElement(ax_attr(menu, c"AXChildren"));
                if kids.0.is_null() || CFGetTypeID(kids.0) != CFArrayGetTypeID() {
                    continue;
                }
                for j in 0..CFArrayGetCount(kids.0) {
                    let k = CFArrayGetValueAtIndex(kids.0, j);
                    if ax_text(k, c"AXRole") == "AXMenuItem" {
                        items.push(AxElement(CFRetain(k)));
                    }
                }
            }
        }
        let titles: Vec<String> = items.iter().map(|e| ax_text(e.0, c"AXTitle")).collect();
        let Some(i) = crate::plugins::logic_view_item(&titles, controls) else {
            if !menus.0.is_null() && CFGetTypeID(menus.0) == CFArrayGetTypeID() {
                for i in 0..CFArrayGetCount(menus.0) {
                    let menu = AxElement(CFRetain(CFArrayGetValueAtIndex(menus.0, i)));
                    let _ = ax_action(&menu, c"AXCancel");
                }
            }
            return Err(format!("the view menu has no {} item ({titles:?})", if controls { "Controls" } else { "plugin" }));
        };
        ax_press(&items[i])?;
        // le bouton prend le titre du nouvel affichage un instant apres (mesure : pas encore a 250 ms) : on l'attend,
        // une seconde au plus
        for _ in 0..10 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if (ax_text(button.0, c"AXTitle") == "Controls") == controls {
                break;
            }
        }
        ax_logic_view_of(app.0, &win).ok_or_else(|| "the view cannot be read".to_string())
    }
}

/// Un nombre de l'Accessibilite (AXValue, AXMinValue…), s'il en est un.
unsafe fn ax_number(e: CFTypeRef, name: &std::ffi::CStr) -> Option<f64> {
    let v = ax_attr(e, name);
    if v.is_null() {
        return None;
    }
    let mut out = 0.0f64;
    let ok = CFGetTypeID(v) == CFNumberGetTypeID() && CFNumberGetValue(v, kCFNumberFloat64Type, &mut out as *mut f64 as *mut c_void);
    CFRelease(v);
    (ok && out.is_finite()).then_some(out)
}

unsafe fn ax_settable(e: CFTypeRef, name: &std::ffi::CStr) -> bool {
    let key = CFStringCreateWithCString(std::ptr::null(), name.as_ptr(), kCFStringEncodingUTF8);
    if key.is_null() {
        return false;
    }
    let mut ok: u8 = 0;
    let err = AXUIElementIsAttributeSettable(e, key, &mut ok);
    CFRelease(key);
    err == 0 && ok != 0
}

/// Le cadre d'un element (x, y, largeur, hauteur en points), s'il le donne.
unsafe fn ax_frame(e: CFTypeRef) -> Option<[f64; 4]> {
    if e.is_null() {
        return None;
    }
    let (pos, size) = (ax_attr(e, c"AXPosition"), ax_attr(e, c"AXSize"));
    let (mut p, mut s) = (CGPoint::default(), CGSize::default());
    let ok = !pos.is_null()
        && !size.is_null()
        && AXValueGetValue(pos, kAXValueCGPointType, &mut p as *mut CGPoint as *mut c_void)
        && AXValueGetValue(size, kAXValueCGSizeType, &mut s as *mut CGSize as *mut c_void);
    for o in [pos, size] {
        if !o.is_null() {
            CFRelease(o);
        }
    }
    ok.then_some([p.x, p.y, s.width, s.height])
}

/// Un attribut d'un element de l'Accessibilite (a liberer par CFRelease), ou nul.
unsafe fn ax_attr(e: CFTypeRef, name: &std::ffi::CStr) -> CFTypeRef {
    let key = CFStringCreateWithCString(std::ptr::null(), name.as_ptr(), kCFStringEncodingUTF8);
    if key.is_null() {
        return std::ptr::null();
    }
    let mut v: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(e, key, &mut v);
    CFRelease(key);
    if err == 0 {
        v
    } else {
        std::ptr::null()
    }
}

type CreateImageFn = unsafe extern "C" fn(CGRect, u32, u32, u32) -> CGImageRef;

fn create_image_fn() -> Option<CreateImageFn> {
    let p = unsafe { dlsym(RTLD_DEFAULT, c"CGWindowListCreateImage".as_ptr()) };
    (!p.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, CreateImageFn>(p) })
}

/// L'image d'une fenetre en BGRA serre (w*4 octets par ligne), en pixels. None : rien (fermee, ou pas d'autorisation :
/// macOS rend alors une image vide ou le fond d'ecran seul).
pub fn window_image(id: u64) -> Result<Option<crate::capture::Frame>, String> {
    let f = create_image_fn().ok_or("this macOS has no CGWindowListCreateImage (ScreenCaptureKit is not done yet)")?;
    // CGRectNull : les limites de la fenetre elle-meme
    let null = CGRect { origin: CGPoint { x: f64::INFINITY, y: f64::INFINITY }, size: CGSize { width: 0.0, height: 0.0 } };
    unsafe {
        let img = f(null, kCGWindowListOptionIncludingWindow, id as u32, kCGWindowImageBoundsIgnoreFraming | kCGWindowImageBestResolution);
        if img.is_null() {
            return Ok(None);
        }
        let (w, h) = (CGImageGetWidth(img), CGImageGetHeight(img));
        if w == 0 || h == 0 || w > 16384 || h > 16384 {
            CGImageRelease(img);
            return Ok(None);
        }
        // redessinee dans un tampon au format connu : BGRA premultiplie (petit-boutiste, alpha en tete)
        let mut buf = vec![0u8; w * h * 4];
        let cs = CGColorSpaceCreateDeviceRGB();
        let ctx = CGBitmapContextCreate(buf.as_mut_ptr() as *mut c_void, w, h, 8, w * 4, cs, kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little);
        CGColorSpaceRelease(cs);
        if ctx.is_null() {
            CGImageRelease(img);
            return Err("CGBitmapContextCreate failed".into());
        }
        CGContextDrawImage(ctx, CGRect { origin: CGPoint::default(), size: CGSize { width: w as f64, height: h as f64 } }, img);
        CGContextRelease(ctx);
        CGImageRelease(img);
        Ok(Some(crate::capture::Frame { w: w as u32, h: h as u32, bgra: buf }))
    }
}

/// La capture d'une fenetre (capture::Capturer) : une image seulement si elle a change.
pub struct WindowCapture {
    id: u64,
    last: u64,
}

impl WindowCapture {
    pub fn open(id: u64) -> Result<WindowCapture, String> {
        if window(id).is_none() {
            return Err(format!("no window {id}"));
        }
        if !permissions().0 {
            // la premiere capture fait apparaitre la demande de macOS ; sans l'autorisation l'image est vide
            crate::log("macOS: Screen Recording is not allowed for this program yet (System Settings › Privacy & Security › Screen Recording)");
        }
        create_image_fn().ok_or("this macOS has no CGWindowListCreateImage (ScreenCaptureKit is not done yet)")?;
        Ok(WindowCapture { id, last: 0 })
    }
}

fn digest(b: &[u8]) -> u64 {
    // FNV-1a sur un octet sur 7 : assez pour savoir si l'image a change, sans tout parcourir
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for x in b.iter().step_by(7) {
        h ^= *x as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h ^ b.len() as u64
}

impl crate::capture::Capturer for WindowCapture {
    fn grab(&mut self) -> Result<Option<crate::capture::Frame>, String> {
        if window(self.id).is_none() {
            return Err("the window is gone".into());
        }
        let Some(f) = window_image(self.id)? else { return Ok(None) };
        let d = digest(&f.bgra);
        if d == self.last {
            return Ok(None);
        }
        self.last = d;
        Ok(Some(f))
    }
    fn kind(&self) -> &'static str {
        "cgwindow"
    }
}

/// Un evenement souris a poster (input::mac_pointer_events le compose ; ici seulement l'envoi).
pub fn post_mouse(pid: u32, m: &crate::input::MacMouse) -> Result<(), String> {
    unsafe {
        let e = CGEventCreateMouseEvent(std::ptr::null(), m.ty, CGPoint { x: m.x, y: m.y }, m.button);
        if e.is_null() {
            return Err("CGEventCreateMouseEvent failed".into());
        }
        // kCGMouseEventClickState = 1
        CGEventSetIntegerValueField(e, 1, m.clicks);
        CGEventSetFlags(e, m.flags);
        CGEventPostToPid(pid as i32, e);
        CFRelease(e as CFTypeRef);
    }
    Ok(())
}

/// La molette (en pixels), au point donne.
pub fn post_wheel(pid: u32, at: (f64, f64), dx: i32, dy: i32, flags: u64) -> Result<(), String> {
    unsafe {
        // kCGScrollEventUnitPixel = 0 ; deux axes : vertical puis horizontal
        let e = CGEventCreateScrollWheelEvent2(std::ptr::null(), 0, 2, dy, dx, 0);
        if e.is_null() {
            return Err("CGEventCreateScrollWheelEvent2 failed".into());
        }
        CGEventSetLocation(e, CGPoint { x: at.0, y: at.1 });
        CGEventSetFlags(e, flags);
        CGEventPostToPid(pid as i32, e);
        CFRelease(e as CFTypeRef);
    }
    Ok(())
}

/// Une touche (code virtuel macOS), avec le texte qu'elle tape (vide : la touche seule).
pub fn post_key(pid: u32, keycode: u16, down: bool, text: &[u16], flags: u64) -> Result<(), String> {
    unsafe {
        let e = CGEventCreateKeyboardEvent(std::ptr::null(), keycode, down);
        if e.is_null() {
            return Err("CGEventCreateKeyboardEvent failed".into());
        }
        if !text.is_empty() {
            CGEventKeyboardSetUnicodeString(e, text.len(), text.as_ptr());
        }
        CGEventSetFlags(e, flags);
        CGEventPostToPid(pid as i32, e);
        CFRelease(e as CFTypeRef);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// (9 nov.) L'appli responsable : rien pour un pid absent ; pour ce processus d'essai, son appli responsable (un
    /// terminal, l'agent de la CI…) ou rien, jamais un nom vide. Lu, jamais rien de modifie.
    #[test]
    fn the_responsible_app_is_read_and_never_empty() {
        assert_eq!(super::responsible_exe(0), None);
        assert_eq!(super::responsible_exe(u32::MAX), None);
        if let Some(n) = super::responsible_exe(std::process::id()) {
            assert!(!n.is_empty() && !n.contains('/'), "{n}");
        }
    }
}
