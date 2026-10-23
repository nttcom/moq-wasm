#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"
source tests/compose-relays.sh

export RELAY_STDOUT_FILTER="${RELAY_STDOUT_FILTER:-relay=info,moqt=info}"
export RELAY_LOG_FILTER="${RELAY_LOG_FILTER:-relay=info,moqt=info}"
# Shorten eviction so the client's before/after-TTL assertions run in seconds.
# The client sleeps assume TTL=5s / interval=1s (see tests/cache-eviction-e2e).
export RELAY_CACHE_TTL_SECS="${RELAY_CACHE_TTL_SECS:-5}"
export RELAY_CACHE_EVICT_INTERVAL_SECS="${RELAY_CACHE_EVICT_INTERVAL_SECS:-1}"

trap stop_compose_relays EXIT

# cache-eviction-e2e talks to a single relay, so only relay-a (and its redis) is needed.
start_compose_relays relay-a
RELAY_URL="$(node scripts/resolve-local-relay-url.mjs moqt://127.0.0.1:4433)"
echo "Using relay URL: $RELAY_URL"

# The client asserts object-TTL and strong_count reclaim and panics/exits
# non-zero on mismatch, so a non-zero exit is the only failure signal needed.
MOQT_E2E_RELAY_URL="$RELAY_URL" cargo run -p cache-eviction-e2e
