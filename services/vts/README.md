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

| Variable        | Default              | Meaning                     |
| --------------- | -------------------- | --------------------------- |
| `VTS_APPS_FILE` | `/etc/vts/apps.json` | Registered apps (see below) |
| `VTS_PORT`      | `8081`               | Listen port                 |

`apps.json` is a JSON array; `apps.example.json` holds local development
values. Never commit a real file: `services/*/apps.json` is ignored.

```json
[
  { "appId": "ac8adbc8-...", "secret": "<random>", "isRelay": false },
  { "appId": "11111111-...", "secret": "<random>", "isRelay": true }
]
```

Tokens are HS256 JWTs signed with the app's `secret` (UTF-8 bytes). Generate
secrets with e.g. `openssl rand -base64 32`.

## Minting tokens

`bin/mint.mjs` signs a token for any registered app. It is an operator tool
and is not exposed over HTTP.

```
node bin/mint.mjs --apps ./apps.json --app-id ac8adbc8-... --publish site1/cam1 --subscribe site1 --ttl 12h
node bin/mint.mjs --apps ./apps.json --app-id 11111111-... --publish "" --subscribe "" --ttl 8760h
```

## Running

```
npm ci
VTS_APPS_FILE=./apps.example.json npm start
npm test
```

The Dockerfile expects the repository root as build context:
`docker build -f services/vts/Dockerfile .`

## Local stack

`docker compose up -d` starts the VTS with `apps.example.json` and points
both relays at it. The relays authenticate to each other with a development
relay token that is the compose default for `AUTH_RELAY_TOKEN`; it was minted
from the relay row of `apps.example.json` and is valid for ten years:

```
node bin/mint.mjs --apps ./apps.example.json --app-id 11111111-2222-3333-4444-555555555555 \
  --publish "" --subscribe "" --ttl 3650d
```

Re-run that command and replace the default in `docker-compose.yml` if the
development relay secret changes. To run the stack against your own ledger,
set `VTS_APPS_FILE` and `AUTH_RELAY_TOKEN` (minted from that ledger) in `.env`.

`vts` is reachable only inside the compose network.

## Deployment

The VTS must be reachable only from the relays: put it on the relays' private
network and do not publish its port. The relay is pointed at it with
`AUTH_VTS_URL=http://<vts-host>:8081/verify` and authenticates its own
inter-relay connections with a token minted for the relay row:

```
node bin/mint.mjs --apps apps.json --app-id <relay-app-id> --publish "" --subscribe "" --ttl 8760h
```

### Where `apps.json` lives

`apps.json` contains every signing secret, so it is stored in a secret store
(GCP Secret Manager, AWS Secrets Manager, HashiCorp Vault, or the equivalent)
and mounted into the container as a file. The VTS code only reads
`VTS_APPS_FILE`, so local compose (bind mount of `apps.example.json`) and
production (a file fetched from the secret store at VM start) run the same
code. On GCP with Secret Manager:

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
self-service or frequent, only `src/apps.mjs` (the lookup) has to change; the
`/verify` contract and the relay stay as they are.
