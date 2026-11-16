"""The native module of the moq-wasm Python SDK (docs/architecture/python)."""

from collections.abc import AsyncIterator

async def connect(
    url: str,
    *,
    verify_certificate: bool = True,
    token: str | None = None,
    versions: list[str] | None = None,
) -> Session:
    """`moqt://` connects over raw QUIC, `https://` over WebTransport, presenting `token` (a JWT) in
    CLIENT_SETUP. The session follows the relay's GOAWAY with the latest token it was given.
    `versions` lists the drafts to offer, most preferred first, each `"draft-14"` or `"draft-18"`;
    without it draft-18, then draft-14, are offered. An unknown name raises `ValueError`, and an empty
    list fails the connect."""

def parse_catalog(payload: bytes) -> dict:
    """Raises `ValueError` when `payload` is not an independent MSF catalog: malformed, of a
    `version` other than `"draft-01"` (draft-ietf-moq-msf-01 §5.1.1), or a delta update, which the
    bindings do not apply."""

class Session:
    async def watch_namespaces(self, prefix: list[str]) -> NamespaceWatch:
        """Namespaces published and withdrawn under `prefix`, including those already published."""
    async def subscribe(self, namespace: list[str], name: str) -> Subscription:
        """Only the objects published after the SUBSCRIBE."""
    async def subscribe_current_group(self, namespace: list[str], name: str) -> Subscription:
        """The objects of the group the relay holds come first (a joining FETCH), then the live ones."""
    async def announce(self, namespace: list[str]) -> None: ...
    def media_broadcast(self, namespace: list[str]) -> MediaBroadcast:
        """A broadcast of encoded media in `namespace`; its `announce` publishes the namespace
        instead of `announce`."""
    async def unannounce(self, namespace: list[str]) -> None:
        """PUBLISH_NAMESPACE_DONE for a namespace `announce` published."""
    async def publish_track(self, namespace: list[str], name: str, *, retention_seconds: float) -> TrackWriter:
        """Registers a track that the session answers SUBSCRIBE and FETCH for. Register it before
        `announce`, since the relay may forward a SUBSCRIBE at once."""
    async def refresh_token(self, token: str, *, app_id: str) -> None:
        """Hands the relay `token`, a later token of the same app, before the current one expires
        (a TRACK_STATUS for `[app_id]` that carries it). Raises RuntimeError for a session connected
        without a token or a token the relay refuses; the session then keeps its current token."""
    async def closed(self) -> None:
        """Waits until the relay closes the session."""
    async def close(self) -> None: ...

class NamespaceEvent:
    namespace: list[str]
    active: bool
    """False once the namespace was withdrawn (PUBLISH_NAMESPACE_DONE)."""

class NamespaceWatch(AsyncIterator[NamespaceEvent]):
    def __aiter__(self) -> NamespaceWatch: ...
    async def __anext__(self) -> NamespaceEvent: ...

class Object:
    group_id: int
    object_id: int
    payload: bytes

class Subscription(AsyncIterator[Object]):
    def __aiter__(self) -> Subscription: ...
    async def __anext__(self) -> Object:
        """Raises StopAsyncIteration once the publisher ends the track. A group whose stream
        fails (reset by a publisher that left, or cut by a GOAWAY's cutover) ends early."""
    async def close(self) -> None:
        """UNSUBSCRIBE; the iteration ends once the relay stops the track."""

class TrackWriter:
    async def write(self, payload: bytes, *, new_group: bool = False) -> None:
        """The object is cached for FETCH whether or not a subscriber takes it."""

class MediaBroadcast:
    async def announce(self) -> None: ...
    def close(self) -> None:
        """Stops answering for the broadcast's tracks; withdraw the namespace with `unannounce`
        first."""
    def add_source(
        self,
        video: str,
        audio: str,
        *,
        label: str,
        video_bitrate: int | None = None,
        video_framerate: int | None = None,
    ) -> MediaSource:
        """A source on the tracks `video` and `audio`, with the media timeline `<video>_timeline`."""

class MediaSource:
    def configure_video(self, codec: str) -> None:
        """`codec` is a WebCodecs codec string (`avc1.*`, `hvc1.*`, `vp8`, `vp09.*`, `av01.*`);
        H.264 and H.265 frames are Annex B. Raises ValueError for another codec."""
    def push_video(self, data: bytes, timestamp_micros: int) -> None:
        """One encoded frame; a keyframe opens a group. Raises RuntimeError before
        `configure_video`."""
    def configure_audio(self, codec: str, sample_rate: int, channels: int) -> None:
        """Only `"opus"`; raises ValueError for another codec."""
    def push_audio(self, data: bytes, timestamp_micros: int) -> None: ...
    async def keyframe_request(self) -> None:
        """Resolves the next time a subscriber asks for a new group (NEW_GROUP_REQUEST) or the
        broadcast needs one, as after dropping frames."""
