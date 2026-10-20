# pyo3-async-runtimes for asyncio awaitables

## Status

Accepted

## Date

2026-10-20

## What

`crates/python` depends on `pyo3-async-runtimes` with `tokio-runtime`. Every
async method of the module returns an asyncio awaitable made by
`pyo3_async_runtimes::tokio::future_into_py`, which runs the Rust future on a
tokio runtime the crate shares across calls.

## Context

`moqt` is async on tokio: a session's receive loops are tokio tasks and quinn
binds its sockets through the ambient runtime. The bots are asyncio programs,
so each call has to start a Rust future and hand Python something it can
`await` without blocking the event loop.

## Alternatives

- **Block the calling thread** (`Runtime::block_on` under
  `Python::allow_threads`). Each call would stall the bot's event loop, which
  also runs its audio pipeline.
- **Write the bridge by hand**: spawn the future on a private runtime and
  resolve an `asyncio.Future` through `loop.call_soon_threadsafe`. That is what
  `pyo3-async-runtimes` implements, with cancellation and the event-loop
  lookup handled.

## Decision

Use `pyo3-async-runtimes`, maintained alongside PyO3 and versioned with it.
Its awaitables are `asyncio.Future`s rather than coroutines, so a caller wraps
one with `asyncio.ensure_future`, not `asyncio.create_task`, to run it in the
background.
