# [M] Share the hyper accept loop and JSON responses across the HTTP services

Next: compare the accept loops at `services/vts/src/server.rs:52` and `services/token-mint/src/server.rs:73`, which differ only in passing the peer address.

## Goal

One `serve(listener, handler)` and one `json_response` / `error_response` serve `vts`, `token-mint`, `console`, `observability` and `recorder`; each now copies the accept → spawn → `http1::Builder::serve_connection` loop and both helpers.

## Plan

No shared crate holds HTTP code yet: pick one (a new crate, or `telemetry`, which already depends on `http`). `console` adds a `no-store` header and `recorder` returns an `Either` body, so the helper takes headers and the body type.
