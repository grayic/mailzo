#!/bin/bash
# Xcode runs this before building the iPhone app. It compiles the engine and screens
# into the library the app links to.
set -euo pipefail
. "$HOME/.cargo/env"
cd "$SRCROOT/../../../.."
case "$PLATFORM_NAME" in
  iphonesimulator) TARGET=aarch64-apple-ios-sim ;;
  *) TARGET=aarch64-apple-ios ;;
esac
FLAGS=()
PROFILE=debug
if [ "$CONFIGURATION" = "release" ]; then
  FLAGS+=(--release)
  PROFILE=release
fi
ROOT="${CARGO_TARGET_DIR:-target}"
# The screens go inside the app, rather than coming from a development server.
touch app/src-tauri/src/lib.rs
cargo build -q -p inbox-app --lib --target "$TARGET" --features tauri/custom-protocol ${FLAGS[@]+"${FLAGS[@]}"}
mkdir -p "$SRCROOT/Externals/arm64/$CONFIGURATION"
cp "$ROOT/$TARGET/$PROFILE/libinbox_app_lib.a" "$SRCROOT/Externals/arm64/$CONFIGURATION/libapp.a"
