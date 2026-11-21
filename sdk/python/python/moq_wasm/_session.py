import asyncio
import logging
from collections.abc import AsyncIterator, Awaitable, Callable, Generator
from dataclasses import dataclass
from typing import Any, Literal, TypeVar

from moq_wasm import _native
from moq_wasm._ended import copied
from moq_wasm._errors import SessionEnded
from moq_wasm._once import Once, wait_through_cancellation
from moq_wasm._token import RefreshToken, TokenKeeper
from moq_wasm._wait import wait

logger = logging.getLogger("moq_wasm")

T = TypeVar("T")
DEFAULT_RETENTION_SECONDS = 60.0
DEFAULT_PRIORITY = 128
Dial = Callable[[str | None], Awaitable[_native.Session]]


class SessionClosed(SessionEnded):
    """The session was closed with `close()`: raised by `Session.run` and by a request made
    afterwards."""


class ConnectionLost(SessionEnded):
    """The relay connection ended, other than by `close()`, and the native session could not
    replace it: it stopped retrying after 60 s, or the relay closed it with a termination code other
    than EXPIRED_AUTH_TOKEN. `close_info` is how the last connection closed. Raised by `Session.run`,
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
    ) -> _native.Subscription:
        """SUBSCRIBE (`_native.Session.subscribe`): by default only the objects published after
        it."""
        return await self._connected().subscribe(
            namespace,
            name,
            filter=filter,
            forward=forward,
            priority=priority,
            delivery_timeout=delivery_timeout,
            group_order=group_order,
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
    ) -> _native.Subscription:
        """Subscribes from the first object of a group (`_native.Session.subscribe_group_start`)."""
        return await self._connected().subscribe_group_start(
            namespace,
            name,
            new_group_after,
            priority=priority,
            delivery_timeout=delivery_timeout,
            group_order=group_order,
        )

    async def fetch(
        self, namespace: list[str], name: str, start: _native.Location, end: _native.Location
    ) -> _native.Fetch:
        """A Standalone FETCH (`_native.Session.fetch`)."""
        return await self._connected().fetch(namespace, name, start, end)

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
    ) -> _native.Subscription:
        """The objects of the group the relay holds come first (a joining FETCH), then the live
        ones (`_native.Session.subscribe_current_group`)."""
        return await self._connected().subscribe_current_group(
            namespace,
            name,
            priority=priority,
            delivery_timeout=delivery_timeout,
            group_order=group_order,
        )

    async def watch_namespaces(self, prefix: list[str]) -> NamespaceWatch:
        """Raises `RequestError` when the relay refuses the SUBSCRIBE_NAMESPACE."""
        watch = NamespaceWatch(prefix, await self._connected().watch_namespaces(prefix))
        self._watches.append(watch)
        return watch

    async def announce(self, namespace: list[str]) -> Announcement:
        """PUBLISH_NAMESPACE. Raises `RequestError` when the relay refuses it."""
        await self._connected().announce(namespace)
        return self._announced(namespace)

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
        a SUBSCRIBE at once; with `publish`, announce first."""
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
        try:
            native = await self._connected().publish_track(
                writer.namespace,
                writer.name,
                retention_seconds=writer.retention_seconds,
                publish=writer.publish,
                datagram=writer.datagram,
                priority=writer.priority,
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
        return tuple(namespace) in self._published

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
        assert self._keeper is not None
        try:
            await self._native.refresh_token(token, app_id=self._keeper.app_id)
        except RuntimeError as error:
            # The relay ends the connection when its token expires, and the native session opens the
            # next one with this token.
            logger.warning("the relay refused the refreshed token: %s", error)

    async def _end(self, failure: BaseException | None) -> None:
        """Ends the session once, with `failure` or as closed, and waits until the session's own
        tasks finished, other than the one ending it, and the native session closed; a caller
        cancelled meanwhile leaves the close running for the next one to await. A task the shutdown
        waits for, such as the token refresh that calls `close()` from its cleanup, returns at once
        instead."""
        current = asyncio.current_task()
        if self._closing.started and current in self._joined:
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
        await self._closing.run(self._shut_down)

    async def _shut_down(self) -> None:
        # A cancelled task may still run cleanup.
        await asyncio.gather(*self._joined, return_exceptions=True)
        await self._native.close()


async def _cancel_work(running: asyncio.Future) -> None:
    """Cancels `running`, the work of `Session.run`, unless it finished, and waits until it
    finished its cleanup, even when cancelled meanwhile."""
    if not running.done():
        running.cancel()
        await wait_through_cancellation([running])
    if not running.cancelled() and (error := running.exception()) is not None:
        logger.debug("the work run in the session failed while it was cancelled: %r", error)


class _Connecting:
    """What `connect` returns: awaited, the open `Session`; used with `async with`, a session closed
    on exit, which re-raises what ended the session."""

    def __init__(self, dial: Dial, keeper: TokenKeeper | None, token: str | None):
        self._dial = dial
        self._keeper = keeper
        self._token = token
        self._session: Session | None = None

    def __await__(self) -> Generator[Any, None, Session]:
        return self._start().__await__()

    async def _start(self) -> Session:
        token = self._keeper.token if self._keeper is not None else self._token
        return Session(await self._dial(token), self._keeper)

    async def __aenter__(self) -> Session:
        self._session = await self._start()
        return self._session

    async def __aexit__(self, *exc_info: object) -> None:
        assert self._session is not None
        session = self._session
        await session.close()
        if exc_info[0] is None and session._done.exception() is not None:
            raise session._done.exception()


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
    A failure to connect raises RuntimeError.

    `refresh_token`, given with `token`, takes the current token and returns a later one of the
    same app. The session calls it five minutes before the token's `exp`, retries every 30 s, and
    ends with `TokenExpired` once the token expired. `versions` lists the drafts to offer, most
    preferred first; without it draft-18, then draft-14, are offered."""
    if refresh_token is not None and token is None:
        raise ValueError("refresh_token needs a token")
    keeper = TokenKeeper(token, refresh_token) if refresh_token is not None and token is not None else None

    async def dial(current_token: str | None) -> _native.Session:
        return await _native.connect(
            url, verify_certificate=verify_certificate, token=current_token, versions=versions
        )

    return _Connecting(dial, keeper, token)
