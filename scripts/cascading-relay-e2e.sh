#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

export RELAY_STDOUT_FILTER="${RELAY_STDOUT_FILTER:-relay=info,moqt=info}"
export RELAY_LOG_FILTER="${RELAY_LOG_FILTER:-relay=info,moqt=info}"
LOGS_PID=""
RESULT_LOG="$(mktemp)"

cleanup() {
  rm -f "$RESULT_LOG"
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
docker compose up -d redis relay-a relay-b
docker compose logs -f --no-color relay-a relay-b &
LOGS_PID=$!
RELAY_A_URL="$(node scripts/resolve-local-relay-url.mjs moqt://127.0.0.1:4433)"
RELAY_B_URL="$(node scripts/resolve-local-relay-url.mjs moqt://127.0.0.1:4434)"
echo "Using relay URLs: $RELAY_A_URL, $RELAY_B_URL"

if ! cargo run -p cascading-relay-e2e -- \
  --relay-a-url "$RELAY_A_URL" \
  --relay-b-url "$RELAY_B_URL" \
  --redis-url redis://localhost:6379 \
  --track-namespace App/Channel/UserA \
  --track-name video 2>&1 | tee "$RESULT_LOG"; then
  exit 1
fi

if ! grep -q "cascading relay e2e passed" "$RESULT_LOG"; then
  echo "cascading relay e2e did not print passed marker" >&2
  exit 1
fi
