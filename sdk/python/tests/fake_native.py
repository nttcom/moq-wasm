"""Stand-ins for `moq_wasm._native`, so the SDK's session logic is tested without a relay."""

import asyncio
import base64
import json
from types import SimpleNamespace

from moq_wasm._session import Session, _Connecting
from moq_wasm._token import TokenKeeper

STAY_OPEN = object()


def unsigned_token(app_id: str, expires_at: float | None) -> str:
    """A JWT carrying `appId` and `exp`; the SDK reads its claims without verifying it."""

    def encode(part: dict) -> str:
        return base64.urlsafe_b64encode(json.dumps(part).encode()).rstrip(b"=").decode()

    claims = {"appId": app_id} if expires_at is None else {"appId": app_id, "exp": expires_at}
    return f"{encode({'alg': 'none'})}.{encode(claims)}.signature"


class FakeWriter:
    def __init__(self):
        self.objects: list[tuple[bytes, bool]] = []

    async def write(self, payload: bytes, *, new_group: bool = False) -> None:
        self.objects.append((payload, new_group))


class FakeSubscription:
    """Yields `payloads` as objects of group 0, then ends as a track its publisher ended; with
    `STAY_OPEN` last, waits for more instead, as a live track does."""

    def __init__(self, payloads: list):
        self._stay_open = bool(payloads) and payloads[-1] is STAY_OPEN
        payloads = payloads[:-1] if self._stay_open else payloads
        self._objects = iter(
            SimpleNamespace(group_id=0, object_id=index, payload=payload) for index, payload in enumerate(payloads)
        )
        self.closed = False

    def __aiter__(self) -> "FakeSubscription":
        return self

    async def __anext__(self):
        try:
            return next(self._objects)
        except StopIteration:
            if self._stay_open:
                await asyncio.Event().wait()
            raise StopAsyncIteration from None

    async def close(self) -> None:
        self.closed = True


class FakeRendition:
    def __init__(self, name: str):
        self.name = name
        self.calls: list[tuple] = []

    def configure_video(self, codec: str) -> None:
        self.calls.append(("configure_video", codec))

    def push_video(self, data: bytes, timestamp_micros: int) -> bool:
        self.calls.append(("push_video", data))
        return True


class FakeMediaSource:
    def __init__(self, video: str, audio: str):
        self.video = video
        self.audio = audio
        self.calls: list[tuple] = []
        self.renditions: list[FakeRendition] = []
        self.keyframe_requested = asyncio.Event()
        self.demands: asyncio.Queue = asyncio.Queue()

    def add_rendition(self, name: str, **options) -> FakeRendition:
        self.renditions.append(FakeRendition(name))
        return self.renditions[-1]

    def configure_video(self, codec: str) -> None:
        self.calls.append(("configure_video", codec))

    def push_video(self, data: bytes, timestamp_micros: int) -> bool:
        self.calls.append(("push_video", data))
        return True

    def configure_audio(self, codec: str, sample_rate: int, channels: int, *, description=None) -> None:
        described = () if description is None else (description,)
        self.calls.append(("configure_audio", codec, sample_rate, channels, *described))

    def push_audio(self, data: bytes, timestamp_micros: int, **level) -> bool:
        self.calls.append(("push_audio", data))
        return True

    async def keyframe_request(self) -> None:
        await self.keyframe_requested.wait()
        self.keyframe_requested.clear()

    async def demand_change(self):
        return await self.demands.get()

    async def remove_track(self, kind: str) -> None:
        self.calls.append(("remove_track", kind))


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

    def close(self) -> None:
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
        # While set and not released, closing a namespace watch waits for the relay's answer.
        self.watch_close_gate: asyncio.Event | None = None
        self.subscriptions: list[FakeSubscription] = []
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
        return FakeNamespaceWatch(self, prefix)

    async def subscribe(self, namespace: list[str], name: str, **options) -> FakeSubscription:
        self.calls.append(("subscribe", name))
        self.subscribe_options.append(options)
        return self._subscription(name)

    async def subscribe_current_group(self, namespace: list[str], name: str, **options) -> FakeSubscription:
        self.calls.append(("subscribe_current_group", name))
        self.subscribe_options.append(options)
        return self._subscription(name)

    def _subscription(self, name: str) -> FakeSubscription:
        if not self.tracks.get(name):
            raise RuntimeError("TrackDoesNotExist")
        subscription = FakeSubscription(self.tracks[name].pop(0))
        self.subscriptions.append(subscription)
        return subscription

    def media_broadcast(self, namespace: list[str]) -> FakeMediaBroadcast:
        self.media_broadcasts.append(FakeMediaBroadcast(self, namespace))
        return self.media_broadcasts[-1]

    async def end_track(self, namespace: list[str], name: str) -> None:
        self.calls.append(("end_track", name))
        self.ended.append((list(namespace), name))

    async def unannounce(self, namespace: list[str]) -> None:
        self.calls.append(("unannounce", namespace))

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


async def open_session(relay: FakeRelay, keeper: TokenKeeper | None = None, token: str | None = None) -> Session:
    return await _Connecting(relay.dial, keeper, token)
