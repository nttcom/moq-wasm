import asyncio
import logging
import operator
from collections.abc import Awaitable, Callable
from typing import TypeVar

from moq_wasm import _native
from moq_wasm._ended import Ended

logger = logging.getLogger("moq_wasm")

T = TypeVar("T")

# The first element of the WebCodecs codec strings `broadcast` publishes as LOC.
VIDEO_CODECS = {"avc1", "avc3", "hvc1", "hev1", "vp8", "vp09", "av01"}
# The names `broadcast` gives the tracks of its default source, and its chat track
# (crates/broadcast/src/broadcast.rs).
DEFAULT_VIDEO = "video"
DEFAULT_AUDIO = "audio"
CHAT = "chat"


class MediaSource:
    """A video and an audio track of a broadcast, fed with encoded frames, as the browser SDK's
    source is: `broadcast` builds the catalog, the LOC tracks and the media timeline. Frames pushed
    before the broadcast is published, the relay having accepted its namespace, or after it was
    left or the session ended, are dropped; the configurations reach the native source as soon as
    it is built, since its catalog lists them. Once publishing the source failed, as the relay
    refused the broadcast's namespace or the native publishing of a live session's source failed in
    a push or a configuration, the pushes, configurations, `add_rendition`, the renditions'
    configurations and pushes and `remove_track` raise RuntimeError from that failure, and
    `keyframe_request` and `demand_change` raise it."""

    def __init__(
        self,
        video: str,
        audio: str,
        *,
        label: str,
        video_bitrate: int | None = None,
        video_framerate: int | None = None,
        cmaf: bool = False,
        on_demand: bool = True,
    ):
        # Checked here, so a broadcast with a source the native broadcast would refuse fails before
        # anything native is built.
        _check_type("video", video, str)
        _check_type("audio", audio, str)
        _check_type("label", label, str)
        _check_optional_unsigned("video_bitrate", video_bitrate, 64)
        _check_optional_unsigned("video_framerate", video_framerate, 32)
        _check_type("cmaf", cmaf, bool)
        _check_type("on_demand", on_demand, bool)
        self._video = video
        self._audio = audio
        self._label = label
        self._video_bitrate = video_bitrate
        self._video_framerate = video_framerate
        self._cmaf = cmaf
        self._on_demand = on_demand
        self._source: _native.MediaSource | None = None
        self._video_codec: str | None = None
        self._audio_config: tuple[str, int, int, bytes | None] | None = None
        # The tracks the native media broadcast registered, or may have, for the configurations a
        # later one replaced, as the CMAF sibling of an AAC audio replaced by Opus.
        self._replaced_track_names: set[str] = set()
        self._renditions: list[Rendition] = []
        self._built = asyncio.Event()
        # Set once the relay accepted the broadcast's namespace: the pushes reach the native source
        # only then, as the media of a namespace the relay refuses would be lost.
        self._announced = asyncio.Event()
        # Ended once the broadcast was left or the session ended, which stop the source, or by
        # what kept the broadcast from being published or stopped publishing the source.
        self._ended = Ended()
        self._session_ended: asyncio.Future[None] | None = None
        # Set by the broadcast the source is added to: claims the tracks of a rendition, raising
        # RuntimeError while another track has the name of one of them.
        self._claim_tracks: Callable[[list[str]], None] | None = None

    @property
    def video(self) -> str:
        """The video track's name; read-only, as the broadcast claimed the source's tracks by it."""
        return self._video

    @property
    def audio(self) -> str:
        """The audio track's name; read-only, as `video` is."""
        return self._audio

    def configure_video(self, codec: str) -> None:
        """`codec` is a WebCodecs codec string: `avc1.*` (Annex B), `hvc1.*` (Annex B), `vp8`,
        `vp09.*` or `av01.*`. Raises ValueError for another codec."""
        _check_video_codec(codec)
        self._publish(lambda source: source.configure_video(codec))
        self._video_codec = codec

    def add_rendition(
        self, name: str, *, label: str, bitrate: int | None = None, framerate: int | None = None
    ) -> "Rendition":
        """Another encoding of the video, in the track `<video>_<name>`, and with `cmaf` its
        `_cmaf` sibling (`_native.MediaSource.add_rendition`): feed it frames of the same pictures,
        with a keyframe on the same pictures as the video's. `bitrate` (bits per second) and
        `framerate` go into the catalog. Raises ValueError for a negative `bitrate` or `framerate`,
        or one too large, TypeError for an option of another type, such as a float `framerate`,
        and RuntimeError once publishing the source failed, and while another track of the
        broadcast, or once it was entered of the session, has the name of one of its tracks;
        ValueError as well for a track the session registered in datagrams or with another
        retention than 60 s."""
        _check_type("name", name, str)
        _check_type("label", label, str)
        _check_optional_unsigned("bitrate", bitrate, 64)
        _check_optional_unsigned("framerate", framerate, 32)
        self._raise_if_failed()
        if self._claim_tracks is not None:
            self._claim_tracks(self._with_cmaf(f"{self.video}_{name}"))
        rendition = Rendition(name, {"label": label, "bitrate": bitrate, "framerate": framerate})
        self._renditions.append(rendition)
        if self._source is not None:
            assert self._session_ended is not None, "a built source knows its session"
            rendition._build(self._source, self._session_ended, self._announced)
        return rendition

    def push_video(self, data: bytes, timestamp_micros: int) -> bool:
        """One encoded frame; a keyframe opens a group. `timestamp_micros` must not run
        backwards. Returns whether the native source took the frame: False for one dropped, before
        the broadcast was published, the relay having accepted its namespace, after it was left or
        once the session ended, for one the native source's publish queue dropped while the relay
        does not keep up (up to the next keyframe that fits, which `keyframe_request` asks for), and
        for one without a picture, as an H.264 access unit without NAL units or without a slice (an
        access unit delimiter or parameter sets alone), which is not published, though the parameter
        sets go to the keyframes after it. Raises RuntimeError before `configure_video`, and once
        publishing the source failed."""
        if self._video_codec is None:
            raise RuntimeError("call configure_video first")
        return self._publish(lambda source: source.push_video(data, timestamp_micros), push=True)

    def configure_audio(
        self, codec: str, sample_rate: int, channels: int, *, description: bytes | None = None
    ) -> None:
        """`"opus"`, or an AAC `"mp4a.40.*"` whose `description` is the AudioSpecificConfig;
        raises ValueError for another codec, an AAC without it, and a negative `sample_rate` or
        `channels`, or one too large, TypeError for one that is no integer, as None, and
        RuntimeError once publishing the source failed. A refused configuration changes nothing."""
        _check_unsigned("sample_rate", sample_rate, 32)
        _check_unsigned("channels", channels, 8)
        if codec != "opus" and not codec.startswith("mp4a.40."):
            raise ValueError(f"unsupported audio codec: {codec}")
        if codec != "opus":
            if description is None:
                raise ValueError("AAC needs its AudioSpecificConfig")
            _native.check_audio_specific_config(description)
        self._publish(lambda source: source.configure_audio(codec, sample_rate, channels, description=description))
        self._replaced_track_names.update(self._registered_track_names())
        self._audio_config = (codec, sample_rate, channels, description)

    def push_audio(
        self, data: bytes, timestamp_micros: int, *, level: int | None = None, voice_activity: bool = False
    ) -> bool:
        """One encoded audio frame; `level` and `voice_activity` as in
        `_native.MediaSource.push_audio`. Returns whether the native source took it, as
        `push_video` does. Raises RuntimeError before `configure_audio`, and once publishing the
        source failed."""
        if self._audio_config is None:
            raise RuntimeError("call configure_audio first")
        return self._publish(
            lambda source: source.push_audio(data, timestamp_micros, level=level, voice_activity=voice_activity),
            push=True,
        )

    def _publish(self, call: Callable[[_native.MediaSource], bool | None], *, push: bool = False) -> bool:
        """`call` on the native source once it is built, and for a `push` once the broadcast was
        published; returns whether the native source took it. Raises RuntimeError once publishing
        the source failed; a failure of `call` in a live session is that failure."""
        self._raise_if_failed()
        if (source := self._source) is None or (push and not self._announced.is_set()):
            return False
        try:
            return _call_native(lambda: call(source), self._session_ended)
        except RuntimeError as failure:
            # Ends the waiting requests with the failure.
            self._detach(failure, failure=True)
            raise

    def _raise_if_failed(self) -> None:
        if self._ended.failed:
            raise RuntimeError("the media source stopped publishing") from self._ended.error()

    async def remove_track(self, kind: str) -> None:
        """`_native.MediaSource.remove_track` (`kind` is `"video"` or `"audio"`): stop pushing its
        media first; `configure_video` or `configure_audio` and the media pushed after it publish
        the track again. Before the broadcast is published, it forgets the kind's configuration
        (the renditions' too, for the video). Raises ValueError for another kind, and RuntimeError
        once publishing the source failed."""
        if kind not in ("video", "audio"):
            raise ValueError(f"unknown track kind: {kind}")
        self._raise_if_failed()
        if self._source is not None:
            await self._source.remove_track(kind)
        elif kind == "video":
            self._video_codec = None
            for rendition in self._renditions:
                rendition._video_codec = None
        else:
            self._audio_config = None

    async def keyframe_request(self) -> None:
        """Resolves the next time a subscriber asks for a new group (NEW_GROUP_REQUEST) or the
        broadcast needs one: encode the next frame as a keyframe then. Waits while the broadcast is
        not published; raises RuntimeError once the broadcast was left or the session ended, and
        what kept the broadcast from being published or stopped publishing the source."""
        await self._while_publishing(lambda source: source.keyframe_request())

    async def demand_change(self) -> _native.Demand:
        """`_native.MediaSource.demand_change`; waits while the broadcast is not published. Raises
        RuntimeError for a source added with `on_demand=False`, and once the broadcast was left or
        the session ended; raises what kept the broadcast from being published or stopped
        publishing the source."""
        if not self._on_demand:
            raise RuntimeError("the source was added with on_demand=False")
        return await self._while_publishing(lambda source: source.demand_change())

    async def _while_publishing(self, request: Callable[[_native.MediaSource], Awaitable[T]]) -> T:
        """`request` on the native source once it is built, ended by the source stopping."""
        built = asyncio.ensure_future(self._built.wait())
        try:
            await self._ended.until(built)
        finally:
            built.cancel()
        if self._ended.ended or self._source is None:
            raise self._stopped_error()
        answered = asyncio.ensure_future(request(self._source))
        await self._ended.until(answered)
        if not answered.done():
            answered.cancel()
            raise self._stopped_error()
        return answered.result()

    def _track_names(self) -> list[str]:
        """The tracks the media broadcast publishes for the source, its renditions aside."""
        return [*self._with_cmaf(self.video), *self._with_cmaf(self.audio), f"{self.video}_timeline"]

    def _implied_track_names(self) -> list[str]:
        """The tracks the media broadcast lists in its catalog next to the source's own, ending
        them once it no longer does (`build_catalog` in crates/broadcast/src/broadcast.rs): the chat
        while it lists the default video or audio, and an audio placeholder, which an audio of that
        name takes the place of, while it lists the default video."""
        implied = [CHAT] if self.video == DEFAULT_VIDEO or self.audio == DEFAULT_AUDIO else []
        if self.video == DEFAULT_VIDEO and self.audio != DEFAULT_AUDIO:
            implied.append(DEFAULT_AUDIO)
        return implied

    def _registered_track_names(self) -> list[str]:
        """The tracks the native media broadcast registered for the source, or may have: those its
        catalog listed for a configuration that reached the native source, which it registers as
        the catalog goes out or a subscriber takes one (`CatalogTrack::send` and `serve_subscribe`
        under crates/broadcast/src/relay_connection): a configured video's or rendition's track and
        CMAF sibling, a configured audio's track and an AAC one's CMAF sibling, the media timeline,
        and the chat and the audio placeholder the default video lists, or the chat the default
        audio does, for the configuration it has and each one it replaced since it was built. A
        track named but never configured, as the video of an audio-only source, is not one."""
        if not self._built.is_set():
            return []
        listed = list(self._replaced_track_names)
        if self._video_codec is not None:
            listed += self._with_cmaf(self.video)
            if self.video == DEFAULT_VIDEO:
                listed += [DEFAULT_AUDIO, CHAT]
        if self._audio_config is not None:
            aac = self._audio_config[0] != "opus"
            listed += self._with_cmaf(self.audio) if aac else [self.audio]
            if self.audio == DEFAULT_AUDIO:
                listed.append(CHAT)
        for rendition in self._renditions:
            if rendition._video_codec is not None:
                listed += self._with_cmaf(f"{self.video}_{rendition.name}")
        if listed:
            listed.append(f"{self.video}_timeline")
        return listed

    def _with_cmaf(self, track: str) -> list[str]:
        """`track`, and its CMAF sibling when the source publishes them."""
        return [track, f"{track}_cmaf"] if self._cmaf else [track]

    def _build(self, broadcast: _native.MediaBroadcast, session_ended: asyncio.Future[None]) -> None:
        source = broadcast.add_source(
            self.video,
            self.audio,
            label=self._label,
            video_bitrate=self._video_bitrate,
            video_framerate=self._video_framerate,
            cmaf=self._cmaf,
            on_demand=self._on_demand,
        )
        if self._video_codec is not None:
            source.configure_video(self._video_codec)
        if self._audio_config is not None:
            codec, sample_rate, channels, description = self._audio_config
            source.configure_audio(codec, sample_rate, channels, description=description)
        self._source = source
        self._session_ended = session_ended
        for rendition in self._renditions:
            rendition._build(source, session_ended, self._announced)
        self._built.set()

    def _stopped_error(self) -> BaseException:
        return self._ended.error() if self._ended.ended else RuntimeError("the media source stopped publishing")

    def _detach(
        self, error: Exception | None = None, *, failure: bool = False, raised: Callable[[], None] | None = None
    ) -> None:
        """Stops the source, which ends it with `error`, what kept its broadcast from being
        published or stopped publishing the source, unless it ended before; with `failure`, its
        pushes, configurations and renditions raise it too. `raised` is called each time it is
        raised."""
        self._end(error or RuntimeError("the media source stopped publishing"), failure=failure, raised=raised)
        self._source = None
        for rendition in self._renditions:
            rendition._rendition = None

    def _stop_with_session(self, _: BaseException) -> None:
        """The session ended: the requests raise that the source stopped, and the pushes reach the
        native source, which drops them."""
        self._end(RuntimeError("the media source stopped publishing"), failure=False)

    def _end(self, error: BaseException, *, failure: bool, raised: Callable[[], None] | None = None) -> None:
        # A source that stopped before, as when the session ended, stays stopped, and so do its
        # renditions.
        if self._ended.end(error, failure=failure, raised=raised) and failure:
            for rendition in self._renditions:
                rendition._ended.end(error, raised=raised)


class Rendition:
    """Another encoding of a `MediaSource`'s video. Frames pushed before the broadcast is
    published, the relay having accepted its namespace, or after it was left or the session ended,
    are dropped. Once publishing the rendition or its source failed, its configuration and pushes
    raise RuntimeError."""

    def __init__(self, name: str, options: dict):
        self._name = name
        self._options = options
        self._rendition: _native.Rendition | None = None
        self._session_ended: asyncio.Future[None] | None = None
        # Its source's: set once the relay accepted the broadcast's namespace.
        self._announced: asyncio.Event | None = None
        self._video_codec: str | None = None
        # Ended by a failure to publish the rendition or its source.
        self._ended = Ended()

    @property
    def name(self) -> str:
        """Read-only, as the broadcast claimed the rendition's tracks by it."""
        return self._name

    def configure_video(self, codec: str) -> None:
        """As `MediaSource.configure_video`; it may differ from the video's codec."""
        _check_video_codec(codec)
        self._publish(lambda rendition: rendition.configure_video(codec))
        self._video_codec = codec

    def push_video(self, data: bytes, timestamp_micros: int) -> bool:
        """An encoded frame of the same picture as a video frame, at that frame's timestamp.
        Returns whether the native rendition took it, as `MediaSource.push_video` does. Raises
        RuntimeError before `configure_video`, and once publishing the rendition failed while the
        session was live."""
        if self._video_codec is None:
            raise RuntimeError("call configure_video first")
        return self._publish(lambda rendition: rendition.push_video(data, timestamp_micros), push=True)

    def _publish(self, call: Callable[[_native.Rendition], bool | None], *, push: bool = False) -> bool:
        """`call` on the native rendition once it is built, and for a `push` once the broadcast was
        published; returns whether the native rendition took it. Raises RuntimeError once
        publishing it failed; a failure of `call` in a live session is that failure."""
        if self._ended.failed:
            raise RuntimeError(f"rendition {self.name} stopped publishing") from self._ended.error()
        if (rendition := self._rendition) is None:
            return False
        if push and not (self._announced is not None and self._announced.is_set()):
            return False
        try:
            return _call_native(lambda: call(rendition), self._session_ended)
        except RuntimeError as failure:
            self._ended.end(failure)
            raise

    def _build(
        self, source: _native.MediaSource, session_ended: asyncio.Future[None], announced: asyncio.Event
    ) -> None:
        rendition = source.add_rendition(self.name, **self._options)
        if self._video_codec is not None:
            rendition.configure_video(self._video_codec)
        self._rendition = rendition
        self._session_ended = session_ended
        self._announced = announced


def _check_optional_unsigned(what: str, value: int | None, bits: int) -> None:
    """As `_check_unsigned`, for an option the native source takes as absent when it is None."""
    if value is not None:
        _check_unsigned(what, value, bits)


def _check_unsigned(what: str, value: int, bits: int) -> None:
    """The native source takes `value` as an unsigned integer of `bits` bits: raises TypeError for
    what is no integer, as a float or None, and ValueError for one out of range."""
    try:
        # What the native source does to take it.
        number = operator.index(value)
    except TypeError:
        raise TypeError(f"{what} must be an integer: {value!r}") from None
    if not 0 <= number < 1 << bits:
        raise ValueError(f"{what} must be between 0 and {(1 << bits) - 1}: {value}")


def _check_type(what: str, value: object, kind: type) -> None:
    """The native source takes `value` only as a `kind`."""
    if not isinstance(value, kind):
        raise TypeError(f"{what} must be a {kind.__name__}: {value!r}")


def _check_video_codec(codec: str) -> None:
    if codec.split(".")[0] not in VIDEO_CODECS:
        raise ValueError(f"unsupported video codec: {codec}")


def _call_native(call: Callable[[], bool | None], session_ended: asyncio.Future[None] | None) -> bool:
    """`call()`, a push or a configuration; returns whether the native source took it, which a
    call returning False, as a push its full queue dropped, did not. Once the session ended, it is
    dropped before it reaches the native source, which may still queue it with nothing left to
    send it. Its arguments are checked before, so the native source refuses it only once its
    publishing stopped: one refused as the session ended is dropped, and the error that stopped
    publishing in a live session is raised."""
    if session_ended is not None and session_ended.done():
        return False
    try:
        taken = call()
    except RuntimeError as error:
        if session_ended is not None and session_ended.done():
            logger.warning("media dropped: %s", error)
            return False
        raise
    return taken is not False
