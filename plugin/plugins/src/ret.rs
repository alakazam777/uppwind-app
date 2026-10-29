// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! « Uppwind Link Return » : un effet audio pour le master (ou une piste de retour) du DAW. Il envoie le son qui le
//! traverse au compagnon, qui le passe au studio par le WebSocket deja appaire (la meme voie que la capture WASAPI du
//! compagnon, sans passer par le melangeur de Windows : il marche aussi en ASIO et en WASAPI exclusif). Le son passe
//! tel quel dans le DAW, ou est coupe (« Mute DAW output » : seul le studio le joue, pas de son en double).
//!
//! Le fil audio (process) copie le bloc, entrelace, dans une file sans verrou, avec un en-tete (position, instant du
//! premier echantillon filtre par la DLL, taille, bits). Il ne bloque jamais, n'alloue jamais, ne touche pas au
//! reseau. File pleine (le fil reseau a cale) : le bloc est perdu, compte, et le suivant porte « discontinuite ».
//! Rien ne part tant que le compagnon n'est pas connecte, ni pendant un export (le DAW calcule plus vite que le temps
//! reel).

use crate::clock::{self, BlockClock};
use crate::link::{self, BlockHeader, Io, NetThread, Shared};
use crate::ring::{self, Producer};
use nice_plug::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use uwl_proto as p;

/// 2^19 echantillons : 5,4 s de stereo a 48 kHz (1,4 s a 192 kHz) d'avance possible sur le fil reseau.
pub const SAMPLE_RING: usize = 1 << 19;
pub const HEADER_RING: usize = 1 << 13;

#[derive(Params)]
pub struct ReturnParams {
    /// Couper la sortie du DAW apres ce plugin (le studio, lui, recoit le son) : pas de son en double.
    #[id = "mute"]
    pub mute: BoolParam,
    #[id = "bypass"]
    pub bypass: BoolParam,
    /// L'identite de l'instance, gardee dans le projet du DAW.
    #[persist = "instance"]
    pub instance: Arc<AtomicU64>,
}

impl Default for ReturnParams {
    fn default() -> Self {
        ReturnParams {
            mute: BoolParam::new("Mute DAW output", false),
            bypass: BoolParam::new("Bypass", false).make_bypass(),
            instance: Arc::new(AtomicU64::new(link::new_instance_id())),
        }
    }
}

pub struct LinkReturn {
    params: Arc<ReturnParams>,
    shared: Arc<Shared>,
    headers: Producer<BlockHeader>,
    samples: Producer<f32>,
    io: Option<Io>,
    net: Option<NetThread>,
    clock: BlockClock,
    pos: u64,
    /// les bits du prochain en-tete (une discontinuite apres un bloc perdu ou une remise a zero)
    pending: u8,
    realtime: bool,
}

impl Default for LinkReturn {
    fn default() -> Self {
        clock::init();
        let params = Arc::new(ReturnParams::default());
        let shared = Arc::new(Shared::new(p::Kind::Return, params.instance.clone()));
        let (headers, hc) = ring::ring(HEADER_RING);
        let (samples, sc) = ring::ring(SAMPLE_RING);
        LinkReturn {
            params,
            shared,
            headers,
            samples,
            io: Some(Io::Return { headers: hc, samples: sc }),
            net: None,
            clock: BlockClock::new(48000.0),
            pos: 0,
            pending: p::flags::DISCONTINUITY,
            realtime: true,
        }
    }
}

/// Deux canaux → une suite entrelacee, coupee en deux tranches (le tour de la file). `a` a une longueur paire : la
/// file a une capacite paire et on y ecrit toujours des trames entieres.
fn interleave(l: &[f32], r: &[f32], a: &mut [f32], b: &mut [f32]) {
    let fa = a.len() / 2;
    for i in 0..fa {
        a[2 * i] = l[i];
        a[2 * i + 1] = r[i];
    }
    for (j, i) in (fa..l.len()).enumerate() {
        b[2 * j] = l[i];
        b[2 * j + 1] = r[i];
    }
}

impl Plugin for LinkReturn {
    const NAME: &'static str = "Uppwind Link Return";
    const VENDOR: &'static str = crate::VENDOR;
    const URL: &'static str = crate::URL;
    const EMAIL: &'static str = crate::EMAIL;
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout { main_input_channels: NonZeroU32::new(2), main_output_channels: NonZeroU32::new(2), ..AudioIOLayout::const_default() },
        AudioIOLayout { main_input_channels: NonZeroU32::new(1), main_output_channels: NonZeroU32::new(1), ..AudioIOLayout::const_default() },
    ];

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
        self.pending |= p::flags::DISCONTINUITY;
        if self.net.is_none() {
            if let Some(io) = self.io.take() {
                self.net = Some(NetThread::start(self.shared.clone(), io));
            }
        }
        self.shared.active.store(true, Ordering::SeqCst);
        true
    }

    fn reset(&mut self) {
        self.clock.reset();
        self.pending |= p::flags::DISCONTINUITY;
    }

    fn deactivate(&mut self) {
        self.shared.active.store(false, Ordering::SeqCst);
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let n = buffer.samples();
        if n == 0 || buffer.channels() == 0 {
            return ProcessStatus::KeepAlive;
        }
        // (26 oct.) le tempo et la lecture du DAW (le studio suit le tempo de FL Studio sans son script)
        {
            let tr = ctx.transport();
            self.shared.note_transport(tr.tempo, tr.playing);
        }
        let tick = self.clock.tick(clock::now_ms(), n);
        if tick.reset {
            self.pending |= p::flags::DISCONTINUITY;
        }
        let bypass = self.params.bypass.value();
        if self.realtime && self.shared.connected() {
            let need = n * 2;
            if self.samples.free() >= need && self.headers.free() >= 1 {
                let mut flags = self.pending;
                if bypass {
                    flags |= p::flags::SILENT | p::flags::BYPASSED;
                    self.samples.write_with(need, |a, b| {
                        a.fill(0.0);
                        b.fill(0.0);
                    });
                } else {
                    let ch = buffer.as_slice_immutable();
                    let l: &[f32] = &ch[0];
                    let r: &[f32] = if ch.len() > 1 { &ch[1] } else { &ch[0] };
                    self.samples.write_with(need, |a, b| interleave(l, r, a, b));
                }
                self.headers.push(BlockHeader { pos: self.pos, t: tick.t0, frames: n as u32, flags });
                self.pending = 0;
            } else {
                self.shared.dropped.fetch_add(n as u64, Ordering::Relaxed);
                self.pending |= p::flags::DISCONTINUITY;
            }
        } else {
            self.pending |= p::flags::DISCONTINUITY;
        }
        if self.params.mute.value() && !bypass {
            for c in buffer.as_slice() {
                c.fill(0.0);
            }
        }
        self.pos += n as u64;
        let st = &self.clock.stats;
        let sh = &self.shared;
        sh.blocks.store(st.blocks, Ordering::Relaxed);
        sh.frames.fetch_add(n as u64, Ordering::Relaxed);
        sh.resets.store(st.resets, Ordering::Relaxed);
        sh.skipped.store(st.skipped, Ordering::Relaxed);
        sh.jitter_us.store((st.jitter_sd() * 1000.0) as u64, Ordering::Relaxed);
        sh.jitter_max_us.store((st.err_max * 1000.0) as u64, Ordering::Relaxed);
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for LinkReturn {
    const CLAP_ID: &'static str = "com.uppwind.link.return";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Sends the DAW's sound to Uppwind Studio through the Uppwind Link companion (works with ASIO)");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Utility, ClapFeature::Stereo];
}

impl Vst3Plugin for LinkReturn {
    const VST3_CLASS_ID: [u8; 16] = *b"UppwindLinkRetrn";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Tools];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleaving_across_the_ring_boundary() {
        let l = [1.0f32, 2.0, 3.0, 4.0];
        let r = [-1.0f32, -2.0, -3.0, -4.0];
        let mut a = [0f32; 6];
        let mut b = [0f32; 2];
        interleave(&l, &r, &mut a, &mut b);
        assert_eq!(a, [1.0, -1.0, 2.0, -2.0, 3.0, -3.0]);
        assert_eq!(b, [4.0, -4.0]);
    }
}
