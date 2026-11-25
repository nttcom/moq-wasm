# `pytest-timeout` for the Python SDK tests (`python`)

**Status:** Accepted (2026-11-25)

## What

Add `pytest-timeout` to the `dev` dependency group of `sdk/python`, with `timeout = 120` and `timeout_func_only = true` in `[tool.pytest.ini_options]`. A test function still running after 120 s fails with the traceback of where it waits, and its xdist worker moves on to the next test.

## Context

Many binding tests await the native module without a bound of their own, such as `create_broadcast`'s `async with` exit or `Session.close()`. When one of them never resolves, the test holds its xdist worker forever: the `pytest (latest)` job once sat in "Run binding tests" for almost two hours, with no output after 86 %, until it was cancelled. A cancelled step uploads no logs, so the run showed neither which test hung nor where. The step's `timeout-minutes` bounds the job, but on its own it still names no test.

The slowest test function takes about 31 s. The first test of each worker spends about 32 s in its setup, building the relay and VTS with cargo and starting them, and much longer on a checkout that has not built them yet.

## Alternatives

- **`faulthandler_timeout` (built into pytest):** Dumps the thread stacks of a slow test but lets it run on. Under asyncio the main thread waits in the event loop's `select`, so the dump names neither the test nor the awaited coroutine.
- **`asyncio.timeout` in every test or an autouse fixture:** Covers only the coroutine the fixture wraps, and every new test must remember it.
- **Only `timeout-minutes` on the CI steps:** Bounds the job, but fails it as a whole, without the name of the hung test.

## Decision

Use `pytest-timeout`. Its default `signal` method raises in the main thread, which ends the event loop's wait and fails only the hung test. `timeout_func_only` leaves the setup out, so a cold cargo build in the first test's fixture does not fail it. The CI steps also get `timeout-minutes: 15`, so a hang in a fixture fails the step instead of running for hours, and the failure uploads the relay, VTS and Redis logs.

## Consequences

- **Positive:** A hung test fails within 120 s, with its name and the traceback of where it waited, and the other tests still run.
- **Negative:** A hang in a fixture's setup or teardown is bounded only by the step's 15 minutes.
- **Negative:** A test that legitimately needs more than 120 s must raise its limit with `@pytest.mark.timeout`.
