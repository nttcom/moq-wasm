#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

export RELAY_STDOUT_FILTER="${RELAY_STDOUT_FILTER:-relay=info,moqt=info}"
export RELAY_LOG_FILTER="${RELAY_LOG_FILTER:-relay=info,moqt=info}"
LOGS_PID=""

cleanup() {
  if [[ -n "$LOGS_PID" ]]; then
    kill "$LOGS_PID" 2>/dev/null || true
    for _ in {1..10}; do
      if ! kill -0 "$LOGS_PID" 2>/dev/null; then
        break
      fi
      sleep 0.1
    done
    kill -9 "$LOGS_PID" 2>/dev/null || true
  fi
  docker compose down -v --remove-orphans
}
trap cleanup EXIT

node scripts/ensure-relay-certs.mjs

# Reuse a prebuilt relay image when present (pulled from the registry in CI);
# otherwise build it locally.
if docker image inspect moqt-relay:local >/dev/null 2>&1; then
  echo "Reusing existing moqt-relay:local image (skipping build)."
else
  docker compose build relay-common
fi
# Both relay-a and relay-b are needed: single-relay scenarios use relay-a,
# and the multi-relay FETCH forwarding scenarios require both.
docker compose up -d redis relay-a relay-b
docker compose logs -f --no-color relay-a relay-b &
LOGS_PID=$!
RELAY_A_URL="$(node scripts/resolve-local-relay-url.mjs moqt://127.0.0.1:4433)"
RELAY_B_URL="$(node scripts/resolve-local-relay-url.mjs moqt://127.0.0.1:4434)"
echo "Using relay URLs: relay-a=$RELAY_A_URL relay-b=$RELAY_B_URL"

# MOQT_E2E_RELAY_URL keeps the single-relay scenarios pointed at relay-a.
# MOQT_E2E_RELAY_A_URL / MOQT_E2E_RELAY_B_URL enable the multi-relay scenarios.
MOQT_E2E_RELAY_URL="$RELAY_A_URL" \
MOQT_E2E_RELAY_A_URL="$RELAY_A_URL" \
MOQT_E2E_RELAY_B_URL="$RELAY_B_URL" \
cargo run -p fetch-e2e
