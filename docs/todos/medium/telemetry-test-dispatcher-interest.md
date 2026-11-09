# [S] Share the second-dispatcher guard across span-capturing tests

Next: move `SECOND_DISPATCHER` from `services/vts/src/test_support.rs:70` into a shared test helper and force it before every `tracing::subscriber::set_default` in tests.

## Goal

Span-capturing tests (`crates/moqt/src/modules/test_support.rs:780`, `crates/telemetry/src/trace_context.rs:102`, `crates/wasm/src/session_routes.rs:507`, the `set_default` tests in `services/relay`) no longer flake.
While one dispatcher exists, `tracing-core` caches a callsite's interest from the thread that hits it first, so a concurrent test without a subscriber caches `never`.

## Plan

Open question: where the helper lives (`telemetry` behind a `testing` feature is a candidate; `moqt` does not depend on `telemetry`).
