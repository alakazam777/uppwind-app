// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Les images : reduire si trop grandes, encoder en JPEG (4:4:4 : le texte fin des plugins reste net).

use crate::capture::Frame;

/// Au-dela, l'image est reduite de moitie (puis encore) : 2560×1600 = un grand plugin sur un ecran 4K.
pub const MAX_PIXELS: u64 = 2560 * 1600;
pub const DEFAULT_QUALITY: u8 = 80;

/// Reduire de moitie (moyenne de 2×2 pixels), autant de fois que necessaire pour passer sous `max_pixels`.
pub fn cap_size(f: Frame, max_pixels: u64) -> Frame {
    let mut f = f;
    while (f.w as u64) * (f.h as u64) > max_pixels && f.w >= 2 && f.h >= 2 {
        f = halve(&f);
    }
    f
}

pub fn halve(f: &Frame) -> Frame {
    let (w, h) = (f.w / 2, f.h / 2);
    let sw = f.w as usize;
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h as usize {
        for x in 0..w as usize {
            for c in 0..4 {
                let p = |yy: usize, xx: usize| f.bgra[(yy * sw + xx) * 4 + c] as u32;
                let s = p(2 * y, 2 * x) + p(2 * y, 2 * x + 1) + p(2 * y + 1, 2 * x) + p(2 * y + 1, 2 * x + 1);
                out[(y * w as usize + x) * 4 + c] = ((s + 2) / 4) as u8;
            }
        }
    }
    Frame { w, h, bgra: out }
}

pub fn jpeg(f: &Frame, quality: u8) -> Result<Vec<u8>, String> {
    if f.w == 0 || f.h == 0 || f.w > 65535 || f.h > 65535 {
        return Err("bad image size".into());
    }
    let mut out = Vec::with_capacity((f.w * f.h / 4) as usize);
    let mut enc = jpeg_encoder::Encoder::new(&mut out, quality.clamp(30, 95));
    enc.set_sampling_factor(jpeg_encoder::SamplingFactor::R_4_4_4);
    enc.encode(&f.bgra, f.w as u16, f.h as u16, jpeg_encoder::ColorType::Bgra).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Une image synthetique (le mode --demo et les tests) : un fond, une barre qui avance avec `n`.
pub fn synthetic(w: u32, h: u32, n: u64) -> Frame {
    let mut bgra = vec![0u8; (w * h * 4) as usize];
    let bar = (n * 7 % w.max(1) as u64) as u32;
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let on = x >= bar && x < bar + 40 && y > h / 3 && y < h / 3 + 40;
            bgra[i] = if on { 0 } else { 0x33 };
            bgra[i + 1] = if on { 0x88 } else { 0x22 };
            bgra[i + 2] = if on { 0xff } else { 0x11 };
            bgra[i + 3] = 255;
        }
    }
    Frame { w, h, bgra }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_big_images() {
        let f = synthetic(400, 300, 0);
        let g = cap_size(f, 100 * 100);
        assert_eq!((g.w, g.h), (100, 75));
        let small = cap_size(synthetic(64, 32, 0), MAX_PIXELS);
        assert_eq!((small.w, small.h), (64, 32));
    }

    #[test]
    fn halve_averages() {
        let f = Frame { w: 2, h: 2, bgra: vec![0, 0, 0, 255, 100, 100, 100, 255, 0, 0, 0, 255, 100, 100, 100, 255] };
        let g = halve(&f);
        assert_eq!(g.bgra, vec![50, 50, 50, 255]);
    }

    #[test]
    fn encodes_jpeg() {
        let j = jpeg(&synthetic(320, 200, 3), 80).unwrap();
        assert_eq!(&j[..2], &[0xff, 0xd8]);
        assert_eq!(&j[j.len() - 2..], &[0xff, 0xd9]);
        assert!(jpeg(&Frame { w: 0, h: 0, bgra: vec![] }, 80).is_err());
    }
}
