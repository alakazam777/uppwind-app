#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Uppwind
#
# Uppwind Link -- the macOS build: the plugins FIRST (Uppwind Link.vst3 / .clap: arm64 + x86_64 joined with lipo,
# bundled by uwl-bundle, then signed), THEN the companion (arm64 + x86_64, lipo), which embeds that signed
# Uppwind Link.vst3 byte for byte (UWL_PLUGIN_BUNDLE, see companion/build.rs) and installs it itself
# (--install-plugins, --setup --apply). By default everything is signed ad hoc (codesign -s -), so a DAW loads the
# plugins on THIS Mac; a release signs the plugins with a Developer ID here and the companion in GitHub Actions
# (.github/workflows/macos-build.yml), which also notarizes the .dmg.
#
#   sh dev/build-macos.sh [OUT]                          (OUT defaults to dist/macos)
#   UWL_RELEASE=1 sh dev/build-macos.sh                  also hides this Mac's paths in the binaries (as RELEASING.md
#                                                        does for Windows), so that dev/release_check.py passes
#   UWL_NO_ADHOC=1 …                                     leaves the companion unsigned (CI signs it with a Developer ID)
#   UWL_PLUGIN_SIGN_IDENTITY="<identity>" …              signs the plugin bundles with this identity (a "Developer ID
#                                                        Application" name, or its SHA-1 hash), with the hardened
#                                                        runtime and Apple's timestamp, instead of ad hoc
#   UWL_SIGN_KEYCHAIN=<keychain> …                       the keychain that holds that identity (CI: the job's own)
#
# Without plugin/plugins/ in this folder, the companion alone (it then carries no plugins).
#
# Needs: Xcode or its Command Line Tools (clang, lipo, codesign), rustup with the aarch64-apple-darwin and
# x86_64-apple-darwin targets. Installs nothing, writes only under OUT and the cargo target folders.
set -eu
HERE=$(cd "$(dirname "$0")/.." && pwd)
OUT=${1:-"$HERE/dist/macos"}
mkdir -p "$OUT"
# (absolute: the companion's build.rs wants an absolute UWL_PLUGIN_BUNDLE)
OUT=$(cd "$OUT" && pwd)
CARGO=${CARGO:-cargo}
command -v "$CARGO" >/dev/null 2>&1 || CARGO="$HOME/.cargo/bin/cargo"
ARCHS="aarch64-apple-darwin x86_64-apple-darwin"
PLUGINS=0
[ -f "$HERE/plugin/plugins/Cargo.toml" ] && PLUGINS=1

# the checkout's root (the private repository's, or the public one's, which is this folder): remapped to /src
REPO=$(git -C "$HERE" rev-parse --show-toplevel 2>/dev/null || (cd "$HERE/../.." && pwd))
CHOME=${CARGO_HOME:-"$HOME/.cargo"}
RHOME=${RUSTUP_HOME:-"$HOME/.rustup"}

# build <target> <manifest>: --locked, and with UWL_RELEASE=1 the paths of this Mac remapped (never RUSTFLAGS)
build() {
  if [ "${UWL_RELEASE:-0}" = "1" ]; then
    "$CARGO" --config "target.$1.rustflags=['--remap-path-prefix=$CHOME=/cargo','--remap-path-prefix=$RHOME=/rustup','--remap-path-prefix=$REPO=/src']" \
      build --release --locked --target "$1" --manifest-path "$2"
  else
    "$CARGO" build --release --locked --target "$1" --manifest-path "$2"
  fi
}

# 1. the plugins, bundled and signed, before the companion that embeds them
if [ "$PLUGINS" = "1" ]; then
  for t in $ARCHS; do
    build "$t" "$HERE/plugin/Cargo.toml"
  done
  lipo -create -output "$OUT/libuppwind_link.dylib" \
    "$HERE/plugin/target/aarch64-apple-darwin/release/libuppwind_link.dylib" \
    "$HERE/plugin/target/x86_64-apple-darwin/release/libuppwind_link.dylib"
  rm -rf "$OUT/Uppwind Link.vst3" "$OUT/Uppwind Link.clap"
  # the bundler of the host's own arch writes Uppwind Link.vst3 and Uppwind Link.clap (Info.plist, PkgInfo, MacOS/)
  "$HERE/plugin/target/aarch64-apple-darwin/release/uwl-bundle" --dll "$OUT/libuppwind_link.dylib" --out "$OUT" \
    || "$HERE/plugin/target/x86_64-apple-darwin/release/uwl-bundle" --dll "$OUT/libuppwind_link.dylib" --out "$OUT"
  rm -f "$OUT/libuppwind_link.dylib"
  ID=${UWL_PLUGIN_SIGN_IDENTITY:--}
  for b in "$OUT/Uppwind Link.vst3" "$OUT/Uppwind Link.clap"; do
    if [ "$ID" = "-" ]; then
      codesign --force --sign - "$b"
    elif [ -n "${UWL_SIGN_KEYCHAIN:-}" ]; then
      codesign --force --options runtime --timestamp --keychain "$UWL_SIGN_KEYCHAIN" --sign "$ID" "$b"
    else
      codesign --force --options runtime --timestamp --sign "$ID" "$b"
    fi
    codesign --verify --deep --strict "$b"
  done
  lipo -info "$OUT/Uppwind Link.vst3/Contents/MacOS/Uppwind Link"
  if [ "$ID" = "-" ]; then echo "plugins signed ad hoc"; else echo "plugins signed with the given identity (hardened runtime, timestamp)"; fi
  # the companion below embeds exactly this signed bundle
  UWL_PLUGIN_BUNDLE="$OUT/Uppwind Link.vst3"
  export UWL_PLUGIN_BUNDLE
else
  echo "plugin/plugins is not in this folder: the companion alone, without the plugins"
fi

# 2. the companion
for t in $ARCHS; do
  build "$t" "$HERE/companion/Cargo.toml"
done
lipo -create -output "$OUT/uppwind-companion" \
  "$HERE/companion/target/aarch64-apple-darwin/release/uppwind-companion" \
  "$HERE/companion/target/x86_64-apple-darwin/release/uppwind-companion"
if [ "${UWL_NO_ADHOC:-0}" != "1" ]; then
  codesign --force --sign - "$OUT/uppwind-companion"
fi
lipo -info "$OUT/uppwind-companion"
echo "macOS build in $OUT"
