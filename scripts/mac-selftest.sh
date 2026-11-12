#!/bin/bash
# Tests the real Mac app as two devices, against the demo mailbox and a local
# account server. Uses throwaway folders and Keychain names, and cleans up.
set -u
cd "$(dirname "$0")/.."
. "$HOME/.cargo/env"
export PATH="$HOME/.local/bin:$PATH"

APP=target/debug/bundle/macos/Mailzo.app/Contents/MacOS/inbox-app
WORK=$(mktemp -d)
SHARE="$WORK/share"; mkdir -p "$SHARE"
KC_A=com.grayic.inbox.selftest-a
KC_B=com.grayic.inbox.selftest-b
KC_C=com.grayic.inbox.selftest-c
KC_D=com.grayic.inbox.selftest-d
HOST_PID=""
ACCOUNT_PID=""
RELAY_PID=""

clean_keychain() {
  for service in "$KC_A" "$KC_B" "$KC_C" "$KC_D"; do
    while security delete-generic-password -s "$service" >/dev/null 2>&1; do :; done
  done
}
cleanup() {
  [ -n "$RELAY_PID" ] && kill "$RELAY_PID" 2>/dev/null
  [ -n "$HOST_PID" ] && kill "$HOST_PID" 2>/dev/null
  [ -n "$ACCOUNT_PID" ] && kill "$ACCOUNT_PID" 2>/dev/null
  clean_keychain
  rm -rf "$WORK"
}
trap cleanup EXIT
clean_keychain

# The names of the secrets in a device's Keychain item, one per line, or nothing if it has
# none. Asked through the app that keeps them, which the Keychain trusts, so macOS never asks
# for the login password partway through.
keychain_names() { # keychain
  "$APP" --selftest-keychain-names "$1" 2>/dev/null
}

start_app() { # name keychain device script
  cat scripts/selftest/common.js scripts/selftest/mail-flow.js scripts/selftest/team-common.js "$4" > "$WORK/$1.js"
  # ONLY_LINK=1 skips the long mail flows, for working on linking and removing devices.
  if [ -n "${ONLY_LINK:-}" ]; then
    sed -i '' -E '/await (mailFlow|allInboxFlow|senderPicturesFlow|multiSelectFlow|rowMenuFlow|settingsFlow|searchFlow|spamFlow|laterFlow|newslettersFlow|providersFlow)\(\);/d' "$WORK/$1.js"
  fi
  INBOX_SELFTEST_DATA="$WORK/$1-data" INBOX_SELFTEST_KEYCHAIN="$2" \
    INBOX_SELFTEST_DEVICE="$3" INBOX_SELFTEST_SERVER="http://127.0.0.1:4791" \
    INBOX_SELFTEST_GOOGLE="http://127.0.0.1:4792/authorize" \
    INBOX_SELFTEST_AVATARS="http://127.0.0.1:4790/avatars" \
    INBOX_SELFTEST_AI="http://127.0.0.1:4793" \
    INBOX_SELFTEST_DIRECTORY="http://127.0.0.1:4790/directory" \
    INBOX_SELFTEST_OAUTH="http://127.0.0.1:4794" \
    INBOX_SELFTEST_SHARE="$SHARE" INBOX_SELFTEST_LINKS="$WORK/links.txt" \
    INBOX_SELFTEST_JS="$WORK/$1.js" INBOX_SELFTEST_OUT="$WORK/$1.json" "$APP" >"$WORK/$1.log" 2>&1 &
  LAST_PID=$!
}

wait_app() { # pid name seconds
  for _ in $(seq 1 "$3"); do
    kill -0 "$1" 2>/dev/null || break
    sleep 1
  done
  if kill -0 "$1" 2>/dev/null; then
    # What it was doing when it got stuck.
    [ -n "${SAMPLE_TO:-}" ] && sample "$1" 2 -file "$SAMPLE_TO/$2.txt" >/dev/null 2>&1
    kill "$1"
    echo "FAIL: $2 did not finish in time"
    echo "last check anyone made: $(cat "$SHARE/progress" 2>/dev/null)"
    tail -5 "$WORK/$2.log"
  fi
  cat "$WORK/$2.json" 2>/dev/null || echo "FAIL: $2 wrote no report"
  echo
}

if curl -s -o /dev/null -m 2 http://127.0.0.1:4790/dev/pixel-hits; then
  echo "FAIL: stop the running demo server first; this test needs a fresh one"
  exit 1
fi
# Signing in, devices and teams need the account server for development, which lives in
# the private dakly-cloud project next to this one (or wherever DAKLY_CLOUD points).
CLOUD="${DAKLY_CLOUD:-$(cd .. && pwd)/dakly-cloud}"
if [ ! -f "$CLOUD/devaccount/Cargo.toml" ]; then
  echo "FAIL: this test needs the dakly-cloud project at $CLOUD (set DAKLY_CLOUD)"
  exit 1
fi
(cd "$CLOUD" && CARGO_TARGET_DIR="$CLOUD/target" cargo build -q -p devaccount) || exit 1
# Always this code, as an app built earlier would pass or fail for the wrong reasons.
(cd app && pnpm tauri build --debug --bundles app) >"$WORK/build.log" 2>&1 || { echo "FAIL: the app did not build"; tail -20 "$WORK/build.log"; exit 1; }
cargo build -q -p devhost
DAKLY_ACCOUNT_URL=http://127.0.0.1:4791 ./target/debug/devhost >"$WORK/host.log" 2>&1 &
HOST_PID=$!
"$CLOUD/target/debug/devaccount" >"$WORK/account.log" 2>&1 &
ACCOUNT_PID=$!
sleep 1

# The demo server keeps sign-in emails in memory. This passes on the newest code,
# what the outgoing server received, and whether any picture was requested.
(
  while true; do
    curl -s -m 2 "http://127.0.0.1:4790/dev/outbox" > "$SHARE/outbox.tmp" && mv "$SHARE/outbox.tmp" "$SHARE/outbox"
    curl -s -m 2 "http://127.0.0.1:4790/dev/pixel-hits" > "$SHARE/pixels.tmp" && mv "$SHARE/pixels.tmp" "$SHARE/pixels"
    curl -s -m 2 "http://127.0.0.1:4790/dev/last-code?email=demo@inbox.test" |
      sed -E 's/.*"code":"?([0-9]*)"?.*/\1/' > "$SHARE/code.tmp" && mv "$SHARE/code.tmp" "$SHARE/code"
    for who in Boss:boss Sam:sam; do
      found=$(curl -s -m 2 "http://127.0.0.1:4790/dev/last-code?email=${who#*:}@team.test" | grep -oE '[0-9]{6}')
      [ -n "$found" ] && printf '%s' "$found" > "$SHARE/code${who%:*}"
    done
    if [ -f "$SHARE/newmail" ]; then
      rm -f "$SHARE/newmail"
      curl -s -m 2 -X POST "http://127.0.0.1:4790/dev/team-mail" >/dev/null
    fi
    if [ -f "$SHARE/strangermail" ]; then
      rm -f "$SHARE/strangermail"
      curl -s -m 2 -X POST "http://127.0.0.1:4790/dev/stranger-mail" >/dev/null
    fi
    if [ -f "$SHARE/newslettermail" ]; then
      rm -f "$SHARE/newslettermail"
      curl -s -m 2 -X POST "http://127.0.0.1:4790/dev/newsletter-mail" >/dev/null
    fi
    if [ -f "$SHARE/codemail" ]; then
      rm -f "$SHARE/codemail"
      curl -s -m 2 -X POST "http://127.0.0.1:4790/dev/code-mail" >/dev/null
    fi
    if [ -f "$SHARE/revoke" ]; then
      rm -f "$SHARE/revoke"
      curl -s -m 2 -X POST "http://127.0.0.1:4790/dev/revoke-oauth" >/dev/null
    fi
    if [ -f "$SHARE/slowstore" ]; then
      secs=$(cat "$SHARE/slowstore"); rm -f "$SHARE/slowstore"
      curl -s -m 2 -X POST "http://127.0.0.1:4790/dev/slow-store/$secs" >/dev/null
    fi
    sleep 0.2
  done
) &
RELAY_PID=$!

# ONLY_LATER=1 runs just snooze, rules and the other daily tools, for working on them.
if [ -n "${ONLY_LATER:-}" ]; then
  start_app a "$KC_A" "Office Test" scripts/selftest/later-only.js; A_PID=$LAST_PID
  wait_app "$A_PID" a 600
  cat "$WORK/links.txt" 2>/dev/null | grep -c print | sed 's/^/print requests: /'
  exit 0
fi

# ONLY_TEAM=1 runs just the two-person team part, for working on it.
if [ -z "${ONLY_TEAM:-}" ]; then
echo "== two devices linking =="
start_app a "$KC_A" "Office Test" scripts/selftest/first-run.js; A_PID=$LAST_PID
# The Mac pauses a window left waiting behind another for this long, so the second one
# only opens when it's needed.
for _ in $(seq 1 700); do
  [ -f "$SHARE/aReady" ] && break
  kill -0 "$A_PID" 2>/dev/null || break
  sleep 1
done
start_app b "$KC_B" "Laptop Test" scripts/selftest/second-device.js; B_PID=$LAST_PID
wait_app "$A_PID" a 900
wait_app "$B_PID" b 120

echo "== links the app sent outside =="
cat "$WORK/links.txt" 2>/dev/null || echo "(none)"

echo "== evidence on disk =="
echo "picture requests, after asking to show them: $(curl -s http://127.0.0.1:4790/dev/pixel-hits)"
if [ -n "${ONLY_LINK:-}" ]; then echo "saved attachment: skipped (no mail flows)"; elif xattr -p com.apple.quarantine "$WORK/a-data/downloads/notes.txt" >/dev/null 2>&1; then echo "saved attachment marked as downloaded: yes"; else echo "FAIL saved attachment has no download mark"; fi
if security find-generic-password -s "$KC_A" -a secrets >/dev/null 2>&1; then echo "secrets in Keychain: yes"; else echo "FAIL secrets in Keychain: no"; fi
# The removed device keeps only "store-key", which locks its now-empty database.
NAMES=$(keychain_names "$KC_B")
LEFT=$(printf '%s\n' "$NAMES" | grep -vx -e 'store-key' -e '' | tr '\n' ' ' | sed 's/ *$//')
if [ -n "$LEFT" ]; then echo "FAIL removed device still has secrets: $LEFT"
elif [ "$NAMES" = "store-key" ]; then echo "removed device's secrets: gone (only the database key is left)"
elif security find-generic-password -s "$KC_B" -a secrets >/dev/null 2>&1; then echo "FAIL removed device's Keychain item could not be read"
else echo "removed device's secrets: gone"; fi
if cat "$WORK"/a-data/mail.db* | grep -aq demo-password; then echo "FAIL password found in the mail database"; else echo "password in mail database: no"; fi
echo "database permissions: $(stat -f '%Sp' "$WORK/a-data/mail.db")"
fi

# SKIP_TEAM=1 leaves out the two-person team part, for changes that don't touch teams.
if [ -z "${SKIP_TEAM:-}" ]; then
echo "== a team in two windows, at the same moment =="
start_app c "$KC_C" "Boss Test" scripts/selftest/team-boss.js; C_PID=$LAST_PID
start_app d "$KC_D" "Sam Test" scripts/selftest/team-sam.js; D_PID=$LAST_PID
wait_app "$C_PID" c 900
wait_app "$D_PID" d 120
fi
[ -n "${ONLY_TEAM:-}" ] && exit 0

echo "== restart with every server off =="
kill "$RELAY_PID"; RELAY_PID=""
kill "$HOST_PID"; wait "$HOST_PID" 2>/dev/null; HOST_PID=""
# The account server runs on its own now, so it has to be stopped too.
kill "$ACCOUNT_PID"; wait "$ACCOUNT_PID" 2>/dev/null; ACCOUNT_PID=""
start_app a "$KC_A" "Office Test" scripts/selftest/offline.js
rm -f "$WORK/a.json"
wait_app "$LAST_PID" a 60
