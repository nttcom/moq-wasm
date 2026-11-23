"""PyAV codecs for the media of LOC tracks. Needs the `codecs` extra, `moq-wasm[codecs]`: the wheel
carries no codec of its own (docs/architecture/python/sdk.md "No codec in the wheel")."""

import asyncio
import functools
import operator
import time
from collections import deque
from collections.abc import AsyncIterable, Callable
from dataclasses import dataclass
from fractions import Fraction
from typing import cast

import av
import numpy as np
from av.error import FFmpegError

from moq_wasm._native import Location, Object
from moq_wasm._once import retrieve
from moq_wasm.media import Following

OPUS_SAMPLE_RATE = 48_000
OPUS_BITRATE = 32_000

# The Object Status of an object that ends its group (draft-ietf-moq-transport-14 §10.2.1.1).
END_OF_GROUP_STATUS = 3
# How many groups `decode_pictures` decodes at once: the objects of groups in flight interleave, and a
# group still open when this many newer ones opened is taken as complete.
MAX_OPEN_GROUPS = 4
# How many complete groups `decode_pictures` remembers, to skip their late objects.
FINISHED_GROUPS_KEPT = 16
# How many objects in a row `decode_pictures` lets the decoder of a group joined at its keyframe
# refuse before it takes the track as undecodable.
MAX_REFUSED_OBJECTS = 3

# A video codec string's first element, as the catalog writes it, to the PyAV decoders that decode
# it in software, the preferred first. FFmpeg's own AV1 decoder (`av1`) is left out: it decodes only
# through a hardware accelerator, which PyAV does not set up, and refuses every packet otherwise.
VIDEO_DECODERS = {
    "avc1": ("h264",),
    "avc3": ("h264",),
    "hvc1": ("hevc",),
    "hev1": ("hevc",),
    "vp8": ("vp8", "libvpx"),
    "vp09": ("vp9", "libvpx-vp9"),
    "av01": ("libdav1d", "libaom-av1"),
}
# The decoders that decode on one thread. dav1d's worker threads release a packet's payload and
# its opaque value, the object's Location, as they finish a frame, which takes the GIL; freeing the
# decoder holds the GIL and waits for those threads, so the process deadlocks. The other decoders
# may keep a packet's payload or opaque value past the call that decodes it, as FFmpeg's VP9 does,
# but release it on the thread that calls them, which holds the GIL already: FFmpeg's own ones
# thread by slice, which PyAV defaults to, and libvpx and libaom decode synchronously. So their
# threads stay, and dav1d alone decodes slower.
THREADLESS_DECODERS = ("libdav1d",)


def _decoder_name(codec: str) -> str | None:
    """The PyAV decoder `VideoDecoder` takes for `codec`, a WebCodecs codec string: the first of its
    `VIDEO_DECODERS` this PyAV build has, or None when it has none of them."""
    family = codec.split(".")[0]
    # Only the families `VIDEO_DECODERS` lists are looked up, and remembered, so the codec strings
    # of catalogs, unsupported ones included, do not pile up.
    return _family_decoder_name(family) if family in VIDEO_DECODERS else None


@functools.cache
def _family_decoder_name(family: str) -> str | None:
    for name in VIDEO_DECODERS[family]:
        try:
            av.Codec(name, "r")
        except ValueError:
            continue
        return name
    return None


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
    each group gets a decoder of its own; a group joined in the middle decodes to nothing. A
    codec with reordered frames (H.264 B-frames) returns a picture a few objects after the one it
    was decoded from, and its last ones from `flush`; each comes with the Location given with
    its object."""

    def __init__(self, codec: str):
        name = _decoder_name(codec)
        if name is None:
            raise ValueError(f"no decoder for video codec {codec}")
        self._decoder = cast(av.VideoCodecContext, av.CodecContext.create(name, "r"))
        if name in THREADLESS_DECODERS:
            self._decoder.thread_count = 1
        # Each packet carries the Location of its object through the decoder to the pictures it
        # makes, so a packet that makes none, as an invisible VP8 frame, leaves nothing behind.
        self._decoder.copy_opaque = True
        self._next_pts = 0

    def decode(self, payload: bytes, location: Location | None = None) -> list[tuple[av.VideoFrame, Location | None]]:
        """The pictures the object completes, in presentation order, each with the `location` of
        the object it was decoded from; none for an object the decoder refuses."""
        try:
            return self._decode(payload, location)
        except FFmpegError:
            return []

    def _decode(self, payload: bytes, location: Location | None) -> list[tuple[av.VideoFrame, Location | None]]:
        """`decode`, raising the FFmpegError of an object the decoder refuses."""
        packet = av.Packet(payload)
        packet.pts = self._next_pts
        packet.opaque = location
        self._next_pts += 1
        return self._located(self._decoder.decode(packet))

    def flush(self) -> list[tuple[av.VideoFrame, Location | None]]:
        """The pictures the decoder still holds, at the end of the group."""
        try:
            return self._located(self._decoder.decode(None))
        except FFmpegError:
            return []

    def _located(self, pictures: list[av.VideoFrame]) -> list[tuple[av.VideoFrame, Location | None]]:
        return [(picture, cast(Location | None, picture.opaque)) for picture in pictures if picture.pts is not None]


class _Group:
    """The decoding of one group of a video track. A group joined at its keyframe, its object 0,
    should decode: the decoder refusing `MAX_REFUSED_OBJECTS` of its objects in a row, or the group
    ending without a picture after it refused one, raises the decoder's error. A group joined
    midway decodes to nothing, which is expected."""

    def __init__(self, codec: str, from_keyframe: bool):
        self._decoder = VideoDecoder(codec)
        self._from_keyframe = from_keyframe
        self._refused_in_a_row = 0
        self._refusal: FFmpegError | None = None
        self._pictured = False

    def decode(self, payload: bytes, location: Location) -> list[tuple[av.VideoFrame, Location | None]]:
        """The pictures the object completes."""
        try:
            pictures = self._decoder._decode(payload, location)
        except FFmpegError as error:
            self._refused_in_a_row += 1
            self._refusal = error
            if self._from_keyframe and self._refused_in_a_row >= MAX_REFUSED_OBJECTS:
                raise
            return []
        self._refused_in_a_row = 0
        self._pictured = self._pictured or bool(pictures)
        return pictures

    def flush(self) -> list[tuple[av.VideoFrame, Location | None]]:
        pictures = self._decoder.flush()
        if self._from_keyframe and not (self._pictured or pictures) and self._refusal is not None:
            raise self._refusal
        return pictures


@dataclass(frozen=True)
class DecodedPicture:
    picture: av.VideoFrame
    location: Location
    """The Location of the object the picture was decoded from."""
    decoded_at: float
    """`time.monotonic()` when it was decoded."""


def decode_pictures(
    objects: AsyncIterable[Object], codec: str, *, interval: float = 0.0
) -> Following[DecodedPicture]:
    """Decodes the objects of one subscription to a LOC video track, such as one that
    `RemoteBroadcast.subscriptions(name)` yields, in a thread and yields every picture, in
    presentation order within its group, or at most one every `interval` seconds. A track followed
    across its subscriptions gives each subscription a `decode_pictures` of its own: the groups of
    the next subscription, as of a publisher that took over, may start at 0 again, which one
    `decode_pictures` takes as groups it already decoded. Each group gets a `VideoDecoder` of its
    own, since it starts at a keyframe and the objects of groups in flight may interleave; a group
    joined midway decodes to nothing. A group's decoder is flushed at its End of Group status
    object, when `MAX_OPEN_GROUPS` newer groups opened since (the objects of that group arriving
    later are skipped), and at the end of `objects`.

    Raises ValueError at once when no decoder takes `codec`; its reads raise the decoder's
    `av.error.FFmpegError` when a group joined at its keyframe does not decode: the decoder refused
    `MAX_REFUSED_OBJECTS` of its objects in a row, or the group ended without a picture after it
    refused one.

    `objects` stays the caller's: each read advances it in the reader's own task and context, and
    neither closing nor dropping the decoding cancels or closes it. A read cancelled, as by
    `asyncio.wait_for` timing out, does not end the decoding: the object being decoded in a thread
    goes on for the next read, as a `Following`'s state does; but a cancellation that reaches a
    read of `objects` reaches `objects` itself, which an async generator does not survive. Read
    `RemoteBroadcast.subscriptions()`'s objects or a `moq_wasm.Subscription` to read with a
    timeout. `aclose()` stops the decoding at once: a read waiting on `objects` raises
    StopAsyncIteration once `objects` gives it its object, which is dropped, and no read reads
    `objects` after it. Closed, the decoding keeps no decoder or picture, once a decode in flight
    in a worker finished, also while a read still waits on `objects`."""
    return _DecodedPictures(objects, codec, interval)


class _DecodedPictures(Following[DecodedPicture]):
    """What `decode_pictures` returns: the pictures of each object, decoded whole in a thread, the
    object in flight kept across a cancelled read."""

    # Its reads await `objects`, which are the caller's.
    _interrupts = False

    def __init__(self, objects: AsyncIterable[Object], codec: str, interval: float):
        super().__init__("the pictures")
        # None once closed: the decoders and the pictures they hold are freed, once a decoding in a
        # worker finished.
        self._decoder: _PictureDecoder | None = _PictureDecoder(codec, interval)
        self._objects = aiter(objects)
        self._objects_ended = False
        # The decoding of the latest object in a thread, or of the end of the objects.
        self._decoding: asyncio.Future[list[DecodedPicture]] | None = None
        self._pictures: deque[DecodedPicture] = deque()

    async def _next(self) -> DecodedPicture:
        while not self._pictures:
            decoding = self._decoding
            if decoding is None:
                # Closed while an object that gave no picture was decoded, the objects are not read
                # again.
                if self._objects_ended or self._closing.started:
                    raise StopAsyncIteration
                try:
                    video_object = await anext(self._objects)
                except StopAsyncIteration:
                    self._objects_ended = True
                    video_object = None
                decoder = self._decoder
                if decoder is None or self._closing.started:
                    raise StopAsyncIteration
                step = decoder.finish if video_object is None else functools.partial(decoder.take, video_object)
                decoding = self._decoding = _in_thread(step)
                # The read may wait on the next object once this one gave no picture: it holds no
                # decoder meanwhile, so closing the decoding frees it.
                del decoder, step
            await asyncio.wait([decoding])
            if self._closing.started:
                raise StopAsyncIteration
            self._decoding = None
            self._pictures.extend(decoding.result())
        return self._pictures.popleft()

    async def _release(self) -> None:
        # A read in flight, waiting on the caller's objects or on a decoding in a worker, is not
        # waited for: it finds the decoding closed and drops what it read.
        self._decoder = None
        self._decoding = None
        self._pictures.clear()


def _in_thread(step: Callable[[], list[DecodedPicture]]) -> asyncio.Future[list[DecodedPicture]]:
    decoding = asyncio.get_running_loop().run_in_executor(None, step)
    # Its failure goes to the read, and to nobody once the decoding was dropped.
    decoding.add_done_callback(retrieve)
    return decoding


class _PictureDecoder:
    """The decoding state of `decode_pictures`, as its docstring describes it. Its steps are
    synchronous, so each runs whole in a worker thread, and the state is never left halfway through
    an object."""

    def __init__(self, codec: str, interval: float):
        # Raises ValueError for an unknown codec.
        VideoDecoder(codec)
        self._codec = codec
        self._interval = interval
        self._next_picture_at = 0.0
        # Each group's decoding, in the order the groups opened.
        self._groups: dict[int, _Group] = {}
        self._finished: deque[int] = deque(maxlen=FINISHED_GROUPS_KEPT)

    def take(self, video_object: Object) -> list[DecodedPicture]:
        """The pictures `video_object` completes."""
        groups = self._groups
        group_id = video_object.group_id
        if video_object.status is not None:
            return self._finish(group_id) if video_object.status == END_OF_GROUP_STATUS else []
        if group_id in self._finished:
            return []
        pictures = []
        group = groups.get(group_id)
        if group is None:
            if len(groups) >= MAX_OPEN_GROUPS:
                pictures.extend(self._finish(next(iter(groups))))
            group = groups[group_id] = _Group(self._codec, video_object.object_id == 0)
        location = Location(group_id, video_object.object_id)
        return pictures + self._sampled(group.decode(video_object.payload, location))

    def finish(self) -> list[DecodedPicture]:
        """The pictures the groups still open hold back, at the end of the objects."""
        pictures = []
        while self._groups:
            pictures.extend(self._finish(min(self._groups)))
        return pictures

    def _sampled(self, pictures: list[tuple[av.VideoFrame, Location | None]]) -> list[DecodedPicture]:
        decoded_at = time.monotonic()
        passed = []
        for picture, location in pictures:
            if location is None or decoded_at < self._next_picture_at:
                continue
            self._next_picture_at = decoded_at + self._interval
            passed.append(DecodedPicture(picture, location, decoded_at))
        return passed

    def _finish(self, group_id: int) -> list[DecodedPicture]:
        """Flushes the group."""
        self._finished.append(group_id)
        group = self._groups.pop(group_id, None)
        return [] if group is None else self._sampled(group.flush())


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
