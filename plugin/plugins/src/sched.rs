// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Le placement des messages MIDI dans les blocs (Uppwind Link Notes).
//!
//! Chaque message arrive avec l'instant ou le DAW doit le jouer (horloge `clock::now_ms`, deja convertie par le fil
//! reseau) ; ici il devient un decalage en echantillons dans le bloc ou il tombe (la DLL dit ou commence le bloc et la
//! duree d'un echantillon). Une reserve triee de taille fixe : aucune allocation dans le fil audio. Les notes tenues
//! sont suivies : si le compagnon part, on les relache (sinon un instrument du DAW tiendrait une note sans fin).

pub const CAPACITY: usize = 2048;

/// Une note qui arrive avec plus de retard que ca n'est plus jouee (un DAW qui reprend apres une pause les jouerait
/// toutes d'un coup) ; les note off, CC et pitch bend passent toujours.
pub const STALE_MS: f64 = 500.0;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Pending {
    t: f64,
    n: u32,
    len: u8,
    data: [u8; 3],
}

/// Un message a sortir dans ce bloc.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Out {
    pub offset: u32,
    pub len: u8,
    pub data: [u8; 3],
    pub n: u32,
    /// l'instant demande (horloge du plugin)
    pub t: f64,
    /// le retard (ms) s'il est arrive apres le debut du bloc ou il devait tomber ; 0 sinon
    pub late_ms: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SchedStats {
    pub inserted: u64,
    pub emitted: u64,
    pub late: u64,
    pub late_max_ms: f64,
    pub stale: u64,
    pub overflow: u64,
    pub panics: u64,
}

pub struct Scheduler {
    buf: Box<[Pending]>,
    len: usize,
    /// les notes tenues : un bit par note, par canal
    held: [u128; 16],
    pub stats: SchedStats,
}

impl Default for Scheduler {
    fn default() -> Self {
        Scheduler::new()
    }
}

fn is_note_on(p: &Pending) -> bool {
    p.data[0] & 0xf0 == 0x90 && p.data[2] > 0
}

impl Scheduler {
    /// Alloue la reserve (a la creation du plugin, pas dans le fil audio).
    pub fn new() -> Scheduler {
        Scheduler { buf: vec![Pending::default(); CAPACITY].into_boxed_slice(), len: 0, held: [0; 16], stats: SchedStats::default() }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Ranger un message par instant (a instant egal : l'ordre d'arrivee). Reserve pleine : false, compte.
    pub fn insert(&mut self, t: f64, n: u32, len: u8, data: [u8; 3]) -> bool {
        if self.len == CAPACITY {
            self.stats.overflow += 1;
            return false;
        }
        // les messages arrivent presque toujours dans l'ordre : on cherche depuis la fin
        let mut i = self.len;
        while i > 0 && self.buf[i - 1].t > t {
            i -= 1;
        }
        self.buf.copy_within(i..self.len, i + 1);
        self.buf[i] = Pending { t, n, len, data };
        self.len += 1;
        self.stats.inserted += 1;
        true
    }

    /// Tous les messages qui tombent avant la fin de ce bloc ([t0, t0 + frames × per)), dans l'ordre : `emit` pour
    /// chacun. Un message en retard sort au debut du bloc (decalage 0) et son retard est compte.
    pub fn take_due(&mut self, t0: f64, per: f64, frames: usize, mut emit: impl FnMut(Out)) {
        if frames == 0 || self.len == 0 {
            return;
        }
        let end = t0 + frames as f64 * per;
        let mut k = 0;
        while k < self.len && self.buf[k].t < end {
            let p = self.buf[k];
            k += 1;
            let (offset, late) = if p.t < t0 { (0u32, t0 - p.t) } else { ((((p.t - t0) / per) as usize).min(frames - 1) as u32, 0.0) };
            if late > STALE_MS && is_note_on(&p) {
                self.stats.stale += 1;
                continue;
            }
            if late > 0.0 {
                self.stats.late += 1;
                if late > self.stats.late_max_ms {
                    self.stats.late_max_ms = late;
                }
            }
            self.track(&p);
            self.stats.emitted += 1;
            emit(Out { offset, len: p.len, data: p.data, n: p.n, t: p.t, late_ms: late });
        }
        if k > 0 {
            self.buf.copy_within(k..self.len, 0);
            self.len -= k;
        }
    }

    /// Le compagnon est parti (ou le DAW remet le plugin a zero) : on oublie ce qui attendait et on relache toutes les
    /// notes tenues (des note off au debut du bloc).
    pub fn panic(&mut self, mut emit: impl FnMut(Out)) {
        self.len = 0;
        self.stats.panics += 1;
        for ch in 0..16 {
            let mut bits = self.held[ch];
            while bits != 0 {
                let key = bits.trailing_zeros() as u8;
                bits &= bits - 1;
                emit(Out { offset: 0, len: 3, data: [0x80 | ch as u8, key, 0], n: 0, t: 0.0, late_ms: 0.0 });
            }
            self.held[ch] = 0;
        }
    }

    pub fn held_count(&self) -> u32 {
        self.held.iter().map(|b| b.count_ones()).sum()
    }

    fn track(&mut self, p: &Pending) {
        let ch = (p.data[0] & 0x0f) as usize;
        match p.data[0] & 0xf0 {
            0x90 if p.data[2] > 0 => self.held[ch] |= 1u128 << p.data[1],
            0x90 | 0x80 => self.held[ch] &= !(1u128 << p.data[1]),
            // All Sound Off, All Notes Off
            0xb0 if p.data[1] == 120 || p.data[1] == 123 => self.held[ch] = 0,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PER: f64 = 1000.0 / 48000.0;

    fn collect(s: &mut Scheduler, t0: f64, frames: usize) -> Vec<Out> {
        let mut v = Vec::new();
        s.take_due(t0, PER, frames, |o| v.push(o));
        v
    }

    #[test]
    fn a_time_becomes_the_right_sample() {
        let mut s = Scheduler::new();
        // des notes a 0, 1, 77, 127 echantillons du debut du bloc (et un poil apres : meme echantillon)
        for (i, k) in [0usize, 1, 77, 127].iter().enumerate() {
            assert!(s.insert(1000.0 + *k as f64 * PER + PER * 0.3, i as u32, 3, [0x90, 60 + i as u8, 100]));
        }
        // une note du bloc suivant
        s.insert(1000.0 + 128.0 * PER + 1e-6, 9, 3, [0x80, 60, 0]);
        let got = collect(&mut s, 1000.0, 128);
        assert_eq!(got.iter().map(|o| o.offset).collect::<Vec<_>>(), vec![0, 1, 77, 127]);
        assert!(got.iter().all(|o| o.late_ms == 0.0));
        assert_eq!(s.len(), 1, "the next block's note waits");
        let next = collect(&mut s, 1000.0 + 128.0 * PER, 128);
        assert_eq!((next.len(), next[0].offset, next[0].n), (1, 0, 9));
        assert_eq!(s.stats.emitted, 5);
    }

    #[test]
    fn late_messages_go_first_and_are_counted() {
        let mut s = Scheduler::new();
        s.insert(990.0, 1, 3, [0x90, 60, 100]); // 10 ms en retard
        s.insert(1001.0, 2, 3, [0x80, 60, 0]);
        let got = collect(&mut s, 1000.0, 128);
        assert_eq!(got[0].offset, 0);
        assert!((got[0].late_ms - 10.0).abs() < 1e-9);
        assert_eq!(got[1].offset, 48);
        assert_eq!((s.stats.late, s.stats.late_max_ms), (1, 10.0));
        // une note qui arrive 600 ms trop tard n'est plus jouee ; son note off, si
        s.insert(400.0, 3, 3, [0x91, 62, 90]);
        s.insert(400.5, 4, 3, [0x81, 62, 0]);
        let got = collect(&mut s, 1000.0, 128);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].data, [0x81, 62, 0]);
        assert_eq!(s.stats.stale, 1);
    }

    #[test]
    fn out_of_order_arrivals_are_sorted_and_ties_keep_their_order() {
        let mut s = Scheduler::new();
        s.insert(1002.0, 1, 3, [0x90, 64, 100]); // une note programmee 2 ms plus tard…
        s.insert(1000.5, 2, 3, [0x90, 60, 100]); // … puis une note « tout de suite », arrivee apres
        s.insert(1002.0, 3, 3, [0x80, 64, 0]); // meme instant que la 1 : apres elle
        let got = collect(&mut s, 1000.0, 256);
        assert_eq!(got.iter().map(|o| o.n).collect::<Vec<_>>(), vec![2, 1, 3]);
        assert!(got.windows(2).all(|w| w[0].offset <= w[1].offset), "sorted offsets, as hosts want them");
    }

    #[test]
    fn a_full_reserve_refuses_and_counts() {
        let mut s = Scheduler::new();
        for i in 0..CAPACITY {
            assert!(s.insert(5000.0 + i as f64, i as u32, 3, [0xb0, 1, 0]));
        }
        assert!(!s.insert(1.0, 0, 3, [0xb0, 1, 0]));
        assert_eq!(s.stats.overflow, 1);
    }

    #[test]
    fn panic_releases_every_held_note() {
        let mut s = Scheduler::new();
        s.insert(1000.0, 1, 3, [0x90, 60, 100]);
        s.insert(1000.1, 2, 3, [0x93, 127, 1]);
        s.insert(1000.2, 3, 3, [0x90, 0, 64]);
        s.insert(1000.3, 4, 3, [0x90, 0, 0]); // note on velocite 0 : une note off
        s.insert(2000.0, 5, 3, [0x90, 70, 100]); // pas encore jouee
        collect(&mut s, 1000.0, 128);
        assert_eq!(s.held_count(), 2);
        let mut offs = Vec::new();
        s.panic(|o| offs.push(o.data));
        offs.sort();
        assert_eq!(offs, vec![[0x80, 60, 0], [0x83, 127, 0]]);
        assert_eq!((s.held_count(), s.len()), (0, 0), "nothing held, nothing pending");
        // All Notes Off efface aussi
        s.insert(1.0, 1, 3, [0x95, 40, 1]);
        s.insert(1.0, 2, 3, [0xb5, 123, 0]);
        collect(&mut s, 0.5, 128);
        assert_eq!(s.held_count(), 0);
    }
}
