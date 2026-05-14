#!/usr/bin/env bash

set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cert_path="${repo_root}/relay/keys/cert.pem"

if [[ ! -f "${cert_path}" ]]; then
  echo "Certificate not found: ${cert_path}" >&2
  echo "Run make relay once or node scripts/setup-media-e2e.mjs first." >&2
  exit 1
fi

chrome_bin="${CHROME_BIN:-}"
if [[ -z "${chrome_bin}" ]]; then
  for candidate in google-chrome google-chrome-stable chromium chromium-browser; do
    if command -v "${candidate}" >/dev/null 2>&1; then
      chrome_bin=$(command -v "${candidate}")
      break
    fi
  done
fi

if [[ -z "${chrome_bin}" ]]; then
  echo "Chrome/Chromium executable not found." >&2
  echo "Set CHROME_BIN=/path/to/chrome and retry make chrome:linux." >&2
  exit 1
fi

certbase64=$(
  openssl x509 -pubkey -noout -in "${cert_path}" \
    | openssl pkey -pubin -outform der \
    | openssl dgst -sha256 -binary \
    | openssl enc -base64 \
    | tr -d '\n'
)

cleanup_user_data_dir=0
user_data_dir="${CHROME_USER_DATA_DIR:-}"
if [[ -z "${user_data_dir}" ]]; then
  user_data_dir=$(mktemp -d -t moq-chrome-linux-XXXXXX)
  cleanup_user_data_dir=1
fi

cleanup() {
  if [[ "${cleanup_user_data_dir}" -eq 1 && -d "${user_data_dir}" ]]; then
    rm -rf "${user_data_dir}"
  fi
}

trap cleanup EXIT

"${chrome_bin}" \
  --test-type \
  --user-data-dir="${user_data_dir}" \
  --origin-to-force-quic-on=127.0.0.1:4433 \
  --ignore-certificate-errors-spki-list="${certbase64}" \
  --use-fake-device-for-media-stream \
  --use-fake-ui-for-media-stream \
  --autoplay-policy=no-user-gesture-required \
  "$@"
