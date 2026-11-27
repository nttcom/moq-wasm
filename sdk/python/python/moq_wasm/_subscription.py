import asyncio
import weakref
from collections.abc import AsyncIterator
from typing import TYPE_CHECKING, Generic, TypeVar

from moq_wasm import _native
from moq_wasm._once import Once, retrieve
from moq_wasm._wait import wait

if TYPE_CHECKING:
    from moq_wasm._session import Session

T = TypeVar("T")

class _Reads(Generic[T]):
    """The reads of a native iterator, one at a time. A native read stays with the iterator until a
    caller takes its result, so a read cancelled after the native read took an item, as by a
    timeout, leaves that item to the next read instead of losing it, while the session is open.
    Once the session ended, a read raises what ended it, a waiting one included, and the native
    read left, which the session's end cancels, is dropped, even when it took an item: the native
    iterator would wait for the native session to give its connection up. The session does not
    keep the iterator: one nobody holds any more is collected, and its native read left cancelled
    with it."""

    def __init__(self, native: AsyncIterator[T], what: str, session: "Session"):
        self._native = native
        self._what = what
        self._session = session
        self._pending: asyncio.Future[T] | None = None
        # Cancels `_pending` once the iterator is collected.
        self._cancel_when_collected: weakref.finalize | None = None
        self._reading = False

    async def next(self) -> T:
        if self._reading:
            raise RuntimeError(f"{self._what} is already being read")
        self._reading = True
        try:
            if self._pending is None:
                self._session._raise_if_ended()
                self._pending = self._read()
            pending = self._pending
            await wait([pending, self._session._done], return_when=asyncio.FIRST_COMPLETED)
            self._let_go()
            if self._session._done.done():
                pending.cancel()
                self._session._raise_if_ended()
            return pending.result()
        finally:
            self._reading = False

    def _read(self) -> asyncio.Future[T]:
        """A native read, which the session's end, or the iterator's collection, cancels and drops
        until a read took it."""
        read = asyncio.ensure_future(anext(self._native))
        # A read nobody takes any more, as the end of an abandoned iterator, does not warn.
        read.add_done_callback(retrieve)
        self._session._on_end_while_held(self, _Reads._drop)
        self._cancel_when_collected = weakref.finalize(self, _cancel, read)
        self._cancel_when_collected.atexit = False
        return read

    def _drop(self, _: BaseException) -> None:
        """Cancels the native read left and lets go of it, with the item it took, if any."""
        if self._pending is not None:
            self._pending.cancel()
            self._let_go()

    def _let_go(self) -> None:
        """Lets go of the native read left, which the session's end and the iterator's collection
        no longer cancel."""
        self._pending = None
        self._session._forget_end_while_held(self)
        if self._cancel_when_collected is not None:
            self._cancel_when_collected.detach()
            self._cancel_when_collected = None


def _cancel(read: asyncio.Future) -> None:
    """Cancels the native read of a collected iterator, from the loop's thread: the collection may
    run in another one."""
    try:
        read.get_loop().call_soon_threadsafe(read.cancel)
    except RuntimeError:
        # Collected after its event loop closed, as at exit: nothing runs any more.
        pass


class Subscription(AsyncIterator[_native.Object]):
    """A subscription (`_native.Subscription`), iterated for its objects until the publisher ends
    it. One read at a time: a read while another waits raises RuntimeError. A read cancelled once
    an object arrived leaves the object to the next read, unless the session ended. It is read
    either by object or by group (`groups()`), whichever is read first; reading the other raises
    RuntimeError at once. One that starts with a joining FETCH is read by object only. Its reads,
    and those of its groups, raise what ended the session once it ended, such as
    `SessionClosed`, a waiting one included."""

    def __init__(self, native: _native.Subscription, session: "Session", *, read_by: str | None = None):
        """`read_by` is the unit the native subscription is already read by, as "object" for one
        that merges a joining FETCH into its objects."""
        self._native = native
        self._session = session
        self._objects = _Reads(native, "the subscription", session)
        self._groups: Groups | None = None
        self._with_end_of_group: ObjectsWithEndOfGroup | None = None
        # "object" or "group", as the first read or the native subscription made it.
        self._read_by = read_by
        # Whether the objects are read with their End of Group status objects, as the first read
        # made it.
        self._reads_end_of_group: bool | None = None
        self._closing: Once[None] = Once()

    def __aiter__(self) -> "Subscription":
        return self

    async def __anext__(self) -> _native.Object:
        """Raises StopAsyncIteration once the publisher ends the track. A group whose stream fails
        (reset by a publisher that left, or cut by a GOAWAY's cutover) ends early."""
        self._read_objects(with_end_of_group=False)
        return await self._objects.next()

    def _read_objects(self, *, with_end_of_group: bool) -> None:
        """`_read_as("object")`, which also raises RuntimeError once the objects were read with
        End of Group status objects or without, the other way: each way holds its own read."""
        self._read_as("object")
        if self._reads_end_of_group is None:
            self._reads_end_of_group = with_end_of_group
        elif self._reads_end_of_group != with_end_of_group:
            raise RuntimeError("the subscription is read " + ("with" if self._reads_end_of_group else "without") + " End of Group")

    def _read_as(self, unit: str) -> None:
        """Raises RuntimeError once the subscription was read by another unit, before the native
        read, which would raise it only once something arrived."""
        if self._read_by is None:
            self._read_by = unit
        elif self._read_by != unit:
            raise RuntimeError(f"the subscription is read by {self._read_by}")

    @property
    def largest(self) -> _native.Location | None:
        """The largest location SUBSCRIBE_OK reported."""
        return self._native.largest

    def done(self) -> asyncio.Future[_native.PublishDone | None]:
        """`_native.Subscription.done`."""
        return self._native.done()

    def with_end_of_group(self) -> "ObjectsWithEndOfGroup":
        """The objects with each End of Group status object among them
        (`_native.Subscription.with_end_of_group`), read as the objects are, instead of iterating
        the subscription itself. Every call returns the same iterator, so an object a cancelled
        read took goes to the next read through it."""
        if self._with_end_of_group is None:
            self._with_end_of_group = ObjectsWithEndOfGroup(self._native.with_end_of_group(), self)
        return self._with_end_of_group

    def groups(self) -> "Groups":
        """The subgroup streams (`_native.Subscription.groups`), read as the objects are. Every
        call returns the same iterator, so a group a cancelled read took goes to the next read
        through any of them."""
        if self._groups is None:
            self._groups = Groups(self._native.groups(), self)
        return self._groups

    def set_forward(self, forward: bool) -> asyncio.Future[None]:
        """`_native.Subscription.set_forward`."""
        return self._native.set_forward(forward)

    def set_priority(self, priority: int) -> asyncio.Future[None]:
        """`_native.Subscription.set_priority`."""
        return self._native.set_priority(priority)

    def request_new_group(self, largest_group: int | None = None) -> asyncio.Future[bool]:
        """`_native.Subscription.request_new_group`."""
        return self._native.request_new_group(largest_group)

    def end_before_group(self, group_id: int) -> asyncio.Future[None]:
        """`_native.Subscription.end_before_group`."""
        return self._native.end_before_group(group_id)

    def close(self) -> asyncio.Future[None]:
        """UNSUBSCRIBE, sent at once, as `_native.Subscription.close` is; the iteration ends once
        the relay stops the track. Every call returns a future of its own that resolves once the
        UNSUBSCRIBE was sent, even after another was cancelled, and raises what failed it;
        cancelling it leaves the UNSUBSCRIBE going."""
        return self._closing.start(self._native.close)


class ObjectsWithEndOfGroup(AsyncIterator[_native.Object]):
    def __init__(self, native: _native.ObjectsWithEndOfGroup, subscription: Subscription):
        self._objects = _Reads(native, "the subscription", subscription._session)
        self._subscription = subscription

    def __aiter__(self) -> "ObjectsWithEndOfGroup":
        return self

    async def __anext__(self) -> _native.Object:
        """Raises StopAsyncIteration once the publisher ends the track."""
        self._subscription._read_objects(with_end_of_group=True)
        return await self._objects.next()


class Groups(AsyncIterator["Group"]):
    """The subgroup streams of a subscription, read one at a time as its objects are."""

    def __init__(self, native: _native.Groups, subscription: Subscription):
        self._groups = _Reads(native, "the subscription's groups", subscription._session)
        self._subscription = subscription

    def __aiter__(self) -> "Groups":
        return self

    async def __anext__(self) -> "Group":
        """Raises StopAsyncIteration once the track has no more streams."""
        self._subscription._read_as("group")
        return Group(await self._groups.next(), self._subscription._session)


class Group(AsyncIterator[_native.Object]):
    """One subgroup stream, iterated for its objects until the publisher finishes it, read one at a
    time as a subscription's objects are."""

    def __init__(self, native: _native.Group, session: "Session"):
        self.group_id = native.group_id
        self.subgroup_id = native.subgroup_id
        self._objects = _Reads(native, f"group {native.group_id}", session)

    def __aiter__(self) -> "Group":
        return self

    async def __anext__(self) -> _native.Object:
        """Raises StopAsyncIteration once the publisher finished the stream, and RuntimeError when
        the stream was reset."""
        return await self._objects.next()


class Fetch(AsyncIterator[_native.Object]):
    """The objects of one FETCH response that carry a payload, read one at a time as a
    subscription's objects are."""

    def __init__(self, native: _native.Fetch, session: "Session"):
        self._objects = _Reads(native, "the fetch", session)

    def __aiter__(self) -> "Fetch":
        return self

    async def __anext__(self) -> _native.Object:
        """Raises StopAsyncIteration once the response ends."""
        return await self._objects.next()
