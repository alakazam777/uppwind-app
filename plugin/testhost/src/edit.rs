// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (19 nov.) L'essai « le compagnon comme hote VST3 » (a-lire/hote-vst3-compagnon-19nov.md, etape 0), Windows :
//!
//!   uwl-host edit --bundle "C:\Program Files\Common Files\VST3\Diva(x64).vst3" [--class NOM] [--seconds 60]
//!                 [--rate 48000] [--block 256] [--every-ms 700] [--len-ms 350] [--window front|behind|offscreen|cloak]
//!                 [--cmd-file F] [--out F.json] [--no-notes] [--state-file F [--link-bundle B]]
//!       charge le plugin comme un DAW (controleur a part, messages, etats), ouvre son editeur dans une fenetre a nous
//!       (titree « <plugin>/Essai », comme celles de Live), le joue depuis un fil cadence comme une carte son (une note
//!       toutes les --every-ms) sans carte son : rien ne sort des haut-parleurs. Mesure : les temps de chargement, le
//!       cout de process(), les notes entendues (attaque, crete), l'etat (taille, aller-retour), le CPU, la memoire.
//!       --cmd-file : une ligne ajoutee au fichier = un ordre (front, behind, offscreen, cloak, uncloak, save, reload,
//!       quit) ; la fenetre change de place pendant que `uppwind-companion --bench hwnd:<n>` la filme.
//!       --state-file F : la chaine du compagnon d'essai, Uppwind Link Notes → l'instrument → Uppwind Link Return
//!       (le bundle installe, --link-bundle sinon) ; les notes viennent alors du studio (`uwl-host e2e`), pas du motif.
//!   uwl-host e2e --state-file F [--seconds 20] [--every-ms 700] [--len-ms 300] [--lead-ms 40] [--out F.json]
//!       le studio de l'essai : appaire avec le compagnon d'essai, demande le son du Return, envoie des notes au Notes
//!       (chacune pour un instant donne) et cherche leur attaque dans le son qui revient : produite (l'instant de
//!       l'echantillon, moins l'instant demande), transport (son arrivee ici, moins l'instant de l'echantillon).

use crate::realhost::{classes, note_event, size_window, Engine, Frame, Instance};
use crate::vsthost::{Block, Module};
use crate::{arg, epoch_ms, num, raise_priority, wait_until, Stats, Wall};
use serde_json::{json, Value};
use std::ffi::c_void;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{ComPtr, ComWrapper};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

struct SendBox<T>(T);
unsafe impl<T> Send for SendBox<T> {}

fn ev(v: Value) {
    println!("{v}");
    let _ = std::io::stdout().flush();
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 10000.0).round() / 10.0
}

/// La vue attachee (le fil de l'interface seulement) : WM_SIZE lui passe la nouvelle taille (IPlugView::onSize), comme un DAW.
static VIEW: std::sync::atomic::AtomicPtr<IPlugView> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_SIZE {
        let v = VIEW.load(Ordering::Relaxed);
        if let Some(v) = vst3::ComRef::from_raw(v) {
            let (w, h) = ((lp & 0xffff) as i32, ((lp >> 16) & 0xffff) as i32);
            let mut cur: ViewRect = std::mem::zeroed();
            v.getSize(&mut cur);
            if w > 0 && h > 0 && (cur.right - cur.left != w || cur.bottom - cur.top != h) {
                let mut r = ViewRect { left: 0, top: 0, right: w, bottom: h };
                v.onSize(&mut r);
            }
        }
    }
    if msg == WM_CLOSE {
        // la croix ne detruit pas la fenetre (le plugin y est attache) : elle demande l'arret
        PostMessageW(hwnd, WM_APP + 1, 0, 0);
        return 0;
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn make_window(title: &str, tool: bool, owned: bool) -> Result<HWND, String> {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    unsafe {
        let inst = GetModuleHandleW(std::ptr::null());
        let class = wide("UppwindPlugHostEssai");
        let wc = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: inst,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        let t = wide(title);
        // --owned : une fenetre proprietaire cachee (une fenetre possedee n'a pas de bouton dans la barre des taches)
        let owner = if owned {
            let o = wide("owner");
            CreateWindowExW(0, class.as_ptr(), o.as_ptr(), WS_OVERLAPPED, 0, 0, 10, 10, std::ptr::null_mut(), std::ptr::null_mut(), inst, std::ptr::null())
        } else {
            std::ptr::null_mut()
        };
        let hwnd = CreateWindowExW(if tool { WS_EX_TOOLWINDOW } else { 0 }, class.as_ptr(), t.as_ptr(), WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN, 120, 120, 800, 500, owner, std::ptr::null_mut(), inst, std::ptr::null());
        if hwnd.is_null() {
            return Err("CreateWindowExW failed".into());
        }
        Ok(hwnd)
    }
}

/// La place de la fenetre : devant (a l'ecran, au-dessus), derriere (a l'ecran, sous tout), hors de l'ecran, masquee
/// par DWM (cloak : a l'ecran pour Windows, invisible), ou plus masquee.
fn apply_mode(hwnd: HWND, mode: &str) -> Result<(), String> {
    use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
    unsafe {
        let cloak = |on: i32| DwmSetWindowAttribute(hwnd, DWMWA_CLOAK as u32, &on as *const i32 as *const c_void, 4);
        match mode {
            "front" => {
                cloak(0);
                SetWindowPos(hwnd, HWND_TOP, 120, 120, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
            }
            "behind" => {
                cloak(0);
                SetWindowPos(hwnd, HWND_BOTTOM, 120, 120, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
            }
            "offscreen" => {
                cloak(0);
                let x = GetSystemMetrics(SM_XVIRTUALSCREEN) + GetSystemMetrics(SM_CXVIRTUALSCREEN) + 200;
                SetWindowPos(hwnd, HWND_BOTTOM, x, 120, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
            }
            "cloak" => {
                SetWindowPos(hwnd, HWND_BOTTOM, 120, 120, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
                let hr = cloak(1);
                if hr != 0 {
                    return Err(format!("DWMWA_CLOAK failed (0x{hr:08x})"));
                }
            }
            "uncloak" => {
                cloak(0);
            }
            _ => return Err(format!("unknown window mode {mode}")),
        }
    }
    Ok(())
}

/// Le CPU (utilisateur + noyau, en ms) et la memoire (Mo : courante, crete) de ce processus.
fn process_usage() -> (f64, f64, f64) {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    unsafe {
        let h = GetCurrentProcess();
        let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut e, mut k, mut u) = (z, z, z, z);
        GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u);
        let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 / 10_000.0;
        let mut m: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        m.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        GetProcessMemoryInfo(h, &mut m, m.cb);
        (ft(k) + ft(u), m.WorkingSetSize as f64 / 1048576.0, m.PeakWorkingSetSize as f64 / 1048576.0)
    }
}

/// Ce que le fil audio mesure.
#[derive(Default)]
struct Meter {
    blocks: u64,
    fails: u64,
    cost_us: Stats,
    late_ms: Stats,
    notes: u64,
    heard: u64,
    onset_ms: Stats,
    peak_db: Stats,
    params_applied: u64,
    out_peak: f32,
}

struct Note {
    at: u64,
    onset: Option<u64>,
    peak: f32,
}

fn db(x: f32) -> f64 {
    if x > 0.0 {
        20.0 * (x as f64).log10()
    } else {
        -200.0
    }
}

pub fn run_edit(a: &[String]) -> Result<(), String> {
    use windows_sys::Win32::System::Ole::OleInitialize;
    use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        OleInitialize(std::ptr::null());
    }
    let bundle = PathBuf::from(arg(a, "--bundle").ok_or("--bundle PATH.vst3")?);
    let want = arg(a, "--class").map(|s| s.to_lowercase());
    let seconds = num(a, "--seconds", 60.0)?;
    let rate = num(a, "--rate", 48000.0)?;
    let block = num(a, "--block", 256.0)? as usize;
    let every = num(a, "--every-ms", 700.0)?;
    let len = num(a, "--len-ms", 350.0)?;
    let mode0 = arg(a, "--window").unwrap_or_else(|| "front".into());
    let cmd_file = arg(a, "--cmd-file");
    let out_file = arg(a, "--out");
    let no_notes = a.iter().any(|x| x == "--no-notes");
    let chain = arg(a, "--state-file").is_some();
    // (19 nov., les 23 ms) l'heure systeme passee aux plugins Uppwind Link de la chaine : 0 (comme les bancs return et
    // notes) ou l'heure de Windows du bloc (« wall »)
    let systime_wall = arg(a, "--systime").map(|s| s == "wall").unwrap_or(false);
    // --click : le Return recoit un clic de 2 ms a l'echantillon ou le Notes a place chaque note, au lieu du son de
    // l'instrument (la latence de la chaine seule, sans l'attaque du son)
    let click = a.iter().any(|x| x == "--click");
    let link_bundle = PathBuf::from(arg(a, "--link-bundle").unwrap_or_else(|| r"C:\Program Files\Common Files\VST3\Uppwind Link.vst3".into()));
    let mut errors: Vec<String> = Vec::new();

    // ── le module, la classe ──
    let t = Instant::now();
    let m = Module::load(&bundle)?;
    let load_ms = ms(t.elapsed());
    let all = classes(&m);
    let list: Vec<Value> = all.iter().map(|c| json!({ "category": c.category, "name": c.name, "vendor": c.vendor, "version": c.version, "sub": c.sub })).collect();
    let pick = all
        .iter()
        .find(|c| c.category == "Audio Module Class" && want.as_ref().map(|w| c.name.to_lowercase().contains(w)).unwrap_or(true))
        .cloned()
        .ok_or_else(|| format!("no audio class{} in {}", want.as_ref().map(|w| format!(" matching \"{w}\"")).unwrap_or_default(), bundle.display()))?;
    ev(json!({ "event": "module", "loadMs": load_ms, "classes": list, "picked": pick.name }));
    let instrument = pick.sub.contains("Instrument");

    // ── l'instance ──
    let t = Instant::now();
    let mut inst = Instance::create(&m, &pick.cid)?;
    let create_ms = ms(t.elapsed());
    let t = Instant::now();
    inst.start(rate, block as i32)?;
    let start_ms = ms(t.elapsed());
    ev(json!({ "event": "instance", "createMs": create_ms, "startMs": start_ms, "separateController": inst.separate, "controller": inst.ctrl.is_some(),
               "params": inst.param_count(), "audioIns": inst.ins, "audioOuts": inst.outs, "eventIns": inst.event_ins, "latencySamples": inst.latency }));

    // ── l'etat, avant l'editeur ──
    let state0 = match inst.get_state() {
        Ok((c, k)) => Some((c, k)),
        Err(e) => {
            errors.push(e);
            None
        }
    };

    // ── la fenetre et l'editeur ──
    let title = format!("{}/Essai", pick.name);
    // --style normal : sans WS_EX_TOOLWINDOW (Windows.Graphics.Capture refuse les fenetres d'outil)
    let tool = arg(a, "--style").map(|s| s != "normal").unwrap_or(true);
    let owned = a.iter().any(|x| x == "--owned");
    let hwnd = make_window(&title, tool, owned)?;
    let frame = ComWrapper::new(Frame { hwnd: AtomicIsize::new(hwnd as isize), resizes: AtomicU32::new(0) });
    let t = Instant::now();
    let mut view: Option<ComPtr<IPlugView>> = None;
    let mut view_err = None;
    let (mut vw, mut vh) = (0, 0);
    if let Some(c) = &inst.ctrl {
        let raw = unsafe { c.createView(ViewType::kEditor) };
        match unsafe { ComPtr::from_raw(raw) } {
            None => view_err = Some("createView(editor) returned null".to_string()),
            Some(v) => unsafe {
                if v.isPlatformTypeSupported(kPlatformTypeHWND) != kResultTrue {
                    view_err = Some("the view does not support HWND".into());
                } else {
                    let fp = frame.to_com_ptr::<IPlugFrame>().unwrap();
                    v.setFrame(fp.as_ptr());
                    if let Some(s) = v.cast::<IPlugViewContentScaleSupport>() {
                        let dpi = GetDpiForWindow(hwnd).max(96);
                        s.setContentScaleFactor(dpi as f32 / 96.0);
                    }
                    let mut r: ViewRect = std::mem::zeroed();
                    v.getSize(&mut r);
                    size_window(hwnd as isize, (r.right - r.left).max(200), (r.bottom - r.top).max(100));
                    // (19 nov., Serum 2 noir) la fenetre visible AVANT attached, puis onSize : comme un DAW
                    if let Err(e) = apply_mode(hwnd, &mode0) {
                        errors.push(e);
                    }
                    let rr = v.attached(hwnd as *mut c_void, kPlatformTypeHWND);
                    VIEW.store(v.as_ptr(), Ordering::Relaxed);
                    if rr != kResultOk {
                        view_err = Some(format!("attached failed ({rr})"));
                    }
                    let mut r: ViewRect = std::mem::zeroed();
                    v.getSize(&mut r);
                    vw = r.right - r.left;
                    vh = r.bottom - r.top;
                    if vw > 0 && vh > 0 {
                        size_window(hwnd as isize, vw, vh);
                        let mut r2 = ViewRect { left: 0, top: 0, right: vw, bottom: vh };
                        v.onSize(&mut r2);
                    }
                    view = Some(v);
                }
            },
        }
    } else {
        view_err = Some("no edit controller".into());
    }
    let view_ms = ms(t.elapsed());
    ev(json!({ "event": "window", "hwnd": hwnd as isize, "tool": tool, "owned": owned, "title": title, "viewMs": view_ms, "w": vw, "h": vh, "mode": mode0, "error": view_err }));

    // ── le fil audio ──
    let stop = Arc::new(AtomicBool::new(false));
    let meter = Arc::new(Mutex::new(Meter::default()));
    let proc_ptr = SendBox(inst.processor.as_ptr());
    let handler_ptr = SendBox(&*inst.handler as *const crate::realhost::Handler);
    let (ins, outs) = (inst.ins.clone(), inst.outs.clone());
    // la chaine (le compagnon d'essai) : nos plugins, crees ici, passes au fil audio
    let link = if chain {
        let lm = Module::load(&link_bundle)?;
        let mut notes = lm.create("Uppwind Link Notes")?;
        notes.start(rate, block as i32, false)?;
        let mut ret = lm.create("Uppwind Link Return")?;
        ret.start(rate, block as i32, true)?;
        // le module en dernier : il est libere apres les plugins (l'ordre des champs du tuple)
        Some(SendBox((notes, ret, lm)))
    } else {
        None
    };
    let (stop2, meter2) = (stop.clone(), meter.clone());
    let audio = std::thread::spawn(move || {
        let proc_ptr = proc_ptr;
        let handler_ptr = handler_ptr;
        let link = link;
        let p = proc_ptr.0;
        let h = unsafe { &*handler_ptr.0 };
        let mut eng = Engine::new(&ins, &outs, block, rate);
        let mut lb = link.as_ref().map(|_| (Block::new(block, rate), Block::new(block, rate)));
        raise_priority();
        let wall = Wall::now();
        let start = Instant::now() + Duration::from_millis(150);
        let every_s = (every / 1000.0 * rate) as u64;
        let len_s = (len / 1000.0 * rate) as u64;
        let pitches = [48i16, 55, 60, 64, 67, 72];
        let mut notes: Vec<Note> = Vec::new();
        let mut k = 0u64;
        let mut params: Vec<(u32, f64)> = Vec::new();
        let mut burst_left = 0usize;
        let mut ons_in_block: Vec<usize> = Vec::new();
        while !stop2.load(Ordering::Relaxed) {
            let pos = k * block as u64;
            let ideal = start + Duration::from_secs_f64(pos as f64 / rate);
            wait_until(ideal);
            let call = Instant::now();
            // les notes : le motif, ou celles du studio par le plugin Notes
            if let (Some(l), Some((nb, _))) = (link.as_ref(), lb.as_mut()) {
                let ns0 = if systime_wall { (wall.of(call) * 1e6) as i64 } else { 0 };
                nb.process(&l.0 .0, block, false, pos as i64, ns0);
                let mut evs = eng.in_events.events.lock().unwrap();
                for e in nb.out_events.events.lock().unwrap().iter() {
                    let mut e2 = *e;
                    e2.busIndex = 0;
                    if e.r#type == Event_::EventTypes_::kNoteOnEvent as u16 {
                        notes.push(Note { at: pos + e.sampleOffset.max(0) as u64, onset: None, peak: 0.0 });
                        ons_in_block.push(e.sampleOffset.max(0) as usize);
                    }
                    evs.push(e2);
                }
            } else if !no_notes {
                let mut evs = eng.in_events.events.lock().unwrap();
                for i in 0..block as u64 {
                    let s = pos + i;
                    if s % every_s == 0 {
                        let n = s / every_s;
                        let pitch = pitches[(n as usize) % pitches.len()];
                        evs.push(note_event(true, i as i32, pitch, 0.8, n as i32));
                        notes.push(Note { at: s, onset: None, peak: 0.0 });
                    }
                    if s % every_s == len_s && s >= len_s {
                        let n = (s - len_s) / every_s;
                        let pitch = pitches[(n as usize) % pitches.len()];
                        evs.push(note_event(false, i as i32, pitch, 0.0, n as i32));
                    }
                }
            }
            params.clear();
            {
                let mut pend = h.pending.lock().unwrap();
                params.append(&mut pend);
            }
            let ns = (wall.of(call) * 1e6) as i64;
            let r = eng.process(p, block, pos as i64, ns, &params);
            let cost = call.elapsed();
            let mut mt = meter2.lock().unwrap();
            mt.blocks += 1;
            mt.params_applied += params.len() as u64;
            if r != kResultOk {
                mt.fails += 1;
            }
            mt.cost_us.push(cost.as_secs_f64() * 1e6);
            mt.late_ms.push((call - ideal).as_secs_f64() * 1000.0);
            let (l, rr) = eng.main_out();
            for i in 0..block.min(l.len()) {
                let x = l[i].abs().max(rr[i].abs());
                mt.out_peak = mt.out_peak.max(x);
                let s = pos + i as u64;
                for nt in notes.iter_mut().rev().take(4) {
                    if s >= nt.at && s < nt.at + (0.3 * rate) as u64 {
                        nt.peak = nt.peak.max(x);
                        if nt.onset.is_none() && x > 1e-3 {
                            nt.onset = Some(s - nt.at);
                        }
                    }
                }
            }
            // le son de l'instrument au Return (vers le compagnon)
            if let (Some(l2), Some((_, rb))) = (link.as_ref(), lb.as_mut()) {
                if click {
                    for i in 0..block {
                        if ons_in_block.contains(&i) {
                            burst_left = (rate * 0.002) as usize;
                        }
                        let v = if burst_left > 0 {
                            burst_left -= 1;
                            0.5
                        } else {
                            0.0
                        };
                        rb.inl[i] = v;
                        rb.inr[i] = v;
                    }
                } else {
                    let (ol, or) = eng.main_out();
                    rb.inl[..block].copy_from_slice(&ol[..block]);
                    rb.inr[..block].copy_from_slice(&or[..block]);
                }
                rb.process(&l2.0 .1, block, true, pos as i64, if systime_wall { ns } else { 0 });
            }
            drop(mt);
            ons_in_block.clear();
            k += 1;
        }
        let mut mt = meter2.lock().unwrap();
        for nt in &notes {
            if nt.at + (0.3 * rate) as u64 > k * block as u64 {
                continue;
            }
            mt.notes += 1;
            if let Some(o) = nt.onset {
                mt.heard += 1;
                mt.onset_ms.push(o as f64 * 1000.0 / rate);
                mt.peak_db.push(db(nt.peak));
            }
        }
        drop(mt);
        if let Some(mut l) = link {
            l.0 .0.stop();
            l.0 .1.stop();
        }
    });

    // ── la boucle de l'interface ──
    let t_run = Instant::now();
    let cpu0 = process_usage().0;
    let mut done_lines = 0usize;
    let mut saved: Option<(Vec<u8>, Option<Vec<u8>>)> = None;
    let mut saved_params: Vec<(u32, f64)> = Vec::new();
    let mut state_report: Vec<Value> = Vec::new();
    unsafe { SetTimer(hwnd, 1, 50, None) };
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    'ui: loop {
        let r = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if r <= 0 {
            break;
        }
        if msg.message == WM_APP + 1 {
            break;
        }
        if msg.message == WM_TIMER && msg.hwnd == hwnd {
            if t_run.elapsed().as_secs_f64() >= seconds {
                break;
            }
            if let Some(f) = &cmd_file {
                let lines: Vec<String> = std::fs::read_to_string(f).unwrap_or_default().lines().map(|s| s.trim().to_string()).collect();
                while done_lines < lines.len() {
                    let c = lines[done_lines].clone();
                    done_lines += 1;
                    let at = (t_run.elapsed().as_secs_f64() * 10.0).round() / 10.0;
                    match c.as_str() {
                        "" => {}
                        "quit" => break 'ui,
                        "save" => match inst.get_state() {
                            Ok(s) => {
                                ev(json!({ "event": "save", "at": at, "componentBytes": s.0.len(), "controllerBytes": s.1.as_ref().map(|b| b.len()) }));
                                saved = Some(s);
                                saved_params = inst.param_values();
                            }
                            Err(e) => ev(json!({ "event": "save", "at": at, "error": e })),
                        },
                        "reload" => {
                            if let Some((c0, k0)) = &saved {
                                let before = inst.param_values();
                                let changed_since_save = before.iter().zip(&saved_params).filter(|(a, b)| (a.1 - b.1).abs() > 1e-6).count();
                                let r = inst.set_state(c0, k0.as_deref());
                                let again = inst.get_state().ok();
                                let same = again.as_ref().map(|(c1, _)| c1 == c0);
                                let after = inst.param_values();
                                let differ = after.iter().zip(&saved_params).filter(|(a, b)| (a.1 - b.1).abs() > 1e-6).count();
                                ev(json!({ "event": "reload", "at": at, "ok": r.is_ok(), "error": r.err(), "componentBytesSame": same,
                                           "paramsChangedSinceSave": changed_since_save, "paramsDifferingAfterReload": differ, "params": after.len() }));
                            }
                        }
                        mode => {
                            let r = apply_mode(hwnd, mode);
                            ev(json!({ "event": "mode", "at": at, "mode": mode, "error": r.err() }));
                        }
                    }
                }
            }
            continue;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    let wall_s = t_run.elapsed().as_secs_f64();
    let (cpu1, ws_mb, peak_mb) = process_usage();

    // ── l'etat : aller-retour (le meme etat recharge, puis relu) ──
    if let Some((c0, k0)) = &state0 {
        let now = inst.get_state().ok();
        let r = inst.set_state(c0, k0.as_deref());
        let again = inst.get_state().ok();
        state_report.push(json!({
            "componentBytes": c0.len(), "controllerBytes": k0.as_ref().map(|b| b.len()),
            "componentBytesAtEnd": now.as_ref().map(|s| s.0.len()),
            "reloadOk": r.is_ok(), "reloadError": r.err(),
            "componentSameAfterReload": again.as_ref().map(|(c1, _)| c1 == c0),
        }));
    }

    stop.store(true, Ordering::Relaxed);
    let _ = audio.join();
    unsafe { KillTimer(hwnd, 1) };
    VIEW.store(std::ptr::null_mut(), Ordering::Relaxed);
    if let Some(v) = view.take() {
        unsafe {
            v.removed();
            v.setFrame(std::ptr::null_mut());
        }
        drop(v);
    }
    let h = &*inst.handler;
    let restarts = h.restarts.lock().unwrap().clone();
    let summary = {
        let mt = meter.lock().unwrap();
        let budget_us = block as f64 * 1e6 / rate;
        let cost = mt.cost_us.summary();
        json!({
            "event": "summary",
            "plugin": pick.name, "vendor": pick.vendor, "version": pick.version, "sub": pick.sub, "instrument": instrument,
            "bundle": bundle.display().to_string(),
            "loadMs": load_ms, "createMs": create_ms, "startMs": start_ms, "viewMs": view_ms, "viewError": view_err, "view": [vw, vh],
            "separateController": inst.separate, "params": inst.param_count(), "audioIns": inst.ins, "audioOuts": inst.outs, "latencySamples": inst.latency,
            "hostMessages": inst.host.messages.load(Ordering::Relaxed),
            "editorEdits": h.edits.load(Ordering::Relaxed), "editorGestures": h.gestures.load(Ordering::Relaxed), "paramsApplied": mt.params_applied, "restartFlags": restarts,
            "resizes": frame.resizes.load(Ordering::Relaxed),
            "rate": rate, "block": block, "blockBudgetUs": budget_us,
            "blocks": mt.blocks, "processFails": mt.fails, "processUs": cost,
            "processP99OfBudget": cost["p99"].as_f64().map(|x| (x / budget_us * 1000.0).round() / 10.0),
            "callbackLateMs": mt.late_ms.summary(),
            "notes": mt.notes, "heard": mt.heard, "onsetMs": mt.onset_ms.summary(), "notePeakDb": mt.peak_db.summary(), "outPeakDb": (db(mt.out_peak) * 10.0).round() / 10.0,
            "state": state_report,
            "cpuPercentOfOneCore": ((cpu1 - cpu0) / (wall_s * 1000.0) * 1000.0).round() / 10.0, "workingSetMb": (ws_mb * 10.0).round() / 10.0, "peakWorkingSetMb": (peak_mb * 10.0).round() / 10.0,
            "seconds": (wall_s * 10.0).round() / 10.0, "chain": chain, "errors": errors,
        })
    };
    ev(summary.clone());
    if let Some(f) = out_file {
        std::fs::write(&f, serde_json::to_string_pretty(&summary).unwrap()).map_err(|e| format!("{f}: {e}"))?;
    }
    inst.close();
    unsafe { DestroyWindow(hwnd) };
    drop(inst);
    drop(m);
    Ok(())
}

// ── Le studio de l'essai : note → son, de bout en bout ───────────────────────────────────────────────────────────

pub fn run_e2e(a: &[String]) -> Result<(), String> {
    use std::net::TcpStream;
    use tungstenite::client::IntoClientRequest;
    let path = crate::explicit_state_file(a)?;
    let st: Value = serde_json::from_str(&std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?).map_err(|e| e.to_string())?;
    let port = st["port"].as_u64().ok_or("no port in the state file")? as u16;
    if port == 47410 {
        return Err("47410 is the user's companion: use a test companion".into());
    }
    let token = st["token"].as_str().ok_or("no token")?.to_string();
    let seconds = num(a, "--seconds", 20.0)?;
    let every = num(a, "--every-ms", 700.0)?;
    let len = num(a, "--len-ms", 300.0)?;
    let lead = num(a, "--lead-ms", 40.0)?;
    let origin = arg(a, "--origin").unwrap_or_else(|| "http://localhost:5747".into());
    let mut req = format!("ws://127.0.0.1:{port}/").into_client_request().map_err(|e| e.to_string())?;
    req.headers_mut().insert("Origin", origin.parse().map_err(|_| "bad origin")?);
    let s = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("no companion on {port} ({e})"))?;
    let _ = s.set_nodelay(true);
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let (mut ws, _) = tungstenite::client(req, s).map_err(|e| format!("handshake: {e}"))?;
    let mut errors = Vec::new();
    let _h = crate::ws_pair(&mut ws, port, &token, &mut errors)?;
    // le Notes et le Return de l'hote d'essai
    let t0 = Instant::now();
    let (notes_id, ret_id) = loop {
        ws.send(tungstenite::Message::text(r#"{"type":"instances"}"#.to_string())).map_err(|e| e.to_string())?;
        let v = crate::ws_wait(&mut ws, "instances", &mut errors)?;
        let l = v["list"].as_array().cloned().unwrap_or_default();
        let n = l.iter().find(|i| i["kind"] == "notes").and_then(|i| i["id"].as_str().map(String::from));
        let r = l.iter().find(|i| i["kind"] == "return").and_then(|i| i["id"].as_str().map(String::from));
        if let (Some(n), Some(r)) = (n, r) {
            break (n, r);
        }
        if t0.elapsed() > Duration::from_secs(20) {
            return Err(format!("no Notes and Return connected: {v}"));
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    ws.send(tungstenite::Message::text(json!({ "type": "audio", "action": "start", "target": format!("plugin:{ret_id}"), "rate": 48000, "format": "f32", "ms": 5 }).to_string())).map_err(|e| e.to_string())?;
    ws.get_mut().set_read_timeout(Some(Duration::from_millis(2))).map_err(|e| e.to_string())?;
    // le son recu : (instant de l'echantillon, |x| max des deux canaux), et l'arrivee de chaque paquet
    let mut frames: Vec<(f64, f32, f64)> = Vec::with_capacity(48_000 * (seconds as usize + 5));
    let mut ages = Stats::default();
    let mut sent: Vec<f64> = Vec::new();
    let n_notes = (seconds * 1000.0 / every) as usize;
    let t_first = epoch_ms() + 1500.0;
    let mut next = 0usize;
    let mut state_on = false;
    let end = Instant::now() + Duration::from_secs_f64(seconds + 3.0);
    while Instant::now() < end {
        if next < n_notes && epoch_ms() >= t_first + next as f64 * every - lead {
            let t = t_first + next as f64 * every;
            let pitch = [48, 55, 60, 64, 67, 72][next % 6];
            let msg = json!({ "type": "midi", "to": notes_id, "events": [
                { "t": t, "d": [0x90, pitch, 100], "n": next }, { "t": t + len, "d": [0x80, pitch, 0], "n": 100_000 + next } ] });
            ws.send(tungstenite::Message::text(msg.to_string())).map_err(|e| e.to_string())?;
            sent.push(t);
            next += 1;
        }
        match ws.read() {
            Ok(tungstenite::Message::Binary(b)) if b.len() >= 36 && &b[..4] == b"UWA1" => {
                let arrived = epoch_ms();
                let t_ms = f64::from_le_bytes(b[16..24].try_into().unwrap());
                let rate = u32::from_le_bytes([b[24], b[25], b[26], b[27]]) as f64;
                let nfr = u16::from_le_bytes([b[28], b[29]]) as usize;
                let (ch, fmt) = (b[30] as usize, b[31]);
                ages.push(arrived - t_ms);
                let body = &b[36..];
                for i in 0..nfr {
                    let mut x = 0f32;
                    for c in 0..ch {
                        let j = i * ch + c;
                        let v = if fmt == 1 { body.get(4 * j..4 * j + 4).map(|s| f32::from_le_bytes(s.try_into().unwrap())).unwrap_or(0.0) } else { 0.0 };
                        x = x.max(v.abs());
                    }
                    frames.push((t_ms + i as f64 * 1000.0 / rate, x, arrived));
                }
            }
            Ok(tungstenite::Message::Text(t)) => {
                let v: Value = serde_json::from_str(t.as_str()).unwrap_or(Value::Null);
                if v["type"] == "audio" {
                    if v["state"] == "on" {
                        state_on = true;
                    } else if v["state"] == "off" {
                        errors.push(format!("audio off: {v}"));
                        break;
                    }
                } else if v["type"] == "error" {
                    errors.push(v.to_string());
                }
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(format!("websocket: {e}")),
        }
    }
    let _ = ws.send(tungstenite::Message::text(r#"{"type":"audio","action":"stop"}"#.to_string()));
    // (19 nov.) ou le Notes a place chaque note (son horloge : l'instant de l'echantillon), contre l'instant demande
    ws.get_mut().set_read_timeout(Some(Duration::from_secs(3))).map_err(|e| e.to_string())?;
    ws.send(tungstenite::Message::text(json!({ "type": "emitted", "id": notes_id }).to_string())).map_err(|e| e.to_string())?;
    let em = crate::ws_wait(&mut ws, "emitted", &mut errors).unwrap_or(Value::Null);
    let mut placed = Stats::default();
    let mut late_reported = Stats::default();
    for e in em["list"].as_array().cloned().unwrap_or_default() {
        let n = e["n"].as_u64().unwrap_or(u64::MAX) as usize;
        if let (Some(&t), Some(te)) = (sent.get(n), e["t"].as_f64()) {
            placed.push(te - t);
            late_reported.push(e["late"].as_f64().unwrap_or(0.0));
        }
    }
    // l'attaque au seuil fixe de 1e-3 (-60 dBFS), comme la mesure sans compagnon
    let mut onset_fixed = Stats::default();
    for &t in &sent {
        if let Some(f) = frames.iter().find(|f| f.0 >= t - 15.0 && f.0 < t + 250.0 && f.1 > 1e-3) {
            onset_fixed.push(f.0 - t);
        }
    }
    // l'attaque de chaque note : la premiere trame au-dessus de 3 fois le niveau d'avant (et de -50 dBFS)
    let (mut produced, mut transport, mut total) = (Stats::default(), Stats::default(), Stats::default());
    let mut missing = 0;
    for &t in &sent {
        let base = frames.iter().filter(|f| f.0 >= t - 60.0 && f.0 < t - 15.0).map(|f| f.1).fold(0f32, f32::max);
        let thr = (base * 3.0).max(0.00316);
        match frames.iter().find(|f| f.0 >= t - 15.0 && f.0 < t + 250.0 && f.1 > thr) {
            Some(f) => {
                produced.push(f.0 - t);
                transport.push(f.2 - f.0);
                total.push(f.2 - t);
            }
            None => missing += 1,
        }
    }
    let r = json!({ "event": "e2e", "notes": sent.len(), "missing": missing, "audioOn": state_on, "leadMs": lead,
                    "producedMs (sample time - requested)": produced.summary(), "transportMs (arrival here - sample time)": transport.summary(),
                    "totalMs (arrival here - requested)": total.summary(),
                    "placedMs (Notes: emitted sample time - requested)": placed.summary(), "notesLateMs": late_reported.summary(),
                    "onsetFixedMs (sample time at -60 dBFS - requested)": onset_fixed.summary(), "packetAgeMs": ages.summary(), "frames": frames.len(), "errors": errors });
    println!("{}", serde_json::to_string_pretty(&r).unwrap());
    if let Some(f) = arg(a, "--out") {
        std::fs::write(&f, serde_json::to_string_pretty(&r).unwrap()).map_err(|e| format!("{f}: {e}"))?;
    }
    let _ = ws.close(None);
    Ok(())
}
