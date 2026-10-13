# tracing-subscriber for logs in moqt tests

## Status

Accepted

## Date

2026-10-13

## What

Add `tracing-subscriber` (with `env-filter`) as a dev-dependency of the `moqt`
crate. The test helper that starts a test server installs a `fmt` subscriber
that writes through the libtest capture (`with_test_writer`), filtered by
`RUST_LOG` and `warn` when it is unset.

## Context

The `moqt` tests that open real QUIC connections fail intermittently. Without a
subscriber every `tracing` event from the crate is dropped, so a failed run
shows only the panic and the cause cannot be read from the output. The capture
keeps passing tests silent: libtest prints a test's output only when it fails.
All `moqt` async tests use the current-thread `#[tokio::test]` runtime, so
spawned tasks log on the test thread and stay inside its capture.

## Alternatives

- `test-log`: a `#[test_log::test]` attribute that installs the same
  subscriber. Adds a crate and an attribute on every test for what one call in
  the shared test helper does.
- Print diagnostics with `eprintln!` in the failing tests. Only covers the
  tests someone thought to instrument and misses the library's own events.

## Decision

Use `tracing-subscriber`. It is already in the workspace lock (`relay`,
`live-ingest`, `observability` and others depend on it), so no new crate enters
the dependency tree, and as a dev-dependency it does not reach crates that
depend on `moqt`. `RUST_LOG=moqt=debug cargo test -p moqt` raises the level
for a run.
