#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"
source tests/compose-relays.sh

export RELAY_STDOUT_FILTER="${RELAY_STDOUT_FILTER:-relay=info,moqt=info}"
export RELAY_LOG_FILTER="${RELAY_LOG_FILTER:-relay=info,moqt=info}"

APPS_FILE="crates/vts/apps.example.json"
APP_ID="ac8adbc8-a2ff-4c41-9f5e-fdaed5e1e65e"
OTHER_APP_ID="9f1c2a3b-4d5e-4f60-8a7b-1c2d3e4f5a6b"
RELAY_APP_ID="11111111-2222-3333-4444-555555555555"
# Short enough to keep the expiry scenarios brief, long enough for moq-cli's
# 10 s token-file poll to pick up a rewrite before the token expires.
SHORT_TOKEN_TTL="15s"
RESULT_LOG="$(mktemp)"

cleanup() {
  rm -f "$RESULT_LOG"
  stop_compose_relays
}
trap cleanup EXIT

mint() {
  local app_id="$1"
  shift
  AUTH_TOKEN_SECRET="$(jq -er --arg id "$app_id" '.[] | select(.appId == $id) | .secret' "$APPS_FILE")" \
    ./target/debug/auth-token --app-id "$app_id" "$@"
}

expect_passed() {
  if ! grep -q "auth e2e passed" "$RESULT_LOG"; then
    echo "auth e2e did not print passed marker" >&2
    exit 1
  fi
}

cargo build -p auth-token --features cli
cargo build -p auth-e2e
cargo build -p moq-cli

APP_TOKEN="$(mint "$APP_ID" --publish site1 --subscribe site1 --ttl 1h)"
OTHER_APP_TOKEN="$(mint "$OTHER_APP_ID" --publish "" --subscribe "" --ttl 1h)"
export AUTH_RELAY_TOKEN
AUTH_RELAY_TOKEN="$(mint "$RELAY_APP_ID" --publish "" --subscribe "" --ttl 8760h)"

start_compose_relays relay-a relay-b

RELAY_A_URL="$(node scripts/resolve-local-relay-url.mjs moqt://127.0.0.1:4433)"
RELAY_B_URL="$(node scripts/resolve-local-relay-url.mjs moqt://127.0.0.1:4434)"
echo "Using relay URLs: $RELAY_A_URL, $RELAY_B_URL"

# Minted last so that its short lifetime starts as close to the run as possible.
SHORT_TOKEN="$(mint "$APP_ID" --publish site1 --subscribe site1 --ttl "$SHORT_TOKEN_TTL")"

if ! AUTH_E2E_APP_ID="$APP_ID" \
  AUTH_E2E_APP_TOKEN="$APP_TOKEN" \
  AUTH_E2E_SHORT_TOKEN="$SHORT_TOKEN" \
  AUTH_E2E_RELAY_TOKEN="$AUTH_RELAY_TOKEN" \
  AUTH_E2E_OTHER_APP_TOKEN="$OTHER_APP_TOKEN" \
  cargo run -p auth-e2e -- \
    --relay-a-url "$RELAY_A_URL" \
    --relay-b-url "$RELAY_B_URL" 2>&1 | tee "$RESULT_LOG"; then
  exit 1
fi
expect_passed

# moq-cli: a publisher whose token file is rewritten before the short token
# expires stays connected; one whose file is left alone is closed by the relay.
MOQ_CLI_DIR="$(mktemp -d)"
REFRESHED_TOKEN_FILE="$MOQ_CLI_DIR/refreshed-token"
EXPIRING_TOKEN_FILE="$MOQ_CLI_DIR/expiring-token"
mint "$APP_ID" --publish site1 --subscribe site1 --ttl "$SHORT_TOKEN_TTL" > "$REFRESHED_TOKEN_FILE"
cp "$REFRESHED_TOKEN_FILE" "$EXPIRING_TOKEN_FILE"
# stdin for moq-cli is a FIFO whose only write end this script holds on fd 3:
# it delivers no data and no EOF, so moq-cli stays idle until fd 3 is closed.
# The subshells running moq-cli are started with fd 3 closed so they do not
# hold a write end themselves.
IDLE_STDIN="$MOQ_CLI_DIR/idle-stdin"
mkfifo "$IDLE_STDIN"
exec 3<>"$IDLE_STDIN"
# Records moq-cli's exit code in "$MOQ_CLI_DIR/<track>.exit" once it ends;
# the file's absence means it is still connected.
moq_cli_publish() {
  local status=0
  ./target/debug/moq-cli publish --relay "$RELAY_A_URL" --insecure --codec avc3 \
    --track "$APP_ID/site1/$1" --auth-token-file "$2" < "$IDLE_STDIN" > "$MOQ_CLI_DIR/$1.log" 2>&1 || status=$?
  echo "$status" > "$MOQ_CLI_DIR/$1.exit"
}
moq_cli_publish refreshed "$REFRESHED_TOKEN_FILE" 3<&- &
REFRESHED_PID=$!
moq_cli_publish expiring "$EXPIRING_TOKEN_FILE" 3<&- &
sleep 2
echo "$APP_TOKEN" > "$REFRESHED_TOKEN_FILE"
sleep 18
if [[ -f "$MOQ_CLI_DIR/refreshed.exit" ]]; then
  echo "moq-cli with a refreshed token file exited before the short token expired" >&2
  cat "$MOQ_CLI_DIR/refreshed.log" >&2
  exit 1
fi
if ! grep -q "authorization token refreshed" "$MOQ_CLI_DIR/refreshed.log"; then
  echo "moq-cli did not report a token refresh" >&2
  cat "$MOQ_CLI_DIR/refreshed.log" >&2
  exit 1
fi
if [[ ! -f "$MOQ_CLI_DIR/expiring.exit" ]] || ! grep -q "session closed by the relay" "$MOQ_CLI_DIR/expiring.log"; then
  echo "moq-cli with an untouched token file did not exit on the relay closing the session" >&2
  cat "$MOQ_CLI_DIR/expiring.log" >&2
  exit 1
fi
echo "moq-cli token refresh scenario passed"
exec 3<&-
for _ in {1..10}; do
  if [[ -f "$MOQ_CLI_DIR/refreshed.exit" ]]; then
    break
  fi
  sleep 1
done
pkill -P "$REFRESHED_PID" 2>/dev/null || true
wait "$REFRESHED_PID" 2>/dev/null || true
rm -rf "$MOQ_CLI_DIR"

stop_vts
if ! AUTH_E2E_APP_TOKEN="$APP_TOKEN" \
  cargo run -p auth-e2e -- \
    --relay-a-url "$RELAY_A_URL" \
    --scenario vts-down 2>&1 | tee "$RESULT_LOG"; then
  exit 1
fi
expect_passed
