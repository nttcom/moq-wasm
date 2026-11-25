# `http-server` Architecture

## Status
Living document. Update alongside any change to design intent, module boundaries, runtime flow, or invariants.

## Scope
Shared HTTP/1 server primitives used by every HTTP service (`console`, `vts`, `temporary-apps`, `observability`, `live-translate`, `recorder`, `bot-dispatcher`, `sip-gateway`). Services own routing and state; this crate handles connection acceptance, request parsing, and response helpers.

## API

### Server & Lifecycle
| Item | Responsibility |
| --- | --- |
| `HttpServer::run(listener, handler)` | Spawns the accept loop and owns its `JoinHandle`. Runs hyper HTTP/1 per connection on its own task, invoking `handler(request, peer)` per request. |
| `shutdown_signal()` | Waits for `SIGINT` or (on Unix) `SIGTERM` so services exit cleanly and flush telemetry. |
| `testing::StubServer` | Loopback test HTTP server recording requests (behind the `testing` feature). |

### Request Parsing
| Item | Responsibility |
| --- | --- |
| `read_json(body)` | Parses JSON up to `MAX_JSON_BODY_BYTES` (16 KiB); returns `None` on oversize, read error, or invalid JSON. |
| `bearer_token(headers)` | Extracts `<token>` from `Authorization: Bearer <token>`, or `None` if missing/empty. |
| `client_ip(headers, peer, trusted_proxy_hops)` | Reads the client IP from `X-Forwarded-For` (`trusted_proxy_hops` from the right across all header lines), falling back to `peer`. |
| `query_params(query)` | Decodes URI query pairs into a map (duplicate keys keep the last value). |

### Response Builders
| Item | Responsibility |
| --- | --- |
| `JsonResponse` | `Response<Full<Bytes>>` alias. |
| `json_response(status, body)` | Serializes `body` as `application/json`. |
| `raw_json_response(status, body)` | Wraps pre-serialized JSON bytes with `application/json`. |
| `healthz_response()` | `200 {"status":"ok"}`. |
| `error_response(status, error)` | `{"error": error}` as JSON. |
| `empty_response(status)` | Empty body with no extra headers. |
| `allow_any_origin(response, methods, headers)` | Adds `Access-Control-Allow-Origin: *` plus allowed methods/headers (used by `temporary-apps`, `live-translate`, `observability`). |

## Runtime Flow

```mermaid
sequenceDiagram
    participant L as Accept loop
    participant C as Connection task
    participant H as Service handler
    L->>L: listener.accept()
    L->>C: tokio::spawn per connection
    C->>H: handler(request, peer)
    H-->>C: Response<B>
```

## Invariants

- **Error logging**: Failed `accept` logs at `WARN` (`accept failed`) and continues; connection errors log at `DEBUG` with the peer address.
- **Infallible handlers**: `handler` always returns a response so each service controls error statuses.
- **Service-owned body types**: Services choose their body type (`Full<Bytes>` for JSON, boxed body for SSE in `live-translate`, `Either` for streamed replay in `recorder`). Services using non-`Full<Bytes>` bodies map the JSON helpers.
- **Drop stops accept only**: Dropping `HttpServer` aborts the accept loop; open connections are not tracked and run to completion.
- **Service-specific headers stay out**: Headers such as `Cache-Control: no-store` (`console`), allow-listed CORS (`recorder`), and `Access-Control-Max-Age` (`observability`) are set by each service.

## Dependencies
- `hyper`, `hyper-util`, `http-body-util`: [hyper.md](hyper.md)
- `url` (for `query_params`): [url.md](url.md)
