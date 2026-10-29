// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! « Uppwind Link Notes » : un instrument sans son, avec une sortie MIDI. Il recoit du compagnon les notes, CC et pitch
//! bend que le studio joue sur une piste, et les sort en MIDI dans le DAW : dans Live, la piste de l'instrument vise
//! les prend par « MIDI From : <piste Notes> / Uppwind Link Notes » ; dans FL, par le port de sortie MIDI du wrapper.
//! Une instance par piste visee. Elle s'annonce au compagnon avec son identite (gardee dans le projet) et un nom.
//!
//! Le placement : chaque message porte l'instant ou le DAW doit le jouer ; la DLL (clock.rs) dit ou commence le bloc
//! et la duree d'un echantillon ; le message sort a l'echantillon qui correspond (sched.rs). Arrive trop tard : au
//! debut du bloc, et le retard est compte. Une sortie stereo muette : Live refuse les plugins sans sortie audio.

use crate::clock::{self, BlockClock};
use crate::link::{self, Io, NetThread, Report, Shared, Wire};
use crate::ring::{self, Consumer, Producer};
use crate::sched::{Out, Scheduler};
use nice_plug::midi::{Channel, Key, VoiceID};
use nice_plug::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use uwl_proto as p;

pub const EVENT_RING: usize = 4096;
pub const REPORT_RING: usize = 8192;

#[derive(Params)]
pub struct NotesParams {
    /// L'identite de l'instance, gardee dans le projet du DAW : le studio relie une piste a cette identite.
    #[persist = "instance"]
    pub instance: Arc<AtomicU64>,
    /// (25 oct.) « Link tag » : le numero que le Remote Script de Live donne a ce Notes (0 : pas encore), garde dans le
    /// set comme tout parametre. Le plugin l'annonce au compagnon (trame TAG) : le studio vise « tag:<n> ». Le plugin ne
    /// sait pas sur quelle piste il est ; le script, lui, le sait et lit ce parametre.
    #[id = "tag"]
    pub tag: IntParam,
}

pub const TAG_PARAM: &str = "Link tag";

impl Default for NotesParams {
    fn default() -> Self {
        NotesParams {
            instance: Arc::new(AtomicU64::new(link::new_instance_id())),
            tag: IntParam::new(TAG_PARAM, 0, IntRange::Linear { min: 0, max: p::TAG_MAX as i32 }),
        }
    }
}

pub struct LinkNotes {
    params: Arc<NotesParams>,
    shared: Arc<Shared>,
    events: Consumer<Wire>,
    reports: Producer<Report>,
    io: Option<Io>,
    net: Option<NetThread>,
    clock: BlockClock,
    sched: Scheduler,
    realtime: bool,
    reported_overflow: u64,
}

impl Default for LinkNotes {
    fn default() -> Self {
        clock::init();
        let params = Arc::new(NotesParams::default());
        let shared = Arc::new(Shared::new(p::Kind::Notes, params.instance.clone()));
        let (ep, events) = ring::ring(EVENT_RING);
        let (reports, rc) = ring::ring(REPORT_RING);
        LinkNotes {
            params,
            shared,
            events,
            reports,
            io: Some(Io::Notes { events: ep, reports: rc }),
            net: None,
            clock: BlockClock::new(48000.0),
            sched: Scheduler::new(),
            realtime: true,
            reported_overflow: 0,
        }
    }
}

/// La velocite MIDI → le reel de VST3 (0..1), un epsilon au-dessus : un hote qui tronque (et non arrondit) en
/// revenant a 0..127 retrouve la meme valeur.
fn velocity(v: u8) -> f32 {
    (v as f32 / 127.0 + 1e-6).min(1.0)
}

/// Un message MIDI de canal → l'evenement de nice-plug (None : rien a sortir).
pub fn note_event(o: &Out) -> Option<NoteEvent<()>> {
    let d = &o.data[..(o.len as usize).min(3)];
    let st = o.data[0] & 0xf0;
    let ch = o.data[0] & 0x0f;
    match st {
        0x90 if o.data[2] > 0 => Some(NoteEvent::NoteOn { timing: o.offset, voice_id: VoiceID::Wildcard, channel: Channel::Number(ch), key: Key::Number(o.data[1]), velocity: velocity(o.data[2]) }),
        0x90 | 0x80 => Some(NoteEvent::NoteOff {
            timing: o.offset,
            voice_id: VoiceID::Wildcard,
            channel: Channel::Number(ch),
            key: Key::Number(o.data[1]),
            velocity: if st == 0x80 { velocity(o.data[2]) } else { 0.0 },
        }),
        _ => NoteEvent::from_midi(o.offset, d).ok(),
    }
}

fn emit<C: ProcessContext<LinkNotes>>(ctx: &mut C, o: &Out, shared: &Shared) {
    match note_event(o) {
        Some(ev) => {
            if ctx.try_send_event(ev).is_err() {
                shared.send_fail.fetch_add(1, Ordering::Relaxed);
            }
        }
        None => {
            shared.send_fail.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl Plugin for LinkNotes {
    const NAME: &'static str = "Uppwind Link Notes";
    const VENDOR: &'static str = crate::VENDOR;
    const URL: &'static str = crate::URL;
    const EMAIL: &'static str = crate::EMAIL;
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout { main_input_channels: None, main_output_channels: NonZeroU32::new(2), ..AudioIOLayout::const_default() }];

    // (24 oct., Live 12.4.3) Live refuse un instrument sans bus d'evenements en entree : « plugin has instrument
    // category, but no valid event input bus ». L'entree est declaree ; ce qu'elle recoit est ignore.
    const MIDI_INPUT: MidiConfig = MidiConfig::Basic;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::MidiCCs;

    type Editor = ();
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn activate(&mut self, _layout: &AudioIOLayout, cfg: &BufferConfig, _ctx: &mut impl ActivateContext<Self>) -> bool {
        self.shared.rate.store(cfg.sample_rate.round() as u32, Ordering::Relaxed);
        self.shared.max_block.store(cfg.max_buffer_size, Ordering::Relaxed);
        self.realtime = cfg.process_mode != ProcessMode::Offline;
        self.clock.set_rate(cfg.sample_rate as f64);
        if self.net.is_none() {
            if let Some(io) = self.io.take() {
                self.net = Some(NetThread::start(self.shared.clone(), io));
            }
        }
        self.shared.tag.store(self.params.tag.value().max(0) as u32, Ordering::Relaxed);
        self.shared.active.store(true, Ordering::SeqCst);
        true
    }

    fn reset(&mut self) {
        self.clock.reset();
    }

    fn deactivate(&mut self) {
        self.shared.active.store(false, Ordering::SeqCst);
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        for c in buffer.as_slice() {
            c.fill(0.0);
        }
        let n = buffer.samples();
        if n == 0 {
            return ProcessStatus::KeepAlive;
        }
        let tick = self.clock.tick(clock::now_ms(), n);
        // (25 oct.) le numero que le script a pu ecrire (un atomique : le fil reseau l'annonce s'il change)
        self.shared.tag.store(self.params.tag.value().max(0) as u32, Ordering::Relaxed);
        // (26 oct.) le tempo et la lecture du DAW (le studio suit le tempo de FL Studio sans son script)
        {
            let tr = ctx.transport();
            self.shared.note_transport(tr.tempo, tr.playing);
        }
        let LinkNotes { events, sched, reports, shared, realtime, .. } = self;
        // 1. ce que le fil reseau a recu → la reserve triee (« relache tout » : des note off tout de suite)
        while let Some(w) = events.pop() {
            if w.len == 0 {
                sched.panic(|o| emit(ctx, &o, shared));
                continue;
            }
            sched.insert(w.t, w.n, w.len, w.data);
        }
        // un export (plus vite que le temps reel) : l'heure ne veut rien dire, rien ne sort
        if *realtime {
            // 2. ce qui tombe dans ce bloc, a son echantillon
            sched.take_due(tick.t0, tick.per, n, |o| {
                emit(ctx, &o, shared);
                let _ = reports.push(Report { n: o.n, t: tick.t0 + o.offset as f64 * tick.per, offset: o.offset, frames: n as u32, late_ms: o.late_ms as f32 });
            });
        }
        let st = &self.clock.stats;
        let ss = &self.sched.stats;
        let sh = &self.shared;
        sh.blocks.store(st.blocks, Ordering::Relaxed);
        sh.frames.fetch_add(n as u64, Ordering::Relaxed);
        sh.resets.store(st.resets, Ordering::Relaxed);
        sh.skipped.store(st.skipped, Ordering::Relaxed);
        sh.jitter_us.store((st.jitter_sd() * 1000.0) as u64, Ordering::Relaxed);
        sh.jitter_max_us.store((st.err_max * 1000.0) as u64, Ordering::Relaxed);
        sh.emitted.store(ss.emitted, Ordering::Relaxed);
        sh.late.store(ss.late, Ordering::Relaxed);
        sh.late_max_us.store((ss.late_max_ms * 1000.0) as u64, Ordering::Relaxed);
        sh.stale.store(ss.stale, Ordering::Relaxed);
        sh.panics.store(ss.panics, Ordering::Relaxed);
        // la reserve pleine (le fil audio) s'ajoute a la file pleine (le fil reseau)
        if ss.overflow > self.reported_overflow {
            sh.overflow.fetch_add(ss.overflow - self.reported_overflow, Ordering::Relaxed);
            self.reported_overflow = ss.overflow;
        }
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for LinkNotes {
    const CLAP_ID: &'static str = "com.uppwind.link.notes";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Plays the notes of an Uppwind Studio track as this plugin's MIDI output, through the Uppwind Link companion");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::Instrument, ClapFeature::NoteEffect, ClapFeature::Utility];
}

impl Vst3Plugin for LinkNotes {
    const VST3_CLASS_ID: [u8; 16] = *b"UppwindLinkNotes";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Instrument, Vst3SubCategory::Tools];
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(len: u8, data: [u8; 3]) -> Out {
        Out { offset: 17, len, data, n: 1, t: 0.0, late_ms: 0.0 }
    }

    #[test]
    fn midi_bytes_become_note_events_that_round_trip() {
        match note_event(&out(3, [0x93, 61, 100])) {
            Some(NoteEvent::NoteOn { timing, channel, key, velocity, .. }) => {
                assert_eq!((timing, channel, key), (17, Channel::Number(3), Key::Number(61)));
                // un hote qui tronque comme un hote qui arrondit retrouve 100
                assert_eq!(((velocity * 127.0) as u8, (velocity * 127.0).round() as u8), (100, 100));
            }
            e => panic!("{e:?}"),
        }
        for v in 1..=127u8 {
            let f = velocity(v);
            assert_eq!(((f * 127.0) as u8, (f * 127.0).round() as u8), (v, v), "velocity {v}");
        }
        assert!(matches!(note_event(&out(3, [0x90, 61, 0])), Some(NoteEvent::NoteOff { .. })));
        assert!(matches!(note_event(&out(3, [0x81, 61, 64])), Some(NoteEvent::NoteOff { .. })));
        assert!(matches!(note_event(&out(3, [0xb0, 64, 127])), Some(NoteEvent::MidiCC { cc: 64, .. })));
        assert!(matches!(note_event(&out(3, [0xe0, 0, 0x40])), Some(NoteEvent::MidiPitchBend { .. })));
        assert!(matches!(note_event(&out(2, [0xd0, 99, 0])), Some(NoteEvent::MidiChannelPressure { .. })));
        assert!(matches!(note_event(&out(2, [0xc0, 5, 0])), Some(NoteEvent::MidiProgramChange { program: 5, .. })));
    }
}
