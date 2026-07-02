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
# multiple-publishers-e2e talks to a single relay, so only relay-a (and its
# redis) is needed.
docker compose up -d redis relay-a
docker compose logs -f --no-color relay-a &
LOGS_PID=$!
RELAY_URL="$(node scripts/resolve-local-relay-url.mjs moqt://127.0.0.1:4433)"
echo "Using relay URL: $RELAY_URL"

# Bob asserts the first-writer-wins guard and panics on violation, so a non-zero
# exit is the only failure signal needed.
MOQT_E2E_RELAY_URL="$RELAY_URL" cargo run -p multiple-publishers-e2e
