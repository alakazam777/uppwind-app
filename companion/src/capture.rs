// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Filmer UNE fenetre par son handle.
//!
//! - `Wgc` : Windows.Graphics.Capture (Windows 10 1903+), la voie preferee : la fenetre couverte continue d'etre
//!   filmee, Windows ne livre une image que si elle a change. Sous Windows 10, Windows dessine un cadre jaune autour
//!   de la fenetre filmee (impossible a retirer avant Windows 11) ; il reste cache derriere Chrome.
//! - `Dwm` : la surface de redirection de la fenetre (ce que DWM compose a l'ecran), lue par Direct3D 11 ; pour les
//!   fenetres de plugins de Live, que WGC refuse. Mesure du 21 oct. sur Diva : 1,6 ms par image, rien n'est demande au
//!   fil du plugin ; elle continue de changer avec la fenetre couverte (la fenetre d'essai, au fond, derriere tout).
//!   Recalee au pixel sur PrintWindow a l'ouverture ; si elle ne correspond pas ou ne se lit plus : PrintWindow.
//! - `Print` : PrintWindow(PW_RENDERFULLCONTENT), le dernier secours ; mesure le 20 oct. sur Diva : l'image continue de
//!   changer avec Chrome devant (83 % des images). 13-15 ms par image sur le fil de Live (le plugin se redessine).
//!
//! Les deux rendent la ZONE CLIENT de la fenetre (sans sa barre de titre), en BGRA serre (w*4 octets par ligne).

pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
}

/// Send : la capture tourne sur son propre fil (stream.rs), pas sur celui de la connexion.
pub trait Capturer: Send {
    /// Une nouvelle image, ou None si rien de neuf. Err : la fenetre a disparu ou la capture ne marche plus.
    fn grab(&mut self) -> Result<Option<Frame>, String>;
    fn kind(&self) -> &'static str;
    /// La taille a l'ecran (pixels physiques) quand l'image est plus petite : une fenetre « DPI unaware » que
    /// Windows agrandit (Diva a 125 % : image 1200×670, a l'ecran 1500×837). None : l'image est a la taille reelle.
    fn display_size(&self) -> Option<(u32, u32)> {
        None
    }
}

/// La taille logique d'une zone client physique w×h pour une fenetre a `window_dpi` sur un ecran a `monitor_dpi`
/// (la meme si les deux sont egaux ou inconnus).
pub fn logical_size(w: u32, h: u32, window_dpi: u32, monitor_dpi: u32) -> (u32, u32) {
    if window_dpi == 0 || monitor_dpi == 0 || window_dpi >= monitor_dpi {
        return (w, h);
    }
    let f = window_dpi as f64 / monitor_dpi as f64;
    (((w as f64) * f).round().max(1.0) as u32, ((h as f64) * f).round().max(1.0) as u32)
}

/// Le fil principal entre dans le « MTA » de COM/WinRT une fois pour toutes, et n'en sort jamais.
/// Vu le 21 oct. (Kontakt, deux fois de suite) : chaque connexion du studio est un fil qui fait RoInitialize(MTA) pour WGC ;
/// quand ce fil se termine (le studio se deconnecte) et qu'il etait le dernier du MTA, COM demonte le MTA, et la
/// fabrique WGC gardee en cache par la crate `windows` pointe dans le vide : la connexion SUIVANTE plantait le
/// compagnon (code 139) des son premier `watch`. Un fil qui vit autant que le programme garde le MTA en vie.
#[cfg(windows)]
pub fn keep_com_alive() {
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
}
#[cfg(not(windows))]
pub fn keep_com_alive() {}

/// Le DPI d'une fenetre et celui de son ecran (pour les menus d'un plugin « DPI unaware », popups.rs).
#[cfg(windows)]
pub fn dpis_of(id: u64) -> (u32, u32) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
    let h = HWND(id as isize as *mut core::ffi::c_void);
    unsafe {
        let w = GetDpiForWindow(h);
        let (mut mx, mut my) = (0u32, 0u32);
        let _ = GetDpiForMonitor(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), MDT_EFFECTIVE_DPI, &mut mx, &mut my);
        (w, mx)
    }
}

/// Ouvre la meilleure capture possible : WGC, sinon la surface DWM, sinon PrintWindow. `force` : « wgc », « dwm » ou
/// « print ».
#[cfg(windows)]
pub fn open(hwnd: u64, force: Option<&str>) -> Result<Box<dyn Capturer>, String> {
    if force == Some("dwm") {
        return Ok(Box::new(win::Dwm::new(hwnd)?));
    }
    if force != Some("print") {
        match win::Wgc::new(hwnd) {
            Ok(c) => return Ok(Box::new(c)),
            Err(e) if force == Some("wgc") => return Err(e),
            // mesure du 20 oct. (Windows 10 22H2, Live 12.4.6) : CreateForWindow refuse les fenetres de plugins de Live
            // (E_INVALIDARG : des fenetres d'outil possedees, que WGC ne filme pas — la raison pour laquelle le
            // selecteur de Chrome ne les liste pas) ; il accepte la fenetre principale.
            Err(e) => crate::log(&format!("Windows.Graphics.Capture refused this window ({e})")),
        }
        // mesure du 21 oct. (Diva) : la surface DWM se lit en 1,6 ms sans toucher au fil du plugin, PrintWindow coute
        // 13-15 ms sur le fil de Live (il force le plugin a se redessiner) ; elle passe donc avant
        match win::Dwm::new(hwnd) {
            Ok(c) => {
                crate::log("capture: the window's DWM surface");
                return Ok(Box::new(c));
            }
            Err(e) => crate::log(&format!("DWM surface unavailable ({e}): using PrintWindow")),
        }
    }
    Ok(Box::new(win::Print::new(hwnd)?))
}

/// (23 oct.) Ce que la capture apprend d'un plugin pendant la vie du compagnon : WGC le refuse (toutes les fenetres de
/// plugins de Live, mesure du 20 oct.), sa surface DWM ne montre pas la fenetre (Kontakt, qui dessine en Direct3D). La
/// fois suivante, on ne le redemande pas : Follow track passe d'une piste a l'autre, et chaque essai rate coutait
/// ~0,1 a 0,3 s avant la premiere image. Par nom de plugin ; `force` passe outre.
pub const LEARN_NO_WGC: u8 = 1;
pub const LEARN_NO_DWM: u8 = 2;

fn learned() -> &'static std::sync::Mutex<std::collections::HashMap<String, u8>> {
    static L: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, u8>>> = std::sync::OnceLock::new();
    L.get_or_init(Default::default)
}

pub fn learned_of(plugin: &str) -> u8 {
    learned().lock().map(|m| m.get(plugin).copied().unwrap_or(0)).unwrap_or(0)
}

pub fn learn(plugin: &str, bit: u8) {
    if plugin.is_empty() {
        return;
    }
    if let Ok(mut m) = learned().lock() {
        *m.entry(plugin.to_string()).or_insert(0) |= bit;
    }
}

/// Comme `open`, pour la fenetre d'un plugin nomme : les essais deja rates pour ce plugin sont sautes.
#[cfg(windows)]
pub fn open_plugin(hwnd: u64, plugin: &str, force: Option<&str>) -> Result<Box<dyn Capturer>, String> {
    if force.is_some() {
        return open(hwnd, force);
    }
    let known = learned_of(plugin);
    if known & LEARN_NO_WGC == 0 {
        match win::Wgc::new(hwnd) {
            Ok(c) => return Ok(Box::new(c)),
            Err(e) => {
                crate::log(&format!("Windows.Graphics.Capture refused this window ({e}): not asked again for {plugin}"));
                learn(plugin, LEARN_NO_WGC);
            }
        }
    }
    if known & LEARN_NO_DWM == 0 {
        match win::Dwm::new(hwnd) {
            Ok(c) => {
                crate::log("capture: the window's DWM surface");
                return Ok(Box::new(c));
            }
            Err(e) => {
                crate::log(&format!("DWM surface unavailable ({e}): using PrintWindow, also next time for {plugin}"));
                learn(plugin, LEARN_NO_DWM);
            }
        }
    }
    Ok(Box::new(win::Print::new(hwnd)?))
}

#[cfg(not(windows))]
pub fn open_plugin(hwnd: u64, _plugin: &str, force: Option<&str>) -> Result<Box<dyn Capturer>, String> {
    open(hwnd, force)
}

/// Le recalage de la surface DWM sur l'image de PrintWindow : le decalage (dx, dy) autour de `guess` (± `r` pixels)
/// ou le plus de pixels sont egaux (un sur `step` dans chaque sens), et cette part. `surf` : la surface entiere,
/// `reference` : la zone client (PrintWindow), plus petite.
pub const MATCH_TOLERANCE: u8 = 12;

pub fn best_offset(surf: &Frame, reference: &Frame, guess: (i32, i32), r: i32, step: usize) -> Option<((u32, u32), f64)> {
    let mut best: Option<((u32, u32), f64)> = None;
    for dy in -r..=r {
        for dx in -r..=r {
            let (ox, oy) = (guess.0 + dx, guess.1 + dy);
            if ox < 0 || oy < 0 || ox as u32 + reference.w > surf.w || oy as u32 + reference.h > surf.h {
                continue;
            }
            let (mut same, mut all) = (0usize, 0usize);
            for y in (0..reference.h as usize).step_by(step) {
                for x in (0..reference.w as usize).step_by(step) {
                    let i = (y * reference.w as usize + x) * 4;
                    let j = ((y + oy as usize) * surf.w as usize + x + ox as usize) * 4;
                    all += 1;
                    // a quelques niveaux pres : mesure du 21 oct., Diva rouvert, la meme image a l'oeil mais 17 % de
                    // pixels exactement egaux seulement (PrintWindow redessine le plugin, la surface garde son dessin)
                    if reference.bgra[i..i + 3].iter().zip(&surf.bgra[j..j + 3]).all(|(a, b)| a.abs_diff(*b) <= MATCH_TOLERANCE) {
                        same += 1;
                    }
                }
            }
            let part = same as f64 / all.max(1) as f64;
            if best.map(|b| part > b.1).unwrap_or(true) {
                best = Some(((ox as u32, oy as u32), part));
            }
        }
    }
    best
}

#[cfg(not(windows))]
pub fn open(_hwnd: u64, _force: Option<&str>) -> Result<Box<dyn Capturer>, String> {
    Err("capture needs Windows".into())
}

/// Le rectangle (x, y, w, h) de la zone client dans l'image de la fenetre entiere, borne a l'image.
pub fn clamp_crop(x: i32, y: i32, w: i32, h: i32, img_w: u32, img_h: u32) -> (u32, u32, u32, u32) {
    let x = x.clamp(0, img_w as i32) as u32;
    let y = y.clamp(0, img_h as i32) as u32;
    let w = (w.max(0) as u32).min(img_w - x);
    let h = (h.max(0) as u32).min(img_h - y);
    (x, y, w, h)
}

#[cfg(windows)]
mod win {
    use super::{clamp_crop, Capturer, Frame};
    use std::sync::OnceLock;
    use windows::core::{s, w, Interface};
    use windows::Win32::Foundation::{HANDLE, LUID};
    use windows::Win32::Graphics::Direct3D11::D3D11_BOX;
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB, DXGI_FORMAT_B8G8R8X8_UNORM};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsIconic};
    use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
    use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Graphics::SizeInt32;
    use windows::Win32::Foundation::{HMODULE, HWND, POINT, RECT};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    };
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;
    use windows::Win32::Graphics::Gdi::{
        ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS, PW_CLIENTONLY};
    use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
    use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
    use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, IsWindow};

    const PW_RENDERFULLCONTENT: u32 = 0x0000_0002;

    fn hwnd_of(id: u64) -> HWND {
        HWND(id as isize as *mut core::ffi::c_void)
    }

    fn alive(h: HWND) -> bool {
        unsafe { IsWindow(Some(h)).as_bool() }
    }

    fn client_size(h: HWND) -> (i32, i32) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(h, &mut r);
        }
        (r.right - r.left, r.bottom - r.top)
    }

    /// La zone client par rapport au coin du cadre visible de la fenetre (ce que WGC filme).
    fn client_in_frame(h: HWND) -> Option<(i32, i32, i32, i32)> {
        unsafe {
            let mut fr = RECT::default();
            DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut fr as *mut RECT as *mut core::ffi::c_void, std::mem::size_of::<RECT>() as u32).ok()?;
            let mut p = POINT { x: 0, y: 0 };
            if !ClientToScreen(h, &mut p).as_bool() {
                return None;
            }
            let (w, hh) = client_size(h);
            Some((p.x - fr.left, p.y - fr.top, w, hh))
        }
    }

    // ── Windows.Graphics.Capture ──────────────────────────────────────────────────────────────────────────────
    pub struct Wgc {
        hwnd: HWND,
        device: ID3D11Device,
        ctx: ID3D11DeviceContext,
        idevice: IDirect3DDevice,
        pool: Direct3D11CaptureFramePool,
        session: GraphicsCaptureSession,
        size: SizeInt32,
        staging: Option<(ID3D11Texture2D, u32, u32)>,
    }

    // Le pool est « free threaded » (aucun fil de messages) et tout l'objet ne sert qu'au fil de la connexion.
    unsafe impl Send for Wgc {}

    impl Wgc {
        pub fn new(id: u64) -> Result<Self, String> {
            let hwnd = hwnd_of(id);
            if !alive(hwnd) {
                return Err("the window is gone".into());
            }
            unsafe {
                let _ = RoInitialize(RO_INIT_MULTITHREADED);
                if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
                    return Err("Windows.Graphics.Capture is not supported here".into());
                }
                let mut device: Option<ID3D11Device> = None;
                let mut ctx: Option<ID3D11DeviceContext> = None;
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_HARDWARE,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut ctx),
                )
                .map_err(|e| format!("D3D11CreateDevice: {e}"))?;
                let device = device.ok_or("no D3D11 device")?;
                let ctx = ctx.ok_or("no D3D11 context")?;
                let dxgi: IDXGIDevice = device.cast().map_err(|e| format!("IDXGIDevice: {e}"))?;
                let insp = CreateDirect3D11DeviceFromDXGIDevice(&dxgi).map_err(|e| format!("CreateDirect3D11DeviceFromDXGIDevice: {e}"))?;
                let idevice: IDirect3DDevice = insp.cast().map_err(|e| format!("IDirect3DDevice: {e}"))?;
                let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(|e| format!("capture interop: {e}"))?;
                let item: GraphicsCaptureItem = interop.CreateForWindow(hwnd).map_err(|e| format!("CreateForWindow: {e}"))?;
                let size = item.Size().map_err(|e| e.to_string())?;
                let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&idevice, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size)
                    .map_err(|e| format!("frame pool: {e}"))?;
                let session = pool.CreateCaptureSession(&item).map_err(|e| format!("capture session: {e}"))?;
                let _ = session.SetIsCursorCaptureEnabled(false); // Windows 10 2004+
                session.StartCapture().map_err(|e| format!("StartCapture: {e}"))?;
                Ok(Wgc { hwnd, device, ctx, idevice, pool, session, size, staging: None })
            }
        }
    }

    impl Drop for Wgc {
        fn drop(&mut self) {
            let _ = self.session.Close();
            let _ = self.pool.Close();
        }
    }

    impl Capturer for Wgc {
        fn kind(&self) -> &'static str {
            "wgc"
        }
        fn grab(&mut self) -> Result<Option<Frame>, String> {
            if !alive(self.hwnd) {
                return Err("the window is gone".into());
            }
            // la plus recente des images en attente
            let mut last = None;
            while let Ok(f) = self.pool.TryGetNextFrame() {
                last = Some(f);
            }
            let Some(frame) = last else { return Ok(None) };
            let content = frame.ContentSize().map_err(|e| e.to_string())?;
            if content.Width <= 0 || content.Height <= 0 {
                return Ok(None);
            }
            if content.Width != self.size.Width || content.Height != self.size.Height {
                // la fenetre a change de taille : un nouveau pool, l'image suivante sera la bonne
                self.size = content;
                self.pool
                    .Recreate(&self.idevice, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, content)
                    .map_err(|e| format!("Recreate: {e}"))?;
                return Ok(None);
            }
            unsafe {
                let surface = frame.Surface().map_err(|e| e.to_string())?;
                let access: IDirect3DDxgiInterfaceAccess = surface.cast().map_err(|e| e.to_string())?;
                let tex: ID3D11Texture2D = access.GetInterface().map_err(|e| e.to_string())?;
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                tex.GetDesc(&mut desc);
                let need = match &self.staging {
                    Some((_, w, h)) => *w != desc.Width || *h != desc.Height,
                    None => true,
                };
                if need {
                    let sd = D3D11_TEXTURE2D_DESC {
                        Width: desc.Width,
                        Height: desc.Height,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: desc.Format,
                        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                        Usage: D3D11_USAGE_STAGING,
                        BindFlags: 0,
                        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                        MiscFlags: 0,
                    };
                    let mut st: Option<ID3D11Texture2D> = None;
                    self.device.CreateTexture2D(&sd, None, Some(&mut st)).map_err(|e| format!("staging texture: {e}"))?;
                    self.staging = Some((st.ok_or("no staging texture")?, desc.Width, desc.Height));
                }
                let staging = self.staging.as_ref().unwrap().0.clone();
                self.ctx.CopyResource(&staging, &tex);
                let mut m = D3D11_MAPPED_SUBRESOURCE::default();
                self.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m)).map_err(|e| format!("Map: {e}"))?;
                let img_w = (content.Width as u32).min(desc.Width);
                let img_h = (content.Height as u32).min(desc.Height);
                let (cx, cy, cw, ch) = match client_in_frame(self.hwnd) {
                    Some((x, y, w, h)) => clamp_crop(x, y, w, h, img_w, img_h),
                    None => (0, 0, img_w, img_h),
                };
                let mut out = Vec::with_capacity((cw * ch * 4) as usize);
                let src = m.pData as *const u8;
                for row in 0..ch {
                    let off = ((cy + row) * m.RowPitch + cx * 4) as usize;
                    out.extend_from_slice(std::slice::from_raw_parts(src.add(off), (cw * 4) as usize));
                }
                self.ctx.Unmap(&staging, 0);
                if cw == 0 || ch == 0 {
                    return Ok(None);
                }
                Ok(Some(Frame { w: cw, h: ch, bgra: out }))
            }
        }
    }

    // ── PrintWindow ───────────────────────────────────────────────────────────────────────────────────────────
    pub struct Print {
        hwnd: HWND,
        display: Option<(u32, u32)>,
    }
    unsafe impl Send for Print {}

    impl Print {
        pub fn new(id: u64) -> Result<Self, String> {
            let hwnd = hwnd_of(id);
            if !alive(hwnd) {
                return Err("the window is gone".into());
            }
            Ok(Print { hwnd, display: None })
        }
    }

    /// Le DPI de la fenetre et celui de son ecran.
    fn dpis(h: HWND) -> (u32, u32) {
        use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
        use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
        unsafe {
            let w = GetDpiForWindow(h);
            let (mut mx, mut my) = (0u32, 0u32);
            let _ = GetDpiForMonitor(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), MDT_EFFECTIVE_DPI, &mut mx, &mut my);
            (w, mx)
        }
    }

    impl Capturer for Print {
        fn kind(&self) -> &'static str {
            "printwindow"
        }
        fn display_size(&self) -> Option<(u32, u32)> {
            self.display
        }
        fn grab(&mut self) -> Result<Option<Frame>, String> {
            if !alive(self.hwnd) {
                return Err("the window is gone".into());
            }
            let (w, h) = client_size(self.hwnd);
            if w <= 0 || h <= 0 {
                return Ok(None); // reduite
            }
            unsafe {
                let hdc = GetDC(Some(self.hwnd));
                let mem = CreateCompatibleDC(Some(hdc));
                let bmp = CreateCompatibleBitmap(hdc, w, h);
                let old = SelectObject(mem, bmp.into());
                let ok = PrintWindow(self.hwnd, mem, PRINT_WINDOW_FLAGS(PW_CLIENTONLY.0 | PW_RENDERFULLCONTENT)).as_bool();
                let mut bi = BITMAPINFO::default();
                bi.bmiHeader = BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                };
                let mut buf = vec![0u8; (w * h * 4) as usize];
                let lines = GetDIBits(mem, bmp, 0, h as u32, Some(buf.as_mut_ptr() as *mut core::ffi::c_void), &mut bi, DIB_RGB_COLORS);
                SelectObject(mem, old);
                let _ = DeleteObject(bmp.into());
                let _ = DeleteDC(mem);
                ReleaseDC(Some(self.hwnd), hdc);
                if !ok || lines == 0 {
                    return Ok(None);
                }
                // PrintWindow laisse l'alpha a 0 : on le met a 255 (le JPEG l'ignore, mais l'image reste propre)
                for px in buf.chunks_exact_mut(4) {
                    px[3] = 255;
                }
                // mesure du 20 oct. (Diva, ecran a 125 %) : une fenetre « DPI unaware » est rendue a sa taille LOGIQUE
                // (1200×670) dans le coin d'une image physique (1500×837), le reste noir. On garde la partie utile ;
                // ses pixels sont ceux de la fenetre (les gestes y tombent juste) et le studio l'agrandit a l'ecran.
                let (wd, md) = dpis(self.hwnd);
                let (lw, lh) = super::logical_size(w as u32, h as u32, wd, md);
                if (lw, lh) != (w as u32, h as u32) {
                    self.display = Some((w as u32, h as u32));
                    let f = Frame { w: w as u32, h: h as u32, bgra: buf };
                    return Ok(Some(crate::stream::crop(&f, (0, 0, lw, lh))));
                }
                self.display = None;
                Ok(Some(Frame { w: w as u32, h: h as u32, bgra: buf }))
            }
        }
    }

    // ── La surface DWM ────────────────────────────────────────────────────────────────────────────────────────
    // DwmGetDxSharedSurface (user32, non documentee, celle qu'utilisent les outils de capture) rend la surface de
    // redirection de la fenetre : ce que DWM compose a l'ecran, a la taille ou le plugin se dessine (1216×709 pour
    // Diva « DPI unaware » : sa zone client logique 1200×670 et son cadre). On la lit par Direct3D 11, sans rien
    // demander au fil de la fenetre (PrintWindow, lui, fait redessiner le plugin sur le fil de Live).

    type DxSurfaceFn = unsafe extern "system" fn(HWND, *mut HANDLE, *mut LUID, *mut u32, *mut u32, *mut u64) -> windows::core::BOOL;

    fn dx_surface_fn() -> Option<DxSurfaceFn> {
        static F: OnceLock<Option<usize>> = OnceLock::new();
        let p = *F.get_or_init(|| unsafe {
            let m = LoadLibraryW(w!("user32.dll")).ok()?;
            GetProcAddress(m, s!("DwmGetDxSharedSurface")).map(|f| f as usize)
        });
        // SAFETY : l'adresse vient de GetProcAddress pour cette signature (celle des outils de capture connus)
        p.map(|p| unsafe { std::mem::transmute::<usize, DxSurfaceFn>(p) })
    }

    /// Les formats de surface lus tels quels (BGRA 8 bits).
    fn bgra_format(f: DXGI_FORMAT) -> bool {
        f == DXGI_FORMAT_B8G8R8A8_UNORM || f == DXGI_FORMAT_B8G8R8A8_UNORM_SRGB || f == DXGI_FORMAT_B8G8R8X8_UNORM
    }

    pub struct Dwm {
        hwnd: HWND,
        f: DxSurfaceFn,
        device: ID3D11Device,
        ctx: ID3D11DeviceContext,
        /// la surface ouverte : son handle, la texture, sa taille
        tex: Option<(isize, ID3D11Texture2D, u32, u32)>,
        staging: Option<(ID3D11Texture2D, u32, u32)>,
        /// la zone client dans la surface, valable pour (taille de la surface, zone client physique)
        crop: Option<((u32, u32), (i32, i32), (u32, u32, u32, u32))>,
        display: Option<(u32, u32)>,
        fails: u32,
        /// la surface ne se lit plus : PrintWindow prend le relais
        fallback: Option<Print>,
    }
    unsafe impl Send for Dwm {}

    /// Les recalages reussis : (fenetre, taille de la surface, zone client) → (la zone client dans la surface, la taille a
    /// l'ecran d'un plugin agrandi par Windows). Une fenetre refermee puis rouverte a un autre handle : recalee de nouveau.
    type CalKey = (u64, u32, u32, i32, i32);
    type CalVal = ((u32, u32, u32, u32), Option<(u32, u32)>);
    fn calibrations() -> &'static std::sync::Mutex<std::collections::HashMap<CalKey, CalVal>> {
        static C: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<CalKey, CalVal>>> = std::sync::OnceLock::new();
        C.get_or_init(Default::default)
    }

    /// Apres tant d'echecs de suite (fenetre visible), la capture repasse a PrintWindow.
    const DWM_MAX_FAILS: u32 = 30;
    /// Le recalage exige au moins cette part de pixels egaux a PrintWindow.
    const DWM_MIN_MATCH: f64 = 0.75; // Diva rouvert, 21 oct. : 93,7 % (a 12 niveaux pres) ; une mauvaise surface : < 30 %

    impl Dwm {
        pub fn new(id: u64) -> Result<Self, String> {
            let hwnd = hwnd_of(id);
            if !alive(hwnd) {
                return Err("the window is gone".into());
            }
            let f = dx_surface_fn().ok_or("no DwmGetDxSharedSurface in user32")?;
            let mut device: Option<ID3D11Device> = None;
            let mut ctx: Option<ID3D11DeviceContext> = None;
            unsafe {
                D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut ctx))
                    .map_err(|e| format!("D3D11CreateDevice: {e}"))?;
            }
            let mut d = Dwm { hwnd, f, device: device.ok_or("no D3D11 device")?, ctx: ctx.ok_or("no D3D11 context")?, tex: None, staging: None, crop: None, display: None, fails: 0, fallback: None };
            // tout de suite : la surface existe-t-elle, et correspond-elle a la fenetre ? (sinon : PrintWindow)
            let (cw, ch) = client_size(hwnd);
            if cw > 0 && ch > 0 && !unsafe { IsIconic(hwnd).as_bool() } {
                let (sw, sh) = d.surface().ok_or("the window has no DWM surface")?;
                d.calibrate(id, sw, sh, cw, ch)?;
            }
            Ok(d)
        }

        /// La surface de la fenetre (ouverte une fois par handle) : sa taille.
        fn surface(&mut self) -> Option<(u32, u32)> {
            unsafe {
                let mut handle = HANDLE::default();
                let mut luid = LUID::default();
                let (mut fmt, mut flags, mut upd) = (0u32, 0u32, 0u64);
                if !(self.f)(self.hwnd, &mut handle, &mut luid, &mut fmt, &mut flags, &mut upd).as_bool() || handle.0.is_null() || handle.is_invalid() {
                    return None;
                }
                let key = handle.0 as isize;
                if let Some((k, _, w, h)) = &self.tex {
                    if *k == key {
                        return Some((*w, *h));
                    }
                }
                let mut tex: Option<ID3D11Texture2D> = None;
                self.device.OpenSharedResource(handle, &mut tex).ok()?;
                let tex = tex?;
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                tex.GetDesc(&mut desc);
                if !bgra_format(desc.Format) {
                    return None;
                }
                self.tex = Some((key, tex, desc.Width, desc.Height));
                Some((desc.Width, desc.Height))
            }
        }

        /// Un rectangle de la surface, en BGRA (alpha a 255).
        fn read(&mut self, r: (u32, u32, u32, u32)) -> Option<Frame> {
            let (x, y, w, h) = r;
            if w == 0 || h == 0 {
                return None;
            }
            let tex = self.tex.as_ref()?.1.clone();
            unsafe {
                let need = !matches!(&self.staging, Some((_, sw, sh)) if *sw == w && *sh == h);
                if need {
                    let sd = D3D11_TEXTURE2D_DESC {
                        Width: w,
                        Height: h,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                        Usage: D3D11_USAGE_STAGING,
                        BindFlags: 0,
                        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                        MiscFlags: 0,
                    };
                    let mut st: Option<ID3D11Texture2D> = None;
                    self.device.CreateTexture2D(&sd, None, Some(&mut st)).ok()?;
                    self.staging = Some((st?, w, h));
                }
                let staging = self.staging.as_ref()?.0.clone();
                let b = D3D11_BOX { left: x, top: y, front: 0, right: x + w, bottom: y + h, back: 1 };
                self.ctx.CopySubresourceRegion(&staging, 0, 0, 0, 0, &tex, 0, Some(&b));
                let mut m = D3D11_MAPPED_SUBRESOURCE::default();
                self.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m)).ok()?;
                let mut out = Vec::with_capacity((w * h * 4) as usize);
                let src = m.pData as *const u8;
                for row in 0..h {
                    out.extend_from_slice(std::slice::from_raw_parts(src.add((row * m.RowPitch) as usize), (w * 4) as usize));
                }
                self.ctx.Unmap(&staging, 0);
                for px in out.chunks_exact_mut(4) {
                    px[3] = 255;
                }
                Some(Frame { w, h, bgra: out })
            }
        }

        /// Ou est la zone client dans la surface : devinee (cadre et echelle), puis recalee au pixel sur l'image de
        /// PrintWindow (une seule fois par taille). Err : la surface ne montre pas la fenetre.
        fn calibrate(&mut self, id: u64, sw: u32, sh: u32, cw: i32, ch: i32) -> Result<(), String> {
            let (wr, p) = unsafe {
                let mut wr = RECT::default();
                let _ = GetWindowRect(self.hwnd, &mut wr);
                let mut p = POINT { x: 0, y: 0 };
                let _ = ClientToScreen(self.hwnd, &mut p);
                (wr, p)
            };
            // (23 oct.) la meme fenetre, aux memes tailles, deja recalee dans cette vie du compagnon : le meme recalage
            // (Follow track revient souvent a une fenetre restee ouverte ; le recalage coute ~0,2 s et un PrintWindow
            // sur le fil de Live)
            if let Some((rect, display)) = calibrations().lock().ok().and_then(|m| m.get(&(id, sw, sh, cw, ch)).copied()) {
                self.crop = Some(((sw, sh), (cw, ch), rect));
                self.display = display;
                return Ok(());
            }
            let (ww, wh) = ((wr.right - wr.left).max(1) as f64, (wr.bottom - wr.top).max(1) as f64);
            let guess = (((p.x - wr.left) as f64 * sw as f64 / ww).round() as i32, ((p.y - wr.top) as f64 * sh as f64 / wh).round() as i32);
            let mut last = 0.0;
            for _ in 0..3 {
                let reference = Print::new(id)?.grab()?.ok_or("PrintWindow gave nothing")?;
                let full = self.read((0, 0, sw, sh)).ok_or("cannot read the DWM surface")?;
                let Some(((ox, oy), part)) = super::best_offset(&full, &reference, guess, 6, 3) else {
                    return Err(format!("the DWM surface ({sw}×{sh}) is smaller than the window ({}×{})", reference.w, reference.h));
                };
                last = part;
                // UWC_DWM_DUMP=DOSSIER (diagnostic) : la surface et l'image de PrintWindow comparees
                if let Some(dir) = std::env::var_os("UWC_DWM_DUMP") {
                    let dir = std::path::PathBuf::from(dir);
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::fs::write(dir.join("dwm-surface.jpg"), crate::encode::jpeg(&full, 90).unwrap_or_default());
                    let _ = std::fs::write(dir.join("printwindow.jpg"), crate::encode::jpeg(&reference, 90).unwrap_or_default());
                    crate::log(&format!("DWM surface {sw}×{sh}, guess {guess:?}, best {ox},{oy}: {:.1} % equal", part * 100.0));
                }
                if part >= DWM_MIN_MATCH {
                    self.crop = Some(((sw, sh), (cw, ch), (ox, oy, reference.w, reference.h)));
                    self.display = if reference.w != cw as u32 || reference.h != ch as u32 { Some((cw as u32, ch as u32)) } else { None };
                    if let Ok(mut m) = calibrations().lock() {
                        m.insert((id, sw, sh, cw, ch), ((ox, oy, reference.w, reference.h), self.display));
                    }
                    crate::log(&format!("DWM surface {sw}×{sh}: client {}×{} at {ox},{oy} ({:.1} % equal to PrintWindow)", reference.w, reference.h, part * 100.0));
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            Err(format!("the DWM surface does not match the window ({:.0} % equal)", last * 100.0))
        }
    }

    impl Capturer for Dwm {
        fn kind(&self) -> &'static str {
            if self.fallback.is_some() {
                "printwindow"
            } else {
                "dwm"
            }
        }
        fn display_size(&self) -> Option<(u32, u32)> {
            match &self.fallback {
                Some(p) => p.display_size(),
                None => self.display,
            }
        }
        fn grab(&mut self) -> Result<Option<Frame>, String> {
            if !alive(self.hwnd) {
                return Err("the window is gone".into());
            }
            if let Some(p) = self.fallback.as_mut() {
                return p.grab();
            }
            let (cw, ch) = client_size(self.hwnd);
            if cw <= 0 || ch <= 0 || unsafe { IsIconic(self.hwnd).as_bool() } {
                return Ok(None); // reduite : plus dessinee
            }
            let id = self.hwnd.0 as isize as u64;
            let Some((sw, sh)) = self.surface() else {
                self.fails += 1;
                if self.fails >= DWM_MAX_FAILS {
                    crate::log("the DWM surface cannot be read any more: using PrintWindow");
                    self.fallback = Some(Print::new(id)?);
                }
                return Ok(None);
            };
            self.fails = 0;
            let ok = matches!(self.crop, Some((s, c, _)) if s == (sw, sh) && c == (cw, ch));
            if !ok {
                if let Err(e) = self.calibrate(id, sw, sh, cw, ch) {
                    crate::log(&format!("{e}: using PrintWindow"));
                    self.fallback = Some(Print::new(id)?);
                    return self.fallback.as_mut().unwrap().grab();
                }
            }
            let rect = self.crop.unwrap().2;
            Ok(self.read(rect))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{best_offset, clamp_crop, learn, learned_of, logical_size, open, Frame, LEARN_NO_DWM, LEARN_NO_WGC};

    #[test]
    fn what_failed_for_a_plugin_is_not_tried_again() {
        assert_eq!(learned_of("Test Kontakt 23"), 0);
        learn("Test Kontakt 23", LEARN_NO_WGC);
        learn("Test Kontakt 23", LEARN_NO_DWM);
        learn("Test Diva 23", LEARN_NO_WGC);
        assert_eq!(learned_of("Test Kontakt 23"), LEARN_NO_WGC | LEARN_NO_DWM);
        assert_eq!(learned_of("Test Diva 23"), LEARN_NO_WGC, "per plugin");
        learn("", LEARN_NO_DWM);
        assert_eq!(learned_of(""), 0, "an unnamed window learns nothing");
    }

    #[test]
    fn dpi_unaware_windows_are_captured_at_their_logical_size() {
        assert_eq!(logical_size(1500, 837, 96, 120), (1200, 670), "Diva at 125 %");
        assert_eq!(logical_size(1500, 837, 120, 120), (1500, 837));
        assert_eq!(logical_size(1500, 837, 0, 120), (1500, 837));
        assert_eq!(logical_size(1500, 837, 144, 120), (1500, 837));
    }

    #[test]
    fn the_dwm_surface_is_registered_on_the_client_area() {
        // une « surface » 40×30 (cadre compris) et la zone client 20×10 en (7, 9), avec un motif
        let mut surf = Frame { w: 40, h: 30, bgra: vec![0; 40 * 30 * 4] };
        let mut client = Frame { w: 20, h: 10, bgra: vec![0; 20 * 10 * 4] };
        for y in 0..10u32 {
            for x in 0..20u32 {
                let v = ((x * 13 + y * 7) % 251) as u8;
                let i = ((y * 20 + x) * 4) as usize;
                client.bgra[i..i + 3].copy_from_slice(&[v, v / 2, 255 - v]);
                let j = (((y + 9) * 40 + x + 7) * 4) as usize;
                surf.bgra[j..j + 3].copy_from_slice(&[v, v / 2, 255 - v]);
            }
        }
        assert_eq!(best_offset(&surf, &client, (8, 8), 3, 1), Some(((7, 9), 1.0)), "found from a guess 1 px off");
        // a quelques niveaux pres (PrintWindow redessine, la surface garde le dessin de l'ecran)
        let mut noisy = Frame { w: surf.w, h: surf.h, bgra: surf.bgra.iter().map(|v| v.saturating_add(5)).collect() };
        noisy.bgra[0] = 0;
        assert_eq!(best_offset(&noisy, &client, (8, 8), 3, 1).map(|b| b.0), Some((7, 9)), "small differences still match");
        assert!(best_offset(&surf, &client, (0, 0), 2, 1).unwrap().1 < 0.9, "too far from the guess: no match");
        assert_eq!(best_offset(&client, &surf, (0, 0), 2, 1), None, "a surface smaller than the window");
    }

    /// La vraie fenetre d'essai, « DPI unaware » et au fond (derriere les autres fenetres) : la surface DWM se recale
    /// sur PrintWindow, rend la taille logique, et voit le potard bouger sans rien demander a la fenetre.
    #[cfg(windows)]
    #[test]
    fn the_dwm_surface_shows_a_hidden_dpi_unaware_window() {
        use crate::testwin;
        let t = testwin::open_with((60, 200), true, true).expect("test window");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let mut d = match open(t.top, Some("dwm")) {
            Ok(d) => d,
            Err(e) => panic!("DWM capture: {e}"),
        };
        let a = d.grab().unwrap().expect("a frame");
        assert_eq!((a.w, a.h), (crate::testwin::CLIENT_W as u32, crate::testwin::CLIENT_H as u32), "the logical size");
        if let Some((dw, dh)) = d.display_size() {
            assert!(dw > a.w && dh > a.h, "on screen bigger: {dw}×{dh}");
        }
        crate::input::win::post(t.knob, &crate::input::WinMsg { msg: 0x0201, wparam: 1, lparam: crate::input::make_lparam(100, 100) }).unwrap();
        let t0 = std::time::Instant::now();
        let mut changed = false;
        while t0.elapsed() < std::time::Duration::from_millis(1000) {
            if let Ok(Some(b)) = d.grab() {
                if crate::stream::dirty_rect(&a, &b).is_some() {
                    changed = true;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let _ = crate::input::win::post(t.knob, &crate::input::WinMsg { msg: 0x0202, wparam: 0, lparam: crate::input::make_lparam(100, 100) });
        assert!(changed, "the press is seen through the DWM surface");
        assert_eq!(d.kind(), "dwm");
    }

    /// Le plantage du 21 oct. : un fil de connexion ouvre WGC puis se termine ; le suivant ouvrait WGC sur un MTA
    /// demonte. Le fil principal du compagnon garde le MTA (keep_com_alive) : ici un fil qui vit toute la suite.
    #[cfg(windows)]
    #[test]
    fn capture_opens_again_after_a_connection_thread_ended() {
        use crate::testwin;
        std::thread::spawn(|| {
            super::keep_com_alive();
            loop {
                std::thread::park();
            }
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        let t = testwin::open_with((60, 420), false, true).expect("test window");
        std::thread::sleep(std::time::Duration::from_millis(200));
        let top = t.top;
        for round in 0..3 {
            // comme une connexion : un fil qui ouvre la capture (WGC d'abord), lit une image, puis se termine
            let r = std::thread::spawn(move || {
                let mut c = open(top, None).map_err(|e| e.to_string())?;
                let mut got = false;
                for _ in 0..50 {
                    if let Ok(Some(_)) = c.grab() {
                        got = true;
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Ok::<_, String>((c.kind(), got))
            })
            .join()
            .expect("the capture thread did not crash");
            let (kind, got) = r.unwrap_or_else(|e| panic!("round {round}: {e}"));
            assert!(got, "round {round}: a frame with {kind}");
        }
    }

    #[test]
    fn crop_stays_inside_the_image() {
        assert_eq!(clamp_crop(8, 31, 800, 500, 816, 539), (8, 31, 800, 500));
        assert_eq!(clamp_crop(-5, -5, 900, 900, 816, 539), (0, 0, 816, 539));
        assert_eq!(clamp_crop(900, 0, 10, 10, 816, 539), (816, 0, 0, 10));
    }
}
