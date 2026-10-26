// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! macOS (26 oct. 2026) : les deux ports MIDI d'Uppwind Link crees par le compagnon lui-meme (CoreMIDI), a la place de
//! loopMIDI (Windows) ou du bus IAC a regler a la main.
//!
//! Chaque port est une boucle, comme un port de loopMIDI : une DESTINATION virtuelle (les autres applis y envoient :
//! pour elles, c'est une sortie MIDI « Uppwind Link ») et une SOURCE virtuelle du meme nom (une entree « Uppwind
//! Link ») ; ce qui arrive dans la destination ressort tel quel par la source (MIDIReceived, sur le fil de CoreMIDI,
//! sans rien lire ni garder). Les deux ports : « Uppwind Link » (le studio → le DAW) et « Uppwind Link Back » (le DAW →
//! le studio).
//!
//! Ils existent tant que le compagnon tourne (macOS les retire a sa sortie) : rien n'est ecrit dans la configuration
//! MIDI de la machine. Un identifiant unique fixe (kMIDIPropertyUniqueID) par port : FL et Live les retrouvent d'un
//! lancement a l'autre. --no-midi-ports les desactive (un bus IAC du meme nom, par exemple).
//! *(a tester)* : FL Studio pour Mac et Chrome (Web MIDI) voient-ils ces ports ; FL accepte-t-il un script sur une
//! source virtuelle.

#![allow(non_upper_case_globals)]

use std::ffi::{c_char, c_void};

type MIDIObjectRef = u32;
type OSStatus = i32;
type CFStringRef = *const c_void;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithCString(alloc: *const c_void, s: *const c_char, enc: u32) -> CFStringRef;
    fn CFRelease(o: *const c_void);
}

#[link(name = "CoreMIDI", kind = "framework")]
extern "C" {
    static kMIDIPropertyUniqueID: CFStringRef;
    fn MIDIClientCreate(name: CFStringRef, notify: *const c_void, refcon: *mut c_void, out: *mut MIDIObjectRef) -> OSStatus;
    fn MIDISourceCreate(client: MIDIObjectRef, name: CFStringRef, out: *mut MIDIObjectRef) -> OSStatus;
    fn MIDIDestinationCreate(client: MIDIObjectRef, name: CFStringRef, read: extern "C" fn(*const c_void, *mut c_void, *mut c_void), refcon: *mut c_void, out: *mut MIDIObjectRef) -> OSStatus;
    fn MIDIReceived(src: MIDIObjectRef, list: *const c_void) -> OSStatus;
    fn MIDIObjectSetIntegerProperty(obj: MIDIObjectRef, prop: CFStringRef, v: i32) -> OSStatus;
    static kMIDIPropertyDisplayName: CFStringRef;
    static kMIDIPropertyName: CFStringRef;
    fn MIDIObjectGetStringProperty(obj: MIDIObjectRef, prop: CFStringRef, out: *mut CFStringRef) -> OSStatus;
    fn MIDIGetNumberOfSources() -> usize;
    fn MIDIGetSource(i: usize) -> MIDIObjectRef;
    fn MIDIGetNumberOfDestinations() -> usize;
    fn MIDIGetDestination(i: usize) -> MIDIObjectRef;
}

fn name_of(obj: MIDIObjectRef) -> String {
    for prop in unsafe { [kMIDIPropertyDisplayName, kMIDIPropertyName] } {
        let mut s: CFStringRef = std::ptr::null();
        if unsafe { MIDIObjectGetStringProperty(obj, prop, &mut s) } == 0 && !s.is_null() {
            let n = crate::macos::cf_string(s);
            unsafe { CFRelease(s) };
            if !n.is_empty() {
                return n;
            }
        }
    }
    String::new()
}

/// Les ports MIDI de la machine, en lecture seule (--setup) : (les entrees = les sources que les applis lisent, les
/// sorties = les destinations ou elles ecrivent), par leur nom affiche (« IAC Driver Bus 1 », « Uppwind Link »).
pub fn endpoints() -> (Vec<String>, Vec<String>) {
    let ins = (0..unsafe { MIDIGetNumberOfSources() }).map(|i| name_of(unsafe { MIDIGetSource(i) })).filter(|n| !n.is_empty()).collect();
    let outs = (0..unsafe { MIDIGetNumberOfDestinations() }).map(|i| name_of(unsafe { MIDIGetDestination(i) })).filter(|n| !n.is_empty()).collect();
    (ins, outs)
}

const UTF8: u32 = 0x0800_0100;

/// Les deux ports (le meme nom que les ports de loopMIDI sous Windows : les scripts des DAW les cherchent par ce nom).
pub const PORTS: [&str; 2] = ["Uppwind Link", "Uppwind Link Back"];

/// L'identifiant unique d'un port : fixe pour un nom (un hachage FNV-1a, positif, jamais 0), pour que les DAW
/// retrouvent le port d'un lancement a l'autre.
pub fn unique_id(name: &str, end: &str) -> i32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in name.bytes().chain(b"/".iter().copied()).chain(end.bytes()) {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    ((h & 0x3fff_ffff) | 0x1000_0000) as i32
}

fn cf(s: &str) -> CFStringRef {
    let c = std::ffi::CString::new(s).unwrap_or_default();
    unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) }
}

/// Le rappel de la destination : refcon porte le numero de la source du meme port.
extern "C" fn forward(list: *const c_void, refcon: *mut c_void, _src: *mut c_void) {
    let src = refcon as usize as MIDIObjectRef;
    if src != 0 && !list.is_null() {
        unsafe {
            MIDIReceived(src, list);
        }
    }
}

/// Cree les deux ports → leurs noms. Erreur : le code de CoreMIDI (par exemple un serveur MIDI absent).
pub fn start() -> Result<Vec<String>, String> {
    let mut client: MIDIObjectRef = 0;
    let name = cf("Uppwind Link companion");
    let st = unsafe { MIDIClientCreate(name, std::ptr::null(), std::ptr::null_mut(), &mut client) };
    unsafe { CFRelease(name) };
    if st != 0 {
        return Err(format!("MIDIClientCreate failed ({st})"));
    }
    let mut made = Vec::new();
    for p in PORTS {
        let n = cf(p);
        let (mut src, mut dst) = (0, 0);
        let s1 = unsafe { MIDISourceCreate(client, n, &mut src) };
        let s2 = if s1 == 0 { unsafe { MIDIDestinationCreate(client, n, forward, src as usize as *mut c_void, &mut dst) } } else { s1 };
        unsafe { CFRelease(n) };
        if s1 != 0 || s2 != 0 {
            return Err(format!("cannot create the MIDI port \"{p}\" ({})", if s1 != 0 { s1 } else { s2 }));
        }
        unsafe {
            // deja pris (une autre copie du compagnon) : CoreMIDI garde l'identifiant qu'il avait donne
            let _ = MIDIObjectSetIntegerProperty(src, kMIDIPropertyUniqueID, unique_id(p, "source"));
            let _ = MIDIObjectSetIntegerProperty(dst, kMIDIPropertyUniqueID, unique_id(p, "destination"));
        }
        made.push(p.to_string());
    }
    // le client et les ports vivent autant que le programme (macOS les retire a sa sortie)
    Ok(made)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_ids_are_fixed_distinct_and_positive() {
        let all: Vec<i32> = PORTS.iter().flat_map(|p| [unique_id(p, "source"), unique_id(p, "destination")]).collect();
        assert!(all.iter().all(|i| *i > 0));
        let mut d = all.clone();
        d.sort();
        d.dedup();
        assert_eq!(d.len(), 4);
        assert_eq!(unique_id("Uppwind Link", "source"), unique_id("Uppwind Link", "source"));
    }
}
