# anon-issuer

Public endpoint that hands out short-lived tokens for the `anon` app so that
deployed demo clients can connect to an authenticated relay. The tokens grant
`publish` and `subscribe` on everything under the `anon` appId and nothing
else, so exposing the endpoint leaks nothing beyond the demo namespace.

This service is deliberately separate from the VTS: the VTS only verifies and
stays inside the private network, this service only issues and sits behind the
public load balancer. They share the `anon` secret and nothing else.

## API

```
POST /anon-token            -> 200 { "token": "eyJ...", "expiresAt": 1759678000 }
OPTIONS /anon-token         -> 204 (CORS preflight)
GET /healthz                -> 200 { "status": "ok" }
```

Errors: `403 origin_not_allowed` when the `Origin` header is present and not
listed, `429 rate_limited` when a client exceeds its per-minute budget.

## Configuration

| Variable                     | Default  | Meaning                                                           |
| ---------------------------- | -------- | ----------------------------------------------------------------- |
| `ANON_SECRET`                | required | Same value as the `anon` row of the VTS `apps.json`               |
| `ANON_APP_ID`                | `anon`   | `appId` claim of the issued tokens                                |
| `ANON_TOKEN_TTL_SECONDS`     | `43200`  | Token lifetime; must not exceed 86400                             |
| `ANON_ISSUER_PORT`           | `8080`   | Listen port                                                       |
| `ANON_ALLOWED_ORIGINS`       | required | Comma-separated browser origins allowed by CORS (`*` is rejected) |
| `ANON_RATE_LIMIT_PER_MINUTE` | `30`     | Tokens per client address per minute                              |
| `ANON_TRUST_PROXY`           | `false`  | Use the first `X-Forwarded-For` entry as the client address       |

## Running

```
npm ci
ANON_SECRET=... ANON_ALLOWED_ORIGINS=http://localhost:5173 npm start
npm test
```

The Dockerfile expects the repository root as build context:
`docker build -f services/anon-issuer/Dockerfile .`

## Deployment

This is the only public part of the authentication setup: place it behind the
public load balancer, with `ANON_TRUST_PROXY=true` so the rate limit keys on
the client address forwarded by the balancer, and list the deployed example
origins (for example the GitHub Pages origin) in `ANON_ALLOWED_ORIGINS`.

`ANON_SECRET` must equal the `secret` of the `anon` row in the VTS
`apps.json`. Source both from the same secret store entry so a rotation
changes them together; after rotating, restart the issuer and the VTS.
