// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (20 nov., l'hote macOS) La coquille macOS du processus enfant (child.rs) : l'appli AppKit sans icone dans le Dock,
//! la fenetre de l'editeur (une NSWindow a nous, l'IPlugView attache a sa contentView, « NSView »), la boucle de
//! l'interface (nextEventMatchingMask : les minuteries et les sources du plugin tournent dans la boucle principale), les
//! ordres du compagnon qui la reveillent (un NSEvent « ApplicationDefined » poste depuis le fil qui lit l'entree), la
//! croix (un delegue cree a l'execution : windowShouldClose: → la fenetre se range, le plugin reste).
//!
//! Par les API C d'Objective-C (libobjc : objc_msgSend, la classe du delegue), sans crate de plus — comme macos.rs du
//! compagnon. Aucun appel ne RENVOIE de structure (NSRect…) : sur x86_64 il faudrait objc_msgSend_stret ; les
//! structures passees en argument (CGRect, CGSize, CGPoint) suivent la convention C des deux architectures.
//!
//! Ou ranger la fenetre quand le studio la filme sans la montrer : UWL_HOST_PARK (l'essai du 20 nov. le mesure) —
//! « behind » (a l'ecran, derriere toutes les fenetres : orderBack), « offscreen » (hors de tout ecran), « alpha »
//! (a l'ecran, transparente, les clics la traversent). Sans valeur : le choix de l'essai (PARK_DEFAULT).

#![allow(non_upper_case_globals)]

use crate::child::UiEv;
use crate::realhost::{Frame, Instance};
use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{ComPtr, ComWrapper};

pub type Id = *mut c_void;
type Sel = *const c_void;
pub type Win = isize;

#[repr(C)]
#[derive(Clone, Copy)]
struct Pt {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Sz {
    w: f64,
    h: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    o: Pt,
    s: Sz,
}

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    fn objc_allocateClassPair(sup: Id, name: *const c_char, extra: usize) -> Id;
    fn objc_registerClassPair(cls: Id);
    fn class_addMethod(cls: Id, sel: Sel, imp: *const c_void, types: *const c_char) -> bool;
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(p: *mut c_void);
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {
    static NSMenuDidBeginTrackingNotification: Id;
    static NSMenuDidEndTrackingNotification: Id;
    static NSMenuDidSendActionNotification: Id;
}

#[link(name = "Foundation", kind = "framework")]
extern "C" {
    static NSDefaultRunLoopMode: Id;
    static NSRunLoopCommonModes: Id;
}

extern "C" {
    fn pthread_set_qos_class_self_np(qos: u32, rel: i32) -> i32;
}

/// objc_msgSend avec la signature exacte (jamais l'appel variadique : arm64 ne le permet pas).
macro_rules! send {
    ($obj:expr, $sel:expr $(, $t:ty = $a:expr)* => $r:ty) => {{
        let f: unsafe extern "C" fn(Id, Sel $(, $t)*) -> $r = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f($obj, sel_registerName($sel.as_ptr()) $(, $a)*)
    }};
}

unsafe fn class(name: &CStr) -> Id {
    objc_getClass(name.as_ptr())
}

unsafe fn app() -> Id {
    send!(class(c"NSApplication"), c"sharedApplication" => Id)
}

unsafe fn nsstring(s: &str) -> Id {
    let c = std::ffi::CString::new(s.replace('\0', "")).unwrap_or_default();
    send!(class(c"NSString"), c"stringWithUTF8String:", *const c_char = c.as_ptr() => Id)
}

/// (20 nov.) La fenetre de l'enfant (une seule par processus) : son pointeur, son numero, son cadre (ecran, points),
/// relus dans le fil principal a chaque changement — le fil qui lit les gestes s'en sert sans toucher a AppKit.
static WIN_PTR: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static WIN_NUM: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static FRAME: std::sync::Mutex<[f64; 4]> = std::sync::Mutex::new([0.0; 4]);

/// (fil principal) relire le numero et le cadre de la fenetre
fn refresh(w: Win) {
    if w == 0 {
        return;
    }
    unsafe {
        let r = frame_of(w as Id);
        *FRAME.lock().unwrap() = [r.o.x, r.o.y, r.s.w, r.s.h];
        WIN_NUM.store(send!(w as Id, c"windowNumber" => isize), Ordering::Relaxed);
    }
    WIN_PTR.store(w, Ordering::Relaxed);
}

/// Le genre d'evenement « defini par l'appli » (NSEventTypeApplicationDefined) et nos sous-types (data1).
const APP_DEFINED: usize = 15;

fn ev_code(e: UiEv) -> isize {
    match e {
        UiEv::Cmd => 1,
        UiEv::Quit => 2,
        UiEv::HideEditor => 3,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Park {
    Behind,
    Offscreen,
    Alpha,
}

/// (20 nov.) le choix par defaut AVANT l'essai a l'ecran (a-lire/hote-vst3-macos-20nov.md, §5) : derriere les autres
/// fenetres, comme la fenetre d'un plugin de Live aujourd'hui. A revoir avec les mesures.
const PARK_DEFAULT: Park = Park::Behind;

pub fn park_mode() -> Park {
    match std::env::var("UWL_HOST_PARK").as_deref() {
        Ok("offscreen") => Park::Offscreen,
        Ok("alpha") => Park::Alpha,
        Ok("behind") => Park::Behind,
        _ => PARK_DEFAULT,
    }
}

pub fn init() {
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let a = app();
        // NSApplicationActivationPolicyAccessory : ni icone dans le Dock ni menu ; ses fenetres peuvent passer devant
        send!(a, c"setActivationPolicy:", isize = 1 => i8);
        send!(a, c"finishLaunching" => ());
        watch_menus();
        objc_autoreleasePoolPop(pool);
    }
}

// ── (20 nov., relecture de securite macOS) le clavier du studio et les boites du plugin ─────────────────────────────
// Une boite que le plugin ouvre (« Ouvrir », « Enregistrer », une alerte) n'est pas filmee : le studio ne voit que
// l'editeur. Posees dans la file d'AppKit, nos touches iraient a la boucle de la boite (modale) a l'aveugle : un chemin,
// puis Entree. Elles ne passent donc plus par la file : le fil de l'entree les range dans KEY_Q et demande uwKey: au fil
// principal (modes « communs » : il tourne aussi pendant une boite modale ou un menu). La, AU MOMENT DE LIVRER, une touche
// est jetee si une fenetre modale est ouverte ou si une feuille est attachee a l'editeur ; sinon l'editeur devient la
// fenetre principale de l'enfant et la recoit directement (sendEvent: de SA fenetre : jamais une autre). Pendant un menu
// du plugin, elle est posee dans la file, que la boucle du menu lit (les fleches, Entree).
static KEY_Q: std::sync::Mutex<std::collections::VecDeque<(usize, usize, String, u16)>> = std::sync::Mutex::new(std::collections::VecDeque::new());
static KEYS_HELD: AtomicBool = AtomicBool::new(false);

unsafe fn key_event(ty: usize, flags: usize, text: &str, kc: u16) -> Id {
    let now = send!(send!(class(c"NSProcessInfo"), c"processInfo" => Id), c"systemUptime" => f64);
    let s = nsstring(text);
    send!(class(c"NSEvent"), c"keyEventWithType:location:modifierFlags:timestamp:windowNumber:context:characters:charactersIgnoringModifiers:isARepeat:keyCode:",
        usize = ty, Pt = Pt { x: 0.0, y: 0.0 }, usize = flags, f64 = now, isize = WIN_NUM.load(Ordering::Relaxed), Id = std::ptr::null_mut(), Id = s, Id = s, i8 = 0, u16 = kc => Id)
}

extern "C" fn key_now(_this: Id, _sel: Sel, _arg: Id) {
    let q: Vec<(usize, usize, String, u16)> = KEY_Q.lock().unwrap().drain(..).collect();
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let a = app();
        let w = WIN_PTR.load(Ordering::Relaxed) as Id;
        for (ty, flags, text, kc) in q {
            if w.is_null() {
                break;
            }
            let e = key_event(ty, flags, &text, kc);
            if e.is_null() {
                continue;
            }
            if menu_open() {
                send!(a, c"postEvent:atStart:", Id = e, i8 = 0 => ());
                continue;
            }
            let held = !send!(a, c"modalWindow" => Id).is_null() || !send!(w, c"attachedSheet" => Id).is_null();
            if KEYS_HELD.swap(held, Ordering::Relaxed) != held {
                eprintln!("plugin host: keys from the studio {}", if held { "dropped: the plugin has a dialog open" } else { "delivered again" });
            }
            if held {
                continue;
            }
            if send!(w, c"isKeyWindow" => i8) == 0 {
                send!(w, c"makeKeyWindow" => ());
            }
            send!(w, c"sendEvent:", Id = e => ());
        }
        objc_autoreleasePoolPop(pool);
    }
}

// ── (20 nov.) Les menus du plugin ───────────────────────────────────────────────────────────────────────────────
// Un clic qui ouvre un menu du plugin (un NSMenu : popUpContextMenu, un selecteur) entre dans la boucle de suivi du menu
// d'AppKit, DANS le sendEvent: de notre boucle : tant qu'il est ouvert, next() ne rend plus la main. Mesures du 20 nov.
// (le menu contextuel de Diva, et un menu d'essai a nous : test_menu) :
// - la boucle de suivi prend les evenements poses dans la file : Echap ou un clic hors du menu le ferment, les fleches
//   et Entree y naviguent et choisissent un article ;
// - un clic SUR un article ne le choisit jamais : qu'il soit pose dans la file pour notre fenetre ou pour celle du menu,
//   ou envoye au processus (CGEventPostToPid), le menu le prend pour un clic a cote et se ferme (il se fie a la vraie
//   position du pointeur, qu'on ne bouge pas : c'est celui de la personne devant le Mac) ;
// - elle fait tourner la boucle d'execution dans un mode « commun » : un performSelector en NSRunLoopCommonModes y passe ;
// - l'accessibilite de nos propres objets (accessibilityFrame de chaque NSMenuItem, sans l'autorisation du systeme)
//   donne la place de chaque article a l'ecran.
// D'ou :
// - les notifications de NSMenu tiennent la pile des menus ouverts (MENU_STACK, le menu et ses sous-menus) et le
//   compagnon apprend l'ouverture et la fermeture ({"event":"menu"}) ;
// - pendant un menu, un geste de souris du studio passe par uwMenuMouse: (fil principal) : un clic sur un article devient
//   le clavier (les fleches jusqu'a lui, puis Entree, ou → pour un sous-menu) ; un clic hors des menus est pose tel quel
//   (il ferme le menu, comme sur le Mac) ; le reste (deplacements, relachements) n'est pas pose ;
// - un ordre du compagnon passe par un performSelector (uwPump:) qui traite ce qui ne touche pas a la fenetre (« save »,
//   le melange, le tempo) ; un autre ordre (quit, editor, view, render, live) ferme d'abord le menu (cancelTracking),
//   puis la boucle reprend et le traite.

/// Le nombre de menus en cours de suivi (le menu et ses sous-menus) ; 0 : aucun. Ecrit par le fil principal, lu partout.
static MENU_DEPTH: AtomicIsize = AtomicIsize::new(0);
/// Le menu du dessus (le premier ouvert) : seulement lu et ecrit dans le fil principal.
static MENU_ROOT: AtomicIsize = AtomicIsize::new(0);
/// Les gestes de souris arrives pendant un menu, pour uwMenuMouse: (le genre NSEvent, le point a l'ecran en
/// coordonnees Cocoa, les modificateurs, le nombre de clics).
static MENU_MOUSE: std::sync::Mutex<std::collections::VecDeque<(usize, Pt, usize, isize)>> = std::sync::Mutex::new(std::collections::VecDeque::new());
/// L'objet qui recoit les notifications et les performSelector (jamais libere).
static WATCH: AtomicIsize = AtomicIsize::new(0);
static MENU_LOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// (essais) les secondes depuis le debut de l'enfant, pour le journal des menus
fn t_log() -> f64 {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
}

fn menu_log() -> bool {
    *MENU_LOG.get_or_init(|| std::env::var("UWL_HOST_MENU_LOG").map(|v| v == "1").unwrap_or(false))
}

/// Un menu du plugin est-il ouvert (en cours de suivi) ?
pub fn menu_open() -> bool {
    MENU_DEPTH.load(Ordering::SeqCst) > 0
}

thread_local! {
    /// (fil principal) les menus ouverts, le premier d'abord (les notifications de NSMenu)
    static MENU_STACK: std::cell::RefCell<Vec<isize>> = const { std::cell::RefCell::new(Vec::new()) };
    /// (fil principal) l'article appuye pendant un menu : (menu, indice), choisi a son relachement
    static PRESSED: std::cell::Cell<Option<(isize, isize)>> = const { std::cell::Cell::new(None) };
    /// (fil principal) ce que next() fait des ordres pendant un menu (child.rs) ; nul hors de next()
    static IN_MENU: std::cell::Cell<Option<*mut (dyn FnMut() + 'static)>> = const { std::cell::Cell::new(None) };
    static PUMPING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

unsafe fn utf8(s: Id) -> String {
    if s.is_null() {
        return String::new();
    }
    let p = send!(s, c"UTF8String" => *const c_char);
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

unsafe fn item_titles(m: Id) -> Vec<String> {
    let n = send!(m, c"numberOfItems" => isize).clamp(0, 64);
    (0..n)
        .map(|i| {
            let it = send!(m, c"itemAtIndex:", isize = i => Id);
            format!("{}{}", utf8(send!(it, c"title" => Id)), if send!(it, c"state" => isize) == 1 { " [x]" } else { "" })
        })
        .collect()
}

extern "C" fn menu_began(_this: Id, _sel: Sel, note: Id) {
    unsafe {
        let m = send!(note, c"object" => Id);
        let d = MENU_STACK.with(|st| {
            let mut st = st.borrow_mut();
            st.push(m as isize);
            st.len()
        });
        MENU_DEPTH.store(d as isize, Ordering::SeqCst);
        if d == 1 {
            MENU_ROOT.store(m as isize, Ordering::SeqCst);
            PRESSED.with(|p| p.set(None));
            crate::child::out(serde_json::json!({ "event": "menu", "open": true }));
            // un ordre arrive juste avant l'ouverture (son evenement deja dans la file, que le menu avale) : traite aussi
            perform(c"uwPump:");
        }
        if menu_log() {
            eprintln!("uwl-menu {:.3}: began depth {d} items {:?}", t_log(), item_titles(m));
        }
    }
}

extern "C" fn menu_ended(_this: Id, _sel: Sel, note: Id) {
    unsafe {
        let m = send!(note, c"object" => Id);
        // le menu et ceux ouverts apres lui (un sous-menu ferme avec son parent)
        let d = MENU_STACK.with(|st| {
            let mut st = st.borrow_mut();
            if let Some(i) = st.iter().position(|x| *x == m as isize) {
                st.truncate(i);
            }
            st.len()
        });
        MENU_DEPTH.store(d as isize, Ordering::SeqCst);
        if menu_log() {
            let h = send!(m, c"highlightedItem" => Id);
            eprintln!("uwl-menu {:.3}: ended depth {d} highlighted {:?}", t_log(), if h.is_null() { None } else { Some(utf8(send!(h, c"title" => Id))) });
        }
        if d == 0 {
            MENU_ROOT.store(0, Ordering::SeqCst);
            PRESSED.with(|p| p.set(None));
            crate::child::out(serde_json::json!({ "event": "menu", "open": false }));
            // la boucle de suivi a pu avaler nos evenements « ordre en attente » : un de plus, pour ce qui attend encore
            post(0, UiEv::Cmd);
        }
    }
}

/// Un article ou l'on peut aller au clavier : visible, actif, pas un separateur.
unsafe fn navigable(it: Id) -> bool {
    !it.is_null() && send!(it, c"isHidden" => i8) == 0 && send!(it, c"isSeparatorItem" => i8) == 0 && send!(it, c"isEnabled" => i8) != 0
}

unsafe fn contains(r: Rect, p: Pt) -> bool {
    p.x >= r.o.x && p.x < r.o.x + r.s.w && p.y >= r.o.y && p.y < r.o.y + r.s.h
}

/// Les menus ouverts, du menu du dessus au dernier sous-menu. Mesure du 20 nov. : un sous-menu ouvert ne poste pas de
/// NSMenuDidBeginTrackingNotification ; il est l'article en surbrillance de son parent, et son cadre d'accessibilite
/// n'est pas vide que s'il est a l'ecran.
/// Un sous-menu n'est ouvert que si une fenetre de menu visible a son cadre : un sous-menu ferme peut garder un cadre
/// non vide (Serum : 10 × 10 a l'origine, mesure du 20 nov.).
unsafe fn open_chain() -> Vec<Id> {
    let ws = send!(app(), c"windows" => Id);
    let n = if ws.is_null() { 0 } else { send!(ws, c"count" => usize).min(64) };
    let shown: Vec<Rect> = (0..n)
        .map(|i| send!(ws, c"objectAtIndex:", usize = i => Id))
        .filter(|w| send!(*w, c"isVisible" => i8) != 0 && send!(*w, c"level" => isize) >= 101)
        .map(|w| frame_of(w))
        .collect();
    let same = |a: Rect, b: Rect| (a.o.x - b.o.x).abs() < 2.0 && (a.o.y - b.o.y).abs() < 2.0 && (a.s.w - b.s.w).abs() < 2.0 && (a.s.h - b.s.h).abs() < 2.0;
    let mut out: Vec<Id> = Vec::new();
    let mut m = MENU_ROOT.load(Ordering::SeqCst) as Id;
    while !m.is_null() && out.len() < 8 {
        out.push(m);
        let h = send!(m, c"highlightedItem" => Id);
        if h.is_null() || send!(h, c"hasSubmenu" => i8) == 0 {
            break;
        }
        let sub = send!(h, c"submenu" => Id);
        if sub.is_null() {
            break;
        }
        let f = rect_send(sub, c"accessibilityFrame");
        if f.s.w <= 0.0 || f.s.h <= 0.0 || !shown.iter().any(|w| same(*w, f)) {
            break;
        }
        m = sub;
    }
    out
}

/// Le menu ouvert (sa place dans la chaine, open_chain) et l'article sous le point `p` (ecran, Cocoa) : le plus recent d'abord (un
/// sous-menu est au-dessus de son parent). (place, menu, Some(indice)) : sur un article ; None pour l'indice : sur le menu
/// mais pas sur un article (un separateur, une marge). None : hors des menus.
unsafe fn menu_hit(p: Pt) -> Option<(usize, Id, Option<isize>)> {
    let st = open_chain();
    for (k, m) in st.iter().enumerate().rev() {
        let m = *m;
        if !contains(rect_send(m, c"accessibilityFrame"), p) {
            continue;
        }
        let n = send!(m, c"numberOfItems" => isize).clamp(0, 4096);
        for i in 0..n {
            let it = send!(m, c"itemAtIndex:", isize = i => Id);
            if !it.is_null() && send!(it, c"isHidden" => i8) == 0 && contains(rect_send(it, c"accessibilityFrame"), p) {
                return Some((k, m, Some(i)));
            }
        }
        return Some((k, m, None));
    }
    None
}

/// Les touches qui vont, depuis l'article en surbrillance de `m`, a l'article `target` (les fleches sautent ce qui n'est
/// pas navigable) : (code de touche, caractere), sans la touche finale.
unsafe fn keys_to(m: Id, target: isize) -> Vec<(u16, char)> {
    let n = send!(m, c"numberOfItems" => isize).clamp(0, 4096);
    let nav: Vec<isize> = (0..n).filter(|i| navigable(send!(m, c"itemAtIndex:", isize = *i => Id))).collect();
    let Some(t) = nav.iter().position(|i| *i == target) else { return Vec::new() };
    let h = send!(m, c"highlightedItem" => Id);
    let cur = if h.is_null() { None } else { nav.iter().position(|i| *i == send!(m, c"indexOfItem:", Id = h => isize)) };
    let (down, up) = ((125u16, '\u{f701}'), (126u16, '\u{f700}'));
    match cur {
        None => vec![down; t + 1],
        Some(c) if t >= c => vec![down; t - c],
        Some(c) => vec![up; c - t],
    }
}

/// (fil principal) poser une touche (appui et relachement) pour notre fenetre : la boucle de suivi du menu la lit
unsafe fn post_key_main(kc: u16, ch: char) {
    let wn = WIN_NUM.load(Ordering::Relaxed);
    let now = send!(send!(class(c"NSProcessInfo"), c"processInfo" => Id), c"systemUptime" => f64);
    let s = nsstring(&ch.to_string());
    for ty in [10usize, 11] {
        let e = send!(class(c"NSEvent"), c"keyEventWithType:location:modifierFlags:timestamp:windowNumber:context:characters:charactersIgnoringModifiers:isARepeat:keyCode:",
            usize = ty, Pt = Pt { x: 0.0, y: 0.0 }, usize = 0, f64 = now, isize = wn, Id = std::ptr::null_mut(), Id = s, Id = s, i8 = 0, u16 = kc => Id);
        if !e.is_null() {
            send!(app(), c"postEvent:atStart:", Id = e, i8 = 0 => ());
        }
    }
}

/// (fil principal, par performSelector) les gestes de souris arrives pendant un menu (voir l'en-tete des menus)
extern "C" fn menu_mouse(_this: Id, _sel: Sel, _arg: Id) {
    loop {
        let Some((ty, p, flags, clicks)) = MENU_MOUSE.lock().unwrap().pop_front() else { break };
        unsafe {
            let pool = objc_autoreleasePoolPush();
            menu_mouse_one(ty, p, flags, clicks);
            objc_autoreleasePoolPop(pool);
        }
    }
}

unsafe fn menu_mouse_one(ty: usize, p: Pt, flags: usize, clicks: isize) {
    let down = matches!(ty, 1 | 3 | 25);
    let up = matches!(ty, 2 | 4 | 26);
    if !menu_open() {
        // le menu s'est ferme entre-temps : le geste va a la fenetre, comme les autres (le point, relatif a elle)
        let [fx, fy, _, _] = *FRAME.lock().unwrap();
        post_mouse_main(ty, Pt { x: p.x - fx, y: p.y - fy }, flags, clicks);
        return;
    }
    let hit = menu_hit(p);
    if menu_log() && (down || up) {
        eprintln!("uwl-menu {:.3}: mouse ty {ty} at {:.0},{:.0} → {:?}", t_log(), p.x, p.y, hit.map(|(k, _, i)| (k, i)));
    }
    let Some((k, m, item)) = hit else {
        if down {
            // hors des menus : le clic les ferme (pose tel quel, la boucle de suivi le prend pour un clic a cote)
            PRESSED.with(|c| c.set(None));
            let [fx, fy, _, _] = *FRAME.lock().unwrap();
            post_mouse_main(ty, Pt { x: p.x - fx, y: p.y - fy }, flags, clicks);
        }
        return;
    };
    let Some(i) = item else { return };
    let it = send!(m, c"itemAtIndex:", isize = i => Id);
    if !navigable(it) {
        return; // un article inactif : rien, le menu reste ouvert (comme sur le Mac)
    }
    let has_sub = send!(it, c"hasSubmenu" => i8) != 0;
    if down {
        PRESSED.with(|c| c.set(Some((m as isize, i))));
    }
    // un sous-menu s'ouvre des l'appui ; un article est choisi au relachement, s'il a ete appuye (pas le relachement du
    // clic qui a ouvert le menu)
    let go = if has_sub { down } else { up && PRESSED.with(|c| c.get()) == Some((m as isize, i)) };
    if !go {
        return;
    }
    PRESSED.with(|c| c.set(None));
    let depth = open_chain().len();
    // fermer les sous-menus ouverts au-dela du menu vise (←), aller a l'article, puis Entree (ou → : son sous-menu)
    let mut keys: Vec<(u16, char)> = vec![(123, '\u{f702}'); depth.saturating_sub(k + 1)];
    keys.extend(keys_to(m, i));
    keys.push(if has_sub { (124, '\u{f703}') } else { (36, '\r') });
    if menu_log() {
        eprintln!("uwl-menu {:.3}: keys {:?}", t_log(), keys.iter().map(|k| k.0).collect::<Vec<_>>());
    }
    for (kc, ch) in keys {
        post_key_main(kc, ch);
    }
}

/// (fil principal) poser un geste de souris pour notre fenetre (le point dans la fenetre, Cocoa : bas a gauche)
unsafe fn post_mouse_main(ty: usize, loc: Pt, flags: usize, clicks: isize) {
    let wn = WIN_NUM.load(Ordering::Relaxed);
    let now = send!(send!(class(c"NSProcessInfo"), c"processInfo" => Id), c"systemUptime" => f64);
    let pressure: f32 = if matches!(ty, 1 | 3 | 25 | 6 | 7 | 27) { 1.0 } else { 0.0 };
    let e = send!(class(c"NSEvent"), c"mouseEventWithType:location:modifierFlags:timestamp:windowNumber:context:eventNumber:clickCount:pressure:",
        usize = ty, Pt = loc, usize = flags, f64 = now, isize = wn, Id = std::ptr::null_mut(), isize = 0, isize = clicks, f32 = pressure => Id);
    if !e.is_null() {
        send!(app(), c"postEvent:atStart:", Id = e, i8 = 0 => ());
    }
}

extern "C" fn menu_action(_this: Id, _sel: Sel, note: Id) {
    if !menu_log() {
        return;
    }
    unsafe {
        let info = send!(note, c"userInfo" => Id);
        let item = if info.is_null() { std::ptr::null_mut() } else { send!(info, c"objectForKey:", Id = nsstring("MenuItem") => Id) };
        let title = if item.is_null() { String::new() } else { utf8(send!(item, c"title" => Id)) };
        eprintln!("uwl-menu {:.3}: action {title:?}", t_log());
    }
}

extern "C" fn menu_item_chosen(_this: Id, _sel: Sel, item: Id) {
    unsafe { eprintln!("uwl-menu {:.3}: chosen {:?}", t_log(), utf8(send!(item, c"title" => Id))) };
}

/// (20 nov., l'essai, seulement avec UWL_HOST_MENU_LOG=1) un menu d'essai a nous, ouvert dans la fenetre au point
/// (x, y) (points, haut a gauche de la fenetre) : ses articles disent quand ils sont choisis (le temoin des essais).
pub fn test_menu(_w: Win, x: f64, y: f64) {
    if !menu_log() {
        return;
    }
    // ouvert depuis la boucle (un performSelector qu'elle fait tourner), comme un plugin l'ouvre dans sendEvent:
    *TEST_AT.lock().unwrap() = (x, y);
    perform(c"uwTestMenu:");
}

static TEST_AT: std::sync::Mutex<(f64, f64)> = std::sync::Mutex::new((0.0, 0.0));

extern "C" fn test_menu_now(_this: Id, _sel: Sel, _arg: Id) {
    let w = WIN_PTR.load(Ordering::Relaxed);
    let (x, y) = *TEST_AT.lock().unwrap();
    if w == 0 || menu_open() {
        return;
    }
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let m = send!(send!(class(c"NSMenu"), c"alloc" => Id), c"initWithTitle:", Id = nsstring("uwl test") => Id);
        send!(m, c"setAutoenablesItems:", i8 = 0 => ());
        let watch = WATCH.load(Ordering::SeqCst) as Id;
        // un article inactif, un separateur, un sous-menu : ce que les fleches sautent ou ouvrent
        let add = |m: Id, t: &str| {
            let it = send!(m, c"addItemWithTitle:action:keyEquivalent:", Id = nsstring(t), Sel = sel_registerName(c"uwMenuItem:".as_ptr()), Id = nsstring("") => Id);
            send!(it, c"setTarget:", Id = watch => ());
            it
        };
        add(m, "Alpha");
        send!(add(m, "Beta"), c"setEnabled:", i8 = 0 => ());
        send!(m, c"addItem:", Id = send!(class(c"NSMenuItem"), c"separatorItem" => Id) => ());
        add(m, "Gamma");
        let more = add(m, "More");
        let sub = send!(send!(class(c"NSMenu"), c"alloc" => Id), c"initWithTitle:", Id = nsstring("More") => Id);
        send!(sub, c"setAutoenablesItems:", i8 = 0 => ());
        // un sous-menu dans le sous-menu, en premier : → le met en surbrillance sans l'ouvrir
        let deep = add(sub, "Deep");
        let dsub = send!(send!(class(c"NSMenu"), c"alloc" => Id), c"initWithTitle:", Id = nsstring("Deep") => Id);
        add(dsub, "P");
        add(dsub, "Q");
        send!(deep, c"setSubmenu:", Id = dsub => ());
        add(sub, "One");
        add(sub, "Two");
        add(sub, "Three");
        send!(more, c"setSubmenu:", Id = sub => ());
        add(m, "Delta");
        let content = send!(w as Id, c"contentView" => Id);
        let fh = FRAME.lock().unwrap()[3];
        let r = send!(m, c"popUpMenuPositioningItem:atLocation:inView:", Id = std::ptr::null_mut(), Pt = Pt { x, y: fh - y }, Id = content => i8);
        eprintln!("uwl-menu {:.3}: test menu returned {r}", t_log());
        objc_autoreleasePoolPop(pool);
    }
}

/// [objet sel] → une NSRect renvoyee (voir frame_of)
unsafe fn rect_send(o: Id, sel: &CStr) -> Rect {
    #[cfg(target_arch = "x86_64")]
    {
        extern "C" {
            fn objc_msgSend_stret();
        }
        let f: unsafe extern "C" fn(Id, Sel) -> Rect = std::mem::transmute(objc_msgSend_stret as unsafe extern "C" fn());
        f(o, sel_registerName(sel.as_ptr()))
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        send!(o, sel => Rect)
    }
}

unsafe fn responds(o: Id, sel: &CStr) -> bool {
    send!(o, c"respondsToSelector:", Sel = sel_registerName(sel.as_ptr()) => i8) != 0
}

unsafe fn ax_dump(o: Id, depth: usize, out: &mut Vec<String>) {
    if o.is_null() || depth > 6 || out.len() > 80 {
        return;
    }
    let cls = utf8(send!(send!(o, c"className" => Id), c"description" => Id));
    let role = if responds(o, c"accessibilityRole") { utf8(send!(o, c"accessibilityRole" => Id)) } else { String::new() };
    let title = if responds(o, c"accessibilityTitle") { utf8(send!(o, c"accessibilityTitle" => Id)) } else { String::new() };
    let f = if responds(o, c"accessibilityFrame") { rect_send(o, c"accessibilityFrame") } else { Rect { o: Pt { x: 0.0, y: 0.0 }, s: Sz { w: 0.0, h: 0.0 } } };
    out.push(format!("{}{cls} role {role:?} title {title:?} frame {:.0},{:.0} {:.0}x{:.0}", "  ".repeat(depth), f.o.x, f.o.y, f.s.w, f.s.h));
    if responds(o, c"accessibilityChildren") {
        let ch = send!(o, c"accessibilityChildren" => Id);
        if !ch.is_null() {
            let n = send!(ch, c"count" => usize).min(40);
            for i in 0..n {
                ax_dump(send!(ch, c"objectAtIndex:", usize = i => Id), depth + 1, out);
            }
        }
    }
}

/// (20 nov., l'essai) les fenetres de l'appli et l'arbre d'accessibilite (interne : nos propres objets) des menus
extern "C" fn menu_probe(_this: Id, _sel: Sel, _arg: Id) {
    unsafe {
        let ws = send!(app(), c"windows" => Id);
        let n = send!(ws, c"count" => usize);
        let mut out = Vec::new();
        for i in 0..n {
            let w = send!(ws, c"objectAtIndex:", usize = i => Id);
            let f = frame_of(w);
            out.push(format!("window #{} {} level {} visible {} frame {:.0},{:.0} {:.0}x{:.0}", send!(w, c"windowNumber" => isize), utf8(send!(send!(w, c"className" => Id), c"description" => Id)),
                send!(w, c"level" => isize), send!(w, c"isVisible" => i8), f.o.x, f.o.y, f.s.w, f.s.h));
            if send!(w, c"level" => isize) >= 101 {
                ax_dump(w, 1, &mut out);
            }
        }
        let m = MENU_ROOT.load(Ordering::SeqCst) as Id;
        if !m.is_null() {
            out.push("menu object:".into());
            ax_dump(m, 1, &mut out);
        }
        eprintln!("uwl-menu {:.3}: probe
{}", t_log(), out.join("
"));
    }
}

extern "C" {
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

/// (20 nov., l'essai) les images de nos fenetres (l'editeur et les menus ouverts) en PNG dans UWL_HOST_SNAP_DIR : un
/// processus filme ses propres fenetres sans l'autorisation « Enregistrement de l'ecran » (les essais par ssh).
extern "C" fn menu_snap(_this: Id, _sel: Sel, _arg: Id) {
    let Ok(dir) = std::env::var("UWL_HOST_SNAP_DIR") else { return };
    type CreateImage = unsafe extern "C" fn(Rect, u32, u32, u32) -> *mut c_void;
    unsafe {
        let p = dlsym(-2isize as *mut c_void, c"CGWindowListCreateImage".as_ptr()); // RTLD_DEFAULT
        if p.is_null() {
            eprintln!("uwl-menu {:.3}: snap: no CGWindowListCreateImage", t_log());
            return;
        }
        let f: CreateImage = std::mem::transmute(p);
        let ws = send!(app(), c"windows" => Id);
        let n = send!(ws, c"count" => usize);
        let k = SNAPS.fetch_add(1, Ordering::SeqCst);
        for i in 0..n {
            let w = send!(ws, c"objectAtIndex:", usize = i => Id);
            if send!(w, c"isVisible" => i8) == 0 {
                continue;
            }
            let num = send!(w, c"windowNumber" => isize);
            let null = Rect { o: Pt { x: f64::INFINITY, y: f64::INFINITY }, s: Sz { w: 0.0, h: 0.0 } };
            // kCGWindowListOptionIncludingWindow, kCGWindowImageBoundsIgnoreFraming | kCGWindowImageBestResolution
            let img = f(null, 1 << 3, num as u32, 1 | 8);
            if img.is_null() {
                eprintln!("uwl-menu {:.3}: snap #{num}: no image", t_log());
                continue;
            }
            let rep = send!(send!(class(c"NSBitmapImageRep"), c"alloc" => Id), c"initWithCGImage:", *mut c_void = img => Id);
            let dict = send!(class(c"NSDictionary"), c"dictionary" => Id);
            let data = send!(rep, c"representationUsingType:properties:", usize = 4, Id = dict => Id); // NSBitmapImageFileTypePNG
            let path = format!("{dir}/snap{k}-w{num}-level{}.png", send!(w, c"level" => isize));
            let ok = !data.is_null() && send!(data, c"writeToFile:atomically:", Id = nsstring(&path), i8 = 1 => i8) != 0;
            eprintln!("uwl-menu {:.3}: snap #{num} {}x{} → {path} {ok}", t_log(), send!(rep, c"pixelsWide" => isize), send!(rep, c"pixelsHigh" => isize));
            CFRelease(img);
        }
    }
}

static SNAPS: AtomicIsize = AtomicIsize::new(0);

/// Depuis n'importe quel fil (essai) : photographier nos fenetres (menu_snap).
pub fn snap() {
    if menu_log() {
        perform(c"uwSnap:");
    }
}

/// Depuis n'importe quel fil (essai) : sonder les menus ouverts.
pub fn probe_menus() {
    if menu_log() && menu_open() {
        perform(c"uwProbe:");
    }
}

/// (fil principal, par performSelector) fermer le menu ouvert : le menu du dessus et ses sous-menus
extern "C" fn menu_cancel(_this: Id, _sel: Sel, _arg: Id) {
    let m = MENU_ROOT.load(Ordering::SeqCst) as Id;
    if !m.is_null() {
        if menu_log() {
            eprintln!("uwl-menu {:.3}: cancelTracking", t_log());
        }
        unsafe { send!(m, c"cancelTracking" => ()) };
    }
}

/// (fil principal, par performSelector, pendant un menu) les ordres en attente : IN_MENU (child.rs)
extern "C" fn menu_pump(_this: Id, _sel: Sel, _arg: Id) {
    if !menu_open() || PUMPING.with(|p| p.replace(true)) {
        return;
    }
    if let Some(f) = IN_MENU.with(|c| c.get()) {
        unsafe { (*f)() };
    }
    PUMPING.with(|p| p.set(false));
}

unsafe fn watch_menus() {
    let cls = objc_allocateClassPair(class(c"NSObject"), c"UppwindPlugHostMenuWatch".as_ptr(), 0);
    let cls = if cls.is_null() {
        class(c"UppwindPlugHostMenuWatch")
    } else {
        for (name, imp) in [
            (c"uwMenuBegan:", menu_began as *const c_void),
            (c"uwMenuEnded:", menu_ended as *const c_void),
            (c"uwMenuAction:", menu_action as *const c_void),
            (c"uwCancelMenu:", menu_cancel as *const c_void),
            (c"uwPump:", menu_pump as *const c_void),
            (c"uwMenuItem:", menu_item_chosen as *const c_void),
            (c"uwTestMenu:", test_menu_now as *const c_void),
            (c"uwProbe:", menu_probe as *const c_void),
            (c"uwMenuMouse:", menu_mouse as *const c_void),
            (c"uwKey:", key_now as *const c_void),
            (c"uwSnap:", menu_snap as *const c_void),
        ] {
            class_addMethod(cls, sel_registerName(name.as_ptr()), imp, c"v@:@".as_ptr());
        }
        objc_registerClassPair(cls);
        cls
    };
    let w = send!(send!(cls, c"alloc" => Id), c"init" => Id);
    if w.is_null() {
        return;
    }
    let nc = send!(class(c"NSNotificationCenter"), c"defaultCenter" => Id);
    for (sel, name) in [
        (c"uwMenuBegan:", NSMenuDidBeginTrackingNotification),
        (c"uwMenuEnded:", NSMenuDidEndTrackingNotification),
        (c"uwMenuAction:", NSMenuDidSendActionNotification),
    ] {
        send!(nc, c"addObserver:selector:name:object:", Id = w, Sel = sel_registerName(sel.as_ptr()), Id = name, Id = std::ptr::null_mut() => ());
    }
    WATCH.store(w as isize, Ordering::SeqCst);
}

/// Depuis n'importe quel fil : `sel` sur l'objet des menus, dans le fil principal, dans les modes « communs » (ceux
/// que tourne aussi la boucle de suivi d'un menu).
fn perform(sel: &CStr) {
    let w = WATCH.load(Ordering::SeqCst) as Id;
    if w.is_null() {
        return;
    }
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let modes = send!(class(c"NSArray"), c"arrayWithObject:", Id = NSRunLoopCommonModes => Id);
        send!(w, c"performSelectorOnMainThread:withObject:waitUntilDone:modes:", Sel = sel_registerName(sel.as_ptr()), Id = std::ptr::null_mut(), i8 = 0, Id = modes => ());
        objc_autoreleasePoolPop(pool);
    }
}

/// Depuis n'importe quel fil : fermer le menu ouvert du plugin (rien sans menu).
pub fn cancel_menu() {
    if menu_open() {
        perform(c"uwCancelMenu:");
    }
}

/// windowShouldClose: — la croix : la fenetre se range (la boucle l'apprend), elle ne se ferme jamais.
extern "C" fn should_close(_this: Id, _sel: Sel, _sender: Id) -> i8 {
    post(0, UiEv::HideEditor);
    0
}

unsafe fn delegate() -> Id {
    static ONCE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let cls = *ONCE.get_or_init(|| {
        let c = objc_allocateClassPair(class(c"NSObject"), c"UppwindPlugHostWindowDelegate".as_ptr(), 0);
        if c.is_null() {
            return class(c"UppwindPlugHostWindowDelegate") as usize;
        }
        class_addMethod(c, sel_registerName(c"windowShouldClose:".as_ptr()), should_close as *const c_void, c"c@:@".as_ptr());
        objc_registerClassPair(c);
        c as usize
    }) as Id;
    let d = send!(cls, c"alloc" => Id);
    send!(d, c"init" => Id)
}

pub fn make_window(title: &str) -> Result<Win, String> {
    unsafe {
        let pool = objc_autoreleasePoolPush();
        // titre et croix ; ni reduction (un clic du studio sur le bouton jaune, dans la barre de titre filmee, rangerait la
        // fenetre dans le Dock), ni « redimensionnable » (le plugin donne sa taille : resizeView)
        let style: usize = 1 | 2;
        let w = send!(class(c"NSWindow"), c"alloc" => Id);
        let r = Rect { o: Pt { x: 120.0, y: 120.0 }, s: Sz { w: 800.0, h: 500.0 } };
        // NSBackingStoreBuffered = 2 ; defer NO : le numero de fenetre (celui que le compagnon filme) existe tout de suite
        let w = send!(w, c"initWithContentRect:styleMask:backing:defer:", Rect = r, usize = style, usize = 2, i8 = 0 => Id);
        if w.is_null() {
            objc_autoreleasePoolPop(pool);
            return Err("NSWindow could not be created".into());
        }
        send!(w, c"setReleasedWhenClosed:", i8 = 0 => ());
        send!(w, c"setTitle:", Id = nsstring(title) => ());
        send!(w, c"setDelegate:", Id = delegate() => ());
        // ni dans le menu Fenetre, ni dans la rotation Cmd-`
        send!(w, c"setExcludedFromWindowsMenu:", i8 = 1 => ());
        // le survol (les plugins qui eclairent ce qui est sous le pointeur) : les deplacements sans bouton (gesture)
        send!(w, c"setAcceptsMouseMovedEvents:", i8 = 1 => ());
        objc_autoreleasePoolPop(pool);
        refresh(w as Win);
        Ok(w as Win)
    }
}

/// Ranger la fenetre (le studio la filme, toi tu ne la vois pas) : voir UWL_HOST_PARK.
pub fn park(w: Win) {
    let w = w as Id;
    unsafe {
        match park_mode() {
            Park::Offscreen => {
                send!(w, c"setAlphaValue:", f64 = 1.0 => ());
                send!(w, c"setIgnoresMouseEvents:", i8 = 0 => ());
                send!(w, c"setFrameOrigin:", Pt = Pt { x: -30000.0, y: -30000.0 } => ());
            }
            Park::Alpha => {
                send!(w, c"setIgnoresMouseEvents:", i8 = 1 => ());
                send!(w, c"setAlphaValue:", f64 = 0.0 => ());
                send!(w, c"setFrameOrigin:", Pt = Pt { x: 120.0, y: 120.0 } => ());
            }
            Park::Behind => {
                send!(w, c"setAlphaValue:", f64 = 1.0 => ());
                send!(w, c"setIgnoresMouseEvents:", i8 = 0 => ());
                send!(w, c"setFrameOrigin:", Pt = Pt { x: 120.0, y: 120.0 } => ());
            }
        }
        send!(w, c"orderBack:", Id = std::ptr::null_mut() => ());
        // (20 nov., relecture macOS) montree puis rangee, l'appli de l'enfant restait au premier plan : ce que la
        // personne tapait ensuite allait a l'editeur cache. Rangee, elle rend la main.
        if send!(app(), c"isActive" => i8) != 0 {
            send!(app(), c"deactivate" => ());
        }
    }
    refresh(w as Win);
}

/// Le numero de la fenetre pour le serveur de fenetres (CGWindowID) : celui que le compagnon filme (CGWindowList).
pub fn window_id(w: Win) -> u64 {
    unsafe { send!(w as Id, c"windowNumber" => isize).max(0) as u64 }
}

pub fn show_on_desktop(w: Win, show: bool) {
    unsafe {
        if show {
            let win = w as Id;
            send!(win, c"setAlphaValue:", f64 = 1.0 => ());
            send!(win, c"setIgnoresMouseEvents:", i8 = 0 => ());
            send!(win, c"center" => ());
            send!(app(), c"activateIgnoringOtherApps:", i8 = 1 => ());
            send!(win, c"makeKeyAndOrderFront:", Id = std::ptr::null_mut() => ());
            refresh(w);
        } else {
            park(w);
        }
    }
}

/// Depuis n'importe quel fil : un evenement pour la boucle (postEvent:atStart: est sur depuis un autre fil).
/// (20 nov.) Pendant un menu du plugin : un ordre passe aussi par uwPump: (la boucle de suivi du menu le fait tourner),
/// la fin ferme d'abord le menu.
pub fn post(_w: Win, ev: UiEv) {
    if menu_open() {
        match ev {
            UiEv::Cmd => perform(c"uwPump:"),
            UiEv::Quit => cancel_menu(),
            UiEv::HideEditor => {}
        }
    }
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let e = send!(class(c"NSEvent"), c"otherEventWithType:location:modifierFlags:timestamp:windowNumber:context:subtype:data1:data2:",
            usize = APP_DEFINED, Pt = Pt { x: 0.0, y: 0.0 }, usize = 0, f64 = 0.0, isize = 0, Id = std::ptr::null_mut(), i16 = 0, isize = ev_code(ev), isize = 0 => Id);
        if !e.is_null() {
            send!(app(), c"postEvent:atStart:", Id = e, i8 = 0 => ());
        }
        objc_autoreleasePoolPop(pool);
    }
}

/// La boucle : les evenements d'AppKit passent (sendEvent:), les notres reviennent. (20 nov.) `in_menu` : ce qu'il faut
/// faire des ordres en attente quand un menu du plugin tient la boucle (appele par uwPump:, dans le fil principal).
pub fn next(in_menu: &mut dyn FnMut()) -> Option<UiEv> {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            IN_MENU.with(|c| c.set(None));
        }
    }
    // seulement pendant cet appel (Reset l'efface au retour) : uwPump: ne le trouve que la, dans le meme fil
    let f: *mut (dyn FnMut() + '_) = in_menu;
    IN_MENU.with(|c| c.set(Some(unsafe { std::mem::transmute::<*mut (dyn FnMut() + '_), *mut (dyn FnMut() + 'static)>(f) })));
    let _reset = Reset;
    unsafe {
        let a = app();
        let far = send!(class(c"NSDate"), c"distantFuture" => Id);
        loop {
            let pool = objc_autoreleasePoolPush();
            let e = send!(a, c"nextEventMatchingMask:untilDate:inMode:dequeue:", u64 = u64::MAX, Id = far, Id = NSDefaultRunLoopMode, i8 = 1 => Id);
            if e.is_null() {
                objc_autoreleasePoolPop(pool);
                continue;
            }
            if send!(e, c"type" => usize) == APP_DEFINED {
                let code = send!(e, c"data1" => isize);
                objc_autoreleasePoolPop(pool);
                match code {
                    1 => return Some(UiEv::Cmd),
                    2 => return Some(UiEv::Quit),
                    3 => return Some(UiEv::HideEditor),
                    _ => continue,
                }
            }
            // (20 nov.) un appui ou une touche pour notre fenetre (un geste du studio, gesture) : elle devient la fenetre
            // principale de l'enfant d'abord, sans passer devant ni activer l'appli (AppKit donne les touches a celle-la)
            let ty = send!(e, c"type" => usize);
            if matches!(ty, 1 | 3 | 25 | 10) {
                let w = WIN_PTR.load(Ordering::Relaxed) as Id;
                if !w.is_null() && send!(e, c"windowNumber" => isize) == WIN_NUM.load(Ordering::Relaxed) && send!(w, c"isKeyWindow" => i8) == 0 {
                    send!(w, c"makeKeyWindow" => ());
                }
            }
            send!(a, c"sendEvent:", Id = e => ());
            send!(a, c"updateWindows" => ());
            objc_autoreleasePoolPop(pool);
        }
    }
}

pub fn destroy(w: Win) {
    unsafe {
        let win = w as Id;
        send!(win, c"setDelegate:", Id = std::ptr::null_mut() => ());
        send!(win, c"orderOut:", Id = std::ptr::null_mut() => ());
        send!(win, c"close" => ());
    }
}

/// La taille du contenu de la fenetre (les points de l'editeur).
pub fn size_window(w: isize, cw: i32, ch: i32) {
    if w == 0 {
        return;
    }
    unsafe { send!(w as Id, c"setContentSize:", Sz = Sz { w: cw as f64, h: ch as f64 } => ()) };
    refresh(w);
}

/// Ouvre l'editeur dans la contentView de la fenetre (rangee) : createView, le cadre, attached(« NSView »), la taille.
/// Pas de setContentScaleFactor sur Mac (le SDK le reserve a Windows et Linux : le plugin lit l'echelle de sa vue).
pub fn open_view(inst: &Instance, w: Win, frame: &ComWrapper<Frame>) -> Result<(ComPtr<IPlugView>, i32, i32), String> {
    let c = inst.ctrl.as_ref().ok_or("the plugin has no editor")?;
    unsafe {
        let v = ComPtr::from_raw(c.createView(ViewType::kEditor)).ok_or("the plugin has no editor")?;
        if v.isPlatformTypeSupported(kPlatformTypeNSView) != kResultTrue {
            return Err("the editor does not support NSView".into());
        }
        let fp = frame.to_com_ptr::<IPlugFrame>().ok_or("frame")?;
        v.setFrame(fp.as_ptr());
        let mut r: ViewRect = std::mem::zeroed();
        v.getSize(&mut r);
        size_window(w, (r.right - r.left).max(200), (r.bottom - r.top).max(100));
        let content = send!(w as Id, c"contentView" => Id);
        if content.is_null() || v.attached(content, kPlatformTypeNSView) != kResultOk {
            v.setFrame(std::ptr::null_mut());
            return Err("the editor did not attach".into());
        }
        crate::child::VIEW.store(v.as_ptr(), Ordering::Relaxed);
        let mut r: ViewRect = std::mem::zeroed();
        v.getSize(&mut r);
        let (w2, h2) = (r.right - r.left, r.bottom - r.top);
        if w2 > 0 && h2 > 0 {
            size_window(w, w2, h2);
            let mut r2 = ViewRect { left: 0, top: 0, right: w2, bottom: h2 };
            v.onSize(&mut r2);
        }
        Ok((v, w2, h2))
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventCreateScrollWheelEvent2(src: *const c_void, units: u32, count: u32, w1: i32, w2: i32, w3: i32) -> *mut c_void;
    fn CGEventSetLocation(e: *mut c_void, p: Pt);
    fn CGEventSetFlags(e: *mut c_void, f: u64);
    fn CGEventSetIntegerValueField(e: *mut c_void, field: u32, v: i64);
    fn CGMainDisplayID() -> u32;
    fn CGDisplayBounds(d: u32) -> Rect;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(o: *const c_void);
}

/// [objet frame] : une NSRect RENVOYEE (32 octets) — sur x86_64 par objc_msgSend_stret, sur arm64 par objc_msgSend.
unsafe fn frame_of(w: Id) -> Rect {
    #[cfg(target_arch = "x86_64")]
    {
        extern "C" {
            fn objc_msgSend_stret();
        }
        let f: unsafe extern "C" fn(Id, Sel) -> Rect = std::mem::transmute(objc_msgSend_stret as unsafe extern "C" fn());
        f(w, sel_registerName(c"frame".as_ptr()))
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        send!(w, c"frame" => Rect)
    }
}

/// (20 nov., l'essai avec Lucas) La hauteur de la barre de titre de la fenetre, en points : le compagnon la retire de
/// l'image filmee (le studio montre l'editeur seul, sans un second titre ni les trois boutons de macOS) et l'ajoute aux
/// gestes.
pub fn titlebar(w: Win) -> f64 {
    if w == 0 {
        return 0.0;
    }
    unsafe {
        let win = w as Id;
        let content = send!(win, c"contentView" => Id);
        if content.is_null() {
            return 0.0;
        }
        (frame_of(win).s.h - frame_of(content).s.h).clamp(0.0, 200.0)
    }
}

/// (20 nov., l'essai) Un geste du studio remis a la fenetre par l'enfant lui-meme. Mesure du 20 nov. : un clic poste
/// au processus (CGEventPostToPid, la voie de Live) ne tourne aucun bouton de Diva dans une fenetre rangee derriere les
/// autres. Ici l'evenement est construit pour CETTE fenetre (son numero, le point dans la fenetre) et pose dans la file
/// d'AppKit (postEvent:atStart:), comme une vraie souris : la boucle le distribue (sendEvent:), et un bouton qui suit la
/// souris dans sa propre boucle (Diva : nextEventMatchingMask dans mouseDown:) recoit les deplacements qui suivent.
/// Appele par le fil qui lit l'entree (jamais par la boucle de l'interface, qui peut etre dans celle du bouton) ; il ne
/// lit que FRAME et WIN_NUM. Quelle que soit la place de la fenetre, et sans l'autorisation Accessibilite.
/// Les ordres (le compagnon, input.rs, les a deja traduits ; les points en points, l'origine en haut a gauche de la
/// fenetre, barre de titre comprise, comme l'image filmee) :
///   {"cmd":"mouse","ty":<NSEventType>,"x","y","clicks","flags"} · {"cmd":"wheel","x","y","dx","dy","flags"}
///   · {"cmd":"key","kc":<kVK>,"down":bool,"text":"…","flags"}
pub fn gesture(v: &serde_json::Value) {
    let wn = WIN_NUM.load(Ordering::Relaxed);
    if wn <= 0 {
        return;
    }
    let [fx, fy, _fw, fh] = *FRAME.lock().unwrap();
    let num = |k: &str| v[k].as_f64().filter(|x| x.is_finite()).unwrap_or(0.0);
    if menu_log() {
        eprintln!("uwl-menu {:.3}: gesture {} ty {} menu open {}", t_log(), v["cmd"], v["ty"], menu_open());
    }
    unsafe {
        let pool = objc_autoreleasePoolPush();
        // le point de l'image (haut a gauche) → la fenetre (bas a gauche)
        let loc = Pt { x: num("x").clamp(-16384.0, 16384.0), y: fh - num("y").clamp(-16384.0, 16384.0) };
        // (20 nov., relecture macOS) seul Maj passe (Cmd, Ctrl, Option : refuses deja par le compagnon, ici aussi)
        let flags = v["flags"].as_u64().unwrap_or(0) & 0x0002_0000;
        let now = send!(send!(class(c"NSProcessInfo"), c"processInfo" => Id), c"systemUptime" => f64);
        let e: Id = match v["cmd"].as_str().unwrap_or("") {
            "mouse" => {
                let ty = v["ty"].as_u64().unwrap_or(0) as usize;
                // seulement les genres souris : appuis, relachements, deplacements, glisser
                if ![1, 2, 3, 4, 5, 6, 7, 25, 26, 27].contains(&ty) {
                    objc_autoreleasePoolPop(pool);
                    return;
                }
                let clicks = v["clicks"].as_i64().unwrap_or(0).clamp(0, 3) as isize;
                let pressure: f32 = if matches!(ty, 1 | 3 | 25 | 6 | 7 | 27) { 1.0 } else { 0.0 };
                // (20 nov.) pendant un menu du plugin : au fil principal (uwMenuMouse:), qui sait ou sont ses articles
                if menu_open() {
                    MENU_MOUSE.lock().unwrap().push_back((ty, Pt { x: fx + loc.x, y: fy + loc.y }, flags as usize, clicks));
                    perform(c"uwMenuMouse:");
                    objc_autoreleasePoolPop(pool);
                    return;
                }
                send!(class(c"NSEvent"), c"mouseEventWithType:location:modifierFlags:timestamp:windowNumber:context:eventNumber:clickCount:pressure:",
                    usize = ty, Pt = loc, usize = flags as usize, f64 = now, isize = wn, Id = std::ptr::null_mut(), isize = 0, isize = clicks, f32 = pressure => Id)
            }
            "wheel" => {
                let c = |k: &str| num(k).round().clamp(-10_000.0, 10_000.0) as i32;
                let ce = CGEventCreateScrollWheelEvent2(std::ptr::null(), 0, 2, c("dy"), c("dx"), 0);
                if ce.is_null() {
                    objc_autoreleasePoolPop(pool);
                    return;
                }
                // l'ecran : le point de la fenetre en coordonnees Cocoa (bas a gauche de l'ecran principal), puis
                // CoreGraphics (haut a gauche)
                let main_h = CGDisplayBounds(CGMainDisplayID()).s.h;
                CGEventSetLocation(ce, Pt { x: fx + loc.x, y: main_h - (fy + loc.y) });
                CGEventSetFlags(ce, flags);
                // le numero de la fenetre sous le pointeur (kCGMouseEventWindowUnderMousePointer et …ThatCanHandleThisEvent)
                CGEventSetIntegerValueField(ce, 91, wn as i64);
                CGEventSetIntegerValueField(ce, 92, wn as i64);
                let e = send!(class(c"NSEvent"), c"eventWithCGEvent:", *mut c_void = ce => Id);
                CFRelease(ce);
                e
            }
            "key" => {
                let text = v["text"].as_str().unwrap_or("");
                // (20 nov., relecture macOS) un texte qui porte une touche de fonction d'AppKit (U+F700–U+F8FF) : jete
                if text.chars().any(|c| ('\u{F700}'..='\u{F8FF}').contains(&c)) {
                    objc_autoreleasePoolPop(pool);
                    return;
                }
                let kc = v["kc"].as_u64().unwrap_or(0).min(0x7f) as u16;
                let ty: usize = if v["down"].as_bool().unwrap_or(false) { 10 } else { 11 };
                // livree au fil principal, verifiee au moment de la livrer (key_now) : jamais par la file d'AppKit
                KEY_Q.lock().unwrap().push_back((ty, flags as usize, text.to_string(), kc));
                perform(c"uwKey:");
                let _ = (loc, now);
                objc_autoreleasePoolPop(pool);
                return;
            }
            _ => std::ptr::null_mut(),
        };
        if !e.is_null() {
            send!(app(), c"postEvent:atStart:", Id = e, i8 = 0 => ());
        }
        objc_autoreleasePoolPop(pool);
    }
}

/// Le fil audio : la classe de service « interactive » (la plus haute pour un processus ordinaire).
pub fn raise_priority() {
    // QOS_CLASS_USER_INTERACTIVE
    unsafe {
        pthread_set_qos_class_self_np(0x21, 0);
    }
}
