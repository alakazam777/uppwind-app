// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (25 oct., phase 0) Un convertisseur de frequence lineaire, stereo, en continu : quand le DAW tourne a 44,1 kHz et le
//! studio a 48 kHz. Suffisant pour l'essai ; la phase 1 prendra un filtre polyphase (le lineaire attenue les aigus et
//! replie un peu). Rien ici ne touche au systeme : tout est teste.

pub struct Linear {
    /// trames d'entree par trame de sortie
    ratio: f64,
    /// la position de la prochaine trame de sortie, en trames d'entree, relative au debut du prochain morceau
    /// (-1 = la derniere trame du morceau d'avant)
    pos: f64,
    prev: [f32; 2],
    have_prev: bool,
}

impl Linear {
    pub fn new(in_rate: u32, out_rate: u32) -> Linear {
        Linear { ratio: in_rate.max(1) as f64 / out_rate.max(1) as f64, pos: 0.0, prev: [0.0; 2], have_prev: false }
    }

    pub fn reset(&mut self) {
        self.pos = 0.0;
        self.have_prev = false;
    }

    /// Un morceau stereo entrelace → les trames de sortie ajoutees a `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let n = input.len() / 2;
        if n == 0 {
            return;
        }
        let frame = |i: isize, prev: &[f32; 2]| -> [f32; 2] {
            if i < 0 {
                *prev
            } else {
                let i = i as usize;
                [input[2 * i], input[2 * i + 1]]
            }
        };
        if !self.have_prev && self.pos < 0.0 {
            self.pos = 0.0;
        }
        let last = (n - 1) as f64;
        while self.pos <= last {
            let i = self.pos.floor();
            let f = (self.pos - i) as f32;
            let a = frame(i as isize, &self.prev);
            let b = if (i as isize) + 1 <= n as isize - 1 { frame(i as isize + 1, &self.prev) } else { a };
            out.push(a[0] + (b[0] - a[0]) * f);
            out.push(a[1] + (b[1] - a[1]) * f);
            self.pos += self.ratio;
        }
        self.pos -= n as f64;
        self.prev = frame(n as isize - 1, &self.prev);
        self.have_prev = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frames: usize, rate: f64, hz: f64, phase: usize) -> Vec<f32> {
        (0..frames).flat_map(|i| {
            let v = (2.0 * std::f64::consts::PI * hz * (phase + i) as f64 / rate).sin() as f32;
            [v, -v]
        }).collect()
    }

    #[test]
    fn same_rate_is_identity() {
        let mut r = Linear::new(48000, 48000);
        let s = sine(300, 48000.0, 440.0, 0);
        let mut out = Vec::new();
        r.process(&s[..200], &mut out);
        r.process(&s[200..], &mut out);
        assert_eq!(out, s);
    }

    #[test]
    fn rate_44100_to_48000_keeps_length_and_shape_across_chunks() {
        // 1 s a 44,1 kHz en morceaux de 128 → ~48000 trames, un sinus de 440 Hz qui reste un sinus de 440 Hz
        let mut r = Linear::new(44100, 48000);
        let s = sine(44100, 44100.0, 440.0, 0);
        let mut out = Vec::new();
        for c in s.chunks(256) {
            r.process(c, &mut out);
        }
        let frames = out.len() / 2;
        assert!((frames as i64 - 48000).abs() <= 2, "{frames}");
        let want = sine(frames, 48000.0, 440.0, 0);
        let err = out.iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(err < 0.01, "max error {err}");
        assert!(out.chunks(2).all(|f| (f[0] + f[1]).abs() < 1e-6), "channels stay apart");
    }
}
