// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Le protocole entre le studio (/atelier) et le compagnon : du JSON en messages texte, les images en messages
//! binaires. Version 1 (etape 1 ; partie 1 : l'image, partie 2 : les gestes).
//!
//! L'appairage (29 oct., v2 : security.rs) : des l'ouverture, le compagnon envoie
//!   {"type":"challenge","v":2,"nonce":"<64 hex>"}        son nonce Nc ; le studio repond (le PREMIER message) :
//!   {"type":"hello","version":1,"v":2,"nonce":"<Ns>","proof":"<HMAC(jeton, uwl-studio|port|Nc|Ns)>"}
//!   et le compagnon, si la preuve est juste : hello {…, "v":2, "proof":"<HMAC(jeton, uwl-companion|port|Ns|Nc)>"}.
//!   Le studio n'envoie rien d'autre avant d'avoir verifie cette preuve. Fermetures : 4400 (pas hello), 4401 (preuve
//!   ou jeton faux), 4429 (trop d'essais faux), 4426 (hello v1 refuse : ce jeton a deja servi en v2, « reload »).
//!
//! Studio → compagnon (texte) :
//!   {"type":"hello","version":1,"token":"<64 hex>"}      (v1, le jeton en clair) accepte tant qu'aucun studio n'a
//!                                                         fait la v2 avec ce jeton (un studio d'avant le 29 oct.)
//!   {"type":"list"}                                       → windows
//!   {"type":"devices"}                                    → devices (les plugins du set, par le script UppwindLink)
//!   {"type":"live"}                                       → live {channel, answers, ms?, error?, script?, live?} : le
//!                                                           script de Live repond-il MAINTENANT ? (le studio le
//!                                                           redemande souvent ; script : UppwindLink ou UppwindLinkDev)
//!   {"type":"open","path":"2.0"} / {"type":"close",…}     → opened / closed (EDITOR_OPEN / EDITOR_CLOSE)
//!   {"type":"watch","id":123,"fps":30,"quality":80}       → watching, puis des images
//!   {"type":"unwatch"}                                    → unwatched
//!   {"type":"ack","seq":42}                               l'image 42 est affichee (au plus 2 images en vol)
//!   {"type":"ping","t":…}                                 → pong (meme t)
//!   {"type":"mode","mode":"direct|focus"}                 → mode (PARTIE 2 : comment les gestes sont injectes)
//!   {"type":"input","kind":"pointer|wheel|key|text",…}    → inputack (PARTIE 2 : un geste, injecte des reception)
//!       pointer : action down|up|move|dblclick, x, y (pixels de l'IMAGE affichee), iw, ih (sa taille), button
//!                 (0 gauche, 1 milieu, 2 droit, comme le DOM), buttons (masque du DOM : 1 gauche, 2 droit, 4 milieu)
//!       wheel   : x, y, iw, ih, dx, dy (en pixels du DOM : 100 = un cran de molette)
//!       key     : action down|up, key, code (KeyboardEvent.key / .code)      text : text (saisie composee)
//!       tous    : mods (1 Maj, 2 Ctrl, 4 Alt, 8 Meta), n (numero du geste), t (instant du geste, ms depuis 1970)
//!   {"type":"place","x":…,"y":…,"w":…,"h":…,"dpr":…}      prevu (aligner la vraie fenetre), lu et ignore
//!   {"type":"audio","action":"start|stop","target":"auto|live|fl|test-tone|<pid>","rate":48000,"format":"f32","ms":5}
//!                                                         → audio {state:on|off,…} puis des paquets « UWA1 » (24 oct. :
//!                                                           le retour audio sans cable, audio/feed.rs et audio/packet.rs ;
//!                                                           25 oct. : target plugin|plugin:<id>, le plugin du DAW)
//!   {"type":"instances"}                                  → instances (25 oct. : les plugins Uppwind Link, plugin_link)
//!   {"type":"midi","to":"<id>","events":[{"t","d","n"}]}  des notes vers un plugin Uppwind Link Notes (plugin_link)
//!   {"type":"emitted","id":"<id>"}                        → emitted : ou et quand il les a sorties (la mesure)
//! Compagnon → studio (texte) : hello, windows, devices, opened, closed, watching, unwatched, pong, mode, inputack,
//!   error. inputack : {n, t, recv, inj, ok, error?} (recv / inj : reception / injection, ms depuis 1970).
//! Compagnon → studio (binaire) : une image = « UWF1 » + longueur de l'en-tete (u32 petit-boutiste) + en-tete JSON
//!   {"seq","id","w","h","t","enc":"jpeg","cap","sw","sh","q","in","near"} + les octets JPEG. `t` : l'instant de la
//!   capture en ms depuis 1970 (meme horloge que Date.now() du navigateur, meme machine). Une TUILE (partie 2) : en
//!   plus {"x","y"} : le JPEG (w×h) se pose en (x, y) sur l'image entiere sw×sh deja affichee. `in` : le dernier geste
//!   injecte avant la capture ; `near` : l'image a change pres du pointeur (la mesure « geste → image »). `pw`, `ph` :
//!   la taille a l'ecran quand Windows agrandit la fenetre (plugin « DPI unaware » : l'image est a SA taille, plus
//!   petite ; les gestes se donnent en pixels de l'image, comme toujours).

use serde::Deserialize;
use serde_json::{json, Value};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAGIC: &[u8; 4] = b"UWF1";
/// Un message du studio n'est jamais gros (le plus gros : un « text » de saisie).
pub const MAX_TEXT_MESSAGE: usize = 16 * 1024;
pub const MAX_TEXT_INPUT: usize = 256;
/// Un glisser peut sortir de l'image (un potard qu'on tire loin) : les coordonnees vont de -COORD_MAX a 2×COORD_MAX.
pub const COORD_MAX: f64 = 16384.0;

#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClientMsg {
    /// v1 : `token` (en clair) ; v2 (29 oct.) : `v` = 2, `nonce` (Ns) et `proof` (la preuve du studio), jamais le jeton
    Hello { version: Option<u32>, token: Option<String>, v: Option<u32>, nonce: Option<String>, proof: Option<String> },
    List,
    Devices,
    /// Le script de dev de Live repond-il maintenant ? → live {channel, answers, ms?, error?} (23 oct.)
    Live,
    Open { path: String },
    Close { path: String },
    Watch { id: u64, fps: Option<f64>, quality: Option<u8> },
    Unwatch,
    Ack { seq: u64 },
    Ping { t: Option<f64> },
    Place { x: f64, y: f64, w: f64, h: f64, dpr: Option<f64> },
    Mode { mode: String },
    Input(RawInput),
    /// (24 oct.) le retour audio sans cable (audio/feed.rs) : action start|stop, target auto|live|fl|test-tone|<pid>
    /// (25 oct. : aussi plugin|plugin:<id>, le plugin Uppwind Link Return du DAW, plugin_link)
    Audio { action: String, target: Option<String>, rate: Option<u32>, format: Option<String>, ms: Option<f64> },
    /// (25 oct., plugin_link) les plugins Uppwind Link connectes → instances
    Instances,
    /// (25 oct., plugin_link) des messages MIDI vers un plugin Uppwind Link Notes (son identite en hexadecimal)
    Midi { to: String, events: Vec<MidiIn> },
    /// (25 oct., plugin_link, la mesure) ou et quand ce Notes a sorti ses messages → emitted (vide la liste)
    Emitted { id: String },
    /// (25 oct., sans loopMIDI) le lien du studio avec le Remote Script de Live par le canal local (live.rs, LINK) : des
    /// morceaux SysEx du protocole Uppwind Link, en hexadecimal. Le premier « link » ouvre le relais (le compagnon releve
    /// alors la boite du script) ; stop : le ferme. → link {sysex:[…]} (ce que le script envoie), link {state:up|down}
    Link { sysex: Option<Vec<String>>, stop: Option<bool> },
    /// (25 oct.) le diagnostic du chemin sans loopMIDI : l'entree de chaque piste de Live et ses Notes (ROUTES) → routes
    Routes,
}

/// (25 oct.) Un morceau SysEx du lien, en hexadecimal : F0 7D 55 57 … F7, 210 octets au plus (l'en-tete et 200
/// octets de JSON), des chiffres hexadecimaux seulement. Le compagnon ne relaie rien d'autre au script.
pub const MAX_LINK_CHUNK_HEX: usize = 2 * 212;
pub const MAX_LINK_CHUNKS: usize = 32;

pub fn link_chunk_ok(h: &str) -> bool {
    let b = h.as_bytes();
    b.len() >= 20
        && b.len() <= MAX_LINK_CHUNK_HEX
        && b.len() % 2 == 0
        && b.iter().all(|c| c.is_ascii_hexdigit())
        && h[..8].eq_ignore_ascii_case("f07d5557")
        && h[h.len() - 2..].eq_ignore_ascii_case("f7")
}

/// Un message MIDI du studio : `t` l'instant ou le DAW doit le jouer (ms depuis 1970 ; absent : tout de suite), `d`
/// les octets (un message de canal), `n` son numero (la mesure).
#[derive(Debug, Deserialize, PartialEq, Clone)]
pub struct MidiIn {
    pub t: Option<f64>,
    pub d: Vec<u8>,
    pub n: Option<u32>,
}

/// Un geste tel qu'il arrive. Tous les champs optionnels : `validate_input` dit ce qui manque.
#[derive(Debug, Deserialize, PartialEq, Clone, Default)]
pub struct RawInput {
    pub kind: String,
    pub id: Option<u64>,
    /// pointer : down | up | move | dblclick ; key : down | up
    pub action: Option<String>,
    /// En pixels de l'image affichee (0,0 = coin haut gauche de la zone client du plugin).
    pub x: Option<f64>,
    pub y: Option<f64>,
    /// La taille de l'image sur laquelle x, y ont ete pris (sinon : la zone client, 1:1).
    pub iw: Option<f64>,
    pub ih: Option<f64>,
    pub button: Option<u8>,
    pub buttons: Option<u8>,
    pub dx: Option<f64>,
    pub dy: Option<f64>,
    pub key: Option<String>,
    pub code: Option<String>,
    pub text: Option<String>,
    /// Bits : 1 Maj, 2 Ctrl, 4 Alt, 8 Meta.
    pub mods: Option<u8>,
    /// Le numero du geste (le studio les compte) et son instant (ms depuis 1970) : renvoyes dans inputack.
    pub n: Option<u64>,
    pub t: Option<f64>,
}

/// Un point du studio : (x, y) dans une image iw×ih (None : la zone client elle-meme).
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
    pub iw: Option<f64>,
    pub ih: Option<f64>,
}

/// Un geste valide.
#[derive(Debug, PartialEq, Clone)]
pub enum InputEvent {
    Pointer { action: PointerAction, pt: Pt, button: u8, buttons: u8, mods: u8 },
    Wheel { pt: Pt, dx: f64, dy: f64, mods: u8 },
    Key { down: bool, key: String, code: String, mods: u8 },
    Text { text: String },
}

impl InputEvent {
    #[allow(dead_code)]
    pub fn is_move(&self) -> bool {
        matches!(self, InputEvent::Pointer { action: PointerAction::Move, .. })
    }
    pub fn kind(&self) -> &'static str {
        match self {
            InputEvent::Pointer { .. } => "pointer",
            InputEvent::Wheel { .. } => "wheel",
            InputEvent::Key { .. } => "key",
            InputEvent::Text { .. } => "text",
        }
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum PointerAction {
    Down,
    Up,
    Move,
    DblClick,
}

fn coord(v: Option<f64>, name: &str, inside: bool) -> Result<f64, String> {
    let (lo, hi) = if inside { (0.0, COORD_MAX) } else { (-COORD_MAX, 2.0 * COORD_MAX) };
    match v {
        Some(x) if x.is_finite() && (lo..=hi).contains(&x) => Ok(x),
        Some(_) => Err(format!("{name} out of range")),
        None => Err(format!("{name} missing")),
    }
}

fn size(v: Option<f64>, name: &str) -> Result<Option<f64>, String> {
    match v {
        None => Ok(None),
        Some(s) if s.is_finite() && (1.0..=COORD_MAX).contains(&s) => Ok(Some(s)),
        Some(_) => Err(format!("{name} out of range")),
    }
}

fn point(r: &RawInput, inside: bool) -> Result<Pt, String> {
    let pt = Pt { x: coord(r.x, "x", inside)?, y: coord(r.y, "y", inside)?, iw: size(r.iw, "iw")?, ih: size(r.ih, "ih")? };
    if inside {
        // un clic ou un cran de molette tombe DANS l'image (un glisser, lui, peut en sortir)
        if pt.iw.map(|w| pt.x > w).unwrap_or(false) || pt.ih.map(|h| pt.y > h).unwrap_or(false) {
            return Err("point outside the image".into());
        }
    }
    Ok(pt)
}

pub fn validate_input(r: &RawInput) -> Result<InputEvent, String> {
    let mods = r.mods.unwrap_or(0) & 0x0f;
    match r.kind.as_str() {
        "pointer" => {
            let action = match r.action.as_deref() {
                Some("down") => PointerAction::Down,
                Some("up") => PointerAction::Up,
                Some("move") => PointerAction::Move,
                Some("dblclick") => PointerAction::DblClick,
                _ => return Err("pointer action must be down, up, move or dblclick".into()),
            };
            let button = r.button.unwrap_or(0);
            if button > 2 {
                return Err("button out of range".into());
            }
            let buttons = r.buttons.unwrap_or(0);
            if buttons > 7 {
                return Err("buttons out of range".into());
            }
            let inside = matches!(action, PointerAction::Down | PointerAction::DblClick);
            Ok(InputEvent::Pointer { action, pt: point(r, inside)?, button, buttons, mods })
        }
        "wheel" => {
            let d = |v: Option<f64>| v.unwrap_or(0.0);
            let (dx, dy) = (d(r.dx), d(r.dy));
            if !dx.is_finite() || !dy.is_finite() || dx.abs() > 10000.0 || dy.abs() > 10000.0 {
                return Err("wheel delta out of range".into());
            }
            Ok(InputEvent::Wheel { pt: point(r, true)?, dx, dy, mods })
        }
        "key" => {
            let down = match r.action.as_deref() {
                Some("down") => true,
                Some("up") => false,
                _ => return Err("key action must be down or up".into()),
            };
            let key = r.key.clone().unwrap_or_default();
            let code = r.code.clone().unwrap_or_default();
            if key.is_empty() || key.chars().count() > 32 || code.chars().count() > 32 {
                return Err("bad key".into());
            }
            Ok(InputEvent::Key { down, key, code, mods })
        }
        "text" => {
            let text = r.text.clone().unwrap_or_default();
            if text.is_empty() || text.chars().count() > MAX_TEXT_INPUT || text.chars().any(|c| c.is_control()) {
                return Err("bad text".into());
            }
            Ok(InputEvent::Text { text })
        }
        _ => Err("unknown input kind".into()),
    }
}

/// Un message texte du studio → le message, ou une erreur courte (le studio la recoit en `error`).
pub fn parse(text: &str) -> Result<ClientMsg, String> {
    if text.len() > MAX_TEXT_MESSAGE {
        return Err("message too large".into());
    }
    let v: Value = serde_json::from_str(text).map_err(|_| "not JSON".to_string())?;
    if !v.is_object() {
        return Err("not a JSON object".into());
    }
    serde_json::from_value(v).map_err(|e| format!("bad message: {e}"))
}

/// L'en-tete d'une image (ou d'une tuile).
pub struct FrameHeader<'a> {
    pub seq: u64,
    pub id: u64,
    pub w: u32,
    pub h: u32,
    pub t_ms: u64,
    pub cap: &'a str,
    /// Taille d'origine si l'image a ete reduite (sinon w, h) ; pour une tuile : la taille de l'image entiere.
    pub src_w: u32,
    pub src_h: u32,
    /// Une tuile : sa place dans l'image entiere.
    pub tile: Option<(u32, u32)>,
    pub quality: u8,
    /// Le dernier geste injecte avant la capture (0 : aucun) ; l'image a-t-elle change pres du pointeur ?
    pub input_n: u64,
    pub near: bool,
    /// La taille a l ecran (pixels physiques) si l image est plus petite (une fenetre agrandie par Windows).
    pub display: Option<(u32, u32)>,
}

pub fn frame_packet(h: &FrameHeader, jpeg: &[u8]) -> Vec<u8> {
    let mut head = json!({ "seq": h.seq, "id": h.id, "w": h.w, "h": h.h, "t": h.t_ms, "enc": "jpeg", "cap": h.cap, "sw": h.src_w, "sh": h.src_h, "q": h.quality });
    if let Some((x, y)) = h.tile {
        head["x"] = json!(x);
        head["y"] = json!(y);
    }
    if let Some((pw, ph)) = h.display {
        head["pw"] = json!(pw);
        head["ph"] = json!(ph);
    }
    if h.input_n > 0 {
        head["in"] = json!(h.input_n);
        head["near"] = json!(h.near);
    }
    let head = head.to_string();
    let hb = head.as_bytes();
    let mut out = Vec::with_capacity(8 + hb.len() + jpeg.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(hb.len() as u32).to_le_bytes());
    out.extend_from_slice(hb);
    out.extend_from_slice(jpeg);
    out
}

/// Lire un paquet (les tests ; le studio fait la meme chose en JS).
#[cfg(test)]
pub fn read_packet(p: &[u8]) -> Option<(Value, &[u8])> {
    if p.len() < 8 || &p[..4] != MAGIC {
        return None;
    }
    let n = u32::from_le_bytes([p[4], p[5], p[6], p[7]]) as usize;
    if p.len() < 8 + n {
        return None;
    }
    let v: Value = serde_json::from_slice(&p[8..8 + n]).ok()?;
    Some((v, &p[8 + n..]))
}

pub fn error(code: &str, message: &str) -> String {
    json!({ "type": "error", "code": code, "message": message }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_messages() {
        assert_eq!(parse(r#"{"type":"hello","version":1,"token":"ab"}"#), Ok(ClientMsg::Hello { version: Some(1), token: Some("ab".into()), v: None, nonce: None, proof: None }));
        assert_eq!(parse(r#"{"type":"hello"}"#), Ok(ClientMsg::Hello { version: None, token: None, v: None, nonce: None, proof: None }));
        // (29 oct.) la v2 : un nonce et une preuve, jamais le jeton
        assert_eq!(
            parse(r#"{"type":"hello","version":1,"v":2,"nonce":"aa","proof":"bb"}"#),
            Ok(ClientMsg::Hello { version: Some(1), token: None, v: Some(2), nonce: Some("aa".into()), proof: Some("bb".into()) })
        );
        assert_eq!(parse(r#"{"type":"list"}"#), Ok(ClientMsg::List));
        assert_eq!(parse(r#"{"type":"live"}"#), Ok(ClientMsg::Live));
        assert_eq!(parse(r#"{"type":"watch","id":42}"#), Ok(ClientMsg::Watch { id: 42, fps: None, quality: None }));
        assert_eq!(parse(r#"{"type":"open","path":"2.0"}"#), Ok(ClientMsg::Open { path: "2.0".into() }));
        assert_eq!(parse(r#"{"type":"ack","seq":7}"#), Ok(ClientMsg::Ack { seq: 7 }));
        assert_eq!(parse(r#"{"type":"mode","mode":"focus"}"#), Ok(ClientMsg::Mode { mode: "focus".into() }));
        assert_eq!(
            parse(r#"{"type":"audio","action":"start","target":"live","rate":48000}"#),
            Ok(ClientMsg::Audio { action: "start".into(), target: Some("live".into()), rate: Some(48000), format: None, ms: None })
        );
        assert!(parse(r#"{"type":"audio"}"#).is_err(), "an action is needed");
        assert_eq!(parse(r#"{"type":"instances"}"#), Ok(ClientMsg::Instances));
        assert_eq!(
            parse(r#"{"type":"midi","to":"00000000000000ab","events":[{"t":12.5,"d":[144,60,100],"n":3},{"d":[128,60,0]}]}"#),
            Ok(ClientMsg::Midi {
                to: "00000000000000ab".into(),
                events: vec![MidiIn { t: Some(12.5), d: vec![144, 60, 100], n: Some(3) }, MidiIn { t: None, d: vec![128, 60, 0], n: None }]
            })
        );
        assert!(parse(r#"{"type":"midi","to":"ab","events":[{"d":[144,300,1]}]}"#).is_err(), "a byte is a byte");
        assert_eq!(parse(r#"{"type":"emitted","id":"ab"}"#), Ok(ClientMsg::Emitted { id: "ab".into() }));
        // (25 oct.) le lien sans loopMIDI
        assert_eq!(parse(r#"{"type":"link","sysex":["f07d5557"]}"#), Ok(ClientMsg::Link { sysex: Some(vec!["f07d5557".into()]), stop: None }));
        assert_eq!(parse(r#"{"type":"link","stop":true}"#), Ok(ClientMsg::Link { sysex: None, stop: Some(true) }));
        assert_eq!(parse(r#"{"type":"routes"}"#), Ok(ClientMsg::Routes));
        assert!(parse(r#"{"type":"link","sysex":[1,2]}"#).is_err());
        assert!(!link_chunk_ok("f07d55570101010001"), "too short");
        assert!(link_chunk_ok("F07D555701010100017b7df7"));
        assert!(!link_chunk_ok("f07d555701010100017b7df"), "odd length");
        assert!(!link_chunk_ok("f07d5557010101000190"), "no F7");
        assert!(!link_chunk_ok("f07e5557010101000100f7"), "another SysEx id");
        assert!(!link_chunk_ok("f07d55570101010001zzf7"));
        assert!(!link_chunk_ok(&format!("f07d5557{}f7", "00".repeat(300))), "too long");
        assert!(matches!(parse(r#"{"type":"place","x":1,"y":2,"w":3,"h":4}"#), Ok(ClientMsg::Place { .. })));
        assert!(parse("[1]").is_err());
        assert!(parse("nope").is_err());
        assert!(parse(r#"{"type":"format-disk"}"#).is_err());
        assert!(parse(r#"{"type":"watch","id":"x"}"#).is_err());
        assert!(parse(&format!(r#"{{"type":"list","pad":"{}"}}"#, "x".repeat(MAX_TEXT_MESSAGE))).is_err());
    }

    #[test]
    fn inputs_are_parsed_and_validated() {
        let m = parse(r#"{"type":"input","kind":"pointer","action":"down","x":10,"y":20,"iw":800,"ih":500,"button":0,"buttons":1,"mods":1,"n":3,"t":1.5}"#).unwrap();
        let ClientMsg::Input(r) = m else { panic!("not input") };
        assert_eq!(r.n, Some(3));
        assert_eq!(
            validate_input(&r),
            Ok(InputEvent::Pointer { action: PointerAction::Down, pt: Pt { x: 10.0, y: 20.0, iw: Some(800.0), ih: Some(500.0) }, button: 0, buttons: 1, mods: 1 })
        );
        let w = RawInput { kind: "wheel".into(), x: Some(1.0), y: Some(1.0), dy: Some(-120.0), ..Default::default() };
        assert!(matches!(validate_input(&w), Ok(InputEvent::Wheel { dy, .. }) if dy == -120.0));
        let k = RawInput { kind: "key".into(), action: Some("down".into()), key: Some("a".into()), code: Some("KeyA".into()), ..Default::default() };
        assert!(validate_input(&k).is_ok());
        let t = RawInput { kind: "text".into(), text: Some("pad".into()), ..Default::default() };
        assert_eq!(validate_input(&t), Ok(InputEvent::Text { text: "pad".into() }));
        let dbl = RawInput { kind: "pointer".into(), action: Some("dblclick".into()), x: Some(1.0), y: Some(1.0), ..Default::default() };
        assert!(matches!(validate_input(&dbl), Ok(InputEvent::Pointer { action: PointerAction::DblClick, .. })));
        // un glisser peut sortir de l'image (potard tire loin), un clic non
        let drag_out = RawInput { kind: "pointer".into(), action: Some("move".into()), x: Some(-300.0), y: Some(900.0), iw: Some(800.0), ih: Some(500.0), buttons: Some(1), ..Default::default() };
        assert!(validate_input(&drag_out).is_ok());
        let click_out = RawInput { kind: "pointer".into(), action: Some("down".into()), x: Some(900.0), y: Some(10.0), iw: Some(800.0), ih: Some(500.0), ..Default::default() };
        assert!(validate_input(&click_out).is_err());
        // refuses
        assert!(validate_input(&RawInput { kind: "pointer".into(), action: Some("down".into()), x: Some(-1.0), y: Some(0.0), ..Default::default() }).is_err());
        assert!(validate_input(&RawInput { kind: "pointer".into(), action: Some("move".into()), x: Some(-1e9), y: Some(0.0), ..Default::default() }).is_err());
        assert!(validate_input(&RawInput { kind: "pointer".into(), action: Some("down".into()), x: Some(f64::NAN), y: Some(0.0), ..Default::default() }).is_err());
        assert!(validate_input(&RawInput { kind: "pointer".into(), action: Some("drag".into()), x: Some(1.0), y: Some(1.0), ..Default::default() }).is_err());
        assert!(validate_input(&RawInput { kind: "pointer".into(), action: Some("down".into()), x: Some(1.0), y: Some(1.0), button: Some(3), ..Default::default() }).is_err());
        assert!(validate_input(&RawInput { kind: "pointer".into(), action: Some("down".into()), x: Some(1.0), y: Some(1.0), iw: Some(0.0), ..Default::default() }).is_err());
        assert!(validate_input(&RawInput { kind: "text".into(), text: Some("a\u{7}".into()), ..Default::default() }).is_err());
        assert!(validate_input(&RawInput { kind: "text".into(), text: Some("x".repeat(300)), ..Default::default() }).is_err());
        assert!(validate_input(&RawInput { kind: "shell".into(), ..Default::default() }).is_err());
    }

    #[test]
    fn frame_packets_round_trip() {
        let h = FrameHeader { seq: 3, id: 99, w: 2, h: 1, t_ms: 1_760_000_000_000, cap: "wgc", src_w: 2, src_h: 1, tile: None, quality: 80, input_n: 0, near: false, display: None };
        let p = frame_packet(&h, &[0xff, 0xd8, 0xff, 0xd9]);
        assert_eq!(&p[..4], b"UWF1");
        let (head, body) = read_packet(&p).unwrap();
        assert_eq!(head["seq"], 3);
        assert_eq!(head["id"], 99);
        assert_eq!(head["w"], 2);
        assert_eq!(head["t"], 1_760_000_000_000u64);
        assert_eq!(head["enc"], "jpeg");
        assert!(head.get("x").is_none() && head.get("in").is_none());
        assert_eq!(body, &[0xff, 0xd8, 0xff, 0xd9]);
        assert!(read_packet(b"UWF0....").is_none());
        let t = FrameHeader { tile: Some((40, 30)), input_n: 7, near: true, quality: 60, ..h };
        let (head, _) = read_packet(&frame_packet(&t, &[0xff, 0xd8])).unwrap();
        assert_eq!((head["x"].as_u64(), head["y"].as_u64(), head["in"].as_u64(), head["near"].as_bool(), head["q"].as_u64()), (Some(40), Some(30), Some(7), Some(true), Some(60)));
    }
}
