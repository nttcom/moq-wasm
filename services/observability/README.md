# observability

Maintains ClickHouse materialized views over 1 Hz relay stats snapshots (`otel.otel_logs`), serves app-scoped topology/series/usage queries over HTTP, and forwards client telemetry reports to OTLP/HTTP endpoints. Design: [architecture.md](../../docs/architecture/observability/architecture.md).

## Run

Start with ClickStack, HyperDX (`http://localhost:8080`), and a local demo relay:

```shell
docker compose -f compose.demo.yml -f compose.observability.yml up
```

Or run the binary directly against a local ClickHouse instance:

```shell
cargo run -p observability
```

Relays export stats only when `OTEL_SDK_DISABLED=false`. Open [`examples/browser/examples/observability/`](../../examples/browser/examples/observability/README.md) to view the live mesh topology and time-series charts.

## API

| Route | Behavior |
| --- | --- |
| `GET /api/snapshots[?at=<ms>&previous=<ms>]` | Latest (or historical, within 10 s) app-scoped relay snapshots |
| `GET /api/series?relay_id=&target=relay\|session\|track\|subscription&from=&to=&points=` | Bucketed rate/metric time series (max 7 days, `points <= 300`) |
| `GET /api/namespaces?from=&to=` | Up to 1,000 namespaces active in range |
| `GET /api/usage?from=&to=[&points=]` | Client-session traffic summary, per-relay totals, top namespaces and sessions |
| `POST /api/client-telemetry` | Accepts `ClientReport` (`<= 64 KiB`, rate-limited per IP) and forwards OTLP/JSON (`404` if no OTLP endpoint is set) |
| `GET /healthz` | Health check |

Requests without `Authorization` read `anon`. Requests with `Authorization: Bearer <JWT>` are verified via VTS and require an app-wide `subscribe: ""` claim; client IPs are masked in API responses.

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `OBSERVABILITY_HTTP_PORT` | `8095` | HTTP listen port |
| `CLICKHOUSE_URL` | `http://127.0.0.1:8123` | ClickHouse HTTP endpoint |
| `CLICKHOUSE_DATABASE` | `observability` | Database created and migrated at startup |
| `CLICKHOUSE_USER` / `CLICKHOUSE_PASSWORD` | — | ClickHouse Basic auth |
| `AUTH_VTS_URL` | — | VTS `/verify` URL (unset restricts queries to `anon` and returns `503` for tokens) |
| `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` | — | OTLP/HTTP `/v1/logs` URL for client logs |
| `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` | — | OTLP/HTTP `/v1/metrics` URL for player/publisher stats |
| `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` | — | OTLP/HTTP `/v1/traces` URL for client spans |
