// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Les horloges du plugin.
//!
//! - `now_ms()` : monotone, en ms depuis une base du processus (`Instant` : QueryPerformanceCounter sous Windows).
//!   Lisible dans le fil audio (une lecture atomique et QPC : pas d'appel bloquant).
//! - `epoch_offset_ms()` : l'ecart entre l'heure de Windows (ms depuis 1970, l'horloge de Date.now() du studio et des
//!   paquets « UWA1 ») et `now_ms()`. Mesure dans le fil reseau seulement : c'est la que les instants passent d'une
//!   horloge a l'autre ; le fil audio ne connait que `now_ms()`.
//! - `BlockClock` : une boucle a verrouillage de delai (DLL : F. Adriaensen, « Using a DLL to filter time », 2005, celle
//!   de JACK) sur l'heure d'appel de process(). Le DAW appelle process() quand sa carte son le demande, avec la gigue
//!   du reveil de son fil (souvent ±0,5-2 ms) ; la DLL en tire l'instant estime du premier echantillon de chaque bloc
//!   et la duree d'un echantillon, lisses. Une note se place alors a l'echantillon pres, sans cette gigue.

use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static BASE: OnceLock<Instant> = OnceLock::new();

/// A appeler a la creation du plugin (pas dans le fil audio : la premiere fois, OnceLock initialise).
pub fn init() {
    BASE.get_or_init(Instant::now);
}

pub fn now_ms() -> f64 {
    BASE.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

pub fn epoch_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

/// epoch - now_ms : la paire de lectures la plus serree sur quelques essais (une preemption entre deux lectures
/// fausserait l'ecart). Hors du fil audio.
pub fn epoch_offset_ms() -> f64 {
    let mut best = (f64::MAX, 0.0);
    for _ in 0..5 {
        let a = now_ms();
        let e = epoch_ms();
        let b = now_ms();
        if b - a < best.0 {
            best = (b - a, e - (a + b) / 2.0);
        }
    }
    best.1
}

/// Ce que la DLL dit d'un bloc.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tick {
    /// l'instant estime (horloge `now_ms`) du premier echantillon du bloc
    pub t0: f64,
    /// la duree estimee d'un echantillon, en ms
    pub per: f64,
    /// la boucle repart de zero (premier bloc, ou le DAW s'est arrete de calculer un moment)
    pub reset: bool,
    /// l'ecart entre l'appel reel et l'appel prevu (ms) : la gigue du fil audio du DAW
    pub err: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ClockStats {
    pub blocks: u64,
    pub resets: u64,
    /// des appels coup sur coup (un DAW qui coupe son bloc en morceaux) : pas une observation de l'horloge
    pub skipped: u64,
    pub err_n: u64,
    pub err_sum: f64,
    pub err_sum2: f64,
    pub err_max: f64,
}

impl ClockStats {
    pub fn jitter_sd(&self) -> f64 {
        if self.err_n < 2 {
            return 0.0;
        }
        let m = self.err_sum / self.err_n as f64;
        (self.err_sum2 / self.err_n as f64 - m * m).max(0.0).sqrt()
    }
}

/// La bande passante de la boucle : 0,5 Hz (deux secondes pour converger ; la gigue du reveil est divisee par ~10 a
/// 128 echantillons). Avant convergence, l'erreur n'est jamais pire que l'heure d'appel brute.
pub const BANDWIDTH_HZ: f64 = 0.5;

pub struct BlockClock {
    nominal: f64,
    per: f64,
    /// l'instant prevu du premier echantillon du prochain bloc
    tp: f64,
    last_now: f64,
    last_n: usize,
    started: bool,
    pub stats: ClockStats,
}

impl BlockClock {
    pub fn new(rate: f64) -> BlockClock {
        let nominal = 1000.0 / rate.max(1.0);
        BlockClock { nominal, per: nominal, tp: 0.0, last_now: 0.0, last_n: 0, started: false, stats: ClockStats::default() }
    }

    pub fn set_rate(&mut self, rate: f64) {
        *self = BlockClock::new(rate);
    }

    /// Repartir de zero au prochain bloc (le DAW a remis le plugin a zero).
    pub fn reset(&mut self) {
        self.started = false;
    }

    pub fn nominal_per(&self) -> f64 {
        self.nominal
    }

    fn restart(&mut self, now: f64, n: usize, err: f64) -> Tick {
        self.started = true;
        self.per = self.nominal;
        self.tp = now + n as f64 * self.per;
        self.last_now = now;
        self.last_n = n;
        self.stats.resets += 1;
        Tick { t0: now, per: self.per, reset: true, err }
    }

    /// Au debut de process() : `now` = now_ms(), `n` = la taille du bloc. Aucune allocation, aucun appel.
    pub fn tick(&mut self, now: f64, n: usize) -> Tick {
        let n = n.max(1);
        self.stats.blocks += 1;
        if !self.started {
            return self.restart(now, n, 0.0);
        }
        let expected = self.last_n as f64 * self.per; // la duree du bloc d'avant
        let since = now - self.last_now;
        let e = now - self.tp;
        // 1. un saut : le DAW ne calculait plus (transport arrete chez certains, plugin desactive, une alerte modale…)
        //    ou il calcule plus vite que le temps reel (export) : on repart
        if e.abs() > (4.0 * expected).max(20.0) {
            return self.restart(now, n, e);
        }
        // 2. appele coup sur coup apres le bloc d'avant (un DAW qui coupe le tampon de la carte en morceaux) : cet
        //    appel ne dit rien de l'horloge, le bloc suit simplement le precedent
        if since < 0.25 * expected {
            self.stats.skipped += 1;
            let t0 = self.tp;
            self.tp = t0 + n as f64 * self.per;
            self.last_now = now;
            self.last_n = n;
            return Tick { t0, per: self.per, reset: false, err: 0.0 };
        }
        // 3. la DLL du second ordre : t0 = la prevision, corrigee pour le bloc suivant ; la periode suit la derive
        //    (l'horloge de la carte son contre QPC : quelques dizaines de ppm)
        let w = (2.0 * std::f64::consts::PI * BANDWIDTH_HZ * (n as f64 * self.per / 1000.0)).min(0.5);
        let b = std::f64::consts::SQRT_2 * w;
        let c = w * w;
        let t0 = self.tp;
        self.tp = t0 + b * e + n as f64 * self.per;
        self.per += c * e / n as f64;
        // garde-fou : jamais plus de 1 % d'ecart avec la frequence annoncee
        self.per = self.per.clamp(self.nominal * 0.99, self.nominal * 1.01);
        self.last_now = now;
        self.last_n = n;
        self.stats.err_n += 1;
        self.stats.err_sum += e;
        self.stats.err_sum2 += e * e;
        if e.abs() > self.stats.err_max {
            self.stats.err_max = e.abs();
        }
        Tick { t0, per: self.per, reset: false, err: e }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un generateur pseudo-aleatoire fixe (les tests rejouent les memes nombres).
    struct Lcg(u64);
    impl Lcg {
        /// dans [-1, 1)
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
        }
    }

    #[test]
    fn the_dll_removes_the_callback_jitter() {
        // 48 kHz, blocs de 128 (2,667 ms) ; la carte son appelle a l'heure ideale + une gigue de ±1 ms (uniforme)
        let (rate, n) = (48000.0, 128usize);
        let period = n as f64 * 1000.0 / rate;
        let mut clk = BlockClock::new(rate);
        let mut rng = Lcg(7);
        let mut errs = Vec::new();
        let mut raw = Vec::new();
        for k in 0..6000 {
            let ideal = 100.0 + k as f64 * period;
            let jitter = rng.next() * 1.0 + 0.3; // et un retard moyen de 0,3 ms (la phase : constante, sans importance)
            let t = clk.tick(ideal + jitter, n);
            // apres 4 s (la convergence)
            if k > 1500 {
                errs.push(t.t0 - ideal);
                raw.push(jitter);
            }
        }
        let sd = |v: &[f64]| {
            let m = v.iter().sum::<f64>() / v.len() as f64;
            (v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / v.len() as f64).sqrt()
        };
        let (s_dll, s_raw) = (sd(&errs), sd(&raw));
        eprintln!("DLL at {BANDWIDTH_HZ} Hz, 128 frames @ 48 kHz, callback jitter ±1 ms: raw sd {s_raw:.3} ms -> filtered sd {s_dll:.4} ms");
        assert!(s_raw > 0.5, "{s_raw}");
        assert!(s_dll < 0.15 * s_raw, "the filtered block time jitters {s_dll:.4} ms (raw {s_raw:.3})");
        assert!((clk.per - 1000.0 / rate).abs() / (1000.0 / rate) < 1e-4);
        assert_eq!(clk.stats.resets, 1);
        assert!(clk.stats.jitter_sd() > 0.4);
    }

    #[test]
    fn the_dll_follows_a_sound_card_that_runs_fast() {
        // la carte a 48 kHz + 100 ppm (sans suivi, la phase deriverait de 3,2 ms en 32 s) : la boucle suit la frequence,
        // la phase reste en place ; la periode instantanee est bruitee (c'est voulu), sa moyenne est juste
        let (rate, n) = (48000.0, 256usize);
        let true_per = 1000.0 / (rate * (1.0 + 100e-6));
        let mut clk = BlockClock::new(rate);
        let mut rng = Lcg(3);
        let (mut pers, mut phase) = (Vec::new(), Vec::new());
        for k in 0..6000 {
            let ideal = k as f64 * n as f64 * true_per;
            let t = clk.tick(ideal + rng.next() * 0.5, n);
            if k >= 3000 {
                pers.push(t.per);
                phase.push(t.t0 - ideal);
            }
        }
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let ppm = (mean(&pers) - true_per) / true_per * 1e6;
        let (first, last) = (mean(&phase[..500]), mean(&phase[phase.len() - 500..]));
        eprintln!("DLL against a +100 ppm sound card: mean period {ppm:.2} ppm off, phase {first:.4} ms at 16 s, {last:.4} ms at 32 s");
        assert!(ppm.abs() < 10.0, "{ppm:.2} ppm off");
        assert!(first.abs() < 0.1 && last.abs() < 0.1 && (last - first).abs() < 0.05, "the phase drifts: {first} -> {last}");
    }

    #[test]
    fn sub_blocks_follow_their_anchor_and_gaps_restart() {
        // un DAW qui coupe chaque tampon de 512 en 384 + 128, appeles coup sur coup
        let rate = 48000.0;
        let per = 1000.0 / rate;
        let mut clk = BlockClock::new(rate);
        let mut last = None;
        for k in 0..400 {
            let t = k as f64 * 512.0 * per;
            let a = clk.tick(t, 384);
            let b = clk.tick(t + 0.02, 128);
            assert!(!b.reset);
            if k > 0 {
                assert!((b.t0 - (a.t0 + 384.0 * a.per)).abs() < 1e-9, "the sub-block follows the anchor");
            }
            last = Some(b);
        }
        assert_eq!(clk.stats.skipped, 400);
        assert_eq!(clk.stats.resets, 1);
        // une seconde sans appel (le DAW a arrete de calculer) : on repart, et on le dit
        let t = 400.0 * 512.0 * per + 1000.0;
        let r = clk.tick(t, 384);
        assert!(r.reset && r.t0 == t);
        assert!(last.unwrap().t0 < t);
        assert_eq!(clk.stats.resets, 2);
    }
}
