#!/bin/bash
# Builds the iPhone app and runs it in the simulator. Needs xcodegen on the PATH.
# It can't go on a real iPhone until the Apple Developer account is ready.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$(pwd)
. "$HOME/.cargo/env"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
(cd app && ./node_modules/.bin/vite build >/dev/null)
SIM="Inbox iPhone"
xcrun simctl list devices | grep -q "$SIM" ||
  xcrun simctl create "$SIM" "iPhone 16 Pro" "$(xcrun simctl list runtimes | grep -o 'com.apple.CoreSimulator.SimRuntime.iOS-[0-9-]*' | tail -1)"
cd app/src-tauri/gen/apple
xcodegen generate -q >/dev/null
xcodebuild -project inbox-app.xcodeproj -scheme inbox-app_iOS -configuration debug \
  -sdk iphonesimulator -destination "platform=iOS Simulator,name=$SIM" \
  -derivedDataPath "$CARGO_TARGET_DIR/iosbuild" ARCHS=arm64 ONLY_ACTIVE_ARCH=YES build | tail -1
APP="$CARGO_TARGET_DIR/iosbuild/Build/Products/debug-iphonesimulator/Mailzo.app"
xcrun simctl boot "$SIM" 2>/dev/null || true
xcrun simctl install "$SIM" "$APP"
xcrun simctl launch "$SIM" com.grayic.inbox
