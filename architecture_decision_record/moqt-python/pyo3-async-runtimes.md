# ADR: pyo3-async-runtimes for asyncio integration

## Status
Accepted

## Date
2026-09-07

## Context
Every operation in `moqt` is an `async fn` on tokio (connect, SUBSCRIBE,
reading and writing objects, receiving session events). The target Python
consumers (pipecat-style media pipelines) are asyncio applications that hold
many sessions concurrently, so the binding should expose awaitables rather
than blocking calls.

## Decision
Use **pyo3-async-runtimes** with the `tokio-runtime` feature.
`future_into_py` converts each Rust future into an `asyncio.Future`; Rust
futures run on the crate's tokio runtime and the GIL is only taken to build
the result object.

## Consequences

### Positive
- `await session.subscribe(...)`, `async for obj in reader`, and
  `async for event in session` map one-to-one onto the Rust API.
- Cancelling the Python awaitable drops the Rust future, so a cancelled read
  releases the reader lock.

### Negative
- Rust futures must be `Send + 'static`; wrappers hold their state in
  `Arc<tokio::sync::Mutex<_>>`.
- A second dependency tracking pyo3's release cadence.

### Neutral
- The default multi-thread tokio runtime is used; no runtime configuration is
  exposed to Python.

## Alternatives Considered

### pyo3 `experimental-async`
Lets `#[pymethods]` be `async fn` directly, but the feature is still marked
experimental and does not integrate with a tokio runtime on its own.

### Blocking API on a private tokio runtime
Simplest to implement (`Runtime::block_on` behind `Python::detach`), but forces
one OS thread per concurrent read and does not compose with asyncio servers.

## References
- https://github.com/PyO3/pyo3-async-runtimes
