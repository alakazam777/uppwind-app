// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (19 nov., etape 3b) Base64 (l'alphabet standard, avec « = ») : le son d'un effet en direct passe entre le compagnon et
//! son processus enfant en lignes JSON. Sans dependance ; des echantillons f32 petit-boutistes.

const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(ALPHA[(n >> 18) as usize & 63] as char);
        s.push(ALPHA[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { ALPHA[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { ALPHA[n as usize & 63] as char } else { '=' });
    }
    s
}

fn val(c: u8) -> Option<u32> {
    Some(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    } as u32)
}

pub fn decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if !b.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 4 * 3);
    for c in b.chunks(4) {
        let pad = c.iter().rev().take_while(|x| **x == b'=').count();
        if pad > 2 {
            return None;
        }
        let mut n = 0u32;
        for (i, x) in c.iter().enumerate() {
            n <<= 6;
            if i < 4 - pad {
                n |= val(*x)?;
            }
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

pub fn encode_f32(x: &[f32]) -> String {
    let mut b = Vec::with_capacity(x.len() * 4);
    for v in x {
        b.extend_from_slice(&v.to_le_bytes());
    }
    encode(&b)
}

pub fn decode_f32(s: &str) -> Option<Vec<f32>> {
    let b = decode(s)?;
    if !b.len().is_multiple_of(4) {
        return None;
    }
    Some(b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for n in 0..10 {
            let b: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(decode(&encode(&b)).unwrap(), b);
        }
        assert_eq!(encode(b"Man"), "TWFu");
        assert_eq!(encode(b"Ma"), "TWE=");
        let x = [0.5f32, -1.0, 0.25, 3.0e-7];
        assert_eq!(decode_f32(&encode_f32(&x)).unwrap(), x);
        assert!(decode("TWF").is_none());
        assert!(decode("T*Fu").is_none());
    }
}
