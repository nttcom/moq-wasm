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

echo "Launching Chrome with certificate: ${CERT_PEM}"

exec "${CHROME_BIN}" \
  --test-type \
  --origin-to-force-quic-on=127.0.0.1:4433,localhost:4433 \
  --ignore-certificate-errors-spki-list="${CERT_SPKI_BASE64}" \
  --use-fake-device-for-media-stream
