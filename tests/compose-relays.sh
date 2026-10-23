# Sourced by tests/*-e2e/run.sh with the repository root as the working directory.

COMPOSE_LOGS_PID=""
VTS_PID=""

# Bypasses the compose vts service, whose image compiles every VTS dependency from scratch.
start_vts() {
  cargo build -p vts
  VTS_PORT=8081 VTS_APPS_FILE=crates/vts/apps.example.json ./target/debug/vts &
  VTS_PID=$!
  for _ in {1..100}; do
    if ! kill -0 "$VTS_PID" 2>/dev/null; then
      echo "vts exited before it became ready" >&2
      return 1
    fi
    if curl -fsS http://127.0.0.1:8081/healthz >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  echo "vts did not become ready" >&2
  return 1
}

stop_vts() {
  if [[ -n "$VTS_PID" ]]; then
    kill "$VTS_PID" 2>/dev/null || true
    wait "$VTS_PID" 2>/dev/null || true
    VTS_PID=""
  fi
}

start_compose_relays() {
  node scripts/ensure-relay-certs.mjs

  # CI pulls moqt-relay:local from the registry before this runs.
  if docker image inspect moqt-relay:local >/dev/null 2>&1; then
    echo "Reusing existing moqt-relay:local image (skipping build)."
  else
    docker compose build relay-common
  fi
  start_vts
  export AUTH_VTS_URL="http://host.docker.internal:8081/verify"
  # --no-deps skips the compose vts and also the redis health gate, so redis comes up first.
  docker compose up -d --wait redis
  docker compose up -d --wait --no-deps "$@"
  docker compose logs -f --no-color "$@" &
  COMPOSE_LOGS_PID=$!
}

stop_compose_relays() {
  if [[ -n "$COMPOSE_LOGS_PID" ]]; then
    kill "$COMPOSE_LOGS_PID" 2>/dev/null || true
    for _ in {1..10}; do
      if ! kill -0 "$COMPOSE_LOGS_PID" 2>/dev/null; then
        break
      fi
      sleep 0.1
    done
    kill -9 "$COMPOSE_LOGS_PID" 2>/dev/null || true
  fi
  docker compose down -v --remove-orphans
  stop_vts
}
