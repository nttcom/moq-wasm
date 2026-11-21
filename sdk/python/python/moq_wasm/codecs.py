"""PyAV codecs for the media of LOC tracks. Needs the `codecs` extra, `moq-wasm[codecs]`: the wheel
carries no codec of its own (docs/architecture/python/sdk.md "No codec in the wheel")."""

import asyncio
import operator
import time
from collections.abc import AsyncIterable, AsyncIterator
from dataclasses import dataclass
from fractions import Fraction

import av
import numpy as np

from moq_wasm._native import Location, Object

OPUS_SAMPLE_RATE = 48_000
OPUS_BITRATE = 32_000

# A video codec string's first element, as the catalog writes it, to the PyAV decoder.
VIDEO_DECODERS = {"avc1": "h264", "avc3": "h264", "hvc1": "hevc", "hev1": "hevc", "vp8": "vp8", "vp09": "vp9", "av01": "av1"}


class OpusDecoder:
    """Turns the Opus packets of a LOC audio track into mono 16-bit PCM at `sample_rate`."""

    def __init__(self, sample_rate: int):
        self._decoder = av.CodecContext.create("opus", "r")
        self._resampler = av.AudioResampler(format="s16", layout="mono", rate=sample_rate)

    def decode(self, packet: bytes) -> bytes:
        return b"".join(
            bytes(resampled.planes[0])[: resampled.samples * 2]
            for decoded in self._decoder.decode(av.Packet(packet))
            for resampled in self._resampler.resample(decoded)
        )


class OpusEncoder:
    """Turns mono 16-bit PCM at any rate into 20 ms Opus packets at 48 kHz."""

    def __init__(self):
        self._encoder = av.CodecContext.create("libopus", "w")
        self._encoder.sample_rate = OPUS_SAMPLE_RATE
        self._encoder.layout = "mono"
        self._encoder.format = "s16"
        self._encoder.bit_rate = OPUS_BITRATE
        self._encoder.open()
        # A resampler takes the rate of its first frame only, so each input rate gets one of its own.
        self._resampler = av.AudioResampler(format="s16", layout="mono", rate=OPUS_SAMPLE_RATE)
        self._input_rate: int | None = None
        self._fifo = av.AudioFifo()
        self.frame_samples = self._encoder.frame_size

    def encode(self, pcm: bytes, sample_rate: int) -> list[bytes]:
        """Returns the packets `pcm`, mono 16-bit PCM at `sample_rate`, completes; the samples short
        of a packet wait for the next call. The rate may change from one call to the next. Raises
        ValueError, taking none of `pcm`, for a `sample_rate` that is not a positive integer."""
        try:
            rate = operator.index(sample_rate)
        except TypeError:
            rate = 0
        if rate <= 0:
            raise ValueError(f"the sample rate must be a positive integer: {sample_rate!r}")
        frame = av.AudioFrame.from_ndarray(np.frombuffer(pcm, np.int16).reshape(1, -1), format="s16", layout="mono")
        frame.sample_rate = rate
        if rate != self._input_rate:
            self._write(self._resampler.resample(None))
            self._resampler = av.AudioResampler(format="s16", layout="mono", rate=OPUS_SAMPLE_RATE)
            self._input_rate = rate
        self._write(self._resampler.resample(frame))
        packets = []
        while (chunk := self._fifo.read(self.frame_samples)) is not None:
            chunk.pts = None
            packets.extend(bytes(packet) for packet in self._encoder.encode(chunk))
        return packets

    def _write(self, resampled: list[av.AudioFrame]) -> None:
        for frame in resampled:
            frame.pts = None
            self._fifo.write(frame)


class VideoDecoder:
    """Decodes one group of a LOC video track into pictures. A group starts at a keyframe, so
    each group gets a decoder of its own; a group joined in the middle decodes to nothing."""

    def __init__(self, codec: str):
        name = VIDEO_DECODERS.get(codec.split(".")[0])
        if name is None:
            raise ValueError(f"no decoder for video codec {codec}")
        self._decoder = av.CodecContext.create(name, "r")

    def decode(self, payload: bytes) -> av.VideoFrame | None:
        try:
            pictures = self._decoder.decode(av.Packet(payload))
        except av.FFmpegError:
            return None
        return pictures[-1] if pictures else None


@dataclass(frozen=True)
class DecodedPicture:
    picture: av.VideoFrame
    location: Location
    """The Location of the object the picture was decoded from."""
    decoded_at: float
    """`time.monotonic()` when it was decoded."""


async def decode_pictures(
    objects: AsyncIterable[Object], codec: str, *, interval: float = 0.0
) -> AsyncIterator[DecodedPicture]:
    """Decodes the objects of a LOC video track, such as `RemoteBroadcast.follow(name)`, in a
    thread and yields at most one picture every `interval` seconds. Each group gets a fresh
    `VideoDecoder`, since it starts at a keyframe; a group joined midway decodes to nothing. Raises
    ValueError when no decoder takes `codec`."""
    next_picture_at = 0.0
    decoder: VideoDecoder | None = None
    group_id: int | None = None
    async for video_object in objects:
        if video_object.group_id != group_id:
            decoder = VideoDecoder(codec)
            group_id = video_object.group_id
        picture = await asyncio.to_thread(decoder.decode, video_object.payload)
        decoded_at = time.monotonic()
        if picture is None or decoded_at < next_picture_at:
            continue
        next_picture_at = decoded_at + interval
        yield DecodedPicture(picture, Location(video_object.group_id, video_object.object_id), decoded_at)


# ITU-T H.264 Table A-1: level_idc, MaxMBPS (macroblocks/s), MaxFS (macroblocks/frame). Level 1b is omitted.
H264_LEVELS = [
    (10, 1485, 99),
    (11, 3000, 396),
    (12, 6000, 396),
    (13, 11880, 396),
    (20, 11880, 396),
    (21, 19800, 792),
    (22, 20250, 1620),
    (30, 40500, 1620),
    (31, 108000, 3600),
    (32, 216000, 5120),
    (40, 245760, 8192),
    (41, 245760, 8192),
    (42, 522240, 8704),
    (50, 589824, 22080),
    (51, 983040, 36864),
    (52, 2073600, 36864),
    (60, 4177920, 139264),
    (61, 8355840, 139264),
    (62, 16711680, 139264),
]


def _h264_level(width: int, height: int, framerate: int) -> int:
    """Returns the lowest level whose limits take this size and framerate (A.3.1): macroblocks per
    frame <= MaxFS, each side <= sqrt(8 MaxFS), macroblocks per second <= MaxMBPS. Returns the
    highest level when none does."""
    mb_width, mb_height = -(-width // 16), -(-height // 16)
    frame_size = mb_width * mb_height
    for level, max_mbps, max_fs in H264_LEVELS:
        if frame_size <= max_fs and max(mb_width, mb_height) ** 2 <= 8 * max_fs and frame_size * framerate <= max_mbps:
            return level
    return H264_LEVELS[-1][0]


# VideoEncoder.encode's color formats -> (PyAV pixel format, bytes per pixel).
COLOR_FORMATS = {"RGB": ("rgb24", 3), "RGBA": ("rgba", 4), "BGR": ("bgr24", 3), "BGRA": ("bgra", 4)}


def _checked_picture(image: bytes, size: tuple[int, int], color_format: str) -> tuple[bytes, tuple[int, int]]:
    if color_format not in COLOR_FORMATS:
        raise ValueError(f"unsupported color format {color_format!r}: use one of {', '.join(COLOR_FORMATS)}")
    try:
        width, height = (operator.index(side) for side in size)
    except (TypeError, ValueError):
        raise ValueError(f"{size!r} is not a width and a height in pixels") from None
    try:
        view = memoryview(image)
    except TypeError:
        raise ValueError(f"a {type(image).__name__} is no picture: use bytes") from None
    if not view.c_contiguous:
        raise ValueError("a picture's buffer must be contiguous")
    if width <= 0 or height <= 0 or view.nbytes != width * height * COLOR_FORMATS[color_format][1]:
        raise ValueError(f"{view.nbytes} bytes are not {width}x{height} pixels in {color_format}")
    return (image if isinstance(image, bytes) else view.tobytes()), (width, height)


class VideoEncoder:
    """Encodes RGB or BGR pictures into H.264 (Annex B, its parameter sets in every keyframe) or VP8. A
    keyframe comes every two seconds, and on `request_keyframe`. `codec_string` is the WebCodecs
    codec string; for H.264 it names Constrained Baseline at the level the SPS carries."""

    def __init__(self, codec: str, width: int, height: int, framerate: int):
        if codec == "H264":
            level = _h264_level(width, height, framerate)
            # x264's baseline sets constraint_set0_flag and constraint_set1_flag: Constrained Baseline.
            self.codec_string = f"avc1.42C0{level:02X}"
            self._encoder = av.CodecContext.create("libx264", "w")
            self._encoder.options = {
                "preset": "ultrafast",
                "tune": "zerolatency",
                "profile": "baseline",
                "x264-params": f"repeat-headers=1:keyint={2 * framerate}:scenecut=0:level={level}",
            }
        elif codec == "VP8":
            self.codec_string = "vp8"
            self._encoder = av.CodecContext.create("libvpx", "w")
            self._encoder.options = {"deadline": "realtime", "lag-in-frames": "0"}
            self._encoder.gop_size = 2 * framerate
        else:
            raise ValueError(f"unsupported video_out_codec {codec!r}: use 'H264' or 'VP8'")
        self._encoder.width = width
        self._encoder.height = height
        self._encoder.pix_fmt = "yuv420p"
        self._encoder.time_base = Fraction(1, framerate)
        self._encoder.framerate = framerate
        self._encoder.open()
        self._width = width
        self._height = height
        self._frames = 0
        self._keyframe_requested = False

    def request_keyframe(self) -> None:
        self._keyframe_requested = True

    def encode(self, image: bytes, size: tuple[int, int], color_format: str) -> list[bytes]:
        """Returns the packets of `image`, `size` pixels in `color_format` (`RGB`, `RGBA`, `BGR` or
        `BGRA`, 8 bits per channel), scaled to the encoder's size. Raises ValueError for another
        format, a `size` that is not two integers, or an `image` that is not a contiguous buffer of
        exactly that many bytes."""
        image, (width, height) = _checked_picture(image, size, color_format)
        pixel_format, channels = COLOR_FORMATS[color_format]
        picture = av.VideoFrame.from_ndarray(
            np.frombuffer(image, np.uint8).reshape(height, width, channels), format=pixel_format
        ).reformat(width=self._width, height=self._height, format="yuv420p")
        picture.pts = self._frames
        self._frames += 1
        if self._keyframe_requested:
            picture.pict_type = av.video.frame.PictureType.I
            self._keyframe_requested = False
        return [bytes(packet) for packet in self._encoder.encode(picture)]
