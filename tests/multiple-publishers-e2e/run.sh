#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"
source tests/compose-relays.sh

export RELAY_STDOUT_FILTER="${RELAY_STDOUT_FILTER:-relay=info,moqt=info}"
export RELAY_LOG_FILTER="${RELAY_LOG_FILTER:-relay=info,moqt=info}"

trap stop_compose_relays EXIT

# relay-b serves the cross-relay handover scenario.
start_compose_relays relay-a relay-b
RELAY_A_URL="$(node scripts/resolve-local-relay-url.mjs moqt://127.0.0.1:4433)"
RELAY_B_URL="$(node scripts/resolve-local-relay-url.mjs moqt://127.0.0.1:4434)"
echo "Using relay URLs: $RELAY_A_URL, $RELAY_B_URL"

# Every scenario fails with a non-zero exit, so no output marker is needed.
MOQT_E2E_RELAY_URL="$RELAY_A_URL" MOQT_E2E_RELAY_B_URL="$RELAY_B_URL" \
  cargo run -p multiple-publishers-e2e
