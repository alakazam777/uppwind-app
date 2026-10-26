// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Les controles de securite du compagnon. Des fonctions pures, testees sans reseau.
//!
//! 1. On n'ecoute que 127.0.0.1 (main.rs : un seul socket, `TcpListener::bind("127.0.0.1:…")`).
//! 2. L'en-tete Host doit etre exactement `127.0.0.1:<notre port>` : un site qui ferait pointer son propre nom vers
//!    127.0.0.1 (DNS rebinding) envoie son nom dans Host, il est refuse.
//! 3. L'en-tete Origin doit etre dans la liste blanche : le studio en ligne, et rien d'autre par defaut (les adresses
//!    de dev, localhost et 127.0.0.1 sur quelques ports, seulement avec --dev-origins ; --allow-origin n'accepte que
//!    https:// ou une adresse locale). Pas d'Origin (un programme local, curl) : refuse aussi. L'origine seule ne
//!    suffit pas (un programme local peut la falsifier) : d'ou le jeton.
//! 4. Le premier message doit porter le jeton d'appairage (256 bits, compare en temps constant). Trop d'essais faux :
//!    on bloque un moment (`PairingGuard`).

use std::time::{Duration, Instant};

/// L'origine du studio en ligne (/atelier) : la seule que le compagnon accepte par defaut.
pub const PROD_ORIGIN: &str = "https://uppwind.lucasmoreau.fr";
/// Les ports des serveurs de dev du studio : 5174 (un serveur de dev local), 5747 (les essais et tests du compagnon).
/// Acceptes seulement avec --dev-origins (audit de publication : jamais par defaut dans un exe public).
pub const DEV_PORTS: &[u16] = &[5174, 5747];
/// L'origine du studio que cet exe accepte par defaut, fixee a la compilation : une copie du compagnon pour un autre
/// studio se construit avec `UPPWIND_STUDIO_ORIGIN=https://… cargo build` (et --studio-origin la remplace au
/// lancement) ; sans elle, c'est PROD_ORIGIN.
pub fn default_studio_origin() -> &'static str {
    match option_env!("UPPWIND_STUDIO_ORIGIN") {
        Some(o) if !o.trim().is_empty() => o,
        _ => PROD_ORIGIN,
    }
}

/// Une origine qu'on peut ajouter (--allow-origin, --studio-origin) : `https://hote[:port]`, ou une adresse locale
/// `http://localhost[:port]` / `http://127.0.0.1[:port]` ; sans chemin, sans identifiants. → sa forme comparee
/// (minuscules, sans « / » final). Tout le reste (http:// vers un autre hote, file:, un chemin…) est refuse.
pub fn normalize_origin(o: &str) -> Result<String, String> {
    let s = o.trim().trim_end_matches('/').to_ascii_lowercase();
    let bad = || Err(format!("\"{}\" is not an origin like https://studio.example or http://localhost:5747", o.trim()));
    let (scheme, rest) = match s.split_once("://") {
        Some(x) => x,
        None => return bad(),
    };
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.len() <= 5 && p.bytes().all(|c| c.is_ascii_digit()) => (h, Some(p)),
        Some(_) => return bad(),
        None => (rest, None),
    };
    if let Some(p) = port {
        if p.parse::<u32>().map(|n| n == 0 || n > 65535).unwrap_or(true) {
            return bad();
        }
    }
    let host_ok = !host.is_empty()
        && host.len() <= 253
        && host.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')
        && !host.starts_with(['.', '-'])
        && !host.ends_with(['.', '-'])
        && !host.contains("..");
    if !host_ok {
        return bad();
    }
    match scheme {
        "https" => Ok(s),
        "http" if host == "localhost" || host == "127.0.0.1" => Ok(s),
        _ => bad(),
    }
}

#[derive(Debug, Clone)]
pub struct Allowlist {
    origins: Vec<String>,
}

impl Allowlist {
    /// La liste par defaut : le studio (default_studio_origin), plus `http://localhost:<p>` et `http://127.0.0.1:<p>`
    /// pour les ports de dev si `dev` est vrai. `extra` : des origines exactes en plus (option --allow-origin).
    pub fn new(dev: bool, extra: &[String]) -> Self {
        Self::with_studio(Some(default_studio_origin()), dev, extra)
    }

    /// `studio` : l'origine du studio (None : aucune — seulement `extra` et, avec `dev`, les adresses de dev).
    pub fn with_studio(studio: Option<&str>, dev: bool, extra: &[String]) -> Self {
        let mut origins: Vec<String> = Vec::new();
        let mut add = |o: &str| {
            let o = o.trim().trim_end_matches('/').to_ascii_lowercase();
            if !o.is_empty() && !origins.contains(&o) {
                origins.push(o);
            }
        };
        if let Some(s) = studio {
            add(s);
        }
        if dev {
            for p in DEV_PORTS {
                add(&format!("http://localhost:{p}"));
                add(&format!("http://127.0.0.1:{p}"));
            }
        }
        for e in extra {
            add(e);
        }
        Allowlist { origins }
    }

    pub fn origins(&self) -> &[String] {
        &self.origins
    }

    /// L'origine telle que le navigateur l'envoie (schema://hote[:port], sans chemin). Comparaison exacte, sans
    /// tenir compte de la casse ; « null » (une page en file:// ou un iframe isole) est refuse.
    pub fn allows(&self, origin: Option<&str>) -> bool {
        match origin {
            None => false,
            Some(o) => {
                let o = o.trim().to_ascii_lowercase();
                !o.is_empty() && o != "null" && self.origins.iter().any(|a| *a == o)
            }
        }
    }
}

/// Host doit etre exactement « 127.0.0.1:<port> » (le studio se connecte a ws://127.0.0.1:<port>).
pub fn host_ok(host: Option<&str>, port: u16) -> bool {
    match host {
        Some(h) => h.trim().eq_ignore_ascii_case(&format!("127.0.0.1:{port}")),
        None => false,
    }
}

/// Comparaison en temps constant (la duree ne dit pas combien de caracteres du debut sont justes).
pub fn token_eq(given: &str, expected: &str) -> bool {
    let a = given.as_bytes();
    let b = expected.as_bytes();
    if a.len() != b.len() || b.is_empty() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Un jeton de 256 bits en hexadecimal (64 caracteres).
pub fn new_token() -> String {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).expect("no system random source");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Un jeton bien forme : 64 caracteres hexadecimaux minuscules.
pub fn token_well_formed(t: &str) -> bool {
    t.len() == 64 && t.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// Les codes d'appairage (« UWC-<port>-<64 hex> ») d'un texte, sans leur jeton → le texte nettoye et combien il y en
/// avait. Sert a nettoyer un journal ecrit par un compagnon d'avant (qui y mettait le code en clair).
pub fn scrub_pairing_codes(text: &str) -> (String, usize) {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut n, mut i, mut last) = (0usize, 0usize, 0usize);
    while let Some(off) = text[i..].find("UWC-") {
        let at = i + off;
        let mut j = at + 4;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > at + 4 && j < b.len() && b[j] == b'-' {
            let (t0, t1) = (j + 1, j + 1 + 64);
            if t1 <= b.len() && b[t0..t1].iter().all(|c| c.is_ascii_hexdigit()) && (t1 == b.len() || !b[t1].is_ascii_hexdigit()) {
                out.push_str(&text[last..t0]);
                out.push_str("<removed>");
                last = t1;
                n += 1;
                i = t1;
                continue;
            }
        }
        i = at + 4;
    }
    out.push_str(&text[last..]);
    (out, n)
}

/// Trop d'essais de jeton faux : on refuse tout appairage pendant `lock` (contre un programme qui essaierait au
/// hasard ; avec 256 bits c'est surtout pour le journal et pour ne pas laisser marteler le port).
pub struct PairingGuard {
    max_failures: u32,
    lock: Duration,
    failures: u32,
    locked_until: Option<Instant>,
}

impl PairingGuard {
    pub fn new(max_failures: u32, lock: Duration) -> Self {
        PairingGuard { max_failures, lock, failures: 0, locked_until: None }
    }
    pub fn locked(&mut self, now: Instant) -> bool {
        match self.locked_until {
            Some(t) if now < t => true,
            Some(_) => {
                self.locked_until = None;
                self.failures = 0;
                false
            }
            None => false,
        }
    }
    pub fn failure(&mut self, now: Instant) {
        self.failures += 1;
        if self.failures >= self.max_failures {
            self.locked_until = Some(now + self.lock);
        }
    }
    pub fn success(&mut self) {
        self.failures = 0;
    }
}

/// Pourquoi une poignee de main est refusee (le journal le dit ; le navigateur ne voit qu'un 403).
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    BadHost,
    BadOrigin,
}

/// Le controle de la poignee de main HTTP → WebSocket : Host puis Origin.
pub fn check_handshake(host: Option<&str>, origin: Option<&str>, port: u16, allow: &Allowlist) -> Result<(), Refusal> {
    if !host_ok(host, port) {
        return Err(Refusal::BadHost);
    }
    if !allow.allows(origin) {
        return Err(Refusal::BadOrigin);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_must_be_loopback_with_our_port() {
        assert!(host_ok(Some("127.0.0.1:47410"), 47410));
        assert!(!host_ok(Some("127.0.0.1:47411"), 47410), "another port");
        assert!(!host_ok(Some("127.0.0.1"), 47410), "no port");
        assert!(!host_ok(Some("localhost:47410"), 47410), "only the exact address the studio uses");
        assert!(!host_ok(Some("evil.example:47410"), 47410), "DNS rebinding");
        assert!(!host_ok(Some("127.0.0.1.evil.example:47410"), 47410));
        assert!(!host_ok(Some("[::1]:47410"), 47410));
        assert!(!host_ok(None, 47410), "no Host header");
    }

    #[test]
    fn origin_must_be_in_the_allowlist() {
        let a = Allowlist::new(true, &[]);
        assert!(a.allows(Some("https://uppwind.lucasmoreau.fr")));
        assert!(a.allows(Some("HTTPS://Uppwind.LucasMoreau.fr")), "case does not matter");
        assert!(a.allows(Some("http://localhost:5747")));
        assert!(a.allows(Some("http://127.0.0.1:5174")));
        assert!(!a.allows(Some("http://uppwind.lucasmoreau.fr")), "http is not the studio");
        assert!(!a.allows(Some("https://uppwind.lucasmoreau.fr.evil.example")));
        assert!(!a.allows(Some("https://evil.example")));
        assert!(!a.allows(Some("http://localhost:8080")), "not a dev port");
        assert!(!a.allows(Some("null")), "file:// or a sandboxed iframe");
        assert!(!a.allows(Some("")));
        assert!(!a.allows(None), "no Origin (a local program)");
        let prod = Allowlist::new(false, &[]);
        assert!(prod.allows(Some("https://uppwind.lucasmoreau.fr")));
        assert!(!prod.allows(Some("http://localhost:5747")), "dev origins off");
        let extra = Allowlist::new(false, &["http://localhost:9999/".to_string()]);
        assert!(extra.allows(Some("http://localhost:9999")));
        // un autre studio a la place du studio en ligne (--studio-origin) ; aucun (none) : seulement les ajouts
        let other = Allowlist::with_studio(Some("https://studio.example"), false, &[]);
        assert!(other.allows(Some("https://studio.example")) && !other.allows(Some(PROD_ORIGIN)));
        assert_eq!(other.origins().len(), 1);
        let none = Allowlist::with_studio(None, false, &[]);
        assert!(none.origins().is_empty() && !none.allows(Some(PROD_ORIGIN)));
        assert_eq!(Allowlist::new(false, &[]).origins(), &[default_studio_origin().to_string()], "by default: the studio, nothing else");
    }

    #[test]
    fn added_origins_are_https_or_local_only() {
        assert_eq!(normalize_origin(" https://Studio.Example/ ").as_deref(), Ok("https://studio.example"));
        assert_eq!(normalize_origin("https://studio.example:8443").as_deref(), Ok("https://studio.example:8443"));
        assert_eq!(normalize_origin("http://localhost:5747").as_deref(), Ok("http://localhost:5747"));
        assert_eq!(normalize_origin("http://127.0.0.1:5174").as_deref(), Ok("http://127.0.0.1:5174"));
        for bad in [
            "http://studio.example",           // en clair sur le reseau
            "http://192.0.2.10:5747",          // une autre machine
            "https://studio.example/atelier",  // un chemin
            "https://user@studio.example",     // des identifiants
            "file://",
            "null",
            "",
            "https://",
            "https://studio.example:0",
            "https://studio.example:99999",
            "https://-bad.example",
            "https://a..b",
            "ftp://studio.example",
            "studio.example",
        ] {
            assert!(normalize_origin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn handshake_checks_host_then_origin() {
        let a = Allowlist::new(true, &[]);
        assert_eq!(check_handshake(Some("127.0.0.1:5000"), Some("http://localhost:5747"), 5000, &a), Ok(()));
        assert_eq!(check_handshake(Some("evil.example:5000"), Some("http://localhost:5747"), 5000, &a), Err(Refusal::BadHost));
        assert_eq!(check_handshake(Some("127.0.0.1:5000"), Some("https://evil.example"), 5000, &a), Err(Refusal::BadOrigin));
        assert_eq!(check_handshake(Some("127.0.0.1:5000"), None, 5000, &a), Err(Refusal::BadOrigin));
    }

    #[test]
    fn tokens() {
        let t = new_token();
        assert!(token_well_formed(&t), "{t}");
        assert_ne!(t, new_token(), "random");
        assert!(token_eq(&t, &t.clone()));
        assert!(!token_eq("", &t), "missing");
        assert!(!token_eq(&t[..63], &t), "too short");
        let mut wrong = t.clone().into_bytes();
        wrong[10] = if wrong[10] == b'a' { b'b' } else { b'a' };
        assert!(!token_eq(std::str::from_utf8(&wrong).unwrap(), &t), "one character off");
        assert!(!token_eq("x", ""), "an empty expected token never matches");
        assert!(!token_well_formed("ABC"));
    }

    #[test]
    fn pairing_codes_are_scrubbed_from_logs() {
        let t = new_token();
        let log = format!("[    0.0s] start\n    UWC-47410-{t}\nan example UWC-1-abc\n    UWC-47411-{t}\n");
        let (s, n) = scrub_pairing_codes(&log);
        assert_eq!(n, 2);
        assert!(!s.contains(&t), "{s}");
        assert!(s.contains("UWC-47410-<removed>") && s.contains("UWC-47411-<removed>") && s.contains("UWC-1-abc"), "{s}");
        assert_eq!(scrub_pairing_codes("nothing here"), ("nothing here".to_string(), 0));
        assert_eq!(scrub_pairing_codes("é UWC-5-é UWC-"), ("é UWC-5-é UWC-".to_string(), 0), "not a code, and UTF-8 stays whole");
        let long = format!("UWC-2-{t}0");
        assert_eq!(scrub_pairing_codes(&long).1, 0, "65 hex digits: not a code");
    }

    #[test]
    fn pairing_guard_locks_after_failures() {
        let t0 = Instant::now();
        let mut g = PairingGuard::new(3, Duration::from_secs(30));
        assert!(!g.locked(t0));
        g.failure(t0);
        g.failure(t0);
        assert!(!g.locked(t0));
        g.failure(t0);
        assert!(g.locked(t0 + Duration::from_secs(1)));
        assert!(!g.locked(t0 + Duration::from_secs(31)), "unlocked after the delay");
        g.failure(t0);
        g.success();
        g.failure(t0);
        g.failure(t0);
        assert!(!g.locked(t0), "a success resets the count");
    }
}
