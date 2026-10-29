// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! uwl-host : un petit hote VST3 d'essai pour les plugins Uppwind Link — sans carte son, sans DAW, rien n'est joue.
//!
//! Il charge le vrai paquet .vst3, prepare le plugin comme un DAW, et l'appelle depuis un fil « audio » cadence par
//! l'horloge, comme le rappel d'une carte ASIO : un bloc toutes les N / frequence secondes, avec la gigue d'appel
//! qu'on choisit (le reveil du fil, les plugins calcules avant dans le graphe du DAW). Puis il mesure.
//!
//!   uwl-host scan   --bundle B
//!   uwl-host return --bundle B [--seconds 30] [--rate 48000] [--block 128] [--jitter MS] [--click-ms 500]
//!                   [--click-db -24] [--click-log FICHIER.jsonl]
//!       des clics dans le son (2 ms a 2 kHz) ; le journal dit pour chacun quand le DAW l'a calcule (« written ») :
//!       un banc cote studio peut les retrouver a la sortie du studio
//!   uwl-host notes  --bundle B --ws-port P --token T [--origin URL] [--seconds 20] [--rate 48000] [--block 128]
//!                   [--jitter MS] [--lead-ms 30] [--every-ms 25] [--tag N] [--out FICHIER.json]
//!       fait le studio (un client WebSocket appaire avec le compagnon) : envoie des notes a l'instance Notes de cet
//!       hote, chacune pour un instant donne, et regarde a quel echantillon elle sort : l'erreur, la gigue.
//!       (25 oct.) --tag N : regle le parametre « Link tag » (comme le Remote Script de Live), attend que le compagnon
//!       le voie (trame TAG) et envoie « vers tag:N », le chemin du studio sans loopMIDI
//!   uwl-host play --state-file F [--to ID] [--pattern scale|pulse] [--bpm 120] [--lead-ms 20] [--channel 1]
//!       l'essai dans un vrai DAW : une gamme (ou des doubles croches) vers un plugin Notes, par le compagnon dont
//!       on nomme le fichier d'etat ; dit si le plugin les a placees a l'heure
//!
//! Le plugin trouve le compagnon par son fichier d'etat. return, notes et play exigent --state-file F ou
//! UWL_STATE_FILE=F (le compagnon d'essai) et le passent au plugin charge ; scan lui donne un fichier qui n'existe pas.
//! Jamais le fichier par defaut (%USERPROFILE%\.uppwind-companion.json) sans qu'on le nomme.

mod vsthost;

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Write;
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use vsthost::{Block, Module};

fn epoch_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

fn arg(a: &[String], name: &str) -> Option<String> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned()
}

fn num(a: &[String], name: &str, def: f64) -> Result<f64, String> {
    match arg(a, name) {
        None => Ok(def),
        Some(v) => v.parse::<f64>().ok().filter(|x| x.is_finite()).ok_or(format!("bad {name}")),
    }
}

fn bundle_arg(a: &[String]) -> PathBuf {
    arg(a, "--bundle").map(PathBuf::from).unwrap_or_else(|| {
        let here = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_default();
        here.join("bundled").join("Uppwind Link.vst3")
    })
}

/// Un generateur fixe (les gigues d'appel sont les memes d'un essai a l'autre).
struct Lcg(u64);
impl Lcg {
    fn unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// L'heure de Windows d'un Instant (l'ecart pris une fois, au debut).
#[derive(Clone, Copy)]
struct Wall {
    i0: Instant,
    e0: f64,
}
impl Wall {
    fn now() -> Wall {
        Wall { i0: Instant::now(), e0: epoch_ms() }
    }
    fn of(&self, t: Instant) -> f64 {
        if t >= self.i0 {
            self.e0 + (t - self.i0).as_secs_f64() * 1000.0
        } else {
            self.e0 - (self.i0 - t).as_secs_f64() * 1000.0
        }
    }
}

/// Attendre l'instant du prochain rappel : dormir (la minuterie fine de Rust), puis ceder le processeur les dernieres
/// centaines de microsecondes (pas de boucle active : elle affamait les fils non temps reel des autres programmes de
/// la machine, un navigateur par exemple, dont le son avait alors des trous).
fn wait_until(target: Instant) {
    loop {
        let now = Instant::now();
        if now >= target {
            return;
        }
        let left = target - now;
        if left > Duration::from_micros(700) {
            std::thread::sleep(left - Duration::from_micros(500));
        } else {
            std::thread::yield_now();
        }
    }
}

#[cfg(windows)]
fn raise_priority() {
    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST};
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
    }
}
/// macOS (26 oct.) : la classe de service « interactive » (QOS_CLASS_USER_INTERACTIVE), le plus haut sans privilege ;
/// un vrai DAW utilise un fil temps reel (Mach, THREAD_TIME_CONSTRAINT_POLICY), ce que l'hote d'essai n'imite pas.
#[cfg(target_os = "macos")]
fn raise_priority() {
    extern "C" {
        fn pthread_set_qos_class_self_np(qos: u32, relpri: i32) -> i32;
    }
    const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;
    unsafe {
        pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
    }
}
#[cfg(not(any(windows, target_os = "macos")))]
fn raise_priority() {}

#[derive(Default)]
struct Stats {
    v: Vec<f64>,
}
impl Stats {
    fn push(&mut self, x: f64) {
        self.v.push(x);
    }
    fn summary(&self) -> Value {
        if self.v.is_empty() {
            return Value::Null;
        }
        let mut s = self.v.clone();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let n = s.len();
        let mean = s.iter().sum::<f64>() / n as f64;
        let sd = (s.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / n as f64).sqrt();
        let q = |p: f64| s[((n - 1) as f64 * p).round() as usize];
        let r = |x: f64| (x * 10000.0).round() / 10000.0;
        json!({ "n": n, "mean": r(mean), "sd": r(sd), "min": r(s[0]), "p50": r(q(0.5)), "p99": r(q(0.99)), "max": r(s[n - 1]) })
    }
}

fn scan(a: &[String]) -> Result<(), String> {
    let bundle = bundle_arg(a);
    let m = Module::load(&bundle)?;
    let mut out = Vec::new();
    for (_, cat, name) in m.classes() {
        if cat != "Audio Module Class" {
            continue;
        }
        let mut p = m.create(&name)?;
        let audio_in = name.contains("Return");
        p.start(48000.0, 512, audio_in)?;
        let (ins, outs) = p.audio_buses();
        let ev = p.event_outputs();
        // (25 oct.) les parametres que voit un DAW (Live les montre ; le Remote Script lit « Link tag » du Notes)
        let params: Vec<Value> = p.params().into_iter().map(|(id, title, steps)| json!({ "id": id, "title": title, "steps": steps })).collect();
        let mut b = Block::new(512, 48000.0);
        let mut ok = 0;
        for k in 0..100 {
            if b.process(&p, 512, audio_in, k * 512, 0) == vst3::Steinberg::kResultOk {
                ok += 1;
            }
        }
        p.stop();
        out.push(json!({ "name": name, "audioInputs": ins, "audioOutputs": outs, "eventOutputs": ev, "processOk": ok, "params": params }));
    }
    println!("{}", serde_json::to_string_pretty(&json!({ "bundle": bundle.display().to_string(), "module": m.path.display().to_string(), "classes": out })).unwrap());
    Ok(())
}

fn run_return(a: &[String]) -> Result<(), String> {
    let bundle = bundle_arg(a);
    let seconds = num(a, "--seconds", 30.0)?;
    let rate = num(a, "--rate", 48000.0)?;
    let block = num(a, "--block", 128.0)? as usize;
    let jitter = num(a, "--jitter", 0.0)?;
    let click_ms = num(a, "--click-ms", 500.0)?;
    let click_db = num(a, "--click-db", -24.0)?;
    let mut log = match arg(a, "--click-log") {
        Some(p) => Some(std::io::BufWriter::new(std::fs::File::create(&p).map_err(|e| format!("{p}: {e}"))?)),
        None => None,
    };
    let m = Module::load(&bundle)?;
    let mut p = m.create("Uppwind Link Return")?;
    p.start(rate, block as i32, true)?;
    let mut b = Block::new(block, rate);
    let amp = 10f64.powf(click_db / 20.0) as f32;
    let burst = (rate * 0.002) as u64;
    let every = (click_ms / 1000.0 * rate).round() as u64;
    raise_priority();
    let wall = Wall::now();
    let start = Instant::now() + Duration::from_millis(200);
    let blocks = (seconds * rate / block as f64) as u64;
    let mut rng = Lcg(11);
    let (mut late, mut cost) = (Stats::default(), Stats::default());
    let mut clicks = 0u64;
    println!("Uppwind Link Return: {rate} Hz, {block} frames ({:.2} ms), callback jitter up to {jitter} ms, {seconds} s", block as f64 * 1000.0 / rate);
    for k in 0..blocks {
        let pos = k * block as u64;
        let ideal = start + Duration::from_secs_f64(pos as f64 / rate);
        wait_until(ideal + Duration::from_secs_f64(rng.unit() * jitter / 1000.0));
        let call = Instant::now();
        for i in 0..block {
            let m = (pos + i as u64) % every;
            let v = if m < burst { amp * (2.0 * std::f64::consts::PI * 2000.0 * m as f64 / rate).sin() as f32 } else { 0.0 };
            b.inl[i] = v;
            b.inr[i] = v;
        }
        let first = pos.div_ceil(every) * every;
        if first < pos + block as u64 {
            if let Some(l) = log.as_mut() {
                let due = wall.of(start) + first as f64 * 1000.0 / rate;
                let written = wall.of(call) + (first - pos) as f64 * 1000.0 / rate;
                let _ = writeln!(l, "{}", json!({ "due": due, "written": written, "pos": first }));
                let _ = l.flush();
            }
            clicks += 1;
        }
        let r = b.process(&p, block, true, pos as i64, 0);
        if r != vst3::Steinberg::kResultOk {
            return Err(format!("process failed ({r}) at block {k}"));
        }
        late.push((call - ideal).as_secs_f64() * 1000.0);
        cost.push(call.elapsed().as_secs_f64() * 1e6);
    }
    p.stop();
    println!("{}", json!({ "plugin": "return", "blocks": blocks, "clicks": clicks, "callbackLateMs": late.summary(), "processUs": cost.summary() }));
    Ok(())
}

/// Ce que le « studio » de cet hote a envoye.
#[derive(Clone, Copy)]
struct Sent {
    pitch: u8,
    vel: u8,
    t: f64,
}

struct WsOut {
    instance: String,
    sent: Vec<Sent>,
    emitted: Vec<Value>,
    errors: Vec<String>,
    send_ms: Stats,
}

fn ws_read_json(ws: &mut tungstenite::WebSocket<TcpStream>) -> Result<Value, String> {
    loop {
        match ws.read() {
            Ok(tungstenite::Message::Text(t)) => return serde_json::from_str(t.as_str()).map_err(|e| e.to_string()),
            Ok(_) => continue,
            Err(e) => return Err(format!("websocket: {e}")),
        }
    }
}

fn ws_wait(ws: &mut tungstenite::WebSocket<TcpStream>, ty: &str, errors: &mut Vec<String>) -> Result<Value, String> {
    loop {
        let v = ws_read_json(ws)?;
        if v["type"] == ty {
            return Ok(v);
        }
        if v["type"] == "error" {
            errors.push(v.to_string());
        }
    }
}

/// (29 oct., v2) L'appairage comme le studio : le defi du compagnon, la preuve du studio, puis celle du compagnon,
/// verifiee avant tout le reste (uwl-proto, auth). Le jeton ne part jamais. → le hello du compagnon.
fn ws_pair(ws: &mut tungstenite::WebSocket<TcpStream>, port: u16, token: &str, errors: &mut Vec<String>) -> Result<Value, String> {
    use uwl_proto::{auth, parse_secret, secret_hex};
    let key = parse_secret(token).ok_or("the pairing token is not 64 hex digits")?;
    let ch = ws_wait(ws, "challenge", errors).map_err(|e| format!("no challenge from the companion ({e}): an older companion?"))?;
    let nc = ch["nonce"].as_str().and_then(parse_secret).ok_or("a challenge without a nonce")?;
    let mut ns = [0u8; 32];
    getrandom::fill(&mut ns).map_err(|e| format!("no system random source ({e})"))?;
    let proof = auth::proof(&key, auth::STUDIO, port, &nc, &ns);
    ws.send(tungstenite::Message::text(json!({ "type": "hello", "version": 1, "v": 2, "nonce": secret_hex(&ns), "proof": secret_hex(&proof) }).to_string())).map_err(|e| e.to_string())?;
    let h = ws_wait(ws, "hello", errors)?;
    let given = h["proof"].as_str().and_then(parse_secret).ok_or("the companion's hello carries no proof")?;
    if !auth::verify(&key, auth::COMPANION, port, &ns, &nc, &given) {
        return Err(format!("the program on port {port} did not prove it knows the pairing token"));
    }
    Ok(h)
}

/// Le studio de l'essai : appaire, trouve l'instance Notes de cet hote, envoie le motif.
fn ws_studio(port: u16, token: String, origin: String, lead: f64, every: f64, seconds: f64, tag: Option<u32>) -> Result<WsOut, String> {
    use tungstenite::client::IntoClientRequest;
    let mut req = format!("ws://127.0.0.1:{port}/").into_client_request().map_err(|e| e.to_string())?;
    req.headers_mut().insert("Origin", origin.parse().map_err(|_| "bad origin")?);
    let s = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("no companion on {port} ({e})"))?;
    let _ = s.set_nodelay(true);
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let (mut ws, _) = tungstenite::client(req, s).map_err(|e| format!("handshake: {e}"))?;
    let mut errors = Vec::new();
    let h = ws_pair(&mut ws, port, &token, &mut errors)?;
    if h["plugins"] != true {
        return Err(format!("this companion does not take plugins: {h}"));
    }
    // l'instance Notes de cet hote (meme pid)
    let me = std::process::id();
    let t0 = Instant::now();
    let instance = loop {
        ws.send(tungstenite::Message::text(r#"{"type":"instances"}"#.to_string())).map_err(|e| e.to_string())?;
        let v = ws_wait(&mut ws, "instances", &mut errors)?;
        // (25 oct.) --tag : et le numero que le plugin a annonce (trame TAG) apres le reglage de son parametre
        if let Some(i) = v["list"].as_array().and_then(|l| l.iter().find(|i| i["kind"] == "notes" && i["pid"] == me && tag.map(|t| i["tag"] == t).unwrap_or(true))) {
            break i["id"].as_str().unwrap_or_default().to_string();
        }
        if t0.elapsed() > Duration::from_secs(15) {
            return Err(format!("the Notes plugin of this host never connected{}: {v}", tag.map(|t| format!(" with Link tag {t}")).unwrap_or_default()));
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let to = tag.map(|t| format!("tag:{t}")).unwrap_or_else(|| instance.clone());
    let n = (seconds * 1000.0 / every) as usize;
    let t_start = epoch_ms() + 500.0;
    let mut sent = Vec::with_capacity(n);
    let mut send_ms = Stats::default();
    for k in 0..n {
        let t = t_start + k as f64 * every;
        let at = t - lead;
        let wait = at - epoch_ms();
        if wait > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(wait / 1000.0));
        }
        let pitch = 36 + (k % 64) as u8;
        let vel = 1 + ((k / 64) % 126) as u8;
        // « tout de suite » (lead 0) : l'instant est celui de l'envoi, comme une note jouee au clavier
        let t_on = if lead <= 0.0 { epoch_ms() } else { t };
        let msg = json!({ "type": "midi", "to": to, "events": [
            { "t": t_on, "d": [0x90, pitch, vel], "n": k },
            { "t": t_on + every * 0.5, "d": [0x80, pitch, 0], "n": 100_000 + k },
        ] });
        let s0 = Instant::now();
        ws.send(tungstenite::Message::text(msg.to_string())).map_err(|e| e.to_string())?;
        send_ms.push(s0.elapsed().as_secs_f64() * 1000.0);
        sent.push(Sent { pitch, vel, t: t_on });
    }
    std::thread::sleep(Duration::from_millis(800));
    ws.send(tungstenite::Message::text(json!({ "type": "emitted", "id": instance }).to_string())).map_err(|e| e.to_string())?;
    let e = ws_wait(&mut ws, "emitted", &mut errors)?;
    let emitted = e["list"].as_array().cloned().unwrap_or_default();
    let _ = ws.close(None);
    Ok(WsOut { instance, sent, emitted, errors, send_ms })
}

fn run_notes(a: &[String]) -> Result<(), String> {
    use vst3::Steinberg::Vst::Event_::EventTypes_;
    let bundle = bundle_arg(a);
    let seconds = num(a, "--seconds", 20.0)?;
    let rate = num(a, "--rate", 48000.0)?;
    let block = num(a, "--block", 128.0)? as usize;
    let jitter = num(a, "--jitter", 0.0)?;
    let lead = num(a, "--lead-ms", 30.0)?;
    let every = num(a, "--every-ms", 25.0)?;
    // le compagnon d'essai : son port et son jeton dans le meme fichier d'etat que le plugin lit (UWL_STATE_FILE, que
    // main() a fixe depuis --state-file ou l'environnement)
    let state: Value = explicit_state_file(a)
        .ok()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let port = match arg(a, "--ws-port") {
        Some(p) => p.parse::<u16>().map_err(|_| "bad --ws-port")?,
        None => state["port"].as_u64().ok_or("--ws-port, or UWL_STATE_FILE with the test companion's port")? as u16,
    };
    if port == 47410 {
        return Err("47410 is the default companion's port (the user's own): use a test companion (47411-47419)".into());
    }
    let token = arg(a, "--token").or_else(|| state["token"].as_str().map(|s| s.to_string())).ok_or("--token, or UWL_STATE_FILE with the test companion's token")?;
    let origin = arg(a, "--origin").unwrap_or_else(|| "http://localhost:5747".into());
    let m = Module::load(&bundle)?;
    let mut p = m.create("Uppwind Link Notes")?;
    p.start(rate, block as i32, false)?;
    if p.event_outputs() < 1 {
        return Err("the Notes plugin has no event output bus".into());
    }
    // (25 oct.) --tag N : regle « Link tag » comme le Remote Script de Live, et le studio vise « tag:N » (pas l'identite)
    // (un DAW fait les deux : le controleur, et le changement passe a process(), que nice-plug lit pendant le traitement)
    let mut tag_change = None;
    let tag = match arg(a, "--tag") {
        Some(t) => {
            let n: u32 = t.parse().ok().filter(|n| (1..=255).contains(n)).ok_or("--tag is 1..255")?;
            let (id, _, steps) = p.params().into_iter().find(|(_, title, _)| title == "Link tag").ok_or("the Notes plugin has no Link tag parameter")?;
            let v = n as f64 / steps.max(1) as f64;
            p.set_param(id, v)?;
            tag_change = Some((id, v));
            Some(n)
        }
        None => None,
    };
    if a.iter().any(|x| x == "--no-studio") {
        return host_only(&p, seconds, rate, block);
    }
    let studio = std::thread::spawn(move || ws_studio(port, token, origin, lead, every, seconds, tag));
    let mut b = Block::new(block, rate);
    if let Some((id, v)) = tag_change {
        b.queue_param(id, v);
    }
    raise_priority();
    let wall = Wall::now();
    let start = Instant::now() + Duration::from_millis(100);
    let total = seconds + 6.0;
    let blocks = (total * rate / block as f64) as u64;
    let mut rng = Lcg(5);
    let mut late = Stats::default();
    // (position absolue, hauteur, velocite MIDI) de chaque note on sortie ; les autres evenements comptes
    let mut ons: Vec<(u64, u8, u8)> = Vec::with_capacity(4096);
    let (mut offs, mut others) = (0u64, 0u64);
    println!("Uppwind Link Notes: {rate} Hz, {block} frames ({:.2} ms), callback jitter up to {jitter} ms, lead {lead} ms, a note every {every} ms", block as f64 * 1000.0 / rate);
    for k in 0..blocks {
        let pos = k * block as u64;
        let ideal = start + Duration::from_secs_f64(pos as f64 / rate);
        wait_until(ideal + Duration::from_secs_f64(rng.unit() * jitter / 1000.0));
        let call = Instant::now();
        let r = b.process(&p, block, false, pos as i64, 0);
        if r != vst3::Steinberg::kResultOk {
            return Err(format!("process failed ({r}) at block {k}"));
        }
        late.push((call - ideal).as_secs_f64() * 1000.0);
        for e in b.out_events.events.lock().unwrap().iter() {
            match e.r#type as u32 {
                t if t == EventTypes_::kNoteOnEvent as u32 => {
                    let on = unsafe { e.__field0.noteOn };
                    ons.push((pos + e.sampleOffset as u64, on.pitch as u8, (on.velocity * 127.0).round() as u8));
                }
                t if t == EventTypes_::kNoteOffEvent as u32 => offs += 1,
                _ => others += 1,
            }
        }
        if studio.is_finished() && k as f64 * block as f64 / rate > 2.0 {
            // le studio a fini : encore 1 s de blocs, puis on s'arrete
            let left = (rate / block as f64) as u64;
            for j in 1..=left {
                let pos = (k + j) * block as u64;
                wait_until(start + Duration::from_secs_f64(pos as f64 / rate));
                b.process(&p, block, false, pos as i64, 0);
            }
            break;
        }
    }
    p.stop();
    let ws = studio.join().map_err(|_| "the studio thread panicked")??;
    // l'erreur de chaque note : l'instant (horloge ideale de cette « carte ») de l'echantillon ou elle est sortie,
    // moins l'instant demande par le studio
    let mut by_key: HashMap<(u8, u8), u64> = HashMap::new();
    for (s, pitch, vel) in &ons {
        by_key.entry((*pitch, *vel)).or_insert(*s);
    }
    let t_of = |s: u64| wall.of(start) + s as f64 * 1000.0 / rate;
    let mut err = Stats::default();
    let mut missing = 0;
    for s in &ws.sent {
        match by_key.get(&(s.pitch, s.vel)) {
            Some(pos) => err.push(t_of(*pos) - s.t),
            None => missing += 1,
        }
    }
    let e = err.summary();
    let mean = e["mean"].as_f64().unwrap_or(0.0);
    let mut jit = Stats::default();
    for x in &err.v {
        jit.push((x - mean).abs() * rate / 1000.0);
    }
    let plugin_late = ws.emitted.iter().filter(|x| x["late"].as_f64().unwrap_or(0.0) > 0.0 && x["n"].as_u64().unwrap_or(0) < 100_000).count();
    let r = json!({
        "plugin": "notes", "rate": rate, "block": block, "blockMs": block as f64 * 1000.0 / rate, "jitterMs": jitter, "leadMs": lead, "everyMs": every,
        "instance": ws.instance, "sent": ws.sent.len(), "noteOnsOut": ons.len(), "noteOffsOut": offs, "otherEventsOut": others, "missing": missing,
        "errorMs": e, "jitterSamples": jit.summary(),
        "pluginReportedLate": plugin_late, "pluginReports": ws.emitted.len(),
        "callbackLateMs": late.summary(), "wsSendMs": ws.send_ms.summary(), "companionErrors": ws.errors,
    });
    println!("{}", serde_json::to_string_pretty(&r).unwrap());
    if let Some(f) = arg(a, "--out") {
        std::fs::write(&f, serde_json::to_string_pretty(&r).unwrap()).map_err(|e| format!("{f}: {e}"))?;
    }
    Ok(())
}

/// notes --no-studio : l'hote seul (le plugin Notes en temps reel pendant --seconds) ; il dit ce qui est sorti. Pour
/// essayer `play` sans DAW.
fn host_only(p: &vsthost::Plugin, seconds: f64, rate: f64, block: usize) -> Result<(), String> {
    use vst3::Steinberg::Vst::Event_::EventTypes_;
    let mut b = Block::new(block, rate);
    raise_priority();
    let start = Instant::now() + Duration::from_millis(100);
    let blocks = (seconds * rate / block as f64) as u64;
    let (mut ons, mut offs, mut others) = (Vec::new(), 0u64, 0u64);
    println!("hosting Uppwind Link Notes for {seconds} s ({rate} Hz, {block} frames)…");
    for k in 0..blocks {
        let pos = k * block as u64;
        wait_until(start + Duration::from_secs_f64(pos as f64 / rate));
        b.process(p, block, false, pos as i64, 0);
        for e in b.out_events.events.lock().unwrap().iter() {
            match e.r#type as u32 {
                t if t == EventTypes_::kNoteOnEvent as u32 => ons.push(unsafe { e.__field0.noteOn }.pitch),
                t if t == EventTypes_::kNoteOffEvent as u32 => offs += 1,
                _ => others += 1,
            }
        }
    }
    println!("{}", json!({ "noteOns": ons.len(), "pitches": ons, "noteOffs": offs, "others": others }));
    Ok(())
}

/// play : un motif d'essai vers un plugin Uppwind Link Notes, par le compagnon nomme (son fichier d'etat : --state-file,
/// sinon UWL_STATE_FILE ; jamais celui par defaut sans le nommer). Pour ecouter dans un vrai DAW, l'utilisateur le
/// lance lui-meme avec le fichier de son compagnon (le banc, lui, ne vise jamais ce compagnon-la).
///   uwl-host play [--to ID] [--pattern scale|pulse] [--bpm 120] [--lead-ms 20] [--channel 1] [--origin URL]
fn run_play(a: &[String]) -> Result<(), String> {
    use tungstenite::client::IntoClientRequest;
    let path = explicit_state_file(a)?;
    let st: Value = serde_json::from_str(&std::fs::read_to_string(&path).map_err(|e| format!("{}: {e} (is the companion running?)", path.display()))?).map_err(|e| e.to_string())?;
    let port = st["port"].as_u64().ok_or("no port in the state file")? as u16;
    let token = st["token"].as_str().ok_or("no token in the state file")?.to_string();
    if st["pluginPort"].is_null() {
        return Err("this companion does not take plugins: start the new companion build first".into());
    }
    let origin = arg(a, "--origin").unwrap_or_else(|| "https://uppwind.lucasmoreau.fr".into());
    let mut req = format!("ws://127.0.0.1:{port}/").into_client_request().map_err(|e| e.to_string())?;
    req.headers_mut().insert("Origin", origin.parse().map_err(|_| "bad origin")?);
    let s = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("no companion on {port} ({e})"))?;
    let _ = s.set_nodelay(true);
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let (mut ws, _) = tungstenite::client(req, s).map_err(|e| format!("handshake: {e}"))?;
    let mut errors = Vec::new();
    let h = ws_pair(&mut ws, port, &token, &mut errors)?;
    if h["plugins"] != true {
        return Err("this companion does not take plugins: start the new companion build first".into());
    }
    ws.send(tungstenite::Message::text(r#"{"type":"instances"}"#.to_string())).map_err(|e| e.to_string())?;
    let list = ws_wait(&mut ws, "instances", &mut errors)?;
    let notes: Vec<&Value> = list["list"].as_array().map(|l| l.iter().filter(|i| i["kind"] == "notes").collect()).unwrap_or_default();
    println!("Uppwind Link plugins connected to the companion (port {port}):");
    for i in list["list"].as_array().into_iter().flatten() {
        println!("  {}  {:<7} {:<28} {} (pid {}), {} Hz, block {}", i["id"].as_str().unwrap_or(""), i["kind"].as_str().unwrap_or(""), i["name"].as_str().unwrap_or(""), i["host"].as_str().unwrap_or(""), i["pid"], i["rate"], i["block"]);
    }
    let to = match arg(a, "--to") {
        Some(t) => t,
        None => notes.last().and_then(|i| i["id"].as_str()).ok_or("no Uppwind Link Notes is connected (put it on a MIDI track of the DAW)")?.to_string(),
    };
    let bpm = num(a, "--bpm", 120.0)?;
    let lead = num(a, "--lead-ms", 20.0)?;
    let ch = (num(a, "--channel", 1.0)? as u8).clamp(1, 16) - 1;
    let pattern = arg(a, "--pattern").unwrap_or_else(|| "scale".into());
    // (instant en temps, note, velocite, duree) en battements
    let mut evs: Vec<(f64, u8, u8, f64)> = Vec::new();
    match pattern.as_str() {
        "scale" => {
            let up = [60u8, 62, 64, 65, 67, 69, 71, 72];
            for (k, n) in up.iter().chain(up.iter().rev().skip(1)).enumerate() {
                evs.push((k as f64, *n, 100, 0.8));
            }
            let at = evs.len() as f64;
            for n in [60u8, 64, 67, 72] {
                evs.push((at, n, 90, 3.0));
            }
        }
        "pulse" => {
            for k in 0..64 {
                evs.push((k as f64 * 0.25, if k % 4 == 0 { 72 } else { 60 }, if k % 4 == 0 { 120 } else { 80 }, 0.1));
            }
        }
        // une note tenue 6 temps : le pitch bend monte, descend, revient ; la molette (CC 1) monte puis redescend
        // (en VST3 les CC et le pitch bend sortent en « LegacyMIDICCOutEvent » : le DAW les prend-il ?)
        "bend" => evs.push((0.0, 60, 100, 6.0)),
        _ => return Err("--pattern is scale, pulse or bend".into()),
    }
    let beat = 60_000.0 / bpm;
    let t0 = epoch_ms() + 300.0;
    let mut out: Vec<(f64, Value)> = Vec::new();
    for (k, (b, n, v, d)) in evs.iter().enumerate() {
        out.push((t0 + b * beat, json!({ "t": t0 + b * beat, "d": [0x90 | ch, n, v], "n": k })));
        out.push((t0 + (b + d) * beat, json!({ "t": t0 + (b + d) * beat, "d": [0x80 | ch, n, 0], "n": 100_000 + k })));
    }
    if pattern == "bend" {
        for i in 0..=96 {
            let b = 1.0 + i as f64 / 24.0; // de 1 a 5 temps
            let x = (i as f64 / 96.0 * 2.0 * std::f64::consts::PI).sin();
            let v = (8192.0 + x * 8191.0).round().clamp(0.0, 16383.0) as u16;
            let t = t0 + b * beat;
            out.push((t, json!({ "t": t, "d": [0xe0 | ch, v & 0x7f, v >> 7], "n": 200_000 + i })));
            let cc = (if i <= 48 { i as f64 / 48.0 } else { (96 - i) as f64 / 48.0 } * 127.0).round() as u8;
            out.push((t, json!({ "t": t, "d": [0xb0 | ch, 1, cc], "n": 300_000 + i })));
        }
    }
    out.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
    println!("Playing \"{pattern}\" at {bpm} BPM on {to}, channel {} ({} events, {lead} ms ahead)…", ch + 1, out.len());
    for (t, e) in &out {
        let wait = t - lead - epoch_ms();
        if wait > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(wait / 1000.0));
        }
        ws.send(tungstenite::Message::text(json!({ "type": "midi", "to": to, "events": [e] }).to_string())).map_err(|e| e.to_string())?;
    }
    std::thread::sleep(Duration::from_millis(600));
    ws.send(tungstenite::Message::text(json!({ "type": "emitted", "id": to }).to_string())).map_err(|e| e.to_string())?;
    let em = ws_wait(&mut ws, "emitted", &mut errors)?;
    let list = em["list"].as_array().cloned().unwrap_or_default();
    let late: Vec<f64> = list.iter().filter_map(|x| x["late"].as_f64()).filter(|l| *l > 0.0).collect();
    println!(
        "The plugin placed {} of {} events{}.",
        list.len(),
        out.len(),
        if late.is_empty() { ", all on time (at their sample)".to_string() } else { format!(", {} late (max {:.2} ms)", late.len(), late.iter().cloned().fold(0.0, f64::max)) }
    );
    ws.send(tungstenite::Message::text(r#"{"type":"instances"}"#.to_string())).map_err(|e| e.to_string())?;
    if let Ok(v) = ws_wait(&mut ws, "instances", &mut errors) {
        if let Some(i) = v["list"].as_array().and_then(|l| l.iter().find(|i| i["id"] == to.as_str())) {
            println!("Plugin stats: {}", i["stats"]);
        }
    }
    for e in &errors {
        println!("companion error: {e}");
    }
    let _ = ws.close(None);
    Ok(())
}

/// Le fichier d'etat que l'hote d'essai ET le plugin qu'il charge liront : --state-file, sinon UWL_STATE_FILE, rien
/// d'autre. Jamais celui du compagnon de l'utilisateur par defaut (%USERPROFILE%\.uppwind-companion.json) : le plugin
/// charge ici se connecterait sinon au vrai compagnon de la session. Pour viser ce compagnon-la (play, dans un vrai
/// DAW), on le nomme : --state-file "%USERPROFILE%\.uppwind-companion.json".
fn explicit_state_file(a: &[String]) -> Result<PathBuf, String> {
    let p = arg(a, "--state-file").map(PathBuf::from).or_else(|| std::env::var_os("UWL_STATE_FILE").filter(|v| !v.is_empty()).map(PathBuf::from));
    p.ok_or_else(|| "uwl-host needs the test companion's state file: --state-file FILE or UWL_STATE_FILE=FILE (it never reads the default one)".into())
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    // le plugin charge dans ce processus lit UWL_STATE_FILE : qu'il lise le meme fichier que l'hote, ou un fichier
    // qui n'existe pas (scan, ou aucune commande), jamais le fichier par defaut
    match a.first().map(|s| s.as_str()) {
        Some("return") | Some("notes") | Some("play") => match explicit_state_file(&a) {
            Ok(p) => std::env::set_var("UWL_STATE_FILE", p),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(2);
            }
        },
        _ => std::env::set_var("UWL_STATE_FILE", std::env::temp_dir().join("uwl-host-no-companion.json")),
    }
    let r = match a.first().map(|s| s.as_str()) {
        Some("scan") => scan(&a),
        Some("return") => run_return(&a),
        Some("notes") => run_notes(&a),
        Some("play") => run_play(&a),
        _ => {
            println!("uwl-host scan|return|notes --bundle \"Uppwind Link.vst3\" … (see the source header)");
            Ok(())
        }
    };
    if let Err(e) = r {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
