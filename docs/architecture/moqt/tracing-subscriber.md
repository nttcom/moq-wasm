# `tracing-subscriber` for Test Diagnostics (`moqt`)

**Status:** Accepted (2026-10-13)

## What

Add `tracing-subscriber` (`env-filter`) as a dev-dependency of `moqt`. The shared test-server helper installs a `fmt` subscriber writing through libtest capture (`with_test_writer`), filtered by `RUST_LOG` (defaulting to `warn`).

## Context

`moqt` tests that open real QUIC connections occasionally fail intermittently. Without a subscriber, all `tracing` events are dropped and failures show only the panic. Using `with_test_writer` keeps passing tests silent while printing logs on failure; because most `moqt` async tests use current-thread `#[tokio::test]`, spawned tasks stay inside the test thread's capture.

## Alternatives

- **`test-log`:** Adds a crate and requires annotating every test with `#[test_log::test]` instead of one call in the shared helper.
- **`eprintln!` in failing tests:** Only instruments manually edited tests and misses library-level events.

## Decision

Use `tracing-subscriber`. It is already in the workspace lock (`relay`, `telemetry-otlp`, `vts`, and others) and, as a dev-dependency, never propagates to downstream crates. Raise verbosity per run with `RUST_LOG=moqt=debug cargo test -p moqt`.
