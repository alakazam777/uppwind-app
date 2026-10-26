// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Le retour audio SANS cable (24 oct.) : le compagnon capture le son du DAW lui-meme (Windows « process loopback » :
//! seulement le processus du DAW et ses enfants) et l'envoie au studio par le WebSocket deja appaire, en paquets de
//! 5 ms. Le studio le joue dans le bus External, la ou arrivait le cable (VB-CABLE).
//!
//!   uppwind-companion --capture-test <pid|nom|tone> [--seconds N] [--rate R] [--exclude-tree]
//!       le diagnostic : ecoute ce processus (n'importe lequel : la console seulement), dit le format, les niveaux
//!       (crete / RMS par seconde), la cadence de Windows, si ca marche. « tone » : lance d'abord la tonalite d'essai
//!       (un autre processus, --tone) et l'ecoute. Rien n'est enregistre ni envoye.
//!   uppwind-companion --tone [--seconds N] [--tone-db -30] [--no-sine] [--click-ms 500] [--click-db -12]
//!       [--click-log FICHIER] [--tone-device NOM] [--tone-mute] [--tone-volume 0..1]
//!       la tonalite d'essai : un sinus de 440 Hz (et des clics cales sur l'horloge) en WASAPI partage.
//!   serveur : --test-tone (les memes options) lance la tonalite d'essai, que le studio peut demander (target
//!       « test-tone ») — la mesure de la latence de bout en bout.
//!
//! Securite : les paquets ne partent que sur une connexion appairee, et seulement pour un DAW de la liste blanche
//! (procs.rs) ; la tonalite d'essai n'est acceptee que si le compagnon l'a lancee lui-meme.

pub mod feed;
pub mod packet;
pub mod procs;
#[cfg(windows)]
pub mod wasapi;

use serde_json::json;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub fn epoch_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

static AVAILABLE: OnceLock<Result<f64, String>> = OnceLock::new();
static TEST_TONE: OnceLock<u32> = OnceLock::new();

/// La capture d'un processus marche-t-elle ici ? (essayee une fois, sur le compagnon lui-meme) → Ok(ms) ou l'erreur.
pub fn available() -> Result<f64, String> {
    AVAILABLE
        .get_or_init(|| {
            #[cfg(windows)]
            {
                wasapi::probe()
            }
            #[cfg(not(windows))]
            {
                Err("the audio return needs Windows".to_string())
            }
        })
        .clone()
}

/// Le pid de la tonalite d'essai lancee par --test-tone (la seule cible hors liste blanche que le studio peut demander).
pub fn test_tone() -> Option<u32> {
    TEST_TONE.get().copied()
}

/// Les options de la tonalite d'essai (--tone, --test-tone) qui se transmettent telles quelles.
const TONE_FLAGS: [&str; 3] = ["--tone-mute", "--no-sine", "--tone-unmute"];
const TONE_VALUES: [&str; 8] = ["--tone-db", "--click-ms", "--click-db", "--click-log", "--tone-device", "--tone-volume", "--tone-hz", "--tone-delay"];

fn tone_args(raw: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let a = raw[i].as_str();
        if TONE_FLAGS.contains(&a) {
            out.push(a.to_string());
        } else if TONE_VALUES.contains(&a) && i + 1 < raw.len() {
            out.push(a.to_string());
            out.push(raw[i + 1].clone());
            i += 1;
        }
        i += 1;
    }
    out
}

/// Les options du serveur que main.rs doit laisser passer (lues ici).
pub fn is_server_flag(a: &str) -> Option<bool> {
    if a == "--test-tone" || TONE_FLAGS.contains(&a) {
        return Some(false);
    }
    if TONE_VALUES.contains(&a) {
        return Some(true); // suivie d'une valeur
    }
    None
}

fn value<'a>(raw: &'a [String], name: &str) -> Option<&'a str> {
    raw.iter().position(|a| a == name).and_then(|i| raw.get(i + 1)).map(|s| s.as_str())
}

fn num(raw: &[String], name: &str) -> Result<Option<f64>, String> {
    match value(raw, name) {
        None => Ok(None),
        Some(v) => v.parse::<f64>().ok().filter(|x| x.is_finite()).map(Some).ok_or(format!("bad {name}")),
    }
}

/// Lancer la tonalite d'essai : un autre processus (le meme exe, --tone), qui s'arrete avec le compagnon.
fn spawn_tone(raw: &[String], seconds: f64) -> Result<std::process::Child, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this program ({e})"))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--tone").arg("--seconds").arg(format!("{seconds}")).arg("--parent").arg(std::process::id().to_string());
    cmd.args(tone_args(raw));
    cmd.stdout(std::process::Stdio::null());
    cmd.spawn().map_err(|e| format!("cannot start the test tone ({e})"))
}

/// Au demarrage du serveur : la capacite (une ligne dans la console) et, avec --test-tone, la tonalite d'essai.
pub fn server_start(raw: &[String], demo: bool) {
    if demo {
        println!("Audio return (no cable): demo sine (440 Hz, -20 dBFS)");
        return;
    }
    #[cfg(windows)]
    let (build, ubr) = wasapi::windows_build();
    #[cfg(not(windows))]
    let (build, ubr) = (0u32, 0u32);
    match available() {
        Ok(ms) => println!("Audio return (no cable): available — Windows build {build}.{ubr} captures a single app (checked in {ms:.0} ms)"),
        Err(e) => println!("Audio return (no cable): NOT available on Windows build {build}.{ubr} ({e}) — use the virtual cable"),
    }
    if raw.iter().any(|a| a == "--test-tone") {
        match spawn_tone(raw, 24.0 * 3600.0) {
            Ok(c) => {
                let pid = c.id();
                let _ = TEST_TONE.set(pid);
                println!("TEST TONE: pid {pid} (the studio can ask for target \"test-tone\")");
                // le processus fils vit seul (il s'arrete quand le compagnon n'est plus la)
                std::mem::forget(c);
            }
            Err(e) => eprintln!("{e}"),
        }
    }
}

/// --tone / --capture-test (sans serveur) → le code de sortie.
pub fn main(raw: &[String]) -> i32 {
    match run(raw) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{e}");
            2
        }
    }
}

fn run(raw: &[String]) -> Result<i32, String> {
    let seconds = num(raw, "--seconds")?.unwrap_or(10.0).clamp(0.5, 24.0 * 3600.0);
    if raw.iter().any(|a| a == "--tone") {
        return tone_main(raw, seconds);
    }
    let target = value(raw, "--capture-test").ok_or("--capture-test needs a process (pid, name, or \"tone\")")?.to_string();
    capture_test(raw, &target, seconds)
}

#[cfg(not(windows))]
fn tone_main(_raw: &[String], _seconds: f64) -> Result<i32, String> {
    Err("--tone needs Windows".into())
}

#[cfg(windows)]
fn tone_main(raw: &[String], seconds: f64) -> Result<i32, String> {
    let o = wasapi::ToneOpts {
        seconds,
        hz: num(raw, "--tone-hz")?.unwrap_or(440.0).clamp(20.0, 20000.0),
        db: if raw.iter().any(|a| a == "--no-sine") { None } else { Some(num(raw, "--tone-db")?.unwrap_or(-30.0).clamp(-90.0, -6.0)) },
        click_ms: num(raw, "--click-ms")?.map(|c| c.clamp(50.0, 10_000.0)),
        click_db: num(raw, "--click-db")?.unwrap_or(-12.0).clamp(-60.0, -3.0),
        click_log: value(raw, "--click-log").map(std::path::PathBuf::from),
        device: value(raw, "--tone-device").map(|s| s.to_string()),
        mute: raw.iter().any(|a| a == "--tone-mute"),
        volume: num(raw, "--tone-volume")?.map(|v| v as f32),
        unmute: raw.iter().any(|a| a == "--tone-unmute"),
        parent: value(raw, "--parent").and_then(|p| p.parse().ok()),
        delay: num(raw, "--tone-delay")?.unwrap_or(0.0).max(0.0),
    };
    println!(
        "Test tone: {} for {seconds:.0} s{}{}{} (pid {})",
        o.db.map(|d| format!("{:.0} Hz sine at {d:.0} dBFS", o.hz)).unwrap_or_else(|| "no sine".into()),
        o.click_ms.map(|c| format!(", a click every {c:.0} ms at {:.0} dBFS", o.click_db)).unwrap_or_default(),
        if o.mute { ", its own session muted" } else { "" },
        o.volume.map(|v| format!(", its own session volume {v:.2}")).unwrap_or_default(),
        std::process::id()
    );
    let r = wasapi::tone(&o)?;
    println!(
        "{}",
        json!({ "tone": "done", "device": r.device, "frames": r.frames, "clicks": r.clicks, "bufferFrames": r.buffer_frames, "underruns": r.underruns,
            "sessionBefore": { "muted": r.session_before.0, "volume": r.session_before.1 }, "session": { "muted": r.session.0, "volume": r.session.1 } })
    );
    Ok(0)
}

#[cfg(not(windows))]
fn capture_test(_raw: &[String], _target: &str, _seconds: f64) -> Result<i32, String> {
    Err("--capture-test needs Windows".into())
}

/// Une attaque (un clic de la tonalite) dans ce qui a ete capture : l'instant de capture de son premier echantillon.
pub fn onsets(mono: &[f32], times: &[f64], thr: f32, quiet_frames: usize) -> Vec<f64> {
    let mut out = Vec::new();
    let mut quiet = quiet_frames;
    for (i, v) in mono.iter().enumerate() {
        if v.abs() >= thr {
            if quiet >= quiet_frames {
                out.push(times[i]);
            }
            quiet = 0;
        } else {
            quiet += 1;
        }
    }
    out
}

#[cfg(windows)]
fn capture_test(raw: &[String], target: &str, seconds: f64) -> Result<i32, String> {
    use packet::{db, Cadence, Levels};
    let rate = num(raw, "--rate")?.map(|r| r as u32).unwrap_or(48000);
    let tree = !raw.iter().any(|a| a == "--exclude-tree");
    let (build, ubr) = wasapi::windows_build();
    println!("Windows build {build}.{ubr}; default output: {}", wasapi::default_output_name());
    // « tone » : la tonalite d'essai d'abord (un autre processus), avec un journal de ses clics
    let mut child = None;
    let mut click_log = None;
    let target_pid = if target == "tone" {
        let mut args: Vec<String> = raw.to_vec();
        if num(raw, "--click-ms")?.is_some() && value(raw, "--click-log").is_none() {
            let p = std::env::temp_dir().join(format!("uwc-clicks-{}.jsonl", std::process::id()));
            args.push("--click-log".into());
            args.push(p.display().to_string());
            click_log = Some(p);
        } else {
            click_log = value(raw, "--click-log").map(std::path::PathBuf::from);
        }
        let c = spawn_tone(&args, seconds + 3.0)?;
        let pid = c.id();
        println!("Started the test tone: pid {pid} ({})", tone_args(&args).join(" "));
        child = Some(c);
        std::thread::sleep(Duration::from_millis(400));
        pid
    } else {
        let all = procs::list();
        let p = procs::pick(target, &all, true, None)?;
        println!("Target: {} (pid {}){}", p.exe, p.pid, if procs::daw_of_exe(&p.exe).is_some() { " — a DAW: levels only, nothing is kept" } else { "" });
        p.pid
    };
    let exe = procs::list().into_iter().find(|p| p.pid == target_pid).map(|p| p.exe).unwrap_or_default();
    let t_open = Instant::now();
    // (diagnostic) --via-output NOM : la boucle classique de cette sortie (tout ce qui y joue) au lieu de la capture du
    // processus — la reference de latence (la tonalite y joue avec --tone-device NOM)
    let via = value(raw, "--via-output").map(|s| s.to_string());
    if let Some(v) = &via {
        println!("REFERENCE: the whole output \"{v}\" (classic loopback), not the process");
    }
    let opened = match &via {
        Some(v) => wasapi::Loopback::open_output(v, rate),
        None => wasapi::Loopback::open(target_pid, tree, rate),
    };
    let mut lb = match opened {
        Ok(l) => l,
        Err(e) => {
            println!("{}", json!({ "worked": false, "windows": format!("{build}.{ubr}"), "target": { "pid": target_pid, "exe": exe }, "error": e }));
            if let Some(mut c) = child {
                let _ = c.kill();
            }
            return Ok(1);
        }
    };
    println!(
        "Process loopback opened in {:.1} ms (activation {:.1} ms, Initialize {:.1} ms): {rate} Hz, 2 ch, {} from Windows, buffer {} frames, include child processes: {tree}",
        t_open.elapsed().as_secs_f64() * 1000.0,
        lb.activate_ms,
        lb.init_ms,
        if lb.float { "float32" } else { "int16" },
        lb.buffer_frames
    );
    lb.start()?;
    wasapi::pro_audio_thread();
    let t0 = Instant::now();
    let mut first_data: Option<f64> = None;
    let mut first_sound: Option<f64> = None;
    let mut cadence = Cadence::new(25.0);
    let mut per_sec = Levels::new(2);
    let mut total = Levels::new(2);
    let (mut chunks, mut silent_chunks, mut frames, mut sec_chunks, mut discont) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut sizes: std::collections::BTreeMap<usize, u64> = Default::default();
    let mut latency_ms = Vec::new(); // capture → lu par le compagnon (l'age du morceau a sa lecture)
    let keep = child.is_some() && click_log.is_some();
    let (mut mono, mut times): (Vec<f32>, Vec<f64>) = (Vec::new(), Vec::new());
    let mut next_report = 1.0;
    let mut err = None;
    while t0.elapsed().as_secs_f64() < seconds {
        let r = lb.read(Duration::from_millis(100), &mut |c| {
            let now = epoch_ms();
            let el = t0.elapsed().as_secs_f64() * 1000.0;
            first_data.get_or_insert(el);
            cadence.tick(wasapi::qpc_ms());
            chunks += 1;
            sec_chunks += 1;
            frames += c.frames as u64;
            *sizes.entry(c.frames).or_default() += 1;
            if c.discontinuity {
                discont += 1;
            }
            latency_ms.push(now - c.t_ms);
            if c.silent {
                silent_chunks += 1;
                per_sec.add_silence(c.frames);
                total.add_silence(c.frames);
            } else {
                per_sec.add(c.samples);
                total.add(c.samples);
                if first_sound.is_none() && c.samples.iter().any(|v| v.abs() > 1e-5) {
                    first_sound = Some(el);
                }
            }
            if keep {
                for i in 0..c.frames {
                    let v = if c.silent { 0.0 } else { c.samples[i * 2].abs().max(c.samples[i * 2 + 1].abs()) };
                    mono.push(v);
                    times.push(c.t_ms + i as f64 * 1000.0 / rate as f64);
                }
            }
        });
        if let Err(e) = r {
            err = Some(e);
            break;
        }
        let el = t0.elapsed().as_secs_f64();
        if el >= next_report {
            let d = |v: f64| db(v).map(|x| format!("{x:6.1}")).unwrap_or_else(|| "  -inf".into());
            println!(
                "  {:4.0} s  {sec_chunks:3} chunks  L peak {} rms {} dBFS   R peak {} rms {} dBFS",
                next_report,
                d(per_sec.peak(0)),
                d(per_sec.rms(0)),
                d(per_sec.peak(1)),
                d(per_sec.rms(1))
            );
            per_sec.reset();
            sec_chunks = 0;
            next_report += 1.0;
        }
    }
    drop(lb);
    if let Some(mut c) = child {
        let _ = c.kill();
        let _ = c.wait();
    }
    let s = t0.elapsed().as_secs_f64();
    latency_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |v: &[f64], p: f64| if v.is_empty() { None } else { Some((v[((v.len() - 1) as f64 * p).round() as usize] * 10.0).round() / 10.0) };
    // les clics : de « doit sortir » (la tonalite) a « capture » (Windows), apparies au plus proche
    let mut clicks = json!(null);
    if let Some(p) = &click_log {
        let lines = std::fs::read_to_string(p).unwrap_or_default();
        let due: Vec<(f64, f64)> = lines
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter_map(|v| Some((v["due"].as_f64()?, v["written"].as_f64()?)))
            .collect();
        let click_db = num(raw, "--click-db")?.unwrap_or(-12.0);
        let found = onsets(&mono, &times, (packet::amp(click_db) * 0.5) as f32, (rate / 20) as usize);
        let mut d_due = Vec::new();
        let mut d_written = Vec::new();
        for t in &found {
            if let Some((du, wr)) = due.iter().min_by(|a, b| (a.0 - t).abs().partial_cmp(&(b.0 - t).abs()).unwrap()) {
                if (du - t).abs() < 250.0 {
                    d_due.push(t - du);
                    d_written.push(t - wr);
                }
            }
        }
        d_due.sort_by(|a, b| a.partial_cmp(b).unwrap());
        d_written.sort_by(|a, b| a.partial_cmp(b).unwrap());
        clicks = json!({
            "written": due.len(), "found": found.len(), "matched": d_due.len(),
            "captured_minus_due_ms": { "median": pct(&d_due, 0.5), "min": pct(&d_due, 0.0), "max": pct(&d_due, 1.0) },
            "captured_minus_written_ms": { "median": pct(&d_written, 0.5), "min": pct(&d_written, 0.0), "max": pct(&d_written, 1.0) },
        });
        if value(raw, "--click-log").is_none() {
            let _ = std::fs::remove_file(p);
        }
    }
    let worked = err.is_none() && chunks > 0;
    let heard = total.peak(0).max(total.peak(1)) > 1e-5;
    let r = |v: f64| (v * 10.0).round() / 10.0;
    let common = sizes.iter().max_by_key(|(_, n)| **n).map(|(k, _)| *k);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "worked": worked, "heard": heard, "windows": format!("{build}.{ubr}"), "via": via.as_deref().unwrap_or("process loopback"),
            "target": { "pid": target_pid, "exe": exe, "includeChildren": tree },
            "format": { "rate": rate, "channels": 2 },
            "seconds": r(s), "firstChunkMs": first_data.map(r), "firstSoundMs": first_sound.map(r),
            "chunks": chunks, "silentChunks": silent_chunks, "frames": frames, "framesPerChunk": common,
            "chunkSizes": sizes.iter().map(|(k, n)| format!("{k}×{n}")).collect::<Vec<_>>(),
            "discontinuities": discont,
            "cadenceMs": { "mean": r(cadence.mean()), "min": if cadence.n > 0 { Some(r(cadence.min)) } else { None }, "max": r(cadence.max), "sd": r(cadence.stddev()), "over25ms": cadence.late },
            "ageAtReadMs": { "median": pct(&latency_ms, 0.5), "p90": pct(&latency_ms, 0.9), "max": pct(&latency_ms, 1.0) },
            "peakDb": [db(total.peak(0)), db(total.peak(1))], "rmsDb": [db(total.rms(0)), db(total.rms(1))],
            "clicks": clicks,
            "error": err,
        }))
        .unwrap()
    );
    Ok(if worked { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_options_pass_through() {
        let raw: Vec<String> = ["--capture-test", "tone", "--seconds", "3", "--tone-db", "-40", "--tone-mute", "--click-ms", "500", "--port", "47411"].iter().map(|s| s.to_string()).collect();
        assert_eq!(tone_args(&raw), vec!["--tone-db", "-40", "--tone-mute", "--click-ms", "500"]);
        assert_eq!(is_server_flag("--test-tone"), Some(false));
        assert_eq!(is_server_flag("--click-log"), Some(true));
        assert_eq!(is_server_flag("--port"), None);
    }

    #[test]
    fn click_onsets() {
        // deux clics (0.25) dans du silence, a 1000 et 2500 trames ; un rebond juste apres le premier est le meme clic
        let mut m = vec![0.0f32; 3000];
        let t: Vec<f64> = (0..3000).map(|i| i as f64).collect();
        for i in 1000..1048 {
            m[i] = 0.25;
        }
        m[1100] = 0.3;
        for i in 2500..2548 {
            m[i] = 0.25;
        }
        assert_eq!(onsets(&m, &t, 0.12, 240), vec![1000.0, 2500.0]);
    }
}
