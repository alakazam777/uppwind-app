// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Une connexion du studio : poignee de main controlee (Host, Origin), appairage (jeton dans le premier message),
//! puis les messages du protocole, les gestes et le flux d'images d'UNE fenetre de plugin. Un fil par connexion
//! (socket non bloquant, TCP_NODELAY) ; la capture tourne sur un fil a elle (stream.rs) pour que les gestes soient
//! injectes des leur arrivee, meme pendant une capture.

use crate::capture;
use crate::input::{self, Injector, Mode};
use crate::protocol::{self, ClientMsg, InputEvent, RawInput, PROTOCOL_VERSION};
use crate::security::{self, Refusal};
use crate::stream::{Req, Resp, Worker};
use crate::{encode, live, plugins, Shared};
use serde_json::json;
use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tungstenite::{Error as WsError, Message, WebSocket};

pub const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// Au plus tant de connexions a la fois, comptees des l'acceptation (une connexion qui traine avant sa poignee de main
/// occupe aussi une place) : au-dela, fermee tout de suite. Un studio en ouvre une.
pub const MAX_CONNECTIONS: usize = 16;
/// Ce que tungstenite garde au plus a envoyer pour une connexion : un studio qui ne lit plus fait tomber la connexion au
/// lieu de faire grossir la memoire (deux images au plus en vol, quelques Mo chacune au pire).
pub const MAX_WRITE_BUFFER: usize = 32 * 1024 * 1024;
/// Le retour audio quand le socket reste plein : les paquets sont jetes (pas empiles), et au-dela de ce temps le flux
/// s'arrete.
pub const AUDIO_STALL_MAX: Duration = Duration::from_secs(3);
/// Un mauvais code pendant le blocage de l'appairage : la reponse attend (un essai au hasard coute du temps). Le bon code,
/// lui, passe toujours (sinon n'importe quel programme local bloquerait le vrai studio en se trompant expres).
pub const LOCKED_DELAY: Duration = Duration::from_millis(1000);
pub const MAX_IN_FLIGHT: u64 = 2;
pub const MAX_MSGS_PER_SEC: u32 = 1000; // (des mouvements a 250/s au plus, plus les accuses des images)
pub const CLOSE_BAD_TOKEN: u16 = 4401;
pub const CLOSE_LOCKED: u16 = 4429;
pub const CLOSE_PROTOCOL: u16 = 4400;
pub const DEMO_ID: u64 = 1;
/// Apres le dernier geste, la voie rapide (capture continue, tuiles) continue encore un instant.
pub const GESTURE_HOLD: Duration = Duration::from_millis(250);
pub const GESTURE_FPS: f64 = 60.0;
/// Apres un geste injecte, la capture attend ce temps (le plugin lit le message et se redessine).
pub const INPUT_TO_CAPTURE: Duration = Duration::from_millis(4);
/// Avec la surface DWM (1-2 ms, rien demande au fil du plugin) : pendant un geste, une capture toutes les 4 ms, la
/// premiere 1 ms apres l'injection. Mesure du 21 oct. sur Diva : il se redessine 15-22 ms apres un clic ; a 60 images/s
/// PrintWindow le voyait 17-44 ms apres (mediane 35), une capture de 13-15 ms toutes les 16,7 ms.
pub const GESTURE_POLL_CHEAP: Duration = Duration::from_millis(4);
pub const INPUT_TO_CAPTURE_CHEAP: Duration = Duration::from_millis(1);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// L'instant en ms depuis 1970, a la microseconde (la meme horloge que performance.timeOrigin + now() du studio).
fn now_f() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn refuse(status: u16, why: &str) -> ErrorResponse {
    let mut r = ErrorResponse::new(Some(why.to_string()));
    *r.status_mut() = tungstenite::http::StatusCode::from_u16(status).unwrap_or(tungstenite::http::StatusCode::FORBIDDEN);
    r
}

/// La poignee de main : refusee (403) si Host ou Origin ne vont pas. Rend la connexion WebSocket et l'origine.
pub fn handshake(stream: TcpStream, sh: &Shared) -> Option<(WebSocket<TcpStream>, String)> {
    let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let mut origin_seen = String::new();
    let port = sh.port;
    let allow = &sh.allow;
    let cb = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        let host = req.headers().get("host").and_then(|v| v.to_str().ok());
        let origin = req.headers().get("origin").and_then(|v| v.to_str().ok());
        origin_seen = origin.unwrap_or("").to_string();
        match security::check_handshake(host, origin, port, allow) {
            Ok(()) => Ok(resp),
            Err(Refusal::BadHost) => {
                crate::log(&format!("REFUSED {peer}: Host {:?} is not 127.0.0.1:{port}", host.unwrap_or("<none>")));
                Err(refuse(403, "bad host"))
            }
            Err(Refusal::BadOrigin) => {
                crate::log(&format!("REFUSED {peer}: Origin {:?} is not allowed", origin.unwrap_or("<none>")));
                Err(refuse(403, "origin not allowed"))
            }
        }
    };
    let cfg = WebSocketConfig::default()
        .max_message_size(Some(protocol::MAX_TEXT_MESSAGE))
        .max_frame_size(Some(protocol::MAX_TEXT_MESSAGE))
        .max_write_buffer_size(MAX_WRITE_BUFFER);
    match tungstenite::accept_hdr_with_config(stream, cb, Some(cfg)) {
        Ok(ws) => Some((ws, origin_seen)),
        Err(e) => {
            let s = e.to_string();
            if !s.contains("Handshake not finished") {
                crate::log(&format!("handshake failed ({s})"));
            }
            None
        }
    }
}

/// La fenetre suivie : son fil de capture et la cadence.
struct Watch {
    id: u64,
    title: String,
    /// Le processus de la fenetre a l'instant ou elle a ete verifiee (un handle reutilise par une autre fenetre
    /// changerait de processus : les gestes s'arretent). 0 : la fenetre de demonstration.
    pid: u32,
    tx: Option<mpsc::Sender<Req>>,
    rx: mpsc::Receiver<Resp>,
    busy: bool,
    interval: Duration,
    next: Instant,
    quality: u8,
    /// Une tuile floue (geste) a ete envoyee : une image nette suivra la fin du geste.
    blurry: bool,
    /// La capture est bon marche (surface DWM) : la voie rapide la refait toutes les GESTURE_POLL_CHEAP.
    cheap: bool,
    sent: u64,
}

/// Les mesures des gestes (le journal toutes les 10 s, sans jamais ce qui est tape).
#[derive(Default)]
struct InputStats {
    count: u64,
    merged: u64,
    refused: u64,
    inject_ms: f64,
    inject_max: f64,
    transit_ms: f64,
    transit_n: u64,
    since: Option<Instant>,
}

struct Conn<'a> {
    ws: WebSocket<TcpStream>,
    sh: &'a Shared,
    pending_flush: bool,
    seq: u64,
    acked: u64,
    watch: Option<Watch>,
    inj: Injector,
    /// Le dernier geste injecte (son numero), son instant, ou est le pointeur (pixels de la zone client).
    input_n: u64,
    /// Le meme numero, partage avec le fil de capture (il le lit au debut de chaque capture).
    latest_input: Arc<AtomicU64>,
    last_input: Option<Instant>,
    pointer: Option<(i32, i32)>,
    stats: InputStats,
    places: u64,
    /// « live » : la reponse du script de dev arrive d'un fil a part (jusqu'a PROBE_TIMEOUT si Live s'est tu) ;
    /// les gestes et les images n'attendent pas. Une seule demande a la fois.
    probe: Option<mpsc::Receiver<live::Probe>>,
    /// « open » / « close » : leurs reponses arrivent de leurs fils (editor_request) ; combien sont en route
    ed_tx: mpsc::Sender<String>,
    ed_rx: mpsc::Receiver<String>,
    editors: Arc<AtomicU64>,
    /// la derniere liste des plugins du set (« devices ») et son heure
    last_devices: Option<(Instant, Vec<live::Device>)>,
    /// (24 oct.) le retour audio sans cable : le flux du DAW (audio/feed.rs), apres l'appairage seulement
    audio: Option<crate::audio::feed::Feed>,
    /// (25 oct., sans loopMIDI) le lien du studio avec le Remote Script de Live par son canal local (link_relay.rs)
    link: Option<crate::link_relay::Relay>,
}

enum Fatal {
    Closed,
}

impl<'a> Conn<'a> {
    fn send_text(&mut self, s: String) -> Result<(), Fatal> {
        self.push(Message::text(s))
    }
    fn push(&mut self, m: Message) -> Result<(), Fatal> {
        match self.ws.send(m) {
            Ok(()) => Ok(()),
            Err(WsError::Io(e)) if e.kind() == ErrorKind::WouldBlock => {
                self.pending_flush = true;
                Ok(())
            }
            Err(_) => Err(Fatal::Closed),
        }
    }
    fn flush(&mut self) -> Result<(), Fatal> {
        if !self.pending_flush {
            return Ok(());
        }
        match self.ws.flush() {
            Ok(()) => {
                self.pending_flush = false;
                Ok(())
            }
            Err(WsError::Io(e)) if e.kind() == ErrorKind::WouldBlock => Ok(()),
            Err(_) => Err(Fatal::Closed),
        }
    }
    fn close(&mut self, code: u16, why: &str) {
        let _ = self.ws.close(Some(CloseFrame { code: CloseCode::from(code), reason: why.to_string().into() }));
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(500) {
            match self.ws.flush() {
                Ok(()) => break,
                Err(WsError::Io(e)) if e.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => break,
            }
        }
    }

    // ── les fenetres ─────────────────────────────────────────────────────────────────────────────────────────
    fn windows(&self) -> (bool, Vec<plugins::PluginWindow>) {
        if self.sh.demo {
            let w = plugins::PluginWindow { id: DEMO_ID, title: "Demo Synth/1-Demo".into(), plugin: "Demo Synth".into(), track: "1-Demo".into(), w: 640, h: 400, minimized: false };
            return (true, vec![w]);
        }
        let all = plugins::enumerate();
        let mut wins = plugins::plugin_windows(&all);
        // --test-window : la fenetre d'essai du compagnon lui-meme (testwin.rs), proposee comme un plugin
        if let Some(t) = self.sh.test_window {
            if let Some(raw) = all.iter().find(|w| w.hwnd == t && w.visible) {
                wins.push(plugins::PluginWindow { id: t, title: raw.title.clone(), plugin: "Uppwind Test".into(), track: "0-Test".into(), w: raw.w, h: raw.h, minimized: raw.minimized });
            }
        }
        (plugins::live_running(&all), wins)
    }

    fn on_list(&mut self) -> Result<(), Fatal> {
        let (live, wins) = self.windows();
        self.send_text(json!({ "type": "windows", "live": live, "windows": wins }).to_string())
    }

    fn on_devices(&mut self) -> Result<(), Fatal> {
        if self.sh.demo {
            return self.send_text(
                json!({ "type": "devices", "ok": true, "devices": [{ "path": "0.0", "name": "Demo Synth", "track": "1-Demo", "editorApi": true, "window": DEMO_ID }] }).to_string(),
            );
        }
        let t0 = Instant::now();
        match live::request(json!({ "cmd": "LIST" }), Duration::from_millis(1500)) {
            Ok(rep) if rep["ok"].as_bool() == Some(true) => {
                let t_list = t0.elapsed();
                let (_, wins) = self.windows();
                if crate::stream::trace() {
                    crate::log(&format!("devices: Live's LIST {} ms, windows {} ms", t_list.as_millis(), (t0.elapsed() - t_list).as_millis()));
                }
                let list = live::plugins_of_list(&rep);
                self.last_devices = Some((Instant::now(), list.clone()));
                let devs: Vec<serde_json::Value> = list
                    .into_iter()
                    .map(|d| {
                        let t = plugins::expected_title(&d.name, &d.track);
                        let win = wins.iter().find(|w| w.title == t).map(|w| w.id);
                        json!({ "path": d.path, "name": d.name, "track": d.track, "editorApi": d.editor_api, "window": win })
                    })
                    .collect();
                self.send_text(json!({ "type": "devices", "ok": true, "devices": devs }).to_string())
            }
            Ok(rep) => self.send_text(json!({ "type": "devices", "ok": false, "error": rep["error"].as_str().unwrap_or("Live refused") }).to_string()),
            Err(e) => self.send_text(json!({ "type": "devices", "ok": false, "error": e }).to_string()),
        }
    }

    /// « live » : le script de dev repond-il maintenant ? La reponse part de poll_probe (la boucle).
    fn on_live(&mut self) -> Result<(), Fatal> {
        if self.sh.demo {
            return self.send_text(json!({ "type": "live", "channel": true, "answers": true, "ms": 0.0, "demo": true }).to_string());
        }
        if self.probe.is_some() {
            return Ok(()); // une demande en route : sa reponse vaut pour les deux
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(live::probe());
        });
        self.probe = Some(rx);
        Ok(())
    }

    fn poll_probe(&mut self) -> Result<(), Fatal> {
        let Some(rx) = &self.probe else { return Ok(()) };
        match rx.try_recv() {
            Ok(p) => {
                self.probe = None;
                // (25 oct.) version, link : le script sert-il le lien du studio (LINK) ? Le studio s'en passe alors de loopMIDI
                self.send_text(json!({ "type": "live", "channel": p.channel, "answers": p.answers, "ms": p.ms, "error": p.error, "script": p.script, "live": p.live, "version": p.version, "link": p.link }).to_string())
            }
            Err(TryRecvError::Empty) => Ok(()),
            Err(TryRecvError::Disconnected) => {
                self.probe = None;
                Ok(())
            }
        }
    }

    fn on_editor(&mut self, path: &str, open: bool) -> Result<(), Fatal> {
        let kind = if open { "opened" } else { "closed" };
        if !live::path_ok(path) {
            return self.send_text(json!({ "type": kind, "ok": false, "path": path, "error": "bad device path" }).to_string());
        }
        if self.sh.demo {
            return self.send_text(json!({ "type": kind, "ok": true, "path": path, "window": if open { Some(DEMO_ID) } else { None }, "wasOpen": true }).to_string());
        }
        // (23 oct.) dans un fil a part : Live met ~0,4 s a ouvrir ou fermer un editeur (mesure sur Kontakt et Diva) ;
        // pendant ce temps les images de la fenetre montree et les gestes continuent (Follow track referme la fenetre
        // d'avant juste apres avoir montre la nouvelle : sa premiere image attendait la fermeture)
        if self.editors.load(Ordering::SeqCst) >= MAX_EDITOR_REQUESTS {
            return self.send_text(json!({ "type": kind, "ok": false, "path": path, "error": "too many Open/Close in flight" }).to_string());
        }
        self.editors.fetch_add(1, Ordering::SeqCst);
        // le titre de la fenetre, d'apres la liste des plugins que « devices » vient de lire (Follow track la lit juste
        // avant « open ») : un LIST de moins dans Live (~0,1 s)
        let known = self
            .last_devices
            .as_ref()
            .filter(|(at, _)| at.elapsed() < DEVICES_FRESH)
            .and_then(|(_, list)| list.iter().find(|d| d.path == path))
            .map(|d| plugins::expected_title(&d.name, &d.track));
        let (tx, busy, path) = (self.ed_tx.clone(), self.editors.clone(), path.to_string());
        std::thread::spawn(move || {
            let _ = tx.send(editor_request(&path, open, known));
            busy.fetch_sub(1, Ordering::SeqCst);
        });
        Ok(())
    }

    /// Les reponses d'« open » / « close » arrivees de leurs fils.
    fn poll_editors(&mut self) -> Result<(), Fatal> {
        while let Ok(s) = self.ed_rx.try_recv() {
            self.send_text(s)?;
        }
        Ok(())
    }

    fn on_watch(&mut self, id: u64, fps: Option<f64>, quality: Option<u8>) -> Result<(), Fatal> {
        let t0 = Instant::now();
        self.stop_watch(None)?;
        // seulement une fenetre de plugin de Live, jamais une autre fenetre de la machine
        let (_, wins) = self.windows();
        let Some(w) = wins.into_iter().find(|w| w.id == id) else {
            return self.send_text(json!({ "type": "unwatched", "id": id, "reason": "not a Live plugin window (closed?)" }).to_string());
        };
        let t_windows = t0.elapsed();
        let cap = if self.sh.demo {
            None
        } else {
            match capture::open_plugin(id, &w.plugin, self.sh.capture.as_deref()) {
                Ok(c) => Some(c),
                Err(e) => return self.send_text(json!({ "type": "unwatched", "id": id, "reason": format!("cannot capture: {e}") }).to_string()),
            }
        };
        let setup = format!("set up in {} ms: windows {} ms, capture {} ms", t0.elapsed().as_millis(), t_windows.as_millis(), (t0.elapsed() - t_windows).as_millis());
        let pid = if self.sh.demo { 0 } else { input::pid_of(id) };
        let fps = fps.filter(|f| f.is_finite()).unwrap_or(30.0).clamp(1.0, 60.0);
        let kind = cap.as_ref().map(|c| c.kind()).unwrap_or("demo");
        crate::log(&format!("watch \"{}\" ({}×{}) with {kind} at {fps:.0} fps (input: {}; {setup})", w.title, w.w, w.h, self.inj.mode.name()));
        self.send_text(
            json!({ "type": "watching", "id": id, "title": w.title, "plugin": w.plugin, "track": w.track, "w": w.w, "h": w.h, "capture": kind, "fps": fps, "mode": self.inj.mode.name() })
                .to_string(),
        )?;
        let (req_tx, req_rx) = mpsc::channel::<Req>();
        let (resp_tx, resp_rx) = mpsc::channel::<Resp>();
        let worker = Worker::new(id, w.title.clone(), cap).with_latest_input(self.latest_input.clone());
        std::thread::spawn(move || worker.run(req_rx, resp_tx));
        self.watch = Some(Watch {
            id,
            title: w.title,
            pid,
            tx: Some(req_tx),
            rx: resp_rx,
            busy: false,
            interval: Duration::from_secs_f64(1.0 / fps),
            next: Instant::now(),
            quality: quality.unwrap_or(encode::DEFAULT_QUALITY).clamp(30, 95),
            blurry: false,
            cheap: kind == "dwm",
            sent: 0,
        });
        self.inj.set_mode(self.inj.mode); // (rien en cours d'une fenetre a l'autre)
        self.pointer = None;
        self.acked = self.seq;
        Ok(())
    }

    fn stop_watch(&mut self, reason: Option<&str>) -> Result<(), Fatal> {
        if let Some(mut w) = self.watch.take() {
            w.tx = None; // le fil de capture s'arrete
            crate::log(&format!("unwatch \"{}\" ({} frames sent)", w.title, w.sent));
            if let Some(r) = reason {
                self.send_text(json!({ "type": "unwatched", "id": w.id, "reason": r }).to_string())?;
            }
        }
        Ok(())
    }

    fn gesture(&self) -> bool {
        self.inj.dragging() || self.last_input.map(|t| t.elapsed() < GESTURE_HOLD).unwrap_or(false)
    }

    /// Les images : la reponse du fil de capture s'il y en a une, puis la demande suivante si c'est l'heure et que le
    /// studio suit (au plus MAX_IN_FLIGHT images non affichees). Pendant un geste : sans attendre (60 images/s visees,
    /// la capture elle-meme donne la cadence), des tuiles autour du pointeur ; apres : une image nette.
    fn pump(&mut self) -> Result<(), Fatal> {
        let got = match self.watch.as_mut() {
            None => return Ok(()),
            Some(w) if w.busy => match w.rx.try_recv() {
                Ok(r) => {
                    w.busy = false;
                    w.blurry = r.blurry;
                    w.cheap = r.cheap;
                    Some(Ok(r))
                }
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => Some(Err("capture stopped".to_string())),
            },
            Some(_) => None,
        };
        match got {
            Some(Ok(Resp { seq, packet: Ok(Some(p)), .. })) => {
                self.seq = seq;
                if let Some(w) = self.watch.as_mut() {
                    w.sent += 1;
                }
                self.push(Message::binary(p))?;
            }
            Some(Ok(Resp { packet: Ok(None), .. })) => {}
            Some(Ok(Resp { packet: Err(e), .. })) | Some(Err(e)) => return self.stop_watch(Some(&e)),
            None => {}
        }
        if self.pending_flush || self.seq.saturating_sub(self.acked) >= MAX_IN_FLIGHT {
            return Ok(());
        }
        let gesture = self.gesture();
        let (seq, pointer, input_n) = (self.seq + 1, self.pointer, self.input_n);
        let Some(w) = self.watch.as_mut() else { return Ok(()) };
        let now = Instant::now();
        if now < w.next {
            return Ok(());
        }
        let interval = match (gesture, w.cheap) {
            (true, true) => GESTURE_POLL_CHEAP,
            (true, false) => Duration::from_secs_f64(1.0 / GESTURE_FPS),
            _ => w.interval,
        };
        // la cadence tient en moyenne (on avance d'un pas depuis l'echeance precedente, sans rattraper plus d'une
        // image de retard). Pendant un geste : 60 images/s, comptees depuis le DEBUT de la capture precedente, et
        // jamais une capture juste apres l'autre — mesure du 20 oct. : des PrintWindow enchaines sans pause
        // affament le WM_PAINT de la fenetre (le message le moins prioritaire) et l'image ne change qu'a la fin du
        // geste (~250 ms de retard)
        w.next = if gesture { now + interval } else { w.next + interval };
        if w.next + interval < now {
            w.next = now;
        }
        let sharp = !gesture && w.blurry;
        let req = Req { seq, gesture, pointer, input_n, quality: w.quality, sharp };
        if let Some(tx) = &w.tx {
            if tx.send(req).is_ok() {
                w.busy = true;
            }
        }
        Ok(())
    }

    // ── les gestes ───────────────────────────────────────────────────────────────────────────────────────────
    fn input_ack(&mut self, raw: &RawInput, recv: f64, inj: f64, merged: u64, err: Option<&str>) -> Result<(), Fatal> {
        let mut a = json!({ "type": "inputack", "n": raw.n, "t": raw.t, "recv": round3(recv), "inj": round3(inj), "ok": err.is_none(), "mode": self.inj.mode.name() });
        if merged > 0 {
            a["merged"] = json!(merged);
        }
        if let Some(e) = err {
            a["error"] = json!(e);
        }
        self.send_text(a.to_string())
    }

    fn on_input(&mut self, raw: RawInput, recv: f64, merged: u64) -> Result<(), Fatal> {
        let ev = match protocol::validate_input(&raw) {
            Ok(e) => e,
            Err(e) => {
                self.stats.refused += 1;
                return self.input_ack(&raw, recv, now_f(), merged, Some(&e));
            }
        };
        let Some(w) = self.watch.as_ref() else {
            self.stats.refused += 1;
            return self.input_ack(&raw, recv, now_f(), merged, Some("no plugin window is shown"));
        };
        if raw.id.map(|i| i != w.id).unwrap_or(false) {
            self.stats.refused += 1;
            return self.input_ack(&raw, recv, now_f(), merged, Some("not the window shown"));
        }
        let (top, pid) = (w.id, w.pid);
        let outcome = if self.sh.demo {
            // la fenetre de demonstration n'existe pas : rien n'est injecte, le reste se comporte pareil
            let p = match &ev {
                InputEvent::Pointer { pt, .. } | InputEvent::Wheel { pt, .. } => Some(input::map_point(pt, 640, 400)),
                _ => None,
            };
            Ok(input::Outcome { applied: true, pointer: p })
        } else {
            #[cfg(any(windows, target_os = "macos"))]
            {
                // toujours la meme fenetre de Live (meme processus) qu'a la verification de « watch »
                if input::pid_of(top) != pid {
                    let _ = self.input_ack(&raw, recv, now_f(), merged, Some("the plugin window is gone"));
                    return self.stop_watch(Some("the plugin window is gone"));
                }
            }
            let _ = pid;
            self.inj.handle(top, &ev)
        };
        let inj = now_f();
        if crate::stream::trace() {
            crate::log(&format!("trace input n {:?} {} injected at {:.1} (studio event at {:.1}) ({:?})", raw.n, ev.kind(), inj % 100_000.0, raw.t.unwrap_or(0.0) % 100_000.0, outcome.as_ref().map(|o| o.applied)));
        }
        let st = &mut self.stats;
        st.since.get_or_insert_with(Instant::now);
        st.count += 1;
        st.merged += merged;
        st.inject_ms += inj - recv;
        st.inject_max = st.inject_max.max(inj - recv);
        if let Some(t) = raw.t.filter(|t| t.is_finite() && *t > 0.0 && recv >= *t && recv - *t < 10_000.0) {
            st.transit_ms += recv - t;
            st.transit_n += 1;
        }
        self.last_input = Some(Instant::now());
        match outcome {
            Ok(o) => {
                if let Some(p) = o.pointer {
                    self.pointer = Some(p);
                }
                if o.applied {
                    self.input_n = raw.n.unwrap_or(self.input_n + 1);
                    self.latest_input.store(self.input_n, Ordering::SeqCst);
                    // une image au plus vite, en laissant au plugin le temps de lire le geste et de se redessiner
                    // (une capture immediate passerait avant le geste : les messages envoyes passent devant)
                    if let Some(w) = self.watch.as_mut() {
                        w.next = w.next.min(Instant::now() + if w.cheap { INPUT_TO_CAPTURE_CHEAP } else { INPUT_TO_CAPTURE });
                    }
                }
                self.log_inputs(false);
                self.input_ack(&raw, recv, inj, merged, None)
            }
            Err(e) => {
                self.stats.refused += 1;
                crate::log(&format!("input ({}) not delivered: {e}", ev.kind()));
                self.input_ack(&raw, recv, inj, merged, Some(&e))
            }
        }
    }

    fn log_inputs(&mut self, force: bool) {
        let Some(since) = self.stats.since else { return };
        if !force && since.elapsed() < Duration::from_secs(10) {
            return;
        }
        let s = &self.stats;
        if s.count > 0 {
            crate::log(&format!(
                "input ({}): {} events ({} moves merged, {} refused), received → injected avg {:.3} ms max {:.3} ms, studio → companion avg {:.2} ms",
                self.inj.mode.name(),
                s.count,
                s.merged,
                s.refused,
                s.inject_ms / s.count as f64,
                s.inject_max,
                if s.transit_n > 0 { s.transit_ms / s.transit_n as f64 } else { 0.0 }
            ));
        }
        self.stats = InputStats::default();
    }

    fn on_message(&mut self, msg: ClientMsg, recv: f64) -> Result<(), Fatal> {
        match msg {
            ClientMsg::Hello { .. } => self.send_text(protocol::error("already-paired", "hello was already accepted")),
            ClientMsg::List => self.on_list(),
            ClientMsg::Devices => self.on_devices(),
            ClientMsg::Live => self.on_live(),
            ClientMsg::Open { path } => self.on_editor(&path, true),
            ClientMsg::Close { path } => self.on_editor(&path, false),
            ClientMsg::Watch { id, fps, quality } => self.on_watch(id, fps, quality),
            ClientMsg::Unwatch => self.stop_watch(Some("unwatch")),
            ClientMsg::Ack { seq } => {
                if seq <= self.seq && seq > self.acked {
                    self.acked = seq;
                }
                Ok(())
            }
            ClientMsg::Ping { t } => self.send_text(json!({ "type": "pong", "t": t, "now": now_ms() }).to_string()),
            ClientMsg::Place { x, y, w, h, dpr } => {
                // prevu : aligner la vraie fenetre sous la fenetre flottante. Lu et note.
                self.places += 1;
                if self.places <= 3 {
                    crate::log(&format!("place {x:.0},{y:.0} {w:.0}×{h:.0} dpr {:?} (not used yet)", dpr));
                }
                Ok(())
            }
            ClientMsg::Mode { mode } => match Mode::parse(&mode) {
                Some(m) => {
                    if m != self.inj.mode {
                        crate::log(&format!("input mode: {}", m.name()));
                    }
                    self.inj.set_mode(m);
                    self.send_text(json!({ "type": "mode", "mode": m.name() }).to_string())
                }
                None => self.send_text(protocol::error("bad-mode", "mode is direct or focus")),
            },
            ClientMsg::Input(raw) => self.on_input(raw, recv, 0),
            ClientMsg::Audio { action, target, rate, format, ms } => {
                let r = crate::audio::feed::on_request(&mut self.audio, &action, target.as_deref(), rate, format.as_deref(), ms, self.sh.demo, crate::audio::test_tone());
                crate::tray::status::audio_reply(&r); // (24 oct.) la fenetre d'etat : une capture qui echoue
                self.send_text(r)
            }
            // (25 oct.) les plugins Uppwind Link du DAW (plugin_link)
            ClientMsg::Instances => self.send_text(crate::plugin_link::instances_json()),
            ClientMsg::Midi { to, events } => match crate::plugin_link::send_midi(&to, &events) {
                Ok(_) => Ok(()),
                Err(e) => self.send_text(protocol::error("midi", &e)),
            },
            ClientMsg::Emitted { id } => self.send_text(crate::plugin_link::emitted_json(&id)),
            // (25 oct., sans loopMIDI) le lien du studio par le canal local du script de Live
            ClientMsg::Link { sysex, stop } => {
                if stop == Some(true) {
                    if self.link.take().is_some() {
                        crate::log("link relay: stopped by the studio");
                    }
                    return self.send_text(json!({ "type": "link", "state": "off" }).to_string());
                }
                if self.link.is_none() {
                    match crate::link_relay::Relay::start(None) {
                        Ok(r) => {
                            crate::log("link relay: the studio talks to Live through the companion (no MIDI port)");
                            self.link = Some(r);
                        }
                        Err(e) => return self.send_text(protocol::error("link", &e)),
                    }
                }
                match self.link.as_ref().unwrap().send(sysex.unwrap_or_default()) {
                    Ok(()) => Ok(()),
                    Err(e) => self.send_text(protocol::error("link", &e)),
                }
            }
            ClientMsg::Routes => {
                // un fil par demande (jusqu'a 1,5 s si Live se tait) : comptes avec Open/Close, pour qu'une rafale ne
                // fasse pas pousser les fils sans fin
                if self.editors.load(Ordering::SeqCst) >= MAX_EDITOR_REQUESTS {
                    return self.send_text(json!({ "type": "routes", "ok": false, "error": "too many requests in flight" }).to_string());
                }
                self.editors.fetch_add(1, Ordering::SeqCst);
                let (tx, busy) = (self.ed_tx.clone(), self.editors.clone());
                std::thread::spawn(move || {
                    let s = match crate::live::routes() {
                        Ok(v) => v.to_string(),
                        Err(e) => json!({ "type": "routes", "ok": false, "error": e }).to_string(),
                    };
                    let _ = tx.send(s);
                    busy.fetch_sub(1, Ordering::SeqCst);
                });
                Ok(())
            }
        }
    }

    /// (25 oct.) Ce que le relais du lien a pour le studio.
    fn poll_link(&mut self) -> Result<(), Fatal> {
        let Some(r) = &self.link else { return Ok(()) };
        for o in r.poll() {
            self.send_text(crate::link_relay::studio_json(&o))?;
        }
        Ok(())
    }

    /// (24 oct.) Les paquets du retour audio arrives du fil de capture : envoyes tout de suite (binaire « UWA1 »).
    /// Le socket plein (le studio ne lit plus assez vite) : le paquet est jete plutot qu'empile ; au-dela de
    /// AUDIO_STALL_MAX, le flux s'arrete et le studio le sait.
    fn pump_audio(&mut self) -> Result<(), Fatal> {
        use crate::audio::feed::{follow_returns, Out};
        // (25 oct.) un plugin Uppwind Link Return s'est connecte (ou est parti) depuis la demande : le flux le suit, et le
        // studio l'apprend (« audio on » avec « switched » : sa latence, ses libelles)
        if self.audio.as_ref().map(|f| f.want.is_some()).unwrap_or(false) {
            let hub = crate::plugin_link::hub();
            if let Some(msg) = follow_returns(hub.as_deref(), &mut self.audio, false) {
                crate::tray::status::audio_reply(&msg);
                self.send_text(msg)?;
            }
        }
        loop {
            let Some(f) = self.audio.as_mut() else { return Ok(()) };
            match f.next() {
                Some(Out::Packet(p)) => {
                    if self.pending_flush {
                        f.dropped += 1;
                        let since = *f.stalled.get_or_insert_with(Instant::now);
                        if since.elapsed() > AUDIO_STALL_MAX {
                            crate::log(&format!("audio return stopped: the studio did not read it for {:.0} s ({} packets dropped)", since.elapsed().as_secs_f64(), f.dropped));
                            self.audio = None;
                            return self.send_text(json!({ "type": "audio", "state": "off", "error": "the studio stopped reading the audio" }).to_string());
                        }
                        continue;
                    }
                    f.stalled = None;
                    f.sent += 1;
                    self.push(Message::binary(p))?;
                }
                Some(Out::Ended(why)) => {
                    // (25 oct.) le Return d'une demande « auto » / « live » / « fl » est parti : la capture de Windows
                    // reprend (ou un autre Return de ce DAW), sans que le studio ait a redemander
                    if f.want.is_some() && f.source.is_some() {
                        let hub = crate::plugin_link::hub();
                        if let Some(msg) = follow_returns(hub.as_deref(), &mut self.audio, true) {
                            crate::tray::status::audio_reply(&msg);
                            self.send_text(msg)?;
                            if self.audio.is_some() {
                                continue;
                            }
                            return Ok(());
                        }
                    }
                    crate::log(&format!("audio return ended: {why}"));
                    self.audio = None;
                    let off = json!({ "type": "audio", "state": "off", "error": why }).to_string();
                    crate::tray::status::audio_reply(&off); // (24 oct.) la fenetre d'etat
                    return self.send_text(off);
                }
                None => return Ok(()),
            }
        }
    }
}

/// Au plus tant d'« open » / « close » en route a la fois (chacun dans son fil).
pub const MAX_EDITOR_REQUESTS: u64 = 4;

/// La liste des plugins lue par « devices » sert encore a « open » pendant ce temps.
pub const DEVICES_FRESH: Duration = Duration::from_secs(2);

/// « open » / « close » d'un editeur dans Live (par UppwindLinkDev) → la reponse pour le studio (opened / closed).
/// `known` : le titre attendu de la fenetre, s'il vient d'etre lu (sinon : LIST).
fn editor_request(path: &str, open: bool, known: Option<String>) -> String {
    let kind = if open { "opened" } else { "closed" };
    // le titre attendu de la fenetre (« device/piste ») : par LIST
    let t0 = Instant::now();
    let title = known.or_else(|| {
        live::request(json!({ "cmd": "LIST" }), Duration::from_millis(1500))
            .ok()
            .and_then(|rep| live::plugins_of_list(&rep).into_iter().find(|d| d.path == path))
            .map(|d| plugins::expected_title(&d.name, &d.track))
    });
    let t_list = t0.elapsed();
    let cmd = if open { "EDITOR_OPEN" } else { "EDITOR_CLOSE" };
    let rep = match live::request(json!({ "cmd": cmd, "path": path }), Duration::from_millis(2000)) {
        Ok(r) => r,
        Err(e) => return json!({ "type": kind, "ok": false, "path": path, "error": e }).to_string(),
    };
    crate::log(&format!(
        "{cmd} {path}{} (Live's LIST {} ms, {cmd} {} ms)",
        title.as_ref().map(|t| format!(" ({t})")).unwrap_or_default(),
        t_list.as_millis(),
        (t0.elapsed() - t_list).as_millis()
    ));
    if rep["ok"].as_bool() != Some(true) {
        return json!({ "type": kind, "ok": false, "path": path, "error": rep["error"].as_str().unwrap_or("Live refused") }).to_string();
    }
    let mut window = None;
    if open {
        if let Some(t) = &title {
            // la fenetre apparait en quelques dizaines de ms : on la guette 2 s au plus (moins d'1 s en pratique)
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_secs(2) {
                let wins = plugins::plugin_windows(&plugins::enumerate());
                if let Some(w) = wins.iter().find(|w| &w.title == t) {
                    window = Some(w.id);
                    crate::log(&format!("window of {path} found after {} ms", t0.elapsed().as_millis()));
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    // wasOpen : la fenetre l'etait-elle deja avant ? (Follow track ne referme que celles qu'il a ouvertes)
    json!({ "type": kind, "ok": true, "path": path, "window": window, "title": title, "wasOpen": live::was_open(&rep) }).to_string()
}

/// La place d'une connexion dans le compte de `serve` (rendue quand le fil se termine, meme sur une panique).
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// La boucle d'acceptation : un fil par connexion, au plus `max` a la fois, comptees des l'acceptation (avant la poignee
/// de main : une rafale de connexions a moitie ouvertes ne fait pas pousser les fils sans fin). Au-dela : fermee.
pub fn serve(listener: TcpListener, sh: Arc<Shared>, max: usize) {
    let active = Arc::new(AtomicUsize::new(0));
    let mut refused: u64 = 0;
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                if active.load(Ordering::SeqCst) >= max {
                    refused += 1;
                    if refused == 1 || refused % 100 == 0 {
                        crate::log(&format!("REFUSED a connection: {max} are already open ({refused} refused so far)"));
                    }
                    drop(s);
                    continue;
                }
                active.fetch_add(1, Ordering::SeqCst);
                let (sh, slot) = (sh.clone(), Slot(active.clone()));
                std::thread::spawn(move || {
                    let _slot = slot;
                    run(s, sh);
                });
            }
            Err(e) => crate::log(&format!("accept: {e}")),
        }
    }
}

/// Une connexion, du premier octet a la fermeture.
pub fn run(stream: TcpStream, sh: Arc<Shared>) {
    // les gestes partent tout de suite (pas d'algorithme de Nagle sur les petits messages)
    let _ = stream.set_nodelay(true);
    let Some((ws, origin)) = handshake(stream, &sh) else { return };
    let _ = ws.get_ref().set_nonblocking(true);
    let (ed_tx, ed_rx) = mpsc::channel();
    let mut c = Conn {
        ws,
        sh: &sh,
        pending_flush: false,
        seq: 0,
        acked: 0,
        watch: None,
        inj: Injector::default(),
        input_n: 0,
        latest_input: Arc::new(AtomicU64::new(0)),
        last_input: None,
        pointer: None,
        stats: InputStats::default(),
        places: 0,
        probe: None,
        ed_tx,
        ed_rx,
        editors: Arc::new(AtomicU64::new(0)),
        last_devices: None,
        audio: None,
        link: None,
    };
    crate::log(&format!("studio connected from {origin}: waiting for the pairing token"));

    // 1. l'appairage : le premier message, dans les 10 s
    let t0 = Instant::now();
    loop {
        if t0.elapsed() > HELLO_TIMEOUT {
            crate::log("no hello within 10 s: closed");
            c.close(CLOSE_PROTOCOL, "no hello");
            return;
        }
        match c.ws.read() {
            Ok(Message::Text(t)) => {
                let parsed = protocol::parse(t.as_str());
                let (version, token) = match parsed {
                    Ok(ClientMsg::Hello { version, token }) => (version, token),
                    _ => {
                        crate::log("REFUSED: the first message is not hello");
                        let _ = c.send_text(protocol::error("not-paired", "the first message must be hello with the pairing token"));
                        c.close(CLOSE_PROTOCOL, "hello first");
                        return;
                    }
                };
                let now = Instant::now();
                let ok = token.as_deref().map(|t| security::token_eq(t, &sh.token)).unwrap_or(false);
                let mut guard = sh.guard.lock().unwrap();
                if !ok {
                    // (audit de publication) trop d'essais faux : les suivants sont ralentis et refuses, mais le bon
                    // code passe toujours — sinon n'importe quel programme local bloquerait le vrai studio
                    let locked = guard.locked(now);
                    guard.failure(now);
                    drop(guard);
                    if locked {
                        std::thread::sleep(LOCKED_DELAY);
                        crate::log("REFUSED: wrong pairing token while pairing is slowed down (too many wrong tokens)");
                        let _ = c.send_text(protocol::error("locked", "too many wrong pairing codes: wait 30 s"));
                        c.close(CLOSE_LOCKED, "locked");
                    } else {
                        crate::log(&format!("REFUSED: {} pairing token from {origin}", if token.is_some() { "wrong" } else { "missing" }));
                        let _ = c.send_text(protocol::error("bad-token", "wrong pairing code: copy it again from the companion window"));
                        c.close(CLOSE_BAD_TOKEN, "bad token");
                    }
                    return;
                }
                guard.success();
                drop(guard);
                if version.unwrap_or(0) != PROTOCOL_VERSION {
                    crate::log(&format!("note: the studio speaks protocol {:?}, the companion {PROTOCOL_VERSION}", version));
                }
                let (live_on, _) = c.windows();
                let hello = json!({
                    "type": "hello", "version": PROTOCOL_VERSION, "companion": env!("CARGO_PKG_VERSION"),
                    "capture": if sh.demo { "demo" } else { sh.capture.as_deref().unwrap_or("wgc") },
                    "input": true, "modes": ["direct", "focus"], "tiles": true,
                    "live": live_on, "liveChannel": sh.demo || live::available(), "demo": sh.demo,
                    // (24 oct.) le retour audio sans cable : la capture d'un processus marche-t-elle sur ce Windows ?
                    "audio": sh.demo || crate::audio::available().is_ok(), "audioFormats": ["f32", "i16"],
                    "audioError": if sh.demo { None } else { crate::audio::available().err() },
                    // (25 oct.) les plugins Uppwind Link (Return, Notes) peuvent se connecter
                    "plugins": crate::plugin_link::available(),
                    // (25 oct.) le lien du studio par le canal local du script de Live (« link », « routes ») : sans loopMIDI
                    "linkTransport": true,
                });
                if c.send_text(hello.to_string()).is_err() {
                    return;
                }
                crate::log(&format!("PAIRED with {origin}"));
                crate::tray::status::paired(&origin); // (24 oct.) la fenetre d'etat
                break;
            }
            Ok(Message::Close(_)) => return,
            Ok(_) => {
                c.close(CLOSE_PROTOCOL, "hello first");
                return;
            }
            Err(WsError::Io(e)) if e.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => return,
        }
    }

    // 2. la boucle : tous les messages arrives (un mouvement de souris suivi d'un autre dans le meme lot est fondu :
    //    seul le dernier est injecte ; rien n'attend s'il est seul), puis les images
    let mut window_start = Instant::now();
    let mut count = 0u32;
    let mut bad = 0u32;
    'outer: loop {
        let mut pending_move: Option<(RawInput, f64, u64)> = None;
        loop {
            match c.ws.read() {
                Ok(Message::Text(t)) => {
                    let recv = now_f();
                    if window_start.elapsed() >= Duration::from_secs(1) {
                        window_start = Instant::now();
                        count = 0;
                    }
                    count += 1;
                    if count > MAX_MSGS_PER_SEC {
                        continue; // trop de messages : ignores
                    }
                    match protocol::parse(t.as_str()) {
                        Ok(ClientMsg::Input(raw)) if raw.kind == "pointer" && raw.action.as_deref() == Some("move") => {
                            let merged = pending_move.as_ref().map(|p| p.2 + 1).unwrap_or(0);
                            pending_move = Some((raw, recv, merged));
                        }
                        Ok(m) => {
                            if let Some((raw, r, k)) = pending_move.take() {
                                if c.on_input(raw, r, k).is_err() {
                                    break 'outer;
                                }
                            }
                            if c.on_message(m, recv).is_err() {
                                break 'outer;
                            }
                        }
                        Err(e) => {
                            bad += 1;
                            if c.send_text(protocol::error("bad-message", &e)).is_err() || bad > 50 {
                                c.close(CLOSE_PROTOCOL, "too many bad messages");
                                break 'outer;
                            }
                        }
                    }
                }
                Ok(Message::Close(_)) => break 'outer,
                Ok(Message::Binary(_)) => {
                    c.close(CLOSE_PROTOCOL, "no binary from the studio");
                    break 'outer;
                }
                Ok(_) => {}
                Err(WsError::Io(e)) if e.kind() == ErrorKind::WouldBlock => break,
                Err(WsError::ConnectionClosed) | Err(WsError::AlreadyClosed) => break 'outer,
                Err(_) => break 'outer,
            }
        }
        if let Some((raw, r, k)) = pending_move.take() {
            if c.on_input(raw, r, k).is_err() {
                break;
            }
        }
        if c.poll_probe().is_err() || c.poll_editors().is_err() || c.poll_link().is_err() || c.flush().is_err() || c.pump_audio().is_err() || c.pump().is_err() || c.flush().is_err() {
            break;
        }
        // 1 ms quand une fenetre est suivie ou que le son du DAW part (les gestes et les paquets de 5 ms partent au plus
        // 1 ms apres leur arrivee) ; (25 oct.) et quand un plugin Uppwind Link Notes attend des notes (mesure : sinon
        // une note jouee « tout de suite » attendait jusqu'a 10 ms ici)
        // ; (26 oct.) et quand le lien de Live passe par ici (link_relay : une commande, sa reponse, sans 2 x 10 ms)
        let busy = c.watch.is_some() || c.audio.is_some() || crate::plugin_link::notes_connected() || c.link.is_some();
        std::thread::sleep(Duration::from_millis(if busy { 1 } else { 10 }));
    }
    c.log_inputs(true);
    let _ = c.stop_watch(None);
    if let Some(f) = c.audio.take() {
        crate::log(&format!("audio return stopped with the connection ({}, {} packets sent)", f.label, f.sent));
    }
    crate::log(&format!("studio {origin} disconnected"));
    crate::tray::status::closed(&origin); // (24 oct.) la fenetre d'etat
}

/// De vraies connexions sur 127.0.0.1 (un port au hasard), le compagnon en mode demo : ce que le studio vivra.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::{Allowlist, PairingGuard};
    use std::net::TcpListener;
    use std::sync::Mutex;
    use tungstenite::handshake::client::generate_key;
    use tungstenite::http::Request as HttpRequest;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn server() -> u16 {
        server_mode(true)
    }

    fn server_mode(demo: bool) -> u16 {
        server_with(demo, 50, MAX_CONNECTIONS)
    }

    /// Le serveur du compagnon (la meme boucle que main.rs) : `failures` essais faux avant le blocage, `max` connexions.
    fn server_with(demo: bool, failures: u32, max: usize) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let sh = Arc::new(Shared {
            port,
            token: TOKEN.into(),
            allow: Allowlist::new(true, &[]),
            guard: Mutex::new(PairingGuard::new(failures, Duration::from_secs(30))),
            capture: None,
            demo,
            test_window: None,
        });
        std::thread::spawn(move || serve(l, sh, max));
        port
    }

    type Client = WebSocket<TcpStream>;

    fn connect(port: u16, host: &str, origin: Option<&str>) -> Result<Client, String> {
        let mut b = HttpRequest::builder()
            .uri(format!("ws://127.0.0.1:{port}/"))
            .header("Host", host)
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", generate_key());
        if let Some(o) = origin {
            b = b.header("Origin", o);
        }
        let req = b.body(()).unwrap();
        let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        tungstenite::client(req, s).map(|(ws, _)| ws).map_err(|e| e.to_string())
    }

    fn good(port: u16) -> Client {
        connect(port, &format!("127.0.0.1:{port}"), Some("http://localhost:5747")).expect("handshake")
    }

    fn text(ws: &mut Client) -> serde_json::Value {
        loop {
            match ws.read().expect("a message") {
                Message::Text(t) => return serde_json::from_str(t.as_str()).unwrap(),
                Message::Binary(_) => continue,
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    /// Le prochain message texte d'un type donne (les images et les autres messages passent).
    fn text_of(ws: &mut Client, ty: &str) -> serde_json::Value {
        loop {
            let v = text(ws);
            if v["type"] == ty {
                return v;
            }
        }
    }

    fn close_code(ws: &mut Client) -> Option<u16> {
        loop {
            match ws.read() {
                Ok(Message::Close(Some(f))) => return Some(u16::from(f.code)),
                Ok(Message::Close(None)) => return None,
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    }

    #[test]
    fn bad_host_is_refused() {
        let port = server();
        let e = connect(port, &format!("evil.example:{port}"), Some("http://localhost:5747")).unwrap_err();
        assert!(e.contains("403"), "{e}");
        let e = connect(port, "localhost:1", Some("http://localhost:5747")).unwrap_err();
        assert!(e.contains("403"), "{e}");
    }

    #[test]
    fn bad_or_missing_origin_is_refused() {
        let port = server();
        let host = format!("127.0.0.1:{port}");
        assert!(connect(port, &host, Some("https://evil.example")).unwrap_err().contains("403"));
        assert!(connect(port, &host, Some("http://localhost:8080")).unwrap_err().contains("403"));
        assert!(connect(port, &host, None).unwrap_err().contains("403"));
        assert!(connect(port, &host, Some("https://uppwind.lucasmoreau.fr")).is_ok());
    }

    #[test]
    fn wrong_or_missing_token_is_refused() {
        let port = server();
        let mut ws = good(port);
        ws.send(Message::text(json!({"type": "hello", "version": 1, "token": "f".repeat(64)}).to_string())).unwrap();
        assert_eq!(text(&mut ws)["code"], "bad-token");
        assert_eq!(close_code(&mut ws), Some(CLOSE_BAD_TOKEN));

        let mut ws = good(port);
        ws.send(Message::text(json!({"type": "hello", "version": 1}).to_string())).unwrap();
        assert_eq!(text(&mut ws)["code"], "bad-token");
        assert_eq!(close_code(&mut ws), Some(CLOSE_BAD_TOKEN));

        let mut ws = good(port);
        ws.send(Message::text(json!({"type": "list"}).to_string())).unwrap();
        assert_eq!(text(&mut ws)["code"], "not-paired", "nothing before hello");
        assert_eq!(close_code(&mut ws), Some(CLOSE_PROTOCOL));

        // un geste avant l'appairage : ferme, rien n'est injecte
        let mut ws = good(port);
        ws.send(Message::text(r#"{"type":"input","kind":"pointer","action":"down","x":5,"y":5}"#.to_string())).unwrap();
        assert_eq!(text(&mut ws)["code"], "not-paired");
        assert_eq!(close_code(&mut ws), Some(CLOSE_PROTOCOL));
    }

    /// (audit de publication) trop d'essais faux ralentissent les suivants, mais le bon code passe toujours : un programme
    /// local qui se trompe expres ne peut plus empecher le vrai studio de s'appairer.
    #[test]
    fn a_locked_pairing_still_takes_the_right_code() {
        let port = server_with(true, 2, MAX_CONNECTIONS);
        let wrong = json!({"type": "hello", "version": 1, "token": "f".repeat(64)}).to_string();
        for _ in 0..2 {
            let mut ws = good(port);
            ws.send(Message::text(wrong.clone())).unwrap();
            assert_eq!(text(&mut ws)["code"], "bad-token");
        }
        let mut ws = good(port);
        let t0 = Instant::now();
        ws.send(Message::text(wrong.clone())).unwrap();
        assert_eq!(text(&mut ws)["code"], "locked");
        assert!(t0.elapsed() >= LOCKED_DELAY - Duration::from_millis(50), "a wrong code while locked waits");
        assert_eq!(close_code(&mut ws), Some(CLOSE_LOCKED));
        let mut ws = good(port);
        ws.send(Message::text(json!({"type": "hello", "version": 1, "token": TOKEN}).to_string())).unwrap();
        assert_eq!(text(&mut ws)["type"], "hello", "the right code pairs even while wrong ones are slowed down");
    }

    /// (audit de publication) au plus `max` connexions a la fois, avant meme la poignee de main ; une place rendue sert
    /// a la suivante.
    #[test]
    fn connections_are_capped_and_slots_come_back() {
        let port = server_with(true, 50, 2);
        let pair = |port: u16| -> Client {
            let mut ws = good(port);
            ws.send(Message::text(json!({"type": "hello", "version": 1, "token": TOKEN}).to_string())).unwrap();
            assert_eq!(text(&mut ws)["type"], "hello");
            ws
        };
        let a = pair(port);
        let _b = pair(port);
        assert!(connect(port, &format!("127.0.0.1:{port}"), Some("http://localhost:5747")).is_err(), "a third connection is closed at once");
        drop(a);
        let t0 = Instant::now();
        let mut back = false;
        while t0.elapsed() < Duration::from_secs(3) {
            if connect(port, &format!("127.0.0.1:{port}"), Some("http://localhost:5747")).is_ok() {
                back = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(back, "the slot of the closed connection is free again");
    }

    #[test]
    fn paired_studio_lists_watches_and_gets_frames() {
        let port = server();
        let mut ws = good(port);
        ws.send(Message::text(json!({"type": "hello", "version": 1, "token": TOKEN}).to_string())).unwrap();
        let h = text(&mut ws);
        assert_eq!(h["type"], "hello");
        assert_eq!(h["input"], true);
        ws.send(Message::text(r#"{"type":"list"}"#.to_string())).unwrap();
        let l = text(&mut ws);
        assert_eq!(l["windows"][0]["title"], "Demo Synth/1-Demo");
        // Follow track : « open » dit si la fenetre l'etait deja (la demo l'est toujours) ; un mauvais chemin : refuse
        ws.send(Message::text(r#"{"type":"open","path":"0.0"}"#.to_string())).unwrap();
        let o = text_of(&mut ws, "opened");
        assert_eq!((o["ok"].as_bool(), o["window"].as_u64(), o["wasOpen"].as_bool()), (Some(true), Some(DEMO_ID), Some(true)));
        ws.send(Message::text(r#"{"type":"open","path":"x"}"#.to_string())).unwrap();
        assert_eq!(text_of(&mut ws, "opened")["ok"], false);
        // une fenetre qui n'est pas une fenetre de plugin : refusee
        ws.send(Message::text(r#"{"type":"watch","id":4242}"#.to_string())).unwrap();
        assert_eq!(text(&mut ws)["type"], "unwatched");
        ws.send(Message::text(json!({"type": "watch", "id": DEMO_ID, "fps": 60}).to_string())).unwrap();
        assert_eq!(text(&mut ws)["type"], "watching");
        let mut seqs = Vec::new();
        let mut tiles = 0;
        while seqs.len() < 4 {
            if let Message::Binary(b) = ws.read().unwrap() {
                let (head, jpeg) = protocol::read_packet(&b).expect("UWF1 packet");
                assert_eq!(head["sw"], 640);
                if head.get("x").is_some() {
                    tiles += 1;
                } else {
                    assert_eq!(head["w"], 640);
                }
                assert_eq!(&jpeg[..2], &[0xff, 0xd8]);
                let seq = head["seq"].as_u64().unwrap();
                seqs.push(seq);
                ws.send(Message::text(json!({"type": "ack", "seq": seq}).to_string())).unwrap();
            }
        }
        assert_eq!(seqs, vec![1, 2, 3, 4]);
        assert!(tiles >= 2, "after the first full image, only the changed rectangle travels");
        ws.send(Message::text(r#"{"type":"ping","t":1}"#.to_string())).unwrap();
        assert_eq!(text_of(&mut ws, "pong")["t"], 1.0);
        // « live » (23 oct.) : le script de dev repond-il maintenant ? (la demo : toujours) — pendant le flux d'images
        ws.send(Message::text(r#"{"type":"live"}"#.to_string())).unwrap();
        let l = text_of(&mut ws, "live");
        assert_eq!((l["channel"].as_bool(), l["answers"].as_bool()), (Some(true), Some(true)));
        ws.send(Message::text("nonsense".to_string())).unwrap();
        assert_eq!(text_of(&mut ws, "error")["code"], "bad-message");
    }

    #[test]
    fn inputs_are_acknowledged_measured_and_mark_the_frames() {
        let port = server();
        let mut ws = good(port);
        ws.send(Message::text(json!({"type": "hello", "version": 1, "token": TOKEN}).to_string())).unwrap();
        text(&mut ws);
        // sans fenetre suivie : refuse
        ws.send(Message::text(r#"{"type":"input","kind":"pointer","action":"down","x":5,"y":5,"n":1}"#.to_string())).unwrap();
        let a = text_of(&mut ws, "inputack");
        assert_eq!((a["ok"].as_bool(), a["error"].as_str()), (Some(false), Some("no plugin window is shown")));
        ws.send(Message::text(r#"{"type":"mode","mode":"focus"}"#.to_string())).unwrap();
        assert_eq!(text_of(&mut ws, "mode")["mode"], "focus");
        ws.send(Message::text(r#"{"type":"mode","mode":"direct"}"#.to_string())).unwrap();
        assert_eq!(text_of(&mut ws, "mode")["mode"], "direct");
        ws.send(Message::text(json!({"type": "watch", "id": DEMO_ID, "fps": 30}).to_string())).unwrap();
        assert_eq!(text_of(&mut ws, "watching")["mode"], "direct");
        let t = now_f();
        ws.send(Message::text(json!({"type": "input", "kind": "pointer", "action": "down", "x": 320, "y": 200, "iw": 640, "ih": 400, "buttons": 1, "n": 7, "t": t}).to_string())).unwrap();
        let a = text_of(&mut ws, "inputack");
        assert_eq!((a["n"].as_u64(), a["ok"].as_bool()), (Some(7), Some(true)));
        let (recv, inj) = (a["recv"].as_f64().unwrap(), a["inj"].as_f64().unwrap());
        assert!(recv >= t - 1.0 && inj >= recv && inj - recv < 5.0, "received {recv}, injected {inj}");
        // les mouvements arrives ensemble sont fondus (le dernier gagne), le reste passe dans l'ordre
        let mut batch = String::new();
        for i in 0..20 {
            batch = json!({"type": "input", "kind": "pointer", "action": "move", "x": 300 - i, "y": 200, "iw": 640, "ih": 400, "buttons": 1, "n": 8 + i}).to_string();
            ws.send(Message::text(batch.clone())).unwrap();
        }
        let _ = batch;
        ws.send(Message::text(json!({"type": "input", "kind": "pointer", "action": "up", "x": 281, "y": 200, "iw": 640, "ih": 400, "buttons": 0, "n": 28}).to_string())).unwrap();
        let mut acks = Vec::new();
        while acks.last().map(|a: &serde_json::Value| a["n"] != 28).unwrap_or(true) {
            acks.push(text_of(&mut ws, "inputack"));
        }
        assert!(acks.len() <= 21, "{} acks", acks.len());
        assert_eq!(acks[acks.len() - 2]["n"], 27, "the last move is never dropped");
        // une image porte le dernier geste injecte (« in ») et dit si elle a change pres du pointeur
        let mut marked = false;
        for _ in 0..40 {
            if let Message::Binary(b) = ws.read().unwrap() {
                let (head, _) = protocol::read_packet(&b).unwrap();
                ws.send(Message::text(json!({"type": "ack", "seq": head["seq"]}).to_string())).unwrap();
                if head["in"].as_u64() == Some(28) {
                    marked = true;
                    break;
                }
            }
        }
        assert!(marked, "a frame names the last input");
        // un geste qui vise une autre fenetre que celle suivie : refuse
        ws.send(Message::text(r#"{"type":"input","kind":"pointer","action":"down","x":5,"y":5,"id":999,"n":30}"#.to_string())).unwrap();
        assert_eq!(text_of(&mut ws, "inputack")["error"], "not the window shown");
        ws.send(Message::text(r#"{"type":"input","kind":"pointer","action":"down","x":-5,"y":5,"n":31}"#.to_string())).unwrap();
        assert_eq!(text_of(&mut ws, "inputack")["ok"], false);
    }

    /// (23 oct.) Live met ~0,4 s a fermer un editeur : la connexion n'attend pas (un faux script de dev, lent, sur un
    /// fichier d'etat de test — jamais le vrai ~/.uppwind-link-dev.json, jamais le vrai Live)
    #[test]
    fn a_slow_editor_close_in_live_does_not_block_the_connection() {
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let dev_port = sock.local_addr().unwrap().port();
        let dir = std::env::temp_dir().join(format!("uwc-editor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = dir.join(".uppwind-link-dev.json");
        std::fs::write(&state, json!({"port": dev_port, "token": "t", "script": "UppwindLink", "version": "1.3.0", "link": 1}).to_string()).unwrap();
        std::env::set_var(live::ENV_FILE, &state);
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                // (Windows : un « port injoignable » d'une reponse precedente remonte comme une erreur de recv_from ;
                // le faux script continue, comme le vrai)
                let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
                let req: serde_json::Value = serde_json::from_slice(&buf[..n]).unwrap();
                let id = req["id"].clone();
                let rep = match req["cmd"].as_str() {
                    Some("LIST") => json!({"id": id, "ok": true, "tracks": [{"index": 0, "name": "1-Test", "devices": [{"path": "0.0", "name": "Nope", "plugin": true, "editor": true}]}]}),
                    Some("EDITOR_CLOSE") => {
                        std::thread::sleep(Duration::from_millis(400));
                        json!({"id": id, "ok": true, "before": true, "after": false})
                    }
                    // (25 oct.) le lien du studio : le faux script renvoie chaque morceau recu (un echo)
                    Some("LINK") => json!({"id": id, "ok": true, "out": req["in"].clone(), "more": false}),
                    Some("ROUTES") => json!({"id": id, "ok": true, "tracks": [{"index": 0, "name": "1-Test", "input": "All Ins"}], "notesInBrowser": true}),
                    _ => json!({"id": id, "ok": true, "events": []}),
                };
                let _ = sock.send_to(rep.to_string().as_bytes(), from);
            }
        });
        let port = server_mode(false);
        let mut ws = good(port);
        ws.send(Message::text(json!({"type": "hello", "version": 1, "token": TOKEN}).to_string())).unwrap();
        assert_eq!(text_of(&mut ws, "hello")["liveChannel"], true);
        let t0 = Instant::now();
        ws.send(Message::text(r#"{"type":"close","path":"0.0"}"#.to_string())).unwrap();
        ws.send(Message::text(r#"{"type":"ping","t":5}"#.to_string())).unwrap();
        // le pong arrive pendant que Live « ferme » encore
        let first = text(&mut ws);
        assert_eq!((first["type"].as_str(), first["t"].as_f64()), (Some("pong"), Some(5.0)), "{first}");
        assert!(t0.elapsed() < Duration::from_millis(300), "pong after {:?}", t0.elapsed());
        let c = text_of(&mut ws, "closed");
        assert_eq!((c["ok"].as_bool(), c["path"].as_str(), c["wasOpen"].as_bool()), (Some(true), Some("0.0"), Some(true)));
        // « live » : le meme faux script repond
        ws.send(Message::text(r#"{"type":"live"}"#.to_string())).unwrap();
        let l = text_of(&mut ws, "live");
        assert_eq!((l["channel"].as_bool(), l["answers"].as_bool()), (Some(true), Some(true)), "{l}");
        // (25 oct.) le script sert le lien du studio : le studio peut se passer de loopMIDI
        assert_eq!((l["link"].as_bool(), l["version"].as_str()), (Some(true), Some("1.3.0")), "{l}");
        // le lien par le compagnon : « up », puis l'echo du faux script ; un morceau etranger est refuse ici
        let hello = "f07d555701010100017b7d7b7df7";
        ws.send(Message::text(json!({"type": "link", "sysex": [hello]}).to_string())).unwrap();
        assert_eq!(text_of(&mut ws, "link")["state"], "up");
        let back = text_of(&mut ws, "link");
        assert_eq!(back["sysex"], json!([hello]), "{back}");
        ws.send(Message::text(json!({"type": "link", "sysex": ["90407f"]}).to_string())).unwrap();
        assert_eq!(text_of(&mut ws, "error")["code"], "link");
        ws.send(Message::text(r#"{"type":"routes"}"#.to_string())).unwrap();
        let r = text_of(&mut ws, "routes");
        assert_eq!((r["ok"].as_bool(), r["tracks"][0]["name"].as_str()), (Some(true), Some("1-Test")), "{r}");
        ws.send(Message::text(r#"{"type":"link","stop":true}"#.to_string())).unwrap();
        assert_eq!(text_of(&mut ws, "link")["state"], "off");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_acks_at_most_two_frames_fly() {
        let port = server();
        let mut ws = good(port);
        ws.send(Message::text(json!({"type": "hello", "version": 1, "token": TOKEN}).to_string())).unwrap();
        text(&mut ws);
        ws.send(Message::text(json!({"type": "watch", "id": DEMO_ID, "fps": 60}).to_string())).unwrap();
        assert_eq!(text(&mut ws)["type"], "watching");
        ws.get_ref().set_read_timeout(Some(Duration::from_millis(600))).unwrap();
        let mut n = 0;
        while let Ok(m) = ws.read() {
            if let Message::Binary(_) = m {
                n += 1;
            }
        }
        assert_eq!(n, MAX_IN_FLIGHT, "the companion waits for the studio");
    }

    /// (24 oct.) Le retour audio sans cable, en mode demo (un sinus) : seulement apres l'appairage, des paquets « UWA1 »
    /// numerotes toutes les 5 ms, en meme temps que les images ; « stop » les arrete.
    #[test]
    fn audio_return_streams_after_pairing_only() {
        let port = server();
        // avant l'appairage : ferme, rien ne part
        let mut ws = good(port);
        ws.send(Message::text(r#"{"type":"audio","action":"start"}"#.to_string())).unwrap();
        assert_eq!(text(&mut ws)["code"], "not-paired");
        assert_eq!(close_code(&mut ws), Some(CLOSE_PROTOCOL));

        let mut ws = good(port);
        ws.send(Message::text(json!({"type": "hello", "version": 1, "token": TOKEN}).to_string())).unwrap();
        let h = text_of(&mut ws, "hello");
        assert_eq!((h["audio"].as_bool(), h["audioFormats"][0].as_str()), (Some(true), Some("f32")));
        ws.send(Message::text(json!({"type": "watch", "id": DEMO_ID, "fps": 30}).to_string())).unwrap();
        ws.send(Message::text(r#"{"type":"audio","action":"start","target":"live","rate":48000,"format":"f32","ms":5}"#.to_string())).unwrap();
        let a = text_of(&mut ws, "audio");
        assert_eq!((a["state"].as_str(), a["rate"].as_u64(), a["frames"].as_u64()), (Some("on"), Some(48000), Some(240)), "{a}");
        let t0 = Instant::now();
        let (mut audio, mut images) = (Vec::new(), 0);
        while audio.len() < 50 {
            match ws.read().unwrap() {
                Message::Binary(b) if b.starts_with(b"UWA1") => audio.push(crate::audio::packet::decode(&b).unwrap().0),
                Message::Binary(b) => {
                    images += 1;
                    let (head, _) = protocol::read_packet(&b).unwrap();
                    ws.send(Message::text(json!({"type": "ack", "seq": head["seq"]}).to_string())).unwrap();
                }
                _ => {}
            }
        }
        let el = t0.elapsed().as_secs_f64();
        assert!(images > 0, "the images still flow");
        assert!(el > 0.18 && el < 1.0, "50 packets of 5 ms in {el} s");
        assert!(audio.windows(2).all(|w| w[1].seq == w[0].seq + 1 && w[1].pos == w[0].pos + 240), "in order, none lost");
        // l'instant de capture suit l'horloge (meme machine que Date.now())
        let age = now_f() - audio.last().unwrap().t_ms;
        assert!(age > -1.0 && age < 100.0, "the last packet is {age} ms old");
        ws.send(Message::text(r#"{"type":"audio","action":"stop"}"#.to_string())).unwrap();
        assert_eq!(text_of(&mut ws, "audio")["state"], "off");
        // apres « stop » : plus de paquet audio (ceux deja partis passent)
        ws.get_ref().set_read_timeout(Some(Duration::from_millis(300))).unwrap();
        let mut late = 0;
        let t1 = Instant::now();
        while t1.elapsed() < Duration::from_millis(250) {
            match ws.read() {
                Ok(Message::Binary(b)) if b.starts_with(b"UWA1") => late += 1,
                Ok(Message::Binary(b)) => {
                    if let Some((head, _)) = protocol::read_packet(&b) {
                        let _ = ws.send(Message::text(json!({"type": "ack", "seq": head["seq"]}).to_string()));
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        assert!(late <= 3, "{late} audio packets after stop");
    }
}
