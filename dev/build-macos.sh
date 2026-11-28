#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Uppwind
#
# Uppwind Link -- the macOS build: the plugins FIRST (Uppwind Link.vst3 / .clap / .component: arm64 + x86_64 joined
# with lipo, bundled by uwl-bundle, then signed), THEN the companion (arm64 + x86_64, lipo), which embeds the signed
# Uppwind Link.vst3 and Uppwind Link.component byte for byte (UWL_PLUGIN_BUNDLE, UWL_PLUGIN_COMPONENT, see
# companion/build.rs) and installs them itself (--install-plugins, --setup --apply). By default everything is signed ad hoc (codesign -s -), so a DAW loads the
# plugins on THIS Mac. A release (.github/workflows/macos-build.yml) runs it in two stages, UWL_STAGE=plugins then
# UWL_STAGE=companion, and signs in between, in jobs where no cargo runs (audit M8); it also notarizes the .dmg.
#
#   sh dev/build-macos.sh [OUT]                          (OUT defaults to dist/macos)
#   UWL_RELEASE=1 sh dev/build-macos.sh                  also hides this Mac's paths in the binaries (as RELEASING.md
#                                                        does for Windows), so that dev/release_check.py passes
#   UWL_NO_ADHOC=1 …                                     leaves the companion unsigned (CI signs it with a Developer ID)
#   UWL_PLUGIN_SIGN_IDENTITY="<identity>" …              signs the plugin bundles with this identity (a "Developer ID
#                                                        Application" name, or its SHA-1 hash), with the hardened
#                                                        runtime and Apple's timestamp, instead of ad hoc
#   UWL_SIGN_KEYCHAIN=<keychain> …                       the keychain that holds that identity (CI: the job's own)
#   UWL_STAGE=plugins …                                  (24 Nov, audit M8) the plugins only, bundled and NOT signed
#                                                        (CI signs them in a job of their own, where no cargo runs);
#                                                        writes Uppwind Link.vst3 and Uppwind Link.component in OUT
#   UWL_STAGE=companion …                                the companion only, embedding the bundles already named by
#                                                        UWL_PLUGIN_BUNDLE and UWL_PLUGIN_COMPONENT (absolute paths,
#                                                        signed beforehand); no plugin is built nor signed
#                                                        (default UWL_STAGE=all: both, as before)
#   UWL_AUV2=0 …                                         leaves out the Audio Units: by default (10 Nov) the plugins
#                                                        are built with the "auv2" feature, and Uppwind Link.component,
#                                                        the Audio Units for Logic Pro, is bundled and signed like the
#                                                        others, then carried and installed by the companion
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
STAGE=${UWL_STAGE:-all}
case "$STAGE" in
  all|plugins|companion) ;;
  *) echo "UWL_STAGE must be all, plugins or companion (not $STAGE)" >&2; exit 2 ;;
esac
command -v "$CARGO" >/dev/null 2>&1 || CARGO="$HOME/.cargo/bin/cargo"
ARCHS="aarch64-apple-darwin x86_64-apple-darwin"
PLUGINS=0
[ -f "$HERE/plugin/plugins/Cargo.toml" ] && PLUGINS=1
if [ "$STAGE" = "plugins" ] && [ "$PLUGINS" != "1" ]; then
  echo "UWL_STAGE=plugins: plugin/plugins is not in this folder" >&2; exit 2
fi
if [ "$STAGE" = "companion" ]; then
  # the bundles come from elsewhere (CI: signed in another job); they must exist, both unless UWL_AUV2=0
  if [ -z "${UWL_PLUGIN_BUNDLE:-}" ] || [ ! -d "$UWL_PLUGIN_BUNDLE" ]; then
    echo "UWL_STAGE=companion: UWL_PLUGIN_BUNDLE must name an existing Uppwind Link.vst3" >&2; exit 2
  fi
  export UWL_PLUGIN_BUNDLE
  if [ "${UWL_AUV2:-1}" = "1" ]; then
    if [ -z "${UWL_PLUGIN_COMPONENT:-}" ] || [ ! -d "$UWL_PLUGIN_COMPONENT" ]; then
      echo "UWL_STAGE=companion: UWL_PLUGIN_COMPONENT must name an existing Uppwind Link.component" >&2; exit 2
    fi
    export UWL_PLUGIN_COMPONENT
  else
    unset UWL_PLUGIN_COMPONENT
  fi
fi

# the checkout's root (the private repository's, or the public one's, which is this folder): remapped to /src
REPO=$(git -C "$HERE" rev-parse --show-toplevel 2>/dev/null || (cd "$HERE/../.." && pwd))
CHOME=${CARGO_HOME:-"$HOME/.cargo"}
RHOME=${RUSTUP_HOME:-"$HOME/.rustup"}

# build <target> <manifest> [cargo options]: --locked, and with UWL_RELEASE=1 the paths of this Mac remapped (never
# RUSTFLAGS)
build() {
  t=$1
  m=$2
  shift 2
  if [ "${UWL_RELEASE:-0}" = "1" ]; then
    "$CARGO" --config "target.$t.rustflags=['--remap-path-prefix=$CHOME=/cargo','--remap-path-prefix=$RHOME=/rustup','--remap-path-prefix=$REPO=/src']" \
      build --release --locked --target "$t" --manifest-path "$m" "$@"
  else
    "$CARGO" build --release --locked --target "$t" --manifest-path "$m" "$@"
  fi
}

# the Audio Units too (Logic Pro), unless UWL_AUV2=0 (9 Nov: a trial; 10 Nov: the default)
PLUGIN_FEATURES=""
BUNDLE_AUV2=""
if [ "${UWL_AUV2:-1}" = "1" ]; then
  PLUGIN_FEATURES="--features uppwind-link/auv2"
  BUNDLE_AUV2="--auv2"
fi

# 1. the plugins, bundled and signed, before the companion that embeds them
if [ "$STAGE" = "companion" ]; then
  echo "the companion embeds $UWL_PLUGIN_BUNDLE${UWL_PLUGIN_COMPONENT:+ and $UWL_PLUGIN_COMPONENT}"
elif [ "$PLUGINS" = "1" ]; then
  for t in $ARCHS; do
    # (PLUGIN_FEATURES: empty, or two words, so unquoted)
    build "$t" "$HERE/plugin/Cargo.toml" $PLUGIN_FEATURES
  done
  lipo -create -output "$OUT/libuppwind_link.dylib" \
    "$HERE/plugin/target/aarch64-apple-darwin/release/libuppwind_link.dylib" \
    "$HERE/plugin/target/x86_64-apple-darwin/release/libuppwind_link.dylib"
  rm -rf "$OUT/Uppwind Link.vst3" "$OUT/Uppwind Link.clap" "$OUT/Uppwind Link.component"
  # the bundler of the host's own arch writes Uppwind Link.vst3 and Uppwind Link.clap (Info.plist, PkgInfo, MacOS/),
  # and with UWL_AUV2=1 Uppwind Link.component
  "$HERE/plugin/target/aarch64-apple-darwin/release/uwl-bundle" --dll "$OUT/libuppwind_link.dylib" --out "$OUT" $BUNDLE_AUV2 \
    || "$HERE/plugin/target/x86_64-apple-darwin/release/uwl-bundle" --dll "$OUT/libuppwind_link.dylib" --out "$OUT" $BUNDLE_AUV2
  rm -f "$OUT/libuppwind_link.dylib"
  ID=${UWL_PLUGIN_SIGN_IDENTITY:--}
  BUNDLES="vst3 clap"
  [ -n "$BUNDLE_AUV2" ] && BUNDLES="vst3 clap component"
  for k in $BUNDLES; do
    b="$OUT/Uppwind Link.$k"
    if [ "$STAGE" = "plugins" ]; then
      # left unsigned: the signing job signs them, and nothing built here ever sees the certificate
      continue
    elif [ "$ID" = "-" ]; then
      codesign --force --sign - "$b"
    elif [ -n "${UWL_SIGN_KEYCHAIN:-}" ]; then
      codesign --force --options runtime --timestamp --keychain "$UWL_SIGN_KEYCHAIN" --sign "$ID" "$b"
    else
      codesign --force --options runtime --timestamp --sign "$ID" "$b"
    fi
    codesign --verify --deep --strict "$b"
  done
  lipo -info "$OUT/Uppwind Link.vst3/Contents/MacOS/Uppwind Link"
  if [ "$STAGE" = "plugins" ]; then
    # the CLAP is neither embedded nor shipped
    rm -rf "$OUT/Uppwind Link.clap"
    echo "plugins built in $OUT, NOT signed (UWL_STAGE=plugins)"
    exit 0
  fi
  if [ "$ID" = "-" ]; then echo "plugins signed ad hoc"; else echo "plugins signed with the given identity (hardened runtime, timestamp)"; fi
  # the companion below embeds exactly these signed bundles
  UWL_PLUGIN_BUNDLE="$OUT/Uppwind Link.vst3"
  export UWL_PLUGIN_BUNDLE
  if [ -n "$BUNDLE_AUV2" ]; then
    UWL_PLUGIN_COMPONENT="$OUT/Uppwind Link.component"
    export UWL_PLUGIN_COMPONENT
  else
    unset UWL_PLUGIN_COMPONENT
  fi
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
# (20 Nov 2026) uppwind-link-host, the program that loads hosted VST 3 plugins (companion/src/bin): built with the
# companion, put next to it; the release signs it with dev/host-entitlements.plist (macos-build.yml)
lipo -create -output "$OUT/uppwind-link-host" \
  "$HERE/companion/target/aarch64-apple-darwin/release/uppwind-link-host" \
  "$HERE/companion/target/x86_64-apple-darwin/release/uppwind-link-host"
if [ "${UWL_NO_ADHOC:-0}" != "1" ]; then
  codesign --force --sign - "$OUT/uppwind-companion"
  codesign --force --sign - "$OUT/uppwind-link-host"
fi
lipo -info "$OUT/uppwind-companion"
lipo -info "$OUT/uppwind-link-host"
echo "macOS build in $OUT"
