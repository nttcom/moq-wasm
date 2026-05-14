#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
RELAY_KEYS_DIR="${1:-$SCRIPT_DIR/../../../keys}"

mkdir -p "$RELAY_KEYS_DIR"

openssl req -new -x509 -nodes \
  -newkey ec:<(openssl ecparam -name prime256v1) \
  -subj '/CN=localhost' \
  -addext "subjectAltName=DNS:localhost" \
  -days 14 \
  -keyout "$RELAY_KEYS_DIR/key.pem" \
  -out "$RELAY_KEYS_DIR/cert.pem" \
  2>/dev/null

HASH=$(openssl x509 -in "$RELAY_KEYS_DIR/cert.pem" -outform der \
  | openssl dgst -sha256 -binary \
  | base64)

echo "Certificate generated in $RELAY_KEYS_DIR"
echo "SHA-256 hash (base64): $HASH"
