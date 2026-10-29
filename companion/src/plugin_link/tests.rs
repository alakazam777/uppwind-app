// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! De vraies connexions sur 127.0.0.1 (un port au hasard) : de faux plugins parlent au hub comme les vrais.

use super::*;
use crate::audio::feed::Out;
use std::io::Read;

const SECRET: [u8; 32] = [0x5a; 32];

struct Fake {
    s: TcpStream,
    rd: p::FrameReader,
}

/// (29 oct.) Ce que le faux plugin repond au CHALLENGE.
#[derive(Clone, Copy)]
enum Answer {
    /// sa preuve, faite avec le secret qu'on lui donne (juste ou non)
    Proof,
    /// ces 32 octets (une preuve rejouee, fausse…)
    Raw([u8; 32]),
    /// une autre trame a la place (FORMAT : un plugin qui sauterait AUTH)
    Skip,
    /// rien du tout
    Silent,
}

impl Fake {
    fn hello(kind: p::Kind, id: u64, nonce: [u8; 32], rate: u32) -> p::Hello {
        p::Hello { version: p::VERSION, kind, flags: 0, nonce, instance: id, rate, max_block: 512, channels: 2, pid: 1234, name: format!("Fake {}", kind.name()), host: "Ableton Live 12 Suite.exe".into() }
    }

    fn connect(hub: &Hub, kind: p::Kind, id: u64, secret: [u8; 32], rate: u32) -> (Fake, p::Welcome) {
        let (f, w, _) = Fake::handshake(hub, Fake::hello(kind, id, crate::security::new_nonce(), rate), secret, Answer::Proof);
        (f, w.expect("welcome"))
    }

    /// Un faux plugin v2 : HELLO, puis le CHALLENGE du compagnon (sa preuve verifiee avec `secret`), puis `answer`, puis
    /// le verdict. → (le faux, le WELCOME s'il vient, le CHALLENGE recu)
    fn handshake(hub: &Hub, hello: p::Hello, secret: [u8; 32], answer: Answer) -> (Fake, Option<p::Welcome>, Option<(p::Challenge, bool)>) {
        let s = TcpStream::connect(("127.0.0.1", hub.port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
        let mut f = Fake { s, rd: p::FrameReader::new(p::MAX_FRAME) };
        let mut out = Vec::new();
        p::encode_hello(&hello, &mut out);
        f.s.write_all(&out).unwrap();
        let Some((t, body)) = f.frame() else { return (f, None, None) };
        if t == p::ty::WELCOME {
            // un refus du HELLO, sans CHALLENGE
            return (f, Some(p::decode_welcome(&body).unwrap()), None);
        }
        assert_eq!(t, p::ty::CHALLENGE);
        let ch = p::decode_challenge(&body).unwrap();
        let proved = p::auth::companion_proof_ok(&secret, hub.port, &hello.nonce, &ch.nonce, &ch.proof);
        out.clear();
        match answer {
            Answer::Proof => p::encode_auth(&p::auth::plugin_proof(&secret, hub.port, &ch.nonce, &hello.nonce), &mut out),
            Answer::Raw(b) => p::encode_auth(&b, &mut out),
            Answer::Skip => p::encode_format(&p::Format { rate: 48000, max_block: 512, channels: 2 }, &mut out),
            Answer::Silent => {}
        }
        f.s.write_all(&out).unwrap();
        let w = f.frame().map(|(t, body)| {
            assert_eq!(t, p::ty::WELCOME);
            p::decode_welcome(&body).unwrap()
        });
        (f, w, Some((ch, proved)))
    }

    fn frame(&mut self) -> Option<(u8, Vec<u8>)> {
        let mut buf = [0u8; 4096];
        loop {
            if let Some(f) = self.rd.next().unwrap() {
                return Some(f);
            }
            match self.s.read(&mut buf) {
                Ok(0) => return None,
                Ok(n) => self.rd.extend(&buf[..n]),
                Err(_) => return None,
            }
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.s.write_all(bytes).unwrap();
    }
}

fn wait_for(mut f: impl FnMut() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < Duration::from_secs(3), "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn audio_frame(pos: u64, t: f64, rate: u32, frames: usize, value: f32, flags: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let samples = vec![value; frames * 2];
    p::encode_audio(&p::AudioHeader { pos, t_ms: t, rate, frames: frames as u16, channels: 2, flags }, &samples, &mut out);
    out
}

#[test]
fn a_return_plugin_feeds_the_studio_in_uwa1_packets() {
    let hub = Hub::listen(0, SECRET).unwrap();
    let (mut f, w) = Fake::connect(&hub, p::Kind::Return, 0xabc, SECRET, 48000);
    assert_eq!(w.status, p::status::OK);
    wait_for(|| hub.get(0xabc).is_some());
    let list = hub.instances_json();
    assert_eq!(list["list"][0]["id"], "0000000000000abc");
    assert_eq!(list["list"][0]["daw"], "live");
    let mut feed = hub.start_feed("plugin", 48000, SampleFormat::F32, 5.0).unwrap();
    assert_eq!((feed.info["capture"].as_str(), feed.info["frames"].as_u64(), feed.info["resampled"].as_bool()), (Some("plugin"), Some(240), Some(false)));
    // 10 blocs de 128 trames (2,67 ms) a partir de la trame 1000 : 5 paquets de 240 trames
    let t0 = 1_761_300_000_000.0;
    for k in 0..10u64 {
        f.send(&audio_frame(1000 + k * 128, t0 + k as f64 * 128.0 / 48.0, 48000, 128, 0.25, if k == 0 { p::flags::DISCONTINUITY } else { 0 }));
    }
    let mut got = Vec::new();
    wait_for(|| {
        while let Some(o) = feed.next() {
            match o {
                Out::Packet(b) => got.push(packet::decode(&b).unwrap()),
                Out::Ended(e) => panic!("{e}"),
            }
        }
        got.len() >= 5
    });
    assert_eq!(got.len(), 5);
    for (i, (h, s)) in got.iter().enumerate() {
        assert_eq!(h.pos, 1000 + i as u64 * 240);
        assert!((h.t_ms - (t0 + i as f64 * 5.0)).abs() < 1e-6, "{} vs {}", h.t_ms, t0 + i as f64 * 5.0);
        assert_eq!(h.frames, 240);
        assert!(s.iter().all(|v| *v == 0.25));
    }
    assert_eq!(got[0].0.flags & packet::FLAG_DISCONTINUITY, packet::FLAG_DISCONTINUITY);
    assert_eq!(got[1].0.flags, 0);
    // le studio arrete : l'abonnement disparait au bloc suivant
    drop(feed);
    f.send(&audio_frame(2280, t0 + 30.0, 48000, 128, 0.25, 0));
    wait_for(|| hub.get(0xabc).map(|i| i.subs.lock().unwrap().is_empty()).unwrap_or(false));
    // un deuxieme flux, puis le plugin part : le flux le dit
    let mut feed = hub.start_feed("plugin:abc", 48000, SampleFormat::I16, 5.0).unwrap();
    drop(f);
    let mut ended = None;
    wait_for(|| {
        // (une fois l'emetteur parti, next() redit « fini » a chaque appel : session.rs s'arrete au premier)
        while let Some(o) = feed.next() {
            if let Out::Ended(e) = o {
                ended = Some(e);
                break;
            }
        }
        ended.is_some()
    });
    assert!(ended.unwrap().contains("left"));
    wait_for(|| hub.get(0xabc).is_none());
    assert!(hub.start_feed("plugin", 48000, SampleFormat::F32, 5.0).err().unwrap().contains("no Uppwind Link Return"));
}

/// (25 oct.) Le studio vise « tag:<n> » : le Notes qui a annonce ce numero (trame TAG), le plus recent s'ils sont deux.
#[test]
fn notes_are_found_by_their_link_tag() {
    use crate::protocol::MidiIn;
    let hub = Hub::listen(0, SECRET).unwrap();
    let (mut a, _) = Fake::connect(&hub, p::Kind::Notes, 0xa1, SECRET, 48000);
    let (mut b, _) = Fake::connect(&hub, p::Kind::Notes, 0xb2, SECRET, 48000);
    wait_for(|| hub.get(0xa1).is_some() && hub.get(0xb2).is_some());
    let ev = [MidiIn { t: None, d: vec![0x90, 60, 100], n: Some(1) }];
    assert!(hub.send_midi("tag:3", &ev).unwrap_err().contains("no Uppwind Link Notes with tag 3"), "nobody has tag 3 yet");
    let mut out = Vec::new();
    p::encode_tag(3, &mut out);
    a.send(&out);
    out.clear();
    p::encode_tag(4, &mut out);
    b.send(&out);
    wait_for(|| hub.notes_by_tag(3).is_some() && hub.notes_by_tag(4).is_some());
    assert_eq!(hub.instances_json()["list"][0]["tag"], 3);
    assert_eq!(hub.send_midi("tag:4", &ev), Ok(1));
    let (t, body) = b.frame().unwrap();
    assert_eq!(t, p::ty::EVENTS);
    let mut got = Vec::new();
    p::decode_events_into(&body, &mut got).unwrap();
    assert_eq!(got[0].data, [0x90, 60, 100]);
    assert_eq!(hub.send_midi("tag:3", &ev), Ok(1));
    assert_eq!(a.frame().unwrap().0, p::ty::EVENTS);
    for bad in ["tag:0", "tag:256", "tag:x", "tag:"] {
        assert!(hub.send_midi(bad, &ev).unwrap_err().contains("bad tag"), "{bad}");
    }
    // un numero qui change (le script l'a reecrit : une piste dupliquee) : l'ancien ne mene plus a rien
    out.clear();
    p::encode_tag(9, &mut out);
    a.send(&out);
    wait_for(|| hub.notes_by_tag(9).is_some());
    assert!(hub.send_midi("tag:3", &ev).is_err());
    // le meme numero sur deux Notes : le plus recent
    out.clear();
    p::encode_tag(9, &mut out);
    b.send(&out);
    wait_for(|| hub.get(0xb2).unwrap().tag.load(Ordering::Relaxed) == 9);
    assert_eq!(hub.notes_by_tag(9).unwrap().id, 0xb2);
    // un Return ne repond jamais a un numero
    let (_r, _) = Fake::connect(&hub, p::Kind::Return, 0xc3, SECRET, 48000);
    wait_for(|| hub.get(0xc3).is_some());
    assert_eq!(hub.notes_by_tag(9).unwrap().id, 0xb2);
}

#[test]
fn a_44100_hz_daw_reaches_a_48000_hz_studio() {
    let hub = Hub::listen(0, SECRET).unwrap();
    let (mut f, _) = Fake::connect(&hub, p::Kind::Return, 0x44, SECRET, 44100);
    wait_for(|| hub.get(0x44).is_some());
    let mut feed = hub.start_feed("plugin:44", 48000, SampleFormat::F32, 5.0).unwrap();
    assert_eq!(feed.info["resampled"], true);
    // 0,5 s a 44,1 kHz → ~24000 trames a 48 kHz → ~100 paquets de 240
    for k in 0..(22050 / 147) as u64 {
        f.send(&audio_frame(k * 147, 1.0e12 + k as f64 * 147.0 / 44.1, 44100, 147, 0.5, 0));
    }
    let mut frames = 0usize;
    let mut last_pos = None;
    wait_for(|| {
        while let Some(Out::Packet(b)) = feed.next() {
            let (h, s) = packet::decode(&b).unwrap();
            assert_eq!(h.rate, 48000);
            if let Some(lp) = last_pos {
                assert_eq!(h.pos, lp, "contiguous");
            }
            last_pos = Some(h.pos + h.frames as u64);
            frames += h.frames as usize;
            assert!(s.iter().all(|v| (*v - 0.5).abs() < 1e-6));
        }
        frames >= 23_760
    });
}

#[test]
fn notes_go_to_the_right_instance_and_bad_ones_are_refused() {
    let hub = Hub::listen(0, SECRET).unwrap();
    let (mut a, _) = Fake::connect(&hub, p::Kind::Notes, 0xa, SECRET, 48000);
    let (mut b, _) = Fake::connect(&hub, p::Kind::Notes, 0xb, SECRET, 48000);
    wait_for(|| hub.get(0xa).is_some() && hub.get(0xb).is_some());
    let now = now_ms();
    let evs = vec![
        crate::protocol::MidiIn { t: Some(now + 20.0), d: vec![0x90, 60, 100], n: Some(7) },
        crate::protocol::MidiIn { t: None, d: vec![0xe0, 0, 0x40], n: None },
    ];
    assert_eq!(hub.send_midi("000000000000000b", &evs).unwrap(), 2);
    let (t, body) = b.frame().unwrap();
    assert_eq!(t, p::ty::EVENTS);
    let mut got = Vec::new();
    p::decode_events_into(&body, &mut got).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!((got[0].n, got[0].data, got[0].t_ms), (7, [0x90, 60, 100], now + 20.0));
    assert!((got[1].t_ms - now).abs() < 1000.0, "no time: now");
    assert_eq!(got[1].n, 1);
    // rien pour l'autre
    a.s.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
    assert!(a.frame().is_none());
    // refuses : un SysEx, une donnee >= 0x80, un instant fou, une instance absente ou d'un autre genre
    let bad = |d: Vec<u8>, t: Option<f64>| vec![crate::protocol::MidiIn { t, d, n: None }];
    assert!(hub.send_midi("b", &bad(vec![0xf0, 1, 2], None)).is_err());
    assert!(hub.send_midi("b", &bad(vec![0x90, 200, 2], None)).is_err());
    assert!(hub.send_midi("b", &bad(vec![0x90, 60, 2], Some(now + 3_600_000.0))).unwrap_err().contains("time"));
    assert!(hub.send_midi("c", &bad(vec![0x90, 60, 2], None)).unwrap_err().contains("not connected"));
    let (_r, _) = Fake::connect(&hub, p::Kind::Return, 0xd, SECRET, 48000);
    wait_for(|| hub.get(0xd).is_some());
    assert!(hub.send_midi("d", &bad(vec![0x90, 60, 2], None)).unwrap_err().contains("not a Notes"));
    // les mesures du plugin reviennent par EMITTED
    let mut out = Vec::new();
    p::encode_emitted(&[p::Emitted { n: 7, t_ms: now + 20.01, offset: 33, frames: 128, late_ms: 0.0 }], &mut out);
    b.send(&out);
    let mut list = Vec::new();
    wait_for(|| {
        list.extend(hub.take_emitted("b").unwrap());
        !list.is_empty()
    });
    assert_eq!((list[0].n, list[0].offset), (7, 33));
}

#[test]
fn wrong_secrets_duplicates_and_strangers_are_refused() {
    let hub = Hub::listen(0, SECRET).unwrap();
    let (_a, w) = Fake::connect(&hub, p::Kind::Notes, 0x1, SECRET, 48000);
    assert_eq!(w.status, p::status::OK);
    wait_for(|| hub.get(0x1).is_some());
    // la meme identite (une piste dupliquee) : refusee, le nouveau venu en prendra une autre
    let (_b, w) = Fake::connect(&hub, p::Kind::Notes, 0x1, SECRET, 48000);
    assert_eq!(w.status, p::status::DUPLICATE);
    // un mauvais secret : refuse ; apres le 5e, les faux sont ralentis et refuses, mais le bon passe toujours
    for _ in 0..5 {
        let (_c, w) = Fake::connect(&hub, p::Kind::Return, 0x9, [1; 32], 48000);
        assert_eq!(w.status, p::status::BAD_SECRET);
    }
    let t0 = Instant::now();
    let (_c, w) = Fake::connect(&hub, p::Kind::Return, 0x9, [1; 32], 48000);
    assert_eq!(w.status, p::status::REFUSED);
    assert!(t0.elapsed() >= conn::LOCKED_DELAY, "a wrong secret during the lock waits");
    let (_d, w) = Fake::connect(&hub, p::Kind::Return, 0x9, SECRET, 48000);
    assert_eq!(w.status, p::status::OK, "the right secret is never locked out");
    // un navigateur (ou n'importe quoi d'autre) : ferme sans reponse
    let mut s = TcpStream::connect(("127.0.0.1", hub.port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    s.write_all(b"POST / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
    let mut buf = [0u8; 16];
    assert!(matches!(s.read(&mut buf), Ok(0) | Err(_)), "closed, nothing said");
}

/// (29 oct., protocole v2, audit PL-1) Le compagnon prouve qu'il connait le secret (sur le nonce du plugin et son port),
/// puis exige la preuve du plugin avant toute trame : une preuve fausse, rejouee (d'une autre connexion), absente, ou
/// une trame a la place d'AUTH, et rien n'est enregistre — le studio ne voit pas ce plugin et ne lui donne rien.
#[test]
fn a_plugin_is_registered_only_after_its_proof() {
    let hub = Hub::listen(0, SECRET).unwrap();
    let np = [0x11; 32];
    // la preuve du compagnon : juste pour le vrai secret, pour CE nonce, pour CE port
    let (ok, w, ch) = Fake::handshake(&hub, Fake::hello(p::Kind::Return, 0x71, np, 48000), SECRET, Answer::Proof);
    let (ch, proved) = ch.unwrap();
    assert!(proved, "the companion proves it knows the secret");
    assert_eq!(w.unwrap().status, p::status::OK);
    assert!(!p::auth::companion_proof_ok(&SECRET, hub.port.wrapping_add(1), &np, &ch.nonce, &ch.proof), "bound to the port");
    assert!(!p::auth::companion_proof_ok(&[1; 32], hub.port, &np, &ch.nonce, &ch.proof), "useless without the secret");
    wait_for(|| hub.get(0x71).is_some());
    // le meme HELLO (meme nonce) une seconde fois : un autre nonce du compagnon, une autre preuve
    let (_f2, _, ch2) = Fake::handshake(&hub, Fake::hello(p::Kind::Return, 0x72, np, 48000), SECRET, Answer::Raw([0; 32]));
    let (ch2, _) = ch2.unwrap();
    assert_ne!(ch2.nonce, ch.nonce, "a fresh companion nonce for every connection");
    drop(ok);
    // une preuve rejouee : la bonne preuve d'une connexion d'avant ne vaut rien sur la suivante
    let replay = p::auth::plugin_proof(&SECRET, hub.port, &ch.nonce, &np);
    for (what, answer) in [("a replayed proof", Answer::Raw(replay)), ("a random proof", Answer::Raw([7; 32])), ("no proof", Answer::Silent), ("another frame instead of auth", Answer::Skip)] {
        let (_f, w, _) = Fake::handshake(&hub, Fake::hello(p::Kind::Notes, 0x73, np, 48000), SECRET, answer);
        if let Some(w) = w {
            assert_eq!(w.status, p::status::BAD_SECRET, "{what}");
        }
        assert!(hub.get(0x73).is_none(), "{what}: never registered");
        assert!(hub.instances_json()["list"].as_array().unwrap().iter().all(|i| i["id"] != "0000000000000073"), "{what}: the studio never sees it");
        assert!(hub.send_midi("0000000000000073", &[crate::protocol::MidiIn { t: None, d: vec![0x90, 60, 100], n: None }]).is_err(), "{what}: no notes go to it");
    }
    // une preuve faite pour un autre port (un relais par un vieux port du fichier d'etat) : refusee
    let s = TcpStream::connect(("127.0.0.1", hub.port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut f = Fake { s, rd: p::FrameReader::new(p::MAX_FRAME) };
    let mut out = Vec::new();
    let h = Fake::hello(p::Kind::Notes, 0x75, [0x13; 32], 48000);
    p::encode_hello(&h, &mut out);
    f.send(&out);
    let (t, body) = f.frame().unwrap();
    assert_eq!(t, p::ty::CHALLENGE);
    let c = p::decode_challenge(&body).unwrap();
    out.clear();
    p::encode_auth(&p::auth::plugin_proof(&SECRET, hub.port.wrapping_add(1), &c.nonce, &h.nonce), &mut out);
    f.send(&out);
    assert_eq!(p::decode_welcome(&f.frame().unwrap().1).unwrap().status, p::status::BAD_SECRET, "a proof for another port");
    assert!(hub.get(0x75).is_none());
}

/// (29 oct.) Un plugin d'avant (protocole v1 : le secret en clair dans HELLO) est refuse avec ce qu'il faut faire, et le
/// studio le voit (« refused » dans instances, un avis « notice ») — une fois par instance ; un faux secret v1 ne fait
/// rien apparaitre.
#[test]
fn an_older_plugin_is_refused_with_a_reason_the_studio_shows() {
    let hub = Hub::listen(0, SECRET).unwrap();
    assert_eq!((hub.notices_generation(), hub.old_notice()), (0, None));
    let v1 = |secret: [u8; 32], id: u64| p::Hello { version: p::V1, ..Fake::hello(p::Kind::Return, id, secret, 48000) };
    // un faux secret v1 : refuse comme avant, rien pour le studio
    let (_f, w, ch) = Fake::handshake(&hub, v1([9; 32], 0x91), SECRET, Answer::Silent);
    assert!(ch.is_none(), "no challenge for a v1 hello");
    assert_eq!(w.unwrap().status, p::status::BAD_SECRET);
    assert_eq!(hub.notices_generation(), 0);
    // le vrai secret, en v1 : refuse (version), avec la raison
    for _ in 0..3 {
        let (_f, w, ch) = Fake::handshake(&hub, v1(SECRET, 0x92), SECRET, Answer::Silent);
        assert!(ch.is_none());
        let w = w.unwrap();
        assert_eq!(w.status, p::status::BAD_VERSION);
        assert!(w.message.contains("older Uppwind Link plugin") && w.message.contains("reinstall the plugins"), "{}", w.message);
    }
    assert!(hub.get(0x92).is_none(), "never registered");
    assert_eq!(hub.notices_generation(), 1, "once per instance, not at every retry");
    let n = hub.old_notice().expect("a notice for the studio");
    assert_eq!((n["type"].as_str(), n["code"].as_str()), (Some("notice"), Some("old-plugin")));
    assert!(n["message"].as_str().unwrap().contains("reinstall the plugins and restart your DAW"), "{n}");
    assert!(n["message"].as_str().unwrap().contains("Ableton Live"), "{n}");
    let refused = &hub.instances_json()["refused"];
    assert_eq!((refused[0]["kind"].as_str(), refused[0]["daw"].as_str(), refused[0]["pid"].as_u64()), (Some("return"), Some("live"), Some(1234)), "{refused}");
    assert!(!refused.to_string().contains(&p::secret_hex(&SECRET)), "the secret is never kept nor shown");
    // une autre instance d'avant : un avis de plus
    let _ = Fake::handshake(&hub, v1(SECRET, 0x93), SECRET, Answer::Silent);
    assert_eq!(hub.notices_generation(), 2);
}

#[test]
fn plugin_connections_are_capped_from_the_accept() {
    let hub = Hub::listen(0, SECRET).unwrap();
    // MAX_CONNECTIONS connexions muettes (avant HELLO) tiennent toutes les places
    let idle: Vec<TcpStream> = (0..MAX_CONNECTIONS).map(|_| TcpStream::connect(("127.0.0.1", hub.port)).unwrap()).collect();
    std::thread::sleep(Duration::from_millis(200));
    let mut s = TcpStream::connect(("127.0.0.1", hub.port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut buf = [0u8; 16];
    assert!(matches!(s.read(&mut buf), Ok(0) | Err(_)), "one more is closed at once");
    drop(idle);
    // les places reviennent des que ces connexions se ferment
    std::thread::sleep(Duration::from_millis(500));
    let (_f, w) = Fake::connect(&hub, p::Kind::Return, 0x42, SECRET, 48000);
    assert_eq!(w.status, p::status::OK);
}

#[test]
fn a_connected_return_is_preferred_for_its_own_daw() {
    let hub = Hub::listen(0, SECRET).unwrap();
    assert_eq!(hub.preferred_return("live"), None, "no plugin: the Windows capture, as before");
    let (_f, _) = Fake::connect(&hub, p::Kind::Return, 0x77, SECRET, 48000); // host : « Ableton Live 12 Suite.exe »
    wait_for(|| hub.get(0x77).is_some());
    assert_eq!(hub.preferred_return("live").as_deref(), Some("plugin:0000000000000077"));
    assert_eq!(hub.preferred_return("auto").as_deref(), Some("plugin:0000000000000077"));
    assert_eq!(hub.preferred_return("fl"), None, "a Return in Live is not FL's sound");
    assert_eq!(hub.preferred_return("test-tone"), None);
    assert_eq!(hub.preferred_return("1234"), None);
}

#[test]
fn the_secret_survives_restarts() {
    let dir = std::env::temp_dir().join(format!("uwc-plugin-secret-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("state.json");
    let a = load_secret(&f);
    std::fs::write(&f, format!(r#"{{"token":"t","pluginSecret":"{}"}}"#, p::secret_hex(&a))).unwrap();
    assert_eq!(load_secret(&f), a);
    std::fs::write(&f, r#"{"pluginSecret":"short"}"#).unwrap();
    assert_ne!(load_secret(&f), a);
    let _ = std::fs::remove_dir_all(&dir);
}

/// (25 oct., un essai reel) le studio a demande le son de Live AVANT que le Return ne se connecte (le compagnon venait
/// de redemarrer) : la capture d'abord ; le Return arrive, le flux passe a lui ; il part, la capture reprend.
#[test]
fn a_live_feed_follows_a_return_that_connects_later_and_leaves() {
    use crate::audio::feed::{follow_returns, open, Want};
    let hub = Hub::listen(0, SECRET).unwrap();
    let w = Want { target: "live".into(), rate: 48000, fmt: SampleFormat::F32, packet_ms: 5.0, demo: true, test_tone: None };
    let mut feed = Some(open(Some(&hub), &w).unwrap());
    assert_eq!(feed.as_ref().unwrap().info["capture"], "demo", "no Return yet: the capture (the demo sine here)");
    assert!(feed.as_ref().unwrap().source.is_none());
    assert!(follow_returns(Some(&hub), &mut feed, false).is_none(), "nothing changed");
    // une demande « fl » ne suit pas un Return de Live ; « plugin:<id> » ne suit rien
    let mut fl = Some(open(Some(&hub), &Want { target: "fl".into(), ..w.clone() }).unwrap());
    // le Return se connecte
    let (mut f, _) = Fake::connect(&hub, p::Kind::Return, 0x5e, SECRET, 48000);
    wait_for(|| hub.get(0x5e).is_some());
    let msg: Value = serde_json::from_str(&follow_returns(Some(&hub), &mut feed, false).expect("switched")).unwrap();
    assert_eq!((msg["type"].as_str(), msg["state"].as_str(), msg["capture"].as_str(), msg["switched"].as_str()), (Some("audio"), Some("on"), Some("plugin"), Some("plugin")));
    assert_eq!(msg["plugin"]["id"], "000000000000005e");
    assert_eq!(feed.as_ref().unwrap().source.as_deref(), Some("plugin:000000000000005e"));
    assert!(follow_returns(Some(&hub), &mut fl, false).is_none(), "FL's feed stays on its capture");
    assert!(follow_returns(Some(&hub), &mut feed, false).is_none(), "once switched, nothing more");
    // le son vient bien du plugin
    let t0 = 1_761_300_000_000.0;
    for k in 0..10u64 {
        f.send(&audio_frame(k * 128, t0 + k as f64 * 128.0 / 48.0, 48000, 128, 0.25, 0));
    }
    let mut got = 0;
    wait_for(|| {
        while let Some(o) = feed.as_mut().unwrap().next() {
            if let Out::Packet(b) = o {
                let (_, s) = packet::decode(&b).unwrap();
                if s.iter().all(|v| *v == 0.25) {
                    got += 1;
                }
            }
        }
        got >= 5
    });
    // un deuxieme Return ne lui prend pas le flux
    let (_g, _) = Fake::connect(&hub, p::Kind::Return, 0x5f, SECRET, 48000);
    wait_for(|| hub.get(0x5f).is_some());
    assert!(follow_returns(Some(&hub), &mut feed, false).is_none(), "the feed keeps its Return");
    // le premier part : l'autre Return de Live le reprend ; puis lui aussi part : la capture
    drop(f);
    wait_for(|| hub.get(0x5e).is_none());
    let msg: Value = serde_json::from_str(&follow_returns(Some(&hub), &mut feed, false).expect("switched")).unwrap();
    assert_eq!((msg["capture"].as_str(), msg["plugin"]["id"].as_str()), (Some("plugin"), Some("000000000000005f")));
    drop(_g);
    wait_for(|| hub.get(0x5f).is_none());
    // (comme session.rs : la fin du flux du plugin, ou la generation, d'abord — les deux menent a la capture)
    let msg: Value = serde_json::from_str(&follow_returns(Some(&hub), &mut feed, true).expect("back to the capture")).unwrap();
    assert_eq!((msg["state"].as_str(), msg["capture"].as_str(), msg["switched"].as_str()), (Some("on"), Some("demo"), Some("capture")));
    assert!(feed.as_ref().unwrap().source.is_none() && feed.as_ref().unwrap().want.is_some());
    // une cible precise ne suit pas
    let (_h, _) = Fake::connect(&hub, p::Kind::Return, 0x60, SECRET, 48000);
    wait_for(|| hub.get(0x60).is_some());
    let mut pinned = Some(open(Some(&hub), &Want { target: "plugin:60".into(), ..w.clone() }).unwrap());
    assert!(pinned.as_ref().unwrap().want.is_none());
    assert!(follow_returns(Some(&hub), &mut pinned, true).is_none());
}
