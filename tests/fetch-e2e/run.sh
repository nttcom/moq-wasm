#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"
source tests/compose-relays.sh

export RELAY_STDOUT_FILTER="${RELAY_STDOUT_FILTER:-relay=info,moqt=info}"
export RELAY_LOG_FILTER="${RELAY_LOG_FILTER:-relay=info,moqt=info}"

trap stop_compose_relays EXIT

# Both relay-a and relay-b are needed: single-relay scenarios use relay-a,
# and the multi-relay FETCH forwarding scenarios require both.
start_compose_relays relay-a relay-b
RELAY_A_URL="$(node scripts/resolve-local-relay-url.mjs moqt://127.0.0.1:4433)"
RELAY_B_URL="$(node scripts/resolve-local-relay-url.mjs moqt://127.0.0.1:4434)"
echo "Using relay URLs: relay-a=$RELAY_A_URL relay-b=$RELAY_B_URL"

# MOQT_E2E_RELAY_URL keeps the single-relay scenarios pointed at relay-a.
# MOQT_E2E_RELAY_A_URL / MOQT_E2E_RELAY_B_URL enable the multi-relay scenarios.
MOQT_E2E_RELAY_URL="$RELAY_A_URL" \
MOQT_E2E_RELAY_A_URL="$RELAY_A_URL" \
MOQT_E2E_RELAY_B_URL="$RELAY_B_URL" \
cargo run -p fetch-e2e
