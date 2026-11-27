# http-server

Shared HTTP/1 accept loop, JSON body/response helpers, CORS, trusted-proxy client IP extraction, bearer token header parsing, and graceful shutdown signal helper.

> **Workspace crate**: Internal library shared by HTTP services (`vts`, `console`, `temporary-apps`, `observability`, `recorder`, `bot-dispatcher`, `sip-gateway`) and test stubs in `auth-token` / `google-cloud`. Routing and state stay in each service. Native-only.

## Usage

Inside an `async fn` returning `anyhow::Result<()>`:

```rust
use std::net::SocketAddr;
use http_server::{
    HttpServer, empty_response, error_response, healthz_response, read_json, shutdown_signal,
};
use hyper::{Method, Request, StatusCode, body::Incoming};
use tokio::net::TcpListener;

let listener = TcpListener::bind("0.0.0.0:8080").await?;
let _server = HttpServer::run(listener, |req: Request<Incoming>, _peer: SocketAddr| async move {
    match (req.method(), req.uri().path()) {
        (&Method::GET, "/healthz") => healthz_response(),
        (&Method::POST, "/items") => {
            let Some(_payload): Option<serde_json::Value> = read_json(req.into_body()).await else {
                return error_response(StatusCode::BAD_REQUEST, "invalid_body");
            };
            empty_response(StatusCode::NO_CONTENT)
        }
        _ => error_response(StatusCode::NOT_FOUND, "not_found"),
    }
});

shutdown_signal().await?;
```

## Public API

| Item | Role |
| --- | --- |
| `HttpServer::run(listener, handler)` | Spawns the HTTP/1 accept loop; stops accepting when dropped |
| `json_response`, `raw_json_response`, `healthz_response`, `error_response`, `empty_response` | Standard `JsonResponse` builders (`healthz_response` returns `200 {"status":"ok"}`) |
| `read_json<T>(body)`, `MAX_JSON_BODY_BYTES` | Reads and deserializes a JSON request body capped at 16 KiB |
| `bearer_token`, `query_params`, `client_ip`, `allow_any_origin` | Helpers for extracting `Authorization: Bearer <token>` (parsing only—verify separately), query strings, `X-Forwarded-For`, and CORS headers |
| `shutdown_signal()` | Resolves on `SIGINT` or (on Unix) `SIGTERM` |

Optional Cargo feature: `testing` exposes `testing::{StubServer, RecordedRequest}` for HTTP client tests (enable in `[dev-dependencies]` only).

## Behavior & caveats

- **Shutdown**: Dropping `HttpServer` aborts the accept task while in-flight connections finish. Pair with `shutdown_signal()` so container `SIGTERM` flushes telemetry cleanly.
- **JSON limits**: `read_json` caps bodies at 16 KiB (`MAX_JSON_BODY_BYTES`) and returns `None` on overflow, I/O error, or malformed JSON. `json_response` falls back to `null` if serialization fails.
- **Trusted proxy hops**: `client_ip(headers, peer, trusted_proxy_hops)` indexes `len - trusted_proxy_hops` in `X-Forwarded-For` and falls back to `peer` when `trusted_proxy_hops == 0`, too few entries exist, or the IP is invalid.
- **Header parsing**: `bearer_token` only extracts a non-empty UTF-8 token after the exact `"Bearer "` prefix—callers must verify it (e.g. via [`auth-token`](../auth-token/README.md)). `query_params` keeps the last value per key; `allow_any_origin` panics if `methods` or `headers` is not a valid static header value.

## Further reading

- Architecture: [docs/architecture/http-server/architecture.md](../../docs/architecture/http-server/architecture.md)
