# VTS (Verify Token Service)

Verifies the JWT a MoQT client presents in `CLIENT_SETUP` and returns the
claims the relay authorizes requests against. The relay never holds signing
secrets; it calls this service once per session.

## API

```
POST /verify
Content-Type: application/json
{ "token": "eyJhbGci..." }

200 OK
{ "appId": "...", "isRelay": false, "claims": { "appId": "...", "publish": "site1", "iat": 1759591600, "exp": 1759678000 } }

401 Unauthorized
{ "error": "invalid_signature" }

GET /healthz -> 200 { "status": "ok" }
```

The VTS answers one question: was this token signed with the secret of a
registered app, and is that app a relay? `claims` is the JWT payload as
signed. What the claims mean, including `exp`, `iat`, the maximum lifetime
and the shape of `publish` / `subscribe`, is decided by the relay, so the
relay's clock and policy apply consistently to accepting a session and to
expiring it later.

`error` values, in the order they are checked: `malformed_token` (not a JWT,
or no string `appId`), `unknown_app`, `invalid_signature` (also returned for
any algorithm other than HS256).

## Configuration

| Variable                | Default              | Meaning                                                         |
| ----------------------- | -------------------- | --------------------------------------------------------------- |
| `VTS_APPS_FILE`         | `/etc/vts/apps.json` | Registered apps in a local file (see below)                     |
| `VTS_REGISTRY_BUCKET`   | unset                | Read the registered apps from this Cloud Storage bucket instead |
| `VTS_REGISTRY_OBJECT`   | `registry.json`      | Object in `VTS_REGISTRY_BUCKET`                                 |
| `STORAGE_EMULATOR_HOST` | unset                | Cloud Storage emulator URL, e.g. `http://localhost:4443`        |
| `VTS_PORT`              | `8081`               | Listen port                                                     |
| `VTS_STDOUT_FILTER`     | `vts=info`           | `tracing` filter for stdout; falls back to `RUST_LOG`           |
| `VTS_OTEL_FILTER`       | `vts=info`           | `tracing` filter for OpenTelemetry traces                       |
| `VTS_LOG_FILTER`        | `vts=info`           | `tracing` filter for OpenTelemetry logs                         |

OTLP export follows the standard `OTEL_*` variables (`OTEL_SDK_DISABLED`,
`OTEL_EXPORTER_OTLP_ENDPOINT`, …), as described in
`docs/architecture/telemetry/architecture.md`.

Set `VTS_APPS_FILE` or `VTS_REGISTRY_BUCKET`, not both. Either way the VTS
reads the registered apps at startup and again on every `/verify`, so a new
app, a rotated secret or a deleted app takes effect on the next verification
without a restart. If the registry cannot be read during a verification, the
VTS logs a warning and verifies with the apps it last read; if it cannot be
read at startup, the VTS stops.

The registered apps are a JSON array, the format `app-registry` reads;
`apps.example.json` holds local development values. Never commit a real file:
`crates/vts/apps.json` is ignored.

```json
[
  { "appId": "ac8adbc8-...", "secret": "<random>", "isRelay": false },
  { "appId": "11111111-...", "secret": "<random>", "isRelay": true }
]
```

Tokens are HS256 JWTs signed with the app's `secret` (UTF-8 bytes). Generate
secrets with e.g. `openssl rand -base64 32`.

## Signing tokens

The VTS only verifies. Tokens are signed with the `auth-token` CLI from the
app's secret, given in `AUTH_TOKEN_SECRET`; see
[`crates/auth-token/README.md`](../auth-token/README.md).

## Running

```
VTS_APPS_FILE=./crates/vts/apps.example.json cargo run -p vts
cargo test -p vts
```

The Dockerfile expects the repository root as build context:
`docker build -f crates/vts/Dockerfile .`

## Local stack

`docker compose up -d` starts the VTS with `apps.example.json` and points
both relays at it. The relays authenticate to each other with development
relay tokens, the compose defaults for `AUTH_RELAY_TOKEN`; each was minted
from the relay row of `apps.example.json` with its relay's `RELAY_ID`
(`relay-a`, `relay-b`) and is valid for ten years:

```
AUTH_TOKEN_SECRET="$(jq -r '.[] | select(.appId == "11111111-2222-3333-4444-555555555555") | .secret' crates/vts/apps.example.json)" \
  cargo run -p auth-token --features cli -- --app-id 11111111-2222-3333-4444-555555555555 --publish "" --subscribe "" --ttl 3650d --relay-id relay-a
```

Re-run that command for each relay and replace the defaults in
`docker-compose.yml` if the development relay secret changes. To run the stack against your own ledger,
set `VTS_APPS_FILE` and `AUTH_RELAY_TOKEN` (minted from that ledger) in `.env`.

`vts` is reachable only inside the compose network.

## Deployment

The VTS must be reachable only from the relays: put it on the relays' private
network and do not publish its port. The relay is pointed at it with
`AUTH_VTS_URL=http://<vts-host>:8081/verify` and authenticates its own
inter-relay connections with a token minted for the relay row:

```
AUTH_TOKEN_SECRET="$(jq -r '.[] | select(.appId == "<relay-app-id>") | .secret' apps.json)" \
  cargo run -p auth-token --features cli -- --app-id <relay-app-id> --publish "" --subscribe "" --ttl 8760h
```

### Where `apps.json` lives

`apps.json` contains every signing secret, so it is stored in a secret store
(GCP Secret Manager, AWS Secrets Manager, HashiCorp Vault, or the equivalent)
and mounted into the container as a file. The VTS only reads
`VTS_APPS_FILE`, so local compose (bind mount of `apps.example.json`) and
production (a file fetched from the secret store at VM start) run the same
binary. On GCP with Secret Manager:

```
gcloud secrets create vts-apps --replication-policy=automatic
gcloud secrets versions add vts-apps --data-file=apps.json
# On the VTS host: fetch the latest version to /etc/vts/apps.json before
# starting the container, and point VTS_APPS_FILE at it.
```

### Adding an app or rotating a secret

1. Edit `apps.json` (new row, or a new `secret` for an existing `appId`).
2. Register it as a new secret version:
   `gcloud secrets versions add vts-apps --data-file=apps.json`.
3. Restart the VTS; it reads the file at startup. Run two instances if the
   few seconds of restart, during which relays answer new connections with
   `INTERNAL_ERROR`, matter.
4. Tokens signed with a replaced secret fail with `invalid_signature` from the
   next connection; sessions already established stay up until their `exp`
   (at most 24 h for client tokens). Re-mint and redistribute relay tokens if
   the relay row changed.

A database (for example Cloud SQL) is not needed at this scale: the file is
changed a few times a month by the operators. Should tenant onboarding become
self-service or frequent, only `src/apps.rs` (the lookup) has to change; the
`/verify` contract and the relay stay as they are.

## Design

[Architecture document](../../docs/architecture/vts/architecture.md).
