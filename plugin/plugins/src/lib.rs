// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Uppwind Link — les plugins (phase 0, 25 oct. 2026). Une seule DLL, deux plugins, VST3 et CLAP :
//!
//! - « Uppwind Link Return » (ret.rs) : sur le master du DAW, envoie le son au studio par le compagnon. Remplace
//!   VB-CABLE, et marche en ASIO (le son ne passe pas par le melangeur de Windows).
//! - « Uppwind Link Notes » (notes.rs) : un instrument sans son dont la sortie MIDI joue les notes du studio. Remplace
//!   loopMIDI pour les notes (Live : « MIDI From » ; FL : le port de sortie MIDI du wrapper).
//!
//! Le temps reel : process() ne bloque jamais, n'alloue jamais, ne touche pas au reseau. Il ne parle au fil reseau
//! (link.rs) que par des files sans verrou de taille fixe (ring.rs) et des compteurs atomiques. Les instants passent
//! par une DLL (clock.rs) ; les notes sont placees a l'echantillon (sched.rs). Le protocole : uwl-proto.
//!
//! Licence : GPL-3.0-or-later.

pub mod clock;
pub mod link;
pub mod notes;
pub mod ret;
pub mod ring;
pub mod sched;

use nice_plug::prelude::*;

pub use notes::LinkNotes;
pub use ret::LinkReturn;

pub const VENDOR: &str = "Uppwind";
pub const URL: &str = "https://uppwind.lucasmoreau.fr/atelier";
pub const EMAIL: &str = "";

nice_export_clap!(LinkReturn, LinkNotes);
nice_export_vst3!(LinkReturn, LinkNotes);
