// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Le son du DAW en paquets (24 oct.) : ce que le fil de capture recoit de Windows (des morceaux de taille variable,
//! ~10 ms) devient des paquets de taille fixe (5 ms par defaut) pour le studio, chacun avec son numero, la position de
//! sa premiere trame et l'instant de sa capture. Rien ici ne touche a Windows : tout est teste.
//!
//! Un paquet (message binaire du WebSocket) — un en-tete fixe de 36 octets, petit-boutiste, puis les echantillons
//! entrelaces (gauche, droite, gauche…) :
//!
//!   0   4  « UWA1 »
//!   4   4  seq     u32   le numero du paquet (1, 2, 3… ; un trou = un paquet perdu)
//!   8   8  pos     u64   la trame (depuis le debut du flux) du premier echantillon : un saut = de la capture perdue
//!   16  8  t       f64   l'instant de capture du premier echantillon, ms depuis 1970 (l'horloge de Date.now())
//!   24  4  rate    u32   la frequence (48000)
//!   28  2  frames  u16   le nombre de trames du paquet
//!   30  1  ch      u8    les canaux (2)
//!   31  1  fmt     u8    1 : float32, 2 : int16
//!   32  1  flags   u8    1 : silence (Windows le dit), 2 : discontinuite (la capture a saute avant ce paquet)
//!   33  3  (zero)
//!   36  …  frames × ch echantillons (f32 : 4 octets, i16 : 2 octets)
//!
//! 36 est un multiple de 4 : le studio lit les echantillons avec un Float32Array pose directement sur le message.

pub const MAGIC: &[u8; 4] = b"UWA1";
pub const HEADER: usize = 36;
pub const FMT_F32: u8 = 1;
pub const FMT_I16: u8 = 2;
pub const FLAG_SILENT: u8 = 1;
pub const FLAG_DISCONTINUITY: u8 = 2;
/// 5 ms a 48 kHz.
pub const DEFAULT_PACKET_MS: f64 = 5.0;
pub const MAX_PACKET_FRAMES: usize = 4800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    F32,
    I16,
}

impl SampleFormat {
    pub fn parse(s: &str) -> Option<SampleFormat> {
        match s {
            "f32" | "float32" => Some(SampleFormat::F32),
            "i16" | "s16" | "int16" => Some(SampleFormat::I16),
            _ => None,
        }
    }
    pub fn code(self) -> u8 {
        match self {
            SampleFormat::F32 => FMT_F32,
            SampleFormat::I16 => FMT_I16,
        }
    }
    pub fn bytes(self) -> usize {
        match self {
            SampleFormat::F32 => 4,
            SampleFormat::I16 => 2,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            SampleFormat::F32 => "f32",
            SampleFormat::I16 => "i16",
        }
    }
}

/// Les trames par paquet pour une duree donnee (au moins 16, au plus MAX_PACKET_FRAMES).
pub fn frames_for(rate: u32, ms: f64) -> usize {
    let ms = if ms.is_finite() { ms.clamp(1.0, 50.0) } else { DEFAULT_PACKET_MS };
    ((rate as f64 * ms / 1000.0).round() as usize).clamp(16, MAX_PACKET_FRAMES)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    pub seq: u32,
    pub pos: u64,
    pub t_ms: f64,
    pub rate: u32,
    pub frames: u16,
    pub channels: u8,
    pub fmt: u8,
    pub flags: u8,
}

pub fn encode(h: &Header, samples: &[f32], fmt: SampleFormat) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + samples.len() * fmt.bytes());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&h.seq.to_le_bytes());
    out.extend_from_slice(&h.pos.to_le_bytes());
    out.extend_from_slice(&h.t_ms.to_le_bytes());
    out.extend_from_slice(&h.rate.to_le_bytes());
    out.extend_from_slice(&h.frames.to_le_bytes());
    out.push(h.channels);
    out.push(fmt.code());
    out.push(h.flags);
    out.extend_from_slice(&[0, 0, 0]);
    match fmt {
        SampleFormat::F32 => {
            for s in samples {
                out.extend_from_slice(&s.to_le_bytes());
            }
        }
        SampleFormat::I16 => {
            for s in samples {
                out.extend_from_slice(&to_i16(*s).to_le_bytes());
            }
        }
    }
    out
}

pub fn to_i16(s: f32) -> i16 {
    let v = if s.is_finite() { s.clamp(-1.0, 1.0) } else { 0.0 };
    (v * 32767.0).round() as i16
}

/// Lire un paquet (les tests ; le studio fait la meme chose en JS) → l'en-tete et les echantillons en f32.
#[allow(dead_code)]
pub fn decode(p: &[u8]) -> Option<(Header, Vec<f32>)> {
    if p.len() < HEADER || &p[..4] != MAGIC {
        return None;
    }
    let u32_at = |i: usize| u32::from_le_bytes([p[i], p[i + 1], p[i + 2], p[i + 3]]);
    let mut b8 = [0u8; 8];
    b8.copy_from_slice(&p[8..16]);
    let pos = u64::from_le_bytes(b8);
    b8.copy_from_slice(&p[16..24]);
    let t_ms = f64::from_le_bytes(b8);
    let h = Header { seq: u32_at(4), pos, t_ms, rate: u32_at(24), frames: u16::from_le_bytes([p[28], p[29]]), channels: p[30], fmt: p[31], flags: p[32] };
    let n = h.frames as usize * h.channels as usize;
    let body = &p[HEADER..];
    let samples = match h.fmt {
        FMT_F32 if body.len() == n * 4 => body.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(),
        FMT_I16 if body.len() == n * 2 => body.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32767.0).collect(),
        _ => return None,
    };
    Some((h, samples))
}

/// Un morceau tel que Windows le rend (IAudioCaptureClient::GetBuffer) : des echantillons entrelaces (vide si
/// `silent`), la position de sa premiere trame (en trames du flux), l'instant de capture de sa premiere trame.
pub struct Chunk<'a> {
    pub samples: &'a [f32],
    pub frames: usize,
    pub pos: u64,
    pub t_ms: f64,
    pub silent: bool,
    pub discontinuity: bool,
}

/// Le decoupage en paquets de taille fixe. Un trou dans les positions (ou une discontinuite dite par Windows) : le
/// paquet en cours part tel quel (plus court), le suivant porte FLAG_DISCONTINUITY.
pub struct Packetizer {
    pub rate: u32,
    pub channels: usize,
    pub frames_per_packet: usize,
    pub fmt: SampleFormat,
    seq: u32,
    buf: Vec<f32>,
    /// position et instant de la premiere trame du paquet en cours
    start_pos: u64,
    start_t: f64,
    /// la position attendue du prochain morceau (None : aucun encore)
    next_pos: Option<u64>,
    silent_all: bool,
    pending_flags: u8,
    pub packets: u64,
    pub gaps: u64,
    pub lost_frames: u64,
}

impl Packetizer {
    pub fn new(rate: u32, channels: usize, frames_per_packet: usize, fmt: SampleFormat) -> Packetizer {
        let fpp = frames_per_packet.clamp(1, MAX_PACKET_FRAMES);
        Packetizer {
            rate,
            channels: channels.max(1),
            frames_per_packet: fpp,
            fmt,
            seq: 0,
            buf: Vec::with_capacity(fpp * channels.max(1)),
            start_pos: 0,
            start_t: 0.0,
            next_pos: None,
            silent_all: true,
            pending_flags: 0,
            packets: 0,
            gaps: 0,
            lost_frames: 0,
        }
    }

    fn fill(&self) -> usize {
        self.buf.len() / self.channels
    }

    fn emit(&mut self, out: &mut Vec<Vec<u8>>) {
        let frames = self.fill();
        if frames == 0 {
            return;
        }
        self.seq = self.seq.wrapping_add(1);
        let mut flags = self.pending_flags;
        if self.silent_all {
            flags |= FLAG_SILENT;
        }
        let h = Header { seq: self.seq, pos: self.start_pos, t_ms: self.start_t, rate: self.rate, frames: frames as u16, channels: self.channels as u8, fmt: self.fmt.code(), flags };
        out.push(encode(&h, &self.buf, self.fmt));
        self.packets += 1;
        self.pending_flags = 0;
        self.silent_all = true;
        self.buf.clear();
        self.start_pos = self.start_pos.wrapping_add(frames as u64);
        self.start_t += frames as f64 * 1000.0 / self.rate as f64;
    }

    /// Un morceau de Windows → zero, un ou plusieurs paquets complets.
    pub fn push(&mut self, c: &Chunk, out: &mut Vec<Vec<u8>>) {
        if c.frames == 0 {
            return;
        }
        let gap = match self.next_pos {
            Some(p) => c.pos != p,
            None => false,
        };
        if gap || c.discontinuity {
            if let Some(p) = self.next_pos {
                if c.pos > p {
                    self.lost_frames = self.lost_frames.saturating_add(c.pos - p);
                }
            }
            self.gaps += 1;
            self.emit(out); // le paquet commence avant le trou : il part, plus court
            self.pending_flags |= FLAG_DISCONTINUITY;
        }
        if self.fill() == 0 {
            self.start_pos = c.pos;
            self.start_t = c.t_ms;
        }
        let ms_per_frame = 1000.0 / self.rate as f64;
        let mut i = 0usize;
        while i < c.frames {
            if self.fill() == 0 {
                // (26 oct.) wrapping : `pos` vient d'un plugin (plugin_link), un u64 au bord ne doit rien faire paniquer
                self.start_pos = c.pos.wrapping_add(i as u64);
                self.start_t = c.t_ms + i as f64 * ms_per_frame;
            }
            let room = self.frames_per_packet - self.fill();
            let take = room.min(c.frames - i);
            if c.silent || c.samples.len() < (i + take) * self.channels {
                self.buf.extend(std::iter::repeat_n(0.0f32, take * self.channels));
            } else {
                self.buf.extend_from_slice(&c.samples[i * self.channels..(i + take) * self.channels]);
                if self.silent_all && c.samples[i * self.channels..(i + take) * self.channels].iter().any(|s| *s != 0.0) {
                    self.silent_all = false;
                }
            }
            i += take;
            if self.fill() == self.frames_per_packet {
                self.emit(out);
            }
        }
        self.next_pos = Some(c.pos.wrapping_add(c.frames as u64));
    }

    /// Le reste (fin du flux).
    #[allow(dead_code)]
    pub fn flush(&mut self, out: &mut Vec<Vec<u8>>) {
        self.emit(out);
    }
}

/// La cadence des morceaux de Windows (ou des paquets) : intervalles moyen, mini, maxi, ecart type, et les retards
/// au-dela d'un seuil. « La gigue » du rapport --capture-test.
#[derive(Debug, Clone, Default)]
pub struct Cadence {
    last: Option<f64>,
    pub n: u64,
    sum: f64,
    sum2: f64,
    pub min: f64,
    pub max: f64,
    /// intervalles plus longs que `late_ms`
    pub late: u64,
    pub late_ms: f64,
}

impl Cadence {
    pub fn new(late_ms: f64) -> Cadence {
        Cadence { late_ms, min: f64::INFINITY, ..Default::default() }
    }
    /// Un evenement a l'instant t (ms).
    pub fn tick(&mut self, t: f64) {
        if let Some(l) = self.last {
            let d = t - l;
            self.n += 1;
            self.sum += d;
            self.sum2 += d * d;
            self.min = self.min.min(d);
            self.max = self.max.max(d);
            if d > self.late_ms {
                self.late += 1;
            }
        }
        self.last = Some(t);
    }
    pub fn mean(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.sum / self.n as f64
        }
    }
    pub fn stddev(&self) -> f64 {
        if self.n < 2 {
            return 0.0;
        }
        let m = self.mean();
        ((self.sum2 / self.n as f64) - m * m).max(0.0).sqrt()
    }
}

/// Les niveaux par canal : RMS et crete, en dB plein echelle.
#[derive(Debug, Clone)]
pub struct Levels {
    pub channels: usize,
    sum2: Vec<f64>,
    peak: Vec<f32>,
    pub frames: u64,
}

impl Levels {
    pub fn new(channels: usize) -> Levels {
        let c = channels.max(1);
        Levels { channels: c, sum2: vec![0.0; c], peak: vec![0.0; c], frames: 0 }
    }
    pub fn add(&mut self, interleaved: &[f32]) {
        for f in interleaved.chunks_exact(self.channels) {
            for (c, s) in f.iter().enumerate() {
                self.sum2[c] += (*s as f64) * (*s as f64);
                let a = s.abs();
                if a > self.peak[c] {
                    self.peak[c] = a;
                }
            }
            self.frames += 1;
        }
    }
    /// Des trames de silence (Windows dit « silent » sans donner d'echantillons).
    pub fn add_silence(&mut self, frames: usize) {
        self.frames += frames as u64;
    }
    pub fn rms(&self, c: usize) -> f64 {
        if self.frames == 0 {
            0.0
        } else {
            (self.sum2[c] / self.frames as f64).sqrt()
        }
    }
    pub fn peak(&self, c: usize) -> f64 {
        self.peak[c] as f64
    }
    pub fn reset(&mut self) {
        for v in &mut self.sum2 {
            *v = 0.0;
        }
        for v in &mut self.peak {
            *v = 0.0;
        }
        self.frames = 0;
    }
}

/// Une amplitude → dB plein echelle, au dixieme (None : le silence numerique).
pub fn db(v: f64) -> Option<f64> {
    if v > 1e-9 {
        Some((20.0 * v.log10() * 10.0).round() / 10.0)
    } else {
        None
    }
}

/// dB → amplitude.
pub fn amp(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// Un sinus de test (le mode demo du compagnon, la fenetre d'essai) : `frames` trames stereo a partir de la phase
/// `phase` (en trames), frequence `hz`, amplitude `a`.
pub fn sine(frames: usize, channels: usize, rate: u32, hz: f64, a: f32, phase: u64) -> Vec<f32> {
    let mut v = Vec::with_capacity(frames * channels);
    let w = 2.0 * std::f64::consts::PI * hz / rate as f64;
    for i in 0..frames {
        let s = (a as f64 * ((phase + i as u64) as f64 * w).sin()) as f32;
        for _ in 0..channels {
            v.push(s);
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(samples: &[f32], frames: usize, pos: u64, t: f64) -> Chunk<'_> {
        Chunk { samples, frames, pos, t_ms: t, silent: false, discontinuity: false }
    }

    #[test]
    fn a_position_at_the_end_of_u64_never_panics() {
        // (26 oct.) la position d'un plugin (plugin_link) n'est pas verifiee : au bord, elle fait le tour
        let mut p = Packetizer::new(48000, 2, 240, SampleFormat::F32);
        let mut out = Vec::new();
        let s = vec![0.1f32; 2 * 300];
        p.push(&chunk(&s, 300, u64::MAX - 10, 0.0), &mut out);
        p.push(&chunk(&s, 300, (u64::MAX - 10).wrapping_add(300), 6.25), &mut out);
        assert!(!out.is_empty());
    }

    #[test]
    fn frames_per_packet() {
        assert_eq!(frames_for(48000, 5.0), 240);
        assert_eq!(frames_for(44100, 5.0), 221);
        assert_eq!(frames_for(48000, 0.01), 48); // au moins 1 ms
        assert_eq!(frames_for(48000, f64::NAN), 240);
        assert_eq!(frames_for(48000, 1000.0), 2400); // au plus 50 ms
    }

    #[test]
    fn header_round_trip_f32_and_i16() {
        let h = Header { seq: 7, pos: 1_000_000_123, t_ms: 1_761_300_000_123.25, rate: 48000, frames: 3, channels: 2, fmt: FMT_F32, flags: FLAG_DISCONTINUITY };
        let s = [0.5f32, -0.5, 0.25, -0.25, 1.0, -1.0];
        let p = encode(&h, &s, SampleFormat::F32);
        assert_eq!(p.len(), HEADER + 6 * 4);
        assert_eq!(&p[..4], b"UWA1");
        assert_eq!(HEADER % 4, 0, "the studio puts a Float32Array right on the message");
        let (h2, s2) = decode(&p).unwrap();
        assert_eq!(h2, h);
        assert_eq!(s2, s.to_vec());
        let p16 = encode(&Header { fmt: FMT_I16, ..h.clone() }, &s, SampleFormat::I16);
        assert_eq!(p16.len(), HEADER + 6 * 2);
        let (h3, s3) = decode(&p16).unwrap();
        assert_eq!(h3.fmt, FMT_I16);
        for (a, b) in s3.iter().zip(s.iter()) {
            assert!((a - b).abs() < 1.0 / 16000.0, "{a} vs {b}");
        }
        assert!(decode(b"UWA0").is_none());
        let mut bad = p.clone();
        bad.pop();
        assert!(decode(&bad).is_none(), "a truncated packet is refused");
        assert_eq!(to_i16(2.0), 32767);
        assert_eq!(to_i16(f32::NAN), 0);
    }

    #[test]
    fn windows_chunks_become_fixed_packets_with_exact_times() {
        // des morceaux de 480 trames (10 ms) → des paquets de 240 trames (5 ms)
        let mut p = Packetizer::new(48000, 2, 240, SampleFormat::F32);
        let mut out = Vec::new();
        let s = sine(480, 2, 48000, 440.0, 0.1, 0);
        p.push(&chunk(&s, 480, 0, 1000.0), &mut out);
        assert_eq!(out.len(), 2);
        let s2 = sine(480, 2, 48000, 440.0, 0.1, 480);
        p.push(&chunk(&s2, 480, 480, 1010.0), &mut out);
        assert_eq!(out.len(), 4);
        let heads: Vec<Header> = out.iter().map(|b| decode(b).unwrap().0).collect();
        assert_eq!(heads.iter().map(|h| h.seq).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
        assert_eq!(heads.iter().map(|h| h.pos).collect::<Vec<_>>(), vec![0, 240, 480, 720]);
        let ts: Vec<f64> = heads.iter().map(|h| h.t_ms).collect();
        assert!((ts[1] - 1005.0).abs() < 1e-9 && (ts[3] - 1015.0).abs() < 1e-9, "{ts:?}");
        assert!(heads.iter().all(|h| h.frames == 240 && h.flags == 0));
        // les echantillons arrivent tels quels, dans l'ordre
        let all: Vec<f32> = out.iter().flat_map(|b| decode(b).unwrap().1).collect();
        let mut want = s.clone();
        want.extend_from_slice(&s2);
        assert_eq!(all, want);
    }

    #[test]
    fn odd_chunks_straddle_packets() {
        let mut p = Packetizer::new(48000, 2, 240, SampleFormat::F32);
        let mut out = Vec::new();
        let mut pos = 0u64;
        for n in [100usize, 300, 7, 333, 220] {
            let s = vec![0.25f32; n * 2];
            p.push(&chunk(&s, n, pos, pos as f64 / 48.0), &mut out);
            pos += n as u64;
        }
        // 960 trames → 4 paquets pleins, 0 en attente
        assert_eq!(out.len(), 4);
        for (i, b) in out.iter().enumerate() {
            let (h, _) = decode(b).unwrap();
            assert_eq!(h.pos, i as u64 * 240);
            assert!((h.t_ms - h.pos as f64 / 48.0).abs() < 1e-9);
        }
        p.flush(&mut out);
        assert_eq!(out.len(), 4, "nothing left");
    }

    #[test]
    fn a_hole_in_the_capture_is_flagged_and_counted() {
        let mut p = Packetizer::new(48000, 2, 240, SampleFormat::F32);
        let mut out = Vec::new();
        let s = vec![0.1f32; 480 * 2];
        p.push(&chunk(&s, 100, 0, 0.0), &mut out);
        assert!(out.is_empty());
        // 380 trames perdues : le debut part tel quel (100 trames), la suite porte « discontinuite »
        p.push(&chunk(&s, 480, 480, 10.0), &mut out);
        let heads: Vec<Header> = out.iter().map(|b| decode(b).unwrap().0).collect();
        assert_eq!(heads[0].frames, 100);
        assert_eq!(heads[0].flags & FLAG_DISCONTINUITY, 0);
        assert_eq!(heads[1].pos, 480);
        assert_eq!(heads[1].flags & FLAG_DISCONTINUITY, FLAG_DISCONTINUITY);
        assert!((heads[1].t_ms - 10.0).abs() < 1e-9);
        assert_eq!(heads[2].flags & FLAG_DISCONTINUITY, 0, "only the first packet after the hole");
        assert_eq!((p.gaps, p.lost_frames), (1, 380));
        // Windows qui dit lui-meme « discontinuite » (sans trou de position)
        let mut p = Packetizer::new(48000, 2, 240, SampleFormat::F32);
        let mut out = Vec::new();
        p.push(&chunk(&s, 240, 0, 0.0), &mut out);
        p.push(&Chunk { samples: &s, frames: 240, pos: 240, t_ms: 5.0, silent: false, discontinuity: true }, &mut out);
        assert_eq!(decode(&out[1]).unwrap().0.flags, FLAG_DISCONTINUITY);
    }

    #[test]
    fn silence_from_windows_is_zeros_and_flagged() {
        let mut p = Packetizer::new(48000, 2, 240, SampleFormat::I16);
        let mut out = Vec::new();
        p.push(&Chunk { samples: &[], frames: 480, pos: 0, t_ms: 0.0, silent: true, discontinuity: false }, &mut out);
        assert_eq!(out.len(), 2);
        let (h, s) = decode(&out[0]).unwrap();
        assert_eq!(h.flags, FLAG_SILENT);
        assert_eq!(h.fmt, FMT_I16);
        assert!(s.iter().all(|v| *v == 0.0));
        let tone = sine(240, 2, 48000, 440.0, 0.5, 0);
        p.push(&chunk(&tone, 240, 480, 10.0), &mut out);
        assert_eq!(decode(&out[2]).unwrap().0.flags & FLAG_SILENT, 0);
    }

    #[test]
    fn cadence_and_levels() {
        let mut c = Cadence::new(15.0);
        for t in [0.0, 10.0, 20.0, 30.5, 49.5, 59.5] {
            c.tick(t);
        }
        assert_eq!(c.n, 5);
        assert!((c.mean() - 11.9).abs() < 1e-9);
        assert_eq!((c.min, c.max, c.late), (10.0, 19.0, 1));
        assert!(c.stddev() > 3.0 && c.stddev() < 4.0, "{}", c.stddev());
        let mut l = Levels::new(2);
        // un sinus a -30 dBFS : crete 0.0316, RMS crete/√2 (−33 dB)
        l.add(&sine(48000, 2, 48000, 440.0, amp(-30.0) as f32, 0));
        assert_eq!(db(l.peak(0)), Some(-30.0));
        assert_eq!(db(l.rms(1)), Some(-33.0));
        l.add_silence(48000);
        assert_eq!(db(l.rms(0)), Some(-36.0), "half the time silent: -3 dB of power");
        l.reset();
        assert_eq!(db(l.rms(0)), None);
    }
}
