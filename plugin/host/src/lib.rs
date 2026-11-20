// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! uwl-vst3host (19 nov.) : heberger un plugin VST3.
//!
//! - `vsthost` : le module d'un paquet .vst3 (LoadLibrary / dlopen, la fabrique) et le strict minimum pour nos propres
//!   plugins (Uppwind Link Notes et Return, nice-plug) ;
//! - `realhost` : ce qu'un plugin du commerce demande en plus (controleur a part, IMessage, IComponentHandler, etats,
//!   tous les bus, IPlugFrame) ;
//! - `child` (Windows) : le processus enfant du compagnon, un plugin par processus (`--plug-host`), et le scan d'un
//!   module (`--plug-scan`).

pub mod b64;
pub mod realhost;
pub mod vsthost;
#[cfg(windows)]
pub mod child;
