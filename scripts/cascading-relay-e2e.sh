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

docker compose build relay-common
docker compose up -d redis relay-a relay-b
docker compose logs -f --no-color relay-a relay-b &
LOGS_PID=$!

if ! cargo run -p cascading-relay-e2e -- \
  --relay-a-url moqt://localhost:4433 \
  --relay-b-url moqt://localhost:4434 \
  --redis-url redis://localhost:6379 \
  --track-namespace App/Channel/UserA \
  --track-name video 2>&1 | tee "$RESULT_LOG"; then
  exit 1
fi

if ! grep -q "cascading relay e2e passed" "$RESULT_LOG"; then
  echo "cascading relay e2e did not print passed marker" >&2
  exit 1
fi
