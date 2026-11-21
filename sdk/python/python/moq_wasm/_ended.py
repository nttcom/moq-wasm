import asyncio
from collections.abc import Callable

from moq_wasm._wait import wait


class Ended:
    """How a handle, such as a track writer or a media source, ended for good, which every method of
    the handle consults: the first `end` sets it, a later one changes nothing, and the writes and
    waits of the handle observe it, those already waiting included."""

    def __init__(self) -> None:
        self._error: BaseException | None = None
        self._failure = False
        self._raised: Callable[[], None] | None = None
        self._event = asyncio.Event()

    @property
    def ended(self) -> bool:
        return self._error is not None

    @property
    def failed(self) -> bool:
        """Whether the handle ended by a failure, rather than by stopping, as a media source whose
        broadcast was left does, whose pushes are dropped instead of raising."""
        return self._failure

    def end(
        self, error: BaseException, *, failure: bool = True, raised: Callable[[], None] | None = None
    ) -> bool:
        """Ends the handle with `error`, unless it ended before; `raised` is called each time
        `error()` hands it to a caller. The same error ending it again sets `raised`, as a failure
        reported first by one path and then by its owner does. Returns whether this call ended it. A
        cancellation is the cancelled task's own and ends no handle, whose other callers would raise
        it and look cancelled."""
        assert not isinstance(error, asyncio.CancelledError), "a cancellation ends no handle"
        if self._error is None:
            self._error = error
            self._failure = failure
            self._raised = raised
            self._event.set()
            return True
        if error is self._error and self._raised is None:
            self._raised = raised
        return False

    def error(self) -> BaseException:
        """What ended the handle, for its caller to raise: a copy of its own each call, caused by
        it, as `copied` makes."""
        assert self._error is not None, "the handle has not ended"
        if self._raised is not None:
            self._raised()
        return copied(self._error)

    async def until(self, awaited: asyncio.Future) -> None:
        """Waits until `awaited` is done or the handle ended, whichever comes first. Neither the
        return nor the caller's cancellation cancels `awaited`."""
        ended = asyncio.ensure_future(self._event.wait())
        try:
            await wait([awaited, ended], return_when=asyncio.FIRST_COMPLETED)
        finally:
            ended.cancel()


def copied(error: BaseException) -> BaseException:
    """A copy of `error` to raise, of its type, with its arguments and attributes, such as a
    `RequestError`'s `code`, and `error` as its cause. Raising `error` itself again and again would
    add the raising frames to its traceback each time, which keeps them alive with it."""
    copy = type(error).__new__(type(error), *error.args)
    copy.__dict__.update(getattr(error, "__dict__", {}))
    copy.__cause__ = error
    return copy
