#!/usr/bin/env bash
# Xcode build phase: builds the Rust app (crates/ios) as a static library for
# the platform and configuration being built, and copies it to
# $BUILT_PRODUCTS_DIR/libpm_ios.a, which the Frameworks phase links.
#
#   iphoneos        -> aarch64-apple-ios
#   iphonesimulator -> aarch64-apple-ios-sim
#   Release         -> cargo --release, anything else -> debug
#
# The phase always runs; cargo decides what to rebuild.
set -euo pipefail

# Xcode runs build phases with a minimal PATH.
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"

case "${PLATFORM_NAME:?run from Xcode}" in
  iphoneos) rust_target=aarch64-apple-ios ;;
  iphonesimulator) rust_target=aarch64-apple-ios-sim ;;
  *) echo "error: unsupported platform $PLATFORM_NAME" >&2; exit 1 ;;
esac

if [ "${CONFIGURATION:?run from Xcode}" = Release ]; then
  profile=release
  profile_flag=--release
else
  profile=debug
  profile_flag=
fi

workspace="$(cd "${PROJECT_DIR:?run from Xcode}/.." && pwd)"
target_dir="${CARGO_TARGET_DIR:-$workspace/target}"

if ! command -v cargo >/dev/null; then
  echo "error: cargo not found; install Rust from https://rustup.rs" >&2
  exit 1
fi

# Build scripts and proc macros are compiled for the Mac: keep Xcode's iOS
# SDK out of their way (rustc finds the iOS SDK itself through xcrun).
env -u SDKROOT \
  cargo build \
  --manifest-path "$workspace/Cargo.toml" \
  -p polyrhythmic-metronome-ios \
  --lib \
  --target "$rust_target" \
  $profile_flag

lib="$target_dir/$rust_target/$profile/libpm_ios.a"
dest="${BUILT_PRODUCTS_DIR:?run from Xcode}/libpm_ios.a"
mkdir -p "$BUILT_PRODUCTS_DIR"
# Copy only when cargo produced a new library, so an unchanged one does not
# relink the app. `cp -c` clones on APFS (the debug library is large).
if [ ! -e "$dest" ] || [ "$lib" -nt "$dest" ]; then
  rm -f "$dest"
  cp -c "$lib" "$dest" 2>/dev/null || cp "$lib" "$dest"
fi
