import asyncio
import logging

from moq_wasm import _native

logger = logging.getLogger("moq_wasm")

# The first element of the WebCodecs codec strings `broadcast` publishes as LOC.
VIDEO_CODECS = {"avc1", "avc3", "hvc1", "hev1", "vp8", "vp09", "av01"}


class MediaSource:
    """A video and an audio track of a broadcast, fed with encoded frames, as the browser SDK's
    source is: `broadcast` builds the catalog, the LOC tracks and the media timeline. On a new
    connection the source is built again with its configuration, and `keyframe_request` resolves so
    that the next frame opens a group; frames pushed while the session reconnects are dropped."""

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
        self._audio_config: tuple[str, int, int] | None = None
        self._rebuilt = asyncio.Event()

    def configure_video(self, codec: str) -> None:
        """`codec` is a WebCodecs codec string: `avc1.*` (Annex B), `hvc1.*` (Annex B), `vp8`,
        `vp09.*` or `av01.*`. Raises ValueError for another codec."""
        if codec.split(".")[0] not in VIDEO_CODECS:
            raise ValueError(f"unsupported video codec: {codec}")
        if self._source is not None:
            self._source.configure_video(codec)
        self._video_codec = codec

    def push_video(self, data: bytes, timestamp_micros: int) -> None:
        """One encoded frame; a keyframe opens a group. `timestamp_micros` must not run
        backwards. Raises RuntimeError before `configure_video`."""
        if self._video_codec is None:
            raise RuntimeError("call configure_video first")
        if self._source is not None:
            _drop_on_lost_connection(self._source.push_video, data, timestamp_micros)

    def configure_audio(self, codec: str, sample_rate: int, channels: int) -> None:
        """Only `"opus"`; raises ValueError for another codec."""
        if codec != "opus":
            raise ValueError(f"unsupported audio codec: {codec}")
        if self._source is not None:
            self._source.configure_audio(codec, sample_rate, channels)
        self._audio_config = (codec, sample_rate, channels)

    def push_audio(self, data: bytes, timestamp_micros: int) -> None:
        """One encoded audio frame. Raises RuntimeError before `configure_audio`."""
        if self._audio_config is None:
            raise RuntimeError("call configure_audio first")
        if self._source is not None:
            _drop_on_lost_connection(self._source.push_audio, data, timestamp_micros)

    async def keyframe_request(self) -> None:
        """Resolves the next time a subscriber asks for a new group (NEW_GROUP_REQUEST), the
        broadcast needs one, or a new connection rebuilt the source: encode the next frame as a
        keyframe then."""
        rebuilt = asyncio.ensure_future(self._rebuilt.wait())
        waits = [rebuilt]
        if self._source is not None:
            waits.append(asyncio.ensure_future(self._source.keyframe_request()))
        try:
            await asyncio.wait(waits, return_when=asyncio.FIRST_COMPLETED)
        finally:
            for wait in waits:
                wait.cancel()
        self._rebuilt.clear()

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
            source.configure_audio(*self._audio_config)
        self._source = source
        self._rebuilt.set()

    def _detach(self) -> None:
        self._source = None


def _drop_on_lost_connection(push, data: bytes, timestamp_micros: int) -> None:
    """The configuration is checked before, so the native source refuses a frame once its
    publishing stopped, which a lost connection causes; the next connection rebuilds it."""
    try:
        push(data, timestamp_micros)
    except RuntimeError as error:
        logger.warning("frame dropped: %s", error)
