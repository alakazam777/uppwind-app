// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Le strict minimum d'un hote VST3 (Windows, et macOS depuis le 26 oct.) : charger le module d'un paquet .vst3, creer une classe par son nom,
//! la preparer comme un DAW (initialize, bus, setupProcessing, setActive, setProcessing), l'appeler (process), la
//! fermer. Les objets que l'hote fournit au plugin : IHostApplication et IEventList (les evenements de sortie).

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComWrapper, Interface};

/// Le module d'un paquet .vst3. Windows : Contents\x86_64-win\<nom>.vst3 ; macOS : Contents/MacOS/<nom sans .vst3>
/// (la regle de Steinberg pour un « bundle » Mach-O).
pub fn dll_of_bundle(bundle: &Path) -> PathBuf {
    if bundle.is_dir() {
        let name = bundle.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        if cfg!(target_os = "macos") {
            let stem = name.strip_suffix(".vst3").unwrap_or(&name).to_string();
            bundle.join("Contents").join("MacOS").join(stem)
        } else {
            bundle.join("Contents").join("x86_64-win").join(name)
        }
    } else {
        bundle.to_path_buf()
    }
}

fn cstr(buf: &[std::ffi::c_char]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

pub struct HostApp;

impl Class for HostApp {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for HostApp {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        if name.is_null() {
            return kInvalidArgument;
        }
        let s: Vec<u16> = "Uppwind Link test host".encode_utf16().collect();
        let dst = unsafe { &mut *name };
        let n = s.len().min(127);
        dst[..n].copy_from_slice(&s[..n]);
        dst[n] = 0;
        kResultOk
    }

    unsafe fn createInstance(&self, _cid: *mut TUID, _iid: *mut TUID, obj: *mut *mut c_void) -> tresult {
        if !obj.is_null() {
            unsafe { *obj = std::ptr::null_mut() };
        }
        kNotImplemented
    }
}

/// Une liste d'evenements tenue par l'hote (la sortie MIDI du plugin, ou une entree vide).
pub struct EventList {
    pub events: Mutex<Vec<Event>>,
}

impl Class for EventList {
    type Interfaces = (IEventList,);
}

impl IEventListTrait for EventList {
    unsafe fn getEventCount(&self) -> int32 {
        self.events.lock().unwrap().len() as int32
    }

    unsafe fn getEvent(&self, index: int32, e: *mut Event) -> tresult {
        match self.events.lock().unwrap().get(index as usize) {
            Some(ev) if !e.is_null() => {
                unsafe { *e = *ev };
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn addEvent(&self, e: *mut Event) -> tresult {
        if e.is_null() {
            return kInvalidArgument;
        }
        self.events.lock().unwrap().push(unsafe { *e });
        kResultOk
    }
}

/// (25 oct.) Les changements de parametres d'un bloc, comme un DAW les passe a process() (VST3 : inputParameterChanges).
/// nice-plug ignore setParamNormalized pendant le traitement : un vrai DAW (Live) envoie aussi le changement ici.
pub struct ParamQueue {
    id: ParamID,
    value: ParamValue,
}

impl Class for ParamQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for ParamQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id
    }
    unsafe fn getPointCount(&self) -> int32 {
        1
    }
    unsafe fn getPoint(&self, index: int32, offset: *mut int32, value: *mut ParamValue) -> tresult {
        if index != 0 || offset.is_null() || value.is_null() {
            return kInvalidArgument;
        }
        unsafe {
            *offset = 0;
            *value = self.value;
        }
        kResultOk
    }
    unsafe fn addPoint(&self, _offset: int32, _value: ParamValue, _index: *mut int32) -> tresult {
        kResultFalse
    }
}

pub struct ParamChanges {
    queues: Vec<ComWrapper<ParamQueue>>,
}

impl Class for ParamChanges {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for ParamChanges {
    unsafe fn getParameterCount(&self) -> int32 {
        self.queues.len() as int32
    }
    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        match self.queues.get(index as usize) {
            Some(q) => q.as_com_ref::<IParamValueQueue>().map(|r| r.as_ptr()).unwrap_or(std::ptr::null_mut()),
            None => std::ptr::null_mut(),
        }
    }
    unsafe fn addParameterData(&self, _id: *const ParamID, _index: *mut int32) -> *mut IParamValueQueue {
        std::ptr::null_mut()
    }
}

#[cfg(windows)]
type Lib = windows_sys::Win32::Foundation::HMODULE;
#[cfg(unix)]
type Lib = *mut c_void;

#[cfg(unix)]
mod dl {
    use std::ffi::{c_char, c_int, c_void};
    pub const RTLD_NOW: c_int = 2;
    extern "C" {
        pub fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
        pub fn dlsym(lib: *mut c_void, name: *const c_char) -> *mut c_void;
        pub fn dlclose(lib: *mut c_void) -> c_int;
        pub fn dlerror() -> *const c_char;
    }
}

pub struct Module {
    lib: Lib,
    factory: Option<ComPtr<IPluginFactory>>,
    exit: Option<unsafe extern "system" fn() -> bool>,
    pub path: PathBuf,
}

impl Module {
    /// macOS (26 oct.) : dlopen du binaire du paquet, puis bundleEntry (un CFBundleRef ; nice-plug ne le lit pas, on
    /// lui passe NULL — un vrai DAW passe le sien), GetPluginFactory ; a la fin bundleExit.
    #[cfg(unix)]
    pub fn load(bundle: &Path) -> Result<Module, String> {
        use std::os::unix::ffi::OsStrExt;
        let bin = dll_of_bundle(bundle);
        if !bin.is_file() {
            return Err(format!("no plugin module at {}", bin.display()));
        }
        let c = std::ffi::CString::new(bin.as_os_str().as_bytes()).map_err(|_| "a NUL in the path")?;
        unsafe {
            let lib = dl::dlopen(c.as_ptr(), dl::RTLD_NOW);
            if lib.is_null() {
                let e = dl::dlerror();
                let why = if e.is_null() { String::new() } else { std::ffi::CStr::from_ptr(e).to_string_lossy().into_owned() };
                return Err(format!("dlopen failed for {} ({why})", bin.display()));
            }
            let entry = dl::dlsym(lib, c"bundleEntry".as_ptr());
            if !entry.is_null() {
                let f: unsafe extern "system" fn(*mut c_void) -> bool = std::mem::transmute(entry);
                if !f(std::ptr::null_mut()) {
                    return Err("bundleEntry returned false".into());
                }
            }
            let ex = dl::dlsym(lib, c"bundleExit".as_ptr());
            let exit = (!ex.is_null()).then(|| std::mem::transmute::<*mut c_void, unsafe extern "system" fn() -> bool>(ex));
            let gpf = dl::dlsym(lib, c"GetPluginFactory".as_ptr());
            if gpf.is_null() {
                return Err("no GetPluginFactory export".into());
            }
            let gpf: unsafe extern "system" fn() -> *mut IPluginFactory = std::mem::transmute(gpf);
            let factory = ComPtr::from_raw(gpf()).ok_or("GetPluginFactory returned null")?;
            Ok(Module { lib, factory: Some(factory), exit, path: bin })
        }
    }

    #[cfg(windows)]
    pub fn load(bundle: &Path) -> Result<Module, String> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        let dll = dll_of_bundle(bundle);
        if !dll.is_file() {
            return Err(format!("no plugin module at {}", dll.display()));
        }
        let wide: Vec<u16> = dll.as_os_str().encode_wide().chain(Some(0)).collect();
        let lib = unsafe { LoadLibraryW(wide.as_ptr()) };
        if lib.is_null() {
            return Err(format!("LoadLibrary failed for {}", dll.display()));
        }
        unsafe {
            if let Some(f) = GetProcAddress(lib, c"InitDll".as_ptr() as *const u8) {
                let init: unsafe extern "system" fn() -> bool = std::mem::transmute(f);
                if !init() {
                    return Err("InitDll returned false".into());
                }
            }
            let exit = GetProcAddress(lib, c"ExitDll".as_ptr() as *const u8).map(|f| std::mem::transmute::<_, unsafe extern "system" fn() -> bool>(f));
            let gpf = GetProcAddress(lib, c"GetPluginFactory".as_ptr() as *const u8).ok_or("no GetPluginFactory export")?;
            let gpf: unsafe extern "system" fn() -> *mut IPluginFactory = std::mem::transmute(gpf);
            let factory = ComPtr::from_raw(gpf()).ok_or("GetPluginFactory returned null")?;
            Ok(Module { lib, factory: Some(factory), exit, path: dll })
        }
    }

    fn factory(&self) -> &ComPtr<IPluginFactory> {
        self.factory.as_ref().unwrap()
    }

    /// (cid, categorie, nom) de chaque classe.
    pub fn classes(&self) -> Vec<(TUID, String, String)> {
        let f = self.factory();
        let n = unsafe { f.countClasses() };
        let mut v = Vec::new();
        for i in 0..n {
            let mut info: PClassInfo = unsafe { std::mem::zeroed() };
            if unsafe { f.getClassInfo(i, &mut info) } == kResultOk {
                v.push((info.cid, cstr(&info.category), cstr(&info.name)));
            }
        }
        v
    }

    pub fn create(&self, name: &str) -> Result<Plugin, String> {
        let (cid, _, _) = self.classes().into_iter().find(|(_, cat, n)| n == name && cat == "Audio Module Class").ok_or_else(|| format!("no class \"{name}\" in {}", self.path.display()))?;
        let mut obj: *mut c_void = std::ptr::null_mut();
        let r = unsafe { self.factory().createInstance(cid.as_ptr(), IComponent::IID.as_ptr() as FIDString, &mut obj) };
        if r != kResultOk || obj.is_null() {
            return Err(format!("createInstance({name}) failed ({r})"));
        }
        let comp = unsafe { ComPtr::from_raw(obj as *mut IComponent) }.ok_or("null component")?;
        let processor = comp.cast::<IAudioProcessor>().ok_or("the component is not an IAudioProcessor")?;
        let host = ComWrapper::new(HostApp);
        Ok(Plugin { comp, processor, host, active: false })
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        // la fabrique avant le module ; puis ExitDll, puis le module
        self.factory = None;
        unsafe {
            if let Some(e) = self.exit {
                e();
            }
            #[cfg(windows)]
            windows_sys::Win32::Foundation::FreeLibrary(self.lib);
            #[cfg(unix)]
            dl::dlclose(self.lib);
        }
    }
}

pub struct Plugin {
    pub comp: ComPtr<IComponent>,
    pub processor: ComPtr<IAudioProcessor>,
    host: ComWrapper<HostApp>,
    active: bool,
}

impl Plugin {
    /// Comme un DAW : initialize, les bus (stereo), setupProcessing (temps reel, 32 bits), setActive, setProcessing.
    pub fn start(&mut self, rate: f64, max_block: i32, audio_in: bool) -> Result<(), String> {
        let ctx = self.host.to_com_ptr::<FUnknown>().ok_or("host context")?;
        unsafe {
            let r = self.comp.initialize(ctx.as_ptr());
            if r != kResultOk {
                return Err(format!("initialize failed ({r})"));
            }
            let mut ins = [SpeakerArr::kStereo];
            let mut outs = [SpeakerArr::kStereo];
            let r = self.processor.setBusArrangements(ins.as_mut_ptr(), if audio_in { 1 } else { 0 }, outs.as_mut_ptr(), 1);
            if r != kResultOk {
                return Err(format!("setBusArrangements failed ({r})"));
            }
            for (ty, dir) in [(MediaTypes_::kAudio, BusDirections_::kInput), (MediaTypes_::kAudio, BusDirections_::kOutput), (MediaTypes_::kEvent, BusDirections_::kInput), (MediaTypes_::kEvent, BusDirections_::kOutput)] {
                let n = self.comp.getBusCount(ty as _, dir as _);
                for i in 0..n {
                    self.comp.activateBus(ty as _, dir as _, i, 1);
                }
            }
            let mut setup = ProcessSetup { processMode: ProcessModes_::kRealtime as _, symbolicSampleSize: SymbolicSampleSizes_::kSample32 as _, maxSamplesPerBlock: max_block, sampleRate: rate };
            let r = self.processor.setupProcessing(&mut setup);
            if r != kResultOk {
                return Err(format!("setupProcessing failed ({r})"));
            }
            let r = self.comp.setActive(1);
            if r != kResultOk {
                return Err(format!("setActive failed ({r})"));
            }
            self.processor.setProcessing(1);
        }
        self.active = true;
        Ok(())
    }

    /// (25 oct.) Les parametres que le plugin declare a l'hote (son IEditController : nice-plug en fait un seul objet
    /// avec le composant) → (id, titre, nombre de pas).
    pub fn params(&self) -> Vec<(u32, String, i32)> {
        let Some(ec) = self.comp.cast::<IEditController>() else { return Vec::new() };
        let n = unsafe { ec.getParameterCount() };
        let mut v = Vec::new();
        for i in 0..n {
            let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
            if unsafe { ec.getParameterInfo(i, &mut info) } == kResultOk {
                let t: Vec<u16> = info.title.iter().take_while(|c| **c != 0).map(|c| *c as u16).collect();
                v.push((info.id, String::from_utf16_lossy(&t), info.stepCount));
            }
        }
        v
    }

    /// (25 oct.) Un parametre, comme un DAW (ou le Remote Script de Live) le regle : setParamNormalized.
    pub fn set_param(&self, id: u32, normalized: f64) -> Result<(), String> {
        let ec = self.comp.cast::<IEditController>().ok_or("the plugin has no IEditController")?;
        let r = unsafe { ec.setParamNormalized(id, normalized) };
        if r != kResultOk {
            return Err(format!("setParamNormalized failed ({r})"));
        }
        Ok(())
    }

    pub fn event_outputs(&self) -> i32 {
        unsafe { self.comp.getBusCount(MediaTypes_::kEvent as _, BusDirections_::kOutput as _) }
    }

    pub fn audio_buses(&self) -> (i32, i32) {
        unsafe { (self.comp.getBusCount(MediaTypes_::kAudio as _, BusDirections_::kInput as _), self.comp.getBusCount(MediaTypes_::kAudio as _, BusDirections_::kOutput as _)) }
    }

    pub fn stop(&mut self) {
        if !self.active {
            return;
        }
        unsafe {
            self.processor.setProcessing(0);
            self.comp.setActive(0);
            self.comp.terminate();
        }
        self.active = false;
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Les tampons d'un bloc : stereo en entree (Return) et en sortie, des listes d'evenements, un contexte de lecture.
pub struct Block {
    pub inl: Vec<f32>,
    pub inr: Vec<f32>,
    pub outl: Vec<f32>,
    pub outr: Vec<f32>,
    in_ptrs: [*mut f32; 2],
    out_ptrs: [*mut f32; 2],
    in_bus: AudioBusBuffers,
    out_bus: AudioBusBuffers,
    pub ctx: ProcessContext,
    pub out_events: ComWrapper<EventList>,
    in_events: ComWrapper<EventList>,
    /// (25 oct.) les parametres a passer au prochain process() (inputParameterChanges)
    params: Vec<(u32, f64)>,
}

impl Block {
    pub fn new(max: usize, rate: f64) -> Block {
        let mut ctx: ProcessContext = unsafe { std::mem::zeroed() };
        ctx.sampleRate = rate;
        ctx.tempo = 120.0;
        ctx.timeSigNumerator = 4;
        ctx.timeSigDenominator = 4;
        Block {
            inl: vec![0.0; max],
            inr: vec![0.0; max],
            outl: vec![0.0; max],
            outr: vec![0.0; max],
            in_ptrs: [std::ptr::null_mut(); 2],
            out_ptrs: [std::ptr::null_mut(); 2],
            in_bus: unsafe { std::mem::zeroed() },
            out_bus: unsafe { std::mem::zeroed() },
            ctx,
            out_events: ComWrapper::new(EventList { events: Mutex::new(Vec::with_capacity(4096)) }),
            in_events: ComWrapper::new(EventList { events: Mutex::new(Vec::new()) }),
            params: Vec::new(),
        }
    }

    /// (25 oct.) Un parametre a changer au debut du prochain bloc, comme un DAW (ou le Remote Script de Live).
    pub fn queue_param(&mut self, id: u32, normalized: f64) {
        self.params.push((id, normalized));
    }

    /// Un appel de process() sur `n` echantillons ; les evenements de sortie restent dans `out_events`.
    pub fn process(&mut self, p: &Plugin, n: usize, audio_in: bool, sample_pos: i64, system_time_ns: i64) -> tresult {
        self.out_events.events.lock().unwrap().clear();
        self.in_ptrs = [self.inl.as_mut_ptr(), self.inr.as_mut_ptr()];
        self.out_ptrs = [self.outl.as_mut_ptr(), self.outr.as_mut_ptr()];
        self.in_bus.numChannels = 2;
        self.in_bus.silenceFlags = 0;
        self.in_bus.__field0.channelBuffers32 = self.in_ptrs.as_mut_ptr();
        self.out_bus.numChannels = 2;
        self.out_bus.silenceFlags = 0;
        self.out_bus.__field0.channelBuffers32 = self.out_ptrs.as_mut_ptr();
        self.ctx.state = (ProcessContext_::StatesAndFlags_::kPlaying | ProcessContext_::StatesAndFlags_::kSystemTimeValid | ProcessContext_::StatesAndFlags_::kContTimeValid | ProcessContext_::StatesAndFlags_::kTempoValid) as u32;
        self.ctx.projectTimeSamples = sample_pos;
        self.ctx.continousTimeSamples = sample_pos;
        self.ctx.systemTime = system_time_ns;
        let out_ev = self.out_events.to_com_ptr::<IEventList>().unwrap();
        let in_ev = self.in_events.to_com_ptr::<IEventList>().unwrap();
        let changes = if self.params.is_empty() {
            None
        } else {
            let queues = self.params.drain(..).map(|(id, value)| ComWrapper::new(ParamQueue { id, value })).collect();
            Some(ComWrapper::new(ParamChanges { queues }))
        };
        let in_params = changes.as_ref().and_then(|c| c.to_com_ptr::<IParameterChanges>());
        let mut data = ProcessData {
            processMode: ProcessModes_::kRealtime as _,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as _,
            numSamples: n as i32,
            numInputs: if audio_in { 1 } else { 0 },
            numOutputs: 1,
            inputs: if audio_in { &mut self.in_bus } else { std::ptr::null_mut() },
            outputs: &mut self.out_bus,
            inputParameterChanges: in_params.as_ref().map(|p| p.as_ptr()).unwrap_or(std::ptr::null_mut()),
            outputParameterChanges: std::ptr::null_mut(),
            inputEvents: in_ev.as_ptr(),
            outputEvents: out_ev.as_ptr(),
            processContext: &mut self.ctx,
        };
        unsafe { p.processor.process(&mut data) }
    }
}
