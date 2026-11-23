import asyncio
from collections.abc import AsyncIterator
from typing import TypeVar, cast

from moq_wasm._once import Once, wait_through_cancellation

T = TypeVar("T")


class Following(AsyncIterator[T]):
    """What `RemoteBroadcast` follows: its catalogs, a track's objects or its subscriptions; and the
    pictures `moq_wasm.codecs` decodes. Each read advances it in the reader's own task, awaiting
    its sources directly, and keeps its state between reads, so a read cancelled, as by
    `asyncio.wait_for` timing out, leaves the next read to go on where it stopped: what the SDK
    awaits, a `moq_wasm.Subscription`'s read or a SUBSCRIBE, stays in flight for it. One read at a
    time: a read while another waits raises RuntimeError at once.

    `aclose()` unsubscribes, waiting until the UNSUBSCRIBE was sent, after the relay answered a
    SUBSCRIBE still in flight, even when its caller is cancelled meanwhile, then raises the
    caller's cancellation; reads after it raise StopAsyncIteration. Called from another task while
    a read waits, it cancels that read, which raises StopAsyncIteration. Called from within a read,
    as from an `on_subscribed` callback, it returns at once: the read finishes the close and raises
    StopAsyncIteration instead of returning what it read. A task the read awaits must not await
    `aclose()`, which waits for the read. The iteration ending, or raising, unsubscribes too,
    before the read raises; a read cancelled meanwhile leaves that to the next read. Read it in
    `contextlib.aclosing` to unsubscribe as soon as the reading stops: one dropped without
    `aclose()` unsubscribes only once it is garbage collected."""

    # Whether `aclose()` from another task cancels the read in flight: false for an iteration whose
    # reads await a source of the caller's, which the cancellation would reach.
    _interrupts = True

    def __init__(self, what: str):
        self._what = what
        # The task of the read in flight.
        self._reader: asyncio.Task | None = None
        # The task of the read whose advance (`_next`) is in flight; `_idle` is clear meanwhile.
        self._advancing: asyncio.Task | None = None
        self._idle = asyncio.Event()
        self._idle.set()
        # Whether `aclose()` cancelled the advance in flight, which the read takes back.
        self._interrupting = False
        # What the read raises once the close the iteration's end started finished: the failure
        # that ended it, or StopAsyncIteration.
        self._ending: BaseException | None = None
        self._closing: Once[None] = Once()

    def __aiter__(self) -> "Following[T]":
        return self

    async def __anext__(self) -> T:
        if self._reader is not None:
            raise RuntimeError(f"{self._what} is already being read")
        reader = asyncio.current_task()
        assert reader is not None, "a read runs in a task"
        self._reader = reader
        try:
            if self._ending is None:
                if self._closing.started:
                    raise StopAsyncIteration
                item = await self._advance(reader)
                if self._ending is None:
                    return cast(T, item)
            # Ended, failed or closed during the read: the close goes first. A read cancelled
            # meanwhile, as by a timeout, leaves it going for the next read to wait for.
            closing = self._start_closing()
            await asyncio.wait([closing])
            ending, self._ending = self._ending, None
            if isinstance(ending, StopAsyncIteration):
                closing.result()
            raise ending
        finally:
            self._reader = None

    async def _advance(self, reader: asyncio.Task) -> T | None:
        """`_next()`; sets `_ending` when the iteration ended, failed or was closed meanwhile, and
        raises the reader's own cancellation, which leaves the state to the next read."""
        self._advancing = reader
        self._idle.clear()
        try:
            item = await self._next()
        except BaseException as error:
            interrupted = self._take_interruption(reader)
            if isinstance(error, asyncio.CancelledError) and not interrupted and reader.cancelling():
                raise
            stopped = interrupted or isinstance(error, StopAsyncIteration)
            self._ending = StopAsyncIteration() if stopped else error
            return None
        else:
            self._take_interruption(reader)
            if self._closing.started:
                self._ending = StopAsyncIteration()
            return item
        finally:
            self._advancing = None
            self._idle.set()

    async def aclose(self) -> None:
        closing = self._start_closing()
        if self._advancing is not None and self._advancing is asyncio.current_task():
            return
        await wait_through_cancellation([closing])
        closing.result()

    def _start_closing(self) -> asyncio.Future[None]:
        """Starts the close unless it started, cancelling an advance in flight in another task;
        returns a future of the caller's own that resolves once it finished."""
        advancing = self._advancing
        if not self._closing.started and self._interrupts and advancing is not None:
            if advancing is not asyncio.current_task():
                self._interrupting = True
                advancing.cancel()
        return self._closing.start(self._close)

    def _take_interruption(self, reader: asyncio.Task) -> bool:
        """Takes back the cancellation `aclose()` sent the read; whether the read was cancelled by
        it alone."""
        if not self._interrupting:
            return False
        self._interrupting = False
        return reader.uncancel() == 0

    async def _close(self) -> None:
        if self._interrupts:
            await self._idle.wait()
        await self._release()

    async def _next(self) -> T:
        """Advances the iteration; raises StopAsyncIteration at its end. Cancelled, it leaves its
        state consistent for the next call."""
        raise NotImplementedError

    async def _release(self) -> None:
        """Releases what the iteration owns, once no read is in flight."""
