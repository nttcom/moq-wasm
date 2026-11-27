# relay

MoQT relay accepting QUIC, WebTransport, and WebSocket on one port, with object caching (`FETCH`), cascading relay forwarding, and [VTS](../vts/README.md) session auth. Design: [architecture.md](../../docs/architecture/relay/architecture.md).

## Run

```shell
make relay
```

Starts a local VTS, mints a relay token, and runs the relay at `https://127.0.0.1:4433` with a self-signed certificate in `services/relay/keys/`. For a two-relay setup, run `make relay-certs && docker compose build relay-common && docker compose up -d` (ports 4433 and 4434).

## Configuration

Empty values are treated as unset.

| Variable | Default | Meaning |
| --- | --- | --- |
| `RELAY_PORT` | `4433` | Client port (QUIC/WebTransport on UDP, WebSocket on TCP) |
| `RELAY_INNER_PORT` | `RELAY_PORT + 1` | Inter-relay QUIC port |
| `RELAY_ID` | `relay-local` | Relay name in logs and route registry |
| `RELAY_ADVERTISE_HOST` | `localhost` | Hostname peers use to reach this relay (must match TLS cert) |
| `RELAY_INNER_CA_FILE` | OS roots | PEM CA bundle trusted for inter-relay connections |
| `RELAY_PUBLIC_HOST` | `localhost:RELAY_PORT` | Client-facing `host[:port]` advertised in `GOAWAY` |
| `RELAY_CERT_DIR` | `services/relay/keys` | Directory containing `cert.pem` and `key.pem` |
| `REDIS_URL` | unset | Shared route registry for cascading relays (unset = standalone) |
| `RELAY_DRAIN_TIMEOUT_SECS` | `30` | Grace period after `GOAWAY` on Ctrl+C / SIGTERM |
| `AUTH_VTS_URL` | required | VTS `/verify` endpoint (e.g. `http://vts:8081/verify`) |
| `AUTH_RELAY_TOKEN` | required | Inter-relay JWT signed with `--relay-id <RELAY_ID>` |
| `AUTH_MAX_TOKEN_TTL_SECONDS` | `86400` | Maximum accepted client token lifetime |
| `AUTH_CLOCK_LEEWAY_SECONDS` | `60` | Clock skew tolerance for `iat`/`exp` |
| `RELAY_CACHE_TTL_SECS` | `60` | In-memory object cache TTL |
| `RELAY_DISK_CACHE_DIR` | unset | Disk cache directory (retains client payloads up to 1 h; unset = memory only) |
| `RELAY_DISK_CACHE_BYTES` | `10737418240` (10 GiB) | Disk cache size cap (evicts oldest segments first) |
| `RELAY_CACHE_EVICT_INTERVAL_SECS` | `5` | Expired-object eviction interval |
| `RELAY_MOQT_VERSIONS` | `draft-18,draft-14` | Accepted drafts in preference order (cross-version relaying supported) |
| `MOQT_FETCH_FILL_TIMEOUT_SECS` | `20` | Timeout waiting for an upstream `FETCH` |
| `RELAY_STDOUT_FILTER`, `RELAY_OTEL_FILTER`, `RELAY_LOG_FILTER` | `relay=info,moqt=info` | `tracing` filters for stdout, OTLP traces, and OTLP logs |
| `RELAY_HOSTNAME` | `$HOSTNAME` | Host name in OpenTelemetry resource attributes |

## OpenTelemetry

OTLP trace and log exporters enable automatically when `OTEL_EXPORTER_OTLP_ENDPOINT` is set:

```shell
OTEL_SERVICE_NAME=moqt-relay
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
OTEL_EXPORTER_OTLP_ENDPOINT=https://api.honeycomb.io
OTEL_EXPORTER_OTLP_HEADERS=x-honeycomb-team=your-api-key,x-honeycomb-dataset=moqt-relay
```
