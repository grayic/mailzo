#!/bin/bash
# Tests the real iPhone app in the simulator, against the demo mailbox and a local
# account server. Needs the app built and installed by scripts/ios-sim.sh.
set -u
cd "$(dirname "$0")/.."
. "$HOME/.cargo/env"
SIM="Inbox iPhone"
WORK=$(mktemp -d)
SHARE="$WORK/share"; mkdir -p "$SHARE"
cleanup() {
  [ -n "${RELAY_PID:-}" ] && kill "$RELAY_PID" 2>/dev/null
  [ -n "${HOST_PID:-}" ] && kill "$HOST_PID" 2>/dev/null
  xcrun simctl terminate "$SIM" com.grayic.inbox >/dev/null 2>&1
  rm -rf "$WORK"
}
trap cleanup EXIT
cargo build -q -p devhost
"${CARGO_TARGET_DIR:-target}/debug/devhost" >"$WORK/host.log" 2>&1 &
HOST_PID=$!
sleep 1
(
  while true; do
    curl -s -m 2 "http://127.0.0.1:4790/dev/outbox" > "$SHARE/outbox.tmp" && mv "$SHARE/outbox.tmp" "$SHARE/outbox"
    curl -s -m 2 "http://127.0.0.1:4790/dev/last-code?email=demo@inbox.test" |
      sed -E 's/.*"code":"?([0-9]*)"?.*/\1/' > "$SHARE/code.tmp" && mv "$SHARE/code.tmp" "$SHARE/code"
    sleep 0.3
  done
) &
RELAY_PID=$!
cat scripts/selftest/common.js scripts/selftest/mail-flow.js scripts/selftest/phone.js > "$WORK/phone.js"
xcrun simctl terminate "$SIM" com.grayic.inbox >/dev/null 2>&1
xcrun simctl uninstall "$SIM" com.grayic.inbox >/dev/null 2>&1
xcrun simctl install "$SIM" "${CARGO_TARGET_DIR:-target}/iosbuild/Build/Products/debug-iphonesimulator/Mailzo.app"
SIMCTL_CHILD_INBOX_SELFTEST_DATA="$WORK/data" \
SIMCTL_CHILD_INBOX_SELFTEST_KEYCHAIN="com.grayic.inbox.selftest-phone" \
SIMCTL_CHILD_INBOX_SELFTEST_DEVICE="Phone Test" \
SIMCTL_CHILD_INBOX_SELFTEST_SERVER="http://127.0.0.1:4791" \
SIMCTL_CHILD_INBOX_SELFTEST_AI="http://127.0.0.1:4793" \
SIMCTL_CHILD_INBOX_SELFTEST_DIRECTORY="http://127.0.0.1:4790/directory" \
SIMCTL_CHILD_INBOX_SELFTEST_AVATARS="http://127.0.0.1:4790/avatars" \
SIMCTL_CHILD_INBOX_SELFTEST_SHARE="$SHARE" \
SIMCTL_CHILD_INBOX_SELFTEST_LINKS="$WORK/links.txt" \
SIMCTL_CHILD_INBOX_SELFTEST_JS="$WORK/phone.js" \
SIMCTL_CHILD_INBOX_SELFTEST_OUT="$WORK/phone.json" \
  xcrun simctl launch "$SIM" com.grayic.inbox >/dev/null
for _ in $(seq 1 120); do
  [ -f "$WORK/phone.json" ] && break
  sleep 1
done
if [ -f "$WORK/phone.json" ]; then cat "$WORK/phone.json"; else echo "FAIL: the phone test wrote no report"; tail -5 "$WORK/host.log"; xcrun simctl io "$SIM" screenshot "${1:-/dev/null}" >/dev/null 2>&1; fi
echo
