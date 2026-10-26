// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! --bench TITLE (dev) : ou passe le temps entre un geste et son image, mesure sur une vraie fenetre de plugin.
//!
//! 1. Le cout de chaque facon de filmer la fenetre (PrintWindow complet, PrintWindow sans PW_RENDERFULLCONTENT,
//!    BitBlt depuis le DC de la fenetre, la surface DWM (capture::Dwm, recalee sur la zone client)), comparees pixel a pixel a PrintWindow ;
//!    le JPEG (image entiere, tuile) et la comparaison d'images.
//! 2. La cadence propre du plugin : la surface DWM lue toutes les 2 ms (sans toucher le fil du plugin), les instants
//!    ou l'image change.
//! 3. --bench-clicks "x,y;…" (pixels de l'image) : un clic en mode Direct, puis le temps jusqu'a la premiere image
//!    changee, par la surface DWM (le plugin lui-meme) et par PrintWindow (la voie du compagnon). A reserver a des
//!    gestes inoffensifs (les onglets du bas de Diva, en finissant par MAIN).
//! Rien n'est envoye ; --bench-out DIR ecrit une image JPEG de la fenetre (pour trouver les points a cliquer).

#[cfg(windows)]
pub use win::run;

#[cfg(windows)]
mod win {
    use crate::capture::{self, Frame};
    use crate::input::Injector;
    use crate::protocol::{InputEvent, PointerAction, Pt};
    use crate::{encode, plugins, stream};
    use serde_json::json;
    use std::time::{Duration, Instant};

    fn ms(d: Duration) -> f64 {
        (d.as_secs_f64() * 10000.0).round() / 10.0
    }

    fn stats(v: &mut [f64]) -> serde_json::Value {
        if v.is_empty() {
            return json!(null);
        }
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let n = v.len();
        let avg = v.iter().sum::<f64>() / n as f64;
        let r = |x: f64| (x * 10.0).round() / 10.0;
        json!({ "n": n, "min": r(v[0]), "med": r(v[n / 2]), "p90": r(v[(n * 9 / 10).min(n - 1)]), "max": r(v[n - 1]), "avg": r(avg) })
    }

    pub fn run(name: &str, clicks: &[(f64, f64)], out: Option<&str>, seconds: f64) -> i32 {
        let wins = plugins::plugin_windows(&plugins::enumerate());
        let needle = name.to_lowercase();
        let Some(w) = wins.iter().find(|w| w.title.to_lowercase().contains(&needle)) else {
            println!("No Live plugin window whose title contains \"{name}\"");
            return 2;
        };
        let hwnd = w.id;
        println!("bench \"{}\" ({}×{})", w.title, w.w, w.h);
        let mut print = match capture::open(hwnd, Some("print")) {
            Ok(c) => c,
            Err(e) => {
                println!("PrintWindow: {e}");
                return 1;
            }
        };
        let base = match print.grab() {
            Ok(Some(f)) => f,
            _ => {
                println!("PrintWindow gave nothing");
                return 1;
            }
        };
        println!("PrintWindow image {}×{} (display {:?})", base.w, base.h, print.display_size());
        if let Some(dir) = out {
            let _ = std::fs::create_dir_all(dir);
            let p = format!("{dir}/window.jpg");
            if let Ok(j) = encode::jpeg(&base, 90) {
                let _ = std::fs::write(&p, j);
                println!("wrote {p}");
            }
        }

        // ── 1. les couts ────────────────────────────────────────────────────────────────────────────────────
        let n = 40;
        let mut t_print = Vec::new();
        let mut t_pw_only = Vec::new();
        let mut t_dib = Vec::new();
        let mut t_pw_noflag = Vec::new();
        let mut t_blt = Vec::new();
        let mut t_dx = Vec::new();
        let mut t_dirty = Vec::new();
        let mut t_jpeg_full = Vec::new();
        let mut t_jpeg_tile = Vec::new();
        let mut t_pw_small = Vec::new();
        let mut eq = json!({});
        let mut dx = capture::open(hwnd, Some("dwm"));
        if let Err(e) = &dx {
            println!("DWM shared surface: {e}");
        }
        for i in 0..n {
            let t0 = Instant::now();
            let f = print.grab().ok().flatten();
            t_print.push(ms(t0.elapsed()));
            let (a, b) = raw_print(hwnd, true, None);
            t_pw_only.push(a);
            t_dib.push(b);
            let t0 = Instant::now();
            let nf = raw_frame(hwnd, false);
            t_pw_noflag.push(ms(t0.elapsed()));
            let t0 = Instant::now();
            let bf = bitblt(hwnd);
            t_blt.push(ms(t0.elapsed()));
            let (sa, _) = raw_print(hwnd, true, Some((200, 200, 400, 300)));
            t_pw_small.push(sa);
            let mut dxf = None;
            if let Ok(d) = dx.as_mut() {
                let t0 = Instant::now();
                dxf = d.grab().ok().flatten();
                t_dx.push(ms(t0.elapsed()));
            }
            if let Some(f) = &f {
                let t0 = Instant::now();
                let _ = stream::dirty_rect(f, &base);
                t_dirty.push(ms(t0.elapsed()));
                let t0 = Instant::now();
                let _ = encode::jpeg(f, 80);
                t_jpeg_full.push(ms(t0.elapsed()));
                let tile = stream::crop(f, (100, 100, 160, 160));
                let t0 = Instant::now();
                let _ = encode::jpeg(&tile, 62);
                t_jpeg_tile.push(ms(t0.elapsed()));
                if i == 0 {
                    eq = json!({
                        "noflag_vs_print": nf.as_ref().map(|x| same_part(x, f)),
                        "bitblt_vs_print": bf.as_ref().map(|x| same_part(x, f)),
                        "dx_vs_print": dxf.as_ref().map(|x| same_part(x, f)),
                        "dx_size": dxf.as_ref().map(|x| format!("{}x{}", x.w, x.h)),
                        "bitblt_size": bf.as_ref().map(|x| format!("{}x{}", x.w, x.h)),
                    });
                    if let (Some(dir), Some(x)) = (out, &dxf) {
                        let _ = std::fs::write(format!("{dir}/dx.jpg"), encode::jpeg(x, 90).unwrap_or_default());
                    }
                    if let (Some(dir), Some(x)) = (out, &bf) {
                        let _ = std::fs::write(format!("{dir}/bitblt.jpg"), encode::jpeg(x, 90).unwrap_or_default());
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        println!(
            "{}",
            json!({
                "costs_ms": {
                    "print_grab_total (companion: PrintWindow+GetDIBits+alpha+crop)": stats(&mut t_print),
                    "PrintWindow_call_only": stats(&mut t_pw_only),
                    "GetDIBits": stats(&mut t_dib),
                    "PrintWindow_no_RENDERFULLCONTENT_total": stats(&mut t_pw_noflag),
                    "PrintWindow_into_400x300_dc": stats(&mut t_pw_small),
                    "BitBlt_window_dc_total": stats(&mut t_blt),
                    "dwm_shared_surface_total": stats(&mut t_dx),
                    "dirty_rect_full": stats(&mut t_dirty),
                    "jpeg_full_q80": stats(&mut t_jpeg_full),
                    "jpeg_tile_160_q62": stats(&mut t_jpeg_tile),
                },
                "same_pixels": eq,
            })
        );

        // ── 2. la cadence du plugin (surface DWM, toutes les 2 ms) ──────────────────────────────────────────
        if let Ok(d) = dx.as_mut() {
            let mut prev = d.grab().ok().flatten();
            let t0 = Instant::now();
            let mut changes = Vec::new();
            let mut polls = 0u64;
            while t0.elapsed().as_secs_f64() < seconds {
                polls += 1;
                let f = d.grab().ok().flatten();
                if let (Some(a), Some(b)) = (&f, &prev) {
                    if let Some(r) = stream::dirty_rect(a, b) {
                        changes.push((t0.elapsed().as_secs_f64() * 1000.0, r));
                    }
                }
                if f.is_some() {
                    prev = f;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            let mut gaps: Vec<f64> = changes.windows(2).map(|w| ((w[1].0 - w[0].0) * 10.0).round() / 10.0).collect();
            let first: Vec<String> = changes.iter().take(12).map(|(t, r)| format!("{t:.0}ms {:?}", r)).collect();
            println!(
                "{}",
                json!({ "idle_cadence": { "seconds": seconds, "polls": polls, "changes": changes.len(), "gaps_ms": stats(&mut gaps), "first": first } })
            );
        }

        // ── 3. clic → image ─────────────────────────────────────────────────────────────────────────────────
        if !clicks.is_empty() {
            let mut inj = Injector::default();
            let (iw, ih) = (base.w as f64, base.h as f64);
            let mut via_dx = Vec::new();
            let mut via_print = Vec::new();
            let mut print_polls = Vec::new();
            let mut rows = Vec::new();
            for (k, &(x, y)) in clicks.iter().enumerate() {
                // la voie a mesurer alterne : surface DWM (le plugin seul), puis PrintWindow (la voie du compagnon)
                let use_dx = (k / 2) % 2 == 0 && dx.is_ok();
                let before_dx = dx.as_mut().ok().and_then(|d| d.grab().ok().flatten());
                let before_pr = print.grab().ok().flatten();
                let pt = Pt { x, y, iw: Some(iw), ih: Some(ih) };
                let t0 = Instant::now();
                let _ = inj.handle(hwnd, &InputEvent::Pointer { action: PointerAction::Down, pt, button: 0, buttons: 1, mods: 0 });
                let _ = inj.handle(hwnd, &InputEvent::Pointer { action: PointerAction::Up, pt, button: 0, buttons: 0, mods: 0 });
                let t_inj = ms(t0.elapsed());
                let mut got = None;
                let mut polls = 0;
                let mut area = 0u64;
                while t0.elapsed() < Duration::from_millis(800) {
                    polls += 1;
                    let changed = if use_dx {
                        let f = dx.as_mut().ok().and_then(|d| d.grab().ok().flatten());
                        match (&f, &before_dx) {
                            (Some(a), Some(b)) => stream::dirty_rect(a, b),
                            _ => None,
                        }
                    } else {
                        let f = print.grab().ok().flatten();
                        match (&f, &before_pr) {
                            (Some(a), Some(b)) => stream::dirty_rect(a, b),
                            _ => None,
                        }
                    };
                    if let Some(r) = changed {
                        got = Some(ms(t0.elapsed()));
                        area = r.2 as u64 * r.3 as u64;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(if use_dx { 1 } else { 4 }));
                }
                // la fin du dessin (surface DWM : plus rien ne change pendant 100 ms)
                let mut settle = None;
                if use_dx && got.is_some() {
                    let mut prev = dx.as_mut().ok().and_then(|d| d.grab().ok().flatten());
                    let mut last = t0.elapsed();
                    while t0.elapsed() - last < Duration::from_millis(100) && t0.elapsed() < Duration::from_millis(1500) {
                        std::thread::sleep(Duration::from_millis(1));
                        let f = dx.as_mut().ok().and_then(|d| d.grab().ok().flatten());
                        if let (Some(a), Some(b)) = (&f, &prev) {
                            if stream::dirty_rect(a, b).is_some() {
                                last = t0.elapsed();
                            }
                        }
                        if f.is_some() {
                            prev = f;
                        }
                    }
                    settle = Some(ms(last));
                }
                rows.push(format!("click {k} at {x:.0},{y:.0} via {}: inject {t_inj} ms, first change {:?} ms ({polls} polls, {area} px), last change {:?} ms", if use_dx { "dwm" } else { "print" }, got, settle));
                if let Some(g) = got {
                    if use_dx {
                        via_dx.push(g);
                    } else {
                        via_print.push(g);
                        print_polls.push(polls as f64);
                    }
                }
                std::thread::sleep(Duration::from_millis(350 + (k as u64 * 37) % 60));
            }
            for r in &rows {
                println!("  {r}");
            }
            println!("{}", json!({ "click_to_change_ms": { "dwm_surface": stats(&mut via_dx), "printwindow": stats(&mut via_print), "print_polls": stats(&mut print_polls) } }));
            if let Some(dir) = out {
                std::thread::sleep(Duration::from_millis(300));
                if let Ok(Some(f)) = print.grab() {
                    let _ = std::fs::write(format!("{dir}/after-clicks.jpg"), encode::jpeg(&f, 85).unwrap_or_default());
                }
            }
        }
        0
    }

    /// La part commune (le coin logique) est-elle identique ? Rend la fraction de pixels egaux.
    fn same_part(a: &Frame, b: &Frame) -> f64 {
        let w = a.w.min(b.w) as usize;
        let h = a.h.min(b.h) as usize;
        let mut same = 0usize;
        for y in 0..h {
            for x in 0..w {
                let i = (y * a.w as usize + x) * 4;
                let j = (y * b.w as usize + x) * 4;
                if a.bgra[i..i + 3] == b.bgra[j..j + 3] {
                    same += 1;
                }
            }
        }
        ((same as f64 / (w * h).max(1) as f64) * 1000.0).round() / 1000.0
    }

        use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject, SetViewportOrgEx, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, SRCCOPY,
    };
    use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS, PW_CLIENTONLY};
    use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

    fn hw(id: u64) -> HWND {
        HWND(id as isize as *mut core::ffi::c_void)
    }

    fn client(h: HWND) -> (i32, i32) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(h, &mut r);
        }
        (r.right - r.left, r.bottom - r.top)
    }

    /// PrintWindow seul, puis GetDIBits (ms, ms). `region` : un DC plus petit, decale sur (x, y).
    fn raw_print(id: u64, full: bool, region: Option<(i32, i32, i32, i32)>) -> (f64, f64) {
        let h = hw(id);
        let (cw, ch) = client(h);
        let (x, y, w, hh) = region.unwrap_or((0, 0, cw, ch));
        unsafe {
            let hdc = GetDC(Some(h));
            let mem = CreateCompatibleDC(Some(hdc));
            let bmp = CreateCompatibleBitmap(hdc, w, hh);
            let old = SelectObject(mem, bmp.into());
            let _ = SetViewportOrgEx(mem, -x, -y, None);
            let flags = PW_CLIENTONLY.0 | if full { 2 } else { 0 };
            let t0 = Instant::now();
            let _ = PrintWindow(h, mem, PRINT_WINDOW_FLAGS(flags));
            let a = ms(t0.elapsed());
            let mut bi = BITMAPINFO::default();
            bi.bmiHeader = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -hh, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() };
            let mut buf = vec![0u8; (w * hh * 4) as usize];
            let t0 = Instant::now();
            GetDIBits(mem, bmp, 0, hh as u32, Some(buf.as_mut_ptr() as *mut core::ffi::c_void), &mut bi, DIB_RGB_COLORS);
            let b = ms(t0.elapsed());
            SelectObject(mem, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem);
            ReleaseDC(Some(h), hdc);
            (a, b)
        }
    }

    fn read_bitmap(mem: windows::Win32::Graphics::Gdi::HDC, bmp: windows::Win32::Graphics::Gdi::HBITMAP, w: i32, h: i32) -> Frame {
        let mut bi = BITMAPINFO::default();
        bi.bmiHeader = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() };
        let mut buf = vec![0u8; (w * h * 4) as usize];
        unsafe {
            GetDIBits(mem, bmp, 0, h as u32, Some(buf.as_mut_ptr() as *mut core::ffi::c_void), &mut bi, DIB_RGB_COLORS);
        }
        for p in buf.chunks_exact_mut(4) {
            p[3] = 255;
        }
        Frame { w: w as u32, h: h as u32, bgra: buf }
    }

    fn raw_frame(id: u64, full: bool) -> Option<Frame> {
        let h = hw(id);
        let (w, hh) = client(h);
        unsafe {
            let hdc = GetDC(Some(h));
            let mem = CreateCompatibleDC(Some(hdc));
            let bmp = CreateCompatibleBitmap(hdc, w, hh);
            let old = SelectObject(mem, bmp.into());
            let ok = PrintWindow(h, mem, PRINT_WINDOW_FLAGS(PW_CLIENTONLY.0 | if full { 2 } else { 0 })).as_bool();
            let f = read_bitmap(mem, bmp, w, hh);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem);
            ReleaseDC(Some(h), hdc);
            ok.then_some(f)
        }
    }

    fn bitblt(id: u64) -> Option<Frame> {
        let h = hw(id);
        let (w, hh) = client(h);
        unsafe {
            let hdc = GetDC(Some(h));
            let mem = CreateCompatibleDC(Some(hdc));
            let bmp = CreateCompatibleBitmap(hdc, w, hh);
            let old = SelectObject(mem, bmp.into());
            let ok = BitBlt(mem, 0, 0, w, hh, Some(hdc), 0, 0, SRCCOPY).is_ok();
            let f = read_bitmap(mem, bmp, w, hh);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem);
            ReleaseDC(Some(h), hdc);
            ok.then_some(f)
        }
    }
}
