// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Une file sans verrou a un producteur et un consommateur (SPSC), de taille fixe, pour des valeurs `Copy`.
//!
//! Le fil audio du DAW en tient un bout : il n'attend jamais, n'alloue jamais, ne fait aucun appel systeme. Une file
//! pleine refuse l'ecriture (l'appelant compte le bloc comme perdu et marque une discontinuite) au lieu d'attendre.
//! Tout est alloue a la creation (hors du fil audio). Les indices croissent sans fin (modulo usize) ; la capacite est
//! une puissance de 2. Ordre memoire : le producteur publie `tail` (Release) apres avoir ecrit ses cases, le
//! consommateur le lit (Acquire) avant de les lire, et inversement pour `head`.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Un compteur sur sa propre ligne de cache (le producteur et le consommateur ne se genent pas).
#[repr(align(64))]
struct Padded(AtomicUsize);

struct Inner<T> {
    buf: Box<[UnsafeCell<T>]>,
    mask: usize,
    /// la prochaine case a lire (seul le consommateur l'ecrit)
    head: Padded,
    /// la prochaine case a ecrire (seul le producteur l'ecrit)
    tail: Padded,
}

// Les cases ne sont jamais touchees par les deux bouts a la fois : le producteur n'ecrit que [tail, head + cap), le
// consommateur ne lit que [head, tail).
unsafe impl<T: Send> Sync for Inner<T> {}
unsafe impl<T: Send> Send for Inner<T> {}

impl<T> Inner<T> {
    fn base(&self) -> *mut T {
        UnsafeCell::raw_get(self.buf.as_ptr())
    }
}

pub struct Producer<T> {
    inner: Arc<Inner<T>>,
    tail: usize,
}

pub struct Consumer<T> {
    inner: Arc<Inner<T>>,
    head: usize,
}

/// Une file de `capacity` cases (arrondi a la puissance de 2 au-dessus, au moins 2). Allouer ici, jamais dans le fil
/// audio.
pub fn ring<T: Copy + Default + Send>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    let cap = capacity.max(2).next_power_of_two();
    let buf: Box<[UnsafeCell<T>]> = (0..cap).map(|_| UnsafeCell::new(T::default())).collect();
    let inner = Arc::new(Inner { buf, mask: cap - 1, head: Padded(AtomicUsize::new(0)), tail: Padded(AtomicUsize::new(0)) });
    (Producer { inner: inner.clone(), tail: 0 }, Consumer { inner, head: 0 })
}

impl<T: Copy> Producer<T> {
    pub fn capacity(&self) -> usize {
        self.inner.mask + 1
    }

    /// Les cases libres maintenant (le consommateur peut en liberer d'autres entre-temps, jamais en reprendre).
    pub fn free(&self) -> usize {
        let head = self.inner.head.0.load(Ordering::Acquire);
        self.capacity() - self.tail.wrapping_sub(head)
    }

    pub fn push(&mut self, v: T) -> bool {
        if self.free() == 0 {
            return false;
        }
        // SAFETY: la case `tail` est libre (free() > 0) : le consommateur ne la lira qu'apres la publication de tail.
        unsafe { *self.inner.base().add(self.tail & self.inner.mask) = v };
        self.tail = self.tail.wrapping_add(1);
        self.inner.tail.0.store(self.tail, Ordering::Release);
        true
    }

    /// Ecrire `n` cases d'un coup : `fill` recoit deux tranches dont les longueurs font `n` (la seconde, souvent vide,
    /// est le tour du tampon). S'il n'y a pas la place : false, rien n'est ecrit, `fill` n'est pas appelee.
    pub fn write_with(&mut self, n: usize, fill: impl FnOnce(&mut [T], &mut [T])) -> bool {
        if n > self.free() {
            return false;
        }
        let cap = self.capacity();
        let start = self.tail & self.inner.mask;
        let first = n.min(cap - start);
        // SAFETY: [tail, tail + n) est libre ; les deux tranches ne se chevauchent pas (start + first <= cap,
        // n - first <= start quand la seconde n'est pas vide, puisque n <= cap).
        let (a, b) = unsafe {
            let base = self.inner.base();
            (std::slice::from_raw_parts_mut(base.add(start), first), std::slice::from_raw_parts_mut(base, n - first))
        };
        fill(a, b);
        self.tail = self.tail.wrapping_add(n);
        self.inner.tail.0.store(self.tail, Ordering::Release);
        true
    }

    /// Tout ou rien.
    pub fn push_slice(&mut self, s: &[T]) -> bool {
        self.write_with(s.len(), |a, b| {
            let k = a.len();
            a.copy_from_slice(&s[..k]);
            b.copy_from_slice(&s[k..]);
        })
    }
}

impl<T: Copy> Consumer<T> {
    pub fn capacity(&self) -> usize {
        self.inner.mask + 1
    }

    pub fn len(&self) -> usize {
        self.inner.tail.0.load(Ordering::Acquire).wrapping_sub(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn peek(&self) -> Option<T> {
        if self.len() == 0 {
            return None;
        }
        // SAFETY: la case `head` est publiee (len() > 0, lu en Acquire).
        Some(unsafe { *self.inner.base().add(self.head & self.inner.mask) })
    }

    pub fn pop(&mut self) -> Option<T> {
        let v = self.peek()?;
        self.head = self.head.wrapping_add(1);
        self.inner.head.0.store(self.head, Ordering::Release);
        Some(v)
    }

    /// Lire et consommer `n` cases d'un coup (deux tranches, comme `write_with`) ; None s'il y en a moins.
    pub fn read_with<R>(&mut self, n: usize, f: impl FnOnce(&[T], &[T]) -> R) -> Option<R> {
        if n > self.len() {
            return None;
        }
        let cap = self.capacity();
        let start = self.head & self.inner.mask;
        let first = n.min(cap - start);
        // SAFETY: [head, head + n) est publie et le producteur n'y ecrira pas avant que head avance.
        let r = unsafe {
            let base = self.inner.base() as *const T;
            f(std::slice::from_raw_parts(base.add(start), first), std::slice::from_raw_parts(base, n - first))
        };
        self.head = self.head.wrapping_add(n);
        self.inner.head.0.store(self.head, Ordering::Release);
        Some(r)
    }

    /// Jeter jusqu'a `n` cases ; rend combien.
    pub fn skip(&mut self, n: usize) -> usize {
        let k = n.min(self.len());
        self.head = self.head.wrapping_add(k);
        self.inner.head.0.store(self.head, Ordering::Release);
        k
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_pop_full_empty() {
        let (mut p, mut c) = ring::<u32>(3);
        assert_eq!(p.capacity(), 4, "rounded up to a power of two");
        assert_eq!(c.pop(), None);
        for i in 0..4 {
            assert!(p.push(i));
        }
        assert!(!p.push(9), "full: refused, never waits");
        assert_eq!(p.free(), 0);
        assert_eq!(c.peek(), Some(0));
        assert_eq!(c.pop(), Some(0));
        assert!(p.push(4));
        assert_eq!((1..=4).map(|_| c.pop().unwrap()).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
        assert!(c.is_empty());
    }

    #[test]
    fn slices_wrap_around_the_buffer() {
        let (mut p, mut c) = ring::<u32>(8);
        assert!(p.push_slice(&[1, 2, 3, 4, 5, 6]));
        assert_eq!(c.skip(6), 6);
        // 5 cases a partir de la case 6 : 2 avant le tour, 3 apres
        let mut sizes = (0, 0);
        assert!(p.write_with(5, |a, b| {
            sizes = (a.len(), b.len());
            a.copy_from_slice(&[10, 11]);
            b.copy_from_slice(&[12, 13, 14]);
        }));
        assert_eq!(sizes, (2, 3));
        assert!(!p.write_with(4, |_, _| panic!("no room: not called")));
        let got = c.read_with(5, |a, b| [a, b].concat()).unwrap();
        assert_eq!(got, vec![10, 11, 12, 13, 14]);
        assert!(c.read_with(1, |_, _| ()).is_none());
        assert!(!p.push_slice(&[0; 9]), "larger than the ring: refused whole");
        assert!(c.is_empty());
    }

    #[test]
    fn two_threads_lose_nothing_and_keep_order() {
        // le producteur ecrit des blocs de tailles variees (comme des blocs audio), le consommateur lit par morceaux
        let (mut p, mut c) = ring::<u32>(1024);
        const N: u32 = 300_000;
        let prod = std::thread::spawn(move || {
            let mut next = 0u32;
            let mut size = 1usize;
            let mut refused = 0u64;
            while next < N {
                let n = size.min((N - next) as usize);
                let ok = p.write_with(n, |a, b| {
                    for (i, v) in a.iter_mut().chain(b.iter_mut()).enumerate() {
                        *v = next + i as u32;
                    }
                });
                if ok {
                    next += n as u32;
                } else {
                    refused += 1;
                    std::thread::yield_now();
                }
                size = size % 61 + 7;
            }
            refused
        });
        let mut expect = 0u32;
        let mut chunk = 1usize;
        while expect < N {
            let n = chunk.min(c.len());
            if n == 0 {
                std::thread::yield_now();
                continue;
            }
            c.read_with(n, |a, b| {
                for v in a.iter().chain(b.iter()) {
                    assert_eq!(*v, expect);
                    expect += 1;
                }
            })
            .unwrap();
            chunk = chunk % 97 + 3;
        }
        let _refused = prod.join().unwrap();
        assert!(c.is_empty());
    }
}
