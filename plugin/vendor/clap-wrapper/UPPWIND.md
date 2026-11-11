# clap-wrapper, Uppwind's copy

The Audio Units of the Uppwind Link plugins (Logic Pro) come from the `clap-wrapper` crate
([blepfx/clap-wrapper-rs](https://github.com/blepfx/clap-wrapper-rs)), which wraps the plugins' CLAP export as AUv2. It
compiles, with `cc`, a copy of [free-audio/clap-wrapper](https://github.com/free-audio/clap-wrapper) 0.14.0 and of
Apple's [AudioUnitSDK](https://github.com/apple/AudioUnitSDK). This folder is that crate with three fixes. The plugin
workspace uses it through `[patch.crates-io]` in `../../Cargo.toml`; it is built only with the `auv2` feature.

## Where it comes from

- `clap-wrapper` 0.3.1 from crates.io, sha256 `bd1028157e0bb8ddb7b0f95269da629bb1b717e904752316ef7752b85c4bde82`
  (the checksum `Cargo.lock` held before the patch).
- Left out: `external/vst3sdk` (built only by the `vst3` feature, never enabled here: nice-plug makes the VST3),
  `Cargo.toml.orig`, `Cargo.lock`, `.cargo_vcs_info.json`.
- Licences, all kept in their folders: the crate MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`); clap-wrapper MIT
  (`external/clap-wrapper/LICENSE`); AudioUnitSDK Apache-2.0 (`external/AudioUnitSDK/LICENSE.txt`); the CLAP headers MIT
  (`external/clap/LICENSE`); ghc::filesystem MIT (`external/filesystem/LICENSE`); fmt MIT (its licence is in
  `external/clap-wrapper/libs/fmt/fmt/format.h`).

## The fixes (10 Nov 2026)

1. **The MIDI output of an Audio Unit keeps each event's sample offset.** `MIDIOutput::addNoteOn`, `addNoteOff` and
   `addMIDI3Byte` (`external/clap-wrapper/src/detail/auv2/auv2_base_classes.h`) took no time and gave every
   `MIDIPacket` the timestamp 0. For an AU's MIDI output callback, that timestamp is the event's sample offset in the
   render cycle, so every note came out at the start of its block. They now take a `time`, which
   `WrapAsAUV2::send` (`wrapasauv2.cpp`) sets to `event.header.time`. Measured on macOS 26.6.2, with a test AU host at
   48 kHz in 128-frame blocks and 64 sixteenth notes from Uppwind Link Notes: the interval error between notes went
   from a standard deviation of 42.6 samples (−112 to +16) to 0.7 (−1 to +2). Upstream `main` still has the 0 (read on
   9 Nov 2026).
2. **A fixed suffix for the two Objective-C class names** (`build.rs`): `wrapAsAUV2_cocoaUI_UppwindLink` instead of the
   build time in nanoseconds, so that two builds of the same sources give the same binary (`RELEASING.md`).
   `CLAP_WRAPPER_COCOA_SUFFIX` replaces the suffix.
3. **No log and no assert in a release** (`build.rs`, `wrapasauv2.cpp`): `NDEBUG` and `CLAP_WRAPPER_LOGLEVEL 0` when
   cargo builds without debug assertions (the crate defined neither, so `LOGINFO` printed on the host's standard output,
   and a failed `assert`, such as the one after the host's MIDI callback, would end the host's process). The two
   messages that went straight to `std::cout` go through `LOGINFO`. (11 Nov 2026) `build.rs` reads cargo's
   `CARGO_CFG_DEBUG_ASSERTIONS` for this, not `DEBUG`, which follows the debug info: a release built with debug info
   still has no assert, and a build with debug assertions keeps them.

To see the whole difference: download `clap-wrapper` 0.3.1 from crates.io
(`https://crates.io/api/v1/crates/clap-wrapper/0.3.1/download`), check its sha256 (above), unpack it, and compare it
with this folder: `diff -ru clap-wrapper-0.3.1 plugin/vendor/clap-wrapper`. Besides the files left out (above), the
differences are the three fixes and this file.

## Updating it

Take the new crate from crates.io, check its sha256, copy it here the same way, apply the three fixes again (or drop
those that upstream has taken), and run the plugins' tests (with `--features uppwind-link/auv2`), `sh dev/build-macos.sh`
(the Audio Units are built by default), and `auval -v` on the three components (`aufx UwRt Uppw`, `aumu UwNt Uppw`,
`aumi UwNm Uppw`). Then check fix 1 again in an Audio Unit host: Uppwind Link Notes playing regular notes, the interval
between two notes must stay within a few samples of the expected one (the measure of fix 1 above).
