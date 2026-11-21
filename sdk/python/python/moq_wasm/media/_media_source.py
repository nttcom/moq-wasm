import asyncio
import logging
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
    before the broadcast is published, or after it ended, are dropped."""

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
        self.video = video
        self.audio = audio
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
        # Ended once the broadcast was left or the session ended, which stop the source, or by
        # what kept the broadcast from being published.
        self._ended = Ended()
        self._session_ended: asyncio.Future[None] | None = None
        # Set by the broadcast the source is added to: claims the tracks of a rendition, raising
        # RuntimeError while another track has the name of one of them.
        self._claim_tracks: Callable[[list[str]], None] | None = None

    def configure_video(self, codec: str) -> None:
        """`codec` is a WebCodecs codec string: `avc1.*` (Annex B), `hvc1.*` (Annex B), `vp8`,
        `vp09.*` or `av01.*`. Raises ValueError for another codec."""
        _check_video_codec(codec)
        if self._source is not None:
            self._source.configure_video(codec)
        self._video_codec = codec

    def add_rendition(
        self, name: str, *, label: str, bitrate: int | None = None, framerate: int | None = None
    ) -> "Rendition":
        """Another encoding of the video, in the track `<video>_<name>`, and with `cmaf` its
        `_cmaf` sibling (`_native.MediaSource.add_rendition`): feed it frames of the same pictures,
        with a keyframe on the same pictures as the video's. `bitrate` (bits per second) and
        `framerate` go into the catalog. Raises RuntimeError while another track of the broadcast,
        or once it was entered of the session, has the name of one of its tracks, and ValueError
        for a track the session registered in datagrams or with another retention than 60 s."""
        if self._claim_tracks is not None:
            self._claim_tracks(self._with_cmaf(f"{self.video}_{name}"))
        rendition = Rendition(name, {"label": label, "bitrate": bitrate, "framerate": framerate})
        self._renditions.append(rendition)
        if self._source is not None:
            rendition._build(self._source)
        return rendition

    def push_video(self, data: bytes, timestamp_micros: int) -> None:
        """One encoded frame; a keyframe opens a group. `timestamp_micros` must not run
        backwards. Raises RuntimeError before `configure_video`."""
        if self._video_codec is None:
            raise RuntimeError("call configure_video first")
        if self._source is not None:
            _drop_once_ended(lambda: self._source.push_video(data, timestamp_micros))

    def configure_audio(
        self, codec: str, sample_rate: int, channels: int, *, description: bytes | None = None
    ) -> None:
        """`"opus"`, or an AAC `"mp4a.40.*"` whose `description` is the AudioSpecificConfig;
        raises ValueError for another codec or an AAC without it."""
        if codec != "opus" and not codec.startswith("mp4a.40."):
            raise ValueError(f"unsupported audio codec: {codec}")
        if codec != "opus":
            if description is None:
                raise ValueError("AAC needs its AudioSpecificConfig")
            _native.check_audio_specific_config(description)
        if self._source is not None:
            self._source.configure_audio(codec, sample_rate, channels, description=description)
        self._replaced_track_names.update(self._registered_track_names())
        self._audio_config = (codec, sample_rate, channels, description)

    def push_audio(
        self, data: bytes, timestamp_micros: int, *, level: int | None = None, voice_activity: bool = False
    ) -> None:
        """One encoded audio frame; `level` and `voice_activity` as in
        `_native.MediaSource.push_audio`. Raises RuntimeError before `configure_audio`."""
        if self._audio_config is None:
            raise RuntimeError("call configure_audio first")
        if self._source is not None:
            _drop_once_ended(
                lambda: self._source.push_audio(
                    data, timestamp_micros, level=level, voice_activity=voice_activity
                )
            )

    async def remove_track(self, kind: str) -> None:
        """`_native.MediaSource.remove_track` (`kind` is `"video"` or `"audio"`): stop pushing its
        media first; `configure_video` or `configure_audio` and the media pushed after it publish
        the track again. Before the broadcast is published, it forgets the kind's configuration
        (the renditions' too, for the video). Raises ValueError for another kind."""
        if kind not in ("video", "audio"):
            raise ValueError(f"unknown track kind: {kind}")
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
        not published; raises RuntimeError once the broadcast was left or the session ended."""
        await self._while_publishing(lambda source: source.keyframe_request())

    async def demand_change(self) -> _native.Demand:
        """`_native.MediaSource.demand_change`; waits while the broadcast is not published. Raises
        RuntimeError for a source added with `on_demand=False`, and once the broadcast was left or the
        session ended."""
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
        stopped = asyncio.ensure_future(self._ended.until(answered))
        ending = [] if self._session_ended is None else [self._session_ended]
        try:
            await asyncio.wait([stopped, *ending], return_when=asyncio.FIRST_COMPLETED)
        finally:
            stopped.cancel()
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
            rendition._build(source)
        self._built.set()

    def _stopped_error(self) -> BaseException:
        return self._ended.error() if self._ended.ended else RuntimeError("the media source stopped publishing")

    def _detach(
        self, error: Exception | None = None, *, failure: bool = False, raised: Callable[[], None] | None = None
    ) -> None:
        """Stops the source, which ends it with `error`, what kept its broadcast from being
        published, unless it ended before. `raised` is called each time it is raised."""
        self._ended.end(error or RuntimeError("the media source stopped publishing"), failure=failure, raised=raised)
        self._source = None
        for rendition in self._renditions:
            rendition._rendition = None

    def _stop_with_session(self, _: BaseException) -> None:
        """The session ended: the requests raise that the source stopped, and the pushes reach the
        native source, which drops them."""
        self._ended.end(RuntimeError("the media source stopped publishing"), failure=False)


class Rendition:
    """Another encoding of a `MediaSource`'s video. Frames pushed before the broadcast is
    published, or after it ended, are dropped."""

    def __init__(self, name: str, options: dict):
        self.name = name
        self._options = options
        self._rendition: _native.Rendition | None = None
        self._video_codec: str | None = None

    def configure_video(self, codec: str) -> None:
        """As `MediaSource.configure_video`; it may differ from the video's codec."""
        _check_video_codec(codec)
        if self._rendition is not None:
            self._rendition.configure_video(codec)
        self._video_codec = codec

    def push_video(self, data: bytes, timestamp_micros: int) -> None:
        """An encoded frame of the same picture as a video frame, at that frame's timestamp. Raises
        RuntimeError before `configure_video`."""
        if self._video_codec is None:
            raise RuntimeError("call configure_video first")
        if self._rendition is not None:
            _drop_once_ended(lambda: self._rendition.push_video(data, timestamp_micros))

    def _build(self, source: _native.MediaSource) -> None:
        rendition = source.add_rendition(self.name, **self._options)
        if self._video_codec is not None:
            rendition.configure_video(self._video_codec)
        self._rendition = rendition


def _check_video_codec(codec: str) -> None:
    if codec.split(".")[0] not in VIDEO_CODECS:
        raise ValueError(f"unsupported video codec: {codec}")


def _drop_once_ended(push) -> None:
    """The configuration is checked before, so the native source refuses a frame only once its
    publishing stopped, as when the session ended."""
    try:
        push()
    except RuntimeError as error:
        logger.warning("frame dropped: %s", error)
