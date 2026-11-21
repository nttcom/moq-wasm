"""Stand-ins for `moq_wasm._native`, so the SDK's session logic is tested without a relay."""

import asyncio
import base64
import json
from types import SimpleNamespace

from moq_wasm import Location
from moq_wasm._session import Session, _Connecting
from moq_wasm._token import RefreshToken
from moq_wasm.media import CATALOG_TRACK_NAME

STAY_OPEN = object()


def unsigned_token(app_id: str, expires_at: float | None) -> str:
    """A JWT carrying `appId` and `exp`; the SDK reads its claims without verifying it."""

    def encode(part: dict) -> str:
        return base64.urlsafe_b64encode(json.dumps(part).encode()).rstrip(b"=").decode()

    claims = {"appId": app_id} if expires_at is None else {"appId": app_id, "exp": expires_at}
    return f"{encode({'alg': 'none'})}.{encode(claims)}.signature"


def native_future(awaitable) -> asyncio.Future:
    """`awaitable` as a native request returns it: a future, which is done as soon as it is
    cancelled, unlike a task."""
    future = asyncio.get_running_loop().create_future()
    task = asyncio.ensure_future(awaitable)

    def settle(task: asyncio.Future) -> None:
        if future.done():
            return
        if task.cancelled():
            future.cancel()
        elif (error := task.exception()) is not None:
            future.set_exception(error)
        else:
            future.set_result(task.result())

    task.add_done_callback(settle)
    future.add_done_callback(lambda _: task.cancel())
    return future


class FakeWriter:
    """`next_subscriber` resolves with each value put in `subscribers`, in turn; a write raises
    `failure` once it is set, and while `send_gate` is set and not released, a write that took its
    payload waits to finish sending it."""

    def __init__(self):
        self.objects: list[tuple[bytes, bool]] = []
        self.subscribers: asyncio.Queue[bool] = asyncio.Queue()
        self.subscriber_waits = 0
        self.failure: Exception | None = None
        self.send_gate: asyncio.Event | None = None

    async def write(self, payload: bytes, *, new_group: bool = False) -> None:
        if self.failure is not None:
            raise self.failure
        self.objects.append((payload, new_group))
        if self.send_gate is not None:
            await self.send_gate.wait()

    def next_subscriber(self) -> asyncio.Future:
        self.subscriber_waits += 1
        return native_future(self.subscribers.get())


class FakeSubscription:
    """Yields `payloads` as objects of group 0, or of the group a `(group_id, payload)` pair names,
    or with the location a `(group_id, object_id, payload)` triple names, or a `SimpleNamespace`
    object as it is, then ends as a track its publisher ended; with `STAY_OPEN` last, waits for
    more instead, as a live track does. An `asyncio.Event` among them holds the objects after it
    back until it is set. `largest` is the Largest Location its SUBSCRIBE_OK reported: that of the
    last of the first `replayed` objects, as of a joining FETCH, or None without any."""

    def __init__(self, payloads: list, unsubscribe_gate: asyncio.Event | None = None, *, replayed: int = 0):
        self._unsubscribe_gate = unsubscribe_gate
        self.largest = _last_location(payloads[:replayed])
        self._stay_open = bool(payloads) and payloads[-1] is STAY_OPEN
        payloads = payloads[:-1] if self._stay_open else payloads
        self._items = iter(payloads)
        self._next_object_id = 0
        # Set once UNSUBSCRIBE is sent, and `closed` once the relay answered it.
        self.unsubscribing = False
        self.closed = False

    def __aiter__(self) -> "FakeSubscription":
        return self

    async def __anext__(self):
        try:
            item = next(self._items)
            while isinstance(item, asyncio.Event):
                await item.wait()
                item = next(self._items)
            if isinstance(item, SimpleNamespace):
                return item
            if isinstance(item, tuple) and len(item) == 3:
                group_id, object_id, payload = item
            else:
                group_id, payload = item if isinstance(item, tuple) else (0, item)
                object_id = self._next_object_id
                self._next_object_id += 1
            return SimpleNamespace(
                group_id=group_id,
                object_id=object_id,
                status=None,
                payload=payload,
                extension_headers=[],
            )
        except StopIteration:
            if self._stay_open:
                await asyncio.Event().wait()
            raise StopAsyncIteration from None

    async def close(self) -> None:
        self.unsubscribing = True
        if self._unsubscribe_gate is not None:
            await self._unsubscribe_gate.wait()
        self.closed = True


def _last_location(items: list) -> Location | None:
    """The location `FakeSubscription` gives the last object among `items`."""
    location = None
    next_object_id = 0
    for item in items:
        if isinstance(item, SimpleNamespace):
            location = (item.group_id, item.object_id)
        elif isinstance(item, tuple) and len(item) == 3:
            location = item[:2]
        elif not isinstance(item, asyncio.Event) and item is not STAY_OPEN:
            location = (item[0] if isinstance(item, tuple) else 0, next_object_id)
            next_object_id += 1
    return None if location is None else Location(*location)


class FakeRendition:
    """Its pushes raise `failure` once it is set, as a native rendition whose publishing stopped."""

    def __init__(self, name: str):
        self.name = name
        self.calls: list[tuple] = []
        self.failure: Exception | None = None

    def configure_video(self, codec: str) -> None:
        self.calls.append(("configure_video", codec))

    def push_video(self, data: bytes, timestamp_micros: int) -> bool:
        if self.failure is not None:
            raise self.failure
        self.calls.append(("push_video", data))
        return True


class FakeMediaSource:
    """Its pushes and `configure_audio` raise `failure` once it is set, as a native source whose
    publishing stopped."""

    def __init__(self, video: str, audio: str):
        self.video = video
        self.audio = audio
        self.failure: Exception | None = None
        self.calls: list[tuple] = []
        self.renditions: list[FakeRendition] = []
        self.keyframe_requested = asyncio.Event()
        self.demands: asyncio.Queue = asyncio.Queue()
        self.remove_gate: asyncio.Event | None = None

    def add_rendition(self, name: str, **options) -> FakeRendition:
        self.renditions.append(FakeRendition(name))
        return self.renditions[-1]

    def configure_video(self, codec: str) -> None:
        self.calls.append(("configure_video", codec))

    def push_video(self, data: bytes, timestamp_micros: int) -> bool:
        if self.failure is not None:
            raise self.failure
        self.calls.append(("push_video", data))
        return True

    def configure_audio(self, codec: str, sample_rate: int, channels: int, *, description=None) -> None:
        if self.failure is not None:
            raise self.failure
        described = () if description is None else (description,)
        self.calls.append(("configure_audio", codec, sample_rate, channels, *described))

    def push_audio(self, data: bytes, timestamp_micros: int, **level) -> bool:
        if self.failure is not None:
            raise self.failure
        self.calls.append(("push_audio", data))
        return True

    def keyframe_request(self) -> asyncio.Future:
        return native_future(self._keyframe_request())

    async def _keyframe_request(self) -> None:
        await self.keyframe_requested.wait()
        self.keyframe_requested.clear()

    def demand_change(self) -> asyncio.Future:
        return native_future(self.demands.get())

    def remove_track(self, kind: str) -> asyncio.Future:
        """While `remove_gate` is set and not released, the track's removal waits before the
        renditions' removals, which a cancelled future drops, as the native one does."""
        return native_future(self._remove_track(kind))

    async def _remove_track(self, kind: str) -> None:
        self.calls.append(("remove_track", kind))
        if self.remove_gate is not None:
            await self.remove_gate.wait()
        if kind == "video":
            self.calls.append(("remove_renditions",))


class FakeMediaBroadcast:
    def __init__(self, session: "FakeNativeSession", namespace: list[str]):
        self._session = session
        self._namespace = namespace
        self.sources: list[FakeMediaSource] = []

    def add_source(self, video: str, audio: str, **options) -> FakeMediaSource:
        source = FakeMediaSource(video, audio)
        source.options = options
        self.sources.append(source)
        return source

    async def announce(self) -> None:
        self._session.calls.append(("media_announce", self._namespace))
        if self._session.media_announce_gate is not None:
            await self._session.media_announce_gate.wait()
        if self._session.media_announce_failure is not None:
            raise self._session.media_announce_failure

    async def close(self) -> None:
        self._session.calls.append(("media_close", self._namespace))


class FakeNamespaceWatch:
    """The namespace events `FakeNativeSession.publish` sends, until `close`."""

    def __init__(self, session: "FakeNativeSession", prefix: list[str]):
        self._session = session
        self._prefix = prefix

    def __aiter__(self) -> "FakeNamespaceWatch":
        return self

    async def __anext__(self):
        return await self._session._namespace_events.get()

    async def close(self) -> None:
        self._session.calls.append(("unsubscribe_namespace", self._prefix))
        if self._session.watch_close_gate is not None:
            await self._session.watch_close_gate.wait()


class FakeNativeSession:
    """One native session. `calls` lists its requests in order; `drop()` ends it as one that gave up
    replacing its connection.
    `tracks[name]` lists, for each SUBSCRIBE to `name` in turn, the payloads it yields; a SUBSCRIBE
    beyond them fails."""

    def __init__(self, token: str | None):
        self.token = token
        self.tracks: dict[str, list[list[bytes]]] = {}
        # While set and not released, PUBLISH_NAMESPACE waits for the relay's answer.
        self.announce_gate: asyncio.Event | None = None
        # While set and not released, a media broadcast's PUBLISH_NAMESPACE waits for the relay's
        # answer, which refuses it with `media_announce_failure` once that is set.
        self.media_announce_gate: asyncio.Event | None = None
        self.media_announce_failure: Exception | None = None
        # While set and not released, SUBSCRIBE waits for the relay's answer.
        self.subscribe_gate: asyncio.Event | None = None
        # Whether SUBSCRIBE_NAMESPACE is refused.
        self.refuse_watch = False
        # For each track name, the gate UNSUBSCRIBE waits on for the relay's answer until released.
        self.unsubscribe_gates: dict[str, asyncio.Event] = {}
        # While set and not released, closing a namespace watch waits for the relay's answer.
        self.watch_close_gate: asyncio.Event | None = None
        # For each track name, the objects of the current group the relay holds, which a joining
        # FETCH (`subscribe_current_group`) brings before those `tracks` lists.
        self.current_groups: dict[str, list] = {}
        self.subscriptions: list[FakeSubscription] = []
        # The namespace and name of each SUBSCRIBE to a track other than the catalog, which
        # `RemoteBroadcast.catalogs` subscribes to.
        self.subscribed: list[tuple[list[str], str]] = []
        self.media_broadcasts: list[FakeMediaBroadcast] = []
        self.calls: list[tuple] = []
        self.subscribe_options: list[dict] = []
        self.writers: dict[str, FakeWriter] = {}
        self.publish_options: dict[str, dict] = {}
        self.refreshed: list[str] = []
        # The namespace and name of each `end_track`.
        self.ended: list[tuple[list[str], str]] = []
        self._closed = asyncio.get_running_loop().create_future()
        self._namespace_events: asyncio.Queue = asyncio.Queue()

    def publish(self, namespace: list[str], *, active: bool = True) -> None:
        self._namespace_events.put_nowait(SimpleNamespace(namespace=namespace, active=active))

    def drop(self, code: int | None = None, reason: str = "the connection was lost") -> None:
        if not self._closed.done():
            self._closed.set_result(SimpleNamespace(code=code, reason=reason))

    async def publish_track(self, namespace: list[str], name: str, **options) -> FakeWriter:
        self.calls.append(("publish_track", name))
        self.publish_options[name] = options
        return self.writers.setdefault(name, FakeWriter())

    async def announce(self, namespace: list[str]) -> None:
        self.calls.append(("announce", namespace))
        if self.announce_gate is not None:
            await self.announce_gate.wait()

    async def watch_namespaces(self, prefix: list[str]):
        self.calls.append(("watch_namespaces", prefix))
        if self.refuse_watch:
            raise RuntimeError("NamespacePrefixUnknown")
        return FakeNamespaceWatch(self, prefix)

    async def subscribe(self, namespace: list[str], name: str, **options) -> FakeSubscription:
        self.calls.append(("subscribe", name))
        self.subscribe_options.append(options)
        self.subscribed.append((namespace, name))
        if self.subscribe_gate is not None:
            await self.subscribe_gate.wait()
        return self._subscription(name)

    async def subscribe_current_group(self, namespace: list[str], name: str, **options) -> FakeSubscription:
        self.calls.append(("subscribe_current_group", name))
        self.subscribe_options.append(options)
        if name != CATALOG_TRACK_NAME:
            self.subscribed.append((namespace, name))
        if self.subscribe_gate is not None:
            await self.subscribe_gate.wait()
        return self._subscription(name, self.current_groups.get(name, []))

    def _subscription(self, name: str, current_group: list | tuple = ()) -> FakeSubscription:
        if not self.tracks.get(name):
            raise RuntimeError("TrackDoesNotExist")
        subscription = FakeSubscription(
            [*current_group, *self.tracks[name].pop(0)], self.unsubscribe_gates.get(name), replayed=len(current_group)
        )
        self.subscriptions.append(subscription)
        return subscription

    def media_broadcast(self, namespace: list[str]) -> FakeMediaBroadcast:
        self.media_broadcasts.append(FakeMediaBroadcast(self, namespace))
        return self.media_broadcasts[-1]

    async def end_track(self, namespace: list[str], name: str) -> None:
        self.calls.append(("end_track", name))
        self.ended.append((list(namespace), name))

    async def unannounce(self, namespace: list[str]) -> None:
        self.calls.append(("unannounce", list(namespace)))

    async def refresh_token(self, token: str, *, app_id: str) -> None:
        self.refreshed.append(token)

    async def closed(self):
        return await self._closed

    async def close(self) -> None:
        self.drop()


class FakeRelay:
    """Opens a `FakeNativeSession` for each dial; `refuse_next` makes the next dials fail."""

    def __init__(self):
        self.sessions: list[FakeNativeSession] = []
        self.refuse_next = 0

    async def dial(self, token: str | None) -> FakeNativeSession:
        if self.refuse_next > 0:
            self.refuse_next -= 1
            raise RuntimeError("connection refused")
        session = FakeNativeSession(token)
        self.sessions.append(session)
        return session


async def called(native: FakeNativeSession, call: tuple) -> None:
    while call not in native.calls:
        await asyncio.sleep(0.01)


async def until(condition) -> None:
    while not condition():
        await asyncio.sleep(0.01)


async def open_session(relay: FakeRelay, token: str | None = None, refresh: RefreshToken | None = None) -> Session:
    return await _Connecting(relay.dial, token, refresh)
