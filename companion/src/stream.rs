// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Le flux d'images d'une fenetre, sur un fil a lui (partie 2) : la connexion ne s'arrete jamais pendant une capture
//! (PrintWindow + JPEG ≈ 22 ms sur Diva 1500×837), les gestes sont donc injectes des leur arrivee.
//!
//! Le fil garde l'image que le studio affiche (`shown`) et n'envoie que ce qui a change :
//! - rien si rien n'a change ;
//! - une TUILE (le rectangle des pixels changes) si c'est petit, sinon l'image entiere ;
//! - pendant un geste (glisser, molette), la tuile est bornee autour du pointeur et envoyee en qualite plus basse
//!   (plus vite encodee, plus legere) ; a la fin du geste, une image nette (`sharp`) repasse sur ce qui a ete flou.
//! Une image reduite (plus de MAX_PIXELS) part toujours entiere.

use crate::capture::{Capturer, Frame};
use crate::encode;
use crate::protocol::{self, FrameHeader};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Pendant un geste, une tuile de plus de TILE_BUDGET pixels est bornee a GESTURE_BOX autour du pointeur.
pub const TILE_BUDGET: u64 = 360 * 360;
pub const GESTURE_BOX: (u32, u32) = (480, 360);
/// « Pres du pointeur » (la mesure geste → image) : un carre de NEAR px autour.
pub const NEAR: u32 = 96;
pub const GESTURE_QUALITY: u8 = 62;

pub type Rect = (u32, u32, u32, u32);

/// UWC_TRACE=1 : chaque capture et sa decision dans le journal (le diagnostic de la latence).
pub fn trace() -> bool {
    static T: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *T.get_or_init(|| std::env::var_os("UWC_TRACE").is_some())
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Le rectangle des pixels qui different (None : identiques). Les deux images ont la meme taille.
pub fn dirty_rect(a: &Frame, b: &Frame) -> Option<Rect> {
    if a.w != b.w || a.h != b.h {
        return Some((0, 0, a.w, a.h));
    }
    let row = (a.w * 4) as usize;
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for y in 0..a.h as usize {
        let ra = &a.bgra[y * row..(y + 1) * row];
        let rb = &b.bgra[y * row..(y + 1) * row];
        if ra == rb {
            continue;
        }
        let first = ra.chunks_exact(4).zip(rb.chunks_exact(4)).position(|(p, q)| p != q).unwrap_or(0) as u32;
        let last = (a.w as usize - 1 - ra.chunks_exact(4).rev().zip(rb.chunks_exact(4).rev()).position(|(p, q)| p != q).unwrap_or(0)) as u32;
        x0 = x0.min(first);
        x1 = x1.max(last + 1);
        y0 = y0.min(y as u32);
        y1 = y as u32 + 1;
    }
    if x0 == u32::MAX {
        None
    } else {
        Some((x0, y0, x1 - x0, y1 - y0))
    }
}

/// Des pixels different-ils dans le rectangle r (deux images de meme taille, r a l'interieur) ?
pub fn differs_in(a: &Frame, b: &Frame, r: Rect) -> bool {
    let row = (a.w * 4) as usize;
    (r.1..r.1 + r.3).any(|y| {
        let s = y as usize * row + r.0 as usize * 4;
        let e = s + (r.2 * 4) as usize;
        a.bgra[s..e] != b.bgra[s..e]
    })
}

/// Le pointeur (pixels physiques de la zone client) dans l'image fw×fh : un plugin « DPI unaware » est filme a sa
/// taille logique, plus petite que sa taille a l'ecran `display`.
pub fn image_point(p: Option<(i32, i32)>, display: Option<(u32, u32)>, fw: u32, fh: u32) -> Option<(i32, i32)> {
    match (p, display) {
        (Some(p), Some((dw, dh))) if dw > 0 && dh > 0 => Some(((p.0 as i64 * fw as i64 / dw as i64) as i32, (p.1 as i64 * fh as i64 / dh as i64) as i32)),
        (p, _) => p,
    }
}

pub fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x0 = a.0.max(b.0);
    let y0 = a.1.max(b.1);
    let x1 = (a.0 + a.2).min(b.0 + b.2);
    let y1 = (a.1 + a.3).min(b.1 + b.3);
    if x1 > x0 && y1 > y0 {
        Some((x0, y0, x1 - x0, y1 - y0))
    } else {
        None
    }
}

pub fn union(a: Option<Rect>, b: Rect) -> Rect {
    match a {
        None => b,
        Some(a) => {
            let x0 = a.0.min(b.0);
            let y0 = a.1.min(b.1);
            let x1 = (a.0 + a.2).max(b.0 + b.2);
            let y1 = (a.1 + a.3).max(b.1 + b.3);
            (x0, y0, x1 - x0, y1 - y0)
        }
    }
}

/// Un rectangle de taille (bw, bh) centre sur p, garde dans l'image.
pub fn box_around(p: (i32, i32), bw: u32, bh: u32, w: u32, h: u32) -> Rect {
    let bw = bw.min(w);
    let bh = bh.min(h);
    let x = (p.0 - bw as i32 / 2).clamp(0, (w - bw) as i32) as u32;
    let y = (p.1 - bh as i32 / 2).clamp(0, (h - bh) as i32) as u32;
    (x, y, bw, bh)
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Choice {
    Full,
    Tile(Rect),
    Nothing,
}

/// Que faut-il envoyer, sachant le rectangle change ?
pub fn choose(dirty: Rect, w: u32, h: u32, gesture: bool, pointer: Option<(i32, i32)>) -> Choice {
    let area = dirty.2 as u64 * dirty.3 as u64;
    let full = w as u64 * h as u64;
    if !gesture {
        return if area * 2 > full { Choice::Full } else { Choice::Tile(dirty) };
    }
    if area <= TILE_BUDGET {
        return Choice::Tile(dirty);
    }
    match pointer {
        Some(p) => match intersect(dirty, box_around(p, GESTURE_BOX.0, GESTURE_BOX.1, w, h)) {
            Some(r) => Choice::Tile(r),
            None => Choice::Nothing, // ce qui change est loin du geste : l'image nette suivra
        },
        None => Choice::Full,
    }
}

pub fn crop(f: &Frame, r: Rect) -> Frame {
    let mut out = Vec::with_capacity((r.2 * r.3 * 4) as usize);
    let row = (f.w * 4) as usize;
    for y in r.1..r.1 + r.3 {
        let s = y as usize * row + r.0 as usize * 4;
        out.extend_from_slice(&f.bgra[s..s + (r.2 * 4) as usize]);
    }
    Frame { w: r.2, h: r.3, bgra: out }
}

pub fn paste(dst: &mut Frame, src: &Frame, r: Rect) {
    let row = (dst.w * 4) as usize;
    for y in 0..r.3 {
        let d = (r.1 + y) as usize * row + r.0 as usize * 4;
        let s = (y * src.w * 4) as usize;
        dst.bgra[d..d + (r.2 * 4) as usize].copy_from_slice(&src.bgra[s..s + (r.2 * 4) as usize]);
    }
}

/// Une demande d'image.
pub struct Req {
    pub seq: u64,
    pub gesture: bool,
    pub pointer: Option<(i32, i32)>,
    pub input_n: u64,
    pub quality: u8,
    /// Le geste est fini : repasser net ce qui a ete envoye flou.
    pub sharp: bool,
}

pub struct Resp {
    pub seq: u64,
    pub packet: Result<Option<Vec<u8>>, String>,
    /// Une tuile floue a ete envoyee (la session demandera une image nette apres le geste).
    pub blurry: bool,
    /// La capture ne coute presque rien et ne touche pas au fil du plugin (la surface DWM) : pendant un geste, la
    /// session peut la refaire toutes les quelques ms.
    pub cheap: bool,
}

pub struct Worker {
    pub id: u64,
    pub title: String,
    pub cap: Option<Box<dyn Capturer>>,
    /// Le dernier geste injecte, lu au DEBUT de chaque capture (la session l'ecrit a chaque injection) : une image
    /// porte exactement les gestes injectes avant qu'elle soit prise (None : celui de la demande, les tests).
    latest_input: Option<Arc<AtomicU64>>,
    shown: Option<Frame>,
    blur: Option<Rect>,
    demo_n: u64,
    // mesures (le journal toutes les 10 s)
    sent: u64,
    tiles: u64,
    bytes: u64,
    skipped_same: u64,
    work: Duration,
    since: Instant,
}

impl Worker {
    pub fn new(id: u64, title: String, cap: Option<Box<dyn Capturer>>) -> Self {
        Worker { id, title, cap, latest_input: None, shown: None, blur: None, demo_n: 0, sent: 0, tiles: 0, bytes: 0, skipped_same: 0, work: Duration::ZERO, since: Instant::now() }
    }

    pub fn with_latest_input(mut self, a: Arc<AtomicU64>) -> Self {
        self.latest_input = Some(a);
        self
    }

    fn kind(&self) -> &'static str {
        self.cap.as_ref().map(|c| c.kind()).unwrap_or("demo")
    }

    /// Une capture, et le paquet a envoyer (None : rien de neuf).
    pub fn step(&mut self, r: &Req) -> Result<(Option<Vec<u8>>, bool), String> {
        let input_n = self.latest_input.as_ref().map(|a| a.load(Ordering::SeqCst)).unwrap_or(r.input_n);
        let t_ms = now_ms();
        let t0 = Instant::now();
        let grabbed = match self.cap.as_mut() {
            None => {
                self.demo_n += 1;
                Some(encode::synthetic(640, 400, self.demo_n))
            }
            // 21 oct. : les menus ouverts du plugin (des fenetres a part) poses sur son image
            Some(c) => c.grab()?.map(|f| crate::popups::compose(self.id, f)),
        };
        let Some(frame) = grabbed else { return Ok((None, self.blur.is_some())) };
        let (fw, fh) = (frame.w, frame.h);
        let reduced = fw as u64 * fh as u64 > encode::MAX_PIXELS;
        // le pointeur dans l'image (21 oct. : sur Diva, le pointeur en pixels physiques tombait 25 % trop loin dans
        // l'image logique : « pres du pointeur » et la tuile du geste visaient a cote)
        let pointer = image_point(r.pointer, self.cap.as_ref().and_then(|c| c.display_size()), fw, fh);
        let near_box = pointer.map(|p| box_around(p, NEAR, NEAR, fw, fh));
        let same_size = self.shown.as_ref().map(|s| s.w == fw && s.h == fh).unwrap_or(false);

        // l'image nette apres un geste : la zone floue, en qualite normale
        // real : les pixels qui ont vraiment change (une image nette qui ne fait que repasser sur du flou n'est pas
        // une reaction a un geste : ni « in » ni « near » pour elle)
        let mut real: Option<Rect> = Some((0, 0, fw, fh));
        let choice = if !same_size {
            Choice::Full
        } else {
            let dirty = dirty_rect(&frame, self.shown.as_ref().unwrap());
            real = dirty;
            let dirty = match (r.sharp, self.blur, dirty) {
                (true, Some(b), Some(d)) => Some(union(Some(b), d)),
                (true, Some(b), None) => Some(b),
                (_, _, d) => d,
            };
            match dirty {
                None => {
                    self.skipped_same += 1;
                    if trace() {
                        crate::log(&format!("trace seq {} in {} unchanged ({:.1} ms)", r.seq, input_n, t0.elapsed().as_secs_f64() * 1000.0));
                    }
                    return Ok((None, self.blur.is_some()));
                }
                Some(d) if reduced => {
                    let _ = d;
                    Choice::Full
                }
                Some(d) => choose(d, fw, fh, r.gesture && !r.sharp, pointer),
            }
        };
        if trace() {
            crate::log(&format!("trace seq {} in {} at {} gesture {} sharp {} pointer {:?} → {:?}", r.seq, input_n, t_ms % 100_000, r.gesture, r.sharp, pointer, choice));
        }
        let quality = if r.gesture && !r.sharp { GESTURE_QUALITY.min(r.quality) } else { r.quality };
        // « pres du pointeur » : des pixels ont VRAIMENT change dans le carre autour du pointeur (et dans ce qui part),
        // pas seulement le rectangle englobant de tous les changements de l'image (21 oct. : un changement ailleurs
        // comptait, un changement juste a cote du carre aussi)
        let changed_near = |within: Rect| -> bool {
            match (near_box, real, self.shown.as_ref()) {
                (Some(nb), Some(_), Some(s)) if same_size => intersect(nb, within).map(|x| differs_in(&frame, s, x)).unwrap_or(false),
                (Some(_), Some(_), _) => true, // une nouvelle taille : tout a change
                _ => false,
            }
        };
        let (img, tile, near) = match choice {
            Choice::Nothing => return Ok((None, self.blur.is_some())),
            Choice::Full => {
                let near = changed_near((0, 0, fw, fh));
                let img = if reduced { encode::cap_size(Frame { w: fw, h: fh, bgra: frame.bgra.clone() }, encode::MAX_PIXELS) } else { crop(&frame, (0, 0, fw, fh)) };
                (img, None, near)
            }
            Choice::Tile(rect) => {
                let near = changed_near(rect);
                (crop(&frame, rect), Some(rect), near)
            }
        };
        let jpeg = encode::jpeg(&img, quality).map_err(|e| format!("encode: {e}"))?;
        if trace() {
            crate::log(&format!("trace seq {} encoded {} bytes, near {near}, {:.1} ms after the capture started", r.seq, jpeg.len(), t0.elapsed().as_secs_f64() * 1000.0));
        }
        let (ow, oh) = (img.w, img.h);
        let packet = protocol::frame_packet(
            &FrameHeader {
                seq: r.seq,
                id: self.id,
                w: ow,
                h: oh,
                t_ms,
                cap: self.kind(),
                src_w: fw,
                src_h: fh,
                tile: tile.map(|t| (t.0, t.1)),
                quality,
                input_n: if real.is_some() { input_n } else { 0 },
                near,
                display: self.cap.as_ref().and_then(|c| c.display_size()),
            },
            &jpeg,
        );
        // ce que le studio affiche maintenant
        let blurry = quality < r.quality;
        match tile {
            Some(rect) => {
                paste(self.shown.as_mut().unwrap(), &img, rect);
                self.blur = if blurry { Some(union(self.blur, rect)) } else { self.blur.and_then(|b| if intersect(b, rect) == Some(b) { None } else { Some(b) }) };
                self.tiles += 1;
            }
            None => {
                self.blur = if blurry { Some((0, 0, fw, fh)) } else { None };
                self.shown = Some(frame);
            }
        }
        if r.sharp && !blurry {
            self.blur = None;
        }
        self.sent += 1;
        self.bytes += packet.len() as u64;
        self.work += t0.elapsed();
        if self.since.elapsed() >= Duration::from_secs(10) {
            let s = self.since.elapsed().as_secs_f64();
            crate::log(&format!(
                "\"{}\": {:.1} fps sent ({} tiles), {:.0} KB/s, {}×{}, capture+JPEG {:.1} ms/frame, {} unchanged frames skipped",
                self.title,
                self.sent as f64 / s,
                self.tiles,
                self.bytes as f64 / 1024.0 / s,
                fw,
                fh,
                self.work.as_secs_f64() * 1000.0 / self.sent.max(1) as f64,
                self.skipped_same
            ));
            self.sent = 0;
            self.tiles = 0;
            self.bytes = 0;
            self.skipped_same = 0;
            self.work = Duration::ZERO;
            self.since = Instant::now();
        }
        Ok((Some(packet), self.blur.is_some()))
    }

    /// Le fil : une demande, une reponse, jusqu'a ce que la session lache son bout du canal.
    pub fn run(mut self, rx: Receiver<Req>, tx: Sender<Resp>) {
        while let Ok(r) = rx.recv() {
            let res = self.step(&r);
            let (packet, blurry) = match res {
                Ok((p, b)) => (Ok(p), b),
                Err(e) => (Err(e), false),
            };
            let stop = packet.is_err();
            let cheap = self.kind() == "dwm";
            if tx.send(Resp { seq: r.seq, packet, blurry, cheap }).is_err() || stop {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, v: u8) -> Frame {
        Frame { w, h, bgra: vec![v; (w * h * 4) as usize] }
    }

    fn set(f: &mut Frame, x: u32, y: u32, v: u8) {
        let i = ((y * f.w + x) * 4) as usize;
        f.bgra[i] = v;
    }

    #[test]
    fn finds_the_changed_rectangle() {
        let a = solid(100, 50, 0);
        let mut b = solid(100, 50, 0);
        assert_eq!(dirty_rect(&a, &b), None);
        set(&mut b, 10, 5, 9);
        set(&mut b, 30, 20, 9);
        assert_eq!(dirty_rect(&a, &b), Some((10, 5, 21, 16)));
        set(&mut b, 99, 49, 1);
        assert_eq!(dirty_rect(&a, &b), Some((10, 5, 90, 45)));
        assert_eq!(dirty_rect(&a, &solid(10, 10, 0)), Some((0, 0, 100, 50)), "size change = everything");
    }

    #[test]
    fn chooses_tile_or_full() {
        // hors geste : petit → tuile, plus de la moitie → entiere
        assert_eq!(choose((10, 10, 50, 50), 1500, 837, false, None), Choice::Tile((10, 10, 50, 50)));
        assert_eq!(choose((0, 0, 1500, 600), 1500, 837, false, None), Choice::Full);
        // pendant un geste : borne autour du pointeur
        assert_eq!(choose((0, 0, 1500, 837), 1500, 837, true, Some((750, 400))), Choice::Tile((510, 220, 480, 360)));
        assert_eq!(choose((0, 0, 100, 100), 1500, 837, true, Some((1400, 800))), Choice::Tile((0, 0, 100, 100)), "small = as is");
        assert_eq!(choose((0, 0, 1000, 200), 1500, 837, true, Some((1400, 800))), Choice::Nothing, "far from the gesture");
        assert_eq!(choose((0, 0, 1500, 837), 1500, 837, true, None), Choice::Full);
        assert_eq!(box_around((5, 5), 480, 360, 1500, 837), (0, 0, 480, 360));
        assert_eq!(box_around((1499, 836), 480, 360, 1500, 837), (1020, 477, 480, 360));
        assert_eq!(box_around((10, 10), 480, 360, 300, 200), (0, 0, 300, 200));
    }

    #[test]
    fn crop_and_paste_round_trip() {
        let mut a = solid(20, 10, 0);
        let mut b = solid(20, 10, 0);
        set(&mut b, 5, 5, 7);
        let r = dirty_rect(&a, &b).unwrap();
        let t = crop(&b, r);
        assert_eq!((t.w, t.h), (1, 1));
        paste(&mut a, &t, r);
        assert_eq!(dirty_rect(&a, &b), None);
        assert_eq!(union(None, (1, 1, 2, 2)), (1, 1, 2, 2));
        assert_eq!(union(Some((0, 0, 2, 2)), (5, 5, 1, 1)), (0, 0, 6, 6));
        assert_eq!(intersect((0, 0, 10, 10), (20, 20, 5, 5)), None);
    }

    /// Une fausse capture : rend les images de la liste, puis la derniere indefiniment.
    struct Script(Vec<Frame>);
    impl Capturer for Script {
        fn grab(&mut self) -> Result<Option<Frame>, String> {
            let f = if self.0.len() > 1 { self.0.remove(0) } else { Frame { w: self.0[0].w, h: self.0[0].h, bgra: self.0[0].bgra.clone() } };
            Ok(Some(f))
        }
        fn kind(&self) -> &'static str {
            "script"
        }
    }

    /// Diva « DPI unaware » sur un ecran a 125 % : l'image a sa taille logique, la fenetre plus grande a l'ecran.
    struct Unaware(Vec<Frame>);
    impl Capturer for Unaware {
        fn grab(&mut self) -> Result<Option<Frame>, String> {
            let f = if self.0.len() > 1 { self.0.remove(0) } else { Frame { w: self.0[0].w, h: self.0[0].h, bgra: self.0[0].bgra.clone() } };
            Ok(Some(f))
        }
        fn kind(&self) -> &'static str {
            "script"
        }
        fn display_size(&self) -> Option<(u32, u32)> {
            Some((1500, 837))
        }
    }

    fn square(f: &mut Frame, cx: u32, cy: u32, r: u32, v: u8) {
        for y in cy - r..cy + r {
            for x in cx - r..cx + r {
                set(f, x, y, v);
            }
        }
    }

    #[test]
    fn the_pointer_is_placed_in_image_pixels() {
        assert_eq!(image_point(Some((750, 375)), Some((1500, 837)), 1200, 670), Some((600, 300)));
        assert_eq!(image_point(Some((750, 375)), None, 1200, 670), Some((750, 375)));
        assert_eq!(image_point(None, Some((1500, 837)), 1200, 670), None);
        let a = solid(20, 10, 0);
        let mut b = solid(20, 10, 0);
        set(&mut b, 15, 5, 1);
        assert!(differs_in(&a, &b, (10, 0, 10, 10)));
        assert!(!differs_in(&a, &b, (0, 0, 10, 10)));
    }

    /// 21 oct. (Diva, « to image 115 ms (last 23) ») : le pointeur arrivait en pixels physiques (1500×837) dans
    /// une image logique (1200×670) ; le carre « pres du pointeur » tombait 25 % trop loin et le potard qui tournait
    /// sous le pointeur ne comptait pas.
    #[test]
    fn a_dpi_unaware_plugin_changing_under_the_pointer_is_near() {
        let a = solid(1200, 670, 0);
        let mut b = solid(1200, 670, 0);
        square(&mut b, 600, 300, 10, 9); // le potard sous le pointeur, en pixels de l'image
        let mut w = Worker::new(1, "diva".into(), Some(Box::new(Unaware(vec![a, Frame { w: 1200, h: 670, bgra: b.bgra.clone() }, b]))));
        // le pointeur en pixels physiques de la zone client : (600, 300) × 1,25
        let req = |seq, gesture| Req { seq, gesture, pointer: Some((750, 375)), input_n: 3, quality: 80, sharp: false };
        w.step(&req(1, false)).unwrap();
        let (p, _) = w.step(&req(2, true)).unwrap();
        let (h, _) = protocol::read_packet(p.as_ref().unwrap()).unwrap();
        assert_eq!((h["near"].as_bool(), h["x"].as_u64(), h["y"].as_u64(), h["pw"].as_u64()), (Some(true), Some(590), Some(290), Some(1500)), "{h}");
    }

    /// « pres du pointeur » : des pixels changes DANS le carre, pas le rectangle englobant de changements lointains.
    #[test]
    fn changes_far_from_the_pointer_are_not_near() {
        let a = solid(400, 300, 0);
        let mut b = solid(400, 300, 0);
        square(&mut b, 20, 20, 5, 9);
        square(&mut b, 380, 280, 5, 9); // englobant : toute l'image, pointeur compris
        let mut w = Worker::new(1, "s".into(), Some(Box::new(Script(vec![a, Frame { w: 400, h: 300, bgra: b.bgra.clone() }, b]))));
        let req = |seq| Req { seq, gesture: false, pointer: Some((200, 150)), input_n: 5, quality: 80, sharp: false };
        w.step(&req(1)).unwrap();
        let (p, _) = w.step(&req(2)).unwrap();
        let (h, _) = protocol::read_packet(p.as_ref().unwrap()).unwrap();
        assert_eq!(h["near"], false, "{h}");
    }

    #[test]
    fn a_blurry_tile_stays_owed_until_the_sharp_frame() {
        let a = solid(64, 64, 0);
        let mut b = solid(64, 64, 0);
        set(&mut b, 10, 10, 200);
        let mut w = Worker::new(1, "s".into(), Some(Box::new(Script(vec![a, Frame { w: 64, h: 64, bgra: b.bgra.clone() }, b]))));
        let req = |seq, gesture, sharp| Req { seq, gesture, pointer: Some((10, 10)), input_n: 1, quality: 80, sharp };
        assert!(w.step(&req(1, false, false)).unwrap().0.is_some(), "first image");
        let (p, blurry) = w.step(&req(2, true, false)).unwrap();
        assert!(p.is_some() && blurry, "a gesture tile, blurry");
        let (p, blurry) = w.step(&req(3, true, false)).unwrap();
        assert!(p.is_none() && blurry, "nothing new, but the sharp image is still owed");
        let (p, blurry) = w.step(&req(4, false, true)).unwrap();
        let (h, _) = protocol::read_packet(p.as_ref().unwrap()).unwrap();
        assert_eq!((h["q"].as_u64(), blurry), (Some(80), false));
    }

    #[test]
    fn demo_worker_sends_a_full_frame_then_tiles_then_sharpens() {
        let mut w = Worker::new(1, "demo".into(), None);
        let req = |seq, gesture, sharp| Req { seq, gesture, pointer: Some((300, 150)), input_n: 4, quality: 80, sharp };
        let (p, _) = w.step(&req(1, false, false)).unwrap();
        let (h, _) = protocol::read_packet(p.as_ref().unwrap()).unwrap();
        assert_eq!((h["w"].as_u64(), h.get("x").is_none(), h["in"].as_u64()), (Some(640), true, Some(4)));
        // la barre avance : une tuile (petite), floue pendant le geste
        let (p, blurry) = w.step(&req(2, true, false)).unwrap();
        let (h, _) = protocol::read_packet(p.as_ref().unwrap()).unwrap();
        assert!(h["x"].is_u64() && h["w"].as_u64().unwrap() < 640, "{h}");
        assert_eq!(h["q"], GESTURE_QUALITY);
        assert!(blurry);
        // fin du geste : une image nette repasse sur la zone floue
        let (p, blurry) = w.step(&req(3, false, true)).unwrap();
        let (h, _) = protocol::read_packet(p.as_ref().unwrap()).unwrap();
        assert_eq!(h["q"], 80);
        assert!(!blurry);
    }
}
