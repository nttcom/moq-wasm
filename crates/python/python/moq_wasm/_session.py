import asyncio
import logging
from collections.abc import AsyncIterator, Awaitable, Callable, Generator
from dataclasses import dataclass
from typing import Any, TypeVar

from moq_wasm import _native
from moq_wasm._token import RefreshToken, TokenKeeper

logger = logging.getLogger("moq_wasm")

FIRST_RECONNECT_DELAY_SECONDS = 1.0
MAX_RECONNECT_DELAY_SECONDS = 30.0

T = TypeVar("T")
Dial = Callable[[str | None], Awaitable[_native.Session]]
MediaRestorer = Callable[[_native.Session], Awaitable[None]]


class SessionClosed(Exception):
    """The session was closed with `close()` while `Session.run` waited."""


@dataclass(frozen=True)
class NamespaceEvent:
    namespace: list[str]
    active: bool
    """False once the namespace was withdrawn (PUBLISH_NAMESPACE_DONE), or its connection ended."""


class NamespaceWatch(AsyncIterator[NamespaceEvent]):
    """The namespaces published and withdrawn under a prefix, across the session's connections.
    A namespace still published when a connection ends is reported withdrawn, and published again
    once the next connection's SUBSCRIBE_NAMESPACE reports it."""

    def __init__(self, prefix: list[str]):
        self.prefix = prefix
        self._events: asyncio.Queue[NamespaceEvent | None] = asyncio.Queue()
        self._published: set[tuple[str, ...]] = set()
        self._pump: asyncio.Task | None = None
        self._ended = False

    def __aiter__(self) -> "NamespaceWatch":
        return self

    async def __anext__(self) -> NamespaceEvent:
        event = await self._events.get()
        if event is None:
            self._events.put_nowait(None)
            raise StopAsyncIteration
        return event

    def close(self) -> None:
        """Ends the iteration. The relay keeps the SUBSCRIBE_NAMESPACE until the connection ends."""
        self._end()

    def _attach(self, events: _native.NamespaceWatch) -> None:
        self._pump = asyncio.create_task(self._forward(events))

    async def _forward(self, events: _native.NamespaceWatch) -> None:
        async for event in events:
            key = tuple(event.namespace)
            if event.active:
                self._published.add(key)
            else:
                self._published.discard(key)
            self._events.put_nowait(NamespaceEvent(event.namespace, event.active))

    def _detach(self) -> None:
        if self._pump is not None:
            self._pump.cancel()
            self._pump = None
        for namespace in sorted(self._published):
            self._events.put_nowait(NamespaceEvent(list(namespace), False))
        self._published.clear()

    def _end(self) -> None:
        if self._ended:
            return
        self._ended = True
        if self._pump is not None:
            self._pump.cancel()
        self._events.put_nowait(None)


class TrackWriter:
    """A track the session publishes, registered again on each connection. A new connection starts
    the track's groups afresh: what was written before is not served from it."""

    def __init__(self, namespace: list[str], name: str, retention_seconds: float):
        self.namespace = namespace
        self.name = name
        self.retention_seconds = retention_seconds
        self._writer: _native.TrackWriter | None = None
        # Called each time a connection registers the track.
        self._on_bound: list[Callable[[], Awaitable[None]]] = []

    async def write(self, payload: bytes, *, new_group: bool = False) -> None:
        """The object is cached for FETCH whether or not a subscriber takes it. While the session
        reconnects, it is dropped."""
        if self._writer is not None:
            await self._writer.write(payload, new_group=new_group)

    async def _bind(self, writer: _native.TrackWriter) -> None:
        self._writer = writer
        for bound in self._on_bound:
            await bound()


class Announcement:
    """A namespace the session publishes on each connection until `close()`."""

    def __init__(self, session: "Session", namespace: list[str]):
        self.namespace = namespace
        self._session = session

    async def close(self) -> None:
        """PUBLISH_NAMESPACE_DONE; the session stops publishing the namespace."""
        await self._session._unannounce(self.namespace)


class Session:
    """A relay session that stays usable across connections: when a connection ends other than by
    `close()`, it dials again with backoff and registers its tracks, publishes its namespaces and
    watches its prefixes again. A subscription ends with its connection."""

    def __init__(self, dial: Dial, keeper: TokenKeeper | None, first_token: str | None):
        self._dial = dial
        self._keeper = keeper
        self._token = first_token
        self._native: _native.Session | None = None
        self._writers: list[TrackWriter] = []
        self._announced: list[list[str]] = []
        self._media: dict[tuple[str, ...], MediaRestorer] = {}
        self._watches: list[NamespaceWatch] = []
        self._state = asyncio.Lock()
        self._done: asyncio.Future[None] = asyncio.get_running_loop().create_future()
        self._tasks: list[asyncio.Task] = []

    async def _open(self) -> None:
        self._native = await self._dial(self._current_token())
        self._tasks.append(asyncio.create_task(self._stay_connected()))
        if self._keeper is not None:
            self._tasks.append(asyncio.create_task(self._keep_token_fresh(self._keeper)))

    def _current_token(self) -> str | None:
        return self._keeper.token if self._keeper is not None else self._token

    async def subscribe(self, namespace: list[str], name: str) -> _native.Subscription:
        """Only the objects published after the SUBSCRIBE. Raises RuntimeError while the session
        reconnects."""
        return await self._connected().subscribe(namespace, name)

    async def subscribe_current_group(self, namespace: list[str], name: str) -> _native.Subscription:
        """The objects of the group the relay holds come first (a joining FETCH), then the live
        ones. Raises RuntimeError while the session reconnects."""
        return await self._connected().subscribe_current_group(namespace, name)

    async def watch_namespaces(self, prefix: list[str]) -> NamespaceWatch:
        """Raises RuntimeError when the relay refuses the SUBSCRIBE_NAMESPACE."""
        watch = NamespaceWatch(prefix)
        async with self._state:
            self._check_open()
            if self._native is not None:
                watch._attach(await self._native.watch_namespaces(prefix))
            self._watches.append(watch)
        return watch

    async def announce(self, namespace: list[str]) -> Announcement:
        """PUBLISH_NAMESPACE, sent again on each connection. Raises RuntimeError when the relay
        refuses it."""
        async with self._state:
            self._check_open()
            if self._native is not None:
                await self._native.announce(namespace)
            self._announced.append(namespace)
        return Announcement(self, namespace)

    async def publish_track(self, namespace: list[str], name: str, *, retention_seconds: float) -> TrackWriter:
        """Registers a track that the session answers SUBSCRIBE and FETCH for. Register it before
        `announce`, since the relay may forward a SUBSCRIBE at once."""
        writer = TrackWriter(namespace, name, retention_seconds)
        await self._register(writer)
        return writer

    async def _register(self, writer: TrackWriter) -> None:
        async with self._state:
            self._check_open()
            if self._native is not None:
                await writer._bind(
                    await self._native.publish_track(
                        writer.namespace, writer.name, retention_seconds=writer.retention_seconds
                    )
                )
            self._writers.append(writer)

    async def _add_media_broadcast(self, namespace: list[str], restorer: MediaRestorer) -> Announcement:
        """Publishes `namespace` through `restorer`, which builds the media broadcast on a
        connection and announces it, now and on each new connection."""
        async with self._state:
            self._check_open()
            if self._native is not None:
                await restorer(self._native)
            self._media[tuple(namespace)] = restorer
        return Announcement(self, namespace)

    async def _unannounce(self, namespace: list[str]) -> None:
        async with self._state:
            if namespace in self._announced:
                self._announced.remove(namespace)
            elif self._media.pop(tuple(namespace), None) is None:
                return
            if self._native is not None:
                await self._native.unannounce(namespace)

    def _publishes(self, namespace: list[str]) -> bool:
        return namespace in self._announced or tuple(namespace) in self._media

    async def run(self, work: Awaitable[T]) -> T:
        """Awaits `work`, cancelling it when the session ends. Raises what ended the session, such as
        `moq_wasm.TokenExpired`, or `SessionClosed` after `close()`."""
        running = asyncio.ensure_future(work)
        try:
            await asyncio.wait([running, self._done], return_when=asyncio.FIRST_COMPLETED)
            if running.done():
                return running.result()
            self._done.result()
            raise SessionClosed("the session was closed")
        finally:
            running.cancel()

    async def closed(self) -> None:
        """Waits until the session ends for good: `close()`, or a failure it cannot recover from,
        which this raises."""
        await asyncio.shield(self._done)

    async def close(self) -> None:
        await self._end(None)

    def _connected(self) -> _native.Session:
        self._check_open()
        if self._native is None:
            raise RuntimeError("the session is reconnecting")
        return self._native

    def _check_open(self) -> None:
        if self._done.done():
            raise RuntimeError("the session closed")

    async def _stay_connected(self) -> None:
        while True:
            assert self._native is not None
            await self._native.closed()
            logger.warning("relay connection closed; reconnecting")
            async with self._state:
                self._native = None
                for watch in self._watches:
                    watch._detach()
            native = await self._redial()
            async with self._state:
                await self._restore(native)
            logger.info("relay connection restored")

    async def _redial(self) -> _native.Session:
        delay = FIRST_RECONNECT_DELAY_SECONDS
        while True:
            await asyncio.sleep(delay)
            try:
                return await self._dial(self._current_token())
            except RuntimeError as error:
                logger.warning("cannot reconnect to the relay: %s", error)
                delay = min(delay * 2, MAX_RECONNECT_DELAY_SECONDS)

    async def _restore(self, native: _native.Session) -> None:
        """Registers the tracks before the namespaces are published, since the relay may forward a
        SUBSCRIBE at once."""
        for writer in self._writers:
            await writer._bind(
                await native.publish_track(writer.namespace, writer.name, retention_seconds=writer.retention_seconds)
            )
        for namespace, restorer in self._media.items():
            try:
                await restorer(native)
            except RuntimeError as error:
                logger.warning("media broadcast %s refused after reconnecting: %s", list(namespace), error)
        for namespace in self._announced:
            try:
                await native.announce(namespace)
            except RuntimeError as error:
                logger.warning("PUBLISH_NAMESPACE %s refused after reconnecting: %s", namespace, error)
        for watch in self._watches:
            try:
                watch._attach(await native.watch_namespaces(watch.prefix))
            except RuntimeError as error:
                logger.warning("SUBSCRIBE_NAMESPACE %s refused after reconnecting: %s", watch.prefix, error)
        self._native = native

    async def _keep_token_fresh(self, keeper: TokenKeeper) -> None:
        try:
            await keeper.keep_fresh(self._hand_over_token)
        except Exception as error:
            asyncio.create_task(self._end(error))

    async def _hand_over_token(self, token: str) -> None:
        assert self._keeper is not None
        native = self._native
        if native is None:
            return
        try:
            await native.refresh_token(token, app_id=self._keeper.app_id)
        except RuntimeError as error:
            # The connection expires with its current token and is opened again with this one.
            logger.warning("the relay refused the refreshed token: %s", error)

    async def _end(self, failure: BaseException | None) -> None:
        if self._done.done():
            return
        if failure is None:
            self._done.set_result(None)
        else:
            self._done.set_exception(failure)
            # Retrieved by `closed()` or `run()`; a session nobody awaits does not warn.
            self._done.exception()
        for task in self._tasks:
            if task is not asyncio.current_task():
                task.cancel()
        for watch in self._watches:
            watch._end()
        native, self._native = self._native, None
        if native is not None:
            await native.close()


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
        session = Session(self._dial, self._keeper, self._token)
        await session._open()
        return session

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
    The first connection's failure raises RuntimeError; later ones are retried with backoff.

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
