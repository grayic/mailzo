#!/bin/bash
# Live mode: the real app with your real mail, updating as the code changes.
# Screen changes appear in the open window straight away. Engine changes rebuild
# and restart the app by themselves. Quitting the app ends live mode.
set -u
cd "$(dirname "$0")/.."
. "$HOME/.cargo/env"
. scripts/local-secrets.sh
ensure_key_helper

# Its own build folder, so finished builds and tests never disturb it.
export CARGO_TARGET_DIR=target/live
BIN="$CARGO_TARGET_DIR/debug/inbox-app"
VITE_PID=""
APP_PID=""

stop_app() {
  [ -n "$APP_PID" ] && kill "$APP_PID" 2>/dev/null && wait "$APP_PID" 2>/dev/null
  APP_PID=""
}
cleanup() {
  stop_app
  [ -n "$VITE_PID" ] && pkill -P "$VITE_PID" 2>/dev/null
  [ -n "$VITE_PID" ] && kill "$VITE_PID" 2>/dev/null
}
trap cleanup EXIT
trap 'exit 0' INT TERM

# Everything that means the app itself must be rebuilt.
engine_fingerprint() {
  find core/src core/Cargo.toml app/src-tauri/src app/src-tauri/Cargo.toml \
    app/src-tauri/tauri.conf.json app/src-tauri/capabilities -type f -exec stat -f '%m %N' {} + | shasum
}

build_and_start() {
  echo "Building…"
  if ! cargo build -q -p inbox-app; then
    echo "The build failed, so the running app stays as it was."
    return 1
  fi
  stop_app
  "$BIN" &
  APP_PID=$!
  echo "Running."
}

if curl -s -o /dev/null -m 1 http://127.0.0.1:5177; then
  echo "Live mode is already running."
  exit 1
fi
# One app at a time may use the mail database. A self-test uses its own copy.
[ -z "${INBOX_SELFTEST_DATA:-}" ] && pkill -x inbox-app && sleep 1

(cd app && exec pnpm -s dev) &
VITE_PID=$!
for _ in $(seq 1 60); do
  curl -s -o /dev/null -m 1 http://127.0.0.1:5177 && break
  sleep 0.5
done

build_and_start || exit 1
SEEN=$(engine_fingerprint)
while kill -0 "$APP_PID" 2>/dev/null; do
  sleep 1
  NOW=$(engine_fingerprint)
  if [ "$NOW" != "$SEEN" ]; then
    SEEN=$NOW
    build_and_start
  fi
done
echo "The app was closed, so live mode has ended."
