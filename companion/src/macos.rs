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
}

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
}

extern "C" {
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
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
