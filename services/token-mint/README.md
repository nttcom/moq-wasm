# token-mint

Issues the tokens of temporary apps to anyone, without an account. A
temporary app (`appId` starting with `temp-`) is a namespace root of its own:
the examples use one so that only the people a page's link is shared with
join its room or see its recordings, instead of the public `anon` root.

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

Every token is valid for one hour and may publish and subscribe anywhere
under its app (`publish` and `subscribe` are `""`). A refresh needs a token
this service signed that has not expired yet, and returns a new hour for the
same app; refreshes are not limited, so an app lives as long as someone keeps
refreshing it. `POST /tokens` is limited to 30 new apps per client address per
hour. Every response allows any origin.

## Configuration

| Variable                          | Default | Meaning                                                                                       |
| --------------------------------- | ------- | --------------------------------------------------------------------------------------------- |
| `TOKEN_MINT_SECRET`             | —       | Secret the tokens are signed with; the VTS's `VTS_TEMPORARY_APP_SECRET` must be the same value |
| `TOKEN_MINT_TRUSTED_PROXY_HOPS` | `0`     | Proxies in front that append to `X-Forwarded-For`; `0` takes the connection's peer address     |
| `PORT`                            | `8083`  | Listen port                                                                                   |

Logging and OTLP export follow `crates/telemetry` (`TOKEN_MINT_STDOUT_FILTER`
and the standard `OTEL_*` variables).

## Running

```
TOKEN_MINT_SECRET=development-temporary-app-secret cargo run -p token-mint
cargo test -p token-mint
```

`docker compose up -d` starts it on port 8083 with the VTS sharing the same
development secret. The Dockerfile expects the repository root as build
context: `docker build -f services/token-mint/Dockerfile .`

## Design

[Architecture document](../../docs/architecture/token-mint/architecture.md).
