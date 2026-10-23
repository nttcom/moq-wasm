# Sourced by tests/*-e2e/run.sh with the repository root as the working directory.

COMPOSE_LOGS_PID=""

start_compose_relays() {
  node scripts/ensure-relay-certs.mjs

  # CI pulls moqt-relay:local from the registry before this runs.
  if docker image inspect moqt-relay:local >/dev/null 2>&1; then
    echo "Reusing existing moqt-relay:local image (skipping build)."
  else
    docker compose build relay-common
  fi
  docker compose up -d --wait redis "$@"
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
}
