// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! WASAPI (24 oct.) : la capture du son d'UN processus et de ses enfants (« process loopback » :
//! ActivateAudioInterfaceAsync sur le peripherique virtuel VAD\Process_Loopback, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
//! PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE) et la tonalite d'essai (--tone, un sinus en mode partage).
//!
//! Microsoft documente la capture d'un processus a partir de Windows 10 build 20348 ; OBS (« Application Audio
//! Capture ») s'en sert des Windows 10 2004 (build 19041). Elle ne voit que ce que le processus joue par le melangeur de
//! Windows (MME, DirectSound, WASAPI partage) : ni l'ASIO, ni le WASAPI exclusif.
//!
//! Lecture seule : rien n'est change dans les reglages audio de Windows ni du processus ecoute. La tonalite d'essai ne
//! touche qu'a sa propre session (--tone-mute, --tone-volume : la sienne seulement).

use super::packet::Chunk;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::core::{implement, Interface, IUnknown, Ref, HRESULT, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, IAgileObject, IAgileObject_Impl, BLOB, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{AvSetMmThreadCharacteristicsW, CreateEventW, SetEvent, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

pub fn hr(what: &str, e: &windows::core::Error) -> String {
    format!("{what} failed: 0x{:08X} {}", e.code().0 as u32, e.message().trim())
}

/// L'instant present en ms depuis 1970 (l'horloge de Date.now()).
pub fn epoch_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

/// Le compteur de performance (QPC) en ms — l'horloge des instants de capture de WASAPI.
pub fn qpc_ms() -> f64 {
    let (mut c, mut f) = (0i64, 0i64);
    unsafe {
        let _ = QueryPerformanceCounter(&mut c);
        let _ = QueryPerformanceFrequency(&mut f);
    }
    if f <= 0 {
        return 0.0;
    }
    (c as i128 * 1000 * 1_000_000 / f as i128) as f64 / 1_000_000.0
}

/// La version de Windows : (build, revision), du registre (lecture seule) ; GetVersionEx ment sans manifeste.
pub fn windows_build() -> (u32, u32) {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};
    let key = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
    let mut build = 0u32;
    let mut ubr = 0u32;
    unsafe {
        let mut buf = [0u16; 32];
        let mut n = (buf.len() * 2) as u32;
        if RegGetValueW(HKEY_LOCAL_MACHINE, key, w!("CurrentBuildNumber"), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut n)).is_ok() {
            let len = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
            build = String::from_utf16_lossy(&buf[..len]).trim().parse().unwrap_or(0);
        }
        let mut n = 4u32;
        let _ = RegGetValueW(HKEY_LOCAL_MACHINE, key, w!("UBR"), RRF_RT_REG_DWORD, None, Some(&mut ubr as *mut u32 as *mut _), Some(&mut n));
    }
    (build, ubr)
}

/// La fin de l'activation (asynchrone) : un evenement. Agile (IAgileObject) comme le demande Windows.
#[implement(IActivateAudioInterfaceCompletionHandler, IAgileObject)]
struct Activated {
    ev: isize,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
    fn ActivateCompleted(&self, _op: Ref<IActivateAudioInterfaceAsyncOperation>) -> windows::core::Result<()> {
        unsafe {
            let _ = SetEvent(HANDLE(self.ev as *mut _));
        }
        Ok(())
    }
}

impl IAgileObject_Impl for Activated_Impl {}

/// Un IAudioClient qui ecoute le processus `pid` (et ses enfants si `tree`).
pub fn activate_process_loopback(pid: u32, tree: bool) -> Result<IAudioClient, String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let ev = CreateEventW(None, false, false, PCWSTR::null()).map_err(|e| hr("CreateEvent", &e))?;
        let handler: IActivateAudioInterfaceCompletionHandler = Activated { ev: ev.0 as isize }.into();
        let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: if tree { PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE } else { PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE },
                },
            },
        };
        // ManuallyDrop : le Drop de PROPVARIANT (windows-rs) appelle PropVariantClear, qui rendrait au tas le BLOB
        // pose sur la pile (vu le 24 oct. : STATUS_HEAP_CORRUPTION)
        let mut pv = std::mem::ManuallyDrop::new(PROPVARIANT::default());
        {
            let inner = &mut *pv.Anonymous.Anonymous;
            inner.vt = VT_BLOB;
            inner.Anonymous.blob = BLOB { cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32, pBlobData: &mut params as *mut _ as *mut u8 };
        }
        let op = match ActivateAudioInterfaceAsync(VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, &IAudioClient::IID, Some(&*pv as *const PROPVARIANT), &handler) {
            Ok(op) => op,
            Err(e) => {
                let _ = CloseHandle(ev);
                return Err(hr("ActivateAudioInterfaceAsync(VAD\\Process_Loopback)", &e));
            }
        };
        let w = WaitForSingleObject(ev, 5000);
        let _ = CloseHandle(ev);
        if w != WAIT_OBJECT_0 {
            return Err("the process loopback activation did not complete within 5 s".into());
        }
        let mut res = HRESULT(0);
        let mut unk: Option<IUnknown> = None;
        op.GetActivateResult(&mut res, &mut unk).map_err(|e| hr("GetActivateResult", &e))?;
        if res.is_err() {
            return Err(hr("process loopback activation", &windows::core::Error::from_hresult(res)));
        }
        let unk = unk.ok_or("process loopback activation gave no interface")?;
        unk.cast::<IAudioClient>().map_err(|e| hr("IAudioClient", &e))
    }
}

fn wave(rate: u32, channels: u16, float: bool) -> WAVEFORMATEX {
    let bits: u16 = if float { 32 } else { 16 };
    let align = channels * bits / 8;
    WAVEFORMATEX {
        wFormatTag: if float { WAVE_FORMAT_IEEE_FLOAT } else { WAVE_FORMAT_PCM },
        nChannels: channels,
        nSamplesPerSec: rate,
        nAvgBytesPerSec: rate * align as u32,
        nBlockAlign: align,
        wBitsPerSample: bits,
        cbSize: 0,
    }
}

/// La capture en cours.
pub struct Loopback {
    _client: IAudioClient,
    capture: IAudioCaptureClient,
    ev: HANDLE,
    pub rate: u32,
    pub channels: usize,
    /// Le format que Windows nous rend (f32, ou i16 si le f32 a ete refuse).
    pub float: bool,
    pub activate_ms: f64,
    pub init_ms: f64,
    pub buffer_frames: u32,
    started: bool,
    /// la position (en trames du flux) du prochain morceau, et la position de peripherique attendue
    pos: u64,
    last_dev: Option<(u64, u64)>,
    scratch: Vec<f32>,
    /// ecart epoque − QPC (ms), refait a chaque morceau (l'horloge systeme peut etre recalee)
    pub dev_pos_jumps: u64,
}

// Les interfaces COM vivent dans le MTA : le fil de capture les cree et s'en sert seul.
unsafe impl Send for Loopback {}

impl Loopback {
    /// Ecouter `pid` (et ses enfants). Le format demande : `rate` Hz, stereo, f32 (sinon i16).
    pub fn open(pid: u32, tree: bool, rate: u32) -> Result<Loopback, String> {
        Self::open_with(rate, &mut || activate_process_loopback(pid, tree))
    }

    /// (diagnostic, la console seulement) la boucle classique d'une SORTIE entiere (tout ce qui y joue) : la reference
    /// a laquelle comparer la capture d'un processus.
    pub fn open_output(name: &str, rate: u32) -> Result<Loopback, String> {
        Self::open_with(rate, &mut || {
            let (dev, _) = device_named(name)?;
            unsafe { dev.Activate::<IAudioClient>(CLSCTX_ALL, None) }.map_err(|e| hr("IMMDevice::Activate", &e))
        })
    }

    fn open_with(rate: u32, activate: &mut dyn FnMut() -> Result<IAudioClient, String>) -> Result<Loopback, String> {
        let t0 = Instant::now();
        let mut errs = Vec::new();
        // (diagnostic) UWC_LOOPBACK=noconvert,ext,i16 : sans conversion de Windows, WAVEFORMATEXTENSIBLE, int16 d'abord
        let knobs = std::env::var("UWC_LOOPBACK").unwrap_or_default();
        let convert = !knobs.contains("noconvert");
        let ext = knobs.contains("ext");
        let order: [bool; 2] = if knobs.contains("i16") { [false, true] } else { [true, false] };
        for float in order {
            let client = activate()?;
            let activate_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let t1 = Instant::now();
            let wf = wave(rate, 2, float);
            let mut wfx = WAVEFORMATEXTENSIBLE::default();
            wfx.Format = wf;
            wfx.Format.wFormatTag = 0xFFFE; // WAVE_FORMAT_EXTENSIBLE
            wfx.Format.cbSize = 22;
            wfx.Samples = WAVEFORMATEXTENSIBLE_0 { wValidBitsPerSample: wf.wBitsPerSample };
            wfx.dwChannelMask = 3; // avant gauche, avant droite
            wfx.SubFormat = if float { windows::core::GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71) } else { windows::core::GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71) };
            let pf: *const WAVEFORMATEX = if ext { &wfx as *const WAVEFORMATEXTENSIBLE as *const WAVEFORMATEX } else { &wf };
            let mut flags = AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
            if convert {
                flags |= AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
            }
            // 20 ms de tampon cote Windows (il rend des morceaux de ~10 ms) ; (diagnostic) UWC_LOOPBACK=buf=N (100 ns)
            let buf = knobs.split(',').find_map(|k| k.strip_prefix("buf=")).and_then(|v| v.parse::<i64>().ok()).unwrap_or(200_000);
            if let Err(e) = unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, buf, 0, pf, None) } {
                errs.push(hr(if float { "Initialize (float32)" } else { "Initialize (int16)" }, &e));
                continue;
            }
            let init_ms = t1.elapsed().as_secs_f64() * 1000.0;
            unsafe {
                let ev = CreateEventW(None, false, false, PCWSTR::null()).map_err(|e| hr("CreateEvent", &e))?;
                client.SetEventHandle(ev).map_err(|e| hr("SetEventHandle", &e))?;
                let capture: IAudioCaptureClient = client.GetService().map_err(|e| hr("GetService(IAudioCaptureClient)", &e))?;
                // GetBufferSize : sur la capture d'un processus (vu sous Windows 10 22H2) il rend n'importe quoi (vu : 3 781 713 120) :
                // 0 = inconnu
                let buffer_frames = client.GetBufferSize().ok().filter(|n| *n < rate * 10).unwrap_or(0);
                return Ok(Loopback {
                    _client: client,
                    capture,
                    ev,
                    rate,
                    channels: 2,
                    float,
                    activate_ms,
                    init_ms,
                    buffer_frames,
                    started: false,
                    pos: 0,
                    last_dev: None,
                    scratch: Vec::new(),
                    dev_pos_jumps: 0,
                });
            }
        }
        Err(errs.join("; "))
    }

    pub fn start(&mut self) -> Result<(), String> {
        if !self.started {
            unsafe { self._client.Start() }.map_err(|e| hr("Start", &e))?;
            self.started = true;
        }
        Ok(())
    }

    /// Attendre au plus `timeout` le prochain evenement, puis rendre tous les morceaux prets a `f`. → le nombre de
    /// morceaux (0 : rien pendant `timeout` — le processus ne joue rien, ou Windows ne signale rien).
    pub fn read(&mut self, timeout: Duration, f: &mut dyn FnMut(&Chunk)) -> Result<usize, String> {
        let w = unsafe { WaitForSingleObject(self.ev, timeout.as_millis().min(u32::MAX as u128) as u32) };
        let _ = w; // (meme sans evenement, on regarde : certains peripheriques virtuels signalent mal)
        let mut n = 0;
        loop {
            let size = unsafe { self.capture.GetNextPacketSize() }.map_err(|e| hr("GetNextPacketSize", &e))?;
            if size == 0 {
                break;
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let (mut frames, mut flags, mut dev, mut qpc) = (0u32, 0u32, 0u64, 0u64);
            unsafe { self.capture.GetBuffer(&mut data, &mut frames, &mut flags, Some(&mut dev), Some(&mut qpc)) }.map_err(|e| hr("GetBuffer", &e))?;
            let now_qpc = qpc_ms();
            let now_epoch = epoch_ms();
            // l'instant de la premiere trame (QPC en unites de 100 ns) → ms depuis 1970
            let t_ms = if qpc > 0 { now_epoch - (now_qpc - qpc as f64 / 10_000.0) } else { now_epoch - frames as f64 * 1000.0 / self.rate as f64 };
            let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
            let mut discontinuity = flags & (AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32) != 0;
            // la position du peripherique saute (de la capture perdue) : la position du flux saute d'autant
            if let Some((ld, lf)) = self.last_dev {
                let want = ld + lf;
                if dev > want && dev - want < self.rate as u64 * 10 {
                    self.pos += dev - want;
                    self.dev_pos_jumps += 1;
                    discontinuity = true;
                }
            }
            self.last_dev = if dev > 0 || self.last_dev.is_some() { Some((dev, frames as u64)) } else { None };
            let fr = frames as usize;
            let n_s = fr * self.channels;
            self.scratch.clear();
            if !silent && !data.is_null() && fr > 0 {
                unsafe {
                    if self.float {
                        let s = std::slice::from_raw_parts(data as *const f32, n_s);
                        self.scratch.extend_from_slice(s);
                    } else {
                        let s = std::slice::from_raw_parts(data as *const i16, n_s);
                        self.scratch.extend(s.iter().map(|v| *v as f32 / 32768.0));
                    }
                }
            }
            let c = Chunk { samples: &self.scratch, frames: fr, pos: self.pos, t_ms, silent: silent || self.scratch.is_empty(), discontinuity };
            f(&c);
            self.pos += fr as u64;
            unsafe { self.capture.ReleaseBuffer(frames) }.map_err(|e| hr("ReleaseBuffer", &e))?;
            n += 1;
        }
        Ok(n)
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        unsafe {
            if self.started {
                let _ = self._client.Stop();
            }
            let _ = CloseHandle(self.ev);
        }
    }
}

/// Le fil de capture en priorite « Pro Audio » (MMCSS), comme un DAW.
pub fn pro_audio_thread() {
    use windows::core::w;
    let mut idx = 0u32;
    unsafe {
        let _ = AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut idx);
    }
}

/// La capture d'un processus marche-t-elle sur ce Windows ? Un essai sur le compagnon lui-meme (rien n'est ecoute :
/// le client est cree, initialise, puis relache). → Ok(ms) ou l'erreur exacte.
pub fn probe() -> Result<f64, String> {
    let t0 = Instant::now();
    let lb = Loopback::open(std::process::id(), true, 48000)?;
    drop(lb);
    Ok(t0.elapsed().as_secs_f64() * 1000.0)
}

// ── La tonalite d'essai (--tone) ──────────────────────────────────────────────────────────────────────────────────

pub struct ToneOpts {
    pub seconds: f64,
    pub hz: f64,
    /// le sinus, en dB plein echelle (None : pas de sinus, seulement les clics)
    pub db: Option<f64>,
    /// un clic toutes les `click_ms` (cale sur l'horloge : a chaque multiple de click_ms depuis 1970)
    pub click_ms: Option<f64>,
    pub click_db: f64,
    /// une ligne JSON par clic : son numero, l'instant ou il est ecrit, l'instant ou il doit sortir
    pub click_log: Option<PathBuf>,
    /// un bout du nom du peripherique de sortie (sinon celui par defaut)
    pub device: Option<String>,
    /// la session de la tonalite, coupee ou a un volume (la sienne seulement), ou remise a 100 % non coupee
    pub mute: bool,
    pub volume: Option<f32>,
    pub unmute: bool,
    /// s'arreter quand ce processus n'est plus la (le compagnon qui l'a lancee)
    pub parent: Option<u32>,
    /// attendre avant d'ouvrir la sortie (s) : la capture commence avant le son
    pub delay: f64,
}

fn device_named(want: &str) -> Result<(IMMDevice, String), String> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
    use windows::Win32::System::Com::STGM_READ;
    use windows::Win32::System::Variant::VT_LPWSTR;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| hr("MMDeviceEnumerator", &e))?;
        let col = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE).map_err(|e| hr("EnumAudioEndpoints", &e))?;
        let n = col.GetCount().unwrap_or(0);
        let mut names = Vec::new();
        for i in 0..n {
            let Ok(dev) = col.Item(i) else { continue };
            let Ok(store) = dev.OpenPropertyStore(STGM_READ) else { continue };
            let mut name = String::new();
            if let Ok(mut v) = store.GetValue(&PKEY_Device_FriendlyName) {
                if v.Anonymous.Anonymous.vt == VT_LPWSTR {
                    name = v.Anonymous.Anonymous.Anonymous.pwszVal.to_string().unwrap_or_default();
                }
                let _ = PropVariantClear(&mut v);
            }
            if name.to_lowercase().contains(&want.to_lowercase()) {
                return Ok((dev, name));
            }
            names.push(name);
        }
        Err(format!("no active output device matching \"{want}\" (outputs: {})", names.join(", ")))
    }
}

/// Le nom du peripherique de sortie par defaut (lecture seule ; le diagnostic).
pub fn default_output_name() -> String {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
    use windows::Win32::System::Com::STGM_READ;
    use windows::Win32::System::Variant::VT_LPWSTR;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return String::new() };
        let Ok(dev) = en.GetDefaultAudioEndpoint(eRender, eConsole) else { return "(none)".into() };
        let Ok(store) = dev.OpenPropertyStore(STGM_READ) else { return String::new() };
        let mut name = String::new();
        if let Ok(mut v) = store.GetValue(&PKEY_Device_FriendlyName) {
            if v.Anonymous.Anonymous.vt == VT_LPWSTR {
                name = v.Anonymous.Anonymous.Anonymous.pwszVal.to_string().unwrap_or_default();
            }
            let _ = PropVariantClear(&mut v);
        }
        name
    }
}

pub struct ToneReport {
    pub device: String,
    pub frames: u64,
    pub clicks: u64,
    pub buffer_frames: u32,
    pub underruns: u64,
    /// la session (coupee ?, volume) a l'ouverture, puis pendant la tonalite
    pub session_before: (bool, f32),
    pub session: (bool, f32),
}

/// Jouer la tonalite : un sinus (et des clics) en WASAPI partage, sur le peripherique par defaut (ou `device`).
pub fn tone(o: &ToneOpts) -> Result<ToneReport, String> {
    if o.delay > 0.0 {
        std::thread::sleep(Duration::from_secs_f64(o.delay.min(30.0)));
    }
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let (dev, name) = match &o.device {
            Some(d) => device_named(d)?,
            None => {
                let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| hr("MMDeviceEnumerator", &e))?;
                (en.GetDefaultAudioEndpoint(eRender, eConsole).map_err(|e| hr("GetDefaultAudioEndpoint", &e))?, "default output".to_string())
            }
        };
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).map_err(|e| hr("IMMDevice::Activate", &e))?;
        let rate = 48000u32;
        let wf = wave(rate, 2, true);
        let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        // 10 ms : un DAW a ~480 echantillons
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 100_000, 0, &wf, None).map_err(|e| hr("Initialize (tone)", &e))?;
        let ev = CreateEventW(None, false, false, PCWSTR::null()).map_err(|e| hr("CreateEvent", &e))?;
        client.SetEventHandle(ev).map_err(|e| hr("SetEventHandle", &e))?;
        let render: IAudioRenderClient = client.GetService().map_err(|e| hr("GetService(IAudioRenderClient)", &e))?;
        // la session de la tonalite (la sienne seulement). Windows RETIENT le volume et la coupure d'une appli d'un
        // lancement a l'autre (vu le 24 oct. : une tonalite coupee une fois l'etait encore aux suivantes) : l'etat
        // d'avant est remis a la fin, et --tone-unmute la remet a 100 %, non coupee
        let vol: ISimpleAudioVolume = client.GetService().map_err(|e| hr("GetService(ISimpleAudioVolume)", &e))?;
        let before = (vol.GetMute().map(|b| b.as_bool()).unwrap_or(false), vol.GetMasterVolume().unwrap_or(1.0));
        if o.unmute {
            let _ = vol.SetMute(false, std::ptr::null());
            let _ = vol.SetMasterVolume(1.0, std::ptr::null());
        }
        if let Some(v) = o.volume {
            vol.SetMasterVolume(v.clamp(0.0, 1.0), std::ptr::null()).map_err(|e| hr("SetMasterVolume", &e))?;
        }
        if o.mute {
            vol.SetMute(true, std::ptr::null()).map_err(|e| hr("SetMute", &e))?;
        }
        let session = (vol.GetMute().map(|b| b.as_bool()).unwrap_or(false), vol.GetMasterVolume().unwrap_or(1.0));
        let bufsize = client.GetBufferSize().map_err(|e| hr("GetBufferSize", &e))?;
        let mut log = o.click_log.as_ref().and_then(|p| std::fs::File::create(p).ok());
        let amp = o.db.map(|d| super::packet::amp(d) as f32).unwrap_or(0.0);
        let click_amp = super::packet::amp(o.click_db) as f32;
        let click_len = (rate / 1000) as u64; // 1 ms
        let w = 2.0 * std::f64::consts::PI * o.hz / rate as f64;
        let mut phase = 0u64;
        let mut clicks = 0u64;
        let mut underruns = 0u64;
        // le clic en cours : les trames qui restent
        let mut click_left = 0u64;
        let mut next_click: Option<f64> = o.click_ms.map(|c| ((epoch_ms() / c).floor() + 2.0) * c);
        // pre-remplir de silence, puis demarrer
        if let Ok(p) = render.GetBuffer(bufsize) {
            std::ptr::write_bytes(p, 0, bufsize as usize * 8);
            let _ = render.ReleaseBuffer(bufsize, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32);
        }
        client.Start().map_err(|e| hr("Start (tone)", &e))?;
        pro_audio_thread();
        let t0 = Instant::now();
        let mut last_parent_check = Instant::now();
        while t0.elapsed().as_secs_f64() < o.seconds {
            let _ = WaitForSingleObject(ev, 200);
            if let Some(pp) = o.parent {
                if last_parent_check.elapsed() > Duration::from_millis(500) {
                    last_parent_check = Instant::now();
                    if !super::procs::alive(pp) {
                        break;
                    }
                }
            }
            let padding = client.GetCurrentPadding().unwrap_or(bufsize);
            if padding == 0 {
                underruns += 1;
            }
            let avail = bufsize.saturating_sub(padding);
            if avail == 0 {
                continue;
            }
            let Ok(p) = render.GetBuffer(avail) else { continue };
            let out = std::slice::from_raw_parts_mut(p as *mut f32, avail as usize * 2);
            let now = epoch_ms();
            let ms_per_frame = 1000.0 / rate as f64;
            for i in 0..avail as usize {
                let due = now + (padding as usize + i) as f64 * ms_per_frame;
                if let (Some(nc), Some(every)) = (next_click, o.click_ms) {
                    if due >= nc {
                        clicks += 1;
                        click_left = click_len;
                        if let Some(f) = log.as_mut() {
                            use std::io::Write;
                            let _ = writeln!(f, "{{\"n\":{clicks},\"at\":{nc:.3},\"due\":{due:.3},\"written\":{now:.3},\"padding\":{padding},\"offset\":{i}}}");
                        }
                        let mut n2 = nc + every;
                        while n2 <= due {
                            n2 += every;
                        }
                        next_click = Some(n2);
                    }
                }
                let mut s = (amp as f64 * ((phase as f64) * w).sin()) as f32;
                if click_left > 0 {
                    s += click_amp;
                    click_left -= 1;
                }
                out[i * 2] = s;
                out[i * 2 + 1] = s;
                phase += 1;
            }
            let _ = render.ReleaseBuffer(avail, 0);
        }
        let _ = client.Stop();
        if !o.unmute && (o.mute || o.volume.is_some()) {
            let _ = vol.SetMute(before.0, std::ptr::null());
            let _ = vol.SetMasterVolume(before.1, std::ptr::null());
        }
        let _ = CloseHandle(ev);
        Ok(ToneReport { device: name, frames: phase, clicks, buffer_frames: bufsize, underruns, session_before: before, session })
    }
}
