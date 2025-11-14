#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CERT_DIR="${REPO_ROOT}/infra/nginx-rtmp/certs"

mkdir -p "${CERT_DIR}"

OPENSSL_SUBJECT="${OPENSSL_SUBJECT:-/CN=localhost}"
DAYS="${CERT_DAYS:-365}"

openssl req \
  -newkey rsa:2048 \
  -nodes \
  -x509 \
  -days "${DAYS}" \
  -keyout "${CERT_DIR}/server.key" \
  -out "${CERT_DIR}/server.crt" \
  -subj "${OPENSSL_SUBJECT}"

echo "Certificates created in ${CERT_DIR}"
