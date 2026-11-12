// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (12 nov. 2026) Logic Pro sans script : le compagnon se presente a Logic comme une surface Mackie Control (le modele
//! `14`), sur des ports a elle, « Uppwind Link MCU » (mcu_port.rs, macOS ; mesure avec Logic Pro 12.3.1). Ce module n'appelle
//! pas CoreMIDI : il se construit et s'essaie partout.
//!
//! Ce que Logic dit a la surface, et ce qu'elle lui repond (mesure sur Logic 12.3.1, le 11 nov.) :
//!   - la poignee de main : Device Query (`00`) du modele `14` → `01` (un numero de serie, un defi : Logic ne verifie
//!     aucun defi pour ce modele), Version Request (`13`) → `14` « V1.00 ». Les autres modeles (Logic Control `10`, son
//!     extension `11`, l'extension Mackie `15`, la C4 `17`) n'ont AUCUNE reponse : Logic interroge tout port qui
//!     apparait pour ces cinq modeles, et il installe une surface par modele qui repond, sur le meme port (l'incident du
//!     11 nov., 19 h) ;
//!   - l'etat : l'ecran (`12`, 2 lignes de 56 caracteres, 7 par tranche : le nom coupe a 6 lettres, puis la valeur des
//!     boutons rotatifs, le pan en mode PAN), les faders (pitch bend 14 bits ; 0 dB vers 76 %), les voyants (note on :
//!     par tranche REC, SOLO, MUTE, SELECT ; le transport), les anneaux des boutons rotatifs (CC 30-37), le compteur
//!     (CC 40-49), l'affichage d'assignation (CC 4A-4B) et le message `72` (un octet par tranche, sans doute son type ;
//!     07 : une tranche vide) ; les vumetres (aftertouch) sont lus et jetes ;
//!   - Logic ne pose plus aucune question apres la premiere seconde (pas de keep-alive) : la surface ne sait qu'il est
//!     parti que par son processus (le fil de la surface le regarde) ou par Go offline (`0F`), s'il l'envoie.
//! Les boutons que le studio peut presser forment une liste fermee : Play et Stop. Record n'y est pas : il ne peut pas
//! etre construit. Tout ce que Logic envoie est borne : 8 tranches, 7 caracteres chacune, l'ASCII imprimable seulement.
//!
//! Le `Hub` (dans Shared) : la surface permise ou non (« mackie on », la console ; le fichier d'etat le garde), ses ports
//! ouverts ou non (seulement arme ET permis : mcu_port.rs), Logic branche ou non, l'etat de la table, et les pressions
//! (une a la fois, MAX_PRESSES_PER_SEC par seconde au plus). Deux generations : « surface » (les booleens) et « mixer »
//! (la table), que chaque connexion suit (session.rs).

// hors de macOS, seuls --demo et le protocole s'en servent (le reste sert mcu_port.rs) : pas d'avertissement pour cela
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Le nom des deux points de la surface (une source et une destination). Jamais une boucle (midi_ports.rs), jamais
/// « Uppwind Link » : Logic prendrait les notes du studio pour des boutons (la note 5E, c'est Play).
pub const PORT_NAME: &str = "Uppwind Link MCU";
/// Le fabricant (Mackie) dans les SysEx, et le seul modele auquel la surface repond : Mackie Control.
const MACKIE: [u8; 3] = [0x00, 0x00, 0x66];
pub const MODEL: u8 = 0x14;
/// Ce que la surface dit d'elle (les octets de la sonde du 11 nov., que Logic a acceptes).
pub const SERIAL: [u8; 7] = *b"UWLMCU1";
pub const CHALLENGE: [u8; 4] = [0x01, 0x02, 0x03, 0x04];
pub const VERSION: [u8; 5] = *b"V1.00";

pub const STRIPS: usize = 8;
/// L'ecran : 2 lignes de 56 caracteres, 7 par tranche.
pub const LCD_LINE: usize = 56;
pub const LCD_LEN: usize = 2 * LCD_LINE;
pub const CELL: usize = 7;
/// Le compteur : 10 caracteres (mesures, temps, divisions, ticks ; ou heures, minutes, secondes, images).
pub const COUNTER_LEN: usize = 10;
/// Un SysEx plus long est jete jusqu'a son F7 (le plus long de Logic : l'ecran entier, 7 + 112 octets).
pub const MAX_SYSEX: usize = 512;
/// Au plus tant de paquets lus dans une MIDIPacketList (CoreMIDI en met quelques-uns).
pub const MAX_PACKETS: usize = 4096;
/// Une pression : le bouton enfonce, puis relache ce temps apres (comme la sonde du 11 nov.).
pub const PRESS_HOLD: Duration = Duration::from_millis(100);
/// Au plus tant de pressions par seconde, toutes connexions confondues.
pub const MAX_PRESSES_PER_SEC: usize = 4;
const PRESS_SLACK: Duration = Duration::from_millis(20);
pub const FADER_MAX: u16 = 0x3FFF;
/// La cle du fichier d'etat ou le choix « mackie on » est garde.
pub const STATE_KEY: &str = "mackieSurface";

/// Les notes (canal 1) des voyants que la surface lit et des boutons qu'elle presse.
pub mod note {
    /// + la tranche (0 a 7)
    pub const REC: u8 = 0x00;
    pub const SOLO: u8 = 0x08;
    pub const MUTE: u8 = 0x10;
    pub const SELECT: u8 = 0x18;
    /// (12 nov.) les 8 tranches precedentes, les 8 suivantes
    pub const BANK_LEFT: u8 = 0x2E;
    pub const BANK_RIGHT: u8 = 0x2F;
    pub const CYCLE: u8 = 0x56;
    pub const CLICK: u8 = 0x59;
    /// le solo global (il clignote tant qu'une piste est en solo)
    pub const SOLO_ANY: u8 = 0x5A;
    pub const REWIND: u8 = 0x5B;
    pub const FORWARD: u8 = 0x5C;
    pub const STOP: u8 = 0x5D;
    pub const PLAY: u8 = 0x5E;
    pub const RECORD: u8 = 0x5F;
    pub const SMPTE: u8 = 0x71;
    pub const BEATS: u8 = 0x72;
}

/// Les boutons que le studio peut presser : une liste fermee. (12 nov.) Play et Stop, puis BANK ◀ et ▶ (les 8 tranches
/// precedentes ou suivantes). Record n'y est pas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Play,
    Stop,
    BankLeft,
    BankRight,
}

impl Button {
    /// « transport » : « play » | « stop » ; rien d'autre.
    pub fn parse(s: &str) -> Option<Button> {
        match s {
            "play" => Some(Button::Play),
            "stop" => Some(Button::Stop),
            _ => None,
        }
    }
    /// « bank » : « left » | « right » ; rien d'autre.
    pub fn bank(dir: &str) -> Option<Button> {
        match dir {
            "left" => Some(Button::BankLeft),
            "right" => Some(Button::BankRight),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Button::Play => "play",
            Button::Stop => "stop",
            Button::BankLeft => "bank left",
            Button::BankRight => "bank right",
        }
    }
    pub fn note(self) -> u8 {
        match self {
            Button::Play => note::PLAY,
            Button::Stop => note::STOP,
            Button::BankLeft => note::BANK_LEFT,
            Button::BankRight => note::BANK_RIGHT,
        }
    }
    /// Le bouton enfonce, puis relache (note on, velocite 7F puis 0, comme une vraie surface).
    pub fn down(self) -> [u8; 3] {
        [0x90, self.note(), 0x7F]
    }
    pub fn up(self) -> [u8; 3] {
        [0x90, self.note(), 0x00]
    }
}

fn sysex(body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(body.len() + 6);
    v.push(0xF0);
    v.extend_from_slice(&MACKIE);
    v.push(MODEL);
    v.extend_from_slice(body);
    v.push(0xF7);
    v
}

/// La reponse a Device Query : Host Connection Query (01), le numero de serie et le defi.
pub fn host_connection_query() -> Vec<u8> {
    let mut b = vec![0x01];
    b.extend_from_slice(&SERIAL);
    b.extend_from_slice(&CHALLENGE);
    sysex(&b)
}

/// La reponse a Version Request : Version Reply (14) « V1.00 » (Control Surface Setup l'affiche).
pub fn version_reply() -> Vec<u8> {
    let mut b = vec![0x14];
    b.extend_from_slice(&VERSION);
    sysex(&b)
}

// ── Les octets d'un port → des messages complets ─────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
enum Sx {
    #[default]
    Out,
    In(Vec<u8>),
    /// trop long : jete jusqu'a son F7
    Skip,
}

/// Les octets d'un port MIDI, paquet apres paquet → des messages complets. Un SysEx est recolle d'un paquet a l'autre
/// (MAX_SYSEX au plus) ; les messages de canal suivent le running status ; le temps reel (F8-FF) est ignore, meme au
/// milieu d'un SysEx ; les messages systeme communs (F1-F6) et les octets de donnees sans statut sont jetes.
#[derive(Debug, Default)]
pub struct Splitter {
    sx: Sx,
    status: u8,
    data: [u8; 2],
    have: usize,
}

fn data_len(status: u8) -> usize {
    match status {
        0x80..=0xBF | 0xE0..=0xEF | 0xF2 => 2,
        0xC0..=0xDF | 0xF1 | 0xF3 => 1,
        _ => 0,
    }
}

impl Splitter {
    pub fn push(&mut self, bytes: &[u8], out: &mut Vec<Vec<u8>>) {
        for &b in bytes {
            if b >= 0xF8 {
                continue;
            }
            match &mut self.sx {
                Sx::In(buf) => {
                    if b == 0xF7 {
                        buf.push(0xF7);
                        out.push(std::mem::take(buf));
                        self.sx = Sx::Out;
                        continue;
                    }
                    if b < 0x80 {
                        if buf.len() >= MAX_SYSEX {
                            self.sx = Sx::Skip;
                        } else {
                            buf.push(b);
                        }
                        continue;
                    }
                    // un statut au milieu : ce SysEx-la est coupe (jete), le statut compte
                    self.sx = Sx::Out;
                }
                Sx::Skip => {
                    if b == 0xF7 {
                        self.sx = Sx::Out;
                        continue;
                    }
                    if b < 0x80 {
                        continue;
                    }
                    self.sx = Sx::Out;
                }
                Sx::Out => {}
            }
            if b == 0xF0 {
                self.sx = Sx::In(vec![0xF0]);
                self.status = 0;
                self.have = 0;
                continue;
            }
            if b == 0xF7 {
                continue;
            }
            if b >= 0x80 {
                // F4 F5 F6 : rien a lire ; un statut systeme ne laisse pas de running status
                self.status = if data_len(b) == 0 { 0 } else { b };
                self.have = 0;
                continue;
            }
            if self.status == 0 {
                continue;
            }
            self.data[self.have] = b;
            self.have += 1;
            let need = data_len(self.status);
            if self.have == need {
                self.have = 0;
                let s = self.status;
                if s >= 0xF0 {
                    self.status = 0;
                    continue;
                }
                let mut m = vec![s];
                m.extend_from_slice(&self.data[..need]);
                out.push(m);
            }
        }
    }
}

/// La place du paquet suivant dans une MIDIPacketList (MIDIPacketNext de CoreMIDI) : apres l'horodatage (8 octets), la
/// longueur (2) et les donnees ; alignee sur 4 octets sous arm64 (et ARM), collee sous x86_64. `offset` : celle du
/// paquet (le premier est a 4, apres le nombre de paquets).
pub fn next_packet(offset: usize, len: usize, aligned: bool) -> usize {
    let end = offset + 10 + len;
    if aligned {
        (end + 3) & !3
    } else {
        end
    }
}

/// Les donnees de chaque paquet d'une MIDIPacketList tenue dans `buf` (les essais ; le rappel de CoreMIDI, mcu_port.rs,
/// suit les memes pas sur sa memoire). Une liste coupee s'arrete la ou elle est coupee.
#[cfg(test)]
pub fn packet_list(buf: &[u8], aligned: bool) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let Some(n) = buf.get(..4).map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]) as usize) else { return out };
    let mut off = 4;
    for _ in 0..n.min(MAX_PACKETS) {
        let Some(h) = buf.get(off + 8..off + 10) else { break };
        let len = u16::from_ne_bytes([h[0], h[1]]) as usize;
        let Some(d) = buf.get(off + 10..off + 10 + len) else { break };
        out.push(d);
        off = next_packet(off, len, aligned);
    }
    out
}

// ── L'etat de la surface ─────────────────────────────────────────────────────────────────────────────────────────

/// Un voyant : eteint, allume (velocite 7F) ou clignotant (01).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Led {
    #[default]
    Off,
    On,
    Blink,
}

impl Led {
    fn of(velocity: u8) -> Led {
        match velocity {
            0 => Led::Off,
            1 => Led::Blink,
            _ => Led::On,
        }
    }
    pub fn lit(self) -> bool {
        self != Led::Off
    }
}

/// Un caractere des afficheurs a 7 segments (le compteur, l'assignation) : 0x00-0x1F → « @ », « A » … « _ » ;
/// 0x20-0x3F → l'ASCII ; 0x40 : le point.
pub fn seg(v: u8) -> (u8, bool) {
    let c = v & 0x3F;
    (if c < 0x20 { c + 0x40 } else { c }, v & 0x40 != 0)
}

/// Des caracteres de Logic → du texte : l'ASCII imprimable, le reste en espaces, sans les espaces des bords.
fn shown(bytes: &[u8]) -> String {
    let s: String = bytes.iter().map(|&c| if (0x20..=0x7E).contains(&c) { c as char } else { ' ' }).collect();
    s.trim().to_string()
}

/// Ce que Logic a dit a la surface depuis sa poignee de main.
#[derive(Debug, Clone, PartialEq)]
pub struct Surface {
    /// Logic a fait la poignee de main (sa Version Request a eu sa reponse)
    pub connected: bool,
    pub lcd: [u8; LCD_LEN],
    /// les 8 tranches, puis le master
    pub faders: [u16; STRIPS + 1],
    pub leds: [Led; 128],
    pub rings: [u8; STRIPS],
    pub counter: [u8; COUNTER_LEN],
    /// les points du compteur (bit i : apres le caractere i)
    pub dots: u16,
    pub assign: [u8; 2],
    /// le message 72 (0 : pas encore dit)
    pub kinds: [u8; STRIPS],
}

impl Default for Surface {
    fn default() -> Self {
        Surface {
            connected: false,
            lcd: [b' '; LCD_LEN],
            faders: [0; STRIPS + 1],
            leds: [Led::Off; 128],
            rings: [0; STRIPS],
            counter: [b' '; COUNTER_LEN],
            dots: 0,
            assign: [b' '; 2],
            kinds: [0; STRIPS],
        }
    }
}

/// Ce qu'un message de Logic a fait : les reponses a lui renvoyer, et si l'etat a change.
#[derive(Debug, Default, PartialEq)]
pub struct Fed {
    pub replies: Vec<Vec<u8>>,
    pub changed: bool,
}

impl Surface {
    /// Un message complet de Logic (Splitter).
    pub fn feed(&mut self, m: &[u8]) -> Fed {
        let before = self.clone();
        let mut replies = Vec::new();
        self.apply(m, &mut replies);
        Fed { changed: *self != before, replies }
    }

    fn apply(&mut self, m: &[u8], replies: &mut Vec<Vec<u8>>) {
        let Some(&s) = m.first() else { return };
        if s == 0xF0 {
            return self.sysex(m, replies);
        }
        if m.len() != 3 {
            return; // les vumetres (D0, deux octets), le reste
        }
        let (d1, d2) = (m[1] & 0x7F, m[2] & 0x7F);
        match (s & 0xF0, s & 0x0F) {
            (0x90, 0) => self.leds[d1 as usize] = Led::of(d2),
            (0x80, 0) => self.leds[d1 as usize] = Led::Off,
            (0xE0, ch) if (ch as usize) <= STRIPS => self.faders[ch as usize] = d1 as u16 | ((d2 as u16) << 7),
            (0xB0, 0) => self.cc(d1, d2),
            _ => {}
        }
    }

    fn cc(&mut self, cc: u8, v: u8) {
        match cc {
            0x30..=0x37 => self.rings[(cc - 0x30) as usize] = v,
            // 40 : le caractere de droite, 49 : celui de gauche
            0x40..=0x49 => {
                let i = COUNTER_LEN - 1 - (cc - 0x40) as usize;
                let (c, dot) = seg(v);
                self.counter[i] = c;
                if dot {
                    self.dots |= 1 << i;
                } else {
                    self.dots &= !(1 << i);
                }
            }
            0x4A..=0x4B => self.assign[1 - (cc - 0x4A) as usize] = seg(v).0,
            _ => {}
        }
    }

    fn sysex(&mut self, m: &[u8], replies: &mut Vec<Vec<u8>>) {
        if m.len() < 7 || m[1..4] != MACKIE || m[m.len() - 1] != 0xF7 {
            return;
        }
        if m[4] != MODEL {
            return; // une autre surface (10, 11, 15, 17) : jamais de reponse, rien de lu
        }
        let body = &m[6..m.len() - 1];
        match m[5] {
            0x00 => replies.push(host_connection_query()),
            0x13 => {
                replies.push(version_reply());
                self.connected = true;
            }
            0x12 => {
                if let Some((&off, chars)) = body.split_first() {
                    for (i, &c) in chars.iter().enumerate() {
                        if let Some(x) = self.lcd.get_mut(off as usize + i) {
                            *x = c;
                        }
                    }
                }
            }
            0x72 => {
                if body.len() >= STRIPS {
                    self.kinds.copy_from_slice(&body[..STRIPS]);
                }
            }
            // Go offline : Logic laisse la surface
            0x0F => *self = Surface::default(),
            0x61 => self.faders = [0; STRIPS + 1],
            0x62 => self.leds = [Led::Off; 128],
            0x63 => {
                let c = self.connected;
                *self = Surface::default();
                self.connected = c;
            }
            _ => {}
        }
    }

    /// Le nom de la tranche `i` (0 a 7) : la 1re ligne de l'ecran.
    pub fn name(&self, i: usize) -> String {
        shown(&self.lcd[i * CELL..(i + 1) * CELL])
    }
    /// Sa valeur : la 2e ligne (le pan en mode PAN : « 0 », « -35 », « +63 »).
    pub fn value(&self, i: usize) -> String {
        shown(&self.lcd[LCD_LINE + i * CELL..LCD_LINE + (i + 1) * CELL])
    }
    /// La place du fader `i` (8 : le master), de 0 a 1, a 4 decimales.
    pub fn fader(&self, i: usize) -> f64 {
        (self.faders[i] as f64 / FADER_MAX as f64 * 10000.0).round() / 10000.0
    }
    /// Le compteur tel qu'il s'affiche, 10 caracteres.
    pub fn counter_text(&self) -> String {
        self.counter.iter().map(|&c| if (0x20..=0x7E).contains(&c) { c as char } else { ' ' }).collect()
    }
    /// Le compteur en mesures et temps (true), en temps SMPTE (false), ou on ne sait pas (None).
    pub fn beats(&self) -> Option<bool> {
        match (self.lit(note::BEATS), self.lit(note::SMPTE)) {
            (true, false) => Some(true),
            (false, true) => Some(false),
            _ => None,
        }
    }
    fn lit(&self, n: u8) -> bool {
        self.leds[n as usize].lit()
    }

    /// La table pour le studio (session.rs l'envoie dans « mixer »).
    pub fn json(&self) -> Value {
        let strips: Vec<Value> = (0..STRIPS)
            .map(|i| {
                let n = i as u8;
                json!({
                    "n": i + 1,
                    "name": self.name(i),
                    "value": self.value(i),
                    "fader": self.fader(i),
                    "rec": self.lit(note::REC + n),
                    "solo": self.lit(note::SOLO + n),
                    "mute": self.lit(note::MUTE + n),
                    "select": self.lit(note::SELECT + n),
                    "kind": if self.kinds[i] == 0 { Value::Null } else { json!(self.kinds[i]) },
                })
            })
            .collect();
        json!({
            "strips": strips,
            "master": { "fader": self.fader(STRIPS) },
            "assign": shown(&self.assign),
            "transport": {
                "play": self.lit(note::PLAY), "stop": self.lit(note::STOP), "record": self.lit(note::RECORD),
                "rewind": self.lit(note::REWIND), "forward": self.lit(note::FORWARD), "cycle": self.lit(note::CYCLE),
                "click": self.lit(note::CLICK), "solo": self.lit(note::SOLO_ANY),
            },
            "counter": self.counter_text(),
            "beats": self.beats(),
        })
    }
}

// ── Le Hub : la surface, partagee par le fil de la surface et les connexions ──────────────────────────────────────

/// Ce que recoit le fil de la surface : les octets que Logic envoie (le rappel de CoreMIDI), ou une pression.
#[derive(Debug, PartialEq)]
pub enum Event {
    Midi(Vec<u8>),
    Press(Button),
}

/// Une pression refusee : un code (le studio le reconnait) et ce qu'il montre.
#[derive(Debug, Clone, PartialEq)]
pub struct Refused {
    pub code: &'static str,
    pub error: String,
}

fn refused(code: &'static str, error: &str) -> Result<(), Refused> {
    Err(Refused { code, error: error.to_string() })
}

/// Ce que dit le compagnon quand la surface n'est pas permise.
#[cfg(target_os = "macos")]
pub const OFF_TEXT: &str = "the Logic Pro surface is off in Uppwind Link: type mackie on in the Terminal where it runs";
#[cfg(not(target_os = "macos"))]
pub const OFF_TEXT: &str = "the Logic Pro surface needs the companion on a Mac";

struct Inner {
    /// ce compagnon peut-il tenir la surface (macOS, ou --demo)
    available: bool,
    /// « mackie on »
    allowed: bool,
    /// ses ports sont ouverts (arme et permis)
    ports: bool,
    /// Logic Pro tourne (le fil de la surface regarde son processus)
    logic: bool,
    surface: Surface,
    /// la file du fil de la surface (les pressions), tant que ses ports sont ouverts
    tx: Option<Sender<Event>>,
    presses: VecDeque<Instant>,
    busy_until: Option<Instant>,
}

impl Inner {
    fn online(&self) -> bool {
        self.ports && self.surface.connected
    }
}

pub struct Hub {
    inner: Mutex<Inner>,
    surface_gen: AtomicU64,
    mixer_gen: AtomicU64,
}

impl Hub {
    /// `available` : macOS (ou --demo) ; `allowed` : le choix d'avant (le fichier d'etat), ou --demo.
    pub fn new(available: bool, allowed: bool) -> Hub {
        Hub {
            inner: Mutex::new(Inner { available, allowed, ports: false, logic: false, surface: Surface::default(), tx: None, presses: VecDeque::new(), busy_until: None }),
            surface_gen: AtomicU64::new(0),
            mixer_gen: AtomicU64::new(0),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Un changement : les generations avancent pour ce qui a change (la table, et les booleens).
    fn with<R>(&self, f: impl FnOnce(&mut Inner) -> (R, bool)) -> R {
        let mut g = self.lock();
        let before = (g.allowed, g.ports, g.online());
        let (r, mixed) = f(&mut g);
        let after = (g.allowed, g.ports, g.online());
        drop(g);
        if before != after {
            self.surface_gen.fetch_add(1, Ordering::SeqCst);
        }
        // (12 nov., l'essai reel) la console le dit quand Logic se branche a la surface, ou la quitte
        if before.2 != after.2 {
            crate::log(if after.2 { "Logic Pro surface: Logic Pro connected (Mackie Control)" } else { "Logic Pro surface: Logic Pro is no longer connected" });
        }
        if mixed || before.2 != after.2 {
            self.mixer_gen.fetch_add(1, Ordering::SeqCst);
        }
        r
    }

    pub fn available(&self) -> bool {
        self.lock().available
    }
    pub fn allowed(&self) -> bool {
        self.lock().allowed
    }
    pub fn online(&self) -> bool {
        self.lock().online()
    }

    /// La personne devant l'ordinateur permet la surface ou la retire (la console) → true si cela a change. Jamais le
    /// studio. Les ports suivent (mcu_port.rs, avec l'armement).
    pub fn set_allowed(&self, on: bool, by: &str) -> bool {
        let changed = self.with(|g| {
            let c = g.allowed != on;
            g.allowed = on;
            (c, false)
        });
        if changed {
            crate::log(&format!("Logic Pro surface (Mackie Control): {} ({by})", if on { "allowed" } else { "off" }));
        }
        changed
    }

    /// Le fil de la surface : ses ports s'ouvrent (avec sa file) ou se ferment. Dans les deux cas l'etat repart de zero :
    /// desarme, le compagnon oublie tout ce que Logic lui a dit, noms compris.
    pub fn set_ports(&self, tx: Option<Sender<Event>>) {
        self.with(|g| {
            g.ports = tx.is_some();
            g.tx = tx;
            g.surface = Surface::default();
            g.presses.clear();
            g.busy_until = None;
            ((), true)
        });
    }

    /// Logic Pro tourne-t-il ? Parti (il ne dit rien en partant), l'etat est oublie : il refera la poignee de main.
    pub fn set_logic(&self, running: bool) {
        self.with(|g| {
            let gone = g.logic && !running;
            g.logic = running;
            if gone {
                g.surface = Surface::default();
            }
            ((), gone)
        });
    }

    /// Un message complet de Logic → les reponses a lui renvoyer. Ports fermes : rien.
    pub fn feed(&self, m: &[u8]) -> Vec<Vec<u8>> {
        self.with(|g| {
            if !g.ports {
                return (Vec::new(), false);
            }
            let f = g.surface.feed(m);
            (f.replies, f.changed)
        })
    }

    /// Les booleens pour le studio (« hello », « surface ») : envoyes meme desarme, rien de Logic dedans.
    pub fn surface_json(&self) -> Value {
        let g = self.lock();
        json!({ "available": g.available, "allowed": g.allowed, "ports": g.ports, "online": g.online() })
    }

    /// « mixer » : la table, seulement quand Logic est branche (donc arme et permis).
    pub fn mixer_json(&self) -> Value {
        let g = self.lock();
        if !g.online() {
            return json!({ "type": "mixer", "online": false });
        }
        let mut v = g.surface.json();
        v["type"] = json!("mixer");
        v["online"] = json!(true);
        v
    }

    pub fn surface_generation(&self) -> u64 {
        self.surface_gen.load(Ordering::SeqCst)
    }
    pub fn mixer_generation(&self) -> u64 {
        self.mixer_gen.load(Ordering::SeqCst)
    }

    /// Presser un bouton (le studio, arme : session.rs le verifie avant). Une pression a la fois, au plus
    /// MAX_PRESSES_PER_SEC par seconde, et seulement quand Logic est branche.
    pub fn press(&self, b: Button) -> Result<(), Refused> {
        self.press_at(b, Instant::now())
    }

    pub fn press_at(&self, b: Button, now: Instant) -> Result<(), Refused> {
        let mut g = self.lock();
        if !g.available || !g.allowed {
            return refused("off", OFF_TEXT);
        }
        if !g.ports {
            return refused("offline", "the MIDI port Uppwind Link MCU is not open: is the companion armed?");
        }
        if !g.surface.connected {
            return refused("offline", "Logic Pro has not connected to the surface Uppwind Link MCU: is Logic Pro open?");
        }
        if g.busy_until.is_some_and(|t| now < t) {
            return refused("busy", "a button of Logic Pro is still being pressed");
        }
        while g.presses.front().is_some_and(|t| now.saturating_duration_since(*t) >= Duration::from_secs(1)) {
            g.presses.pop_front();
        }
        if g.presses.len() >= MAX_PRESSES_PER_SEC {
            return refused("busy", "at most 4 presses per second");
        }
        let sent = g.tx.as_ref().is_some_and(|tx| tx.send(Event::Press(b)).is_ok());
        if !sent {
            return refused("offline", "the surface stopped");
        }
        g.presses.push_back(now);
        g.busy_until = Some(now + PRESS_HOLD + PRESS_SLACK);
        Ok(())
    }
}

// ── --demo : une fausse table, sans aucun port MIDI ──────────────────────────────────────────────────────────────

/// --demo : une fausse table de 8 tranches, sans aucun port MIDI ; Play et Stop allument leurs voyants et le compteur
/// avance pendant la lecture ; (12 nov.) BANK ▶ montre les 8 tranches suivantes, ◀ revient. Ce sont les messages de
/// Logic, donc le meme decodage : de quoi essayer le vrai compagnon avec le studio sans Logic.
pub mod demo {
    use super::*;

    const NAMES: [&str; STRIPS] = ["Piano", "Bass", "Drums", "Pad", "Lead", "Vox", "St Out", "Master"];
    const VALUES: [&str; STRIPS] = ["0", "-12", "0", "+20", "0", "+5", "0", ""];
    const FADERS: [u16; STRIPS] = [12443, 11000, 12443, 9500, 12443, 10400, 12443, 12443];
    const KINDS: [u8; STRIPS] = [2, 2, 2, 2, 2, 2, 5, 4];
    // les 8 suivantes (BANK ▶) : six pistes, puis deux tranches vides
    const NAMES_2: [&str; STRIPS] = ["Strngs", "Choir", "Arp", "Sub", "Keys", "Perc", "", ""];
    const VALUES_2: [&str; STRIPS] = ["0", "+10", "-30", "0", "0", "+3", "", ""];
    const FADERS_2: [u16; STRIPS] = [11500, 12443, 10000, 12443, 9000, 12000, 0, 0];
    const KINDS_2: [u8; STRIPS] = [2, 2, 2, 2, 2, 2, 7, 7];
    const MASTER: u16 = 12443;

    /// Un groupe de 8 tranches (0 : le projet, 1 : les 8 suivantes), comme Logic le renvoie apres BANK : l'ecran, les
    /// faders, tous les voyants des tranches (le 1er groupe : la 1re selectionnee, la 3e coupee) et le message 72.
    pub fn table(bank: usize) -> Vec<Vec<u8>> {
        let (names, values, faders, kinds) = if bank == 0 { (NAMES, VALUES, FADERS, KINDS) } else { (NAMES_2, VALUES_2, FADERS_2, KINDS_2) };
        let mut v = Vec::new();
        let mut lcd = vec![0x12, 0x00];
        for n in names {
            lcd.extend(format!("{:<7}", n).bytes().take(CELL));
        }
        for x in values {
            lcd.extend(format!("{:<7}", x).bytes().take(CELL));
        }
        v.push(sysex(&lcd));
        for (ch, pos) in faders.iter().enumerate() {
            v.push(vec![0xE0 | ch as u8, (pos & 0x7F) as u8, (pos >> 7) as u8]);
        }
        for i in 0..STRIPS as u8 {
            for (base, on) in [(note::REC, false), (note::SOLO, false), (note::MUTE, bank == 0 && i == 2), (note::SELECT, bank == 0 && i == 0)] {
                v.push(vec![0x90, base + i, if on { 0x7F } else { 0x00 }]);
            }
        }
        let mut k = vec![0x72];
        k.extend_from_slice(&kinds);
        v.push(sysex(&k));
        v
    }

    /// Ce que Logic dirait d'un projet de 8 tranches, juste apres la poignee de main.
    pub fn script() -> Vec<Vec<u8>> {
        let mut v = vec![sysex(&[0x00]), sysex(&[0x13, 0x00])];
        v.extend(table(0));
        v.push(vec![0xE8, (MASTER & 0x7F) as u8, (MASTER >> 7) as u8]);
        for n in [note::STOP, note::BEATS, note::CLICK] {
            v.push(vec![0x90, n, 0x7F]);
        }
        v.push(vec![0xB0, 0x4B, 0x10]);
        v.push(vec![0xB0, 0x4A, 0x0E]);
        v.extend(counter(0));
        v
    }

    /// Le compteur apres `ms` de lecture, a 120 BPM en 4/4 (mesures, temps, doubles croches, ticks sur 240).
    pub fn counter(ms: u64) -> Vec<Vec<u8>> {
        let sixteenths = ms / 125;
        let tick = (ms % 125) * 240 / 125;
        let (bar, beat, sub) = (1 + sixteenths / 16, 1 + (sixteenths / 4) % 4, 1 + sixteenths % 4);
        let text = format!("{:>3}{:>2}{:>2}{:>3}", bar % 1000, beat, sub, tick + 1);
        text.bytes()
            .enumerate()
            .map(|(i, c)| vec![0xB0, 0x49 - i as u8, if c >= 0x40 { c - 0x40 } else { c }])
            .collect()
    }

    /// La fausse table : le groupe montre, et depuis quand elle joue.
    #[derive(Debug, Default)]
    pub struct Demo {
        pub bank: usize,
        since: Option<Instant>,
    }

    impl Demo {
        /// Ce que Logic repond a une pression : les voyants de Play et de Stop ; avec BANK, l'autre groupe (la fausse
        /// table en a deux : au-dela, rien ne change, comme Logic au bout de ses pistes).
        pub fn press(&mut self, b: Button, now: Instant) -> Vec<Vec<u8>> {
            match b {
                Button::Play | Button::Stop => {
                    let play = b == Button::Play;
                    self.since = if play { self.since.or(Some(now)) } else { None };
                    let (on, off) = if play { (note::PLAY, note::STOP) } else { (note::STOP, note::PLAY) };
                    vec![vec![0x90, on, 0x7F], vec![0x90, off, 0x00]]
                }
                Button::BankLeft | Button::BankRight => {
                    let next = if b == Button::BankRight { (self.bank + 1).min(1) } else { self.bank.saturating_sub(1) };
                    if next == self.bank {
                        return Vec::new();
                    }
                    self.bank = next;
                    table(next)
                }
            }
        }

        /// Le compteur, pendant la lecture.
        pub fn tick(&self, now: Instant) -> Vec<Vec<u8>> {
            self.since.map(|t| counter(now.saturating_duration_since(t).as_millis() as u64)).unwrap_or_default()
        }
    }

    /// Le fil de la fausse table (--demo) : ses « ports » ouverts tout de suite, sans CoreMIDI.
    pub fn start(sh: std::sync::Arc<crate::Shared>) {
        let _ = std::thread::Builder::new().name("mcu-demo".into()).spawn(move || {
            use std::sync::mpsc::RecvTimeoutError;
            let (tx, rx) = std::sync::mpsc::channel();
            sh.mcu.set_ports(Some(tx));
            for m in script() {
                sh.mcu.feed(&m);
            }
            let mut d = Demo::default();
            loop {
                let out = match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(Event::Press(b)) => d.press(b, Instant::now()),
                    Ok(Event::Midi(_)) => Vec::new(),
                    Err(RecvTimeoutError::Timeout) => d.tick(Instant::now()),
                    Err(RecvTimeoutError::Disconnected) => break,
                };
                for m in out {
                    sh.mcu.feed(&m);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    /// Une fixture : (l'instant, le sens, les octets), sans les lignes de commentaire.
    fn fixture(text: &str) -> Vec<(f64, char, Vec<u8>)> {
        text.lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .map(|l| {
                let mut it = l.splitn(3, ' ');
                let t = it.next().unwrap().parse().unwrap();
                let dir = it.next().unwrap().chars().next().unwrap();
                (t, dir, hex(it.next().unwrap()))
            })
            .collect()
    }

    const QUERIES: &str = include_str!("../tests/fixtures/mcu/logic-12.3.1-queries.txt");
    const PLAY_STOP: &str = include_str!("../tests/fixtures/mcu/logic-12.3.1-play-stop.txt");

    fn feed_all(s: &mut Surface, msgs: &[&str]) -> Vec<Vec<u8>> {
        msgs.iter().flat_map(|m| s.feed(&hex(m)).replies).collect()
    }

    #[test]
    fn only_a_mackie_control_is_answered() {
        let mut s = Surface::default();
        // les cinq modeles que Logic interroge : seul 14 a une reponse (l'incident du 11 nov., 19 h)
        for model in ["10", "11", "15", "17"] {
            assert!(feed_all(&mut s, &[&format!("F0 00 00 66 {model} 00 F7"), &format!("F0 00 00 66 {model} 13 00 F7")]).is_empty(), "model {model}");
        }
        assert!(!s.connected);
        // les octets de la sonde, que Logic 12.3.1 a acceptes
        assert_eq!(feed_all(&mut s, &["F0 00 00 66 14 00 F7"]), vec![hex("F0 00 00 66 14 01 55 57 4C 4D 43 55 31 01 02 03 04 F7")]);
        assert!(!s.connected, "a Device Query alone is not a connection");
        assert_eq!(feed_all(&mut s, &["F0 00 00 66 14 13 00 F7"]), vec![hex("F0 00 00 66 14 14 56 31 2E 30 30 F7")]);
        assert!(s.connected);
        // ni un autre fabricant, ni un SysEx coupe, ni une Host Connection Reply (Logic n'en envoie pas pour 14)
        assert!(feed_all(&mut s, &["F0 00 00 67 14 00 F7", "F0 00 00 66 14 00", "F0 00 00 66 14 02 01 02 03 04 05 06 07 01 02 03 04 F7"]).is_empty());
        // l'ecran d'une autre surface n'est pas le notre
        feed_all(&mut s, &["F0 00 00 66 15 12 00 41 42 43 F7"]);
        assert_eq!(s.name(0), "");
    }

    #[test]
    fn the_queries_logic_sent_on_the_11th_get_only_mackie_control_answers() {
        let mut s = Surface::default();
        let mut sp = Splitter::default();
        let mut replies = Vec::new();
        for (_, dir, bytes) in fixture(QUERIES) {
            assert_eq!(dir, '<');
            let mut msgs = Vec::new();
            sp.push(&bytes, &mut msgs);
            for m in msgs {
                replies.extend(s.feed(&m).replies);
            }
        }
        // 3 Device Query et 3 Version Request du modele 14 dans ces 0,4 s ; aucune reponse aux modeles 10, 11, 15 et 17,
        // ni a la reponse au defi de la Logic Control XT (11 02)
        assert_eq!(replies.iter().filter(|r| **r == host_connection_query()).count(), 3);
        assert_eq!(replies.iter().filter(|r| **r == version_reply()).count(), 3);
        assert_eq!(replies.len(), 6);
        assert!(replies.iter().all(|r| r[4] == MODEL));
        assert!(s.connected);
    }

    #[test]
    fn the_screen_is_written_by_pieces_and_read_as_plain_text() {
        let mut s = Surface::default();
        let mut line1 = vec![0x12, 0x00];
        line1.extend(b"DelCls St Out Master ");
        s.feed(&sysex(&line1));
        assert_eq!((s.name(0), s.name(1), s.name(2), s.name(3)), ("DelCls".into(), "St Out".into(), "Master".into(), "".into()));
        // un morceau, plus loin
        let mut piece = vec![0x12, 7];
        piece.extend(b"Bass  ");
        s.feed(&sysex(&piece));
        assert_eq!(s.name(1), "Bass");
        // la 2e ligne : les valeurs
        let mut line2 = vec![0x12, LCD_LINE as u8];
        line2.extend(b"+63    +11    -35    ");
        s.feed(&sysex(&line2));
        assert_eq!((s.value(0), s.value(1), s.value(2)), ("+63".into(), "+11".into(), "-35".into()));
        // des caracteres de controle deviennent des espaces ; au-dela de l'ecran, rien (et pas de panique)
        s.feed(&sysex(&[0x12, 21, 0x01, b'<', b'b', 0x7F, b'>', 0x02, 0x03]));
        assert_eq!(s.name(3), "<b >");
        s.feed(&sysex(&[0x12, 0x6E, b'x', b'y', b'z', b'w']));
        assert_eq!(&s.lcd[0x6E..], b"xy");
        s.feed(&sysex(&[0x12]));
        let names = s.json()["strips"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().len()).max().unwrap();
        assert!(names <= CELL);
    }

    #[test]
    fn faders_leds_rings_and_the_counter() {
        let mut s = Surface::default();
        // 12443 : 0 dB (76 %), mesure le 11 nov. ; E8 : le master
        s.feed(&hex("E0 1B 61"));
        s.feed(&hex("E8 1B 61"));
        assert_eq!((s.faders[0], s.faders[8]), (12443, 12443));
        assert_eq!(s.fader(0), 0.7595);
        s.feed(&hex("E9 7F 7F")); // un 10e fader n'existe pas
        assert_eq!(s.faders.iter().filter(|f| **f != 0).count(), 2);
        // les voyants : allume, clignotant, eteint ; un autre canal n'est pas pour nous
        s.feed(&hex("90 5E 7F"));
        assert!(s.json()["transport"]["play"].as_bool().unwrap());
        s.feed(&hex("90 5A 01"));
        assert_eq!(s.leds[0x5A], Led::Blink);
        assert!(s.json()["transport"]["solo"].as_bool().unwrap());
        s.feed(&hex("90 5E 00"));
        s.feed(&hex("91 5D 7F"));
        assert!(!s.leds[0x5E].lit() && !s.leds[0x5D].lit());
        s.feed(&hex("90 5D 7F"));
        s.feed(&hex("80 5D 40"));
        assert!(!s.leds[0x5D].lit(), "note off: off");
        // par tranche : REC 1, SOLO 2, MUTE 3, SELECT 8
        for m in ["90 00 7F", "90 09 7F", "90 12 7F", "90 1F 7F"] {
            s.feed(&hex(m));
        }
        let j = s.json();
        let t = |i: usize, k: &str| j["strips"][i][k].as_bool().unwrap();
        assert!(t(0, "rec") && t(1, "solo") && t(2, "mute") && t(7, "select"));
        assert!(!t(0, "solo") && !t(1, "mute") && !t(2, "select") && !t(7, "rec"));
        // le compteur (les octets du 11 nov. : « 2 2 1224 ») et l'assignation (« PN »)
        for m in ["B0 49 20", "B0 48 20", "B0 47 32", "B0 46 20", "B0 45 32", "B0 44 20", "B0 43 31", "B0 42 32", "B0 41 32", "B0 40 34"] {
            s.feed(&hex(m));
        }
        assert_eq!(s.counter_text(), "  2 2 1224");
        s.feed(&hex("B0 47 72"));
        assert_eq!((s.counter[2], s.dots), (b'2', 1 << 2), "0x40: the point");
        s.feed(&hex("B0 4B 10"));
        s.feed(&hex("B0 4A 0E"));
        assert_eq!(s.json()["assign"], "PN");
        // un anneau ; un vumetre (lu, jete)
        s.feed(&hex("B0 30 16"));
        assert_eq!(s.rings[0], 0x16);
        let before = s.clone();
        assert!(!s.feed(&hex("D0 1C")).changed);
        assert_eq!(s, before);
        // BEATS ou SMPTE
        assert_eq!(s.beats(), None);
        s.feed(&hex("90 72 7F"));
        assert_eq!(s.beats(), Some(true));
        s.feed(&hex("90 71 7F"));
        assert_eq!(s.beats(), None);
        s.feed(&hex("90 72 00"));
        assert_eq!(s.beats(), Some(false));
    }

    #[test]
    fn kinds_resets_and_go_offline() {
        let mut s = Surface::default();
        s.feed(&hex("F0 00 00 66 14 13 00 F7"));
        s.feed(&hex("F0 00 00 66 14 72 02 05 04 07 07 07 07 07 F7"));
        assert_eq!(s.kinds, [2, 5, 4, 7, 7, 7, 7, 7]);
        assert_eq!(s.json()["strips"][0]["kind"], 2);
        s.feed(&hex("F0 00 00 66 14 72 02 F7"));
        assert_eq!(s.kinds[0], 2, "too short: left as it was");
        s.feed(&hex("E0 1B 61"));
        s.feed(&hex("90 5E 7F"));
        s.feed(&hex("F0 00 00 66 14 61 F7"));
        assert_eq!(s.faders[0], 0);
        assert!(s.leds[0x5E].lit());
        s.feed(&hex("F0 00 00 66 14 62 F7"));
        assert!(!s.leds[0x5E].lit());
        s.feed(&hex("F0 00 00 66 14 12 00 41 F7"));
        s.feed(&hex("F0 00 00 66 14 63 F7"));
        assert_eq!((s.name(0), s.connected), ("".into(), true), "Reset: the state goes, the connection stays");
        s.feed(&hex("F0 00 00 66 14 0F 7F F7"));
        assert_eq!(s, Surface::default(), "Go offline");
        assert_eq!(Surface::default().json()["strips"][0]["kind"], Value::Null);
    }

    #[test]
    fn the_splitter_joins_sysex_across_packets_and_follows_running_status() {
        let mut sp = Splitter::default();
        let mut out = Vec::new();
        // un SysEx sur trois paquets, avec une horloge (F8) au milieu
        sp.push(&hex("F0 00 00"), &mut out);
        sp.push(&hex("66 14 F8 12 00"), &mut out);
        sp.push(&hex("41 42 F7 90 5E 7F 5D 00"), &mut out);
        assert_eq!(out, vec![hex("F0 00 00 66 14 12 00 41 42 F7"), hex("90 5E 7F"), hex("90 5D 00")], "running status");
        let split = |s: &str| {
            let mut out = Vec::new();
            Splitter::default().push(&hex(s), &mut out);
            out
        };
        // des donnees sans statut, un F7 seul, un message systeme commun (F2 : deux octets, jetes), D0 (un octet)
        assert_eq!(split("12 34 F7 F2 01 02 D0 1C 2C B0 40 31"), vec![hex("D0 1C"), hex("D0 2C"), hex("B0 40 31")]);
        // un statut coupe un SysEx : le SysEx est jete, le message compte
        assert_eq!(split("F0 00 00 66 14 12 90 5E 7F"), vec![hex("90 5E 7F")]);
        // trop long : jete jusqu'a son F7, puis tout repart
        let mut long = vec![0xF0];
        long.extend(std::iter::repeat(0x41).take(MAX_SYSEX + 10));
        long.extend(hex("F7 F0 00 00 66 14 13 00 F7"));
        let mut out = Vec::new();
        Splitter::default().push(&long, &mut out);
        assert_eq!(out, vec![hex("F0 00 00 66 14 13 00 F7")]);
        // F6 (sans donnee) coupe le running status
        assert_eq!(split("90 5E 7F F6 5D 00"), vec![hex("90 5E 7F")]);
    }

    /// Une MIDIPacketList comme CoreMIDI la pose, alignee ou non.
    fn list_of(packets: &[&[u8]], aligned: bool) -> Vec<u8> {
        let mut b = (packets.len() as u32).to_ne_bytes().to_vec();
        for p in packets {
            let off = b.len();
            b.extend_from_slice(&7u64.to_ne_bytes());
            b.extend_from_slice(&(p.len() as u16).to_ne_bytes());
            b.extend_from_slice(p);
            b.resize(next_packet(off, p.len(), aligned), 0xEE);
        }
        b
    }

    #[test]
    fn packet_lists_are_read_with_both_alignments() {
        let packets: [&[u8]; 3] = [&[0x90, 0x5E, 0x7F], &[0xF0, 0x00, 0x00, 0x66, 0x14, 0x13, 0x00, 0xF7], &[0xD0]];
        for aligned in [true, false] {
            let b = list_of(&packets, aligned);
            assert_eq!(packet_list(&b, aligned), packets.to_vec(), "aligned {aligned}");
            // coupee dans l'en-tete du dernier paquet : ce qui est entier, puis rien
            assert_eq!(packet_list(&b[..b.len() - 3], aligned), packets[..2].to_vec());
        }
        assert_eq!(next_packet(4, 3, true), 20);
        assert_eq!(next_packet(4, 3, false), 17);
        assert_eq!(next_packet(4, 2, true), 16);
        assert!(packet_list(&[1, 0], true).is_empty());
        // un nombre de paquets absurde : borne, sans lire hors de la liste
        let mut b = list_of(&packets, true);
        b[..4].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert_eq!(packet_list(&b, true).len(), 3);
    }

    #[test]
    fn random_bytes_never_panic() {
        // une graine fixe : le meme essai a chaque fois
        let mut x: u64 = 0x2545_F491_4F6C_DD1D;
        let mut sp = Splitter::default();
        let mut s = Surface::default();
        let mut out = Vec::new();
        for _ in 0..2000 {
            let mut chunk = Vec::new();
            for _ in 0..64 {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                // plus de F0 et de F7 qu'au hasard, et l'en-tete de Mackie de temps en temps
                chunk.push(match x % 16 {
                    0 => 0xF0,
                    1 => 0xF7,
                    2 => 0x66,
                    3 => 0x14,
                    _ => (x >> 8) as u8,
                });
            }
            sp.push(&chunk, &mut out);
            for m in out.drain(..) {
                s.feed(&m);
            }
            let _ = packet_list(&chunk, x % 2 == 0);
        }
        let j = s.json();
        assert_eq!(j["strips"].as_array().unwrap().len(), STRIPS);
        assert!(j["strips"].as_array().unwrap().iter().all(|t| t["name"].as_str().unwrap().chars().count() <= CELL));
        assert!(j["counter"].as_str().unwrap().chars().count() == COUNTER_LEN);
    }

    #[test]
    fn only_play_stop_and_bank_can_be_pressed() {
        assert_eq!(Button::parse("play"), Some(Button::Play));
        assert_eq!(Button::parse("stop"), Some(Button::Stop));
        for no in ["record", "Play", "rewind", "", "bank", "left"] {
            assert_eq!(Button::parse(no), None, "{no}");
        }
        // (12 nov.) BANK : « left » | « right », rien d'autre
        assert_eq!((Button::bank("left"), Button::bank("right")), (Some(Button::BankLeft), Some(Button::BankRight)));
        for no in ["", "Left", "up", "play", "record"] {
            assert_eq!(Button::bank(no), None, "{no}");
        }
        assert_eq!((Button::Play.down(), Button::Play.up()), ([0x90, 0x5E, 0x7F], [0x90, 0x5E, 0x00]));
        assert_eq!((Button::Stop.down(), Button::Stop.up()), ([0x90, 0x5D, 0x7F], [0x90, 0x5D, 0x00]));
        assert_eq!((Button::BankLeft.down(), Button::BankRight.down()), ([0x90, 0x2E, 0x7F], [0x90, 0x2F, 0x7F]));
        assert_eq!(Button::BankRight.up(), [0x90, 0x2F, 0x00]);
        assert!([Button::Play, Button::Stop, Button::BankLeft, Button::BankRight].iter().all(|b| b.note() != note::RECORD));
    }

    #[test]
    fn the_session_of_the_11th_replays_names_faders_play_and_stop() {
        let mut s = Surface::default();
        let mut sp = Splitter::default();
        let (mut ours, mut sent) = (Vec::new(), Vec::new());
        let mut seen: Vec<(f64, Value)> = Vec::new();
        for (t, dir, bytes) in fixture(PLAY_STOP) {
            if dir == '>' {
                sent.push(bytes); // ce que la sonde a envoye (ses reponses, ses pressions)
                continue;
            }
            let mut msgs = Vec::new();
            sp.push(&bytes, &mut msgs);
            for m in msgs {
                ours.extend(s.feed(&m).replies);
            }
            seen.push((t, s.json()));
        }
        // les reponses de la sonde sont les notres, a l'octet pres ; ses pressions aussi
        let probe_replies: Vec<Vec<u8>> = sent.iter().filter(|b| b[0] == 0xF0).cloned().collect();
        assert_eq!(ours, probe_replies);
        let presses: Vec<Vec<u8>> = sent.iter().filter(|b| b[0] == 0x90).cloned().collect();
        assert_eq!(presses, vec![Button::Play.down().to_vec(), Button::Play.up().to_vec(), Button::Stop.down().to_vec(), Button::Stop.up().to_vec()]);
        assert!(s.connected);
        let last_before = |t: f64| seen.iter().rev().find(|(x, _)| *x < t).map(|(_, j)| j.clone()).unwrap();
        // juste avant Play (12,09 s) : la table de ton projet d'essai
        let j = last_before(12.0);
        let names: Vec<&str> = j["strips"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["DelCls", "St Out", "Master", "", "", "", "", ""]);
        assert_eq!((j["strips"][0]["value"].as_str(), j["strips"][1]["value"].as_str()), (Some("+63"), Some("+11")));
        assert_eq!((j["strips"][0]["fader"].as_f64(), j["strips"][1]["fader"].as_f64(), j["master"]["fader"].as_f64()), (Some(0.7497), Some(0.7595), Some(0.7595)));
        assert_eq!((j["strips"][0]["rec"].as_bool(), j["strips"][0]["select"].as_bool()), (Some(true), Some(true)));
        assert_eq!(j["strips"][0]["kind"], 2);
        assert_eq!(j["strips"][3]["kind"], 7);
        assert_eq!(j["assign"], "PN");
        assert_eq!((j["transport"]["stop"].as_bool(), j["transport"]["play"].as_bool(), j["transport"]["click"].as_bool()), (Some(true), Some(false), Some(true)));
        assert_eq!((j["counter"].as_str(), j["beats"].as_bool()), (Some("  2 2 1224"), Some(true)));
        // 24 ms apres la pression de Play : PLAY allume, STOP eteint, puis le compteur avance
        let j = last_before(12.12);
        assert_eq!((j["transport"]["play"].as_bool(), j["transport"]["stop"].as_bool()), (Some(true), Some(false)));
        assert_ne!(last_before(15.0)["counter"], last_before(12.0)["counter"]);
        // apres Stop (15,10 s) : STOP allume, PLAY eteint
        let j = s.json();
        assert_eq!((j["transport"]["play"].as_bool(), j["transport"]["stop"].as_bool()), (Some(false), Some(true)));
    }

    #[test]
    fn the_hub_presses_only_when_logic_is_connected_one_at_a_time() {
        let t0 = Instant::now();
        let h = Hub::new(true, false);
        assert_eq!(h.press_at(Button::Play, t0).unwrap_err().code, "off");
        assert!(h.set_allowed(true, "a test"));
        assert!(!h.set_allowed(true, "a test"));
        assert_eq!(h.press_at(Button::Play, t0).unwrap_err().code, "offline", "no port yet");
        let (tx, rx) = std::sync::mpsc::channel();
        h.set_ports(Some(tx));
        assert_eq!(h.press_at(Button::Play, t0).unwrap_err().code, "offline", "Logic has not connected");
        assert_eq!(h.feed(&hex("F0 00 00 66 14 13 00 F7")), vec![version_reply()]);
        assert!(h.online());
        assert!(h.press_at(Button::Play, t0).is_ok());
        assert_eq!(rx.try_recv(), Ok(Event::Press(Button::Play)));
        assert_eq!(h.press_at(Button::Stop, t0 + Duration::from_millis(50)).unwrap_err().code, "busy", "Play is still held");
        // au plus 4 par seconde
        let mut t = t0;
        for _ in 0..3 {
            t += PRESS_HOLD + PRESS_SLACK;
            assert!(h.press_at(Button::Stop, t).is_ok());
        }
        t += PRESS_HOLD + PRESS_SLACK;
        assert_eq!(h.press_at(Button::Stop, t).unwrap_err().code, "busy");
        assert!(h.press_at(Button::Stop, t0 + Duration::from_millis(1500)).is_ok());
        assert_eq!(rx.try_iter().count(), 4);
        // le fil de la surface parti : plus rien ne part
        drop(rx);
        assert_eq!(h.press_at(Button::Play, t0 + Duration::from_secs(5)).unwrap_err().code, "offline");
        // un compagnon sans surface (Windows)
        let w = Hub::new(false, true);
        assert_eq!(w.press_at(Button::Play, t0).unwrap_err().code, "off");
        assert_eq!(w.surface_json(), json!({"available": false, "allowed": true, "ports": false, "online": false}));
    }

    #[test]
    fn the_hub_forgets_everything_when_its_ports_close_or_logic_quits() {
        let h = Hub::new(true, true);
        let (g0, m0) = (h.surface_generation(), h.mixer_generation());
        assert!(h.feed(&hex("F0 00 00 66 14 00 F7")).is_empty(), "no port: nothing is read");
        assert_eq!(h.mixer_json(), json!({"type": "mixer", "online": false}));
        let (tx, _rx) = std::sync::mpsc::channel();
        h.set_ports(Some(tx));
        assert!(h.surface_generation() > g0);
        h.feed(&hex("F0 00 00 66 14 13 00 F7"));
        h.feed(&hex("F0 00 00 66 14 12 00 44 65 6C 43 6C 73 F7"));
        let j = h.mixer_json();
        assert_eq!((j["type"].as_str(), j["online"].as_bool(), j["strips"][0]["name"].as_str()), (Some("mixer"), Some(true), Some("DelCls")));
        assert_eq!(h.surface_json()["online"], true);
        let m1 = h.mixer_generation();
        assert!(m1 > m0);
        // un message qui ne change rien ne fait pas avancer la table
        h.feed(&hex("F0 00 00 66 14 12 00 44 65 6C 43 6C 73 F7"));
        assert_eq!(h.mixer_generation(), m1);
        // Logic vu, puis parti : il refera la poignee de main
        h.set_logic(true);
        assert!(h.online());
        h.set_logic(false);
        assert!(!h.online());
        assert_eq!(h.mixer_json()["online"], false);
        // desarme : les ports se ferment, l'etat est oublie
        h.feed(&hex("F0 00 00 66 14 13 00 F7"));
        h.feed(&hex("F0 00 00 66 14 12 00 41 F7"));
        let g1 = h.surface_generation();
        h.set_ports(None);
        assert!(h.surface_generation() > g1);
        assert_eq!(h.surface_json(), json!({"available": true, "allowed": true, "ports": false, "online": false}));
        let (tx, _rx) = std::sync::mpsc::channel();
        h.set_ports(Some(tx));
        h.feed(&hex("F0 00 00 66 14 13 00 F7"));
        assert_eq!(h.mixer_json()["strips"][0]["name"], "", "nothing is kept from before");
    }

    #[test]
    fn the_demo_table_goes_through_the_same_decoding() {
        let mut s = Surface::default();
        for m in demo::script() {
            s.feed(&m);
        }
        assert!(s.connected);
        let j = s.json();
        assert_eq!(j["strips"][0]["name"], "Piano");
        assert_eq!(j["strips"][7]["name"], "Master");
        assert_eq!(j["strips"][1]["value"], "-12");
        assert_eq!(j["strips"][2]["mute"], true);
        assert_eq!((j["counter"].as_str(), j["assign"].as_str()), (Some("  1 1 1  1"), Some("PN")));
        let t0 = Instant::now();
        let mut d = demo::Demo::default();
        for m in d.press(Button::Play, t0) {
            s.feed(&m);
        }
        for m in d.tick(t0 + Duration::from_millis(2125)) {
            s.feed(&m);
        }
        let j = s.json();
        assert_eq!((j["transport"]["play"].as_bool(), j["transport"]["stop"].as_bool()), (Some(true), Some(false)));
        // 2,125 s a 120 BPM : la 2e mesure, 1er temps, 2e double croche
        assert_eq!(j["counter"], "  2 1 2  1");
        // (12 nov.) BANK ▶ : les 8 suivantes (deux vides, et plus rien de coupe ni de selectionne) ; encore ▶ : rien ;
        // ◀ : le projet revient
        for m in d.press(Button::BankRight, t0) {
            s.feed(&m);
        }
        let j = s.json();
        assert_eq!((j["strips"][0]["name"].as_str(), j["strips"][6]["kind"].as_u64(), j["strips"][2]["mute"].as_bool(), j["strips"][0]["select"].as_bool()), (Some("Strngs"), Some(7), Some(false), Some(false)));
        assert!(d.press(Button::BankRight, t0).is_empty(), "past the last group: nothing");
        for m in d.press(Button::BankLeft, t0) {
            s.feed(&m);
        }
        let j = s.json();
        assert_eq!((j["strips"][0]["name"].as_str(), j["strips"][2]["mute"].as_bool(), j["strips"][0]["select"].as_bool()), (Some("Piano"), Some(true), Some(true)));
        assert!(d.press(Button::BankLeft, t0).is_empty(), "before the first group: nothing");
        assert_eq!(j["transport"]["play"], true, "BANK does not touch the transport");
        for m in d.press(Button::Stop, t0) {
            s.feed(&m);
        }
        assert!(d.tick(t0 + Duration::from_secs(5)).is_empty(), "stopped: the counter stays");
    }
}
