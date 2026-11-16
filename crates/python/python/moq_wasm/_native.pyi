"""The native module of the moq-wasm Python SDK (docs/architecture/python)."""

from collections.abc import AsyncIterator
from typing import Literal

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
        """SUBSCRIBE. `largest_object` starts after the largest object, `next_group_start` at the next
        group. With `forward=False` the relay sends nothing until `set_forward(True)`. `priority` is
        the Subscriber Priority, 128 by default: the relay sends a lower value first.
        `delivery_timeout` is in seconds: the relay stops sending an object that long after receiving
        it. An unknown `filter` or `group_order` raises ValueError, and a SUBSCRIBE the relay refuses
        RuntimeError."""
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
        """Subscribes from the first object of a group: the current group through a joining FETCH,
        or, for a track with dynamic groups whose largest object ID is `new_group_after` or more, a new
        group it asks the publisher for (NEW_GROUP_REQUEST). The options are as in `subscribe`."""
    async def fetch(self, namespace: list[str], name: str, start: Location, end: Location) -> Fetch:
        """A Standalone FETCH from `start` to `end`, returned once the response stream arrives.
        `end.object_id` is the last object plus 1, and 0 means the whole group. Raises RuntimeError
        when the relay refuses the FETCH or the stream does not arrive."""
    async def track_status(self, namespace: list[str], name: str) -> Location | None:
        """TRACK_STATUS: the track's largest location, or None while it has none. Raises
        RuntimeError when the relay refuses it."""
    async def subscribe_current_group(self, namespace: list[str], name: str) -> Subscription:
        """The objects of the group the relay holds come first (a joining FETCH), then the live ones."""
    async def announce(self, namespace: list[str]) -> None: ...
    def media_broadcast(self, namespace: list[str]) -> MediaBroadcast:
        """A broadcast of encoded media in `namespace`; its `announce` publishes the namespace
        instead of `announce`."""
    async def unannounce(self, namespace: list[str]) -> None:
        """PUBLISH_NAMESPACE_DONE for a namespace `announce` published."""
    async def publish_track(
        self,
        namespace: list[str],
        name: str,
        *,
        retention_seconds: float,
        publish: bool,
        datagram: bool,
    ) -> TrackWriter:
        """Registers a track that the session answers SUBSCRIBE and FETCH for, keeping each object
        `retention_seconds` to answer FETCH; 0 keeps only the newest. Register it before `announce`,
        since the relay may forward a SUBSCRIBE at once; with `publish`, announce first: it also
        sends PUBLISH, so the relay takes the track before anyone subscribes. `datagram` sends each
        object in its own datagram: a lost one is not resent, one larger than the path's datagram
        size (about 1 KB is safe) is dropped, and a subscriber reads it by object, not with
        `groups()`. Raises RuntimeError when the relay refuses the PUBLISH or, with `datagram`, when
        the track was already written to on streams."""
    async def refresh_token(self, token: str, *, app_id: str) -> None:
        """Hands the relay `token`, a later token of the same app, before the current one expires
        (a TRACK_STATUS for `[app_id]` that carries it); the session presents it on the connections it
        opens from now on. Raises RuntimeError for a session connected without a token or a token the
        relay refuses; the relay then keeps the connection's token until it expires."""
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

class Location:
    group_id: int
    object_id: int
    def __init__(self, group_id: int, object_id: int) -> None: ...

class Object:
    group_id: int
    subgroup_id: int
    """0 for an object that arrived in a datagram."""
    object_id: int
    status: int | None
    """The Object Status of an object without a payload (3 is End of Group); None for one with a
    payload."""
    payload: bytes

class PublishDone:
    code: int
    """The PUBLISH_DONE code as draft-14 numbers it (§13.1.3), also on a draft-18 session."""
    reason: str

class Subscription(AsyncIterator[Object]):
    def __aiter__(self) -> Subscription: ...
    async def __anext__(self) -> Object:
        """Raises StopAsyncIteration once the publisher ends the track. A group whose stream
        fails (reset by a publisher that left, or cut by a GOAWAY's cutover) ends early."""
    @property
    def largest(self) -> Location | None:
        """The largest location SUBSCRIBE_OK reported."""
    async def done(self) -> PublishDone | None:
        """Resolves once the publisher ends the subscription with PUBLISH_DONE, or to None once the
        session ends or `close()` is called without one."""
    def groups(self) -> Groups:
        """The subgroup streams, each iterated for its objects, status objects included. A
        subscription is read either by object or by group, whichever comes first; the other raises
        RuntimeError, as does a track delivered in datagrams."""
    async def set_forward(self, forward: bool) -> None:
        """SUBSCRIBE_UPDATE with the Forward flag. On draft-18 it returns once the relay applied the
        update, so an object published afterwards is delivered; draft-14 defines no answer, so there
        an object published right after it may still be withheld. Raises RuntimeError when the relay
        refuses the update or does not answer it within 10 s."""
    async def set_priority(self, priority: int) -> None:
        """SUBSCRIBE_UPDATE with the Subscriber Priority; returns and raises as `set_forward`."""
    async def request_new_group(self, largest_group: int | None = None) -> bool:
        """Asks the publisher for a new group (NEW_GROUP_REQUEST); `largest_group` is the largest
        group ID seen. Returns False, without sending, for a track without dynamic groups."""
    async def end_before_group(self, group_id: int) -> None:
        """SUBSCRIBE_UPDATE ending the subscription before `group_id`; the publisher sends
        PUBLISH_DONE once it has sent the groups below it."""
    async def close(self) -> None:
        """UNSUBSCRIBE; the iteration ends once the relay stops the track."""

class Groups(AsyncIterator[Group]):
    def __aiter__(self) -> Groups: ...
    async def __anext__(self) -> Group:
        """Raises StopAsyncIteration once the track has no more streams."""

class Group(AsyncIterator[Object]):
    group_id: int
    subgroup_id: int
    def __aiter__(self) -> Group: ...
    async def __anext__(self) -> Object:
        """Raises StopAsyncIteration once the publisher finished the stream, and RuntimeError when
        the stream was reset."""

class Fetch(AsyncIterator[Object]):
    """The objects of one FETCH response that carry a payload."""
    def __aiter__(self) -> Fetch: ...
    async def __anext__(self) -> Object:
        """Raises StopAsyncIteration once the response ends."""

class TrackWriter:
    async def write(self, payload: bytes, *, new_group: bool = False) -> int | None:
        """The object is cached for FETCH whether or not a subscriber takes it. Returns its group id
        when a subscriber took it, and None otherwise; an object within a group that was never
        started is dropped."""
    async def write_group(self, payload: bytes) -> int | None:
        """Sends `payload` as a group of its own, the layout of an MSF catalog. Returns the group id
        when a subscriber took it, and None otherwise."""
    async def next_subscriber(self) -> bool:
        """True once a SUBSCRIBE for the track is answered, False once the session has ended: the
        moment to send what a new subscriber needs, since its SUBSCRIBE starts after the largest
        object."""

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
