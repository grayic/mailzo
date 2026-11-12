#!/bin/bash
# Turns background.html into background.tiff (sharp on Retina screens too).
set -euo pipefail
cd "$(dirname "$0")"
CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
PROFILE=$(mktemp -d)
WORK=$(mktemp -d)
trap 'rm -rf "$PROFILE" "$WORK"' EXIT
for scale in 1 2; do
  "$CHROME" --headless=new --use-mock-keychain --password-store=basic --user-data-dir="$PROFILE" \
    --hide-scrollbars --window-size=660,390 --force-device-scale-factor=$scale \
    --screenshot="$WORK/bg$scale.png" "file://$PWD/background.html" 2>/dev/null
done
sips -s format tiff "$WORK/bg1.png" --out "$WORK/bg1.tiff" >/dev/null
sips -s format tiff -s dpiWidth 144 -s dpiHeight 144 "$WORK/bg2.png" --out "$WORK/bg2.tiff" >/dev/null
tiffutil -cathidpicheck "$WORK/bg1.tiff" "$WORK/bg2.tiff" -out background.tiff
