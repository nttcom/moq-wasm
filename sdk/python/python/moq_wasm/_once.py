import asyncio
from collections.abc import Awaitable, Callable
from typing import Generic, TypeVar

T = TypeVar("T")


class Once(Generic[T]):
    """An operation the SDK owns, such as a close: the first `run` starts it, and it runs to its end
    even when the callers awaiting it are cancelled. Every call, a later one or one after a cancelled
    one, awaits that same run and raises what it raised."""

    def __init__(self) -> None:
        self._running: asyncio.Future[T] | None = None

    @property
    def started(self) -> bool:
        return self._running is not None

    async def run(self, operation: Callable[[], Awaitable[T]]) -> T:
        """Starts `operation()` unless it was started before, and awaits it."""
        return await self.start(operation)

    def start(self, operation: Callable[[], Awaitable[T]]) -> asyncio.Future[T]:
        """Starts `operation()` unless it was started before; returns a future of the caller's own
        that resolves as the run does, which cancelling leaves the run going."""
        if self._running is None:
            self._running = asyncio.ensure_future(operation())
            # Its failure goes to the callers, and to nobody once they were all cancelled.
            self._running.add_done_callback(retrieve)
        awaited = asyncio.shield(self._running)
        # A caller that only starts the run, as a fire-and-forget close, does not warn.
        awaited.add_done_callback(retrieve)
        return awaited


def in_thread(work: Callable[[], T]) -> asyncio.Future[T]:
    """Like `loop.run_in_executor(None, work)`, but once the future resolves no worker reaches
    `work` or its result: a worker can still hold the callable it ran and its future after the
    loop has resolved it, so both pass through lists emptied on the way."""
    loop = asyncio.get_running_loop()
    finished: asyncio.Future[T] = loop.create_future()
    works = [work]
    futures = [finished]

    def run() -> None:
        outcome: list = []
        try:
            outcome.append((True, works.pop()()))
        except BaseException as error:
            outcome.append((False, error))
        loop.call_soon_threadsafe(_settle, futures, outcome)

    loop.run_in_executor(None, run).add_done_callback(retrieve)
    return finished


def _settle(futures: list[asyncio.Future], outcome: list) -> None:
    future = futures.pop()
    succeeded, value = outcome.pop()
    if future.cancelled():
        return
    if succeeded:
        future.set_result(value)
    else:
        future.set_exception(value)


def retrieve(future: asyncio.Future) -> None:
    """Marks the failure of `future` retrieved, so one nobody awaits any more does not warn."""
    if not future.cancelled():
        future.exception()


async def wait_through_cancellation(tasks: list[asyncio.Future]) -> None:
    """Waits until every task finished without cancelling them, even when the caller is cancelled
    meanwhile; then raises the caller's cancellation."""
    cancelled = False
    pending = [task for task in tasks if not task.done()]
    while pending:
        try:
            await asyncio.wait(pending)
        except asyncio.CancelledError:
            cancelled = True
        pending = [task for task in pending if not task.done()]
    if cancelled:
        raise asyncio.CancelledError
