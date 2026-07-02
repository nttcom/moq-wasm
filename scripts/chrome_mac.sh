#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
REPO_ROOT=$(cd "${SCRIPT_DIR}/.." && pwd)

RELAY_CERT_PEM="${REPO_ROOT}/relay/keys/cert.pem"
DEFAULT_CERT_PEM="${REPO_ROOT}/keys/cert.pem"

if [[ -n "${MOQT_CERT_PEM:-}" ]]; then
  CERT_PEM="${MOQT_CERT_PEM}"
elif [[ -f "${RELAY_CERT_PEM}" ]]; then
  CERT_PEM="${RELAY_CERT_PEM}"
elif [[ -f "${DEFAULT_CERT_PEM}" ]]; then
  CERT_PEM="${DEFAULT_CERT_PEM}"
else
  echo "certificate not found." >&2
  echo "expected one of:" >&2
  echo "  ${RELAY_CERT_PEM}" >&2
  echo "  ${DEFAULT_CERT_PEM}" >&2
  echo "" >&2
  echo "run the relay once to generate keys first:" >&2
  echo "  cargo run -p relay" >&2
  exit 1
fi

CHROME_BIN="${CHROME_BIN:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
if [[ ! -x "${CHROME_BIN}" ]]; then
  echo "Chrome binary not found: ${CHROME_BIN}" >&2
  echo "set CHROME_BIN to override the path." >&2
  exit 1
fi

CERT_SPKI_BASE64=$(
  openssl x509 -pubkey -noout -in "${CERT_PEM}" \
    | openssl pkey -pubin -outform der \
    | openssl dgst -sha256 -binary \
    | openssl enc -base64
)

RELAY_A_URL="${MOQT_RELAY_A_URL:-${CALL_RELAY_A_URL:-$(node "${REPO_ROOT}/scripts/resolve-local-relay-url.mjs" https://127.0.0.1:4433)}}"
RELAY_B_URL="${MOQT_RELAY_B_URL:-${CALL_RELAY_B_URL:-$(node "${REPO_ROOT}/scripts/resolve-local-relay-url.mjs" https://127.0.0.1:4434)}}"
RELAY_A_AUTHORITY=$(node -e "console.log(new URL(process.argv[1]).host)" "${RELAY_A_URL}")
RELAY_B_AUTHORITY=$(node -e "console.log(new URL(process.argv[1]).host)" "${RELAY_B_URL}")

BROWSER_APP_ORIGIN="${BROWSER_APP_ORIGIN:-http://localhost:5173}"
BROWSER_APP_ORIGIN="${BROWSER_APP_ORIGIN%/}"
DEFAULT_BROWSER_APP_URL="${BROWSER_APP_ORIGIN}/moq-wasm/index.html"
if [[ -n "${BROWSER_EXAMPLE_PATH:-}" ]]; then
  if [[ "${BROWSER_EXAMPLE_PATH}" == http://* || "${BROWSER_EXAMPLE_PATH}" == https://* ]]; then
    BROWSER_APP_URL="${BROWSER_EXAMPLE_PATH}"
  else
    NORMALIZED_BROWSER_EXAMPLE_PATH="${BROWSER_EXAMPLE_PATH}"
    if [[ "${NORMALIZED_BROWSER_EXAMPLE_PATH}" != /* ]]; then
      NORMALIZED_BROWSER_EXAMPLE_PATH="/${NORMALIZED_BROWSER_EXAMPLE_PATH}"
    fi
    BROWSER_APP_URL="${BROWSER_APP_ORIGIN}${NORMALIZED_BROWSER_EXAMPLE_PATH}"
  fi
else
  BROWSER_APP_URL="${BROWSER_APP_URL:-${CALL_APP_URL:-${DEFAULT_BROWSER_APP_URL}}}"
fi

BROWSER_APP_URL_WITH_RELAYS=$(
  node -e '
    const url = new URL(process.argv[1]);
    const relayAUrl = process.argv[2];
    const relayBUrl = process.argv[3];
    url.searchParams.set("relayAUrl", relayAUrl);
    url.searchParams.set("relayBUrl", relayBUrl);
    url.searchParams.set("relayUrl", relayAUrl);
    url.searchParams.set("moqtUrl", relayAUrl);
    url.searchParams.set("url", relayAUrl);
    url.searchParams.set("relay", relayAUrl);
    console.log(url.toString());
  ' "${BROWSER_APP_URL}" "${RELAY_A_URL}" "${RELAY_B_URL}"
)

CLEANUP_USER_DATA_DIR=0
USER_DATA_DIR="${CHROME_USER_DATA_DIR:-}"
if [[ -z "${USER_DATA_DIR}" ]]; then
  USER_DATA_DIR=$(mktemp -d -t moq-chrome-mac-XXXXXX)
  CLEANUP_USER_DATA_DIR=1
fi

cleanup() {
  if [[ "${CLEANUP_USER_DATA_DIR}" -eq 1 && -d "${USER_DATA_DIR}" ]]; then
    rm -rf "${USER_DATA_DIR}"
  fi
}

trap cleanup EXIT

echo "Launching Chrome with certificate: ${CERT_PEM}"
echo "Using relay URLs: ${RELAY_A_URL}, ${RELAY_B_URL}"
echo "Opening browser example: ${BROWSER_APP_URL_WITH_RELAYS}"

"${CHROME_BIN}" \
  --test-type \
  --user-data-dir="${USER_DATA_DIR}" \
  --origin-to-force-quic-on=127.0.0.1:4433,localhost:4433,127.0.0.1:4434,localhost:4434,${RELAY_A_AUTHORITY},${RELAY_B_AUTHORITY} \
  --ignore-certificate-errors-spki-list="${CERT_SPKI_BASE64}" \
  --ignore-certificate-errors \
  --use-fake-device-for-media-stream \
  --use-fake-ui-for-media-stream \
  --autoplay-policy=no-user-gesture-required \
  "${BROWSER_APP_URL_WITH_RELAYS}"
