# VTS (Verify Token Service)

Verifies MoQT JWTs on authenticated `CLIENT_SETUP` and on each in-session token refresh, returning their claims to the relay while keeping signing secrets off the relay. Design: [architecture.md](../../docs/architecture/vts/architecture.md).

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

VTS only checks whether the token is a valid HS256 JWT signed by a registered app (and whether that app is a relay); claim semantics (`exp`, max TTL, namespace permissions) are enforced by the relay.

`error` values (in check order): `malformed_token`, `unknown_app`, `invalid_signature` (including non-HS256 algorithms).

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `VTS_APPS_FILE` | `/etc/vts/apps.json` | Local app registry JSON file |
| `VTS_REGISTRY_BUCKET` | unset | Cloud Storage bucket for the app registry |
| `VTS_REGISTRY_OBJECT` | `registry.json` | Object name in `VTS_REGISTRY_BUCKET` |
| `STORAGE_EMULATOR_HOST` | unset | Cloud Storage emulator URL (e.g. `http://localhost:4443`) |
| `VTS_TEMPORARY_APP_SECRET` | unset | Signing secret for `temp-*` apps (unset disables them) |
| `VTS_PORT` | `8081` | Listen port |
| `VTS_STDOUT_FILTER`, `VTS_OTEL_FILTER`, `VTS_LOG_FILTER` | `vts=info` | `tracing` filters for stdout, OTLP traces, and OTLP logs |

OTLP export uses standard `OTEL_*` variables ([telemetry architecture](../../docs/architecture/telemetry/architecture.md)).

Set either `VTS_APPS_FILE` or `VTS_REGISTRY_BUCKET` (not both). VTS reloads the registry on startup and on every `/verify`, so app changes apply without restart.

Registry format (`app-registry`; see `apps.example.json` for local dev — never commit real secrets):

```json
[
  { "appId": "ac8adbc8-...", "secret": "<random>", "isRelay": false },
  { "appId": "11111111-...", "secret": "<random>", "isRelay": true }
]
```

Sign HS256 tokens with the UTF-8 bytes of `secret` using [`auth-token`](../../crates/auth-token/README.md) (generate secrets with e.g. `openssl rand -base64 32`). Temporary apps (`temp-*`) bypass the registry, verify against `VTS_TEMPORARY_APP_SECRET`, and are never relays.

## Running

```shell
VTS_APPS_FILE=./services/vts/apps.example.json cargo run -p vts
```

`docker compose up -d` runs VTS with `apps.example.json` and connects both relays using dev `AUTH_RELAY_TOKEN` values minted from the relay entry:

```shell
AUTH_TOKEN_SECRET="$(jq -r '.[] | select(.appId == "11111111-2222-3333-4444-555555555555") | .secret' services/vts/apps.example.json)" \
  cargo run -p auth-token --features cli -- --app-id 11111111-2222-3333-4444-555555555555 --publish "" --subscribe "" --ttl 3650d --relay-id relay-a
```

To use your own apps in Compose, override `VTS_APPS_FILE` and mint a distinct token per relay (`--relay-id relay-a` and `--relay-id relay-b`) via a Compose override rather than a single `AUTH_RELAY_TOKEN` in `.env` (which would apply the same token to both relays).

## Deployment

- Keep VTS on a private network reachable only by relays (`AUTH_VTS_URL=http://<vts-host>:8081/verify`) and services that verify tokens (such as `bot-dispatcher` and `recorder serve`).
- Store the registry in a secret store (e.g. GCP Secret Manager) or a GCS bucket readable only by VTS, console, and (when deployed) `bot-dispatcher` (read-only).
- Tokens signed with a replaced secret fail with `invalid_signature` on the next connection; active sessions remain up until their `exp`.
