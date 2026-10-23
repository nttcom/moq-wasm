#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"
source tests/compose-relays.sh

export RELAY_STDOUT_FILTER="${RELAY_STDOUT_FILTER:-relay=info,moqt=info}"
export RELAY_LOG_FILTER="${RELAY_LOG_FILTER:-relay=info,moqt=info}"

trap stop_compose_relays EXIT

start_compose_relays relay-a
RELAY_URL="$(node scripts/resolve-local-relay-url.mjs moqt://127.0.0.1:4433)"
echo "Using relay URL: $RELAY_URL"

PLUGIN_EXT=$([[ "$(uname -s)" == Darwin ]] && echo dylib || echo so)

cargo build -p gst-plugin-moqt
MOQT_E2E_PLUGIN_FILE="$ROOT_DIR/target/debug/libgstmoqt.$PLUGIN_EXT" MOQT_E2E_RELAY_URL="$RELAY_URL" \
  cargo run -p moqtsink-e2e
