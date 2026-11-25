# `pyo3-async-runtimes` for `asyncio` Awaitables (`python`)

**Status:** Accepted (2026-10-20)

## What

Add `pyo3-async-runtimes` (`tokio-runtime`) to `sdk/python`. Every async method returns an `asyncio` awaitable created via `pyo3_async_runtimes::tokio::future_into_py`, executing the Rust future on a shared Tokio runtime.

## Context

`moqt` runs on Tokio (receive loops are Tokio tasks and `quinn` binds sockets on the ambient runtime), while Python bots run on `asyncio`. Async calls must drive Rust futures and return awaitables without blocking the Python event loop.

## Alternatives

- **Block the calling thread (`Runtime::block_on` inside `Python::allow_threads`):** Stalls the bot's event loop, including its audio pipeline.
- **Custom bridge via private runtime + `loop.call_soon_threadsafe`:** Reimplements what `pyo3-async-runtimes` already handles, including cancellation and event-loop lookup.

## Decision

Use `pyo3-async-runtimes`, which is maintained and versioned alongside PyO3. Because it returns `asyncio.Future` objects rather than coroutines, callers scheduling them in the background must use `asyncio.ensure_future`, not `asyncio.create_task`.
