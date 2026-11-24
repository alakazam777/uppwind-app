// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (19 nov., etape 1) Le studio de l'essai pour l'hote VST3 du compagnon (companion/src/host.rs), de bout en bout :
//!
//!   uwl-host hoststudio --state-file F --plugin NOM [--seconds 12] [--every-ms 700] [--lead-ms 60] [--show]
//!       appaire avec le compagnon d'essai (jamais celui de l'utilisateur, 47410) ; « host status » jusqu'a la fin du
//!       scan ; « host plugins » → la classe dont le nom contient NOM ; « host add » ; le son de son Return (« audio
//!       start plugin:<id> ») ; des notes vers « tag:<n> » ; leur attaque dans le son ; « host save » (l'etat) ; un second
//!       « host add » avec cet etat ; « host remove » des deux. Dit chaque etape et ses temps.

use crate::{arg, epoch_ms, num, ws_pair, Stats};
use serde_json::{json, Value};
use std::net::TcpStream;
use std::time::{Duration, Instant};
use tungstenite::{Message, WebSocket};

type Ws = WebSocket<TcpStream>;

fn read_until(ws: &mut Ws, pred: impl Fn(&Value) -> bool, timeout: Duration, audio: &mut Vec<(f64, f32, f64)>) -> Result<Value, String> {
    let t0 = Instant::now();
    while t0.elapsed() < timeout {
        match ws.read() {
            Ok(Message::Text(t)) => {
                let v: Value = serde_json::from_str(t.as_str()).unwrap_or(Value::Null);
                if pred(&v) {
                    return Ok(v);
                }
                if v["type"] == "error" {
                    println!("  companion error: {v}");
                }
            }
            Ok(Message::Binary(b)) if b.len() >= 36 && &b[..4] == b"UWA1" => {
                let arrived = epoch_ms();
                let t_ms = f64::from_le_bytes(b[16..24].try_into().unwrap());
                let rate = u32::from_le_bytes([b[24], b[25], b[26], b[27]]) as f64;
                let nfr = u16::from_le_bytes([b[28], b[29]]) as usize;
                let (ch, fmt) = (b[30] as usize, b[31]);
                let body = &b[36..];
                for i in 0..nfr {
                    let mut x = 0f32;
                    for c in 0..ch {
                        let j = i * ch + c;
                        if fmt == 1 {
                            if let Some(s) = body.get(4 * j..4 * j + 4) {
                                x = x.max(f32::from_le_bytes(s.try_into().unwrap()).abs());
                            }
                        }
                    }
                    audio.push((t_ms + i as f64 * 1000.0 / rate, x, arrived));
                }
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(format!("websocket: {e}")),
        }
    }
    Err("timeout".into())
}

fn host(ws: &mut Ws, v: Value) -> Result<(), String> {
    let mut m = v;
    m["type"] = json!("host");
    ws.send(Message::text(m.to_string())).map_err(|e| e.to_string())
}

fn is_host(action: &'static str) -> impl Fn(&Value) -> bool {
    move |v: &Value| v["type"] == "host" && v["action"] == action
}

pub fn run(a: &[String]) -> Result<(), String> {
    use tungstenite::client::IntoClientRequest;
    let path = crate::explicit_state_file(a)?;
    let st: Value = serde_json::from_str(&std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?).map_err(|e| e.to_string())?;
    let port = st["port"].as_u64().ok_or("no port in the state file")? as u16;
    if port == 47410 {
        return Err("47410 is the user's companion: use a test companion".into());
    }
    let token = st["token"].as_str().ok_or("no token")?.to_string();
    let want = arg(a, "--plugin").unwrap_or_default().to_lowercase();
    let seconds = num(a, "--seconds", 12.0)?;
    let every = num(a, "--every-ms", 700.0)?;
    let lead = num(a, "--lead-ms", 60.0)?;
    let show = a.iter().any(|x| x == "--show");
    // (19 nov.) --keep : le plugin reste ouvert a la fin (les essais de plantage) ; --list-only : appaire, dit la liste
    let keep = a.iter().any(|x| x == "--keep");
    let list_only = a.iter().any(|x| x == "--list-only");
    let origin = arg(a, "--origin").unwrap_or_else(|| "http://localhost:5747".into());
    let mut req = format!("ws://127.0.0.1:{port}/").into_client_request().map_err(|e| e.to_string())?;
    req.headers_mut().insert("Origin", origin.parse().map_err(|_| "bad origin")?);
    let s = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("no companion on {port} ({e})"))?;
    let _ = s.set_nodelay(true);
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let (mut ws, _) = tungstenite::client(req, s).map_err(|e| format!("handshake: {e}"))?;
    let mut errors = Vec::new();
    let h = ws_pair(&mut ws, port, &token, &mut errors)?;
    println!("paired: host = {}, armed = {}", h["host"], h["armed"]);
    ws.get_mut().set_read_timeout(Some(Duration::from_millis(5))).map_err(|e| e.to_string())?;
    let mut audio = Vec::new();
    // (19 nov., etape 3) --render NOM [--signal impulse|sine] [--secs 2] : un effet heberge applique a un son, hors temps reel
    if let Some(name) = arg(a, "--render") {
        let sig = arg(a, "--signal").unwrap_or_else(|| "impulse".into());
        let secs = num(a, "--secs", 2.0)?;
        let r = render_test(&mut ws, &name, &sig, secs);
        let _ = ws.close(None);
        return r;
    }
    // (20 nov., Link to controller) --params NOM : un instrument heberge, la liste de ses parametres (pages), un parametre
    // pose (param-set) puis relu, et le suivi (param-watch, param-changes)
    if let Some(name) = arg(a, "--params") {
        let r = params_test(&mut ws, &name);
        let _ = ws.close(None);
        return r;
    }
    // (24 nov., 0.4.15) --unsaved NOM --phase open|back|reopen [--id N] [--p P] [--state SHA] : un reglage que le studio
    // n'a pas enregistre survit-il au dechargement ? open : ouvert, enregistre, un parametre change, parti sans enregistrer
    // (le plugin reste) ; back : la liste, l'instance --id encore la ?, enregistree, le parametre --p relu ; reopen : rouvert
    // avec --state, le parametre --p relu, puis retire
    if let Some(name) = arg(a, "--unsaved") {
        let r = unsaved_test(&mut ws, &name, &arg(a, "--phase").unwrap_or_default(), arg(a, "--id").and_then(|x| x.parse().ok()), arg(a, "--p").and_then(|x| x.parse().ok()), arg(a, "--state"));
        let _ = ws.close(None);
        return r;
    }
    // (19 nov., etape 3b) --fxlive NOM [--secs 60] : un effet heberge en direct, le son date aller-retour
    if let Some(name) = arg(a, "--fxlive") {
        let secs = num(a, "--secs", 60.0)?;
        let block = num(a, "--block", 256.0)? as usize;
        let r = fxlive_test(&mut ws, &name, secs, block.clamp(16, 512));
        let _ = ws.close(None);
        return r;
    }
    // (20 nov., l'hote macOS) --watch NOM [--secs 6] [--out DOSSIER] [--click X,Y] : l'editeur ouvert (« view »), filme par
    // le compagnon (« watch », la vraie capture), des notes pendant qu'on compte les images ; puis un clic au point X,Y de
    // l'image (fractions 0..1) et le temps jusqu'a l'image suivante. Les images (premiere, derniere, apres le clic) dans --out.
    if let Some(name) = arg(a, "--watch") {
        let secs = num(a, "--secs", 6.0)?;
        let out = arg(a, "--out").map(std::path::PathBuf::from);
        let pair = |k: &str| arg(a, k).and_then(|c| { let v: Vec<f64> = c.split(',').filter_map(|x| x.parse().ok()).collect(); (v.len() == 2).then(|| (v[0], v[1])) });
        let click = pair("--click");
        // (20 nov.) --menu X,Y : un clic droit au point (un menu contextuel), puis ce que le studio en fait (menu_steps)
        let menu = pair("--menu");
        // (24 nov., 0.4.15) --hold SECS : a la fin, le menu laisse ouvert et le plugin garde, SECS secondes (l'essai de
        // l'enfant orphelin pendant un menu : on tue le compagnon entre-temps)
        let hold = num(a, "--hold", 0.0)?;
        let r = watch_test(&mut ws, &name, secs, out, click, menu, hold);
        let _ = ws.close(None);
        return r;
    }
    // (19 nov., etape 2) --multi NOM1,NOM2 : deux plugins heberges, un flux chacun, la meme note au meme instant
    if let Some(m) = arg(a, "--multi") {
        let r = multi(&mut ws, &m, seconds, every, lead);
        let _ = ws.close(None);
        return r;
    }
    if list_only {
        host(&mut ws, json!({ "action": "list" }))?;
        let l = read_until(&mut ws, |v| v["type"] == "host" && (v["action"] == "list" || v["error"].is_string()), Duration::from_secs(5), &mut audio)?;
        println!("list: {l}");
        let _ = ws.close(None);
        return Ok(());
    }

    // ── le scan ──
    let t0 = Instant::now();
    loop {
        host(&mut ws, json!({ "action": "status" }))?;
        let v = read_until(&mut ws, is_host("status"), Duration::from_secs(10), &mut audio)?;
        if v["error"].is_string() {
            return Err(format!("host status: {v}"));
        }
        if v["scanning"] != true && v["total"].as_u64().unwrap_or(0) > 0 && v["done"] == v["total"] {
            println!("scan: {} modules in {:.1} s", v["total"], t0.elapsed().as_secs_f64());
            break;
        }
        if t0.elapsed() > Duration::from_secs(900) {
            return Err("the scan takes more than 15 min".into());
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
    host(&mut ws, json!({ "action": "plugins" }))?;
    let p = read_until(&mut ws, is_host("plugins"), Duration::from_secs(10), &mut audio)?;
    let list = p["list"].as_array().cloned().unwrap_or_default();
    let inst = list.iter().filter(|x| x["kind"] == "instrument").count();
    println!("plugins: {} ({} instruments, {} effects, {} modules failed)", list.len(), inst, list.iter().filter(|x| x["kind"] == "effect").count(), p["failed"]);
    let pick = list.iter().find(|x| x["kind"] == "instrument" && x["name"].as_str().unwrap_or("").to_lowercase().contains(&want)).ok_or(format!("no instrument named like \"{want}\""))?.clone();
    println!("picked: {} ({})", pick["name"], pick["vendor"]);

    // ── add ──
    let t = Instant::now();
    host(&mut ws, json!({ "action": "add", "plugin": pick["id"], "n": 1 }))?;
    let added = read_until(&mut ws, |v| v["type"] == "host" && v["action"] == "add" || v["action"] == "added", Duration::from_secs(70), &mut audio)?;
    if added["error"].is_string() {
        return Err(format!("add: {added}"));
    }
    let item = added["item"].clone();
    println!("added in {:.0} ms: {item}", t.elapsed().as_secs_f64() * 1000.0);
    let (id, tag, pid) = (item["id"].as_u64().unwrap(), item["tag"].as_u64().unwrap(), item["pid"].as_u64().unwrap());

    // ── le Return de cet enfant ──
    let t = Instant::now();
    let ret = loop {
        ws.send(Message::text(r#"{"type":"instances"}"#.to_string())).map_err(|e| e.to_string())?;
        let v = read_until(&mut ws, |v| v["type"] == "instances", Duration::from_secs(5), &mut audio)?;
        if let Some(r) = v["list"].as_array().and_then(|l| l.iter().find(|i| i["kind"] == "return" && i["pid"] == pid)) {
            break r["id"].as_str().unwrap_or("").to_string();
        }
        if t.elapsed() > Duration::from_secs(10) {
            return Err(format!("the hosted plugin's Return never connected: {v}"));
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    ws.send(Message::text(json!({ "type": "audio", "action": "start", "target": format!("plugin:{ret}"), "rate": 48000, "format": "f32", "ms": 5 }).to_string())).map_err(|e| e.to_string())?;
    if show {
        host(&mut ws, json!({ "action": "editor", "id": id, "show": true }))?;
    }

    // ── des notes vers tag:<n> ──
    let n_notes = (seconds * 1000.0 / every) as usize;
    let t_first = epoch_ms() + 800.0;
    let mut sent = Vec::new();
    let mut next = 0;
    let end = Instant::now() + Duration::from_secs_f64(seconds + 2.0);
    while Instant::now() < end {
        if next < n_notes && epoch_ms() >= t_first + next as f64 * every - lead {
            let t = t_first + next as f64 * every;
            let pitch = [48, 55, 60, 64, 67, 72][next % 6];
            let m = json!({ "type": "midi", "to": format!("tag:{tag}"), "events": [{ "t": t, "d": [0x90, pitch, 100], "n": next }, { "t": t + 250.0, "d": [0x80, pitch, 0], "n": 100_000 + next }] });
            ws.send(Message::text(m.to_string())).map_err(|e| e.to_string())?;
            sent.push(t);
            next += 1;
        }
        let _ = read_until(&mut ws, |_| false, Duration::from_millis(2), &mut audio);
    }
    let _ = ws.send(Message::text(r#"{"type":"audio","action":"stop"}"#.to_string()));
    let mut onset = Stats::default();
    let mut heard = 0;
    let mut peak = 0f32;
    for &t in &sent {
        let base = audio.iter().filter(|f| f.0 >= t - 60.0 && f.0 < t - 15.0).map(|f| f.1).fold(0f32, f32::max);
        let thr = (base * 3.0).max(0.00316);
        if let Some(f) = audio.iter().find(|f| f.0 >= t - 15.0 && f.0 < t + 300.0 && f.1 > thr) {
            heard += 1;
            onset.push(f.0 - t);
        }
    }
    for f in &audio {
        peak = peak.max(f.1);
    }
    println!("notes: {} sent, {} heard, onset {} ; peak {:.1} dBFS ; {} frames", sent.len(), heard, onset.summary(), 20.0 * (peak.max(1e-9) as f64).log10(), audio.len());

    // ── l'etat ──
    let t = Instant::now();
    host(&mut ws, json!({ "action": "save", "id": id, "n": 2 }))?;
    let saved = read_until(&mut ws, is_host("saved"), Duration::from_secs(20), &mut audio).or_else(|_| read_until(&mut ws, is_host("save"), Duration::from_secs(1), &mut audio))?;
    println!("saved in {:.0} ms: {saved}", t.elapsed().as_secs_f64() * 1000.0);
    let sha = saved["state"].as_str().unwrap_or("").to_string();
    let t = Instant::now();
    host(&mut ws, json!({ "action": "add", "plugin": pick["id"], "state": sha, "n": 3 }))?;
    let again = read_until(&mut ws, |v| v["type"] == "host" && (v["action"] == "added" || v["action"] == "add"), Duration::from_secs(70), &mut audio)?;
    println!("added again with the state in {:.0} ms: {}", t.elapsed().as_secs_f64() * 1000.0, again["item"]);
    // un etat inconnu : le plugin s'ouvre quand meme (reglages par defaut), avec un avis
    host(&mut ws, json!({ "action": "list" }))?;
    let l = read_until(&mut ws, is_host("list"), Duration::from_secs(5), &mut audio)?;
    println!("list: {} items", l["items"].as_array().map(|a| a.len()).unwrap_or(0));
    if keep {
        println!("kept open (--keep)");
        let _ = ws.close(None);
        return Ok(());
    }
    for it in l["items"].as_array().cloned().unwrap_or_default() {
        host(&mut ws, json!({ "action": "remove", "id": it["id"] }))?;
        let r = read_until(&mut ws, is_host("removed"), Duration::from_secs(10), &mut audio)?;
        println!("removed {}", r["id"]);
    }
    let _ = ws.close(None);
    Ok(())
}


/// (19 nov., etape 2) Deux plugins heberges (ou plus), chacun son flux (audio stream k, octet 33 des paquets) : la meme
/// note, au meme instant, vers chacun ; l'attaque dans chaque flux, et l'ecart entre les flux.
fn multi(ws: &mut Ws, names: &str, seconds: f64, every: f64, lead: f64) -> Result<(), String> {
    let mut audio = Vec::new();
    // le scan fini
    loop {
        host(ws, json!({ "action": "status" }))?;
        let v = read_until(ws, is_host("status"), Duration::from_secs(10), &mut audio)?;
        if v["scanning"] != true && v["total"].as_u64().unwrap_or(0) > 0 && v["done"] == v["total"] {
            break;
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
    host(ws, json!({ "action": "plugins" }))?;
    let p = read_until(ws, is_host("plugins"), Duration::from_secs(10), &mut audio)?;
    let list = p["list"].as_array().cloned().unwrap_or_default();
    let mut items = Vec::new();
    for (k, name) in names.split(',').map(|s| s.trim().to_lowercase()).enumerate() {
        let pick = list.iter().find(|x| x["kind"] == "instrument" && x["name"].as_str().unwrap_or("").to_lowercase().contains(&name)).ok_or(format!("no instrument like {name}"))?;
        host(ws, json!({ "action": "add", "plugin": pick["id"], "n": 10 + k }))?;
        let added = read_until(ws, |v| v["type"] == "host" && (v["action"] == "added" || v["action"] == "add"), Duration::from_secs(70), &mut audio)?;
        if added["error"].is_string() {
            return Err(format!("add {name}: {added}"));
        }
        let it = added["item"].clone();
        println!("added {} (id {}, tag {})", it["name"], it["id"], it["tag"]);
        items.push(it);
    }
    // un flux par plugin
    for (k, it) in items.iter().enumerate() {
        let st = (k + 1) as u8;
        ws.send(Message::text(json!({ "type": "audio", "action": "start", "target": format!("host:{}", it["id"]), "rate": 48000, "format": "f32", "ms": 5, "stream": st }).to_string())).map_err(|e| e.to_string())?;
    }
    // la meme note, au meme instant, vers chacun
    let mut frames: Vec<Vec<(f64, f32, f64)>> = vec![Vec::new(); items.len() + 1];
    let n_notes = (seconds * 1000.0 / every) as usize;
    let t_first = epoch_ms() + 2000.0;
    let mut sent = Vec::new();
    let mut next = 0;
    let end = Instant::now() + Duration::from_secs_f64(seconds + 3.0);
    while Instant::now() < end {
        if next < n_notes && epoch_ms() >= t_first + next as f64 * every - lead {
            let t = t_first + next as f64 * every;
            for it in &items {
                let m = json!({ "type": "midi", "to": format!("tag:{}", it["tag"]), "events": [{ "t": t, "d": [0x90, 60, 100] }, { "t": t + 200.0, "d": [0x80, 60, 0] }] });
                ws.send(Message::text(m.to_string())).map_err(|e| e.to_string())?;
            }
            sent.push(t);
            next += 1;
        }
        match ws.read() {
            Ok(Message::Binary(b)) if b.len() >= 36 && &b[..4] == b"UWA1" => {
                let arrived = epoch_ms();
                let k = b[33] as usize;
                if k >= frames.len() {
                    continue;
                }
                let t_ms = f64::from_le_bytes(b[16..24].try_into().unwrap());
                let rate = u32::from_le_bytes([b[24], b[25], b[26], b[27]]) as f64;
                let nfr = u16::from_le_bytes([b[28], b[29]]) as usize;
                let ch = b[30] as usize;
                let body = &b[36..];
                for i in 0..nfr {
                    let mut x = 0f32;
                    for c in 0..ch {
                        let j = i * ch + c;
                        if let Some(s) = body.get(4 * j..4 * j + 4) {
                            x = x.max(f32::from_le_bytes(s.try_into().unwrap()).abs());
                        }
                    }
                    frames[k].push((t_ms + i as f64 * 1000.0 / rate, x, arrived));
                }
            }
            Ok(Message::Text(t)) => {
                let v: Value = serde_json::from_str(t.as_str()).unwrap_or(Value::Null);
                if v["type"] == "audio" && v["stream"].is_number() {
                    println!("stream {}: {}", v["stream"], v["state"]);
                }
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(format!("websocket: {e}")),
        }
    }
    for k in 1..=items.len() {
        let _ = ws.send(Message::text(json!({ "type": "audio", "action": "stop", "stream": k }).to_string()));
    }
    // l'attaque de chaque note dans chaque flux (la premiere trame au-dessus de -50 dBFS et de 3 fois le niveau d'avant)
    let onset = |fr: &Vec<(f64, f32, f64)>, t: f64| -> Option<(f64, f64)> {
        let base = fr.iter().filter(|f| f.0 >= t - 60.0 && f.0 < t - 15.0).map(|f| f.1).fold(0f32, f32::max);
        let thr = (base * 3.0).max(0.00316);
        fr.iter().find(|f| f.0 >= t - 15.0 && f.0 < t + 250.0 && f.1 > thr).map(|f| (f.0 - t, f.2 - t))
    };
    let mut gaps = Stats::default();
    let mut per: Vec<Stats> = (0..=items.len()).map(|_| Stats::default()).collect();
    let mut heard = vec![0usize; items.len() + 1];
    for &t in &sent {
        let mut on = Vec::new();
        for k in 1..=items.len() {
            if let Some((o, arr)) = onset(&frames[k], t) {
                heard[k] += 1;
                per[k].push(arr);
                on.push(o);
            }
        }
        if on.len() == items.len() && on.len() >= 2 {
            let lo = on.iter().cloned().fold(f64::INFINITY, f64::min);
            let hi = on.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            gaps.push(hi - lo);
        }
    }
    for k in 1..=items.len() {
        println!("stream {k} ({}): {} frames, {}/{} notes heard, arrival here after the requested time: {}", items[k - 1]["name"], frames[k].len(), heard[k], sent.len(), per[k].summary());
    }
    println!("gap between the streams' onsets (same note, same time): {}", gaps.summary());
    println!("main stream (0): {} frames", frames[0].len());
    for it in &items {
        host(ws, json!({ "action": "remove", "id": it["id"] }))?;
        let _ = read_until(ws, is_host("removed"), Duration::from_secs(10), &mut audio);
    }
    Ok(())
}


/// (19 nov., etape 3) Le rendu d'un effet par le compagnon : le son (une impulsion, ou un sinus de 440 Hz) a 44,1 kHz
/// part en morceaux « UWJ1 », revient en « UWJ2 » ; on compare : longueur, latence annoncee, queue, energie apres la fin
/// de l'entree, et l'ecart echantillon par echantillon avec l'entree (un effet transparent : ~0).
fn params_test(ws: &mut Ws, name: &str) -> Result<(), String> {
    let mut audio = Vec::new();
    loop {
        host(ws, json!({ "action": "status" }))?;
        let v = read_until(ws, is_host("status"), Duration::from_secs(10), &mut audio)?;
        if v["scanning"] != true && v["total"].as_u64().unwrap_or(0) > 0 && v["done"] == v["total"] {
            break;
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
    host(ws, json!({ "action": "plugins" }))?;
    let p = read_until(ws, is_host("plugins"), Duration::from_secs(10), &mut audio)?;
    let list = p["list"].as_array().cloned().unwrap_or_default();
    let want = name.to_lowercase();
    let pick = list.iter().find(|x| x["name"].as_str().unwrap_or("").to_lowercase().contains(&want)).ok_or(format!("no plugin like {name}"))?.clone();
    host(ws, json!({ "action": "add", "plugin": pick["id"], "n": 1 }))?;
    let added = read_until(ws, |v| v["type"] == "host" && (v["action"] == "added" || v["action"] == "add"), Duration::from_secs(70), &mut audio)?;
    if added["error"].is_string() {
        return Err(format!("add: {added}"));
    }
    let id = added["item"]["id"].as_u64().unwrap();
    let t = Instant::now();
    let mut all: Vec<Value> = Vec::new();
    let mut start = 0u64;
    let mut pages = 0;
    loop {
        host(ws, json!({ "action": "params", "id": id, "start": start, "n": 10 + pages }))?;
        let r = read_until(ws, |v| v["type"] == "host" && v["action"] == "params", Duration::from_secs(15), &mut audio)?;
        if r["error"].is_string() {
            return Err(format!("params: {r}"));
        }
        pages += 1;
        all.extend(r["items"].as_array().cloned().unwrap_or_default());
        match r["next"].as_u64() {
            Some(n) if n > start && pages < 80 => start = n,
            _ => break,
        }
    }
    println!("params: {} in {} pages, {:.0} ms ; first: {}", all.len(), pages, t.elapsed().as_secs_f64() * 1000.0,
        all.iter().take(6).map(|x| format!("{}={:.3} ({})", x["p"], x["v"].as_f64().unwrap_or(-1.0), x["name"].as_str().unwrap_or(""))).collect::<Vec<_>>().join(", "));
    let target = all.iter().find(|x| x["name"].as_str().unwrap_or("").to_lowercase().contains("cutoff")).or(all.first()).cloned().ok_or("no parameter")?;
    let p0 = target["p"].as_u64().unwrap();
    let v0 = target["v"].as_f64().unwrap_or(0.0);
    let v1 = if v0 > 0.5 { 0.2 } else { 0.8 };
    host(ws, json!({ "action": "param-watch", "id": id, "on": true, "n": 3 }))?;
    let w = read_until(ws, |v| v["type"] == "host" && v["action"] == "param-watch", Duration::from_secs(5), &mut audio)?;
    println!("watch: {w}");
    host(ws, json!({ "action": "param-set", "id": id, "p": p0, "v": v1 }))?;
    std::thread::sleep(Duration::from_millis(400));
    host(ws, json!({ "action": "params", "id": id, "start": p0, "n": 4 }))?;
    let r = read_until(ws, |v| v["type"] == "host" && v["action"] == "params", Duration::from_secs(10), &mut audio)?;
    let back = r["items"].as_array().and_then(|a| a.iter().find(|x| x["p"].as_u64() == Some(p0)).cloned()).unwrap_or(Value::Null);
    println!("set {} ({}) {:.3} -> {:.3} ; read back {}", p0, target["name"], v0, v1, back["v"]);
    host(ws, json!({ "action": "param-changes", "id": id, "n": 5 }))?;
    let c = read_until(ws, |v| v["type"] == "host" && v["action"] == "param-changes", Duration::from_secs(5), &mut audio)?;
    println!("changes (no editor gesture expected): {}", c["items"]);
    host(ws, json!({ "action": "param-watch", "id": id, "on": false, "n": 6 }))?;
    let _ = read_until(ws, |v| v["type"] == "host" && v["action"] == "param-watch", Duration::from_secs(5), &mut audio)?;
    host(ws, json!({ "action": "remove", "id": id }))?;
    Ok(())
}

fn render_test(ws: &mut Ws, name: &str, sig: &str, secs: f64) -> Result<(), String> {
    let mut audio = Vec::new();
    loop {
        host(ws, json!({ "action": "status" }))?;
        let v = read_until(ws, is_host("status"), Duration::from_secs(10), &mut audio)?;
        if v["scanning"] != true && v["total"].as_u64().unwrap_or(0) > 0 && v["done"] == v["total"] {
            break;
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
    host(ws, json!({ "action": "plugins" }))?;
    let p = read_until(ws, is_host("plugins"), Duration::from_secs(10), &mut audio)?;
    let list = p["list"].as_array().cloned().unwrap_or_default();
    let want = name.to_lowercase();
    let pick = list.iter().find(|x| x["kind"] == "effect" && x["name"].as_str().unwrap_or("").to_lowercase().contains(&want)).ok_or(format!("no effect like {name}"))?.clone();
    let t = Instant::now();
    host(ws, json!({ "action": "add", "plugin": pick["id"], "n": 1 }))?;
    let added = read_until(ws, |v| v["type"] == "host" && (v["action"] == "added" || v["action"] == "add"), Duration::from_secs(70), &mut audio)?;
    if added["error"].is_string() {
        return Err(format!("add: {added}"));
    }
    let id = added["item"]["id"].as_u64().unwrap();
    println!("effect {} ({}) opened in {:.0} ms", pick["name"], pick["vendor"], t.elapsed().as_secs_f64() * 1000.0);
    // le son
    let rate = 44100u32;
    let frames = (secs * rate as f64) as usize;
    let mut x = vec![0f32; frames * 2];
    for i in 0..frames {
        let v = match sig {
            "sine" => 0.5 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / rate as f64).sin() as f32,
            _ => if i == rate as usize / 10 { 0.9 } else { 0.0 },
        };
        x[2 * i] = v;
        x[2 * i + 1] = v;
    }
    let t = Instant::now();
    host(ws, json!({ "action": "render-begin", "id": id, "frames": frames, "rate": rate, "bpm": 120, "tail": 3.0, "n": 2 }))?;
    let b = read_until(ws, |v| v["type"] == "host" && v["action"] == "render-begin", Duration::from_secs(10), &mut audio)?;
    if b["error"].is_string() {
        return Err(format!("render-begin: {b}"));
    }
    let job = b["job"].as_u64().unwrap() as u32;
    let chunk = b["chunk"].as_u64().unwrap_or(16384) as usize;
    let mut at = 0usize;
    while at < frames {
        let n = chunk.min(frames - at);
        let mut m = Vec::with_capacity(12 + n * 8);
        m.extend_from_slice(b"UWJ1");
        m.extend_from_slice(&job.to_le_bytes());
        m.extend_from_slice(&(at as u32).to_le_bytes());
        for v in &x[2 * at..2 * (at + n)] {
            m.extend_from_slice(&v.to_le_bytes());
        }
        ws.send(Message::binary(m)).map_err(|e| e.to_string())?;
        at += n;
    }
    host(ws, json!({ "action": "render-end", "job": job, "n": 3 }))?;
    // le resultat
    let mut out: Vec<f32> = Vec::new();
    let mut info = Value::Null;
    let end = Instant::now() + Duration::from_secs(120);
    while Instant::now() < end {
        match ws.read() {
            Ok(Message::Binary(b)) if b.len() >= 12 && &b[..4] == b"UWJ2" => {
                let j = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                let off = u32::from_le_bytes([b[8], b[9], b[10], b[11]]) as usize;
                if j != job || off * 2 != out.len() {
                    return Err(format!("chunk out of order: job {j}, at {off}, have {}", out.len() / 2));
                }
                for c in b[12..].chunks_exact(4) {
                    out.push(f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
                }
            }
            Ok(Message::Text(t)) => {
                let v: Value = serde_json::from_str(t.as_str()).unwrap_or(Value::Null);
                if v["type"] == "host" && (v["action"] == "rendered" || (v["action"] == "render" && v["error"].is_string())) {
                    info = v;
                    break;
                }
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(format!("websocket: {e}")),
        }
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    if info["error"].is_string() || info.is_null() {
        return Err(format!("render: {info}"));
    }
    let n_out = out.len() / 2;
    let peak = |a: &[f32]| a.iter().fold(0f32, |m, v| m.max(v.abs()));
    let after = if n_out > frames { peak(&out[2 * frames..]) } else { 0.0 };
    let diff = (0..frames.min(n_out)).map(|i| (out[2 * i] - x[2 * i]).abs()).fold(0f32, f32::max);
    let first = out.chunks_exact(2).position(|c| c[0].abs() > 1e-3);
    println!("render: {ms:.0} ms for {secs} s ; {info}");
    println!("  frames in {frames}, out {n_out} (declared {}), latency removed {}, tail {}", info["frames"], info["latency"], info["tail"]);
    println!("  peak in {:.3}, out {:.3}, after the input's end {:.4} ; max |out - in| {:.5} ; first sound at frame {:?} (input impulse at {})", peak(&x), peak(&out), after, diff, first, rate / 10);
    host(ws, json!({ "action": "remove", "id": id }))?;
    let _ = read_until(ws, is_host("removed"), Duration::from_secs(10), &mut audio);
    Ok(())
}

/// Ouvre l'effet dont le nom contient `name` (apres le scan) → son id.
fn open_effect(ws: &mut Ws, name: &str) -> Result<u64, String> {
    let mut audio = Vec::new();
    loop {
        host(ws, json!({ "action": "status" }))?;
        let v = read_until(ws, is_host("status"), Duration::from_secs(10), &mut audio)?;
        if v["scanning"] != true && v["total"].as_u64().unwrap_or(0) > 0 && v["done"] == v["total"] {
            break;
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
    host(ws, json!({ "action": "plugins" }))?;
    let p = read_until(ws, is_host("plugins"), Duration::from_secs(10), &mut audio)?;
    let list = p["list"].as_array().cloned().unwrap_or_default();
    let want = name.to_lowercase();
    let pick = list.iter().find(|x| x["kind"] == "effect" && x["name"].as_str().unwrap_or("").to_lowercase().contains(&want)).ok_or(format!("no effect like {name}"))?.clone();
    let t = Instant::now();
    host(ws, json!({ "action": "add", "plugin": pick["id"], "n": 1 }))?;
    let added = read_until(ws, |v| v["type"] == "host" && (v["action"] == "added" || v["action"] == "add"), Duration::from_secs(70), &mut audio)?;
    if added["error"].is_string() {
        return Err(format!("add: {added}"));
    }
    println!("effect {} ({}) opened in {:.0} ms", pick["name"], pick["vendor"], t.elapsed().as_secs_f64() * 1000.0);
    Ok(added["item"]["id"].as_u64().unwrap())
}

/// (19 nov., etape 3b) Un effet en direct, comme le studio s'en servira : des paquets « UWF1 » de 256 trames a 44,1 kHz,
/// au rythme du temps reel, dates de leur trame (a partir de 100 000) ; le son revient en « UWA1 » (flux 9), date de la
/// meme trame moins la latence du plugin. On mesure : le retard de chaque retour sur l'envoi de son entree (p50,
/// p99,9, max : il fixe le retard D du studio), les trames manquantes, l'ecart avec l'entree (un effet transparent : 0),
/// et la place des impulsions (une par seconde) : un effet a latence doit rester aligne.
fn fxlive_test(ws: &mut Ws, name: &str, secs: f64, b_frames: usize) -> Result<(), String> {
    const K: u8 = 9;
    let bf: usize = b_frames;
    const P0: u64 = 100_000;
    let rate = 44100u32;
    let sig = |pos: u64| -> f32 {
        let s = 0.25 * (2.0 * std::f64::consts::PI * 440.0 * pos as f64 / rate as f64).sin();
        let imp = if pos % rate as u64 == 0 { 0.6 } else { 0.0 };
        (s + imp) as f32
    };
    let id = open_effect(ws, name)?;
    let mut audio = Vec::new();
    host(ws, json!({ "action": "fx-live", "id": id, "rate": rate, "stream": K, "n": 5 }))?;
    let on = read_until(ws, is_host("fx-live"), Duration::from_secs(20), &mut audio)?;
    if on["on"] != true {
        return Err(format!("fx-live: {on}"));
    }
    let lat = on["latency"].as_u64().unwrap_or(0);
    println!("live: {on}");
    ws.get_mut().set_read_timeout(Some(Duration::from_millis(1))).map_err(|e| e.to_string())?;
    let blocks = (secs * rate as f64 / bf as f64) as u64;
    let mut sent_at: Vec<Instant> = Vec::with_capacity(blocks as usize);
    let mut got: std::collections::BTreeMap<u64, Vec<f32>> = std::collections::BTreeMap::new();
    let mut late_ms: Vec<f64> = Vec::new();
    let t0 = Instant::now() + Duration::from_millis(20);
    let mut k = 0u64;
    let until = |k: u64| t0 + Duration::from_secs_f64((k * bf as u64) as f64 / rate as f64);
    let end_wait = Duration::from_secs(2);
    let mut last_sent = Instant::now();
    loop {
        let now = Instant::now();
        if k < blocks && now >= until(k) {
            let pos = P0 + k * bf as u64;
            let mut m = Vec::with_capacity(20 + bf * 8);
            m.extend_from_slice(b"UWF1");
            m.extend_from_slice(&[K, 0, 0, 0]);
            m.extend_from_slice(&pos.to_le_bytes());
            m.extend_from_slice(&(bf as u16).to_le_bytes());
            m.extend_from_slice(&[0, 0]);
            for i in 0..bf as u64 {
                let v = sig(pos + i);
                m.extend_from_slice(&v.to_le_bytes());
                m.extend_from_slice(&v.to_le_bytes());
            }
            // (Windows) une lecture avec delai laisse parfois l'erreur 997 : le message reste dans le tampon, il partira
            match ws.send(Message::binary(m)) {
                Ok(()) => {}
                Err(tungstenite::Error::Io(e)) if soft(&e) => {}
                Err(e) => return Err(e.to_string()),
            }
            sent_at.push(Instant::now());
            last_sent = Instant::now();
            k += 1;
            continue;
        }
        if k >= blocks && now.duration_since(last_sent) > end_wait {
            break;
        }
        match ws.read() {
            Ok(Message::Binary(b)) if b.len() >= 36 && &b[..4] == b"UWA1" && b[33] == K => {
                let arrived = Instant::now();
                let pos = u64::from_le_bytes(b[8..16].try_into().unwrap());
                let nfr = u16::from_le_bytes([b[28], b[29]]) as usize;
                let x: Vec<f32> = b[36..].chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
                if x.len() == nfr * 2 {
                    let inp = pos + lat;
                    if inp >= P0 && (inp - P0) % bf as u64 == 0 {
                        if let Some(t) = sent_at.get(((inp - P0) / bf as u64) as usize) {
                            late_ms.push(arrived.duration_since(*t).as_secs_f64() * 1000.0);
                        }
                    }
                    got.insert(pos, x);
                }
            }
            Ok(Message::Text(t)) => {
                let v: Value = serde_json::from_str(t.as_str()).unwrap_or(Value::Null);
                if v["type"] == "host" || v["type"] == "error" {
                    println!("  {v}");
                }
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if soft(&e) => {
                let _ = ws.flush();
            }
            Err(e) => return Err(format!("websocket: {e}")),
        }
    }
    // le son revenu, par trame du studio
    let first = P0.saturating_sub(lat);
    let total = blocks * bf as u64;
    let mut out = vec![f32::NAN; total as usize];
    for (pos, x) in &got {
        for i in 0..x.len() / 2 {
            let p = pos + i as u64;
            if p >= first && p < first + total {
                out[(p - first) as usize] = x[2 * i];
            }
        }
    }
    let missing = out.iter().filter(|v| v.is_nan()).count();
    // un effet transparent : la sortie a la trame p est l'entree a la trame p (au-dela de la latence du debut)
    let (mut diff, mut diff_at, mut over) = (0f32, 0u64, 0usize);
    for (i, v) in out.iter().enumerate() {
        let p = first + i as u64;
        if !v.is_nan() && p >= P0 {
            let d = (v - sig(p)).abs();
            if d > 1e-6 {
                over += 1;
            }
            if d > diff {
                diff = d;
                diff_at = p;
            }
        }
    }
    if over > 0 {
        let i = (diff_at - first) as usize;
        println!("  {over} frames differ ; worst at {diff_at}: out {:?} in {:?}", &out[i.saturating_sub(2)..(i + 3).min(out.len())], (diff_at - 2..diff_at + 3).map(|p| sig(p)).collect::<Vec<_>>());
    }
    // la place des impulsions
    let mut offs: Vec<i64> = Vec::new();
    let mut p = (P0 / rate as u64 + 1) * rate as u64;
    while p + 4000 < first + total {
        let (mut best, mut at) = (0f32, 0i64);
        for d in -2000i64..2000 {
            let q = (p as i64 + d - first as i64) as usize;
            if let Some(v) = out.get(q) {
                if !v.is_nan() && v.abs() > best {
                    best = v.abs();
                    at = d;
                }
            }
        }
        offs.push(at);
        p += rate as u64;
    }
    late_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q = |f: f64| late_ms.get(((late_ms.len() as f64 - 1.0) * f).round() as usize).copied().unwrap_or(f64::NAN);
    println!("sent {} blocks of {bf} ({:.1} s), back {} packets ; missing frames {missing} of {total}", blocks, total as f64 / rate as f64, got.len());
    println!("  return after its input was sent (ms): p50 {:.2}, p99 {:.2}, p99.9 {:.2}, max {:.2} ; plugin latency {lat} frames ({:.2} ms)", q(0.5), q(0.99), q(0.999), q(1.0), lat as f64 * 1000.0 / rate as f64);
    println!("  max |out - in| {diff:.6} ; impulse offsets (frames) {:?}", offs.iter().collect::<std::collections::BTreeSet<_>>());
    host(ws, json!({ "action": "fx-stop", "stream": K }))?;
    ws.get_mut().set_read_timeout(Some(Duration::from_millis(5))).map_err(|e| e.to_string())?;
    let _ = read_until(ws, is_host("fx-stop"), Duration::from_secs(5), &mut audio);
    host(ws, json!({ "action": "remove", "id": id }))?;
    let _ = read_until(ws, is_host("removed"), Duration::from_secs(10), &mut audio);
    Ok(())
}

fn soft(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) || e.raw_os_error() == Some(997)
}

/// (20 nov., l'hote macOS) Les messages et les images d'une capture : (le texte, ou l'en-tete d'une image et son JPEG).
enum Got {
    Text(Value),
    Frame(Value, Vec<u8>),
    Nothing,
}

fn read_any(ws: &mut Ws) -> Got {
    match ws.read() {
        Ok(Message::Text(t)) => Got::Text(serde_json::from_str(t.as_str()).unwrap_or(Value::Null)),
        Ok(Message::Binary(b)) if b.len() >= 8 && &b[..4] == b"UWF1" => {
            let n = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
            match (b.get(8..8 + n), b.get(8 + n..)) {
                (Some(h), Some(j)) => Got::Frame(serde_json::from_slice(h).unwrap_or(Value::Null), j.to_vec()),
                _ => Got::Nothing,
            }
        }
        _ => Got::Nothing,
    }
}

/// (20 nov., l'hote macOS) Un menu contextuel du plugin, depuis le studio : clic droit au point `m` (fractions de
/// l'image), « save » pendant que le menu est ouvert (il ne doit ni attendre ni le fermer), un clic sur son premier
/// article (25 × 5 points sous le pointeur, la place du premier article d'un menu contextuel de macOS 26), « save », le
/// menu rouvert puis ferme par Echap, et le menu rouvert puis ferme par un clic a cote. Chaque etape : le temps, les
/// images recues (la derniere dans --out), et ce que l'enfant a dit ({"event":"menu"} n'arrive pas jusqu'ici : les
/// images et les temps de « save » sont les temoins).
fn menu_steps(ws: &mut Ws, id: u64, size: (f64, f64), scale: f64, m: (f64, f64), out: &Option<std::path::PathBuf>) -> Result<(), String> {
    let mut audio = Vec::new();
    let (w, h) = size;
    let at = (m.0 * w, m.1 * h);
    let item = (at.0 + 25.0 * scale, at.1 + 5.0 * scale);
    let away = (0.08 * w, 0.92 * h);
    let pointer = |ws: &mut Ws, act: &str, p: (f64, f64), button: u8| -> Result<(), String> {
        let buttons = match (act, button) {
            ("down", 2) => 2,
            ("down", _) => 1,
            _ => 0,
        };
        let msg = json!({ "type": "input", "kind": "pointer", "action": act, "x": p.0, "y": p.1, "iw": w, "ih": h, "button": button, "buttons": buttons, "mods": 0 });
        ws.send(Message::text(msg.to_string())).map_err(|e| e.to_string())
    };
    let click = |ws: &mut Ws, p: (f64, f64), button: u8| -> Result<(), String> {
        pointer(ws, "down", p, button)?;
        std::thread::sleep(Duration::from_millis(50));
        pointer(ws, "up", p, button)
    };
    // les images pendant `ms` : combien, et la derniere dans --out sous `name`
    let frames_for = |ws: &mut Ws, ms: u64, name: &str| -> usize {
        let end = Instant::now() + Duration::from_millis(ms);
        let (mut n, mut last) = (0usize, Vec::new());
        while Instant::now() < end {
            if let Got::Frame(hd, j) = read_any(ws) {
                if let Some(seq) = hd["seq"].as_u64() {
                    let _ = ws.send(Message::text(json!({ "type": "ack", "seq": seq }).to_string()));
                }
                n += 1;
                if hd.get("x").is_none() {
                    last = j;
                }
            }
        }
        if let (Some(d), false) = (out, last.is_empty()) {
            let _ = std::fs::create_dir_all(d);
            let _ = std::fs::write(d.join(format!("{name}.jpg")), &last);
        }
        n
    };
    let mut k = 20;
    let mut save = |ws: &mut Ws, what: &str| -> Result<Value, String> {
        k += 1;
        let t = Instant::now();
        host(ws, json!({ "action": "save", "id": id, "n": k }))?;
        let r = read_until(ws, |v| v["type"] == "host" && (v["action"] == "saved" || v["error"].is_string()), Duration::from_secs(20), &mut audio);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        match &r {
            Ok(v) => println!("  save {what}: {ms:.0} ms, state {}", v["state"].as_str().map(|s| &s[..12.min(s.len())]).unwrap_or("?")),
            Err(e) => println!("  save {what}: FAILED after {ms:.0} ms ({e})"),
        }
        r
    };
    println!("menu test: right-click at {:.0},{:.0} of {w:.0}x{h:.0} (scale {scale:.2}), first item at {:.0},{:.0}", at.0, at.1, item.0, item.1);
    let s0 = save(ws, "before")?;
    click(ws, at, 2)?;
    println!("  right-click: {} images in 1 s (menu-open.jpg)", frames_for(ws, 1000, "menu-open"));
    let s1 = save(ws, "while the menu is open")?;
    println!("  after that save: {} images in 0.6 s (menu-still-open.jpg)", frames_for(ws, 600, "menu-still-open"));
    click(ws, item, 0)?;
    println!("  click on the first item: {} images in 1 s (after-item.jpg)", frames_for(ws, 1000, "after-item"));
    let s2 = save(ws, "after the item")?;
    click(ws, at, 2)?;
    println!("  right-click again: {} images in 1 s (menu-again.jpg)", frames_for(ws, 1000, "menu-again"));
    for act in ["down", "up"] {
        let msg = json!({ "type": "input", "kind": "key", "action": act, "key": "Escape", "code": "Escape", "mods": 0 });
        ws.send(Message::text(msg.to_string())).map_err(|e| e.to_string())?;
    }
    println!("  Escape: {} images in 0.8 s (after-escape.jpg)", frames_for(ws, 800, "after-escape"));
    save(ws, "after Escape")?;
    click(ws, at, 2)?;
    frames_for(ws, 800, "menu-third");
    click(ws, away, 0)?;
    println!("  click away at {:.0},{:.0}: {} images in 0.8 s (after-away.jpg)", away.0, away.1, frames_for(ws, 800, "after-away"));
    save(ws, "after the click away")?;
    // le menu ouvert, puis le plugin retire (watch_test) : l'enfant doit se fermer sans attendre
    click(ws, at, 2)?;
    frames_for(ws, 600, "menu-before-remove");
    println!(
        "states: before {} · during the menu {} · after the item {}",
        s0["state"].as_str().unwrap_or("?").get(..12).unwrap_or("?"),
        s1["state"].as_str().unwrap_or("?").get(..12).unwrap_or("?"),
        s2["state"].as_str().unwrap_or("?").get(..12).unwrap_or("?")
    );
    Ok(())
}

fn watch_test(ws: &mut Ws, name: &str, secs: f64, out: Option<std::path::PathBuf>, click: Option<(f64, f64)>, menu: Option<(f64, f64)>, hold: f64) -> Result<(), String> {
    let mut audio = Vec::new();
    let want = name.to_lowercase();
    // le scan
    let t0 = Instant::now();
    let list = loop {
        host(ws, json!({ "action": "plugins" }))?;
        let p = read_until(ws, is_host("plugins"), Duration::from_secs(10), &mut audio)?;
        if p["scanning"] != true || t0.elapsed() > Duration::from_secs(300) {
            break p["list"].as_array().cloned().unwrap_or_default();
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    let pick = list.iter().find(|x| x["name"].as_str().unwrap_or("").to_lowercase().contains(&want)).ok_or(format!("no plugin named like \"{want}\""))?.clone();
    host(ws, json!({ "action": "add", "plugin": pick["id"], "n": 1 }))?;
    let added = read_until(ws, |v| v["type"] == "host" && (v["action"] == "added" || v["error"].is_string()), Duration::from_secs(70), &mut audio)?;
    if added["error"].is_string() {
        return Err(format!("add: {added}"));
    }
    let item = added["item"].clone();
    let (id, tag) = (item["id"].as_u64().unwrap_or(0), item["tag"].as_u64().unwrap_or(0));
    println!("added {} (id {id}, pid {}, window {})", pick["name"], item["pid"], item["hwnd"]);
    // le plugin est retire meme si la mesure echoue (un enfant laisse ouvert garde son menu, sa fenetre)
    let r = watch_body(ws, id, tag, secs, out, click, menu);
    if hold > 0.0 {
        println!("holding: the menu stays open, the plugin loaded, for {hold:.0} s");
        let end = Instant::now() + Duration::from_secs_f64(hold);
        while Instant::now() < end {
            let _ = read_any(ws);
        }
        return r;
    }
    let _ = ws.send(Message::text(r#"{"type":"unwatch"}"#.to_string()));
    let t = Instant::now();
    host(ws, json!({ "action": "remove", "id": id }))?;
    let rm = read_until(ws, is_host("removed"), Duration::from_secs(8), &mut audio);
    println!("removed in {:.0} ms{}", t.elapsed().as_secs_f64() * 1000.0, if rm.is_err() { " (NO answer)" } else { "" });
    r
}

fn watch_body(ws: &mut Ws, id: u64, tag: u64, secs: f64, out: Option<std::path::PathBuf>, click: Option<(f64, f64)>, menu: Option<(f64, f64)>) -> Result<(), String> {
    let mut audio = Vec::new();
    let t = Instant::now();
    host(ws, json!({ "action": "view", "id": id, "open": true }))?;
    // la fenetre dans la liste des fenetres (piste « #<id> »)
    let win = loop {
        ws.send(Message::text(r#"{"type":"list"}"#.to_string())).map_err(|e| e.to_string())?;
        let v = read_until(ws, |v| v["type"] == "windows", Duration::from_secs(5), &mut audio)?;
        if let Some(w) = v["windows"].as_array().and_then(|l| l.iter().find(|w| w["track"] == format!("#{id}"))) {
            break w.clone();
        }
        if t.elapsed() > Duration::from_secs(15) {
            return Err(format!("the editor window never showed in the list: {v}"));
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    println!("editor open in {:.0} ms: {win}", t.elapsed().as_secs_f64() * 1000.0);
    let wid = win["id"].as_u64().unwrap_or(0);
    ws.send(Message::text(json!({ "type": "watch", "id": wid, "fps": 30 }).to_string())).map_err(|e| e.to_string())?;
    let save = |n: &str, j: &[u8]| {
        if let Some(d) = &out {
            let _ = std::fs::create_dir_all(d);
            let _ = std::fs::write(d.join(n), j);
        }
    };
    let mut frames = 0usize;
    let mut first = true;
    let mut last: Vec<u8> = Vec::new();
    let mut size = (0u64, 0u64);
    let t_first = epoch_ms() + 300.0;
    let mut next = 0usize;
    let end = Instant::now() + Duration::from_secs_f64(secs);
    let mut times = Vec::new();
    while Instant::now() < end {
        if epoch_ms() >= t_first + next as f64 * 500.0 - 60.0 {
            let tt = t_first + next as f64 * 500.0;
            let pitch = [48, 55, 60, 64, 67, 72][next % 6];
            let m = json!({ "type": "midi", "to": format!("tag:{tag}"), "events": [{ "t": tt, "d": [0x90, pitch, 100] }, { "t": tt + 300.0, "d": [0x80, pitch, 0] }] });
            let _ = ws.send(Message::text(m.to_string()));
            next += 1;
        }
        match read_any(ws) {
            Got::Text(v) => {
                if v["type"] == "watching" || v["type"] == "unwatched" {
                    println!("{v}");
                    if v["type"] == "unwatched" {
                        return Err("the companion refused the capture".into());
                    }
                }
            }
            Got::Frame(h, j) => {
                frames += 1;
                times.push(epoch_ms());
                if let Some(seq) = h["seq"].as_u64() {
                    let _ = ws.send(Message::text(json!({ "type": "ack", "seq": seq }).to_string()));
                }
                if h.get("x").is_none() {
                    size = (h["w"].as_u64().unwrap_or(0), h["h"].as_u64().unwrap_or(0));
                    if first {
                        save("first.jpg", &j);
                        first = false;
                    }
                    last = j;
                }
            }
            Got::Nothing => {}
        }
    }
    if !last.is_empty() {
        save("last.jpg", &last);
    }
    println!("frames in {secs:.0} s while notes played: {frames} (image {}x{})", size.0, size.1);
    if let Some(m) = menu {
        if size.0 == 0 {
            size = (win["w"].as_u64().unwrap_or(0), win["h"].as_u64().unwrap_or(0));
        }
        // l'echelle de l'image (2 sur un ecran Retina) : les decalages du menu sont en points
        let pts = win["w"].as_f64().unwrap_or(size.0 as f64).max(1.0);
        return menu_steps(ws, id, (size.0.max(1) as f64, size.1.max(1) as f64), size.0 as f64 / pts, m, &out);
    }
    if let Some((fx, fy)) = click {
        // sans image (pas d'autorisation d'enregistrer l'ecran) : la taille de la fenetre dans la liste
        if size.0 == 0 {
            size = (win["w"].as_u64().unwrap_or(0), win["h"].as_u64().unwrap_or(0));
        }
        let (w, h) = (size.0.max(1) as f64, size.1.max(1) as f64);
        let (x, y) = (fx * w, fy * h);
        // l'etat avant le geste (le plugin dit-il un changement apres ? un bouton tourne par le geste)
        host(ws, json!({ "action": "save", "id": id, "n": 11 }))?;
        let before = read_until(ws, |v| v["type"] == "host" && (v["action"] == "saved" || v["error"].is_string()), Duration::from_secs(15), &mut audio)?;
        // (20 nov., Link to controller) le suivi des parametres pendant le geste
        host(ws, json!({ "action": "param-watch", "id": id, "on": true, "n": 13 }))?;
        let _ = read_until(ws, |v| v["type"] == "host" && v["action"] == "param-watch", Duration::from_secs(10), &mut audio)?;
        let t = Instant::now();
        // un glisser vers le haut (un bouton rotatif), puis le relachement
        let mut steps = vec![("down", 1, y)];
        for k in 1..=8 {
            steps.push(("move", 1, y - 8.0 * k as f64));
        }
        steps.push(("up", 0, y - 64.0));
        for (act, btns, yy) in steps {
            let m = json!({ "type": "input", "kind": "pointer", "action": act, "x": x, "y": yy, "iw": w, "ih": h, "button": 0, "buttons": btns, "mods": 0 });
            ws.send(Message::text(m.to_string())).map_err(|e| e.to_string())?;
            // comme le studio (plugin-ctl.js) : le suivi redemande a l'appui, en meme temps que le geste
            if act == "down" {
                host(ws, json!({ "action": "param-watch", "id": id, "on": true, "n": 15 }))?;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut after = None;
        let mut ack = None;
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end {
            match read_any(ws) {
                Got::Text(v) if v["type"] == "inputack" => {
                    if ack.is_none() {
                        ack = Some(v.clone());
                    }
                }
                Got::Frame(h, j) => {
                    if let Some(seq) = h["seq"].as_u64() {
                        let _ = ws.send(Message::text(json!({ "type": "ack", "seq": seq }).to_string()));
                    }
                    if after.is_none() {
                        after = Some(t.elapsed().as_secs_f64() * 1000.0);
                    }
                    if h.get("x").is_none() {
                        save("after-click.jpg", &j);
                    }
                }
                _ => {}
            }
        }
        println!("drag at {x:.0},{y:.0}: ack {} ; next image after {:?} ms", ack.map(|v| v.to_string()).unwrap_or("none".into()), after.map(|a| a.round()));
        host(ws, json!({ "action": "param-changes", "id": id, "n": 14 }))?;
        let pc = read_until(ws, |v| v["type"] == "host" && v["action"] == "param-changes", Duration::from_secs(5), &mut audio)?;
        println!("param changes after the drag: {}", pc["items"]);
        host(ws, json!({ "action": "save", "id": id, "n": 12 }))?;
        let aft = read_until(ws, |v| v["type"] == "host" && (v["action"] == "saved" || v["error"].is_string()), Duration::from_secs(15), &mut audio)?;
        println!("state before {} ; after {} → the gesture {} the plugin", before["state"], aft["state"], if before["state"] != aft["state"] { "CHANGED" } else { "did not change" });
    }
    Ok(())
}

/// (24 nov., 0.4.15) --unsaved : voir run.
fn unsaved_test(ws: &mut Ws, name: &str, phase: &str, id: Option<u64>, p: Option<u64>, state: Option<String>) -> Result<(), String> {
    let mut audio = Vec::new();
    let read_p = |ws: &mut Ws, id: u64, p: u64, audio: &mut Vec<(f64, f32, f64)>| -> Result<f64, String> {
        host(ws, json!({ "action": "params", "id": id, "start": p, "n": 4 }))?;
        let r = read_until(ws, |v| v["type"] == "host" && v["action"] == "params", Duration::from_secs(10), audio)?;
        r["items"].as_array().and_then(|a| a.iter().find(|x| x["p"].as_u64() == Some(p))).and_then(|x| x["v"].as_f64()).ok_or(format!("params: {r}"))
    };
    let save = |ws: &mut Ws, id: u64, audio: &mut Vec<(f64, f32, f64)>| -> Result<String, String> {
        host(ws, json!({ "action": "save", "id": id, "n": 7 }))?;
        let v = read_until(ws, |v| v["type"] == "host" && (v["action"] == "saved" || v["action"] == "save"), Duration::from_secs(20), audio)?;
        v["state"].as_str().map(str::to_string).ok_or(format!("save: {v}"))
    };
    let add = |ws: &mut Ws, state: Option<&str>, audio: &mut Vec<(f64, f32, f64)>| -> Result<u64, String> {
        loop {
            host(ws, json!({ "action": "status" }))?;
            let v = read_until(ws, is_host("status"), Duration::from_secs(10), audio)?;
            if v["scanning"] != true && v["total"].as_u64().unwrap_or(0) > 0 && v["done"] == v["total"] {
                break;
            }
            std::thread::sleep(Duration::from_millis(1000));
        }
        host(ws, json!({ "action": "plugins" }))?;
        let pl = read_until(ws, is_host("plugins"), Duration::from_secs(10), audio)?;
        let want = name.to_lowercase();
        let pick = pl["list"].as_array().and_then(|l| l.iter().find(|x| x["name"].as_str().unwrap_or("").to_lowercase().contains(&want)).cloned()).ok_or(format!("no plugin like {name}"))?;
        host(ws, json!({ "action": "add", "plugin": pick["id"], "state": state, "n": 1 }))?;
        let added = read_until(ws, |v| v["type"] == "host" && (v["action"] == "added" || v["action"] == "add"), Duration::from_secs(70), audio)?;
        added["item"]["id"].as_u64().ok_or(format!("add: {added}"))
    };
    match phase {
        "open" => {
            let id = add(ws, None, &mut audio)?;
            let s1 = save(ws, id, &mut audio)?;
            host(ws, json!({ "action": "params", "id": id, "start": 0, "n": 10 }))?;
            let r = read_until(ws, |v| v["type"] == "host" && v["action"] == "params", Duration::from_secs(15), &mut audio)?;
            let all = r["items"].as_array().cloned().unwrap_or_default();
            let t = all.iter().find(|x| x["name"].as_str().unwrap_or("").to_lowercase().contains("cutoff")).or(all.first()).cloned().ok_or("no parameter")?;
            let (p, v0) = (t["p"].as_u64().unwrap(), t["v"].as_f64().unwrap_or(0.0));
            let v1 = if v0 > 0.5 { 0.2 } else { 0.8 };
            host(ws, json!({ "action": "param-set", "id": id, "p": p, "v": v1 }))?;
            std::thread::sleep(Duration::from_millis(400));
            let back = read_p(ws, id, p, &mut audio)?;
            println!("open: id {id}, saved S1 {s1}, param {p} ({}) {v0:.3} -> {v1:.3}, read {back:.3} ; leaving WITHOUT saving", t["name"]);
        }
        "back" => {
            let id = id.ok_or("--id")?;
            host(ws, json!({ "action": "list" }))?;
            let l = read_until(ws, |v| v["type"] == "host" && v["action"] == "list", Duration::from_secs(5), &mut audio)?;
            let there = l["items"].as_array().is_some_and(|a| a.iter().any(|x| x["id"].as_u64() == Some(id)));
            println!("back: instance {id} still loaded: {there}");
            if there {
                let s2 = save(ws, id, &mut audio)?;
                let v = read_p(ws, id, p.ok_or("--p")?, &mut audio)?;
                println!("back: saved S2 {s2}, param {} = {v:.3}", p.unwrap());
            }
        }
        "reopen" => {
            let id = add(ws, state.as_deref(), &mut audio)?;
            let v = read_p(ws, id, p.ok_or("--p")?, &mut audio)?;
            println!("reopen with {}: param {} = {v:.3}", state.as_deref().unwrap_or("-"), p.unwrap());
            host(ws, json!({ "action": "remove", "id": id }))?;
            let _ = read_until(ws, |v| v["type"] == "host" && (v["action"] == "removed" || v["action"] == "remove"), Duration::from_secs(10), &mut audio);
        }
        _ => return Err("--phase open|back|reopen".into()),
    }
    Ok(())
}
