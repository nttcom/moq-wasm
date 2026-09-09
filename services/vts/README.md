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
{ "appId": "...", "publish": "site1", "subscribe": null, "isRelay": false, "exp": 1759678000 }

401 Unauthorized
{ "error": "invalid_signature" }

GET /healthz -> 200 { "status": "ok" }
```

`error` values, in the order they are checked: `malformed_token`,
`unknown_app`, `invalid_signature`, `expired`, `not_yet_valid`,
`ttl_too_long` (client tokens longer than 24 h), `invalid_claims`
(`publish` / `subscribe` not a string, or a path with an empty element).

## Configuration

| Variable                    | Default              | Meaning                                      |
| --------------------------- | -------------------- | -------------------------------------------- |
| `VTS_APPS_FILE`             | `/etc/vts/apps.json` | Registered apps (see below)                  |
| `VTS_PORT`                  | `8081`               | Listen port                                  |
| `VTS_MAX_TOKEN_TTL_SECONDS` | `86400`              | Upper bound of `exp - iat` for client tokens |
| `VTS_CLOCK_LEEWAY_SECONDS`  | `60`                 | Tolerance applied to `exp` and `iat`         |

`apps.json` is a JSON array; `apps.example.json` holds local development
values. Never commit a real file: `services/*/apps.json` is ignored.

```json
[
  { "appId": "ac8adbc8-...", "secret": "<random>", "isRelay": false },
  { "appId": "anon", "secret": "<random>", "isRelay": false },
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
