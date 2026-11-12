#!/bin/bash
# Runs the screens in a browser against the real engine and the demo mailbox.
# Needs no real account: it starts signed in, with the demo mailbox demo@inbox.test on
# server 127.0.0.1, port 1143. Signing in, devices and teams also need Mailzo's account
# server for development (in the private dakly-cloud project); with it running, start this
# with DAKLY_ACCOUNT_URL=http://127.0.0.1:4791.
set -e
cd "$(dirname "$0")/.."
. "$HOME/.cargo/env"
cargo build -q -p devhost
./target/debug/devhost &
HOST_PID=$!
trap 'kill $HOST_PID 2>/dev/null' EXIT
cd app
pnpm -s dev
