"""An event loop whose clock moves only while the loop has nothing to run, so timing tests hold on a
slow machine."""

import asyncio
import selectors
import time


class _Selector(selectors.DefaultSelector):
    """Waits by moving the loop's clock to the next timer instead of sleeping, unless nothing is
    scheduled, when only another thread can wake the loop."""

    def __init__(self, loop: "VirtualTimeLoop"):
        super().__init__()
        self._loop = loop

    def select(self, timeout: float | None = None):
        ready = super().select(0)
        if ready or (timeout is not None and timeout <= 0):
            return ready
        if timeout is None:
            return super().select(None)
        self._loop.advance(timeout)
        return []


class VirtualTimeLoop(asyncio.SelectorEventLoop):
    def __init__(self):
        self._now = 0.0
        self._epoch = time.time()
        super().__init__(_Selector(self))

    def time(self) -> float:
        return self._now

    def unix_time(self) -> float:
        return self._epoch + self._now

    def advance(self, seconds: float) -> None:
        """Called from a callback, it simulates the loop stalling `seconds` inside it."""
        self._now += seconds
