#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"
source tests/compose-relays.sh

export RELAY_STDOUT_FILTER="${RELAY_STDOUT_FILTER:-relay=info,moqt=info}"
export RELAY_LOG_FILTER="${RELAY_LOG_FILTER:-relay=info,moqt=info}"
LIVE_TRANSLATE_PID=""
LIVE_TRANSLATE_PORT=18090
# The VTS verifies a temporary app's token with this secret.
export VTS_TEMPORARY_APP_SECRET="development-temporary-app-secret"

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
cargo build -p auth-token --features cli
RUST_LOG="${LIVE_TRANSLATE_LOG:-live_translate=info,moqt=warn}" PORT="$LIVE_TRANSLATE_PORT" \
  ./target/debug/live-translate --relay "$RELAY_URL" --insecure --echo --public-root &
LIVE_TRANSLATE_PID=$!

echo "== a public broadcast under anon/live-translate"
MOQT_E2E_RELAY_URL="$RELAY_URL" ./target/debug/live-translate-e2e

echo "== a broadcast of a temporary app, translated once its token is handed over"
TEMPORARY_APP_ID="temp-$(openssl rand -hex 16)"
TEMPORARY_TOKEN="$(AUTH_TOKEN_SECRET="$VTS_TEMPORARY_APP_SECRET" \
  ./target/debug/auth-token --app-id "$TEMPORARY_APP_ID" --publish "" --subscribe "" --ttl 1h)"
curl --fail --silent --show-error --retry 10 --retry-connrefused --retry-delay 1 \
  -X POST -H "Authorization: Bearer $TEMPORARY_TOKEN" \
  "http://127.0.0.1:$LIVE_TRANSLATE_PORT/sessions"
echo
MOQT_E2E_RELAY_URL="$RELAY_URL" \
  LIVE_TRANSLATE_E2E_ROOT="$TEMPORARY_APP_ID/live-translate" \
  LIVE_TRANSLATE_E2E_TOKEN="$TEMPORARY_TOKEN" \
  ./target/debug/live-translate-e2e
