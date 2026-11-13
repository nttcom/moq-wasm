# relay

MoQT relay. It accepts QUIC, WebTransport and WebSocket on one port number,
caches objects for FETCH, forwards subscriptions between relays, and
authenticates every session through the [VTS](../vts/README.md).
Design: [architecture document](../../docs/architecture/relay/architecture.md).

## Run

```shell
make relay
```

Starts a local VTS, mints the relay's own token and runs the relay on
`https://127.0.0.1:4433`, with a self-signed certificate generated under
`services/relay/keys/`. For two relays, use `docker compose up -d` (ports
4433 and 4434).

## Configuration

An empty value counts as unset and selects the default.

| Variable | Default | Meaning |
| --- | --- | --- |
| `RELAY_PORT` | `4433` | Client port: QUIC and WebTransport on UDP, WebSocket on TCP |
| `RELAY_INNER_PORT` | `RELAY_PORT + 1` | QUIC port other relays connect to |
| `RELAY_ID` | `relay-local` | Name in logs and in the route registry |
| `RELAY_ADVERTISE_HOST` | `localhost` | Host other relays use to reach this one; this relay's certificate must name it |
| `RELAY_INNER_CA_FILE` | the OS root certificates | PEM file of the certificates trusted when connecting to other relays |
| `RELAY_PUBLIC_HOST` | `localhost:RELAY_PORT` | `host[:port]` clients connect to, named in a GOAWAY sent for this relay |
| `RELAY_CERT_DIR` | `services/relay/keys` | Directory holding `cert.pem` and `key.pem` |
| `REDIS_URL` | unset | Route registry shared by cascading relays; unset runs standalone |
| `RELAY_DRAIN_TIMEOUT_SECS` | `30` | On Ctrl+C or SIGTERM, how long clients get to leave after GOAWAY |
| `AUTH_VTS_URL` | required | VTS verify endpoint, e.g. `http://vts:8081/verify` |
| `AUTH_RELAY_TOKEN` | required | JWT this relay presents to other relays, signed with `--relay-id` set to its `RELAY_ID` |
| `AUTH_MAX_TOKEN_TTL_SECONDS` | `3600` | Longest client token lifetime accepted |
| `AUTH_CLOCK_LEEWAY_SECONDS` | `5` | Tolerance when checking `iat` / `exp` |
| `RELAY_CACHE_TTL_SECS` | `60` | How long objects stay in memory |
| `RELAY_DISK_CACHE_DIR` | unset | Directory keeping the payloads from client publishers up to 1 hour; unset caches in memory only |
| `RELAY_DISK_CACHE_BYTES` | `10737418240` (10 GiB) | Cap on the disk cache; the oldest segments go first |
| `RELAY_CACHE_EVICT_INTERVAL_SECS` | `5` | How often expired objects are drained |
| `RELAY_MOQT_VERSIONS` | `draft-18,draft-14` | Drafts accepted, most preferred first; tracks are relayed between peers of either draft |
| `MOQT_FETCH_FILL_TIMEOUT_SECS` | `20` | The most the relay waits for an upstream FETCH |
| `RELAY_STDOUT_FILTER`, `RELAY_OTEL_FILTER`, `RELAY_LOG_FILTER` | `relay=info,moqt=info` | `tracing` filters for stdout, OpenTelemetry traces and OpenTelemetry logs |
| `RELAY_HOSTNAME` | `$HOSTNAME` | Host name reported in OpenTelemetry resources |

## OpenTelemetry

OTLP trace and log exporters follow the standard OpenTelemetry variables and
run only once `OTEL_EXPORTER_OTLP_ENDPOINT` is set:

```shell
OTEL_SERVICE_NAME=moqt-relay
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
OTEL_EXPORTER_OTLP_ENDPOINT=https://api.honeycomb.io
OTEL_EXPORTER_OTLP_HEADERS=x-honeycomb-team=your-api-key,x-honeycomb-dataset=moqt-relay
```
