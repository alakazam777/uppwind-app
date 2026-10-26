#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Uppwind
#
# Uppwind Link -- the macOS build (26 Oct 2026): the companion and the plugins, arm64 + x86_64, made "universal" with
# lipo, the plugins bundled (Uppwind Link.vst3 / .clap) and signed ad hoc (codesign -s -) so a DAW loads them on THIS
# Mac. A release is signed with a Developer ID and notarized instead: docs/handoff/uppwind-link/MAC-SIGNATURE.md.
#
#   sh tools/uppwind-link/dev/build-macos.sh [OUT]       (OUT defaults to tools/uppwind-link/dist/macos)
#   UWL_RELEASE=1 sh …/build-macos.sh                    also hides this Mac's paths in the binaries (as RELEASING.md
#                                                        does for Windows), so that dev/release_check.py passes
#
# Needs: Xcode or its Command Line Tools (clang, lipo, codesign), rustup with the aarch64-apple-darwin and
# x86_64-apple-darwin targets. Installs nothing, writes only under OUT and the cargo target folders.
set -eu
HERE=$(cd "$(dirname "$0")/.." && pwd)
OUT=${1:-"$HERE/dist/macos"}
CARGO=${CARGO:-cargo}
command -v "$CARGO" >/dev/null 2>&1 || CARGO="$HOME/.cargo/bin/cargo"
ARCHS="aarch64-apple-darwin x86_64-apple-darwin"

REPO=$(cd "$HERE/../.." && pwd)
CHOME=${CARGO_HOME:-"$HOME/.cargo"}
RHOME=${RUSTUP_HOME:-"$HOME/.rustup"}
for t in $ARCHS; do
  if [ "${UWL_RELEASE:-0}" = "1" ]; then
    CFG="target.$t.rustflags=['--remap-path-prefix=$CHOME=/cargo','--remap-path-prefix=$RHOME=/rustup','--remap-path-prefix=$REPO=/src']"
    "$CARGO" --config "$CFG" build --release --locked --target "$t" --manifest-path "$HERE/companion/Cargo.toml"
    "$CARGO" --config "$CFG" build --release --locked --target "$t" --manifest-path "$HERE/plugin/Cargo.toml"
  else
    "$CARGO" build --release --locked --target "$t" --manifest-path "$HERE/companion/Cargo.toml"
    "$CARGO" build --release --locked --target "$t" --manifest-path "$HERE/plugin/Cargo.toml"
  fi
done

mkdir -p "$OUT"
lipo -create -output "$OUT/uppwind-companion" \
  "$HERE/companion/target/aarch64-apple-darwin/release/uppwind-companion" \
  "$HERE/companion/target/x86_64-apple-darwin/release/uppwind-companion"
codesign --force --sign - "$OUT/uppwind-companion"

lipo -create -output "$OUT/libuppwind_link.dylib" \
  "$HERE/plugin/target/aarch64-apple-darwin/release/libuppwind_link.dylib" \
  "$HERE/plugin/target/x86_64-apple-darwin/release/libuppwind_link.dylib"
# the bundler of the host's own arch makes Uppwind Link.vst3 and Uppwind Link.clap, then signs them ad hoc
"$HERE/plugin/target/aarch64-apple-darwin/release/uwl-bundle" --dll "$OUT/libuppwind_link.dylib" --out "$OUT" --sign - \
  || "$HERE/plugin/target/x86_64-apple-darwin/release/uwl-bundle" --dll "$OUT/libuppwind_link.dylib" --out "$OUT" --sign -
rm -f "$OUT/libuppwind_link.dylib"

lipo -info "$OUT/uppwind-companion"
lipo -info "$OUT/Uppwind Link.vst3/Contents/MacOS/Uppwind Link"
codesign --verify --deep --strict "$OUT/Uppwind Link.vst3"
echo "macOS build in $OUT"
