#!/bin/bash
# Builds a .dmg to share with other Macs with Apple chips. Google sign-in comes from
# .env, and so does Microsoft sign-in once INBOX_MICROSOFT_CLIENT_ID is there (it isn't a
# secret: Microsoft desktop apps have none). It leaves out the Keychain helper.
#
# With the Apple "Developer ID Application" certificate in this Mac's Keychain, the app
# and the .dmg are signed with it and checked by Apple (notarized), so they open on any
# Mac with no warning. That needs the Apple sign-in saved once with:
#   xcrun notarytool store-credentials mailzo-notary --team-id <team id>
# Without the certificate, it falls back to the older signatures below, and people open
# it the first time through Privacy & Security. Don't open this build on the Mac it was
# made on: being a new program, it would make macOS ask for the Keychain password.
set -euo pipefail
cd "$(dirname "$0")/.."
. "$HOME/.cargo/env"
. scripts/local-secrets.sh
unset INBOX_SECRETS_HELPER INBOX_ACCOUNT_SERVER

export CARGO_TARGET_DIR="$PWD/target/share"
rustup target add aarch64-apple-darwin
(cd app && pnpm tauri build --target aarch64-apple-darwin --bundles app)

APP="$CARGO_TARGET_DIR/aarch64-apple-darwin/release/bundle/macos/Mailzo.app"
VERSION=$(/usr/libexec/PlistBuddy -c "Print CFBundleShortVersionString" "$APP/Contents/Info.plist")
# Signed with the same identity every time, so macOS sees an updated copy as the same app and
# the Keychain doesn't ask for a password. The identity lives in ~/.dakly-signing, never here.
# Without it (another Mac), the build is signed ad-hoc and can't be published as an update.
SIGNING="$HOME/.dakly-signing"
NOTARY="${MAILZO_NOTARY_PROFILE:-mailzo-notary}"
DEVELOPER_ID="${MAILZO_DEVELOPER_ID:-$(security find-identity -v -p codesigning |
  sed -nE 's/.*"(Developer ID Application: [^"]+)".*/\1/p' | head -1)}"

# Sends a file to Apple to be checked, and waits for the answer.
notarize() {
  local log
  log=$(xcrun notarytool submit "$1" --keychain-profile "$NOTARY" --wait 2>&1) || true
  if ! printf '%s' "$log" | grep -q "status: Accepted"; then
    echo "Apple didn't accept $1:" >&2
    printf '%s\n' "$log" >&2
    local id
    id=$(printf '%s' "$log" | sed -nE 's/^ *id: ([0-9a-f-]+).*/\1/p' | head -1)
    [ -n "$id" ] && xcrun notarytool log "$id" --keychain-profile "$NOTARY" >&2 || true
    exit 1
  fi
}

if [ -n "$DEVELOPER_ID" ]; then
  echo "Signing with $DEVELOPER_ID"
  # Inside out: every program in the bundle, then the bundle itself.
  find "$APP/Contents" -type f -perm +111 ! -path "$APP/Contents/MacOS/*" -print0 |
    xargs -0 -I{} codesign --force --options runtime --timestamp --sign "$DEVELOPER_ID" {}
  codesign --force --options runtime --timestamp --sign "$DEVELOPER_ID" "$APP"
  ZIP="$CARGO_TARGET_DIR/Mailzo-notarize.zip"
  rm -f "$ZIP"
  ditto -c -k --keepParent "$APP" "$ZIP"
  echo "Apple is checking the app (a few minutes)..."
  notarize "$ZIP"
  rm -f "$ZIP"
  xcrun stapler staple -q "$APP"
elif [ -f "$SIGNING/codesign.p12" ] && [ -x "$SIGNING/tools/bin/rcodesign" ]; then
  "$SIGNING/tools/bin/rcodesign" sign --p12-file "$SIGNING/codesign.p12" \
    --p12-password-file "$SIGNING/codesign.pw" "$APP" > /dev/null
else
  echo "No Mailzo signing identity on this Mac: signing ad-hoc." >&2
  codesign --force --deep --sign - "$APP"
fi
codesign --verify --deep --strict "$APP"

# The window: scripts/dmg/background.html, saved as background.tiff by render-background.sh.
TOOLS="$CARGO_TARGET_DIR/dmg-tools"
[ -x "$TOOLS/bin/dmgbuild" ] || { python3 -m venv "$TOOLS"; "$TOOLS/bin/pip" -q install dmgbuild==1.6.5; }

mkdir -p dist
OUT="dist/Mailzo-$VERSION.dmg"
rm -f "$OUT"
"$TOOLS/bin/dmgbuild" -s scripts/dmg/settings.py -D app="$APP" \
  -D icon=app/src-tauri/icons/icon.icns -D background=scripts/dmg/background.tiff "Mailzo" "$OUT"
hdiutil verify -quiet "$OUT"
if [ -n "$DEVELOPER_ID" ]; then
  codesign --force --timestamp --sign "$DEVELOPER_ID" "$OUT"
  echo "Apple is checking the .dmg (a few minutes)..."
  notarize "$OUT"
  xcrun stapler staple -q "$OUT"
  spctl -a -t open --context context:primary-signature "$OUT"
  spctl -a -t exec "$APP"
fi
echo "Built: $OUT"
