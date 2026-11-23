"""The native module of the moq-wasm Python SDK (docs/architecture/python). Its awaitable methods
return `asyncio.Future`s: await them, or pass them to `asyncio.ensure_future`, not to
`asyncio.create_task`."""

import asyncio
from collections.abc import AsyncIterator
from typing import Literal

def connect(
    url: str,
    *,
    verify_certificate: bool = True,
    token: str | None = None,
    versions: list[str] | None = None,
) -> asyncio.Future[Session]:
    """`moqt://` connects over raw QUIC, `https://` over WebTransport, presenting `token` (a JWT) in
    CLIENT_SETUP. The session follows the relay's GOAWAY with the latest token it was given.
    `versions` lists the drafts to offer, most preferred first, each `"draft-14"` or `"draft-18"`;
    without it draft-18, then draft-14, are offered. An unknown name raises `ValueError`, and an empty
    list fails the connect."""

def check_audio_specific_config(description: bytes) -> None:
    """Raises ValueError for bytes that are no AudioSpecificConfig (ISO/IEC 14496-3 §1.6.2.1)."""

def set_log_sink(level: str, spans: bool) -> None:
    """Installs the process-wide `tracing` subscriber that queues every event at `level` (`error`,
    `warn`, `info`, `debug` or `trace`) or above as a log record and, with `spans`, every closed INFO
    or higher span as a span record, which then carries the trace of the requests that send
    TRACE_CONTEXT. Raises ValueError for an unknown level and RuntimeError when a subscriber is
    already set."""

def wait_log_records() -> list[tuple[str, str, str | None]]:
    """The records queued since the last call, as `(kind, json, token)` with `kind` `"log"` or
    `"span"` and the token of the session the record belongs to, led by a `log records dropped`
    warning when the queue was full meanwhile; waits half a second for the first one, releasing
    the GIL, and returns an empty list after that. Raises RuntimeError before `set_log_sink`."""

def parse_catalog(payload: bytes) -> dict:
    """Raises `ValueError` when `payload` is not an independent MSF catalog: malformed, of a
    `version` other than `"draft-02"` (draft-ietf-moq-msf-02 §5.1.1), a delta update, which the
    bindings do not apply, or invalid as `msf::Catalog::parse` checks (§5.2.2, §5.2.3, §5.2.4.1,
    §5.2.15, §8.3)."""

class Session:
    """`watch_namespaces`, `subscribe`, `subscribe_current_group`, `subscribe_group_start`,
    `fetch`, `track_status` and `announce` raise `RequestError` when the relay refuses the request,
    does not answer it within 10 s or the session ends first."""
    def watch_namespaces(self, prefix: list[str]) -> asyncio.Future[NamespaceWatch]:
        """Namespaces published and withdrawn under `prefix`, including those already published."""
    def subscribe(
        self,
        namespace: list[str],
        name: str,
        *,
        filter: Literal["largest_object", "next_group_start"] = "largest_object",
        forward: bool = True,
        priority: int | None = None,
        delivery_timeout: float | None = None,
        group_order: Literal["ascending", "descending"] = "ascending",
    ) -> asyncio.Future[Subscription]:
        """SUBSCRIBE. `largest_object` starts after the largest object, `next_group_start` at the next
        group. With `forward=False` the relay sends nothing until `set_forward(True)`. `priority` is
        the Subscriber Priority, 128 by default: the relay sends a lower value first.
        `delivery_timeout` is in seconds: the relay stops sending an object that long after receiving
        it. An unknown `filter` or `group_order` raises ValueError."""
    def subscribe_group_start(
        self,
        namespace: list[str],
        name: str,
        new_group_after: int,
        *,
        priority: int | None = None,
        delivery_timeout: float | None = None,
        group_order: Literal["ascending", "descending"] = "ascending",
    ) -> asyncio.Future[Subscription]:
        """Subscribes from the first object of a group: the current group through a joining FETCH,
        or, for a track with dynamic groups whose largest object ID is `new_group_after` or more, a new
        group it asks the publisher for (NEW_GROUP_REQUEST). The options are as in `subscribe`."""
    def fetch(self, namespace: list[str], name: str, start: Location, end: Location) -> asyncio.Future[Fetch]:
        """A Standalone FETCH from `start` to `end`, returned once the response stream arrives.
        `end.object_id` is the last object plus 1, and 0 means the whole group. Raises RuntimeError
        when the response stream does not arrive."""
    def track_status(self, namespace: list[str], name: str) -> asyncio.Future[Location | None]:
        """TRACK_STATUS: the track's largest location, or None while it has none."""
    def subscribe_current_group(
        self,
        namespace: list[str],
        name: str,
        *,
        priority: int | None = None,
        delivery_timeout: float | None = None,
        group_order: Literal["ascending", "descending"] = "ascending",
    ) -> asyncio.Future[Subscription]:
        """The objects of the group the relay holds come first (a joining FETCH), then the live ones.
        The options are as in `subscribe`."""
    def announce(self, namespace: list[str]) -> asyncio.Future[None]: ...
    def media_broadcast(self, namespace: list[str]) -> MediaBroadcast:
        """A broadcast of encoded media in `namespace`; its `announce` publishes the namespace
        instead of `announce`."""
    def unannounce(self, namespace: list[str]) -> asyncio.Future[None]:
        """PUBLISH_NAMESPACE_DONE for a namespace `announce` published."""
    def publish_track(
        self,
        namespace: list[str],
        name: str,
        *,
        retention_seconds: float,
        publish: bool,
        datagram: bool,
        priority: int,
    ) -> asyncio.Future[TrackWriter]:
        """Registers a track that the session answers SUBSCRIBE and FETCH for, keeping each object
        `retention_seconds` to answer FETCH; 0 keeps only the newest. Register it before `announce`,
        since the relay may forward a SUBSCRIBE at once; with `publish`, announce first: it also
        sends PUBLISH, so the relay takes the track before anyone subscribes. `datagram` sends each
        object in its own datagram: a lost one is not resent, one larger than the path's datagram
        size (about 1 KB is safe) is dropped, and a subscriber reads it by object, not with
        `groups()`. `priority` is the Publisher Priority: among the session's tracks, a lower value
        is sent first. Raises `RequestError` when the relay refuses the PUBLISH, ValueError when the
        track already has an object and `datagram` differs from its earlier registration, and
        OverflowError for a `priority` outside 0-255."""
    def end_track(self, namespace: list[str], name: str) -> asyncio.Future[None]:
        """Ends every subscription to a track `publish_track` registered with PUBLISH_DONE; the
        track keeps its numbering and cache, so registering it again continues them."""
    def refresh_token(self, token: str, *, app_id: str) -> asyncio.Future[None]:
        """Hands the relay `token`, a later token of the same app, before the current one expires
        (a TRACK_STATUS for `[app_id]` that carries it); the session presents it on the connections it
        opens from now on. Raises RuntimeError for a session connected without a token or a token the
        relay refuses; the relay then keeps the connection's token until it expires."""
    def closed(self) -> asyncio.Future[CloseInfo]:
        """How the session's last connection closed, once it has."""
    def close(self) -> asyncio.Future[None]: ...
    def transport_stats(self) -> TransportStats:
        """The bytes the session's connections have carried, a reconnection's included."""

class RequestError(RuntimeError):
    """A request that failed: the relay refused it (its `*_ERROR`), it timed out (TIMEOUT), or it
    failed on this side or with the session (INTERNAL_ERROR)."""
    message_type: str
    """The request, such as `"SUBSCRIBE"` or `"FETCH"`."""
    code: int
    """The draft-14 number of the error code, also on a draft-18 session; `RequestErrorCode` names
    those of SUBSCRIBE, FETCH and TRACK_STATUS."""
    reason: str
    """The relay's reason phrase, or a description of the failure."""

class CloseInfo:
    @property
    def code(self) -> int | None:
        """The termination code the connection closed with; None when it was lost without one."""
    @property
    def reason(self) -> str: ...

class TransportStats:
    @property
    def sent_bytes(self) -> int: ...
    @property
    def received_bytes(self) -> int: ...

class NamespaceEvent:
    @property
    def namespace(self) -> list[str]: ...
    @property
    def active(self) -> bool:
        """False once the namespace was withdrawn (PUBLISH_NAMESPACE_DONE)."""

class NamespaceWatch(AsyncIterator[NamespaceEvent]):
    def __aiter__(self) -> NamespaceWatch: ...
    def __anext__(self) -> asyncio.Future[NamespaceEvent]: ...
    def close(self) -> asyncio.Future[None]:
        """UNSUBSCRIBE_NAMESPACE; a `__anext__` that waits stops the iteration."""

class Location:
    @property
    def group_id(self) -> int: ...
    @property
    def object_id(self) -> int: ...
    def __init__(self, group_id: int, object_id: int) -> None: ...

class Object:
    @property
    def group_id(self) -> int: ...
    @property
    def subgroup_id(self) -> int:
        """0 for an object that arrived in a datagram."""
    @property
    def object_id(self) -> int: ...
    @property
    def status(self) -> int | None:
        """The Object Status of an object without a payload (3 is End of Group); None for one with a
        payload."""
    @property
    def payload(self) -> bytes: ...
    @property
    def extension_headers(self) -> list[tuple[int, int | bytes]]:
        """The extension headers as `(id, value)` pairs in the order they came, which carry the
        object's LOC properties (draft-ietf-moq-loc-04 §2.2): an even id carries an integer, an odd
        one bytes."""
    @property
    def timestamp_micros(self) -> int | None:
        """The LOC Timestamp property in µs (draft-ietf-moq-loc-04 §2.3.1): wall-clock time since
        the Unix epoch, or media time when a Timescale property comes with it; None without one."""

class PublishDone:
    @property
    def code(self) -> int:
        """The PUBLISH_DONE code as draft-14 numbers it (§13.1.3), also on a draft-18 session."""
    @property
    def reason(self) -> str: ...

class Subscription(AsyncIterator[Object]):
    def __aiter__(self) -> Subscription: ...
    def __anext__(self) -> asyncio.Future[Object]:
        """Raises StopAsyncIteration once the publisher ends the track. A group whose stream
        fails (reset by a publisher that left, or cut by a GOAWAY's cutover) ends early."""
    @property
    def largest(self) -> Location | None:
        """The largest location SUBSCRIBE_OK reported."""
    def done(self) -> asyncio.Future[PublishDone | None]:
        """Resolves once the publisher ends the subscription with PUBLISH_DONE, or to None once the
        session ends or `close()` is called without one."""
    def groups(self) -> Groups:
        """The subgroup streams, each iterated for its objects, status objects included. A
        subscription is read either by object or by group, whichever comes first; the other raises
        RuntimeError, as does a track delivered in datagrams."""
    def set_forward(self, forward: bool) -> asyncio.Future[None]:
        """SUBSCRIBE_UPDATE with the Forward flag. On draft-18 it returns once the relay applied the
        update, so an object published afterwards is delivered; draft-14 defines no answer, so there
        an object published right after it may still be withheld. Raises RuntimeError when the relay
        refuses the update or does not answer it within 10 s."""
    def set_priority(self, priority: int) -> asyncio.Future[None]:
        """SUBSCRIBE_UPDATE with the Subscriber Priority; returns and raises as `set_forward`."""
    def request_new_group(self, largest_group: int | None = None) -> asyncio.Future[bool]:
        """Asks the publisher for a new group (NEW_GROUP_REQUEST); `largest_group` is the largest
        group ID seen. Returns False, without sending, for a track without dynamic groups."""
    def end_before_group(self, group_id: int) -> asyncio.Future[None]:
        """SUBSCRIBE_UPDATE ending the subscription before `group_id`; the publisher sends
        PUBLISH_DONE once it has sent the groups below it."""
    def close(self) -> asyncio.Future[None]:
        """UNSUBSCRIBE; the iteration ends once the relay stops the track."""

class Groups(AsyncIterator[Group]):
    def __aiter__(self) -> Groups: ...
    def __anext__(self) -> asyncio.Future[Group]:
        """Raises StopAsyncIteration once the track has no more streams."""

class Group(AsyncIterator[Object]):
    @property
    def group_id(self) -> int: ...
    @property
    def subgroup_id(self) -> int: ...
    def __aiter__(self) -> Group: ...
    def __anext__(self) -> asyncio.Future[Object]:
        """Raises StopAsyncIteration once the publisher finished the stream, and RuntimeError when
        the stream was reset."""

class Fetch(AsyncIterator[Object]):
    """The objects of one FETCH response that carry a payload."""
    def __aiter__(self) -> Fetch: ...
    def __anext__(self) -> asyncio.Future[Object]:
        """Raises StopAsyncIteration once the response ends."""

class TrackWriter:
    def write(self, payload: bytes, *, new_group: bool = False) -> asyncio.Future[int | None]:
        """The object is cached for FETCH whether or not a subscriber takes it. Returns its group id
        when a subscriber took it, and None otherwise; an object within a group that was never
        started is dropped."""
    def write_group(self, payload: bytes) -> asyncio.Future[int | None]:
        """Sends `payload` as a group of its own, the layout of an MSF catalog. Returns the group id
        when a subscriber took it, and None otherwise."""
    def next_subscriber(self) -> asyncio.Future[bool]:
        """True once a SUBSCRIBE for the track is answered, False once the session has ended: the
        moment to send what a new subscriber needs, since its SUBSCRIBE starts after the largest
        object."""

class MediaBroadcast:
    def announce(self) -> asyncio.Future[None]: ...
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
        cmaf: bool = False,
        on_demand: bool = True,
    ) -> MediaSource:
        """A source on the tracks `video` and `audio`, with the media timeline `<video>_timeline`.
        With `cmaf`, the video, its renditions and AAC audio also travel as CMAF in `<track>_cmaf`
        (draft-ietf-moq-cmsf-01); Opus stays LOC alone. The video, its renditions, their CMAF
        siblings and the audio are published on demand (draft-18 §10.10), as the browser SDK's
        are: the relay has each sent only while a subscriber takes it, and
        `MediaSource.demand_change` reports when that changes. `on_demand=False` keeps them sent
        with no subscriber, for a track that should stay in the relay's cache, FETCHable, while
        nobody takes it."""

class MediaSource:
    def configure_video(self, codec: str) -> None:
        """`codec` is a WebCodecs codec string (`avc1.*`, `hvc1.*`, `vp8`, `vp09.*`, `av01.*`);
        H.264 and H.265 frames are Annex B. Raises ValueError for another codec."""
    def push_video(self, data: bytes, timestamp_micros: int) -> None:
        """One encoded frame; a keyframe opens a group. Raises RuntimeError before
        `configure_video`."""
    def add_rendition(
        self, name: str, *, label: str, bitrate: int | None = None, framerate: int | None = None
    ) -> Rendition:
        """Another encoding of the video in the track `<video>_<name>`, in the group ids of the
        video's keyframes on the same pictures (draft-ietf-moq-cmsf-01 §3.2). It joins the video's
        switching set (one catalog `altGroup`), so a rendition in another codec is an alternative
        to the video. `bitrate` (bits per second) and `framerate` go into the catalog."""
    def configure_audio(
        self, codec: str, sample_rate: int, channels: int, *, description: bytes | None = None
    ) -> None:
        """`"opus"`, or an AAC `"mp4a.40.*"` whose `description` is the AudioSpecificConfig. Raises
        ValueError for another codec, an AAC without `description` or a malformed one."""
    def push_audio(
        self, data: bytes, timestamp_micros: int, *, level: int | None = None, voice_activity: bool = False
    ) -> None:
        """One encoded audio frame. `level` is its RFC 6464 level in -dBov (0 to 127), written with
        `voice_activity` as its LOC Audio Level property."""
    def remove_track(self, kind: str) -> asyncio.Future[None]:
        """Ends the source's video or audio track (`kind` is `"video"` or `"audio"`) with
        PUBLISH_DONE and sends the catalog without it; removing the video removes its renditions
        too. Stop pushing its media first; `configure_video` or `configure_audio` and the media
        pushed after it publish the track again. Raises ValueError for another kind and
        RuntimeError when the relay session has ended."""
    def keyframe_request(self) -> asyncio.Future[None]:
        """Resolves the next time a subscriber asks for a new group (NEW_GROUP_REQUEST) or the
        broadcast needs one, as after dropping frames; encode the renditions' next frame as a keyframe
        too."""

    def demand_change(self) -> asyncio.Future[Demand]:
        """The next change of whether a subscriber takes one of the source's tracks, the video's and
        renditions' CMAF siblings included: reported once when a track is published, then at each
        change. A change replaces its track's unread one, so a caller that reads late gets each
        track's latest. The caller need not encode a track nobody takes. Raises RuntimeError for a
        source added with `on_demand=False`, and once every track of the source is gone."""

class Demand:
    @property
    def track_name(self) -> str: ...
    @property
    def wanted(self) -> bool: ...

class Rendition:
    def configure_video(self, codec: str) -> None:
        """As `MediaSource.configure_video`; it may differ from the video's codec."""
    def push_video(self, data: bytes, timestamp_micros: int) -> None:
        """An encoded frame of the same picture as a video frame, at that frame's timestamp. Raises
        RuntimeError before `configure_video`, and once an earlier frame stopped the rendition, as
        when the session ended."""
