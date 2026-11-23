import asyncio
import logging
import weakref
from collections import deque
from collections.abc import AsyncIterator, Awaitable, Callable, Coroutine, Generator, Hashable
from dataclasses import dataclass
from typing import Any, Literal, TypeVar

from moq_wasm import _native
from moq_wasm._ended import Ended, copied
from moq_wasm._errors import SessionEnded
from moq_wasm._once import Once, retrieve, wait_through_cancellation
from moq_wasm._subscription import Fetch, Subscription
from moq_wasm._token import RefreshToken, TokenKeeper, read_claims
from moq_wasm._wait import wait

logger = logging.getLogger("moq_wasm")

T = TypeVar("T")
H = TypeVar("H")
DEFAULT_RETENTION_SECONDS = 60.0
DEFAULT_PRIORITY = 128
Dial = Callable[[str | None], Awaitable[_native.Session]]


class SessionClosed(SessionEnded):
    """The session was closed with `close()`: raised by `Session.run` and by a request made
    afterwards."""


class ConnectionLost(SessionEnded):
    """The relay connection ended, other than by `close()`, and the native session could not
    replace it: it stopped retrying after 60 s, or the relay closed it with a termination code other
    than EXPIRED_AUTH_TOKEN, or with EXPIRED_AUTH_TOKEN for a session connected without
    `refresh_token`. `close_info` is how the last connection closed. Raised by `Session.run`,
    `Session.closed` and the requests made afterwards."""

    def __init__(self, message: str, close_info: _native.CloseInfo):
        super().__init__(message)
        self.close_info = close_info


@dataclass(frozen=True)
class NamespaceEvent:
    namespace: list[str]
    active: bool
    """False once the namespace was withdrawn (PUBLISH_NAMESPACE_DONE)."""


class NamespaceWatch(AsyncIterator[NamespaceEvent]):
    """The namespaces published and withdrawn under a prefix, until `close()` or the session
    ends. One read at a time: a read while another waits raises RuntimeError."""

    def __init__(self, session: "Session", prefix: list[str], events: _native.NamespaceWatch):
        self.prefix = prefix
        self._session = session
        self._events = events
        self._ended = asyncio.Event()
        self._next: asyncio.Future | None = None
        self._reading = False
        self._closing: Once[None] = Once()

    def __aiter__(self) -> "NamespaceWatch":
        return self

    async def __anext__(self) -> NamespaceEvent:
        if self._reading:
            raise RuntimeError(f"the watch of {self.prefix} is already being read")
        self._reading = True
        try:
            return await self._read()
        finally:
            self._reading = False

    async def _read(self) -> NamespaceEvent:
        if self._ended.is_set():
            raise StopAsyncIteration
        # Kept across a cancelled call, as by a timeout, so the event it brings is not lost.
        if self._next is None:
            self._next = asyncio.ensure_future(anext(self._events))
        ended = asyncio.ensure_future(self._ended.wait())
        try:
            await wait([self._next, ended], return_when=asyncio.FIRST_COMPLETED)
        finally:
            ended.cancel()
        if self._ended.is_set():
            raise StopAsyncIteration
        event, self._next = self._next, None
        try:
            native_event = event.result()
        except StopAsyncIteration:
            self._ended.set()
            raise
        return NamespaceEvent(native_event.namespace, native_event.active)

    async def close(self) -> None:
        """UNSUBSCRIBE_NAMESPACE, so the session may watch the prefix again; ends the iteration.
        Every call returns once the UNSUBSCRIBE_NAMESPACE was sent, even after one was cancelled."""
        if not self._closing.started:
            self._end()
            self._session._watches.discard(self)
        await self._closing.run(self._unsubscribe)

    async def _unsubscribe(self) -> None:
        if not self._session._done.done():
            await self._events.close()

    def _end(self) -> None:
        self._ended.set()
        if self._next is not None:
            self._next.cancel()


class TrackWriter:
    """A track the session publishes. What is written before the track is registered is dropped.
    The writer ends for good at the first of its `close()`, the session ending, the failure of its
    registration, the failure to publish the namespace of the broadcast it belongs to and the
    failure or cancellation of the opening written once it is registered, as an event timeline's,
    which closes it: from then on, writing raises what ended it (RuntimeError after `close()`), at
    once and before any native call, so nothing reaches the native origin's cache any more, a write
    waiting for its turn included. Writes are sent one at a time, in the order they were made; a
    send that started runs to its end even when its caller is cancelled, as by a timeout, and the
    next write waits for it. `close()` is final: it waits for a registration and a send under way
    and ends its track. A track has one writer in a session, from its creation until its `close()`
    finished or its registration failed. Objects carry no extension headers (LOC properties)."""

    def __init__(
        self,
        session: "Session",
        namespace: list[str],
        name: str,
        retention_seconds: float,
        *,
        publish: bool = False,
        datagram: bool = False,
        priority: int = DEFAULT_PRIORITY,
    ):
        # Immutable, so a caller changing a list or the name cannot redirect `close()`.
        self._namespace = tuple(namespace)
        self._name = name
        self.retention_seconds = retention_seconds
        self.publish = publish
        self.datagram = datagram
        self.priority = priority
        self._session = session
        self._writer: _native.TrackWriter | None = None
        # Set once the track is registered.
        self._registered = asyncio.Event()
        self._ended = Ended()
        # Set by `close()`, after which the writer no longer owns its track.
        self._closed = False
        self._ending: Once[None] = Once()
        # The native `next_subscriber` no caller took the answer of yet, as one whose caller was
        # cancelled: the next caller takes it, so no subscriber is missed.
        self._subscriber: asyncio.Future[bool] | None = None
        self._awaiting_subscriber = False
        # Clear while the track is being registered, so `close()` ends it once registered.
        self._not_registering = asyncio.Event()
        self._not_registering.set()
        # The opening `_when_bound` runs once the track is registered.
        self._on_bound: Callable[[], Awaitable[None]] | None = None
        # Called once `close()` finished.
        self._on_closed: list[Callable[[], None]] = []
        # The turns of the writes, in the order they came: the first one's holds it, through its
        # native send, and the others wait for theirs.
        self._turns: deque[asyncio.Future[None]] = deque()
        # The latest native send, which runs to its end even when its caller is cancelled.
        self._sent: asyncio.Future[int | None] | None = None

    @property
    def namespace(self) -> list[str]:
        """A copy: changing it changes nothing the writer does."""
        return list(self._namespace)

    @property
    def name(self) -> str:
        return self._name

    async def write(self, payload: bytes, *, new_group: bool = False) -> int | None:
        """`_native.TrackWriter.write` once the writes made before were sent; None for an object
        dropped before the track is registered. Raises what ended the writer once it ended, even
        when it ends while the write waits."""
        return await self._send(lambda writer: writer.write(payload, new_group=new_group))

    async def write_group(self, payload: bytes) -> int | None:
        """`_native.TrackWriter.write_group` once the writes made before were sent; raises and
        drops as `write`."""
        return await self._send(lambda writer: writer.write_group(payload))

    async def _send(self, send: Callable[[_native.TrackWriter], Awaitable[int | None]]) -> int | None:
        # Checked before waiting, so a write after `close()` does not wait behind a stalled send.
        self._raise_if_ended()
        turn = await self._take_turn()
        try:
            if self._sent is not None:
                # The native send a cancelled caller left.
                await self._ended.until(self._sent)
            # Checked again, as the writer may have ended meanwhile.
            self._raise_if_ended()
            if (writer := self._writer) is None:
                return None
            self._sent = asyncio.ensure_future(send(writer))
            # Its failure goes to the caller, and to nobody once the caller was cancelled.
            self._sent.add_done_callback(retrieve)
            return await asyncio.shield(self._sent)
        finally:
            self._leave_turn(turn)

    async def _take_turn(self) -> asyncio.Future[None]:
        """Waits for the write's turn, which comes in the order the writes came, at once when no
        other write has it. Raises what ended the writer instead once it ended, so a write queued
        behind a stalled send does not wait for it."""
        turn = asyncio.get_running_loop().create_future()
        if not self._turns:
            turn.set_result(None)
        self._turns.append(turn)
        if turn.done():
            return turn
        try:
            await self._ended.until(turn)
            self._raise_if_ended()
        except BaseException:
            self._leave_turn(turn)
            raise
        return turn

    def _leave_turn(self, turn: asyncio.Future[None]) -> None:
        """Gives `turn` up, passing it on to the next write when it was this write's."""
        given = self._turns[0] is turn
        self._turns.remove(turn)
        if given and self._turns:
            self._turns[0].set_result(None)

    async def _sent_before(self) -> None:
        if self._sent is not None:
            await asyncio.wait([self._sent])

    async def next_subscriber(self) -> bool:
        """`_native.TrackWriter.next_subscriber`; False once the writer ended, as by `close()`,
        leaving its broadcast or the session ending, and for a track that will not be registered,
        a waiting call included. One caller at a time: a call while another waits raises
        RuntimeError, and a subscriber that came as its caller was cancelled goes to the next
        call."""
        if self._awaiting_subscriber:
            raise RuntimeError(f"next_subscriber of track {self.name} is already awaited")
        self._awaiting_subscriber = True
        try:
            return await self._take_subscriber()
        finally:
            self._awaiting_subscriber = False

    async def _take_subscriber(self) -> bool:
        await self._registered_or_ended()
        if self._ended.ended or self._writer is None:
            return False
        if self._subscriber is None:
            self._subscriber = asyncio.ensure_future(self._writer.next_subscriber())
            self._subscriber.add_done_callback(retrieve)
        subscribed = self._subscriber
        await self._ended.until(subscribed)
        if self._ended.ended or subscribed.cancelled():
            return False
        self._subscriber = None
        return subscribed.result()

    async def registered(self) -> None:
        """Waits until the track is registered. Raises what ended the writer once it ended, as
        what failed the registration does, such as a `RequestError` for a refused PUBLISH, or a
        RuntimeError for a track that will not be registered, as one of a broadcast left first."""
        await self._registered_or_ended()
        self._raise_if_ended()

    async def _registered_or_ended(self) -> None:
        registered = asyncio.ensure_future(self._registered.wait())
        try:
            await self._ended.until(registered)
        finally:
            registered.cancel()

    async def close(self) -> None:
        """Ends the subscriptions to the track with PUBLISH_DONE and stops writing to it. The
        session keeps the track's numbering and cache, so a later `publish_track` of the same track
        continues them. Writing raises at once; every call returns once the PUBLISH_DONE was sent,
        even after one was cancelled."""
        await self._close(RuntimeError(f"track {self.name} of {self.namespace} was closed"))

    def _close(self, error: BaseException) -> asyncio.Future[None]:
        """Ends the writer with `error`, unless it ended before, and starts closing it; the close
        runs to its end even when the caller awaiting it is cancelled."""
        self._closed = True
        self._end_with(error)
        return self._ending.start(self._end)

    async def _end(self) -> None:
        try:
            await self._not_registering.wait()
            # The send under way goes out before the track ends; the writes waiting after it raise,
            # so none starts another.
            await self._sent_before()
            if self._writer is not None and not self._session._done.done():
                await self._session._native.end_track(list(self._namespace), self._name)
        finally:
            self._session._release_track(self)
            for closed in self._on_closed:
                closed()

    def _raise_if_ended(self) -> None:
        if self._ended.ended:
            raise self._ended.error()

    async def _bind(self, writer: _native.TrackWriter) -> None:
        self._writer = writer
        if self._closed:
            # Closed while it was being registered: `close()` ends the track now.
            return
        self._registered.set()
        if self._on_bound is not None:
            await self._open(self._on_bound)

    def _when_bound(self, opening: Callable[[], Awaitable[None]]) -> None:
        """Runs `opening`, a write ahead of every other, once the track is registered, or at once
        when it is: its failure or cancellation closes the writer, as at its registration."""
        if not self._registered.is_set():
            self._on_bound = opening
            return
        asyncio.ensure_future(self._open(opening)).add_done_callback(retrieve)

    async def _open(self, opening: Callable[[], Awaitable[None]]) -> None:
        try:
            await opening()
        except BaseException as error:
            # The opening failed or was cancelled, as by leaving the broadcast: the track is closed,
            # so nothing is written to it without its opening, ended with the failure for its writes
            # to raise.
            closed = RuntimeError(f"track {self.name} of {self.namespace} was closed")
            self._close(error if isinstance(error, Exception) else closed)
            raise

    def _end_with_session(self) -> None:
        """Ends the writer once the session ends, or at once when it ended, the session holding it
        until then. Called by its owner once it owns its track, so a writer refused its track leaves
        the session nothing to call."""
        self._session._on_end(self._end_with)

    def _end_with(self, error: BaseException, *, raised: Callable[[], None] | None = None) -> None:
        """Ends the writer with `error`, unless it ended before; `raised` is called each time a
        write or a wait raises it. Stops waiting for the next subscriber."""
        self._ended.end(error, raised=raised)
        self._session._forget_end(self._end_with)
        if self._subscriber is not None:
            self._subscriber.cancel()
            self._subscriber = None

    def _abandon(self, failure: Exception | None = None) -> None:
        """The track will not be registered, because of `failure` if given, which ends the
        writer."""
        self._end_with(failure or RuntimeError(f"track {self.name} of {self.namespace} was not registered"))


class Announcement:
    """A namespace the session publishes until `close()`."""

    def __init__(self, session: "Session", namespace: list[str], *, owns: bool):
        # Immutable, so a caller changing a list cannot redirect `close()`.
        self._namespace = tuple(namespace)
        self._session = session
        # Whether the withdrawal ends the session's ownership of the namespace, which a broadcast
        # keeps instead until it was left.
        self._owns = owns
        self._closing: Once[None] = Once()

    @property
    def namespace(self) -> list[str]:
        """A copy: changing it changes nothing the announcement does."""
        return list(self._namespace)

    async def close(self) -> None:
        """PUBLISH_NAMESPACE_DONE; the session stops publishing the namespace. Does nothing once the
        namespace was withdrawn, even when the session publishes it again since. Every call returns
        once the PUBLISH_NAMESPACE_DONE was sent, even after one was cancelled."""
        await self._closing.run(lambda: self._session._unannounce(self))


class Session:
    """A relay session. Its native session follows GOAWAY and replaces a lost connection for up to
    60 s, carrying its subscriptions, namespaces and namespace subscriptions over
    (docs/architecture/moqt/session-migration.md); when it cannot, the session ends with
    `ConnectionLost`."""

    def __init__(self, native: _native.Session, keeper: TokenKeeper | None):
        self._native = native
        self._keeper = keeper
        # The current announcement of each namespace the session publishes.
        self._published: dict[tuple[str, ...], Announcement] = {}
        # The owner of each track the session publishes, so no other one of the session publishes
        # it, and closing one cannot end another's subscriptions.
        self._tracks: dict[tuple[tuple[str, ...], str], object] = {}
        # How the native origin keeps each track the session registered, as of its first
        # registration, for the session's life.
        self._registrations: dict[tuple[tuple[str, ...], str], _Registration] = {}
        # The namespaces an announcement or a broadcast of the session owns, from its start until
        # it finished withdrawing them, so no other one of the session publishes them meanwhile.
        self._owned: set[tuple[str, ...]] = set()
        self._watches: set[NamespaceWatch] = set()
        # What `_end` calls with what ended the session: each ends a handle of the session.
        self._enders: set[Callable[[BaseException], None]] = set()
        # What `_end` calls with each handle that no part of the session owns yet, as one declared
        # on a broadcast before it was entered, and with what ended the session: kept only while
        # something else holds the handle, so a broadcast never entered is not kept alive.
        self._held_enders: weakref.WeakKeyDictionary[Any, Callable[[Any, BaseException], None]] = (
            weakref.WeakKeyDictionary()
        )
        # The tasks releasing what a request whose caller was cancelled acquired, with the key of
        # what they release.
        self._releases: dict[asyncio.Task, Hashable] = {}
        self._done: asyncio.Future[None] = asyncio.get_running_loop().create_future()
        self._closing: Once[None] = Once()
        # The session's own tasks the shutdown waits for.
        self._joined: list[asyncio.Task] = []
        self._tasks = [asyncio.create_task(self._watch_connection())]
        if keeper is not None:
            self._tasks.append(asyncio.create_task(self._keep_token_fresh(keeper)))

    async def subscribe(
        self,
        namespace: list[str],
        name: str,
        *,
        filter: Literal["largest_object", "next_group_start"] = "largest_object",
        forward: bool = True,
        priority: int | None = None,
        delivery_timeout: float | None = None,
        group_order: Literal["ascending", "descending"] = "ascending",
    ) -> Subscription:
        """SUBSCRIBE (`_native.Session.subscribe`): by default only the objects published after
        it. Cancelled before it returns, it unsubscribes once the relay answered."""
        namespace = list(namespace)
        return await self._subscription(
            namespace,
            name,
            lambda native: native.subscribe(
                namespace,
                name,
                filter=filter,
                forward=forward,
                priority=priority,
                delivery_timeout=delivery_timeout,
                group_order=group_order,
            ),
        )

    async def subscribe_group_start(
        self,
        namespace: list[str],
        name: str,
        new_group_after: int,
        *,
        priority: int | None = None,
        delivery_timeout: float | None = None,
        group_order: Literal["ascending", "descending"] = "ascending",
    ) -> Subscription:
        """Subscribes from the first object of a group (`_native.Session.subscribe_group_start`);
        cancelled as `subscribe`. It is read by object only."""
        namespace = list(namespace)
        return await self._subscription(
            namespace,
            name,
            lambda native: native.subscribe_group_start(
                namespace,
                name,
                new_group_after,
                priority=priority,
                delivery_timeout=delivery_timeout,
                group_order=group_order,
            ),
            joining=True,
        )

    async def fetch(
        self, namespace: list[str], name: str, start: _native.Location, end: _native.Location
    ) -> Fetch:
        """A Standalone FETCH (`_native.Session.fetch`)."""
        return Fetch(await self._connected().fetch(namespace, name, start, end), self)

    async def track_status(self, namespace: list[str], name: str) -> _native.Location | None:
        """TRACK_STATUS (`_native.Session.track_status`)."""
        return await self._connected().track_status(namespace, name)

    async def subscribe_current_group(
        self,
        namespace: list[str],
        name: str,
        *,
        priority: int | None = None,
        delivery_timeout: float | None = None,
        group_order: Literal["ascending", "descending"] = "ascending",
    ) -> Subscription:
        """The objects of the group the relay holds come first (a joining FETCH), then the live
        ones (`_native.Session.subscribe_current_group`); cancelled as `subscribe`. It is read by
        object only."""
        namespace = list(namespace)
        return await self._subscription(
            namespace,
            name,
            lambda native: native.subscribe_current_group(
                namespace,
                name,
                priority=priority,
                delivery_timeout=delivery_timeout,
                group_order=group_order,
            ),
            joining=True,
        )

    async def _subscription(
        self,
        namespace: list[str],
        name: str,
        request: Callable[[_native.Session], Awaitable[_native.Subscription]],
        *,
        joining: bool = False,
    ) -> Subscription:
        """`joining` for a subscription whose native reader merges a joining FETCH into its
        objects, which is read by object from the start."""
        native = await self._acquire(
            _subscription_key(namespace, name),
            lambda: request(self._connected()),
            lambda subscription: subscription.close(),
        )
        return Subscription(native, self, read_by="object" if joining else None)

    async def watch_namespaces(self, prefix: list[str]) -> NamespaceWatch:
        """Raises `RequestError` when the relay refuses the SUBSCRIBE_NAMESPACE. Cancelled before
        it returns, it unsubscribes once the relay answered, and a later watch of the prefix waits
        for that."""
        prefix = list(prefix)
        native = await self._acquire(
            ("prefix", tuple(prefix)),
            lambda: self._connected().watch_namespaces(prefix),
            lambda events: events.close(),
        )
        watch = NamespaceWatch(self, prefix, native)
        self._watches.add(watch)
        return watch

    async def announce(self, namespace: list[str]) -> Announcement:
        """PUBLISH_NAMESPACE. Raises `RequestError` when the relay refuses it, and RuntimeError while
        another announcement or broadcast of the session publishes the namespace, until it finished
        withdrawing it. Cancelled before it returns, it withdraws the namespace once the relay
        answered, and a later announcement of the namespace waits for that."""
        namespace = list(namespace)
        self._raise_if_ended()
        self._own(namespace)
        try:
            return await self._announce(namespace, lambda native: native.announce(namespace), owns=True)
        except BaseException:
            self._disown(namespace)
            raise

    def _own(self, namespace: list[str]) -> None:
        """Raises RuntimeError while another announcement or broadcast of the session owns
        `namespace`."""
        key = tuple(namespace)
        if key in self._owned:
            raise RuntimeError(f"the session already publishes {namespace}")
        self._owned.add(key)

    def _disown(self, namespace: list[str]) -> None:
        self._owned.discard(tuple(namespace))

    async def _announce(
        self, namespace: list[str], request: Callable[[_native.Session], Awaitable[None]], *, owns: bool
    ) -> Announcement:
        withdrawn = list(namespace)
        await self._acquire(
            _namespace_key(namespace),
            lambda: request(self._connected()),
            lambda _: self._native.unannounce(withdrawn),
        )
        return self._announced(namespace, owns=owns)

    async def _acquire(
        self, key: Hashable, request: Callable[[], Awaitable[T]], release: Callable[[T], Awaitable[None]]
    ) -> T:
        """`request()`, a native request whose success leaves something active on the relay, once
        what an earlier request of `key` left was released. A caller cancelled before it takes the
        result leaves the request running, and `release` undoes what it acquired, so nothing stays
        active without an owner."""
        await self._released(key)
        acquiring = asyncio.ensure_future(request())
        # Its failure goes to the caller, and to nobody once the caller and its release are gone.
        acquiring.add_done_callback(retrieve)
        try:
            await asyncio.wait([acquiring])
        except asyncio.CancelledError:
            self._release_later(key, self._release(acquiring, release))
            raise
        return acquiring.result()

    def _release_later(self, key: Hashable, releasing: Coroutine[Any, Any, None]) -> None:
        """Runs `releasing`, which undoes what the session acquired for `key` and no caller owns;
        a later request of `key` waits for it."""
        task = asyncio.create_task(releasing)
        self._releases[task] = key
        task.add_done_callback(self._releases.pop)

    async def _release(self, acquiring: asyncio.Future[T], release: Callable[[T], Awaitable[None]]) -> None:
        await asyncio.wait([acquiring])
        # The native session's close ends what it acquired.
        if acquiring.cancelled() or acquiring.exception() is not None or self._done.done():
            return
        try:
            await release(acquiring.result())
        except Exception as error:
            logger.debug("releasing what a cancelled request acquired failed: %r", error)

    async def _released(self, key: Hashable) -> None:
        """Waits until what the cancelled requests of `key` acquired was released."""
        if releasing := self._releasing(key):
            await asyncio.wait(releasing)

    def _unsubscribing(self, namespace: list[str], name: str) -> set[asyncio.Task]:
        """The tasks unsubscribing what the cancelled subscriptions to the track acquired, once the
        relay answered them."""
        return self._releasing(_subscription_key(namespace, name))

    def _releasing(self, key: Hashable) -> set[asyncio.Task]:
        """The tasks releasing what the cancelled requests of `key` acquired."""
        return {task for task, released in self._releases.items() if released == key}

    async def publish_track(
        self,
        namespace: list[str],
        name: str,
        *,
        retention_seconds: float = DEFAULT_RETENTION_SECONDS,
        publish: bool = False,
        datagram: bool = False,
        priority: int = DEFAULT_PRIORITY,
    ) -> TrackWriter:
        """Registers a track that the session answers SUBSCRIBE and FETCH for
        (`_native.Session.publish_track`). Register it before `announce`, since the relay may forward
        a SUBSCRIBE at once; with `publish`, announce first. `priority` is the Publisher Priority:
        among the session's tracks, a lower value is sent first. Raises what ended the session once
        it ended, RuntimeError while another writer of the session publishes the track, until its
        `close()` finished, ValueError when `retention_seconds` is not the one the track was
        registered with first in the session, which it keeps (the newest object for a media
        broadcast's catalog or 60 s for its other tracks, for a track a media broadcast published),
        when the track has an object and `datagram` is not the delivery it sent it with, which it
        keeps, and for an invalid `retention_seconds`, which registers nothing, and OverflowError
        for a `priority` outside 0-255. Cancelled before it returns, it ends the track once
        registered, and keeps the track its own until then: a later `publish_track` of the track
        waits for that, and another owner of the track, such as a broadcast, is refused
        meanwhile."""
        self._raise_if_ended()
        await self._released(_published_track_key(tuple(namespace), name))
        self._raise_if_ended()
        writer = self._track_writer(
            namespace, name, retention_seconds, publish=publish, datagram=datagram, priority=priority
        )
        await self._register(writer)
        return writer

    def _track_writer(
        self,
        namespace: list[str],
        name: str,
        retention_seconds: float,
        *,
        publish: bool,
        datagram: bool,
        priority: int,
    ) -> TrackWriter:
        """Raises RuntimeError while another writer of the session publishes the track."""
        writer = TrackWriter(
            self, namespace, name, retention_seconds, publish=publish, datagram=datagram, priority=priority
        )
        self._claim_tracks(writer._namespace, {name: writer})
        writer._end_with_session()
        return writer

    def _claim_tracks(self, namespace: tuple[str, ...], owners: dict[str, object]) -> None:
        """Makes each owner the one of its track. Raises RuntimeError, claiming none of them, while
        another owner of the session publishes one of the tracks."""
        for name in owners:
            if (namespace, name) in self._tracks:
                raise RuntimeError(f"the session already publishes track {name} of {list(namespace)}")
        for name, owner in owners.items():
            self._tracks[(namespace, name)] = owner

    def _release_tracks(self, namespace: tuple[str, ...], owners: dict[str, object]) -> None:
        """Releases the tracks each owner still owns."""
        for name, owner in owners.items():
            if self._tracks.get((namespace, name)) is owner:
                del self._tracks[(namespace, name)]

    def _release_track(self, writer: TrackWriter) -> None:
        """Releases the writer's track once what a cancelled registration of it acquired was
        released: until then that cleanup may still end the track, which would end the
        subscriptions of another owner of it."""
        releasing = self._releasing(_published_track_key(writer._namespace, writer._name))
        if not releasing:
            self._release_tracks(writer._namespace, {writer._name: writer})
            return

        def released(task: asyncio.Task) -> None:
            releasing.discard(task)
            if not releasing:
                self._release_tracks(writer._namespace, {writer._name: writer})

        for task in list(releasing):
            task.add_done_callback(released)

    async def _register(self, writer: TrackWriter) -> None:
        if writer._closed:
            # Closing it released the track, which another writer may publish by now.
            return
        writer._not_registering.clear()
        namespace = list(writer.namespace)
        try:
            # Cancelled, it ends the track once registered, before another writer registers it.
            native = await self._acquire(
                _published_track_key(writer._namespace, writer.name),
                lambda: self._publish_track(writer),
                lambda _: self._native.end_track(namespace, writer.name),
            )
        except BaseException as error:
            # A cancellation is the cancelled caller's, not the track's: the writer's other callers
            # raise that it was not registered.
            writer._abandon(error if isinstance(error, Exception) else None)
            self._release_track(writer)
            raise
        finally:
            writer._not_registering.set()
        await writer._bind(native)

    async def _publish_track(self, writer: TrackWriter) -> _native.TrackWriter:
        """`_native.Session.publish_track` for `writer`. Raises ValueError, before any native call,
        when the writer asks for a retention other than the one the track was registered with first
        in the session, which the native origin keeps; the native call raises ValueError for the
        other delivery than the one the track sent its objects with. The registration is recorded
        once the native origin registered the track: when it succeeded, and when only its PUBLISH
        was refused, but not when the native call refused its arguments, as an invalid
        `retention_seconds` or the other delivery, before registering anything."""
        native = self._connected()
        key = (writer._namespace, writer.name)
        established = self._registrations.get(key)
        if established is not None:
            established.refuse_other_retention(writer)
        registering = native.publish_track(
            list(writer.namespace),
            writer.name,
            retention_seconds=writer.retention_seconds,
            publish=writer.publish,
            datagram=writer.datagram,
            priority=writer.priority,
        )
        # The native origin takes the delivery of each registration, and keeps the retention of
        # the first one.
        registration = _Registration(writer.datagram, writer.retention_seconds)
        try:
            registered = await registering
        except _native.RequestError:
            # The relay refused the PUBLISH, which follows the track's registration.
            self._registrations[key] = registration
            raise
        self._registrations[key] = registration
        return registered

    def _record_media_tracks(self, namespace: tuple[str, ...], retentions: dict[str, float]) -> None:
        """Records the tracks of a media broadcast, which the native media broadcast may register
        at any time from now on, on subgroup streams, each with the retention it gives it."""
        for name, retention_seconds in retentions.items():
            self._registrations.setdefault((namespace, name), _Registration(False, retention_seconds))

    def _refuse_other_registrations(self, namespace: tuple[str, ...], retentions: dict[str, float]) -> None:
        """Raises ValueError when the session registered one of a media broadcast's tracks, each
        with the retention `retentions` gives it, in datagrams or with another retention, which the
        native origin keeps: a media broadcast would send that track in datagrams too, which its
        subscribers cannot read by group, or keep another span of its objects for late
        subscribers."""
        for name, retention_seconds in retentions.items():
            registration = self._registrations.get((namespace, name))
            if registration is None:
                continue
            if registration.datagram:
                raise ValueError(
                    f"track {name} of {list(namespace)} sends its objects in datagrams, which a media "
                    "broadcast's track cannot: give the media track another name"
                )
            if registration.retention_seconds != retention_seconds:
                raise ValueError(
                    f"track {name} of {list(namespace)} keeps its objects for {registration.retention_seconds} s, "
                    f"a media broadcast's for {retention_seconds} s: give the media track another name"
                )

    def _announced(self, namespace: list[str], *, owns: bool) -> Announcement:
        announcement = Announcement(self, namespace, owns=owns)
        self._published[tuple(namespace)] = announcement
        return announcement

    async def _unannounce(self, announcement: Announcement) -> None:
        key = announcement._namespace
        if self._published.get(key) is not announcement:
            return
        del self._published[key]
        try:
            if not self._done.done():
                await self._native.unannounce(announcement.namespace)
        finally:
            if announcement._owns:
                self._disown(announcement.namespace)

    def _publishes(self, namespace: list[str]) -> bool:
        """Whether the session publishes `namespace`, is publishing it or is still withdrawing it,
        as the relay may forward the namespace before it answered the PUBLISH_NAMESPACE."""
        key = tuple(namespace)
        return (
            key in self._owned
            or key in self._published
            or _namespace_key(namespace) in self._releases.values()
        )

    async def run(self, work: Awaitable[T]) -> T:
        """Awaits `work`, cancelling it when the session ends and waiting for it to finish its
        cleanup, such as a pipeline's. Raises what ended the session, such as `ConnectionLost` or
        `moq_wasm.TokenExpired`, or `SessionClosed` after `close()`; called once the session ended,
        it raises that without starting `work`, closing it when it is a coroutine, and cancelling
        it, and waiting for its cleanup, when it is a task or a future already scheduled. Cancelled,
        it cancels `work` and raises the cancellation once `work` finished, even when cancelled again
        while `work` cleans up. The wait covers `work` itself, not what it started: a future that
        finishes before its children, as `asyncio.gather`'s does once one of them ended cancelled,
        leaves the others cleaning up, so run several workers in one coroutine that starts them in
        an `asyncio.TaskGroup`, which waits for every one and raises their failures inside an
        `ExceptionGroup`, which `except*` catches."""
        if self._done.done():
            if asyncio.iscoroutine(work):
                work.close()
            elif asyncio.isfuture(work):
                await _cancel_work(work)
            self._raise_if_ended()
        running = asyncio.ensure_future(work)
        try:
            await wait([running, self._done], return_when=asyncio.FIRST_COMPLETED)
            # A failure also ends the watches, so `work` may have returned because of it.
            if self._done.done() and (failure := self._done.exception()) is not None:
                raise failure
            if running.done():
                return running.result()
            raise SessionClosed("the session was closed")
        finally:
            await _cancel_work(running)

    async def closed(self) -> None:
        """Waits until the session ends: `close()`, or a failure, which this raises."""
        await asyncio.shield(self._done)

    def transport_stats(self) -> _native.TransportStats:
        return self._native.transport_stats()

    async def close(self) -> None:
        await self._end(None)

    def _connected(self) -> _native.Session:
        """The native session; raises what ended the session once it ended, which is no
        RuntimeError, so a loop that retries a refused request stops."""
        self._raise_if_ended()
        return self._native

    def _raise_if_ended(self) -> None:
        if self._done.done():
            raise copied(self._ended_with())

    def _ended_with(self) -> BaseException:
        """What ended the session, once it ended."""
        return self._done.exception() or SessionClosed("the session was closed")

    def _on_end(self, ender: Callable[[BaseException], None]) -> None:
        """Calls `ender` with what ended the session once it ends, or at once when it ended."""
        if self._done.done():
            ender(self._ended_with())
        else:
            self._enders.add(ender)

    def _forget_end(self, ender: Callable[[BaseException], None]) -> None:
        self._enders.discard(ender)

    def _on_end_while_held(self, handle: H, ender: Callable[[H, BaseException], None]) -> None:
        """Calls `ender(handle, ...)` with what ended the session once it ends, or at once when it
        ended, unless nothing but the session holds `handle` by then."""
        if self._done.done():
            ender(handle, self._ended_with())
        else:
            self._held_enders[handle] = ender

    def _forget_end_while_held(self, handle: object) -> None:
        self._held_enders.pop(handle, None)

    async def _watch_connection(self) -> None:
        close_info = await self._native.closed()
        await self._end(ConnectionLost("the relay connection was lost", close_info))

    async def _keep_token_fresh(self, keeper: TokenKeeper) -> None:
        try:
            await keeper.keep_fresh(self._hand_over_token)
        except Exception as error:
            # `_end` cancels the other tasks, not this one.
            await self._end(error)

    async def _hand_over_token(self, token: str) -> None:
        """Raises RuntimeError when the relay refuses the token, which the keeper retries. The
        native session presents the token on its next connection all the same, until a later one
        the relay accepts replaces it."""
        assert self._keeper is not None
        await self._native.refresh_token(token, app_id=self._keeper.app_id)

    async def _end(self, failure: BaseException | None) -> None:
        """Ends the session once, with `failure` or as closed, and waits until the session's own
        tasks finished, other than the one ending it, and the native session closed; a caller
        cancelled meanwhile leaves the close running for the next one to await. A task the shutdown
        waits for, such as a `refresh_token` call that calls `close()` from its cleanup, returns at
        once instead."""
        current = asyncio.current_task()
        if self._closing.started and self._waited_by_shutdown(current):
            return
        if not self._closing.started:
            if failure is None:
                self._done.set_result(None)
            else:
                self._done.set_exception(failure)
                # Retrieved by `closed()` or `run()`; a session nobody awaits does not warn.
                self._done.exception()
            others = [task for task in self._tasks if task is not current]
            self._joined = others
            for task in others:
                task.cancel()
            for watch in self._watches:
                watch._end()
            ended_with = self._ended_with()
            for ender in list(self._enders):
                ender(ended_with)
            self._enders.clear()
            for handle, held_ender in list(self._held_enders.items()):
                held_ender(handle, ended_with)
            self._held_enders.clear()
        await self._closing.run(lambda: self._shut_down(current))

    def _waited_by_shutdown(self, task: asyncio.Task | None) -> bool:
        return task in self._joined or (self._keeper is not None and self._keeper.runs(task))

    async def _shut_down(self, ending: asyncio.Task | None) -> None:
        """Waits for the session's own tasks other than `ending`, the one that ended the session,
        then closes the native session."""
        # A cancelled task may still run cleanup.
        await asyncio.gather(*self._joined, return_exceptions=True)
        if self._keeper is not None:
            # The calls of `refresh_token`, one abandoned at its deadline included, clean up too.
            await self._keeper.stop(sparing=ending)
        await self._native.close()


@dataclass(frozen=True)
class _Registration:
    """How the native origin keeps a track the session registered: with the delivery of its latest
    registration and the retention of its first one, for the session's life."""

    datagram: bool
    retention_seconds: float

    def refuse_other_retention(self, writer: TrackWriter) -> None:
        """Raises ValueError when `writer` asks for another retention."""
        if self.retention_seconds != writer.retention_seconds:
            raise ValueError(
                f"track {writer.name} of {writer.namespace} keeps its objects for "
                f"{self.retention_seconds} s: register it again with retention_seconds={self.retention_seconds}"
            )


async def _cancel_work(running: asyncio.Future) -> None:
    """Cancels `running`, the work of `Session.run`, unless it finished, and waits until it
    finished its cleanup, even when cancelled meanwhile."""
    if not running.done():
        running.cancel()
        await wait_through_cancellation([running])
    if not running.cancelled() and (error := running.exception()) is not None:
        logger.debug("the work run in the session failed while it was cancelled: %r", error)


def _namespace_key(namespace: list[str]) -> Hashable:
    return ("namespace", tuple(namespace))


def _subscription_key(namespace: list[str], name: str) -> Hashable:
    return ("track", tuple(namespace), name)


def _published_track_key(namespace: tuple[str, ...], name: str) -> Hashable:
    return ("published track", namespace, name)


class _Connecting:
    """What `connect` returns: awaited, the open `Session`; used with `async with`, a session closed
    on exit, which re-raises what ended the session. Entering it while it is entered raises
    RuntimeError without connecting, since one exit closes one session. Each session it opens
    refreshes its own token, starting from the newest one the session opened before it holds."""

    def __init__(self, dial: Dial, token: str | None, refresh: RefreshToken | None = None):
        self._dial = dial
        self._token = token
        self._refresh = refresh
        # The keeper of the session opened last, whose token the next session starts from.
        self._latest: TokenKeeper | None = None
        self._session: Session | None = None
        # From the start of `__aenter__` until its failure or the end of `__aexit__`.
        self._entered = False

    def __await__(self) -> Generator[Any, None, Session]:
        return self._start().__await__()

    async def _start(self) -> Session:
        token = self._latest.token if self._latest is not None else self._token
        # One keeper per session: two sessions sharing one would each take the token the other
        # refreshed as not later than the current one, and end with TokenExpired.
        keeper = TokenKeeper(token, self._refresh) if self._refresh is not None and token is not None else None
        session = Session(await self._dial(token), keeper)
        if keeper is not None:
            self._latest = keeper
        return session

    async def __aenter__(self) -> Session:
        if self._entered:
            raise RuntimeError("the connection is already entered with async with")
        self._entered = True
        try:
            self._session = await self._start()
        except BaseException:
            self._entered = False
            raise
        return self._session

    async def __aexit__(self, *exc_info: object) -> None:
        assert self._session is not None
        session = self._session
        try:
            await session.close()
        finally:
            self._session = None
            self._entered = False
        if exc_info[0] is None and (failure := session._done.exception()) is not None:
            raise failure


def connect(
    url: str,
    *,
    verify_certificate: bool = True,
    token: str | None = None,
    refresh_token: RefreshToken | None = None,
    versions: list[str] | None = None,
) -> _Connecting:
    """Connects to a relay: `moqt://` over raw QUIC, `https://` over WebTransport, presenting
    `token` (a JWT) in CLIENT_SETUP. Use it with `async with`, or await it and call `close()`.
    A failure to connect raises RuntimeError, as does a relay that does not open the transport, or
    answer CLIENT_SETUP once it is open, within 30 s. Without `refresh_token`, the session ends with
    `ConnectionLost` once the relay closes it at the token's expiry.

    `refresh_token`, given with `token`, takes the current token and returns a later one of the
    same app. The session calls it five minutes before the token's `exp`, or halfway through a
    shorter lifetime. A call that raises, takes over 30 s or past the token's expiry together with
    the relay's answer to its token (even one that caught its timeout and returned), or returns a
    token of another app, one that does not expire later or one the relay refuses is retried with
    the current token after 30 s, or half the time left, until the token expired; the session then
    ends with `TokenExpired`. A call that ends cancelled while the session is not, as one awaiting a
    shared future another task cancelled, is retried as well. A call is given up at its deadline,
    however long its cleanup takes. `versions` lists the drafts to offer, most preferred first;
    without it draft-18, then draft-14, are offered."""
    if refresh_token is not None and token is None:
        raise ValueError("refresh_token needs a token")
    if refresh_token is not None and token is not None:
        # Raises ValueError for what is not a token before anything is dialed.
        read_claims(token)

    async def dial(current_token: str | None) -> _native.Session:
        return await _native.connect(
            url,
            verify_certificate=verify_certificate,
            token=current_token,
            refreshes_token=refresh_token is not None,
            versions=versions,
        )

    return _Connecting(dial, token, refresh_token)
