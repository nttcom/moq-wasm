"""Native module for the moq-wasm Python SDK (docs/architecture/python).

Awaitable methods return `asyncio.Future` (use `await` or `asyncio.ensure_future`, not `asyncio.create_task`).
"""

import asyncio
from collections.abc import AsyncIterator
from typing import Literal

def connect(
    url: str,
    *,
    verify_certificate: bool = True,
    token: str | None = None,
    refreshes_token: bool = False,
    versions: list[str] | None = None,
) -> asyncio.Future[Session]:
    """Connect to a relay (`moqt://` for QUIC, `https://` for WebTransport) with optional JWT `token`.

    Follows relay `GOAWAY` using the latest token. With `refreshes_token=True`, also reconnects after
    expired-token closes; otherwise an expired token ends the session and `refresh_token()` raises `RuntimeError`.
    `versions` lists drafts to offer, most preferred first (`"draft-14"`, `"draft-18"`; default: draft-18 then draft-14).
    Raises `ValueError` for unknown version names; an empty list fails connection.
    """

def check_audio_specific_config(description: bytes) -> None:
    """Validate `description` as an AudioSpecificConfig (ISO/IEC 14496-3 §1.6.2.1); raises `ValueError` if invalid."""

def set_log_sink(level: str, spans: bool) -> None:
    """Install the process-wide `tracing` subscriber queuing logs at `level` (`error`/`warn`/`info`/`debug`/`trace`) and above.

    With `spans=True`, also queues closed `INFO`+ spans (carrying `TRACE_CONTEXT` traces).
    Raises `ValueError` for unknown levels and `RuntimeError` if already set.
    """

def wait_log_records() -> list[tuple[str, str, str | None]]:
    """Return queued `(kind, json, token)` records (`kind` is `"log"` or `"span"`, `token` is the owning session's token).

    Waits up to 0.5 s (GIL released) and returns `[]` on timeout; prepends a `log records dropped` warning if the queue overflowed.
    Raises `RuntimeError` before `set_log_sink`.
    """

def parse_catalog(payload: bytes) -> dict:
    """Parse an independent MSF `draft-02` catalog (draft-ietf-moq-msf-02 §5, §8.3).

    Raises `ValueError` for malformed JSON, a missing `tracks` array, unsupported versions, delta updates, or invalid tracks.
    """

class Session:
    """`watch_namespaces`, `subscribe`, `subscribe_current_group`, `subscribe_group_start`,
    `fetch`, `track_status`, and `announce` raise `RequestError` on relay rejection, 10 s timeout, or session end."""
    def watch_namespaces(self, prefix: list[str]) -> asyncio.Future[NamespaceWatch]:
        """Watch namespaces published and withdrawn under `prefix`, including existing ones."""
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
        """Send `SUBSCRIBE` (`largest_object`: after largest object; `next_group_start`: next group).

        - `forward=False`: relay withholds objects until `set_forward(True)`.
        - `priority`: subscriber priority (default 128; lower goes first).
        - `delivery_timeout`: seconds from relay receipt before the relay resets the subgroup stream and skips its remaining objects.

        Raises `ValueError` for unknown `filter` or `group_order`.
        """
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
        """Subscribe from group start: joins the current group via joining `FETCH`, or sends `NEW_GROUP_REQUEST`
        when a dynamic-group track's largest object ID is >= `new_group_after`. Options match `subscribe`.
        """
    def fetch(self, namespace: list[str], name: str, start: Location, end: Location) -> asyncio.Future[Fetch]:
        """Standalone `FETCH` from `start` to `end` (`end.object_id` is exclusive; `0` means the whole group).

        Returns once the response stream arrives; raises `RuntimeError` if it never arrives.
        """
    def track_status(self, namespace: list[str], name: str) -> asyncio.Future[Location | None]:
        """Send `TRACK_STATUS`; returns the track's largest location or `None` if empty."""
    def subscribe_current_group(
        self,
        namespace: list[str],
        name: str,
        *,
        priority: int | None = None,
        delivery_timeout: float | None = None,
        group_order: Literal["ascending", "descending"] = "ascending",
    ) -> asyncio.Future[Subscription]:
        """Deliver the relay's current group via joining `FETCH`, then continue live. Options match `subscribe`."""
    def announce(self, namespace: list[str]) -> asyncio.Future[None]: ...
    def media_broadcast(self, namespace: list[str]) -> MediaBroadcast:
        """Create a media broadcast in `namespace` (call its `announce()` instead of `session.announce()`)."""
    def unannounce(self, namespace: list[str]) -> asyncio.Future[None]:
        """Send `PUBLISH_NAMESPACE_DONE` for a previously announced namespace."""
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
        """Register a track to serve `SUBSCRIBE` and `FETCH`, retaining objects for `retention_seconds` (`0` = newest only).

        Register before `announce()` when `publish=False`; announce first when `publish=True` (sends `PUBLISH` immediately).
        With `datagram=True`, each object is sent as an unreliable datagram (objects exceeding the path MTU, ~1 KB safe limit, are dropped; read by object, not `groups()`).
        `priority` is Publisher Priority (0–255; lower is sent first among this session's tracks).
        Raises `RequestError` on `PUBLISH` rejection, `ValueError` if objects already exist and `datagram` differs from its earlier registration, or `OverflowError` for out-of-range `priority`.
        """
    def end_track(self, namespace: list[str], name: str) -> asyncio.Future[None]:
        """End all subscriptions to the track with `PUBLISH_DONE` while preserving its numbering and cache."""
    def refresh_token(self, token: str, *, app_id: str) -> asyncio.Future[None]:
        """Hand the relay a later JWT for the same app before the current one expires (via `TRACK_STATUS` on `[app_id]`), and use it on future reconnects.

        Raises `RuntimeError` if connected without `token`/`refreshes_token` or if rejected by the relay (the relay then keeps the connection's current token until it expires).
        """
    def closed(self) -> asyncio.Future[CloseInfo]:
        """Resolves with how the session's final connection closed."""
    def close(self) -> asyncio.Future[None]: ...
    def transport_stats(self) -> TransportStats:
        """Cumulative bytes carried across the session's connections (including reconnects)."""

class RequestError(RuntimeError):
    """Failed MoQT request: relay rejection (`*_ERROR`), timeout (`TIMEOUT`), or local/session error (`INTERNAL_ERROR`)."""
    message_type: str
    """Request name (e.g. `"SUBSCRIBE"`, `"FETCH"`)."""
    code: int
    """Draft-14 error code number (also mapped on draft-18); see `RequestErrorCode`."""
    reason: str
    """Relay reason phrase or local failure description."""

class CloseInfo:
    @property
    def code(self) -> int | None: ...
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
        """`False` when withdrawn (`PUBLISH_NAMESPACE_DONE`)."""

class NamespaceWatch(AsyncIterator[NamespaceEvent]):
    def __aiter__(self) -> NamespaceWatch: ...
    def __anext__(self) -> asyncio.Future[NamespaceEvent]: ...
    def close(self) -> asyncio.Future[None]:
        """Send `UNSUBSCRIBE_NAMESPACE` and end iteration."""

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
        """Always `0` for datagram objects."""
    @property
    def object_id(self) -> int: ...
    @property
    def status(self) -> int | None:
        """Object status for empty objects (`3` = End of Group), or `None` when payload is present."""
    @property
    def payload(self) -> bytes: ...
    @property
    def extension_headers(self) -> list[tuple[int, int | bytes]]:
        """Ordered `(id, value)` extension headers (LOC properties, draft-ietf-moq-loc-04 §2.2; even = int, odd = bytes)."""
    @property
    def timestamp_micros(self) -> int | None:
        """LOC Timestamp in µs (draft-ietf-moq-loc-04 §2.3.1): Unix wall clock or media time with Timescale, else `None`."""

class PublishDone:
    @property
    def code(self) -> int:
        """Draft-14 `PUBLISH_DONE` code (§13.1.3), also mapped on draft-18."""
    @property
    def reason(self) -> str: ...

class Subscription(AsyncIterator[Object]):
    def __aiter__(self) -> Subscription: ...
    def __anext__(self) -> asyncio.Future[Object]:
        """Reset or cut-off subgroup streams end early."""
    @property
    def largest(self) -> Location | None:
        """Largest location reported by `SUBSCRIBE_OK`."""
    def done(self) -> asyncio.Future[PublishDone | None]:
        """Resolves with `PublishDone` on publisher termination, or `None` on local `close()` / session end."""
    def groups(self) -> Groups:
        """Iterate subgroup streams (including status objects). Mutually exclusive with object iteration; raises `RuntimeError` if already read by object or if datagram-delivered."""
    def set_forward(self, forward: bool) -> asyncio.Future[None]:
        """Send `SUBSCRIBE_UPDATE` for the Forward flag.

        Draft-18 waits for relay confirmation; draft-14 has no ack so immediately following objects may still be withheld.
        Raises `RuntimeError` on rejection or 10 s timeout.
        """
    def set_priority(self, priority: int) -> asyncio.Future[None]:
        """Send `SUBSCRIBE_UPDATE` for Subscriber Priority (same completion/error semantics as `set_forward`)."""
    def request_new_group(self, largest_group: int | None = None) -> asyncio.Future[bool]:
        """Send `NEW_GROUP_REQUEST` for `largest_group + 1` (or `0` when `None`); `largest_group` is the largest group already seen. Returns `False` without sending for non-dynamic tracks."""
    def end_before_group(self, group_id: int) -> asyncio.Future[None]:
        """Send `SUBSCRIBE_UPDATE` ending the subscription before `group_id`; the publisher sends `PUBLISH_DONE` after earlier groups finish."""
    def close(self) -> asyncio.Future[None]:
        """Send `UNSUBSCRIBE` and end iteration once the relay stops the track."""

class Groups(AsyncIterator[Group]):
    def __aiter__(self) -> Groups: ...
    def __anext__(self) -> asyncio.Future[Group]: ...

class Group(AsyncIterator[Object]):
    @property
    def group_id(self) -> int: ...
    @property
    def subgroup_id(self) -> int: ...
    def __aiter__(self) -> Group: ...
    def __anext__(self) -> asyncio.Future[Object]:
        """Raises `RuntimeError` if the stream was reset."""

class Fetch(AsyncIterator[Object]):
    """Payload-carrying objects of a `FETCH` response."""
    def __aiter__(self) -> Fetch: ...
    def __anext__(self) -> asyncio.Future[Object]: ...

class TrackWriter:
    def write(self, payload: bytes, *, new_group: bool = False) -> asyncio.Future[int | None]:
        """Write an object; open the first group with `new_group=True` (objects before a group opens are dropped).

        Accepted objects are cached for `FETCH` even without subscribers; returns the group ID if sent to a subscriber, else `None`.
        """
    def write_group(self, payload: bytes) -> asyncio.Future[int | None]:
        """Write `payload` as a standalone single-object group (e.g. MSF catalog). Returns the group ID if sent, else `None`."""
    def next_subscriber(self) -> asyncio.Future[bool]:
        """Returns `True` when a new `SUBSCRIBE` is answered (the moment to send initial state, since `SUBSCRIBE` starts after the largest object), or `False` on session end."""

class MediaBroadcast:
    def announce(self) -> asyncio.Future[None]:
        """Publish the namespace; raises `RequestError` on `PUBLISH_NAMESPACE` rejection."""
    def close(self) -> asyncio.Future[None]:
        """End all tracks and catalog with `PUBLISH_DONE` after flushing queued media (idempotent). Unannounce first; subsequent pushes raise `RuntimeError`."""
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
        """Add a source on `video`, `audio`, and `<video>_timeline` (plus `<track>_cmaf` for video/AAC when `cmaf=True`; Opus stays LOC-only).

        `on_demand=True` (draft-18 §10.10) sends tracks only while subscribed (reportable via `MediaSource.demand_change`).
        `on_demand=False` sends continuously so the relay cache stays `FETCH`able without subscribers.
        Raises `RuntimeError` after `close()`.
        """

class MediaSource:
    def configure_video(self, codec: str) -> None:
        """Set WebCodecs video codec (`avc1.*`, `hvc1.*`, `vp8`, `vp09.*`, `av01.*`; H.264/H.265 frames must be Annex B). Raises `ValueError` if unsupported."""
    def push_video(self, data: bytes, timestamp_micros: int) -> bool:
        """Push one encoded video frame (keyframes open a new group).

        Returns `False` if dropped on queue backpressure (until next keyframe; triggers `keyframe_request`) or if the frame has no picture (parameter sets are retained for later keyframes).
        Raises `RuntimeError` before `configure_video`, after publishing stops, or after broadcast close.
        """
    def add_rendition(
        self, name: str, *, label: str, bitrate: int | None = None, framerate: int | None = None
    ) -> Rendition:
        """Add a video rendition `<video>_<name>` to the video's switching set (`altGroup`, `draft-ietf-moq-cmsf-01` §3.2).

        Feed the same pictures and keyframe positions as the primary video (`bitrate` in bits/s, `framerate` go in the catalog).
        Buffers up to 10 s on relay backpressure before dropping to the next keyframe (triggering `keyframe_request`). Raises `RuntimeError` after broadcast close.
        """
    def configure_audio(
        self, codec: str, sample_rate: int, channels: int, *, description: bytes | None = None
    ) -> None:
        """Configure `"opus"` or AAC (`"mp4a.40.*"` with `AudioSpecificConfig` `description`). Raises `ValueError` if invalid."""
    def push_audio(
        self, data: bytes, timestamp_micros: int, *, level: int | None = None, voice_activity: bool = False
    ) -> bool:
        """Push one encoded audio frame with optional RFC 6464 `level` (0–127 -dBov) and `voice_activity` (LOC Audio Level).

        Returns `False` if dropped on backpressure; raises `RuntimeError` after publishing stops or broadcast close.
        """
    def remove_track(self, kind: str) -> asyncio.Future[None]:
        """End `"video"` (plus its renditions) or `"audio"` with `PUBLISH_DONE` and update the catalog.

        Stop pushing first; re-configure and push again to resume. Raises `ValueError` for invalid `kind` and `RuntimeError` after session end.
        """
    def keyframe_request(self) -> asyncio.Future[None]:
        """Resolves when a subscriber sends `NEW_GROUP_REQUEST` or the broadcast needs a keyframe (also encode renditions as keyframes)."""

    def demand_change(self) -> asyncio.Future[Demand]:
        """Resolves on each subscriber demand change across the source's tracks (coalesced per track to latest state).

        Raises `RuntimeError` when `on_demand=False` or once all tracks are removed.
        """

class Demand:
    @property
    def track_name(self) -> str: ...
    @property
    def wanted(self) -> bool: ...

class Rendition:
    def configure_video(self, codec: str) -> None:
        """Same as `MediaSource.configure_video` (may use a different codec)."""
    def push_video(self, data: bytes, timestamp_micros: int) -> bool:
        """Push an encoded frame matching the primary video's timestamp. Returns `False` if dropped or non-picture; raises `RuntimeError` before `configure_video`, after failure, or after close."""
