#!/bin/bash
# Proves live mode works, using a throwaway copy: a style change and a screen
# change must reach the open window with no reload, and an engine change must
# restart the app. Puts every file back afterwards.
set -u
cd "$(dirname "$0")/.."
WORK=$(mktemp -d)
SHARE="$WORK/share"; mkdir -p "$SHARE"
cp app/ui/tokens.css "$WORK/tokens.css"
cp app/ui/components/SignIn.tsx "$WORK/SignIn.tsx"
cat > "$WORK/probe.js" <<'JS'
const stamp = Math.random().toString(36).slice(2);
setInterval(() => {
  const state = {
    stamp,
    title: document.querySelector("h1")?.innerText ?? "",
    probe: getComputedStyle(document.documentElement).getPropertyValue("--live-probe").trim(),
  };
  window.__TAURI_INTERNALS__.invoke("selftest_share", { name: "live", value: JSON.stringify(state) });
}, 200);
JS
LIVE=""
cleanup() {
  cp "$WORK/tokens.css" app/ui/tokens.css
  cp "$WORK/SignIn.tsx" app/ui/components/SignIn.tsx
  [ -n "$LIVE" ] && kill "$LIVE" 2>/dev/null && wait "$LIVE" 2>/dev/null
  while security delete-generic-password -s com.grayic.inbox.live-check >/dev/null 2>&1; do :; done
  rm -rf "$WORK"
}
trap cleanup EXIT

read_field() { python3 -c "import json,sys;print(json.load(open('$SHARE/live')).get('$1',''))" 2>/dev/null; }
wait_for() { # seconds, shell test
  local start=$SECONDS
  while [ $((SECONDS - start)) -lt "$1" ]; do
    if eval "$2"; then echo $((SECONDS - start)); return 0; fi
    sleep 0.2
  done
  return 1
}

INBOX_SELFTEST_DATA="$WORK/data" INBOX_SELFTEST_KEYCHAIN=com.grayic.inbox.live-check \
  INBOX_SELFTEST_DEVICE="Live check" INBOX_SELFTEST_SERVER="http://127.0.0.1:4791" \
  INBOX_SELFTEST_SHARE="$SHARE" INBOX_SELFTEST_JS="$WORK/probe.js" \
  scripts/live.sh >"$WORK/live.log" 2>&1 &
LIVE=$!

if ! wait_for 500 '[ -f "$SHARE/live" ] && [ -n "$(read_field title)" ]' >/dev/null; then
  echo "FAIL: the app did not open"; tail -20 "$WORK/live.log"; exit 1
fi
FIRST=$(read_field stamp)
echo "app opened, showing: $(read_field title)"

printf '\n:root { --live-probe: yes; }\n' >> app/ui/tokens.css
if T=$(wait_for 15 '[ "$(read_field probe)" = yes ]'); then echo "style change reached the window in about ${T}s"; else echo "FAIL: style change didn't arrive"; fi
[ "$(read_field stamp)" = "$FIRST" ] && echo "with no reload" || echo "FAIL: the page reloaded"

sed -i '' 's/title="Connect your email"/title="Connect your email now"/' app/ui/components/SignIn.tsx
if T=$(wait_for 15 '[ "$(read_field title)" = "Connect your email now" ]'); then echo "screen change reached the window in about ${T}s"; else echo "FAIL: screen change didn't arrive, showing: $(read_field title)"; fi
[ "$(read_field stamp)" = "$FIRST" ] && echo "with no reload" || echo "FAIL: the page reloaded"

touch core/src/lib.rs
if T=$(wait_for 200 '[ "$(read_field stamp)" != "$FIRST" ]'); then echo "engine change rebuilt and restarted the app in about ${T}s"; else echo "FAIL: the app didn't restart"; tail -10 "$WORK/live.log"; fi
