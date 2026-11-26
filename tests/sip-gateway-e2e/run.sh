#!/usr/bin/env bash
# Places calls through the SIP gateway to a local Asterisk standing in for
# Twilio Elastic SIP Trunking (asterisk/), and has it call the app's number,
# with a local VTS and relay: first from a Rust client standing in for a page,
# then from the phone example in Chromium (examples/browser/tests/phone-e2e.spec.ts)
# with the dev app server (app-server.mjs).
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
E2E_DIR="$ROOT_DIR/tests/sip-gateway-e2e"
cd "$ROOT_DIR"
source tests/compose-relays.sh

APPS_FILE="$E2E_DIR/apps.json"
KEYS="$E2E_DIR/keys"
LOGS="$(mktemp -d)"
PIDS=()

cleanup() {
  for pid in "${PIDS[@]}"; do
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  docker compose -f "$E2E_DIR/compose.yaml" down -v --remove-orphans >/dev/null 2>&1 || true
  echo "logs: $LOGS"
}
trap cleanup EXIT

# Waits `tries` times 0.2 s (100 by default) for `check` to pass.
wait_for() {
  local name="$1" check="$2" tries="${3:-100}"
  for ((i = 0; i < tries; i++)); do
    if eval "$check" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.2
  done
  echo "$name did not become ready (logs: $LOGS)" >&2
  return 1
}

# rustls refuses a self-signed certificate as its own trust anchor, so the
# trunk's certificate is issued by a CA of the run. The gateway's listener
# presents the same certificate.
rm -rf "$KEYS"
mkdir -p "$KEYS"
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj "/CN=sip-gateway-e2e CA" \
  -keyout "$KEYS/ca.key" -out "$KEYS/ca.crt" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj "/CN=localhost" \
  -keyout "$KEYS/trunk.key" -out "$KEYS/trunk.csr" 2>/dev/null
openssl x509 -req -in "$KEYS/trunk.csr" -CA "$KEYS/ca.crt" -CAkey "$KEYS/ca.key" \
  -CAcreateserial -days 1 -out "$KEYS/trunk.crt" \
  -extfile <(printf "subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth,clientAuth\n") 2>/dev/null
chmod 644 "$KEYS"/*

node scripts/ensure-relay-certs.mjs
cargo build -p relay -p vts -p sip-gateway -p sip-gateway-e2e
cargo build -p auth-token --features cli

docker compose -f "$E2E_DIR/compose.yaml" up -d
wait_for asterisk "docker compose -f '$E2E_DIR/compose.yaml' exec -T trunk asterisk -rx 'pjsip show transports' | grep -q transport-tls"

VTS_PORT=8081 VTS_APPS_FILE="$APPS_FILE" OTEL_SDK_DISABLED=true \
  ./target/debug/vts >"$LOGS/vts.log" 2>&1 &
PIDS+=($!)
wait_for vts "curl -fsS http://127.0.0.1:8081/healthz"

RELAY_PORT=4433 AUTH_VTS_URL=http://127.0.0.1:8081/verify OTEL_SDK_DISABLED=true \
  AUTH_RELAY_TOKEN="$(mint "$APPS_FILE" sip-e2e-relay --publish "" --subscribe "" --ttl 1h --relay-id relay-local)" \
  ./target/debug/relay >"$LOGS/relay.log" 2>&1 &
PIDS+=($!)
wait_for relay "grep -q 'Relay server started' '$LOGS/relay.log'"

# The trunk is published on 127.0.0.1 only, and its RTP leaves the container
# for the address the gateway's SDP names. Its INVITEs reach the gateway from
# the container network, whose address depends on the Docker host.
SIGNALLING_ADDRESSES="127.0.0.0/8,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,::1"
SIP_GATEWAY_SIP_ADDRESS=0.0.0.0:5062 \
  SIP_GATEWAY_SIP_PUBLIC_ADDRESS=127.0.0.1:5062 \
  SIP_GATEWAY_TLS_CERT_FILE="$KEYS/trunk.crt" \
  SIP_GATEWAY_TLS_KEY_FILE="$KEYS/trunk.key" \
  SIP_GATEWAY_TRUNK_HOST=127.0.0.1:5061 \
  SIP_GATEWAY_TRUNK_CA_FILE="$KEYS/ca.crt" \
  SIP_GATEWAY_TRUNK_USERNAME=gateway \
  SIP_GATEWAY_TRUNK_PASSWORD=gateway-secret \
  SIP_GATEWAY_TRUNK_SIGNALLING_ADDRESSES="$SIGNALLING_ADDRESSES" \
  SIP_GATEWAY_MEDIA_ADDRESS=127.0.0.1 \
  SIP_GATEWAY_RELAY_URL=moqt://localhost:4433 \
  SIP_GATEWAY_RELAY_INSECURE_SKIP_TLS_VERIFY=true \
  SIP_GATEWAY_VTS_URL=http://127.0.0.1:8081/verify \
  SIP_GATEWAY_REGISTRY_FILE="$APPS_FILE" \
  OTEL_SDK_DISABLED=true \
  ./target/debug/sip-gateway >"$LOGS/sip-gateway.log" 2>&1 &
PIDS+=($!)
wait_for sip-gateway "curl -fsS http://127.0.0.1:8099/healthz"

E2E_RELAY_URL=moqt://localhost:4433 \
  E2E_GATEWAY_URL=http://127.0.0.1:8099 \
  E2E_PAGE_TOKEN="$(mint "$APPS_FILE" sip-e2e --publish room --subscribe room --ttl 1h)" \
  E2E_CALL_TOKEN="$(mint "$APPS_FILE" sip-e2e --calls room --ttl 5m)" \
  E2E_APP_SECRET="$(app_secret "$APPS_FILE" sip-e2e)" \
  E2E_WEBHOOK_ADDRESS=127.0.0.1:8097 \
  E2E_COMPOSE_FILE="$E2E_DIR/compose.yaml" \
  ./target/debug/sip-gateway-e2e

# The webhook the Rust client served is gone; the app server takes its port.
APP_ID=sip-e2e APP_SECRET="$(app_secret "$APPS_FILE" sip-e2e)" GATEWAY_URL=http://127.0.0.1:8099 PORT=8097 \
  node "$E2E_DIR/app-server.mjs" >"$LOGS/app-server.log" 2>&1 &
PIDS+=($!)
wait_for app-server "grep -q 'listening' '$LOGS/app-server.log'"
(cd examples/browser && exec "$ROOT_DIR/node_modules/.bin/vite" --host 127.0.0.1 --port 4173 --strictPort) \
  >"$LOGS/vite.log" 2>&1 &
PIDS+=($!)
wait_for vite "curl -fsS http://127.0.0.1:4173/moq-wasm/examples/phone/index.html" 600
(cd examples/browser && \
  MEDIA_E2E_BASE_URL=http://127.0.0.1:4173 \
  PHONE_E2E_MOQT_URL=https://127.0.0.1:4433 \
  PHONE_E2E_APP_SERVER_URL=http://127.0.0.1:8097 \
  PHONE_E2E_COMPOSE_FILE="$E2E_DIR/compose.yaml" \
  npm run e2e:phone)
echo "sip-gateway browser e2e passed"
