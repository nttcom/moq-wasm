"""An event loop whose clock moves only while the loop has nothing to run, so timing tests hold on a
slow machine."""

import asyncio
import selectors
import threading
import time


class _Selector(selectors.DefaultSelector):
    """Waits by moving the loop's clock to the next timer instead of sleeping, unless nothing is
    scheduled or an executor job is running, when only another thread can wake the loop."""

    def __init__(self, loop: "VirtualTimeLoop"):
        super().__init__()
        self._loop = loop

    def select(self, timeout: float | None = None):
        ready = super().select(0)
        if ready or (timeout is not None and timeout <= 0):
            return ready
        if timeout is None or self._loop.running_jobs > 0:
            return super().select(None)
        self._loop.advance(timeout)
        return []


class VirtualTimeLoop(asyncio.SelectorEventLoop):
    def __init__(self):
        self._now = 0.0
        self._epoch = time.time()
        # Executor jobs running on the real clock, which the loop's clock waits for; one sleeping in
        # `sleep_in_job` does not count. Changed on the loop's thread only.
        self.running_jobs = 0
        super().__init__(_Selector(self))

    def time(self) -> float:
        return self._now

    def unix_time(self) -> float:
        return self._epoch + self._now

    def advance(self, seconds: float) -> None:
        """Called from a callback, it simulates the loop stalling `seconds` inside it."""
        self._now += seconds

    def run_in_executor(self, executor, func, *args):
        self.running_jobs += 1
        future = super().run_in_executor(executor, func, *args)
        future.add_done_callback(self._job_ended)
        return future

    async def shutdown_default_executor(self, timeout: float | None = None) -> None:
        # Joins the executor's threads on a thread of its own, which the loop's clock waits for.
        self.running_jobs += 1
        try:
            await super().shutdown_default_executor(timeout)
        finally:
            self.running_jobs -= 1

    def sleep_in_job(self, seconds: float) -> None:
        """Called from an executor job, it blocks the job for `seconds` of the loop's clock, letting the
        clock advance meanwhile."""
        woken = threading.Event()

        def wake() -> None:
            self.running_jobs += 1
            woken.set()

        def sleep() -> None:
            self.running_jobs -= 1
            self.call_later(seconds, wake)

        self.call_soon_threadsafe(sleep)
        woken.wait()

    def _job_ended(self, _: asyncio.Future) -> None:
        self.running_jobs -= 1
