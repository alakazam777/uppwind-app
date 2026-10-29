// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Uppwind Link — le protocole entre les plugins du DAW (« Uppwind Link Return », « Uppwind Link Notes ») et le
//! compagnon. Version 2 (29 oct. 2026 : la poignee de main a preuves des deux cotes ; la version 1 du 25 oct. donnait
//! le secret en clair a qui ecoutait). Deux dependances, hmac et sha2 (RustCrypto, MIT/Apache-2.0) : le plugin, l'hote
//! d'essai et le compagnon partagent ce fichier (pas de jumeaux a garder d'accord), preuves comprises (`auth`).
//!
//! TCP sur 127.0.0.1 seulement. Le compagnon ecoute ; son port et un secret a part (PAS le jeton d'appairage du
//! studio : un plugin ne peut ni filmer ni cliquer) sont dans son fichier d'etat %USERPROFILE%\.uppwind-companion.json
//! (« pluginPort », « pluginSecret »), que le plugin lit. Le compagnon refuse tout pair qui n'est pas la machine
//! elle-meme. Des trames binaires, petit-boutistes :
//!
//! ```text
//!   0  4  len   u32   la taille de ce qui suit (le type + le contenu), de 1 a MAX_FRAME
//!   4  1  type  u8
//!   5  …  le contenu
//! ```
//!
//! La poignee de main (v2). Le secret ne circule jamais ; chaque cote prouve qu'il le connait, sur deux nonces neufs
//! (32 octets du generateur du systeme) et le port de la connexion (`auth`) :
//!
//! ```text
//!   plugin    → HELLO      version 2, nonce Np (a la place du secret de la v1), le reste comme avant
//!   compagnon → CHALLENGE  Nc, Pc = HMAC-SHA256(secret, "uwl-companion|" port "|" hex(Np) "|" hex(Nc))
//!   plugin      verifie Pc (temps constant) AVANT tout : faux ou absent → il ferme, attend, n'envoie aucun son et
//!               ne joue aucune note ; un WELCOME qui arrive sans CHALLENGE verifie ne vaut rien
//!   plugin    → AUTH       Pp = HMAC-SHA256(secret, "uwl-plugin|" port "|" hex(Nc) "|" hex(Np))
//!   compagnon   verifie Pp (temps constant) ; faux → WELCOME 1 et il ferme
//!   compagnon → WELCOME    0 (les trames peuvent partir), 3 (identite deja connectee : le plugin en prend une neuve)…
//! ```
//!
//! Le port (celui que le plugin a appele, celui que le compagnon ecoute) est dans les preuves : un programme qui
//! ecoute sur un vieux port du fichier d'etat ne peut pas relayer la poignee de main vers le vrai compagnon (les ports
//! different, les preuves aussi). Un refus du HELLO (version, format) part en WELCOME tout de suite, sans CHALLENGE.
//! Un plugin v1 (le secret en clair dans HELLO) est refuse : WELCOME 2 « an older Uppwind Link plugin… ».
//!
//! Plugin → compagnon :
//! - HELLO   0x01  la premiere trame, dans les 2 s (au plus MAX_HELLO octets) : « UWLP », version u16, kind u8
//!   (1 Return, 2 Notes), flags u8, nonce [32] (v1 : le secret), instance u64, rate u32, max_block u32, channels u8,
//!   pid u32, name (u8 + UTF-8), host (u8 + UTF-8 : l'exe du DAW, pour l'affichage seulement).
//! - AUTH    0x04  (v2) la preuve du plugin [32], la reponse au CHALLENGE (la seule trame permise entre les deux).
//! - AUDIO   0x10  (Return) pos u64 (la trame du premier echantillon), t f64 (l'instant de cette trame, ms depuis
//!   1970), rate u32, frames u16, ch u8, flags u8 (1 silence, 2 discontinuite, 4 contourne), puis frames × ch f32
//!   entrelaces. Les memes bits que les paquets « UWA1 » du studio.
//! - FORMAT  0x11  rate u32, max_block u32, channels u8 : au debut et a chaque changement (le DAW change de frequence).
//! - EMITTED 0x21  (Notes, la mesure) count u16, puis par evenement : n u32, t f64 (l'instant prevu de l'echantillon ou
//!   il est sorti, horloge du plugin), offset u32 (dans le bloc), frames u32 (la taille du bloc), late f32 (ms de
//!   retard ; 0 : a l'heure).
//! - TAG     0x22  (Notes, 25 oct.) u32 : le numero de son parametre « Link tag » (0..=TAG_MAX ; 0 : pas encore
//!   choisi), a la connexion et a chaque changement. Le Remote Script de Live l'ecrit ; le studio vise « tag:<n> ».
//! - STATS   0x30  du JSON (UTF-8), toutes les 2 s : compteurs et mesures, pour le journal. Rien de vital.
//!
//! Compagnon → plugin :
//! - CHALLENGE 0x03 (v2) le nonce du compagnon [32], sa preuve [32].
//! - WELCOME 0x02  version u16, status u8 (0 ok, 1 mauvais secret ou mauvaise preuve, 2 version, 3 instance deja
//!   connectee, 4 refuse), message (u8 + UTF-8). Sauf 0, la connexion se ferme ensuite. En v2, le verdict apres AUTH
//!   (ou le refus du HELLO).
//! - EVENTS  0x20  (Notes) count u16, puis par evenement, 16 octets : t f64 (ms depuis 1970 : quand le DAW doit le
//!   jouer), n u32 (son numero, pour la mesure), len u8, data [3] (un message MIDI de canal : notes, CC, pitch bend…).
//!
//! Les deux sens : BYE 0x7f, une raison (UTF-8), puis la connexion se ferme.

#![forbid(unsafe_code)]

use std::fmt;

/// (29 oct.) 2 : la poignee de main a preuves (CHALLENGE, AUTH). Le compagnon refuse un plugin v1.
pub const VERSION: u16 = 2;
/// La version d'avant : le secret en clair dans HELLO (refusee, mais reconnue pour dire quoi faire).
pub const V1: u16 = 1;
pub const MAGIC: [u8; 4] = *b"UWLP";
pub const SECRET_LEN: usize = 32;
pub const NONCE_LEN: usize = 32;
pub const PROOF_LEN: usize = 32;
/// Une trame AUDIO de 8192 trames stereo fait 64 Ko : large.
pub const MAX_FRAME: usize = 256 * 1024;
/// Avant l'appairage, une trame ne depasse pas ca (la poignee de main).
pub const MAX_HELLO: usize = 1024;
pub const MAX_NAME: usize = 64;
pub const MAX_EVENTS: usize = 512;
pub const MAX_AUDIO_FRAMES: usize = 8192;
pub const MAX_CHANNELS: u8 = 8;
pub const EVENT_BYTES: usize = 16;

pub mod ty {
    pub const HELLO: u8 = 0x01;
    pub const WELCOME: u8 = 0x02;
    /// (v2) compagnon → plugin : son nonce et sa preuve, la reponse au HELLO
    pub const CHALLENGE: u8 = 0x03;
    /// (v2) plugin → compagnon : sa preuve, la reponse au CHALLENGE
    pub const AUTH: u8 = 0x04;
    pub const AUDIO: u8 = 0x10;
    pub const FORMAT: u8 = 0x11;
    pub const EVENTS: u8 = 0x20;
    pub const EMITTED: u8 = 0x21;
    /// (25 oct.) Notes → compagnon : le numero « Link tag » (u32), a la connexion et a chaque changement
    pub const TAG: u8 = 0x22;
    pub const STATS: u8 = 0x30;
    pub const BYE: u8 = 0x7f;
}

pub mod status {
    pub const OK: u8 = 0;
    pub const BAD_SECRET: u8 = 1;
    pub const BAD_VERSION: u8 = 2;
    pub const DUPLICATE: u8 = 3;
    pub const REFUSED: u8 = 4;
}

/// Les bits d'une trame AUDIO (1 et 2 : les memes que les paquets « UWA1 » du studio).
pub mod flags {
    pub const SILENT: u8 = 1;
    pub const DISCONTINUITY: u8 = 2;
    pub const BYPASSED: u8 = 4;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Return,
    Notes,
}

impl Kind {
    pub fn code(self) -> u8 {
        match self {
            Kind::Return => 1,
            Kind::Notes => 2,
        }
    }
    pub fn from_code(v: u8) -> Option<Kind> {
        match v {
            1 => Some(Kind::Return),
            2 => Some(Kind::Notes),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Kind::Return => "return",
            Kind::Notes => "notes",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// La trame est trop courte pour ce qu'elle annonce.
    Short,
    BadMagic,
    /// Une longueur de trame hors bornes.
    BadLength(usize),
    BadUtf8,
    Bad(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Short => write!(f, "truncated frame"),
            Error::BadMagic => write!(f, "not an Uppwind Link plugin"),
            Error::BadLength(n) => write!(f, "bad frame length {n}"),
            Error::BadUtf8 => write!(f, "bad text"),
            Error::Bad(w) => write!(f, "bad {w}"),
        }
    }
}

impl std::error::Error for Error {}

// ─── ecrire / lire ───────────────────────────────────────────────────────────────────────────────────────────

/// Commencer une trame : la place de la longueur, le type. `finish` ecrit la longueur.
pub fn begin(ty: u8, out: &mut Vec<u8>) -> usize {
    let at = out.len();
    out.extend_from_slice(&[0, 0, 0, 0, ty]);
    at
}

pub fn finish(at: usize, out: &mut [u8]) {
    let len = (out.len() - at - 4) as u32;
    out[at..at + 4].copy_from_slice(&len.to_le_bytes());
}

fn put_str8(s: &str, out: &mut Vec<u8>) {
    // au plus 255 octets, coupe sur une frontiere de caractere
    let mut end = s.len().min(255);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    out.push(end as u8);
    out.extend_from_slice(&s.as_bytes()[..end]);
}

struct R<'a> {
    p: &'a [u8],
    i: usize,
}

impl<'a> R<'a> {
    fn new(p: &'a [u8]) -> R<'a> {
        R { p, i: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if self.i + n > self.p.len() {
            return Err(Error::Short);
        }
        let s = &self.p[self.i..self.i + n];
        self.i += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, Error> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, Error> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }
    fn f32(&mut self) -> Result<f32, Error> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn f64(&mut self) -> Result<f64, Error> {
        Ok(f64::from_bits(self.u64()?))
    }
    fn str8(&mut self) -> Result<String, Error> {
        let n = self.u8()? as usize;
        let b = self.take(n)?;
        std::str::from_utf8(b).map(|s| s.to_string()).map_err(|_| Error::BadUtf8)
    }
    fn rest(&self) -> &'a [u8] {
        &self.p[self.i..]
    }
}

// ─── HELLO / WELCOME ─────────────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Hello {
    pub version: u16,
    pub kind: Kind,
    pub flags: u8,
    /// (v2) le nonce du plugin, 32 octets neufs a chaque connexion. Un plugin v1 mettait ici le secret lui-meme, en
    /// clair : voir `v1_secret`.
    pub nonce: [u8; NONCE_LEN],
    /// L'identite de l'instance (au hasard a sa creation, gardee dans le projet du DAW).
    pub instance: u64,
    pub rate: u32,
    pub max_block: u32,
    pub channels: u8,
    pub pid: u32,
    pub name: String,
    pub host: String,
}

impl Hello {
    /// Ce qu'un plugin v1 (refuse) envoyait a la place du nonce : le secret, en clair. None pour un plugin v2.
    pub fn v1_secret(&self) -> Option<&[u8; SECRET_LEN]> {
        (self.version == V1).then_some(&self.nonce)
    }
}

pub fn encode_hello(h: &Hello, out: &mut Vec<u8>) {
    let at = begin(ty::HELLO, out);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&h.version.to_le_bytes());
    out.push(h.kind.code());
    out.push(h.flags);
    out.extend_from_slice(&h.nonce);
    out.extend_from_slice(&h.instance.to_le_bytes());
    out.extend_from_slice(&h.rate.to_le_bytes());
    out.extend_from_slice(&h.max_block.to_le_bytes());
    out.push(h.channels);
    out.extend_from_slice(&h.pid.to_le_bytes());
    put_str8(&h.name, out);
    put_str8(&h.host, out);
    finish(at, out);
}

pub fn decode_hello(p: &[u8]) -> Result<Hello, Error> {
    let mut r = R::new(p);
    if r.take(4)? != MAGIC {
        return Err(Error::BadMagic);
    }
    let version = r.u16()?;
    let kind = Kind::from_code(r.u8()?).ok_or(Error::Bad("kind"))?;
    let flags = r.u8()?;
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(r.take(NONCE_LEN)?);
    let instance = r.u64()?;
    let rate = r.u32()?;
    let max_block = r.u32()?;
    let channels = r.u8()?;
    let pid = r.u32()?;
    let name = r.str8()?;
    let host = r.str8()?;
    if name.chars().count() > MAX_NAME || name.chars().any(|c| c.is_control()) {
        return Err(Error::Bad("name"));
    }
    if host.chars().any(|c| c.is_control()) {
        return Err(Error::Bad("host"));
    }
    Ok(Hello { version, kind, flags, nonce, instance, rate, max_block, channels, pid, name, host })
}

/// (v2) La reponse du compagnon au HELLO : son nonce et sa preuve (`auth::companion_proof`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    pub nonce: [u8; NONCE_LEN],
    pub proof: [u8; PROOF_LEN],
}

pub fn encode_challenge(c: &Challenge, out: &mut Vec<u8>) {
    let at = begin(ty::CHALLENGE, out);
    out.extend_from_slice(&c.nonce);
    out.extend_from_slice(&c.proof);
    finish(at, out);
}

/// Exactement 64 octets (une trame plus courte ou plus longue est refusee).
pub fn decode_challenge(p: &[u8]) -> Result<Challenge, Error> {
    if p.len() != NONCE_LEN + PROOF_LEN {
        return Err(Error::Short);
    }
    let mut c = Challenge { nonce: [0; NONCE_LEN], proof: [0; PROOF_LEN] };
    c.nonce.copy_from_slice(&p[..NONCE_LEN]);
    c.proof.copy_from_slice(&p[NONCE_LEN..]);
    Ok(c)
}

/// (v2) La preuve du plugin (`auth::plugin_proof`), la reponse au CHALLENGE.
pub fn encode_auth(proof: &[u8; PROOF_LEN], out: &mut Vec<u8>) {
    let at = begin(ty::AUTH, out);
    out.extend_from_slice(proof);
    finish(at, out);
}

/// Exactement 32 octets.
pub fn decode_auth(p: &[u8]) -> Result<[u8; PROOF_LEN], Error> {
    p.try_into().map_err(|_| Error::Short)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Welcome {
    pub version: u16,
    pub status: u8,
    pub message: String,
}

pub fn encode_welcome(w: &Welcome, out: &mut Vec<u8>) {
    let at = begin(ty::WELCOME, out);
    out.extend_from_slice(&w.version.to_le_bytes());
    out.push(w.status);
    put_str8(&w.message, out);
    finish(at, out);
}

pub fn decode_welcome(p: &[u8]) -> Result<Welcome, Error> {
    let mut r = R::new(p);
    Ok(Welcome { version: r.u16()?, status: r.u8()?, message: r.str8()? })
}

// ─── AUDIO / FORMAT (Return) ─────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioHeader {
    pub pos: u64,
    pub t_ms: f64,
    pub rate: u32,
    pub frames: u16,
    pub channels: u8,
    pub flags: u8,
}

pub const AUDIO_HEADER: usize = 8 + 8 + 4 + 2 + 1 + 1;

/// Une trame AUDIO : l'en-tete, puis `samples` (frames × channels f32 entrelaces).
pub fn encode_audio(h: &AudioHeader, samples: &[f32], out: &mut Vec<u8>) {
    let at = begin(ty::AUDIO, out);
    out.extend_from_slice(&h.pos.to_le_bytes());
    out.extend_from_slice(&h.t_ms.to_le_bytes());
    out.extend_from_slice(&h.rate.to_le_bytes());
    out.extend_from_slice(&h.frames.to_le_bytes());
    out.push(h.channels);
    out.push(h.flags);
    out.reserve(samples.len() * 4);
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    finish(at, out);
}

/// Lire une trame AUDIO ; les echantillons vont dans `samples` (vide d'abord).
pub fn decode_audio_into(p: &[u8], samples: &mut Vec<f32>) -> Result<AudioHeader, Error> {
    let mut r = R::new(p);
    let h = AudioHeader { pos: r.u64()?, t_ms: r.f64()?, rate: r.u32()?, frames: r.u16()?, channels: r.u8()?, flags: r.u8()? };
    if h.channels == 0 || h.channels > MAX_CHANNELS {
        return Err(Error::Bad("channels"));
    }
    if h.frames as usize > MAX_AUDIO_FRAMES {
        return Err(Error::Bad("frames"));
    }
    if !(8000..=384_000).contains(&h.rate) {
        return Err(Error::Bad("rate"));
    }
    if !h.t_ms.is_finite() {
        return Err(Error::Bad("time"));
    }
    let n = h.frames as usize * h.channels as usize;
    let body = r.rest();
    if body.len() != n * 4 {
        return Err(Error::Short);
    }
    samples.clear();
    samples.reserve(n);
    for c in body.chunks_exact(4) {
        let v = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        // un NaN ou un infini dans le son du DAW : du silence (le studio ne doit jamais le recevoir)
        samples.push(if v.is_finite() { v } else { 0.0 });
    }
    Ok(h)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub rate: u32,
    pub max_block: u32,
    pub channels: u8,
}

pub fn encode_format(f: &Format, out: &mut Vec<u8>) {
    let at = begin(ty::FORMAT, out);
    out.extend_from_slice(&f.rate.to_le_bytes());
    out.extend_from_slice(&f.max_block.to_le_bytes());
    out.push(f.channels);
    finish(at, out);
}

pub fn decode_format(p: &[u8]) -> Result<Format, Error> {
    let mut r = R::new(p);
    Ok(Format { rate: r.u32()?, max_block: r.u32()?, channels: r.u8()? })
}

// ─── EVENTS / EMITTED (Notes) ────────────────────────────────────────────────────────────────────────────────

/// Un message MIDI a jouer : `t` en ms depuis 1970 (l'instant ou le DAW doit le jouer), `n` son numero (la mesure).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Event {
    pub t_ms: f64,
    pub n: u32,
    pub len: u8,
    pub data: [u8; 3],
}

/// La longueur d'un message MIDI de canal d'apres son statut (None : pas un message de canal, refuse).
pub fn midi_len(status: u8) -> Option<u8> {
    match status & 0xf0 {
        0x80 | 0x90 | 0xa0 | 0xb0 | 0xe0 => Some(3),
        0xc0 | 0xd0 => Some(2),
        _ => None,
    }
}

/// Un message de canal bien forme (pas de SysEx, pas de messages systeme : la phase 0 n'en a pas besoin).
pub fn valid_midi(len: u8, d: &[u8; 3]) -> bool {
    match midi_len(d[0]) {
        Some(l) if l == len => d[1..len as usize].iter().all(|b| *b < 0x80),
        _ => false,
    }
}

pub fn encode_events(evs: &[Event], out: &mut Vec<u8>) {
    let at = begin(ty::EVENTS, out);
    let n = evs.len().min(MAX_EVENTS);
    out.extend_from_slice(&(n as u16).to_le_bytes());
    for e in &evs[..n] {
        out.extend_from_slice(&e.t_ms.to_le_bytes());
        out.extend_from_slice(&e.n.to_le_bytes());
        out.push(e.len);
        out.extend_from_slice(&e.data);
    }
    finish(at, out);
}

/// Lire une trame EVENTS ; un message mal forme refuse toute la trame.
pub fn decode_events_into(p: &[u8], out: &mut Vec<Event>) -> Result<(), Error> {
    let mut r = R::new(p);
    let n = r.u16()? as usize;
    if n > MAX_EVENTS {
        return Err(Error::Bad("event count"));
    }
    if r.rest().len() != n * EVENT_BYTES {
        return Err(Error::Short);
    }
    out.clear();
    for _ in 0..n {
        let t_ms = r.f64()?;
        let num = r.u32()?;
        let len = r.u8()?;
        let d = r.take(3)?;
        let data = [d[0], d[1], d[2]];
        if !t_ms.is_finite() {
            return Err(Error::Bad("event time"));
        }
        if !valid_midi(len, &data) {
            return Err(Error::Bad("midi message"));
        }
        out.push(Event { t_ms, n: num, len, data });
    }
    Ok(())
}

/// Ou et quand le plugin a sorti un evenement (la mesure de la gigue).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Emitted {
    pub n: u32,
    /// L'instant prevu de l'echantillon ou l'evenement est sorti (horloge filtree du plugin, ms depuis 1970).
    pub t_ms: f64,
    pub offset: u32,
    pub frames: u32,
    /// Le retard (ms) quand l'evenement est arrive trop tard pour sa place exacte ; 0 sinon.
    pub late_ms: f32,
}

pub const EMITTED_BYTES: usize = 4 + 8 + 4 + 4 + 4;

pub fn encode_emitted(list: &[Emitted], out: &mut Vec<u8>) {
    let at = begin(ty::EMITTED, out);
    let n = list.len().min(MAX_EVENTS);
    out.extend_from_slice(&(n as u16).to_le_bytes());
    for e in &list[..n] {
        out.extend_from_slice(&e.n.to_le_bytes());
        out.extend_from_slice(&e.t_ms.to_le_bytes());
        out.extend_from_slice(&e.offset.to_le_bytes());
        out.extend_from_slice(&e.frames.to_le_bytes());
        out.extend_from_slice(&e.late_ms.to_le_bytes());
    }
    finish(at, out);
}

pub fn decode_emitted_into(p: &[u8], out: &mut Vec<Emitted>) -> Result<(), Error> {
    let mut r = R::new(p);
    let n = r.u16()? as usize;
    if n > MAX_EVENTS || r.rest().len() != n * EMITTED_BYTES {
        return Err(Error::Short);
    }
    out.clear();
    for _ in 0..n {
        out.push(Emitted { n: r.u32()?, t_ms: r.f64()?, offset: r.u32()?, frames: r.u32()?, late_ms: r.f32()? });
    }
    Ok(())
}

// ─── TAG (25 oct.) ───────────────────────────────────────────────────────────────────────────────────────────

/// Le numero d'un Notes (son parametre « Link tag », 1..=TAG_MAX ; 0 : pas encore choisi). Le Remote Script de Live
/// le choisit et l'ecrit ; le plugin l'annonce (a la connexion, puis a chaque changement) ; le studio envoie les notes
/// d'une piste « vers tag:<n> ». Un compagnon d'avant ignore cette trame.
pub const TAG_MAX: u32 = 255;

pub fn encode_tag(tag: u32, out: &mut Vec<u8>) {
    let at = begin(ty::TAG, out);
    out.extend_from_slice(&tag.min(TAG_MAX).to_le_bytes());
    finish(at, out);
}

pub fn decode_tag(p: &[u8]) -> Result<u32, Error> {
    if p.len() != 4 {
        return Err(Error::Short);
    }
    let t = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
    if t > TAG_MAX {
        return Err(Error::Bad("tag"));
    }
    Ok(t)
}

// ─── STATS / BYE ─────────────────────────────────────────────────────────────────────────────────────────────

pub fn encode_text(ty: u8, text: &str, out: &mut Vec<u8>) {
    let at = begin(ty, out);
    let max = MAX_FRAME - 1;
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    out.extend_from_slice(&text.as_bytes()[..end]);
    finish(at, out);
}

pub fn decode_text(p: &[u8]) -> Result<&str, Error> {
    std::str::from_utf8(p).map_err(|_| Error::BadUtf8)
}

// ─── le decoupage du flux TCP en trames ─────────────────────────────────────────────────────────────────────

/// Les octets arrives (dans n'importe quel decoupage) → des trames entieres.
pub struct FrameReader {
    buf: Vec<u8>,
    start: usize,
    max: usize,
}

impl FrameReader {
    pub fn new(max: usize) -> FrameReader {
        FrameReader { buf: Vec::with_capacity(8192), start: 0, max }
    }
    /// La taille maximale d'une trame (MAX_HELLO avant la poignee de main, MAX_FRAME ensuite).
    pub fn set_max(&mut self, max: usize) {
        self.max = max;
    }
    pub fn extend(&mut self, data: &[u8]) {
        if self.start > 0 && self.start == self.buf.len() {
            self.buf.clear();
            self.start = 0;
        } else if self.start > 64 * 1024 {
            self.buf.drain(..self.start);
            self.start = 0;
        }
        self.buf.extend_from_slice(data);
    }
    /// Les octets en attente (pas encore une trame entiere).
    pub fn pending(&self) -> usize {
        self.buf.len() - self.start
    }
    /// La prochaine trame entiere : (type, contenu). Une longueur hors bornes est une erreur (on ferme).
    pub fn next(&mut self) -> Result<Option<(u8, Vec<u8>)>, Error> {
        let avail = &self.buf[self.start..];
        if avail.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_le_bytes([avail[0], avail[1], avail[2], avail[3]]) as usize;
        if len == 0 || len > self.max {
            return Err(Error::BadLength(len));
        }
        if avail.len() < 4 + len {
            return Ok(None);
        }
        let ty = avail[4];
        let payload = avail[5..4 + len].to_vec();
        self.start += 4 + len;
        Ok(Some((ty, payload)))
    }
}

// ─── le secret ───────────────────────────────────────────────────────────────────────────────────────────────

/// 64 caracteres hexadecimaux → 32 octets.
pub fn parse_secret(s: &str) -> Option<[u8; SECRET_LEN]> {
    let s = s.trim();
    if s.len() != SECRET_LEN * 2 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; SECRET_LEN];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

pub fn secret_hex(b: &[u8; SECRET_LEN]) -> String {
    let mut s = String::with_capacity(SECRET_LEN * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

/// Comparer deux secrets en temps constant.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut d = 0u8;
    for (x, y) in a.iter().zip(b) {
        d |= x ^ y;
    }
    d == 0
}

// ─── les preuves (v2, 29 oct.) ───────────────────────────────────────────────────────────────────────────────

/// Les preuves de la poignee de main : HMAC-SHA256 sur une etiquette (qui prouve), le port de la connexion et les deux
/// nonces en hexadecimal minuscule, le texte « <etiquette><port>|<hex 1>|<hex 2> ». Le compagnon s'en sert aussi avec
/// le studio (le jeton d'appairage pour cle : companion/src/security.rs, public/js/atelier/companion-link.js).
/// La verification passe par `Mac::verify_slice` (temps constant). Le secret, lui, ne circule jamais.
pub mod auth {
    use super::{NONCE_LEN, PROOF_LEN, SECRET_LEN};
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;

    /// Ce que le compagnon prouve (au plugin ; au studio aussi, avec le jeton)
    pub const COMPANION: &str = "uwl-companion|";
    /// Ce que le plugin prouve
    pub const PLUGIN: &str = "uwl-plugin|";
    /// Ce que le studio prouve (companion/src/security.rs)
    pub const STUDIO: &str = "uwl-studio|";

    pub type Nonce = [u8; NONCE_LEN];

    fn push_hex(b: &[u8], out: &mut Vec<u8>) {
        const H: &[u8; 16] = b"0123456789abcdef";
        for x in b {
            out.push(H[(x >> 4) as usize]);
            out.push(H[(x & 15) as usize]);
        }
    }

    /// Le texte signe : « <etiquette><port>|<hex(first)>|<hex(second)> » (le port en decimal, sans zero devant).
    pub fn message(label: &str, port: u16, first: &Nonce, second: &Nonce) -> Vec<u8> {
        let mut m = Vec::with_capacity(label.len() + 7 + 4 * NONCE_LEN + 1);
        m.extend_from_slice(label.as_bytes());
        m.extend_from_slice(port.to_string().as_bytes());
        m.push(b'|');
        push_hex(first, &mut m);
        m.push(b'|');
        push_hex(second, &mut m);
        m
    }

    fn mac(key: &[u8], label: &str, port: u16, first: &Nonce, second: &Nonce) -> Hmac<Sha256> {
        // (HMAC prend une cle de n'importe quelle longueur : pas d'erreur possible ici)
        let mut m = <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("HMAC takes keys of any length");
        m.update(&message(label, port, first, second));
        m
    }

    pub fn proof(key: &[u8], label: &str, port: u16, first: &Nonce, second: &Nonce) -> [u8; PROOF_LEN] {
        mac(key, label, port, first, second).finalize().into_bytes().into()
    }

    /// La preuve recue est-elle la bonne ? (temps constant ; une longueur fausse : non)
    pub fn verify(key: &[u8], label: &str, port: u16, first: &Nonce, second: &Nonce, given: &[u8]) -> bool {
        given.len() == PROOF_LEN && mac(key, label, port, first, second).verify_slice(given).is_ok()
    }

    /// Celle du compagnon au plugin : HMAC(secret, "uwl-companion|" port "|" hex(Np) "|" hex(Nc)).
    pub fn companion_proof(secret: &[u8; SECRET_LEN], port: u16, plugin_nonce: &Nonce, companion_nonce: &Nonce) -> [u8; PROOF_LEN] {
        proof(secret, COMPANION, port, plugin_nonce, companion_nonce)
    }

    pub fn companion_proof_ok(secret: &[u8; SECRET_LEN], port: u16, plugin_nonce: &Nonce, companion_nonce: &Nonce, given: &[u8]) -> bool {
        verify(secret, COMPANION, port, plugin_nonce, companion_nonce, given)
    }

    /// Celle du plugin au compagnon : HMAC(secret, "uwl-plugin|" port "|" hex(Nc) "|" hex(Np)).
    pub fn plugin_proof(secret: &[u8; SECRET_LEN], port: u16, companion_nonce: &Nonce, plugin_nonce: &Nonce) -> [u8; PROOF_LEN] {
        proof(secret, PLUGIN, port, companion_nonce, plugin_nonce)
    }

    pub fn plugin_proof_ok(secret: &[u8; SECRET_LEN], port: u16, companion_nonce: &Nonce, plugin_nonce: &Nonce, given: &[u8]) -> bool {
        verify(secret, PLUGIN, port, companion_nonce, plugin_nonce, given)
    }
}

/// L'identite d'une instance en texte (16 caracteres hexadecimaux) : ce que le studio voit et renvoie.
pub fn instance_hex(id: u64) -> String {
    format!("{id:016x}")
}

pub fn parse_instance(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() || s.len() > 16 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(s, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_frame(bytes: &[u8]) -> (u8, Vec<u8>) {
        let mut r = FrameReader::new(MAX_FRAME);
        r.extend(bytes);
        let f = r.next().unwrap().unwrap();
        assert!(r.next().unwrap().is_none());
        assert_eq!(r.pending(), 0);
        f
    }

    #[test]
    fn hello_round_trip_and_checks() {
        let h = Hello {
            version: VERSION,
            kind: Kind::Notes,
            flags: 0,
            nonce: [7u8; 32],
            instance: 0x0123_4567_89ab_cdef,
            rate: 48000,
            max_block: 512,
            channels: 2,
            pid: 4242,
            name: "Uppwind Link Notes 89ab".into(),
            host: "Ableton Live 12 Suite.exe".into(),
        };
        let mut out = Vec::new();
        encode_hello(&h, &mut out);
        assert!(out.len() < MAX_HELLO);
        let (t, p) = one_frame(&out);
        assert_eq!(t, ty::HELLO);
        assert_eq!(decode_hello(&p).unwrap(), h);
        // pas un plugin
        let mut bad = p.clone();
        bad[0] = b'X';
        assert_eq!(decode_hello(&bad), Err(Error::BadMagic));
        // tronque
        assert_eq!(decode_hello(&p[..p.len() - 3]), Err(Error::Short));
        // un caractere de controle dans le nom
        let mut h2 = h.clone();
        h2.name = "a\u{7}b".into();
        let mut o2 = Vec::new();
        encode_hello(&h2, &mut o2);
        assert_eq!(decode_hello(&one_frame(&o2).1), Err(Error::Bad("name")));
        // un nom trop long est coupe a 255 octets sur une frontiere de caractere
        let mut h3 = h.clone();
        h3.host = "é".repeat(200);
        let mut o3 = Vec::new();
        encode_hello(&h3, &mut o3);
        let back = decode_hello(&one_frame(&o3).1).unwrap();
        assert_eq!(back.host.len(), 254);
        // (v2) le nonce n'est jamais pris pour un secret ; un HELLO v1 (meme place, le secret en clair) se reconnait
        assert_eq!(h.v1_secret(), None);
        let old = Hello { version: V1, ..h.clone() };
        assert_eq!(old.v1_secret(), Some(&[7u8; 32]));
    }

    #[test]
    fn challenge_and_auth_frames_have_exact_sizes() {
        let c = Challenge { nonce: [0x22; NONCE_LEN], proof: [0x5c; PROOF_LEN] };
        let mut out = Vec::new();
        encode_challenge(&c, &mut out);
        assert!(out.len() < MAX_HELLO);
        let (t, p) = one_frame(&out);
        assert_eq!((t, decode_challenge(&p)), (ty::CHALLENGE, Ok(c)));
        assert_eq!(decode_challenge(&p[..63]), Err(Error::Short));
        assert_eq!(decode_challenge(&[p.clone(), vec![0]].concat()), Err(Error::Short), "one byte too many");
        out.clear();
        encode_auth(&[0xa5; PROOF_LEN], &mut out);
        let (t, p) = one_frame(&out);
        assert_eq!((t, decode_auth(&p)), (ty::AUTH, Ok([0xa5; PROOF_LEN])));
        assert_eq!(decode_auth(&p[..31]), Err(Error::Short));
        assert_eq!(decode_auth(&[0u8; 33]), Err(Error::Short));
    }

    /// Les valeurs attendues viennent d'ailleurs (le module hmac de Python, 29 oct.) : la meme recette que le studio
    /// (Web Crypto) et le faux compagnon des essais.
    #[test]
    fn proofs_match_an_independent_hmac_and_bind_label_port_and_nonces() {
        let key: [u8; 32] = std::array::from_fn(|i| i as u8);
        let (np, nc, ns) = ([0x11u8; 32], [0x22u8; 32], [0x33u8; 32]);
        assert_eq!(
            std::str::from_utf8(&auth::message(auth::COMPANION, 47411, &np, &nc)).unwrap(),
            format!("uwl-companion|47411|{}|{}", "11".repeat(32), "22".repeat(32))
        );
        let pc = auth::companion_proof(&key, 47411, &np, &nc);
        assert_eq!(secret_hex(&pc), "3249cdee13451d6d64f6daf0b32fe2ce10177ea23bfbfacd76daeec9b7c0f5a1");
        let pp = auth::plugin_proof(&key, 47411, &nc, &np);
        assert_eq!(secret_hex(&pp), "dae2f9af0929923d1b5c741a39c6e011f0dd9bd19deaa6d67740c90f7d7c8691");
        assert_eq!(secret_hex(&auth::proof(&key, auth::STUDIO, 47410, &nc, &ns)), "fc4756cc3ed3f609d234404a686e1e70b2f4e2faa4acc1fead089ed839782746");
        assert_eq!(secret_hex(&auth::proof(&key, auth::COMPANION, 47410, &ns, &nc)), "2ddee8d1ac156e9775b516a12b88a8ac4e598390d0ed7ef8b4d2eb492ff986e9");
        assert!(auth::companion_proof_ok(&key, 47411, &np, &nc, &pc));
        assert!(auth::plugin_proof_ok(&key, 47411, &nc, &np, &pp));
        // chaque morceau compte : un autre secret, un autre port (un relais), d'autres nonces (un rejeu), les roles
        // echanges (un reflet), une preuve tronquee ou vide
        let mut other = key;
        other[0] ^= 1;
        assert!(!auth::companion_proof_ok(&other, 47411, &np, &nc, &pc), "another secret");
        assert!(!auth::companion_proof_ok(&key, 47412, &np, &nc, &pc), "another port: a relay through a stale port fails");
        assert!(!auth::companion_proof_ok(&key, 47411, &[0x12; 32], &nc, &pc), "a new plugin nonce: an old proof is worthless");
        assert!(!auth::companion_proof_ok(&key, 47411, &np, &[0x23; 32], &pc), "a new companion nonce");
        assert!(!auth::companion_proof_ok(&key, 47411, &nc, &np, &pc), "the nonces swapped");
        assert!(!auth::plugin_proof_ok(&key, 47411, &nc, &np, &pc), "the companion's proof is not a plugin's");
        assert!(!auth::companion_proof_ok(&key, 47411, &np, &nc, &pp), "nor the other way round");
        assert!(!auth::companion_proof_ok(&key, 47411, &np, &nc, &pc[..31]));
        assert!(!auth::companion_proof_ok(&key, 47411, &np, &nc, &[]));
        let mut flipped = pc;
        flipped[31] ^= 0x80;
        assert!(!auth::companion_proof_ok(&key, 47411, &np, &nc, &flipped));
    }

    #[test]
    fn welcome_round_trip() {
        let w = Welcome { version: VERSION, status: status::DUPLICATE, message: "instance already connected".into() };
        let mut out = Vec::new();
        encode_welcome(&w, &mut out);
        let (t, p) = one_frame(&out);
        assert_eq!(t, ty::WELCOME);
        assert_eq!(decode_welcome(&p).unwrap(), w);
    }

    #[test]
    fn audio_round_trip_and_sanitizing() {
        let h = AudioHeader { pos: 1 << 40, t_ms: 1_761_300_000_123.25, rate: 48000, frames: 3, channels: 2, flags: flags::DISCONTINUITY };
        let s = [0.5f32, -0.5, 0.25, f32::NAN, 1.0, f32::INFINITY];
        let mut out = Vec::new();
        encode_audio(&h, &s, &mut out);
        assert_eq!(out.len(), 5 + AUDIO_HEADER + 24);
        let (t, p) = one_frame(&out);
        assert_eq!(t, ty::AUDIO);
        let mut got = Vec::new();
        assert_eq!(decode_audio_into(&p, &mut got).unwrap(), h);
        assert_eq!(got, vec![0.5, -0.5, 0.25, 0.0, 1.0, 0.0], "NaN and infinity become silence");
        // le nombre d'echantillons ne colle pas a l'en-tete
        assert_eq!(decode_audio_into(&p[..p.len() - 4], &mut got), Err(Error::Short));
        let bad = AudioHeader { channels: 0, ..h };
        let mut o2 = Vec::new();
        encode_audio(&bad, &[], &mut o2);
        assert_eq!(decode_audio_into(&one_frame(&o2).1, &mut got), Err(Error::Bad("channels")));
        let bad = AudioHeader { rate: 1000, frames: 0, ..h };
        let mut o3 = Vec::new();
        encode_audio(&bad, &[], &mut o3);
        assert_eq!(decode_audio_into(&one_frame(&o3).1, &mut got), Err(Error::Bad("rate")));
    }

    #[test]
    fn events_round_trip_and_validation() {
        let evs = vec![
            Event { t_ms: 1000.5, n: 1, len: 3, data: [0x90, 60, 100] },
            Event { t_ms: 1001.0, n: 2, len: 3, data: [0x80, 60, 0] },
            Event { t_ms: 1002.0, n: 3, len: 3, data: [0xe1, 0x00, 0x40] },
            Event { t_ms: 1003.0, n: 4, len: 2, data: [0xc2, 5, 0] },
            Event { t_ms: 1004.0, n: 5, len: 3, data: [0xbf, 64, 127] },
        ];
        let mut out = Vec::new();
        encode_events(&evs, &mut out);
        assert_eq!(out.len(), 5 + 2 + evs.len() * EVENT_BYTES);
        let (t, p) = one_frame(&out);
        assert_eq!(t, ty::EVENTS);
        let mut got = Vec::new();
        decode_events_into(&p, &mut got).unwrap();
        assert_eq!(got, evs);
        // un SysEx, un message systeme, une donnee >= 0x80, une longueur fausse : refuse
        for bad in [
            Event { t_ms: 1.0, n: 1, len: 3, data: [0xf0, 1, 2] },
            Event { t_ms: 1.0, n: 1, len: 1, data: [0xf8, 0, 0] },
            Event { t_ms: 1.0, n: 1, len: 3, data: [0x90, 128, 1] },
            Event { t_ms: 1.0, n: 1, len: 2, data: [0x90, 60, 1] },
            Event { t_ms: f64::NAN, n: 1, len: 3, data: [0x90, 60, 1] },
        ] {
            let mut o = Vec::new();
            encode_events(&[bad], &mut o);
            assert!(decode_events_into(&one_frame(&o).1, &mut got).is_err(), "{bad:?}");
        }
        assert!(valid_midi(2, &[0xd3, 40, 0]));
        assert!(!valid_midi(3, &[0x3c, 40, 0]), "running status is not accepted");
    }

    #[test]
    fn emitted_round_trip() {
        let e = vec![Emitted { n: 9, t_ms: 12.5, offset: 77, frames: 128, late_ms: 0.0 }, Emitted { n: 10, t_ms: 13.0, offset: 0, frames: 128, late_ms: 1.5 }];
        let mut out = Vec::new();
        encode_emitted(&e, &mut out);
        let (t, p) = one_frame(&out);
        assert_eq!(t, ty::EMITTED);
        let mut got = Vec::new();
        decode_emitted_into(&p, &mut got).unwrap();
        assert_eq!(got, e);
    }

    #[test]
    fn tag_round_trip_and_bounds() {
        let mut out = Vec::new();
        encode_tag(42, &mut out);
        let (t, p) = one_frame(&out);
        assert_eq!((t, decode_tag(&p)), (ty::TAG, Ok(42)));
        out.clear();
        encode_tag(9999, &mut out);
        assert_eq!(decode_tag(&one_frame(&out).1), Ok(TAG_MAX), "clamped when written");
        assert_eq!(decode_tag(&[1, 0, 0]), Err(Error::Short));
        assert_eq!(decode_tag(&256u32.to_le_bytes()), Err(Error::Bad("tag")));
    }

    #[test]
    fn frames_split_anywhere_and_bad_lengths() {
        let mut all = Vec::new();
        encode_format(&Format { rate: 44100, max_block: 1024, channels: 2 }, &mut all);
        encode_text(ty::STATS, r#"{"blocks":3}"#, &mut all);
        encode_text(ty::BYE, "bye", &mut all);
        // octet par octet
        let mut r = FrameReader::new(MAX_FRAME);
        let mut got = Vec::new();
        for b in &all {
            r.extend(std::slice::from_ref(b));
            while let Some(f) = r.next().unwrap() {
                got.push(f);
            }
        }
        assert_eq!(got.len(), 3);
        assert_eq!(decode_format(&got[0].1).unwrap(), Format { rate: 44100, max_block: 1024, channels: 2 });
        assert_eq!((got[1].0, decode_text(&got[1].1).unwrap()), (ty::STATS, r#"{"blocks":3}"#));
        assert_eq!((got[2].0, decode_text(&got[2].1).unwrap()), (ty::BYE, "bye"));
        // une longueur nulle, trop grande (un navigateur qui ecrit « POST / HTTP/1.1 » : 0x54534f50 octets)
        let mut r = FrameReader::new(MAX_HELLO);
        r.extend(b"POST / HTTP/1.1\r\n");
        assert!(matches!(r.next(), Err(Error::BadLength(_))));
        let mut r = FrameReader::new(MAX_HELLO);
        r.extend(&[0, 0, 0, 0, 1]);
        assert_eq!(r.next(), Err(Error::BadLength(0)));
    }

    #[test]
    fn secrets_and_instances() {
        let s = [0xabu8; 32];
        let h = secret_hex(&s);
        assert_eq!(h.len(), 64);
        assert_eq!(parse_secret(&h), Some(s));
        assert_eq!(parse_secret(&h[1..]), None);
        assert_eq!(parse_secret(&h.replace('a', "z")), None);
        assert!(ct_eq(&s, &s));
        assert!(!ct_eq(&s, &[0xab; 31]));
        let mut t = s;
        t[31] = 0;
        assert!(!ct_eq(&s, &t));
        assert_eq!(instance_hex(0xbeef), "000000000000beef");
        assert_eq!(parse_instance("000000000000beef"), Some(0xbeef));
        assert_eq!(parse_instance("beef"), Some(0xbeef));
        assert_eq!(parse_instance("xyz"), None);
        assert_eq!(parse_instance("11112222333344445"), None);
    }
}
