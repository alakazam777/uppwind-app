// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (25 oct., phase 0) Une connexion d'un plugin « Uppwind Link » : la poignee de main (HELLO, la version, les preuves,
//! l'identite), puis les trames jusqu'a la fin. Un fil par connexion ; rien ici ne touche au studio directement : le son
//! passe aux abonnes (Instance::push_audio), les notes partent de Hub::send_midi par une copie du socket.
//! (29 oct., protocole v2, audit PL-1) HELLO porte un nonce, jamais le secret ; le compagnon repond CHALLENGE (son
//! nonce et sa preuve), le plugin AUTH (la sienne), verifiee en temps constant AVANT toute trame : sans elle, rien n'est
//! enregistre ni montre au studio. Un plugin v1 (le secret en clair) est refuse, et le studio le dit (Hub::note_old).

use super::{Hub, Instance};
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uwl_proto as p;

pub const HELLO_TIMEOUT: Duration = Duration::from_secs(2);
/// (v2) AUTH, apres le CHALLENGE, dans ce delai.
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(2);
/// Un secret (ou une preuve) faux pendant le blocage attend ceci avant d'etre refuse (le bon, lui, n'attend jamais).
pub const LOCKED_DELAY: Duration = Duration::from_secs(1);
const LOG_EVERY: Duration = Duration::from_secs(10);
/// (29 oct.) Ce qu'un plugin v1 lit (son statut) et ce que le studio montre.
pub const OLD_PLUGIN: &str = "an older Uppwind Link plugin: reinstall the plugins and restart your DAW";

fn welcome(s: &mut TcpStream, status: u8, message: &str) {
    let mut out = Vec::new();
    p::encode_welcome(&p::Welcome { version: p::VERSION, status, message: message.into() }, &mut out);
    let _ = s.write_all(&out);
}

/// La trame suivante dans `limit` depuis `t0`. Err(Some(raison)) : a dire dans le journal (le delai, une trame
/// absurde) ; Err(None) : le pair est parti (ferme, une erreur du socket), rien a dire.
fn next_frame(s: &mut TcpStream, rd: &mut p::FrameReader, buf: &mut [u8], t0: Instant, limit: Duration, what: &str) -> Result<(u8, Vec<u8>), Option<String>> {
    loop {
        match rd.next() {
            Ok(Some(f)) => return Ok(f),
            Ok(None) => {}
            // (un navigateur qui ecrirait « POST / HTTP/1.1 » tombe ici : une longueur absurde)
            Err(e) => return Err(Some(e.to_string())),
        }
        if t0.elapsed() > limit {
            return Err(Some(format!("no {what} within {} s", limit.as_secs())));
        }
        match s.read(buf) {
            Ok(0) => return Err(None),
            Ok(n) => rd.extend(&buf[..n]),
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {}
            Err(_) => return Err(None),
        }
    }
}

/// Un secret (v1) ou une preuve (v2) faux : compte ; trop en 30 s, les suivants sont ralentis et refuses — mais le bon
/// passe toujours, sinon n'importe quel programme local fermerait la porte aux vrais plugins du DAW.
fn refuse_wrong(s: &mut TcpStream, hub: &Hub, what: &str, who: &p::Hello) {
    let locked = hub.locked();
    hub.failure();
    if locked {
        std::thread::sleep(LOCKED_DELAY);
        welcome(s, p::status::REFUSED, &format!("too many wrong {what}s: wait 30 s"));
        crate::log(&format!("plugin link REFUSED: wrong {what} while the door is slowed down (too many wrong ones)"));
    } else {
        welcome(s, p::status::BAD_SECRET, &format!("wrong {what}: the companion state file changed (restart the DAW's plugin)"));
        crate::log(&format!("plugin link REFUSED: wrong {what} from {} {} (pid {})", who.kind.name(), who.host, who.pid));
    }
}

pub fn run(mut s: TcpStream, hub: Arc<Hub>, peer: SocketAddr) {
    let _ = s.set_nodelay(true);
    let _ = s.set_read_timeout(Some(Duration::from_millis(100)));
    let _ = s.set_write_timeout(Some(Duration::from_millis(500)));
    let mut rd = p::FrameReader::new(p::MAX_HELLO);
    let mut buf = vec![0u8; 64 * 1024];

    // 1. HELLO, la premiere trame, dans les 2 s
    let hello = match next_frame(&mut s, &mut rd, &mut buf, Instant::now(), HELLO_TIMEOUT, "hello") {
        Ok((p::ty::HELLO, body)) => match p::decode_hello(&body) {
            Ok(h) => h,
            Err(e) => {
                crate::log(&format!("plugin link REFUSED {peer}: {e}"));
                return;
            }
        },
        Ok((t, _)) => {
            crate::log(&format!("plugin link REFUSED {peer}: the first frame (0x{t:02x}) is not hello"));
            return;
        }
        Err(Some(e)) => {
            crate::log(&format!("plugin link REFUSED {peer}: {e}: closed"));
            return;
        }
        Err(None) => return,
    };

    // 2. la version. (29 oct.) Un plugin v1 a mis le secret en clair dans son HELLO : refuse, avec quoi faire. Son secret
    // est compare d'abord (temps constant), comme avant : seul un vrai plugin d'avant fait apparaitre l'avis au studio
    if let Some(old_secret) = hello.v1_secret() {
        if !hub.secret_ok(old_secret) {
            refuse_wrong(&mut s, &hub, "secret", &hello);
            return;
        }
        // (l'avis d'abord : quand le plugin lit le refus, le studio peut deja le savoir)
        let new = hub.note_old(&hello);
        welcome(&mut s, p::status::BAD_VERSION, OLD_PLUGIN);
        if new {
            crate::log(&format!(
                "plugin link REFUSED an older Uppwind Link plugin (protocol 1): {} \"{}\" in {} (pid {}): reinstall the plugins and restart the DAW",
                hello.kind.name(),
                hello.name,
                if hello.host.is_empty() { "?" } else { &hello.host },
                hello.pid
            ));
        }
        return;
    }
    if hello.version != p::VERSION {
        welcome(&mut s, p::status::BAD_VERSION, &format!("the companion speaks version {}", p::VERSION));
        crate::log(&format!("plugin link REFUSED: {} speaks version {}, the companion {}", hello.name, hello.version, p::VERSION));
        return;
    }
    if !(8000..=384_000).contains(&hello.rate) || hello.max_block > 65536 || hello.channels == 0 || hello.channels > p::MAX_CHANNELS {
        welcome(&mut s, p::status::REFUSED, "bad format");
        crate::log(&format!("plugin link REFUSED: bad format from {} ({} Hz, block {}, {} ch)", hello.name, hello.rate, hello.max_block, hello.channels));
        return;
    }

    // 3. (v2) le defi : la preuve du compagnon (le plugin la verifie avant tout), puis celle du plugin, avant toute trame
    let nonce = crate::security::new_nonce();
    let mut out = Vec::new();
    p::encode_challenge(&p::Challenge { nonce, proof: hub.companion_proof(&hello.nonce, &nonce) }, &mut out);
    if s.write_all(&out).is_err() {
        return;
    }
    let proof = match next_frame(&mut s, &mut rd, &mut buf, Instant::now(), AUTH_TIMEOUT, "auth") {
        Ok((p::ty::AUTH, body)) => body,
        Ok((t, _)) => {
            crate::log(&format!("plugin link REFUSED {} {}: frame 0x{t:02x} instead of auth", hello.kind.name(), hello.host));
            return;
        }
        Err(e) => {
            crate::log(&format!("plugin link: {} {} gave no proof ({}): closed", hello.kind.name(), hello.host, e.as_deref().unwrap_or("it left")));
            return;
        }
    };
    if !hub.plugin_proof_ok(&nonce, &hello.nonce, &proof) {
        refuse_wrong(&mut s, &hub, "proof", &hello);
        return;
    }
    let serial = hub.next_serial();
    let inst = Arc::new(Instance {
        id: hello.instance,
        kind: hello.kind,
        name: hello.name.clone(),
        host: hello.host.clone(),
        pid: hello.pid,
        rate: AtomicU32::new(hello.rate),
        max_block: AtomicU32::new(hello.max_block),
        tag: AtomicU32::new(0),
        since: Instant::now(),
        serial,
        writer: Mutex::new(None),
        subs: Mutex::new(Vec::new()),
        stats: Mutex::new(Value::Null),
        emitted: Mutex::new(VecDeque::new()),
        frames_in: AtomicU64::new(0),
        events_out: AtomicU64::new(0),
        ages: Mutex::new(VecDeque::new()),
    });
    if inst.kind == p::Kind::Notes {
        match s.try_clone() {
            Ok(w) => {
                let _ = w.set_write_timeout(Some(Duration::from_millis(200)));
                *inst.writer.lock().unwrap() = Some(w);
            }
            Err(e) => {
                welcome(&mut s, p::status::REFUSED, "the companion cannot write to this plugin");
                crate::log(&format!("plugin link: cannot clone the socket of {} ({e})", hello.name));
                return;
            }
        }
    }
    if !hub.register(inst.clone()) {
        welcome(&mut s, p::status::DUPLICATE, "an instance with this identity is already connected");
        crate::log(&format!("plugin link: {} {} is already connected (a duplicated track?): the newcomer takes a new identity", hello.name, inst.hex()));
        return;
    }
    welcome(&mut s, p::status::OK, "welcome");
    crate::log(&format!(
        "PLUGIN connected: {} \"{}\" {} from {} (pid {}), {} Hz, block {}",
        inst.kind.name(),
        inst.name,
        inst.hex(),
        if inst.host.is_empty() { "?" } else { &inst.host },
        inst.pid,
        hello.rate,
        hello.max_block
    ));

    // 3. les trames
    rd.set_max(p::MAX_FRAME);
    let _ = s.set_read_timeout(Some(Duration::from_millis(250)));
    let mut samples: Vec<f32> = Vec::new();
    let mut emitted: Vec<p::Emitted> = Vec::new();
    let mut last_log = Instant::now();
    let reason: String = 'outer: loop {
        loop {
            match rd.next() {
                Ok(Some((t, body))) => match t {
                    p::ty::AUDIO if inst.kind == p::Kind::Return => match p::decode_audio_into(&body, &mut samples) {
                        Ok(h) => inst.push_audio(&h, &samples),
                        Err(e) => break 'outer format!("bad audio frame ({e})"),
                    },
                    p::ty::FORMAT => {
                        if let Ok(f) = p::decode_format(&body) {
                            let old = inst.rate.swap(f.rate, Ordering::Relaxed);
                            inst.max_block.store(f.max_block, Ordering::Relaxed);
                            if old != f.rate {
                                crate::log(&format!("plugin {} {}: now {} Hz, block {}", inst.name, inst.hex(), f.rate, f.max_block));
                            }
                        }
                    }
                    p::ty::STATS => {
                        if let Ok(v) = p::decode_text(&body).map(|t| serde_json::from_str::<Value>(t).unwrap_or(Value::Null)) {
                            *inst.stats.lock().unwrap() = v;
                        }
                    }
                    // (25 oct.) le numero « Link tag » que le Remote Script de Live a donne a ce Notes
                    p::ty::TAG if inst.kind == p::Kind::Notes => {
                        if let Ok(t) = p::decode_tag(&body) {
                            let old = inst.tag.swap(t, Ordering::Relaxed);
                            if old != t {
                                crate::log(&format!("plugin {} {}: Link tag {t}", inst.name, inst.hex()));
                            }
                        }
                    }
                    p::ty::EMITTED if inst.kind == p::Kind::Notes => {
                        if p::decode_emitted_into(&body, &mut emitted).is_ok() {
                            inst.keep_emitted(&emitted);
                        }
                    }
                    p::ty::BYE => break 'outer format!("the plugin said bye ({})", p::decode_text(&body).unwrap_or("")),
                    _ => {}
                },
                Ok(None) => break,
                Err(e) => break 'outer format!("bad frame ({e})"),
            }
        }
        if last_log.elapsed() >= LOG_EVERY {
            last_log = Instant::now();
            log_stats(&inst);
        }
        match s.read(&mut buf) {
            Ok(0) => break 'outer "the plugin closed the connection (removed, or the DAW quit)".into(),
            Ok(n) => rd.extend(&buf[..n]),
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {}
            Err(e) => break 'outer format!("connection lost ({e})"),
        }
    };
    hub.unregister(inst.id, serial);
    *inst.writer.lock().unwrap() = None;
    inst.end_subs(&format!("the Uppwind Link Return plugin left: {reason}"));
    crate::log(&format!("PLUGIN gone: {} \"{}\" {}: {reason}", inst.kind.name(), inst.name, inst.hex()));
}

/// Une ligne toutes les 10 s : ce que le plugin dit de lui (STATS) et ce que le compagnon voit.
fn log_stats(inst: &Instance) {
    let st = inst.stats.lock().map(|s| s.clone()).unwrap_or(Value::Null);
    let f = |k: &str| st[k].as_f64().unwrap_or(0.0);
    match inst.kind {
        p::Kind::Return => {
            let ages = inst.age_summary();
            crate::log(&format!(
                "plugin {} {}: {} frames in, DAW callback jitter sd {:.3} ms (max {:.2}), {} frames dropped, arrival age median {} ms (p90 {}), {} listener(s)",
                inst.name,
                inst.hex(),
                inst.frames_in.load(Ordering::Relaxed),
                f("jitterMs"),
                f("jitterMaxMs"),
                f("dropped"),
                ages["median"],
                ages["p90"],
                inst.subs.lock().map(|s| s.len()).unwrap_or(0)
            ));
        }
        p::Kind::Notes => {
            crate::log(&format!(
                "plugin {} {}: {} events sent, {} emitted, {} late (max {:.2} ms), {} stale, {} overflow, DAW callback jitter sd {:.3} ms (max {:.2})",
                inst.name,
                inst.hex(),
                inst.events_out.load(Ordering::Relaxed),
                f("emitted"),
                f("late"),
                f("lateMaxMs"),
                f("stale"),
                f("overflow"),
                f("jitterMs"),
                f("jitterMaxMs")
            ));
        }
    }
}
