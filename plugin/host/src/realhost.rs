// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (19 nov.) L'ESSAI « le compagnon comme hote VST3 » (a-lire/hote-vst3-compagnon-19nov.md, etape 0) : ce que
//! vsthost.rs ne fait pas et qu'un vrai plugin (Diva, Serum 2, Kontakt) demande. Un outil de developpement : rien ici
//! n'est dans le compagnon.
//!
//! - le controleur A PART (getControllerClassId), relie au composant par IConnectionPoint, et l'etat du composant
//!   passe au controleur (setComponentState) ;
//! - IHostApplication::createInstance : IMessage et IAttributeList (les plugins se parlent entre composant et
//!   controleur avec) ; IPlugInterfaceSupport ;
//! - IComponentHandler : un bouton tourne dans l'editeur revient au fil audio (inputParameterChanges) ;
//! - IBStream en memoire : getState / setState du composant et du controleur ;
//! - les bus tels que le plugin les declare (des tampons pour chacun, la sortie principale en stereo) ;
//! - l'editeur : IPlugView dans une fenetre a nous (Windows ; macOS depuis le 20 nov., macui.rs), IPlugFrame::resizeView,
//!   la mise a l'echelle.

use crate::vsthost::{param_changes, EventList, Module};
use std::collections::HashMap;
use std::ffi::{c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::Steinberg::Vst::IAttributeList_::AttrID;
use vst3::{Class, ComPtr, ComRef, ComWrapper, Interface};

fn cstr(buf: &[std::ffi::c_char]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn same_iid(p: *const TUID, iid: &[u8; 16]) -> bool {
    if p.is_null() {
        return false;
    }
    let t = unsafe { &*p };
    t.iter().zip(iid.iter()).all(|(a, b)| *a as u8 == *b)
}

fn u16z(p: *const u16) -> Vec<u16> {
    let mut v = Vec::new();
    if p.is_null() {
        return v;
    }
    let mut i = 0;
    loop {
        let c = unsafe { *p.add(i) };
        if c == 0 || i > 1 << 20 {
            return v;
        }
        v.push(c);
        i += 1;
    }
}

// ── IAttributeList, IMessage ────────────────────────────────────────────────────────────────────────────────────

enum Attr {
    Int(i64),
    Float(f64),
    Str(Vec<u16>),
    Bin(Vec<u8>),
}

#[derive(Default)]
pub struct AttrList {
    m: Mutex<HashMap<Vec<u8>, Attr>>,
}

fn attr_key(id: AttrID) -> Option<Vec<u8>> {
    if id.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(id) }.to_bytes().to_vec())
    }
}

impl Class for AttrList {
    type Interfaces = (IAttributeList,);
}

impl IAttributeListTrait for AttrList {
    unsafe fn setInt(&self, id: AttrID, value: int64) -> tresult {
        let Some(k) = attr_key(id) else { return kInvalidArgument };
        self.m.lock().unwrap().insert(k, Attr::Int(value));
        kResultOk
    }
    unsafe fn getInt(&self, id: AttrID, value: *mut int64) -> tresult {
        let Some(k) = attr_key(id) else { return kInvalidArgument };
        match self.m.lock().unwrap().get(&k) {
            Some(Attr::Int(v)) if !value.is_null() => {
                *value = *v;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn setFloat(&self, id: AttrID, value: f64) -> tresult {
        let Some(k) = attr_key(id) else { return kInvalidArgument };
        self.m.lock().unwrap().insert(k, Attr::Float(value));
        kResultOk
    }
    unsafe fn getFloat(&self, id: AttrID, value: *mut f64) -> tresult {
        let Some(k) = attr_key(id) else { return kInvalidArgument };
        match self.m.lock().unwrap().get(&k) {
            Some(Attr::Float(v)) if !value.is_null() => {
                *value = *v;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn setString(&self, id: AttrID, string: *const TChar) -> tresult {
        let Some(k) = attr_key(id) else { return kInvalidArgument };
        self.m.lock().unwrap().insert(k, Attr::Str(u16z(string as *const u16)));
        kResultOk
    }
    unsafe fn getString(&self, id: AttrID, string: *mut TChar, size_in_bytes: uint32) -> tresult {
        let Some(k) = attr_key(id) else { return kInvalidArgument };
        match self.m.lock().unwrap().get(&k) {
            Some(Attr::Str(v)) if !string.is_null() && size_in_bytes >= 2 => {
                let cap = (size_in_bytes as usize / 2) - 1;
                let n = v.len().min(cap);
                let dst = string as *mut u16;
                std::ptr::copy_nonoverlapping(v.as_ptr(), dst, n);
                *dst.add(n) = 0;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn setBinary(&self, id: AttrID, data: *const c_void, size_in_bytes: uint32) -> tresult {
        let Some(k) = attr_key(id) else { return kInvalidArgument };
        let v = if data.is_null() || size_in_bytes == 0 { Vec::new() } else { std::slice::from_raw_parts(data as *const u8, size_in_bytes as usize).to_vec() };
        self.m.lock().unwrap().insert(k, Attr::Bin(v));
        kResultOk
    }
    unsafe fn getBinary(&self, id: AttrID, data: *mut *const c_void, size_in_bytes: *mut uint32) -> tresult {
        let Some(k) = attr_key(id) else { return kInvalidArgument };
        match self.m.lock().unwrap().get(&k) {
            // le tas du Vec ne bouge pas tant que la valeur reste : le pointeur vit avec la liste (comme dans le SDK)
            Some(Attr::Bin(v)) if !data.is_null() && !size_in_bytes.is_null() => {
                *data = v.as_ptr() as *const c_void;
                *size_in_bytes = v.len() as u32;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
}

pub struct Message {
    id: Mutex<Option<CString>>,
    attrs: ComWrapper<AttrList>,
}

impl Class for Message {
    type Interfaces = (IMessage,);
}

impl IMessageTrait for Message {
    unsafe fn getMessageID(&self) -> FIDString {
        self.id.lock().unwrap().as_ref().map(|c| c.as_ptr()).unwrap_or(std::ptr::null())
    }
    unsafe fn setMessageID(&self, id: FIDString) {
        *self.id.lock().unwrap() = if id.is_null() { None } else { Some(CStr::from_ptr(id).to_owned()) };
    }
    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        // comme le SDK (HostMessage) : la liste appartient au message, pas de reference en plus
        self.attrs.as_com_ref::<IAttributeList>().map(|r| r.as_ptr()).unwrap_or(std::ptr::null_mut())
    }
}

// ── L'hote ───────────────────────────────────────────────────────────────────────────────────────────────────────

pub struct Host {
    pub messages: AtomicU32,
}

impl Class for Host {
    type Interfaces = (IHostApplication, IPlugInterfaceSupport);
}

impl IHostApplicationTrait for Host {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        if name.is_null() {
            return kInvalidArgument;
        }
        let s: Vec<u16> = "Uppwind Studio".encode_utf16().collect();
        let dst = &mut *name;
        dst[..s.len()].copy_from_slice(&s);
        dst[s.len()] = 0;
        kResultOk
    }

    unsafe fn createInstance(&self, cid: *mut TUID, iid: *mut TUID, obj: *mut *mut c_void) -> tresult {
        if obj.is_null() {
            return kInvalidArgument;
        }
        *obj = std::ptr::null_mut();
        if same_iid(cid, &IMessage::IID) || same_iid(iid, &IMessage::IID) {
            self.messages.fetch_add(1, Ordering::Relaxed);
            let m = ComWrapper::new(Message { id: Mutex::new(None), attrs: ComWrapper::new(AttrList::default()) });
            if let Some(p) = m.to_com_ptr::<IMessage>() {
                *obj = p.into_raw() as *mut c_void;
                return kResultOk;
            }
        } else if same_iid(cid, &IAttributeList::IID) || same_iid(iid, &IAttributeList::IID) {
            let l = ComWrapper::new(AttrList::default());
            if let Some(p) = l.to_com_ptr::<IAttributeList>() {
                *obj = p.into_raw() as *mut c_void;
                return kResultOk;
            }
        }
        kResultFalse
    }
}

impl IPlugInterfaceSupportTrait for Host {
    unsafe fn isPlugInterfaceSupported(&self, iid: *const TUID) -> tresult {
        let ours: [&[u8; 16]; 8] = [&IComponent::IID, &IAudioProcessor::IID, &IEditController::IID, &IConnectionPoint::IID, &IUnitInfo::IID, &IMidiMapping::IID, &IEditController2::IID, &IPlugViewContentScaleSupport::IID];
        if ours.iter().any(|i| same_iid(iid, i)) {
            kResultTrue
        } else {
            kResultFalse
        }
    }
}

// ── IBStream en memoire ─────────────────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct MemStream {
    d: Mutex<(Vec<u8>, usize)>,
}

impl MemStream {
    pub fn with(bytes: &[u8]) -> MemStream {
        MemStream { d: Mutex::new((bytes.to_vec(), 0)) }
    }
    pub fn bytes(&self) -> Vec<u8> {
        self.d.lock().unwrap().0.clone()
    }
    pub fn rewind(&self) {
        self.d.lock().unwrap().1 = 0;
    }
}

impl Class for MemStream {
    type Interfaces = (IBStream,);
}

impl IBStreamTrait for MemStream {
    unsafe fn read(&self, buffer: *mut c_void, num_bytes: int32, num_bytes_read: *mut int32) -> tresult {
        let mut g = self.d.lock().unwrap();
        let (ref data, ref mut pos) = *g;
        let n = if num_bytes <= 0 || buffer.is_null() { 0 } else { (num_bytes as usize).min(data.len().saturating_sub(*pos)) };
        if n > 0 {
            std::ptr::copy_nonoverlapping(data.as_ptr().add(*pos), buffer as *mut u8, n);
        }
        *pos += n;
        if !num_bytes_read.is_null() {
            *num_bytes_read = n as i32;
        }
        kResultOk
    }
    unsafe fn write(&self, buffer: *mut c_void, num_bytes: int32, num_bytes_written: *mut int32) -> tresult {
        if num_bytes < 0 || (buffer.is_null() && num_bytes > 0) {
            return kInvalidArgument;
        }
        let mut g = self.d.lock().unwrap();
        let (ref mut data, ref mut pos) = *g;
        let n = num_bytes as usize;
        if *pos + n > data.len() {
            data.resize(*pos + n, 0);
        }
        if n > 0 {
            std::ptr::copy_nonoverlapping(buffer as *const u8, data.as_mut_ptr().add(*pos), n);
        }
        *pos += n;
        if !num_bytes_written.is_null() {
            *num_bytes_written = n as i32;
        }
        kResultOk
    }
    unsafe fn seek(&self, pos: int64, mode: int32, result: *mut int64) -> tresult {
        let mut g = self.d.lock().unwrap();
        let len = g.0.len() as i64;
        let base = match mode {
            0 => 0,
            1 => g.1 as i64,
            2 => len,
            _ => return kInvalidArgument,
        };
        let p = (base + pos).max(0);
        g.1 = p as usize;
        if !result.is_null() {
            *result = p;
        }
        kResultOk
    }
    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        if pos.is_null() {
            return kInvalidArgument;
        }
        *pos = self.d.lock().unwrap().1 as i64;
        kResultOk
    }
}

// ── IComponentHandler ───────────────────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct Handler {
    /// les changements a passer au prochain bloc (le fil audio les prend)
    pub pending: Mutex<Vec<(u32, f64)>>,
    pub edits: AtomicU32,
    pub gestures: AtomicU32,
    pub restarts: Mutex<Vec<i32>>,
    /// (20 nov., Link to controller) le studio suit les parametres : chaque performEdit de l'editeur est aussi note ici
    /// (le fil du suivi, child.rs, les envoie au compagnon) ; partages par Arc avec ce fil
    pub watch: Arc<AtomicBool>,
    pub touched: Arc<Mutex<Vec<(u32, f64)>>>,
}

impl Class for Handler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for Handler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        self.gestures.fetch_add(1, Ordering::Relaxed);
        kResultOk
    }
    unsafe fn performEdit(&self, id: ParamID, value: ParamValue) -> tresult {
        self.edits.fetch_add(1, Ordering::Relaxed);
        let mut p = self.pending.lock().unwrap();
        if p.len() < 4096 {
            p.push((id, value));
        }
        drop(p);
        if self.watch.load(Ordering::Relaxed) {
            let mut t = self.touched.lock().unwrap();
            if t.len() < 1024 {
                t.push((id, value));
            }
        }
        kResultOk
    }
    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn restartComponent(&self, flags: int32) -> tresult {
        self.restarts.lock().unwrap().push(flags);
        kResultOk
    }
}

// ── Les classes d'un module ─────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ClassInfo {
    pub cid: TUID,
    pub category: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub sub: String,
}

pub fn classes(m: &Module) -> Vec<ClassInfo> {
    let f = m.factory();
    let n = unsafe { f.countClasses() };
    let f2 = f.cast::<IPluginFactory2>();
    let mut fi: PFactoryInfo = unsafe { std::mem::zeroed() };
    let vendor0 = if unsafe { f.getFactoryInfo(&mut fi) } == kResultOk { cstr(&fi.vendor) } else { String::new() };
    let mut v = Vec::new();
    for i in 0..n {
        if let Some(f2) = &f2 {
            let mut c: PClassInfo2 = unsafe { std::mem::zeroed() };
            if unsafe { f2.getClassInfo2(i, &mut c) } == kResultOk {
                let vendor = cstr(&c.vendor);
                v.push(ClassInfo { cid: c.cid, category: cstr(&c.category), name: cstr(&c.name), vendor: if vendor.is_empty() { vendor0.clone() } else { vendor }, version: cstr(&c.version), sub: cstr(&c.subCategories) });
                continue;
            }
        }
        let mut c: PClassInfo = unsafe { std::mem::zeroed() };
        if unsafe { f.getClassInfo(i, &mut c) } == kResultOk {
            v.push(ClassInfo { cid: c.cid, category: cstr(&c.category), name: cstr(&c.name), vendor: vendor0.clone(), version: String::new(), sub: String::new() });
        }
    }
    v
}

// ── Une instance ────────────────────────────────────────────────────────────────────────────────────────────────

pub struct Instance {
    pub comp: ComPtr<IComponent>,
    pub processor: ComPtr<IAudioProcessor>,
    pub ctrl: Option<ComPtr<IEditController>>,
    /// le controleur est un objet a part (le cas de presque tous les plugins du commerce)
    pub separate: bool,
    cps: Option<(ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>)>,
    pub host: ComWrapper<Host>,
    pub handler: ComWrapper<Handler>,
    /// les canaux de chaque bus audio (entrees, sorties) et le nombre de bus d'evenements en entree
    pub ins: Vec<i32>,
    pub outs: Vec<i32>,
    pub event_ins: i32,
    pub latency: u32,
    active: bool,
    initialized: bool,
}

fn channels(arr: SpeakerArrangement) -> i32 {
    arr.count_ones() as i32
}

impl Instance {
    pub fn create(m: &Module, cid: &TUID) -> Result<Instance, String> {
        let f = m.factory();
        let mut obj: *mut c_void = std::ptr::null_mut();
        let r = unsafe { f.createInstance(cid.as_ptr(), IComponent::IID.as_ptr() as FIDString, &mut obj) };
        if r != kResultOk || obj.is_null() {
            return Err(format!("createInstance(IComponent) failed ({r})"));
        }
        let comp = unsafe { ComPtr::from_raw(obj as *mut IComponent) }.ok_or("null component")?;
        let host = ComWrapper::new(Host { messages: AtomicU32::new(0) });
        let ctx = host.to_com_ptr::<FUnknown>().ok_or("host context")?;
        let r = unsafe { comp.initialize(ctx.as_ptr()) };
        if r != kResultOk {
            return Err(format!("IComponent::initialize failed ({r})"));
        }
        let processor = comp.cast::<IAudioProcessor>().ok_or("the component is not an IAudioProcessor")?;
        let mut separate = false;
        let ctrl = match comp.cast::<IEditController>() {
            Some(c) => Some(c),
            None => {
                let mut tuid: TUID = unsafe { std::mem::zeroed() };
                if unsafe { comp.getControllerClassId(&mut tuid) } == kResultOk && tuid.iter().any(|b| *b != 0) {
                    let mut o: *mut c_void = std::ptr::null_mut();
                    let r = unsafe { f.createInstance(tuid.as_ptr(), IEditController::IID.as_ptr() as FIDString, &mut o) };
                    if r == kResultOk && !o.is_null() {
                        let c = unsafe { ComPtr::from_raw(o as *mut IEditController) }.ok_or("null controller")?;
                        let r = unsafe { c.initialize(ctx.as_ptr()) };
                        if r != kResultOk {
                            return Err(format!("IEditController::initialize failed ({r})"));
                        }
                        separate = true;
                        Some(c)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
        };
        // composant <-> controleur (seulement s'ils sont deux objets)
        let mut cps = None;
        if let (true, Some(c)) = (separate, &ctrl) {
            if let (Some(a), Some(b)) = (comp.cast::<IConnectionPoint>(), c.cast::<IConnectionPoint>()) {
                unsafe {
                    a.connect(b.as_ptr());
                    b.connect(a.as_ptr());
                }
                cps = Some((a, b));
            }
        }
        let handler = ComWrapper::new(Handler::default());
        let inst = Instance { comp, processor, ctrl, separate, cps, host, handler, ins: Vec::new(), outs: Vec::new(), event_ins: 0, latency: 0, active: false, initialized: true };
        // l'etat du composant au controleur (comme un DAW a l'ouverture)
        if let (true, Some(c)) = (inst.separate, &inst.ctrl) {
            let s = ComWrapper::new(MemStream::default());
            let sp = s.to_com_ptr::<IBStream>().unwrap();
            if unsafe { inst.comp.getState(sp.as_ptr()) } == kResultOk {
                s.rewind();
                unsafe { c.setComponentState(sp.as_ptr()) };
            }
        }
        if let Some(c) = &inst.ctrl {
            let h = inst.handler.to_com_ptr::<IComponentHandler>().unwrap();
            unsafe { c.setComponentHandler(h.as_ptr()) };
        }
        Ok(inst)
    }

    pub fn param_count(&self) -> i32 {
        self.ctrl.as_ref().map(|c| unsafe { c.getParameterCount() }).unwrap_or(0)
    }

    /// (20 nov., Link to controller) les parametres que le studio peut lier, a partir de l'indice `start`, `max` au plus :
    /// (indice, id, titre, valeur normalisee) — ni caches, ni en lecture seule, ni le changement de programme ; l'indice
    /// est celui du controleur (0..4095, le « p » du protocole). → (la page, l'indice suivant s'il en reste)
    pub fn param_page(&self, start: usize, max: usize) -> (Vec<(u32, u32, String, f64)>, Option<usize>) {
        let Some(c) = &self.ctrl else { return (Vec::new(), None) };
        let n = (unsafe { c.getParameterCount() }).clamp(0, 4096) as usize;
        let mut v = Vec::new();
        let mut i = start;
        while i < n {
            if v.len() >= max {
                return (v, Some(i));
            }
            let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
            if unsafe { c.getParameterInfo(i as i32, &mut info) } == kResultOk {
                let hidden = info.flags & (ParameterInfo_::ParameterFlags_::kIsHidden | ParameterInfo_::ParameterFlags_::kIsReadOnly | ParameterInfo_::ParameterFlags_::kIsProgramChange) != 0;
                if !hidden {
                    let t: Vec<u16> = info.title.iter().take_while(|c| **c != 0).map(|c| *c as u16).collect();
                    let name: String = String::from_utf16_lossy(&t).chars().filter(|c| !c.is_control()).take(64).collect();
                    let val = unsafe { c.getParamNormalized(info.id) };
                    v.push((i as u32, info.id, if name.is_empty() { format!("Parameter {}", i + 1) } else { name }, if val.is_finite() { val.clamp(0.0, 1.0) } else { 0.0 }));
                }
            }
            i += 1;
        }
        (v, None)
    }

    /// L'id du parametre d'indice `p` (None : hors de la liste, cache, en lecture seule).
    pub fn param_id(&self, p: u32) -> Option<u32> {
        let c = self.ctrl.as_ref()?;
        if p as i32 >= unsafe { c.getParameterCount() } {
            return None;
        }
        let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
        (unsafe { c.getParameterInfo(p as i32, &mut info) } == kResultOk
            && info.flags & (ParameterInfo_::ParameterFlags_::kIsHidden | ParameterInfo_::ParameterFlags_::kIsReadOnly | ParameterInfo_::ParameterFlags_::kIsProgramChange) == 0)
            .then_some(info.id)
    }

    /// Poser un parametre comme un DAW : au controleur (l'editeur suit), et au processeur au prochain bloc (le son suit).
    pub fn set_param(&self, id: u32, v: f64) {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { return };
        if let Some(c) = &self.ctrl {
            unsafe { c.setParamNormalized(id, v) };
        }
        let mut p = self.handler.pending.lock().unwrap();
        if p.len() < 4096 {
            p.push((id, v));
        }
    }

    /// La valeur (normalisee) de chaque parametre, lue au controleur : pour verifier qu'un etat recharge remet le son.
    pub fn param_values(&self) -> Vec<(u32, f64)> {
        let Some(c) = &self.ctrl else { return Vec::new() };
        let n = unsafe { c.getParameterCount() };
        (0..n)
            .filter_map(|i| {
                let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
                (unsafe { c.getParameterInfo(i, &mut info) } == kResultOk).then(|| (info.id, unsafe { c.getParamNormalized(info.id) }))
            })
            .collect()
    }

    /// Les bus (la sortie principale en stereo), setupProcessing, setActive, setProcessing.
    pub fn start(&mut self, rate: f64, max_block: i32) -> Result<(), String> {
        self.start_mode(rate, max_block, false)
    }

    /// (19 nov., etape 3) `offline` : le rendu hors temps reel (kOffline, ProcessSetup et ProcessData), pour appliquer
    /// un effet a une piste deja rendue ; sinon le temps reel.
    pub fn start_mode(&mut self, rate: f64, max_block: i32, offline: bool) -> Result<(), String> {
        let mode = if offline { ProcessModes_::kOffline } else { ProcessModes_::kRealtime };
        unsafe {
            let n_in = self.comp.getBusCount(MediaTypes_::kAudio as _, BusDirections_::kInput as _);
            let n_out = self.comp.getBusCount(MediaTypes_::kAudio as _, BusDirections_::kOutput as _);
            self.event_ins = self.comp.getBusCount(MediaTypes_::kEvent as _, BusDirections_::kInput as _);
            let read = |dir: BusDirection, n: i32| -> Vec<SpeakerArrangement> {
                (0..n)
                    .map(|i| {
                        let mut a: SpeakerArrangement = 0;
                        self.processor.getBusArrangement(dir, i, &mut a);
                        a
                    })
                    .collect()
            };
            let mut ins = read(BusDirections_::kInput as _, n_in);
            let mut outs = read(BusDirections_::kOutput as _, n_out);
            if let Some(a) = ins.first_mut() {
                *a = SpeakerArr::kStereo;
            }
            if let Some(a) = outs.first_mut() {
                *a = SpeakerArr::kStereo;
            }
            // le plugin peut refuser : on garde alors ce qu'il dit
            self.processor.setBusArrangements(ins.as_mut_ptr(), n_in, outs.as_mut_ptr(), n_out);
            self.ins = read(BusDirections_::kInput as _, n_in).into_iter().map(channels).collect();
            self.outs = read(BusDirections_::kOutput as _, n_out).into_iter().map(channels).collect();
            if n_out > 0 {
                self.comp.activateBus(MediaTypes_::kAudio as _, BusDirections_::kOutput as _, 0, 1);
            }
            if n_in > 0 {
                self.comp.activateBus(MediaTypes_::kAudio as _, BusDirections_::kInput as _, 0, 1);
            }
            if self.event_ins > 0 {
                self.comp.activateBus(MediaTypes_::kEvent as _, BusDirections_::kInput as _, 0, 1);
            }
            let mut setup = ProcessSetup { processMode: mode as _, symbolicSampleSize: SymbolicSampleSizes_::kSample32 as _, maxSamplesPerBlock: max_block, sampleRate: rate };
            let r = self.processor.setupProcessing(&mut setup);
            if r != kResultOk {
                return Err(format!("setupProcessing failed ({r})"));
            }
            let r = self.comp.setActive(1);
            if r != kResultOk {
                return Err(format!("setActive failed ({r})"));
            }
            self.latency = self.processor.getLatencySamples();
            self.processor.setProcessing(1);
        }
        self.active = true;
        Ok(())
    }

    /// L'etat : celui du composant, et celui du controleur (s'il est a part).
    pub fn get_state(&self) -> Result<(Vec<u8>, Option<Vec<u8>>), String> {
        let s = ComWrapper::new(MemStream::default());
        let sp = s.to_com_ptr::<IBStream>().unwrap();
        let r = unsafe { self.comp.getState(sp.as_ptr()) };
        if r != kResultOk {
            return Err(format!("IComponent::getState failed ({r})"));
        }
        let comp = s.bytes();
        let mut ctrl = None;
        if let (true, Some(c)) = (self.separate, &self.ctrl) {
            let s = ComWrapper::new(MemStream::default());
            let sp = s.to_com_ptr::<IBStream>().unwrap();
            if unsafe { c.getState(sp.as_ptr()) } == kResultOk {
                ctrl = Some(s.bytes());
            }
        }
        Ok((comp, ctrl))
    }

    /// Recharger un etat, comme un DAW qui ouvre un projet : le composant, puis le controleur (l'etat du composant, puis le sien).
    pub fn set_state(&self, comp: &[u8], ctrl: Option<&[u8]>) -> Result<(), String> {
        let s = ComWrapper::new(MemStream::with(comp));
        let sp = s.to_com_ptr::<IBStream>().unwrap();
        let r = unsafe { self.comp.setState(sp.as_ptr()) };
        if r != kResultOk {
            return Err(format!("IComponent::setState failed ({r})"));
        }
        if let Some(c) = &self.ctrl {
            if self.separate {
                s.rewind();
                unsafe { c.setComponentState(sp.as_ptr()) };
                if let Some(b) = ctrl {
                    let s2 = ComWrapper::new(MemStream::with(b));
                    let sp2 = s2.to_com_ptr::<IBStream>().unwrap();
                    unsafe { c.setState(sp2.as_ptr()) };
                }
            }
        }
        Ok(())
    }

    pub fn stop(&mut self) {
        if self.active {
            unsafe {
                self.processor.setProcessing(0);
                self.comp.setActive(0);
            }
            self.active = false;
        }
    }

    pub fn close(&mut self) {
        self.stop();
        if !self.initialized {
            return;
        }
        if let Some((a, b)) = self.cps.take() {
            unsafe {
                a.disconnect(b.as_ptr());
                b.disconnect(a.as_ptr());
            }
        }
        unsafe {
            if let Some(c) = &self.ctrl {
                c.setComponentHandler(std::ptr::null_mut());
                if self.separate {
                    c.terminate();
                }
            }
            self.comp.terminate();
        }
        self.initialized = false;
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.close();
    }
}

// ── Le traitement : des tampons pour CHAQUE bus ──────────────────────────────────────────────────────────────────

pub struct Engine {
    ins: Vec<Vec<Vec<f32>>>,
    outs: Vec<Vec<Vec<f32>>>,
    in_ptrs: Vec<Vec<*mut f32>>,
    out_ptrs: Vec<Vec<*mut f32>>,
    in_bufs: Vec<AudioBusBuffers>,
    out_bufs: Vec<AudioBusBuffers>,
    pub ctx: ProcessContext,
    pub in_events: ComWrapper<EventList>,
    out_events: ComWrapper<EventList>,
    /// (19 nov., etape 3) le mode de traitement passe a process() (kRealtime par defaut, kOffline pour un rendu)
    pub mode: i32,
    /// (19 nov., la mesure en direct) la position du MORCEAU du prochain process() (projectTimeSamples, les temps, la
    /// mesure) quand elle n'est pas `pos` : le direct date ses blocs de la trame du worklet, le plugin lit celle du morceau
    pub song: Option<i64>,
    /// (19 nov., la mesure en direct) le morceau joue-t-il (kPlaying) ; vrai par defaut (le rendu, les instruments)
    pub playing: bool,
}

impl Engine {
    pub fn new(ins: &[i32], outs: &[i32], max: usize, rate: f64) -> Engine {
        let mk = |v: &[i32]| -> Vec<Vec<Vec<f32>>> { v.iter().map(|&c| (0..c.max(0)).map(|_| vec![0.0f32; max]).collect()).collect() };
        let mut ctx: ProcessContext = unsafe { std::mem::zeroed() };
        ctx.sampleRate = rate;
        ctx.tempo = 120.0;
        ctx.timeSigNumerator = 4;
        ctx.timeSigDenominator = 4;
        Engine {
            mode: ProcessModes_::kRealtime as i32,
            ins: mk(ins),
            outs: mk(outs),
            in_ptrs: Vec::new(),
            out_ptrs: Vec::new(),
            in_bufs: Vec::new(),
            out_bufs: Vec::new(),
            ctx,
            song: None,
            playing: true,
            in_events: ComWrapper::new(EventList { events: Mutex::new(Vec::with_capacity(256)) }),
            out_events: ComWrapper::new(EventList { events: Mutex::new(Vec::with_capacity(256)) }),
        }
    }

    /// La sortie principale (gauche, droite ; un bus mono : deux fois le meme canal).
    pub fn main_out(&self) -> (&[f32], &[f32]) {
        match self.outs.first() {
            Some(b) if b.len() >= 2 => (&b[0], &b[1]),
            Some(b) if b.len() == 1 => (&b[0], &b[0]),
            _ => (&[], &[]),
        }
    }

    #[allow(dead_code)]
    pub fn main_in(&mut self) -> Option<&mut Vec<Vec<f32>>> {
        self.ins.first_mut()
    }

    pub fn process(&mut self, p: *mut IAudioProcessor, n: usize, pos: i64, system_ns: i64, params: &[(u32, f64)]) -> tresult {
        self.out_events.events.lock().unwrap().clear();
        self.in_ptrs = self.ins.iter_mut().map(|b| b.iter_mut().map(|c| c.as_mut_ptr()).collect()).collect();
        self.out_ptrs = self.outs.iter_mut().map(|b| b.iter_mut().map(|c| c.as_mut_ptr()).collect()).collect();
        let mk = |ptrs: &mut Vec<Vec<*mut f32>>| -> Vec<AudioBusBuffers> {
            ptrs.iter_mut()
                .map(|b| {
                    let mut a: AudioBusBuffers = unsafe { std::mem::zeroed() };
                    a.numChannels = b.len() as i32;
                    a.__field0.channelBuffers32 = if b.is_empty() { std::ptr::null_mut() } else { b.as_mut_ptr() };
                    a
                })
                .collect()
        };
        self.in_bufs = mk(&mut self.in_ptrs);
        self.out_bufs = mk(&mut self.out_ptrs);
        let song = self.song.unwrap_or(pos);
        let secs = song as f64 / self.ctx.sampleRate;
        let beats = secs * self.ctx.tempo / 60.0;
        let playing = if self.playing { ProcessContext_::StatesAndFlags_::kPlaying } else { 0 };
        self.ctx.state = (playing
            | ProcessContext_::StatesAndFlags_::kSystemTimeValid
            | ProcessContext_::StatesAndFlags_::kContTimeValid
            | ProcessContext_::StatesAndFlags_::kTempoValid
            | ProcessContext_::StatesAndFlags_::kTimeSigValid
            | ProcessContext_::StatesAndFlags_::kProjectTimeMusicValid
            | ProcessContext_::StatesAndFlags_::kBarPositionValid) as u32;
        self.ctx.projectTimeSamples = song;
        self.ctx.continousTimeSamples = pos;
        self.ctx.systemTime = system_ns;
        self.ctx.projectTimeMusic = beats;
        self.ctx.barPositionMusic = (beats / 4.0).floor() * 4.0;
        let in_ev = self.in_events.to_com_ptr::<IEventList>().unwrap();
        let out_ev = self.out_events.to_com_ptr::<IEventList>().unwrap();
        let changes = if params.is_empty() { None } else { Some(param_changes(params)) };
        let in_params = changes.as_ref().and_then(|c| c.to_com_ptr::<IParameterChanges>());
        let mut data = ProcessData {
            processMode: self.mode as _,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as _,
            numSamples: n as i32,
            numInputs: self.in_bufs.len() as i32,
            numOutputs: self.out_bufs.len() as i32,
            inputs: if self.in_bufs.is_empty() { std::ptr::null_mut() } else { self.in_bufs.as_mut_ptr() },
            outputs: if self.out_bufs.is_empty() { std::ptr::null_mut() } else { self.out_bufs.as_mut_ptr() },
            inputParameterChanges: in_params.as_ref().map(|p| p.as_ptr()).unwrap_or(std::ptr::null_mut()),
            outputParameterChanges: std::ptr::null_mut(),
            inputEvents: in_ev.as_ptr(),
            outputEvents: out_ev.as_ptr(),
            processContext: &mut self.ctx,
        };
        let Some(pr) = (unsafe { ComRef::from_raw(p) }) else { return kInvalidArgument };
        let r = unsafe { pr.process(&mut data) };
        self.in_events.events.lock().unwrap().clear();
        r
    }
}

/// Une note pour la liste d'entree du prochain bloc.
pub fn note_event(on: bool, offset: i32, pitch: i16, velocity: f32, id: i32) -> Event {
    let mut e: Event = unsafe { std::mem::zeroed() };
    e.busIndex = 0;
    e.sampleOffset = offset;
    if on {
        e.r#type = Event_::EventTypes_::kNoteOnEvent as u16;
        e.__field0.noteOn = NoteOnEvent { channel: 0, pitch, tuning: 0.0, velocity, length: 0, noteId: id };
    } else {
        e.r#type = Event_::EventTypes_::kNoteOffEvent as u16;
        e.__field0.noteOff = NoteOffEvent { channel: 0, pitch, velocity: 0.0, noteId: id, tuning: 0.0 };
    }
    e
}

// ── L'editeur ───────────────────────────────────────────────────────────────────────────────────────────────────

pub struct Frame {
    pub hwnd: AtomicIsize,
    pub resizes: AtomicU32,
}

impl Class for Frame {
    type Interfaces = (IPlugFrame,);
}

#[cfg(windows)]
pub fn size_window(hwnd: isize, w: i32, h: i32) {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let mut r = RECT { left: 0, top: 0, right: w, bottom: h };
        AdjustWindowRectEx(&mut r, style, 0, ex);
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, r.right - r.left, r.bottom - r.top, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

impl IPlugFrameTrait for Frame {
    unsafe fn resizeView(&self, view: *mut IPlugView, new_size: *mut ViewRect) -> tresult {
        if view.is_null() || new_size.is_null() {
            return kInvalidArgument;
        }
        self.resizes.fetch_add(1, Ordering::Relaxed);
        let r = *new_size;
        #[cfg(windows)]
        size_window(self.hwnd.load(Ordering::Relaxed), r.right - r.left, r.bottom - r.top);
        #[cfg(target_os = "macos")]
        crate::macui::size_window(self.hwnd.load(Ordering::Relaxed), r.right - r.left, r.bottom - r.top);
        if let Some(v) = ComRef::from_raw(view) {
            let mut r2 = r;
            v.onSize(&mut r2);
        }
        kResultOk
    }
}
