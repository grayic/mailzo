#!/bin/bash
# Builds the Mac app for use on this Mac. It reaches the Keychain through a helper
# that never changes, so rebuilding never makes macOS ask for a password. Other
# people's Macs need the Apple Developer signature instead.
set -euo pipefail
cd "$(dirname "$0")/.."
. "$HOME/.cargo/env"
APP=target/release/bundle/macos/Mailzo.app

. scripts/local-secrets.sh
ensure_key_helper

(cd app && pnpm tauri build --bundles app)
# Without a signature on the whole app, macOS won't let it ask to notify, and with no
# permission it shows no count on the Dock icon.
codesign --force --deep --sign - "$APP"
echo "Built: $APP"
