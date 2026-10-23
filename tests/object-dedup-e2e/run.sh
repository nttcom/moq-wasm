#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"
source tests/compose-relays.sh

export RELAY_STDOUT_FILTER="${RELAY_STDOUT_FILTER:-relay=info,moqt=info}"
export RELAY_LOG_FILTER="${RELAY_LOG_FILTER:-relay=info,moqt=info}"

trap stop_compose_relays EXIT

# object-dedup-e2e talks to a single relay, so only relay-a (and its redis)
# is needed.
start_compose_relays relay-a
RELAY_URL="$(node scripts/resolve-local-relay-url.mjs moqt://127.0.0.1:4433)"
echo "Using relay URL: $RELAY_URL"

# bob asserts group 0 deduplicates to o0..o4 and panics on a duplicate, so a
# non-zero exit is the only failure signal needed.
MOQT_E2E_RELAY_URL="$RELAY_URL" cargo run -p object-dedup-e2e
