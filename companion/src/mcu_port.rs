// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (12 nov. 2026) macOS : les deux points de la surface Mackie Control (mcu.rs). Une DESTINATION
//! virtuelle « Uppwind Link MCU » (Logic → la surface), lue ici, et une SOURCE virtuelle du meme nom (la surface →
//! Logic), ou seul le compagnon ecrit. Ce n'est PAS une boucle comme les ports du gardien (midi_ports.rs) : sur une
//! boucle, Logic recevrait ses propres voyants comme des pressions de boutons (le voyant PLAY allume, 90 5E 7F, est le
//! message « PLAY presse »), et il jouerait, s'arreterait et armerait des pistes tout seul.
//!
//! Les ports n'existent que si la surface est permise (« mackie on ») ET le compagnon arme : le fil de la surface les
//! ouvre et les ferme (en 250 ms au plus) ; desarme, rien n'est ecoute et l'etat est oublie. Pour Logic, c'est une
//! surface USB qu'on branche et qu'on debranche : il interroge tout port qui apparait (en 0,3 s, mesure le 11 nov.) et
//! la retrouve, sans en ajouter une deuxieme. Les identifiants des deux points sont fixes (midi_ports::unique_id).
//!
//! Le rappel de CoreMIDI (un fil de CoreMIDI) ne fait que copier les octets vers le fil de la surface ; celui-ci les
//! decoupe (mcu::Splitter), repond a la poignee de main, tient l'etat (mcu::Hub) et presse les boutons du studio :
//! enfonce, puis relache mcu::PRESS_HOLD apres. Toutes les 2 s, il regarde si Logic Pro tourne encore (Logic ne dit rien
//! en partant). Aucun essai de `cargo test` n'ouvre ces ports : Logic interroge tout port qui apparait, et une surface
//! d'essai lui repondrait (l'incident du 11 nov., 19 h). Ce fichier, mince, s'essaie avec Logic.

use crate::mcu::{self, Button, Event, Splitter};
use crate::midi_ports::{self, cf, kMIDIPropertyUniqueID, CFRelease, MIDIClientCreate, MIDIClientDispose, MIDIDestinationCreate, MIDIEndpointDispose, MIDIObjectRef, MIDIObjectSetIntegerProperty, MIDIPacketListAdd, MIDIPacketListInit, MIDIReceived, MIDISourceCreate};
use crate::Shared;
use std::ffi::c_void;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// L'armement et la permission sont relus au moins toutes les TICK : les ports suivent.
const TICK: Duration = Duration::from_millis(250);
/// Ports ouverts : ce que Logic envoie et les pressions sont lus par pas de STEP (une pression part en 10 ms au plus).
const STEP: Duration = Duration::from_millis(10);
/// Logic Pro tourne-t-il encore ? (une fenetre de l'appli, quelle qu'elle soit : son nom se lit sans autorisation)
const LOGIC_EVERY: Duration = Duration::from_secs(2);
/// Une ouverture ratee (le serveur MIDI absent ?) : la suivante attend ce temps, sans remplir le journal.
const RETRY: Duration = Duration::from_secs(10);

/// La file du rappel de CoreMIDI vers le fil de la surface ; None : les ports sont fermes.
static SINK: Mutex<Option<Sender<Event>>> = Mutex::new(None);

fn sink() -> std::sync::MutexGuard<'static, Option<Sender<Event>>> {
    SINK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Un nom permis pour la surface : jamais un port du lien (« Uppwind Link », « Uppwind Link Back »), ou Logic prendrait
/// les notes du studio pour des boutons (la note 5E, c'est Play).
pub fn surface_name_ok(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && !midi_ports::PORTS.iter().any(|p| p.eq_ignore_ascii_case(n))
}

/// Le rappel de la destination (un fil de CoreMIDI) : les octets de chaque paquet, copies vers le fil de la surface.
/// Les memes pas que mcu::packet_list (essaye avec les deux alignements).
extern "C" fn read(list: *const c_void, _refcon: *mut c_void, _src: *mut c_void) {
    if list.is_null() {
        return;
    }
    let aligned = cfg!(any(target_arch = "aarch64", target_arch = "arm"));
    let mut bytes = Vec::new();
    // SAFETY : CoreMIDI donne une MIDIPacketList entiere : un u32 (le nombre de paquets), puis chaque paquet (un
    // horodatage u64, une longueur u16, les donnees), comme MIDIPacketNext les enchaine.
    unsafe {
        let base = list as *const u8;
        let n = std::ptr::read_unaligned(base as *const u32) as usize;
        let mut off = 4usize;
        for _ in 0..n.min(mcu::MAX_PACKETS) {
            let p = base.add(off);
            let len = std::ptr::read_unaligned(p.add(8) as *const u16) as usize;
            // (12 nov. 2026, relecture de securite de la 0.4.2) jamais plus de mcu::MAX_LIST_BYTES par liste
            if !mcu::fits_in_list(bytes.len(), len) {
                break;
            }
            bytes.extend_from_slice(std::slice::from_raw_parts(p.add(10), len));
            off = mcu::next_packet(off, len, aligned);
        }
    }
    if bytes.is_empty() {
        return;
    }
    if let Some(tx) = sink().as_ref() {
        let _ = tx.send(Event::Midi(bytes));
    }
}

/// Les deux points ouverts, et leur client ; tout est defait au Drop (les points disparaissent pour Logic).
struct Ports {
    client: MIDIObjectRef,
    src: MIDIObjectRef,
    dst: MIDIObjectRef,
}

impl Ports {
    fn open(name: &str, tx: Sender<Event>) -> Result<Ports, String> {
        if !surface_name_ok(name) {
            return Err(format!("\"{name}\" is a port of the link, never the surface's"));
        }
        *sink() = Some(tx);
        // des maintenant : une erreur plus bas defait ce qui est fait (Drop)
        let mut p = Ports { client: 0, src: 0, dst: 0 };
        let n = cf("Uppwind Link companion (Mackie Control)");
        let st = unsafe { MIDIClientCreate(n, None, std::ptr::null_mut(), &mut p.client) };
        unsafe { CFRelease(n) };
        if st != 0 {
            return Err(format!("MIDIClientCreate failed ({st})"));
        }
        let n = cf(name);
        let s1 = unsafe { MIDISourceCreate(p.client, n, &mut p.src) };
        let s2 = if s1 == 0 { unsafe { MIDIDestinationCreate(p.client, n, read, std::ptr::null_mut(), &mut p.dst) } } else { s1 };
        unsafe { CFRelease(n) };
        if s1 != 0 || s2 != 0 {
            return Err(format!("cannot create the MIDI port \"{name}\" ({})", if s1 != 0 { s1 } else { s2 }));
        }
        unsafe {
            // deja pris : CoreMIDI garde l'identifiant qu'il avait donne
            let _ = MIDIObjectSetIntegerProperty(p.src, kMIDIPropertyUniqueID, midi_ports::unique_id(name, "source"));
            let _ = MIDIObjectSetIntegerProperty(p.dst, kMIDIPropertyUniqueID, midi_ports::unique_id(name, "destination"));
        }
        Ok(p)
    }

    /// Un message vers Logic, par la source de la surface.
    fn send(&self, msg: &[u8]) -> bool {
        // une MIDIPacketList de 512 octets, alignee sur 4 (nos messages en font 18 au plus)
        let mut buf = [0u32; 128];
        let list = buf.as_mut_ptr() as *mut c_void;
        unsafe {
            let cur = MIDIPacketListInit(list);
            let cur = MIDIPacketListAdd(list, std::mem::size_of_val(&buf), cur, 0, msg.len(), msg.as_ptr());
            !cur.is_null() && MIDIReceived(self.src, list) == 0
        }
    }
}

impl Drop for Ports {
    fn drop(&mut self) {
        unsafe {
            if self.dst != 0 {
                MIDIEndpointDispose(self.dst);
            }
            if self.src != 0 {
                MIDIEndpointDispose(self.src);
            }
            if self.client != 0 {
                MIDIClientDispose(self.client);
            }
        }
        *sink() = None;
    }
}

/// Le fil de la surface (macOS, hors --demo).
pub fn start(sh: Arc<Shared>) {
    let _ = std::thread::Builder::new().name("mcu".into()).spawn(move || run(&sh));
}

struct Open {
    ports: Ports,
    rx: Receiver<Event>,
    split: Splitter,
    /// le bouton enfonce, et quand le relacher
    held: Option<(Instant, Button)>,
    looked: Option<Instant>,
}

impl Open {
    fn release_due(&mut self, now: Instant) {
        if let Some((t, b)) = self.held {
            if now >= t {
                self.ports.send(&b.up());
                self.held = None;
            }
        }
    }
}

fn run(sh: &Shared) {
    let mut open: Option<Open> = None;
    let mut failed: Option<Instant> = None;
    loop {
        let want = sh.mcu.allowed() && sh.arming.armed();
        if want && open.is_none() && failed.is_none_or(|t| t.elapsed() >= RETRY) {
            let (tx, rx) = mpsc::channel();
            match Ports::open(mcu::PORT_NAME, tx.clone()) {
                Ok(ports) => {
                    sh.mcu.set_ports(Some(tx));
                    crate::log(&format!("Logic Pro surface: the MIDI port {} is open (Mackie Control); Logic Pro connects to it by itself", mcu::PORT_NAME));
                    open = Some(Open { ports, rx, split: Splitter::default(), held: None, looked: None });
                    failed = None;
                }
                Err(e) => {
                    crate::log(&format!("Logic Pro surface: {e}"));
                    failed = Some(Instant::now());
                }
            }
        }
        if !want {
            failed = None;
            if let Some(mut o) = open.take() {
                // jamais un bouton laisse enfonce
                if let Some((_, b)) = o.held.take() {
                    o.ports.send(&b.up());
                }
                // d'abord le Hub (plus aucune pression, l'etat oublie), puis les points (Drop : ils disparaissent)
                sh.mcu.set_ports(None);
                drop(o);
                crate::log(&format!("Logic Pro surface: the MIDI port {} is closed", mcu::PORT_NAME));
            }
        }
        let Some(o) = open.as_mut() else {
            std::thread::sleep(TICK);
            continue;
        };
        if o.looked.is_none_or(|t| t.elapsed() >= LOGIC_EVERY) {
            sh.mcu.set_logic(crate::plugins::logic_running(&crate::plugins::enumerate()));
            o.looked = Some(Instant::now());
        }
        let until = Instant::now() + TICK;
        loop {
            match o.rx.recv_timeout(STEP) {
                Ok(Event::Midi(bytes)) => {
                    let mut msgs = Vec::new();
                    o.split.push(&bytes, &mut msgs);
                    for m in msgs {
                        for r in sh.mcu.feed(&m) {
                            o.ports.send(&r);
                        }
                    }
                }
                Ok(Event::Press(b)) => {
                    if let Some((_, h)) = o.held.take() {
                        o.ports.send(&h.up());
                    }
                    o.ports.send(&b.down());
                    o.held = Some((Instant::now() + mcu::PRESS_HOLD, b));
                }
                Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {}
            }
            let now = Instant::now();
            o.release_due(now);
            if now >= until {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_surface_never_takes_a_port_of_the_link() {
        assert!(surface_name_ok(mcu::PORT_NAME));
        for no in ["Uppwind Link", "Uppwind Link Back", "uppwind link", " Uppwind Link Back ", ""] {
            assert!(!surface_name_ok(no), "{no:?}");
        }
        // et l'inverse : le gardien ne tient jamais le port de la surface (une boucle), midi_ports.rs l'essaie aussi
        assert!(!midi_ports::PORTS.contains(&mcu::PORT_NAME));
    }
}
