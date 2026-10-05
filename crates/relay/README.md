# relay

MoQT relay. It accepts QUIC and WebTransport on one port, caches objects for
FETCH, forwards subscriptions between relays, and authenticates every session
through the [VTS](../vts/README.md).

## Run

```shell
make relay
```

Starts a local VTS, mints the relay's own token and runs `cargo run -p relay`
on `https://127.0.0.1:4433`. A self-signed certificate is generated under
`crates/relay/keys/` on the first run. For two relays, use `docker compose up -d`
(ports 4433 and 4434).

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `RELAY_PORT` | `4433` | QUIC and WebTransport listen port |
| `RELAY_INNER_PORT` | `RELAY_PORT + 1` | QUIC port other relays connect to |
| `RELAY_ID` | `relay-local` | Name in logs and in the route registry |
| `RELAY_ADVERTISE_HOST` | `localhost` | Host other relays use to reach this one |
| `REDIS_URL` | unset | Route registry shared by cascading relays; unset runs standalone |
| `AUTH_VTS_URL` | required | VTS verify endpoint, e.g. `http://vts:8081/verify` |
| `AUTH_RELAY_TOKEN` | required | JWT this relay presents to other relays |
| `AUTH_MAX_TOKEN_TTL_SECONDS` | `3600` | Longest client token lifetime accepted |
| `AUTH_CLOCK_LEEWAY_SECONDS` | `5` | Tolerance when checking `iat` / `exp` |
| `RELAY_CACHE_TTL_SECS` | `60` | How long objects stay in the cache |
| `RELAY_CACHE_EVICT_INTERVAL_SECS` | `5` | How often expired objects are drained |
| `RELAY_STDOUT_FILTER` | `relay=info,moqt=info` | `tracing` filter for stdout |
| `RELAY_OTEL_FILTER` | `relay=info,moqt=info` | `tracing` filter for OpenTelemetry traces |
| `RELAY_LOG_FILTER` | `relay=info,moqt=info` | `tracing` filter for OpenTelemetry logs |
| `RELAY_HOSTNAME` | `$HOSTNAME` | Host name reported in OpenTelemetry resources |

## OpenTelemetry

OTLP trace and log exporters are configured from the standard OpenTelemetry
variables. Example for an OTLP/HTTP backend:

```shell
OTEL_SERVICE_NAME=moqt-relay
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
OTEL_EXPORTER_OTLP_ENDPOINT=https://api.honeycomb.io
OTEL_EXPORTER_OTLP_HEADERS=x-honeycomb-team=your-api-key,x-honeycomb-dataset=moqt-relay
```

`OTEL_EXPORTER_OTLP_HEADERS` is optional.

## Design

[Architecture document](../docs/architecture/relay/architecture.md).
