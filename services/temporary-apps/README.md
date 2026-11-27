# temporary-apps

Issues temporary-app tokens (`appId` starting with `temp-`) to anyone without an account, giving shared demo links their own namespace root instead of `anon`. Design: [architecture.md](../../docs/architecture/temporary-apps/architecture.md).

## API

```
POST /tokens
201 Created
{ "token": "eyJhbGci...", "appId": "temp-3f9a6c...", "expiresAt": 1759603600 }
429 Too Many Requests   { "error": "rate_limited" }

POST /tokens/refresh
Content-Type: application/json
{ "token": "eyJhbGci..." }
200 OK                  (same body as POST /tokens, same appId)
401 Unauthorized        { "error": "expired_token" | "invalid_token" }

GET /healthz -> 200 { "status": "ok" }
```

- Tokens last 1 hour with full publish/subscribe access under their `appId`.
- Refreshing requires an unexpired token issued by this service, keeping an app alive while refreshed.
- Rate limits per client IP: 30 `POST /tokens` per hour; 10 `POST /bots/{type}` per hour.
- `/bots/{type}` proxies bot requests to `bot-dispatcher` for the caller's temporary app (`stay_when_empty: true` is rejected with `403 forbidden` and not forwarded; `false` is forwarded).

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `TEMPORARY_APPS_SECRET` | — | Token signing secret (must match VTS `VTS_TEMPORARY_APP_SECRET`) |
| `TEMPORARY_APPS_TRUSTED_PROXY_HOPS` | `0` | Number of trusted proxies appending to `X-Forwarded-For` (`0` = direct peer IP) |
| `TEMPORARY_APPS_BOT_DISPATCHER_URL` | — | `bot-dispatcher` base URL for `/bots/{type}` (unset returns `404`) |
| `TEMPORARY_APPS_BOT_TYPES` | — | Comma-separated bot types allowed for temporary apps (e.g. `recorder`) |
| `PORT` | `8083` | Listen port |

Logging/OTLP use `TEMPORARY_APPS_STDOUT_FILTER` and standard `OTEL_*` variables (`crates/telemetry-otlp`).

## Running

```shell
TEMPORARY_APPS_SECRET=development-temporary-app-secret cargo run -p temporary-apps
```

`docker compose up -d vts temporary-apps` starts it on port 8083 sharing the dev secret with VTS (for the full two-relay stack, run `make relay-certs && docker compose build relay-common && docker compose up -d`).
