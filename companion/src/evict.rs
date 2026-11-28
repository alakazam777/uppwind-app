//! (24 nov., relecture, probleme 7 ; audit F1) Les connexions qui n'ont pas encore prouve le code d'appairage.
//!
//! Quand les places d'un port sont toutes prises, la plus ancienne de ces connexions est fermee pour laisser entrer la
//! nouvelle : seize clients lents (un octet, puis plus rien) ne bloquent plus le vrai studio ni les vrais plugins. Une
//! connexion qui a prouve le code n'est jamais fermee ainsi.

use std::collections::VecDeque;
use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Une connexion plus jeune que cela n'est pas fermee pour une autre : sans ce delai, des clients qui se reconnectent
/// des qu'ils sont coupes se fermaient l'un l'autre en cascade, jusqu'au studio qui venait d'entrer (essai reel du
/// 24 nov.). Un vrai studio prouve le code en quelques millisecondes.
pub const GRACE: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct Unproven {
    // du plus ancien au plus recent : (numero, une copie de la connexion)
    list: Mutex<(u64, VecDeque<(u64, Instant, TcpStream)>)>,
}

impl Unproven {
    pub fn new() -> Arc<Unproven> {
        Arc::new(Unproven::default())
    }

    /// Une connexion acceptee, pas encore appairee. Sans copie possible de la connexion : un ticket vide (elle ne pourra
    /// pas etre fermee pour une autre, rien de plus).
    pub fn enter(self: &Arc<Self>, s: &TcpStream) -> Ticket {
        let Ok(copy) = s.try_clone() else { return Ticket { reg: self.clone(), id: None } };
        let mut l = self.list.lock().unwrap_or_else(|e| e.into_inner());
        l.0 += 1;
        let id = l.0;
        l.1.push_back((id, Instant::now(), copy));
        Ticket { reg: self.clone(), id: Some(id) }
    }

    /// Ferme la plus ancienne connexion pas encore appairee, si elle a au moins GRACE. Faux sinon.
    pub fn evict_oldest(&self) -> bool {
        self.evict_older_than(GRACE)
    }

    fn evict_older_than(&self, grace: Duration) -> bool {
        // (sous le verrou : une connexion qui prouve le code au meme instant n'est jamais fermee apres sa preuve)
        let mut l = self.list.lock().unwrap_or_else(|e| e.into_inner());
        if !l.1.front().is_some_and(|(_, t, _)| t.elapsed() >= grace) {
            return false;
        }
        if let Some((_, _, s)) = l.1.pop_front() {
            let _ = s.shutdown(Shutdown::Both);
        }
        true
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.list.lock().unwrap().1.len()
    }

    fn leave(&self, id: u64) {
        self.list.lock().unwrap_or_else(|e| e.into_inner()).1.retain(|(i, _, _)| *i != id);
    }
}

/// La place d'une connexion dans la liste : elle en sort quand elle prouve le code (`proved`) ou quand elle se termine.
pub struct Ticket {
    reg: Arc<Unproven>,
    id: Option<u64>,
}

impl Ticket {
    /// Le code est prouve : cette connexion ne sera plus fermee pour une autre.
    pub fn proved(&mut self) {
        if let Some(id) = self.id.take() {
            self.reg.leave(id);
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.proved();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    fn pair(l: &TcpListener) -> (TcpStream, TcpStream) {
        let c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let (s, _) = l.accept().unwrap();
        (c, s)
    }

    #[test]
    fn the_oldest_unproven_is_closed_and_a_proved_one_never() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let reg = Unproven::new();
        let (mut c1, s1) = pair(&l);
        let (mut c2, s2) = pair(&l);
        let (_c3, s3) = pair(&l);
        let mut t1 = reg.enter(&s1);
        let _t2 = reg.enter(&s2);
        let t3 = reg.enter(&s3);
        t1.proved(); // la premiere a prouve le code
        drop(t3); // la troisieme s'est terminee
        assert_eq!(reg.len(), 1);
        assert!(!reg.evict_oldest(), "younger than GRACE: it stays");
        assert!(reg.evict_older_than(Duration::ZERO), "the second one goes");
        let mut b = [0u8; 1];
        assert_eq!(c2.read(&mut b).unwrap_or(0), 0, "closed");
        c1.set_read_timeout(Some(std::time::Duration::from_millis(100))).unwrap();
        assert!(c1.read(&mut b).is_err(), "the proved one stays open");
        assert!(!reg.evict_older_than(Duration::ZERO), "nobody left to close");
    }
}
