// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! uwl-vst3host (19 nov.) : heberger un plugin VST3.
//!
//! - `vsthost` : le module d'un paquet .vst3 (LoadLibrary / dlopen, la fabrique) et le strict minimum pour nos propres
//!   plugins (Uppwind Link Notes et Return, nice-plug) ;
//! - `realhost` : ce qu'un plugin du commerce demande en plus (controleur a part, IMessage, IComponentHandler, etats,
//!   tous les bus, IPlugFrame) ;
//! - `child` (Windows ; macOS depuis le 20 nov., sa coquille AppKit dans `macui`) : le processus enfant du compagnon, un
//!   plugin par processus (`--plug-host`), et le scan d'un module (`--plug-scan`).

pub mod b64;
pub mod realhost;
pub mod vsthost;
#[cfg(any(windows, target_os = "macos"))]
pub mod child;
#[cfg(target_os = "macos")]
pub mod macui;

/// (23 nov., 0.4.14, relecture macOS) Une variable d'environnement d'essai (UWL_HOST_LINK_BUNDLE, UWL_HOST_MENU_LOG,
/// UWL_HOST_SNAP_DIR, UWL_HOST_PARK) : lue seulement par une version de developpement (debug, ou construite avec
/// UWL_DEV_BUILD=1 : les scripts d'essai de docs/handoff/hote-vst3-macos-*) ; une version publiee ne la voit jamais.
pub fn dev_env(name: &str) -> Option<std::ffi::OsString> {
    if cfg!(debug_assertions) || matches!(option_env!("UWL_DEV_BUILD"), Some("1")) {
        std::env::var_os(name)
    } else {
        None
    }
}
