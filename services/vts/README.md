# VTS (Verify Token Service)

Verifies the JWT a MoQT client presents in `CLIENT_SETUP` and returns the
claims the relay authorizes requests against. The relay never holds signing
secrets; it calls this service once per session.
Design: [architecture document](../../docs/architecture/vts/architecture.md).

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
registered app, and is that app a relay? What the claims mean, `exp` and the
maximum lifetime included, is decided by the relay.

`error` values, in the order they are checked: `malformed_token`,
`unknown_app`, `invalid_signature` (also for any algorithm other than HS256).

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `VTS_APPS_FILE` | `/etc/vts/apps.json` | Registered apps in a local file |
| `VTS_REGISTRY_BUCKET` | unset | Read the registered apps from this Cloud Storage bucket instead |
| `VTS_REGISTRY_OBJECT` | `registry.json` | Object in `VTS_REGISTRY_BUCKET` |
| `STORAGE_EMULATOR_HOST` | unset | Cloud Storage emulator URL, e.g. `http://localhost:4443` |
| `VTS_TEMPORARY_APP_SECRET` | unset | Secret of every temporary app; unset disables them |
| `VTS_PORT` | `8081` | Listen port |
| `VTS_STDOUT_FILTER`, `VTS_OTEL_FILTER`, `VTS_LOG_FILTER` | `vts=info` | `tracing` filters for stdout, OpenTelemetry traces and OpenTelemetry logs |

OTLP export follows the standard `OTEL_*` variables
(`docs/architecture/telemetry/architecture.md`).

Set `VTS_APPS_FILE` or `VTS_REGISTRY_BUCKET`, not both. The VTS reads the
registered apps at startup and again on every `/verify`, so a new app, a
rotated secret or a deleted app takes effect without a restart.

The registered apps are a JSON array, the format `app-registry` reads.
`apps.example.json` holds local development values; never commit a real file.

```json
[
  { "appId": "ac8adbc8-...", "secret": "<random>", "isRelay": false },
  { "appId": "11111111-...", "secret": "<random>", "isRelay": true }
]
```

Tokens are HS256 JWTs signed with the app's `secret` (UTF-8 bytes) by the
`auth-token` CLI ([`crates/auth-token/README.md`](../../crates/auth-token/README.md));
the VTS only verifies. Generate secrets with e.g. `openssl rand -base64 32`.

An `appId` starting with `temp-` names a temporary app: it is not in the
registry, and its tokens are signed with `VTS_TEMPORARY_APP_SECRET` by
whoever issues them (`services/token-mint`). A temporary app is never a
relay.

## Running

```
VTS_APPS_FILE=./services/vts/apps.example.json cargo run -p vts
```

`docker compose up -d` starts the VTS with `apps.example.json` and points
both relays at it. The relays authenticate to each other with development
relay tokens, the compose defaults for `AUTH_RELAY_TOKEN`, each minted from
the relay row of `apps.example.json` with its relay's `RELAY_ID`:

```
AUTH_TOKEN_SECRET="$(jq -r '.[] | select(.appId == "11111111-2222-3333-4444-555555555555") | .secret' services/vts/apps.example.json)" \
  cargo run -p auth-token --features cli -- --app-id 11111111-2222-3333-4444-555555555555 --publish "" --subscribe "" --ttl 3650d --relay-id relay-a
```

To run the stack against your own apps, set `VTS_APPS_FILE` and
`AUTH_RELAY_TOKEN` in `.env`.

## Deployment

- The VTS must be reachable only from the relays: put it on their private
  network and do not publish its port. Point each relay at it with
  `AUTH_VTS_URL=http://<vts-host>:8081/verify`.
- The registered apps contain every signing secret, so keep the file in a
  secret store (for example GCP Secret Manager) and mount it into the
  container, or use a Cloud Storage bucket only the VTS and the console can
  read.
- Tokens signed with a replaced secret fail with `invalid_signature` from the
  next connection; sessions already established stay up until their `exp`.
