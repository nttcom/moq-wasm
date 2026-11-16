import asyncio
import logging

from moq_wasm import _native

logger = logging.getLogger("moq_wasm")

# The first element of the WebCodecs codec strings `broadcast` publishes as LOC.
VIDEO_CODECS = {"avc1", "avc3", "hvc1", "hev1", "vp8", "vp09", "av01"}


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
    ):
        self.video = video
        self.audio = audio
        self._label = label
        self._video_bitrate = video_bitrate
        self._video_framerate = video_framerate
        self._source: _native.MediaSource | None = None
        self._video_codec: str | None = None
        self._audio_config: tuple[str, int, int, bytes | None] | None = None
        self._renditions: list[Rendition] = []
        self._built = asyncio.Event()

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
        """Another encoding of the video, in the track `<video>_<name>`
        (`_native.MediaSource.add_rendition`): feed it frames of the same pictures, with a keyframe
        on the same pictures as the video's. `bitrate` (bits per second) and `framerate` go into
        the catalog."""
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

    async def keyframe_request(self) -> None:
        """Resolves the next time a subscriber asks for a new group (NEW_GROUP_REQUEST) or the
        broadcast needs one: encode the next frame as a keyframe then. Waits while the broadcast is
        not published."""
        await self._built.wait()
        assert self._source is not None
        await self._source.keyframe_request()

    def _build(self, broadcast: _native.MediaBroadcast) -> None:
        source = broadcast.add_source(
            self.video,
            self.audio,
            label=self._label,
            video_bitrate=self._video_bitrate,
            video_framerate=self._video_framerate,
        )
        if self._video_codec is not None:
            source.configure_video(self._video_codec)
        if self._audio_config is not None:
            codec, sample_rate, channels, description = self._audio_config
            source.configure_audio(codec, sample_rate, channels, description=description)
        self._source = source
        for rendition in self._renditions:
            rendition._build(source)
        self._built.set()

    def _detach(self) -> None:
        self._source = None
        for rendition in self._renditions:
            rendition._rendition = None
        self._built.clear()


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
