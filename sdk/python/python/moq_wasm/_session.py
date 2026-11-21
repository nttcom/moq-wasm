import asyncio
import logging
import weakref
from collections.abc import AsyncIterator, Awaitable, Callable, Coroutine, Generator, Hashable
from dataclasses import dataclass
from typing import Any, Literal, TypeVar

from moq_wasm import _native
from moq_wasm._ended import copied
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
    """The namespaces published and withdrawn under a prefix, until the session ends."""

    def __init__(self, prefix: list[str], events: _native.NamespaceWatch):
        self.prefix = prefix
        self._events: asyncio.Queue[NamespaceEvent | None] = asyncio.Queue()
        self._ended = False
        self._pump = asyncio.create_task(self._forward(events))

    def __aiter__(self) -> "NamespaceWatch":
        return self

    async def __anext__(self) -> NamespaceEvent:
        event = await self._events.get()
        if event is None:
            self._events.put_nowait(None)
            raise StopAsyncIteration
        return event

    def close(self) -> None:
        """Ends the iteration. The relay keeps the SUBSCRIBE_NAMESPACE until the session ends."""
        self._end()

    async def _forward(self, events: _native.NamespaceWatch) -> None:
        async for event in events:
            self._events.put_nowait(NamespaceEvent(event.namespace, event.active))
        self._end()

    def _end(self) -> None:
        if self._ended:
            return
        self._ended = True
        if self._pump is not asyncio.current_task():
            self._pump.cancel()
        self._events.put_nowait(None)


class TrackWriter:
    """A track the session publishes. What is written before the track is registered, or after the
    session ended, is dropped. Objects carry no extension headers (LOC properties)."""

    def __init__(
        self,
        namespace: list[str],
        name: str,
        retention_seconds: float,
        *,
        publish: bool = False,
        datagram: bool = False,
        priority: int = DEFAULT_PRIORITY,
        session_ended: asyncio.Future[None] | None = None,
    ):
        self.namespace = namespace
        self.name = name
        self.retention_seconds = retention_seconds
        self.publish = publish
        self.datagram = datagram
        self.priority = priority
        self._session_ended = session_ended
        self._writer: _native.TrackWriter | None = None
        # Set once the track is registered, or will not be.
        self._settled = asyncio.Event()
        self._on_bound: Callable[[], Awaitable[None]] | None = None

    async def write(self, payload: bytes, *, new_group: bool = False) -> int | None:
        """`_native.TrackWriter.write`; None for an object dropped before the track is
        registered."""
        if self._writer is None:
            return None
        return await self._writer.write(payload, new_group=new_group)

    async def write_group(self, payload: bytes) -> int | None:
        """`_native.TrackWriter.write_group`; None for an object dropped before the track is
        registered."""
        if self._writer is None:
            return None
        return await self._writer.write_group(payload)

    async def next_subscriber(self) -> bool:
        """`_native.TrackWriter.next_subscriber`; False when the session ends before the track is
        registered."""
        settled = asyncio.ensure_future(self._settled.wait())
        ended = [] if self._session_ended is None else [self._session_ended]
        try:
            await wait([settled, *ended], return_when=asyncio.FIRST_COMPLETED)
        finally:
            settled.cancel()
        if self._writer is None:
            return False
        return await self._writer.next_subscriber()

    async def _bind(self, writer: _native.TrackWriter) -> None:
        self._writer = writer
        self._settled.set()
        if self._on_bound is not None:
            await self._on_bound()

    def _abandon(self) -> None:
        """The track will not be registered."""
        self._settled.set()


class Announcement:
    """A namespace the session publishes until `close()`."""

    def __init__(self, session: "Session", namespace: list[str]):
        self.namespace = namespace
        self._session = session

    async def close(self) -> None:
        """PUBLISH_NAMESPACE_DONE; the session stops publishing the namespace."""
        await self._session._unannounce(self.namespace)


class Session:
    """A relay session. Its native session follows GOAWAY and replaces a lost connection for up to
    60 s, carrying its subscriptions, namespaces and namespace subscriptions over
    (docs/architecture/moqt/session-migration.md); when it cannot, the session ends with
    `ConnectionLost`."""

    def __init__(self, native: _native.Session, keeper: TokenKeeper | None):
        self._native = native
        self._keeper = keeper
        self._published: set[tuple[str, ...]] = set()
        self._watches: list[NamespaceWatch] = []
        # What `_end` calls with each handle the session holds only weakly, as the reads of a
        # subscription, and with what ended the session: kept only while something else holds the
        # handle, so a subscription nobody reads any more is not kept alive.
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
        watch = NamespaceWatch(prefix, native)
        self._watches.append(watch)
        return watch

    async def announce(self, namespace: list[str]) -> Announcement:
        """PUBLISH_NAMESPACE. Raises `RequestError` when the relay refuses it. Cancelled before it
        returns, it withdraws the namespace once the relay answered, and a later announcement of the
        namespace waits for that."""
        namespace = list(namespace)
        return await self._announce(namespace, lambda native: native.announce(namespace))

    async def _announce(
        self, namespace: list[str], request: Callable[[_native.Session], Awaitable[None]]
    ) -> Announcement:
        withdrawn = list(namespace)
        await self._acquire(
            _namespace_key(namespace),
            lambda: request(self._connected()),
            lambda _: self._native.unannounce(withdrawn),
        )
        return self._announced(namespace)

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
        a SUBSCRIBE at once; with `publish`, announce first. Cancelled before it returns, it ends the
        track once registered, and a later `publish_track` of the track waits for that."""
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
        return TrackWriter(
            namespace,
            name,
            retention_seconds,
            publish=publish,
            datagram=datagram,
            priority=priority,
            session_ended=self._done,
        )

    async def _register(self, writer: TrackWriter) -> None:
        namespace = list(writer.namespace)
        try:
            # Cancelled, it ends the track once registered, before another writer registers it.
            native = await self._acquire(
                _published_track_key(tuple(namespace), writer.name),
                lambda: self._connected().publish_track(
                    namespace,
                    writer.name,
                    retention_seconds=writer.retention_seconds,
                    publish=writer.publish,
                    datagram=writer.datagram,
                    priority=writer.priority,
                ),
                lambda _: self._native.end_track(namespace, writer.name),
            )
        except BaseException:
            writer._abandon()
            raise
        await writer._bind(native)

    def _announced(self, namespace: list[str]) -> Announcement:
        """Call once the relay accepted `namespace`."""
        self._published.add(tuple(namespace))
        return Announcement(self, namespace)

    async def _unannounce(self, namespace: list[str]) -> None:
        if tuple(namespace) not in self._published:
            return
        self._published.discard(tuple(namespace))
        if not self._done.done():
            await self._native.unannounce(namespace)

    def _publishes(self, namespace: list[str]) -> bool:
        """Whether the session publishes `namespace` or is still withdrawing it, as the relay may
        forward the namespace before it answered the PUBLISH_NAMESPACE."""
        key = tuple(namespace)
        return key in self._published or _namespace_key(namespace) in self._releases.values()

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
