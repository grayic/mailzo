#!/bin/bash
# Publishes a new version of the Mac app as an in-app update, and as a .dmg for new people.
#
#   scripts/release.sh beta "What's new, in one or two plain sentences"
#   scripts/release.sh stable            # moves the current beta to everyone
#   scripts/release.sh rollback <version> stable   # points a channel back to an older version
#
# The version comes from app/src-tauri/tauri.conf.json: raise it before a beta release.
# Needs ~/.dakly-signing (the signing identity and the private update key) on this Mac.
set -euo pipefail
cd "$(dirname "$0")/.."

SIGNING="$HOME/.dakly-signing"
# Where the update feed lives, set on the releasing Mac only.
FEED_HOST="${DAKLY_UPDATE_HOST:?set DAKLY_UPDATE_HOST to the update server (an ssh host)}"
FEED_DIR="${DAKLY_UPDATE_DIR:?set DAKLY_UPDATE_DIR to the feed folder on that server}"
FEED_URL="${DAKLY_UPDATE_FEED:-https://updates.getmailzo.com}"
OUT=dist/updates

# Both go into commands run on the server, so nothing else may get through.
check_channel() {
  case "$1" in
    stable | beta) ;;
    *) echo "The channel must be stable or beta, not \"$1\"." >&2; exit 2 ;;
  esac
}
check_version() {
  [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
    { echo "The version must look like 1.2.3, not \"$1\"." >&2; exit 2; }
}

publish_channel() { # version channel [rollback]
  local version="$1" channel="$2" rollback="${3:-}"
  check_version "$version"
  check_channel "$channel"
  ssh "$FEED_HOST" "test -f '$FEED_DIR/$version/latest.json'" ||
    { echo "Version $version isn't on the update server." >&2; exit 1; }
  # A rollback says so in the list, which is the only way the app accepts an older version.
  # A normal release leaves it out, so people are never moved back by accident.
  local mark=""
  if [ -n "$rollback" ]; then mark=1; fi
  ssh "$FEED_HOST" "python3 -c 'import json,sys
d=json.load(open(sys.argv[1]))
d.pop(\"rollback\",None)
if sys.argv[3]: d[\"rollback\"]=True
json.dump(d,open(sys.argv[2],\"w\"),indent=2)' '$FEED_DIR/$version/latest.json' '$FEED_DIR/$channel.json.new' '$mark' && mv '$FEED_DIR/$channel.json.new' '$FEED_DIR/$channel.json'"
  # New people download the version everyone has.
  if [ "$channel" = stable ]; then
    ssh "$FEED_HOST" "cp '$FEED_DIR/$version/Mailzo.dmg' '$FEED_DIR/Mailzo.dmg.new' && mv '$FEED_DIR/Mailzo.dmg.new' '$FEED_DIR/Mailzo.dmg'"
  fi
  echo "The $channel channel now offers $version."
}

case "${1:-}" in
  stable)
    version=$(ssh "$FEED_HOST" "python3 -c 'import json;print(json.load(open(\"$FEED_DIR/beta.json\"))[\"version\"])'")
    publish_channel "$version" stable
    exit 0 ;;
  rollback)
    check_version "${2:-}"
    check_channel "${3:-}"
    publish_channel "$2" "$3" rollback
    exit 0 ;;
  beta) ;;
  *) echo "Usage: scripts/release.sh beta \"notes\" | stable | rollback <version> <channel>" >&2; exit 2 ;;
esac
NOTES="${2:-}"

[ -f "$SIGNING/updater.key" ] ||
  { echo "This Mac doesn't have the private update key in ~/.dakly-signing." >&2; exit 1; }

# Without the Apple certificate the build falls back to a throwaway signature, and every Mac
# that installs that update loses its saved passwords and permissions, as the app looks like
# another.
security find-identity -v -p codesigning | grep -q "Developer ID Application: .*(XZXDTFCSZ7)" ||
  { echo "This Mac doesn't have the Apple Developer ID certificate in its Keychain, so the update can't be signed properly. Nothing was published." >&2; exit 1; }

scripts/build-dmg.sh
APP="target/share/aarch64-apple-darwin/release/bundle/macos/Mailzo.app"
# The app must carry the fixed signing identity, or nothing goes out.
# Apple's Developer ID for the Creefty LLC team. (Until November 2026 it was a certificate of
# our own; a Mac that had that build signs in to its mail once more after this one.)
IDENTITY='certificate leaf[subject.OU] = XZXDTFCSZ7'
REQUIREMENT=$(codesign -d -r- "$APP" 2>&1 || true)
case "$REQUIREMENT" in
  *"$IDENTITY"*) ;;
  *) echo "The built app isn't signed with the Mailzo signing identity, so it wasn't published. What it has:" >&2
     echo "$REQUIREMENT" >&2
     exit 1 ;;
esac
VERSION=$(/usr/libexec/PlistBuddy -c "Print CFBundleShortVersionString" "$APP/Contents/Info.plist")
if ssh "$FEED_HOST" "test -e '$FEED_DIR/$VERSION'"; then
  echo "Version $VERSION is already published. Raise the version in app/src-tauri/tauri.conf.json." >&2
  exit 1
fi

# The update itself: the signed app, packed, and signed again with the private update key.
rm -rf "$OUT/$VERSION" && mkdir -p "$OUT/$VERSION"
TAR="$OUT/$VERSION/Mailzo.app.tar.gz"
COPYFILE_DISABLE=1 tar -czf "$TAR" -C "$(dirname "$APP")" Mailzo.app
(cd app && TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" ./node_modules/.bin/tauri signer sign \
  -f "$SIGNING/updater.key" -p "" "../$TAR" > /dev/null)
cp "dist/Mailzo-$VERSION.dmg" "$OUT/$VERSION/Mailzo.dmg"
python3 - "$VERSION" "$NOTES" "$FEED_URL" "$TAR.sig" > "$OUT/$VERSION/latest.json" <<'PY'
import json, sys, datetime
version, notes, feed, sig = sys.argv[1:]
print(json.dumps({
    "version": version,
    "notes": notes,
    "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "platforms": {"darwin-aarch64": {
        "signature": open(sig).read().strip(),
        "url": f"{feed}/{version}/Mailzo.app.tar.gz",
    }},
}, indent=2))
PY

# Old versions stay on the server, so any channel can be pointed back to one in a minute.
ssh "$FEED_HOST" "mkdir -p '$FEED_DIR'"
rsync -a "$OUT/$VERSION" "$FEED_HOST:$FEED_DIR/"
publish_channel "$VERSION" beta
echo "Beta testers get it now. When it's fine: scripts/release.sh stable"
