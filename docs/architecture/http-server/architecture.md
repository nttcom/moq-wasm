# `http-server` Architecture

## Status

Living document. Update this file in the same change whenever the design intent,
module boundaries, runtime flow, or invariants described here change.

## Scope

`http-server` is the HTTP/1 server part every HTTP service shares: `console`,
`vts`, `temporary-apps`, `observability`, `live-translate`, `recorder`,
`bot-dispatcher` and `sip-gateway`. Each
service keeps its own routing and state; this crate accepts connections,
reads JSON bodies, bearer tokens and query strings, and builds responses.

| Item | Responsibility |
| --- | --- |
| `HttpServer::run(listener, handler)` | Spawns the accept loop and owns its `JoinHandle`. Each connection runs hyper HTTP/1 on its own task and calls `handler(request, peer)` for every request. |
| `JsonResponse` | `Response<Full<Bytes>>` alias for JSON services. |
| `json_response(status, body)` | `body` serialized as JSON with `Content-Type: application/json`. |
| `raw_json_response(status, body)` | Pre-serialized JSON bytes with `Content-Type: application/json`. |
| `healthz_response()` | `200 {"status":"ok"}` through `json_response`. |
| `error_response(status, error)` | `{"error": error}` through `json_response`. |
| `empty_response(status)` | `status` with an empty body and no headers. |
| `allow_any_origin(response, methods, headers)` | `response` with `Access-Control-Allow-Origin: *` and the given allowed methods and request headers, for the services whose pages are served from any origin (`temporary-apps`, `live-translate`, `observability`). |
| `bearer_token(headers)` | The token of `Authorization: Bearer <token>`, or `None` without one or with an empty token. |
| `client_ip(headers, peer, trusted_proxy_hops)` | Extracts the client IP from `X-Forwarded-For` across all header lines `trusted_proxy_hops` from the end, falling back to `peer`. |
| `shutdown_signal()` | Waits for `SIGINT` or (on Unix) `SIGTERM`, so containerized services exit cleanly and flush telemetry. |
| `query_params(query)` | The decoded pairs of a URI's query as a map; of a name given twice, the last value. |
| `read_json(body)` | The body parsed as JSON, or `None` when it is longer than `MAX_JSON_BODY_BYTES` (16 KiB), fails to read or does not parse. The service picks its own error response. |
| `testing::StubServer` | Loopback test HTTP server recording received requests; behind the `testing` feature. |

## Runtime flow

```mermaid
sequenceDiagram
    participant L as accept loop
    participant C as connection task
    participant H as service handler
    L->>L: listener.accept()
    L->>C: tokio::spawn per connection
    C->>H: handler(request, peer)
    H-->>C: Response<B>
```

## Invariants

- A failed `accept` is logged at WARN (`accept failed`) and the loop goes on;
  a connection that ends with an error is logged at DEBUG with the peer.
- The handler cannot fail: it returns a response for every request, so the
  service decides the status of each failure.
- The response body type is the service's: `Full<Bytes>` for JSON, a boxed
  body for server-sent events (`live-translate`), `Either` for a streamed
  replay (`recorder`). The JSON helpers return `Full<Bytes>`; a service with
  another body type maps it.
- Dropping `HttpServer` aborts the accept loop. Open connections are not
  tracked and run to their end.
- Headers that only some services send (`Cache-Control: no-store` in
  `console`, the allowed-origin CORS of `recorder`, `Access-Control-Max-Age`
  in `observability`) are added by the service, not here.

## Dependencies

`hyper`, `hyper-util` and `http-body-util`: see [hyper.md](hyper.md).
`url` for `query_params`: see [url.md](url.md).
