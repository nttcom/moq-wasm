#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"
source tests/compose-relays.sh

export RELAY_STDOUT_FILTER="${RELAY_STDOUT_FILTER:-relay=info,moqt=info}"
export RELAY_LOG_FILTER="${RELAY_LOG_FILTER:-relay=info,moqt=info}"
LIVE_TRANSLATE_PID=""

stop_live_translate() {
  if [[ -n "$LIVE_TRANSLATE_PID" ]]; then
    kill "$LIVE_TRANSLATE_PID" 2>/dev/null || true
    for _ in {1..10}; do
      if ! kill -0 "$LIVE_TRANSLATE_PID" 2>/dev/null; then
        break
      fi
      sleep 0.1
    done
    kill -9 "$LIVE_TRANSLATE_PID" 2>/dev/null || true
  fi
}

cleanup() {
  stop_live_translate
  stop_compose_relays
}
trap cleanup EXIT

start_compose_relays relay-a
RELAY_URL="$(node scripts/resolve-local-relay-url.mjs moqt://127.0.0.1:4433)"
echo "Using relay URL: $RELAY_URL"

# --echo returns the source audio after each language's fixed Gemini delay,
# which is what the timestamp check expects.
cargo build -p live-translate -p live-translate-e2e
RUST_LOG="${LIVE_TRANSLATE_LOG:-live_translate=info,moqt=warn}" \
  ./target/debug/live-translate --relay "$RELAY_URL" --insecure --echo &
LIVE_TRANSLATE_PID=$!

MOQT_E2E_RELAY_URL="$RELAY_URL" ./target/debug/live-translate-e2e
