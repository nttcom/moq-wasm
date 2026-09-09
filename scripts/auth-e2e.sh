#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

export RELAY_STDOUT_FILTER="${RELAY_STDOUT_FILTER:-relay=info,moqt=info}"
export RELAY_LOG_FILTER="${RELAY_LOG_FILTER:-relay=info,moqt=info}"

APPS_FILE="services/vts/apps.example.json"
APP_ID="ac8adbc8-a2ff-4c41-9f5e-fdaed5e1e65e"
RELAY_APP_ID="11111111-2222-3333-4444-555555555555"
ANON_ISSUER_URL="http://127.0.0.1:8080/anon-token"
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
  docker compose --profile auth down -v --remove-orphans
}
trap cleanup EXIT

mint() {
  node services/vts/bin/mint.mjs --apps "$APPS_FILE" "$@"
}

expect_passed() {
  if ! grep -q "auth e2e passed" "$RESULT_LOG"; then
    echo "auth e2e did not print passed marker" >&2
    exit 1
  fi
}

node scripts/ensure-relay-certs.mjs

# Reuse a prebuilt relay image when present (pulled from the registry in CI);
# otherwise build it locally.
if docker image inspect moqt-relay:local >/dev/null 2>&1; then
  echo "Reusing existing moqt-relay:local image (skipping build)."
else
  docker compose build relay-common
fi
docker compose --profile auth build vts anon-issuer

if [[ ! -d services/vts/node_modules ]]; then
  npm --prefix services/vts ci
fi
cargo build -p auth-e2e

APP_TOKEN="$(mint --app-id "$APP_ID" --publish site1 --subscribe site1 --ttl 1h)"
export AUTH_RELAY_TOKEN
AUTH_RELAY_TOKEN="$(mint --app-id "$RELAY_APP_ID" --publish "" --subscribe "" --ttl 8760h)"
export AUTH_VTS_URL="http://vts:8081/verify"

docker compose --profile auth up -d --wait redis vts anon-issuer relay-a relay-b
docker compose --profile auth logs -f --no-color relay-a relay-b vts anon-issuer &
LOGS_PID=$!

RELAY_A_URL="$(node scripts/resolve-local-relay-url.mjs moqt://127.0.0.1:4433)"
RELAY_B_URL="$(node scripts/resolve-local-relay-url.mjs moqt://127.0.0.1:4434)"
echo "Using relay URLs: $RELAY_A_URL, $RELAY_B_URL"

ANON_TOKEN="$(curl -fsS -X POST "$ANON_ISSUER_URL" | node -e 'let d="";process.stdin.on("data",c=>d+=c).on("end",()=>console.log(JSON.parse(d).token))')"
# Minted last so that its short lifetime starts as close to the run as possible.
SHORT_TOKEN="$(mint --app-id "$APP_ID" --publish site1 --subscribe site1 --ttl 30s)"

if ! AUTH_E2E_APP_ID="$APP_ID" \
  AUTH_E2E_APP_TOKEN="$APP_TOKEN" \
  AUTH_E2E_SHORT_TOKEN="$SHORT_TOKEN" \
  AUTH_E2E_RELAY_TOKEN="$AUTH_RELAY_TOKEN" \
  AUTH_E2E_ANON_TOKEN="$ANON_TOKEN" \
  cargo run -p auth-e2e -- \
    --relay-a-url "$RELAY_A_URL" \
    --relay-b-url "$RELAY_B_URL" 2>&1 | tee "$RESULT_LOG"; then
  exit 1
fi
expect_passed

docker compose --profile auth stop vts
if ! AUTH_E2E_APP_TOKEN="$APP_TOKEN" \
  cargo run -p auth-e2e -- \
    --relay-a-url "$RELAY_A_URL" \
    --scenario vts-down 2>&1 | tee "$RESULT_LOG"; then
  exit 1
fi
expect_passed
