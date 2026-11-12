#!/bin/bash
# Everything that must pass before a change is finished.
set -e
cd "$(dirname "$0")/.."
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
node scripts/check-design.mjs
node scripts/check-markdown.mjs
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd app
pnpm -s exec tsc --noEmit
pnpm -s exec vite build
