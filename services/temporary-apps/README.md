# temporary-apps

Issues the tokens of temporary apps to anyone, without an account. A
temporary app (`appId` starting with `temp-`) is a namespace root of its own:
the examples use one so that only the people a page's link is shared with
join its room, instead of the public `anon` root.
Design: [architecture document](../../docs/architecture/temporary-apps/architecture.md).

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

- Every token is valid for one hour and may publish and subscribe anywhere
  under its app.
- A refresh needs an unexpired token this service signed, so an app lives as
  long as someone keeps refreshing it.
- `POST /tokens` is limited to 30 new apps per client address per hour.
- `/bots/{type}` passes a page's call on to `bots` as the app server of its
  temporary app; `POST` is limited to 10 calls per client address per hour.

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `TEMPORARY_APPS_SECRET` | — | Secret the tokens are signed with; the VTS's `VTS_TEMPORARY_APP_SECRET` must be the same value |
| `TEMPORARY_APPS_TRUSTED_PROXY_HOPS` | `0` | Proxies in front that append to `X-Forwarded-For`; `0` takes the connection's peer address |
| `TEMPORARY_APPS_BOTS_URL` | — | Base URL of `bots` the pages reach through `/bots/{type}`; without it those routes are `404` |
| `TEMPORARY_APPS_BOT_TYPES` | — | Comma-separated bot types a temporary app may start, e.g. `recorder` |
| `PORT` | `8083` | Listen port |

Logging and OTLP export follow `crates/telemetry-otlp` (`TEMPORARY_APPS_STDOUT_FILTER`
and the standard `OTEL_*` variables).

## Running

```
TEMPORARY_APPS_SECRET=development-temporary-app-secret cargo run -p temporary-apps
```

`docker compose up -d` starts it on port 8083 with the VTS sharing the same
development secret.
