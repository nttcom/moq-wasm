#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

cleanup() {
  docker compose down -v --remove-orphans
}
trap cleanup EXIT

docker compose build relay-common
docker compose up -d redis relay-a relay-b

cargo run -p cascading-relay-e2e -- \
  --relay-a-url moqt://localhost:4433 \
  --relay-b-url moqt://localhost:4434 \
  --redis-url redis://localhost:6379 \
  --track-namespace App/Channel/UserA \
  --track-name video
