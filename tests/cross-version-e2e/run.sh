#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

# A local relay rather than the compose one: the moqt-relay:local image may
# predate RELAY_MOQT_VERSIONS. Override the ports when they are taken.
RELAY_PORT="${CROSS_VERSION_E2E_RELAY_PORT:-4633}"
VTS_PORT="${CROSS_VERSION_E2E_VTS_PORT:-8081}"
# Development relay token minted from apps.example.json (see crates/vts/README.md).
DEFAULT_RELAY_TOKEN="eyJhbGciOiJIUzI1NiJ9.eyJhcHBJZCI6IjExMTExMTExLTIyMjItMzMzMy00NDQ0LTU1NTU1NTU1NTU1NSIsInB1Ymxpc2giOiIiLCJzdWJzY3JpYmUiOiIiLCJpYXQiOjE3ODkwOTMzMzMsImV4cCI6MjEwNDQ1MzMzM30.F_vveCJktf2C5cAnojDN_wLUZm6ILQpCACOvXxezqD4"

VTS_PID=""
RELAY_PID=""
WORK_DIR="$(mktemp -d)"

stop() {
  for pid in "$RELAY_PID" "$VTS_PID"; do
    if [[ -n "$pid" ]]; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  rm -rf "$WORK_DIR"
}
trap stop EXIT

wait_until() {
  local what="$1" pid="$2"
  shift 2
  for _ in {1..300}; do
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "$what exited before it became ready" >&2
      return 1
    fi
    if "$@"; then
      return 0
    fi
    sleep 0.1
  done
  echo "$what did not become ready" >&2
  return 1
}

cargo build -p vts -p relay -p cross-version-e2e

VTS_PORT="$VTS_PORT" VTS_APPS_FILE=crates/vts/apps.example.json ./target/debug/vts &
VTS_PID=$!
wait_until vts "$VTS_PID" curl -fsS "http://127.0.0.1:$VTS_PORT/healthz" -o /dev/null

RELAY_PORT="$RELAY_PORT" \
RELAY_MOQT_VERSIONS="draft-14,draft-18" \
RELAY_CERT_DIR="$WORK_DIR/keys" \
AUTH_VTS_URL="http://127.0.0.1:$VTS_PORT/verify" \
AUTH_RELAY_TOKEN="${AUTH_RELAY_TOKEN:-$DEFAULT_RELAY_TOKEN}" \
OTEL_SDK_DISABLED=true \
RELAY_STDOUT_FILTER="${RELAY_STDOUT_FILTER:-relay=info,moqt=info}" \
  ./target/debug/relay >"$WORK_DIR/relay.log" 2>&1 &
RELAY_PID=$!
wait_until relay "$RELAY_PID" grep -q "Relay server started" "$WORK_DIR/relay.log" || {
  cat "$WORK_DIR/relay.log" >&2
  exit 1
}

# The client exits non-zero on the first scenario that fails.
if ! MOQT_E2E_RELAY_URL="moqt://127.0.0.1:$RELAY_PORT" ./target/debug/cross-version-e2e; then
  echo "--- relay log ---" >&2
  tail -n 200 "$WORK_DIR/relay.log" >&2
  exit 1
fi
