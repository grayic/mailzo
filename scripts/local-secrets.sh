# Shared by the build and live scripts, for builds used on this Mac until the app
# has its Apple signature. Sets up the Keychain helper once and never replaces it,
# because a changed helper would make macOS ask for approval again.
ensure_key_helper() {
  HELPER_DIR="$HOME/Library/Application Support/com.grayic.inbox.keyhelper"
  export INBOX_SECRETS_HELPER="$HELPER_DIR/inbox-keyhelper"
  if [ ! -x "$INBOX_SECRETS_HELPER" ]; then
    echo "Setting up the Keychain helper…"
    CARGO_TARGET_DIR=target/keyhelper cargo build -q --release -p inbox-keyhelper
    mkdir -p "$HELPER_DIR"
    chmod 700 "$HELPER_DIR"
    cp target/keyhelper/release/inbox-keyhelper "$INBOX_SECRETS_HELPER"
  fi
}

# Browser sign-in IDs, kept out of the code in a file that's never committed.
if [ -f .env ]; then
  set -a
  . ./.env
  set +a
  # Empty means not set up, rather than a sign-in button that can't work.
  for name in INBOX_GOOGLE_CLIENT_ID INBOX_GOOGLE_CLIENT_SECRET INBOX_MICROSOFT_CLIENT_ID; do
    if [ -z "${!name:-}" ]; then
      unset "$name"
    fi
  done
fi
