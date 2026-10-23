#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"
source tests/compose-relays.sh

export RELAY_STDOUT_FILTER="${RELAY_STDOUT_FILTER:-relay=info,moqt=info}"
export RELAY_LOG_FILTER="${RELAY_LOG_FILTER:-relay=info,moqt=info}"
RESULT_LOG="$(mktemp)"

cleanup() {
  rm -f "$RESULT_LOG"
  stop_compose_relays
}
trap cleanup EXIT

start_compose_relays relay-a relay-b
RELAY_A_URL="$(node scripts/resolve-local-relay-url.mjs moqt://127.0.0.1:4433)"
RELAY_B_URL="$(node scripts/resolve-local-relay-url.mjs moqt://127.0.0.1:4434)"
echo "Using relay URLs: $RELAY_A_URL, $RELAY_B_URL"

if ! cargo run -p cascading-relay-e2e -- \
  --relay-a-url "$RELAY_A_URL" \
  --relay-b-url "$RELAY_B_URL" \
  --redis-url redis://localhost:6379 \
  --track-namespace anon/App/Channel/UserA \
  --track-name video 2>&1 | tee "$RESULT_LOG"; then
  exit 1
fi

if ! grep -q "cascading relay e2e passed" "$RESULT_LOG"; then
  echo "cascading relay e2e did not print passed marker" >&2
  exit 1
fi
