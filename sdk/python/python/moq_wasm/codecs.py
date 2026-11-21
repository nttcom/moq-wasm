"""PyAV codecs for the media of LOC tracks. Needs the `codecs` extra, `moq-wasm[codecs]`: the wheel
carries no codec of its own (docs/architecture/python/sdk.md "No codec in the wheel")."""

import asyncio
import functools
import logging
import operator
import time
from collections import deque
from collections.abc import AsyncIterable, Awaitable, Callable
from dataclasses import dataclass, replace
from fractions import Fraction
from typing import cast

import av
import numpy as np
from av.error import FFmpegError

from moq_wasm._native import Location, Object
from moq_wasm._once import retrieve
from moq_wasm.media import Following, RemoteBroadcast
from moq_wasm.media._remote_broadcast import DEFAULT_DELIVERY_TIMEOUT_SECONDS, _Objects

logger = logging.getLogger("moq_wasm")

OPUS_SAMPLE_RATE = 48_000
PCM_LAYOUTS = {1: "mono", 2: "stereo"}
OPUS_BITRATE = 32_000

# The Object Status of an object that ends its group (draft-ietf-moq-transport-14 §10.2.1.1).
END_OF_GROUP_STATUS = 3
# The LOC Video Config property: the codec's extradata, such as an H.264 avcC record
# (draft-ietf-moq-loc-04 §2.3.2.1).
VIDEO_CONFIG_ID = 0x0D
# How many groups `decode_pictures` decodes at once: the objects of groups in flight interleave, and a
# group still open when this many newer ones opened is taken as complete.
MAX_OPEN_GROUPS = 4
# How many complete groups `decode_pictures` remembers, to skip their late objects.
FINISHED_GROUPS_KEPT = 16
# How many objects, and how many bytes of their payloads, a group without a Video Config of its own
# holds while the group before it has not arrived, before `decode_pictures` takes that group as late
# or lost.
MAX_HELD_OBJECTS = 5
MAX_HELD_BYTES = 4 * 1024 * 1024
# How many objects in a row `decode_pictures` lets the decoder of a group joined at its keyframe
# refuse before it takes the track as undecodable.
MAX_REFUSED_OBJECTS = 3
# The first elements of the codec strings whose payloads may be length-prefixed, which only the Video
# Config tells how to read (draft-ietf-moq-loc-04 §2.1.3).
LENGTH_PREFIXED_CODECS = ("avc1", "avc3", "hvc1", "hev1")
# The 4-byte start code an Annex B payload starts with, and the 3-byte one a track without a Video
# Config may use instead (draft-ietf-moq-loc-04 §2.1.4).
START_CODE = b"\x00\x00\x00\x01"
SHORT_START_CODE = b"\x00\x00\x01"
# The NAL unit type of a sequence parameter set: H.264's (ITU-T H.264 Table 7-1) and H.265's (ITU-T
# H.265 Table 7-1).
H264_SPS = 7
HEVC_SPS = 33

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
    """Turns the Opus packets of a LOC audio track into 16-bit PCM at `sample_rate` with `channels`
    interleaved channels, mono or stereo, mixed from the track's own. Raises ValueError for another
    channel count."""

    def __init__(self, sample_rate: int, channels: int = 1):
        self._channels = channels
        # `kind` types the context as audio, which PyAV 15 does not for
        # `CodecContext.create("opus")`.
        self._decoder = av.Codec("opus", "r").create("audio")
        self._resampler = av.AudioResampler(format="s16", layout=_pcm_layout(channels), rate=sample_rate)

    def decode(self, packet: bytes) -> bytes:
        return b"".join(
            bytes(resampled.planes[0])[: resampled.samples * self._channels * 2]
            for decoded in self._decoder.decode(av.Packet(packet))
            for resampled in self._resampler.resample(decoded)
        )


def _pcm_layout(num_channels: int) -> str:
    layout = PCM_LAYOUTS.get(num_channels)
    if layout is None:
        raise ValueError(f"cannot take {num_channels} channels: give mono or stereo PCM")
    return layout


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
    its object. `config` is the track's LOC Video Config, the codec's extradata, which
    length-prefixed H.264 and H.265 need (an avcC or hvcC record)."""

    def __init__(self, codec: str, config: bytes | None = None):
        name = _decoder_name(codec)
        if name is None:
            raise ValueError(f"no decoder for video codec {codec}")
        self._decoder = cast(av.VideoCodecContext, av.CodecContext.create(name, "r"))
        if name in THREADLESS_DECODERS:
            self._decoder.thread_count = 1
        if config is not None:
            self._decoder.extradata = config
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
    """The decoding of one group of a video track, with the LOC Video Config it started with. An
    object carrying another Video Config, a keyframe whose parameters changed, starts a decoder with
    it, after the pictures the previous one held back. A group joined at its keyframe, its object 0,
    should decode: the decoder refusing `MAX_REFUSED_OBJECTS` of its objects in a row, or the group
    ending without a picture after it refused one, raises the decoder's error. A group joined midway
    decodes to nothing, which is expected. An H.264 or H.265 object of a group without a Video
    Config is read as Annex B when it starts with a start code, 4-byte or 3-byte, and else as 4-byte
    length prefixes; one that reads either way is read as the group's earlier objects were. Length
    prefixes are rewritten as start codes once the group carried its parameter sets in band (an
    SPS), as `avc3` and `hev1` may. One readable neither way, or length-prefixed before the group's
    parameter sets, is skipped without being decoded, which could make garbage of it, and, in a
    group joined at its keyframe, which would have carried them, `unconfigured` is called."""

    def __init__(self, codec: str, config: bytes | None, from_keyframe: bool, unconfigured: Callable[[], None]):
        self._codec = codec
        self._unconfigured = unconfigured
        family = codec.split(".")[0]
        self._length_prefixed = family in LENGTH_PREFIXED_CODECS
        self._hevc = family in ("hvc1", "hev1")
        # Whether a length-prefixed object of the group carried an SPS, which a group without a
        # Video Config needs before its length-prefixed objects decode.
        self._in_band_parameter_sets = False
        # Whether the group's objects are Annex B, as its first one readable either way was; None
        # before it.
        self._framed_annex_b: bool | None = None
        # The Video Config the group decodes with now.
        self.config = config
        self._decoder = VideoDecoder(codec, config)
        self._from_keyframe = from_keyframe
        self._refused_in_a_row = 0
        self._refusal: FFmpegError | None = None
        self._pictured = False

    def decode(
        self, payload: bytes, location: Location, config: bytes | None
    ) -> list[tuple[av.VideoFrame, Location | None]]:
        """The pictures the object completes; `config` is the Video Config the object carries."""
        held_back = []
        if config is not None and config != self.config:
            held_back = self._decoder.flush()
            self._pictured = self._pictured or bool(held_back)
            self.config = config
            self._decoder = VideoDecoder(self._codec, config)
        if self.config is None and self._length_prefixed:
            annex_b = self._annex_b(payload)
            if annex_b is None:
                # Without the parameter sets, as after a subscriber joined past the only object
                # carrying the Video Config.
                if self._from_keyframe:
                    self._unconfigured()
                return held_back
            payload = annex_b
        try:
            pictures = self._decoder._decode(payload, location)
        except FFmpegError as error:
            self._refused_in_a_row += 1
            self._refusal = error
            if self._from_keyframe and self._refused_in_a_row >= MAX_REFUSED_OBJECTS:
                raise
            return held_back
        self._refused_in_a_row = 0
        self._pictured = self._pictured or bool(pictures)
        return held_back + pictures

    def _annex_b(self, payload: bytes) -> bytes | None:
        """`payload`, of a group without a Video Config, in Annex B, or None when it cannot be read
        without one. A payload starting with a start code whose 4-byte length prefixes also cover
        it exactly, as a 3-byte one followed by the first bytes of a length of 256 to 511 bytes
        does, is read as the group's earlier objects were (draft-ietf-moq-loc-04 §2.1.3–2.1.4); in
        the group's first object, a 4-byte start code is Annex B and a 3-byte one a length."""
        units = _length_prefixed_units(payload)
        annex_b = payload.startswith((SHORT_START_CODE, START_CODE))
        if annex_b and units is not None:
            framed = self._framed_annex_b
            annex_b = payload.startswith(START_CODE) if framed is None else framed
        if annex_b or units is not None:
            self._framed_annex_b = annex_b
        if annex_b:
            return payload
        if units is None:
            return None
        self._in_band_parameter_sets = self._in_band_parameter_sets or any(map(self._is_sps, units))
        return b"".join(START_CODE + unit for unit in units) if self._in_band_parameter_sets else None

    def _is_sps(self, unit: bytes) -> bool:
        if self._hevc:
            return unit[0] >> 1 & 0x3F == HEVC_SPS
        return unit[0] & 0x1F == H264_SPS

    def flush(self) -> list[tuple[av.VideoFrame, Location | None]]:
        pictures = self._decoder.flush()
        if self._from_keyframe and not (self._pictured or pictures) and self._refusal is not None:
            raise self._refusal
        return pictures


@dataclass(frozen=True)
class _PassedConfig:
    """The Video Config a group passes on to the group after it, and the group whose own one it is:
    the group itself for one it carried, -1 for none or the initial one `decode_pictures` was
    given."""

    config: bytes | None
    supplier: int


@dataclass(frozen=True)
class DecodedPicture:
    picture: av.VideoFrame
    location: Location
    """The Location of the object the picture was decoded from."""
    decoded_at: float
    """`time.monotonic()` when it was decoded."""
    largest: Location | None = None
    """For a picture `follow_pictures` decoded, the Largest Location the SUBSCRIBE_OK of its
    subscription reported: the pictures before it were replayed from what the relay held, older
    than the subscription. None from `decode_pictures`, and for a track that had no object when it
    was subscribed to."""


def decode_pictures(
    objects: AsyncIterable[Object], codec: str, *, interval: float = 0.0, config: bytes | None = None
) -> Following[DecodedPicture]:
    """Decodes the objects of one subscription to a LOC video track, such as one that
    `RemoteBroadcast.subscriptions(name)` yields, in a thread and yields every picture, in
    presentation order within its group, or at most one every `interval` seconds. A track followed
    across its subscriptions is decoded with `follow_pictures`, which gives each subscription a
    `decode_pictures` of its own: the groups of the next subscription, as of a publisher that took
    over, may start at 0 again, which one `decode_pictures` takes as groups it already decoded. Each
    group gets a `VideoDecoder` of its own, since it starts at a keyframe and the objects of groups
    in flight may interleave; a group joined midway decodes to nothing. A group's decoder is flushed
    at its End of Group status object, when `MAX_OPEN_GROUPS` newer groups opened since (the objects
    of that group arriving later are skipped), and at the end of `objects`.

    A group's decoder takes the LOC Video Config its first object carries; a later object of the
    group carrying another one, a keyframe whose parameters changed, starts a new decoder with it. A
    group without a Video Config of its own takes the one the group before it decoded with last:
    until that group arrives, the group holds its objects. Taken while that group is open, the Video
    Config is provisional until the group decoded `MAX_HELD_OBJECTS` objects or `MAX_HELD_BYTES`
    bytes: a later keyframe of that group changing it, arriving late as the groups' streams race,
    decodes the group's objects again with the new one, and their pictures are given again; past
    that, the group keeps decoding with the old one, but the groups after it that inherit its Video
    Config take the new one. So do they when the group before it, arriving after it started, brings
    a Video Config other than the one it took, and so do the groups after a group that has not
    arrived, as a lost one, which took the Video Config of the newest earlier group that had one: a
    group between the two arriving late passes its own on to them, until the next group with a Video
    Config of its own, even one equal to the one that group inherited. A held object carrying a
    Video Config of its own starts the group at it, the objects held before it dropped. A group
    stops holding once the group before it arrives or closes, and, the group before it taken as late
    or lost, once it holds `MAX_HELD_OBJECTS` objects or `MAX_HELD_BYTES` bytes, a later group
    opens, it ends or `objects` end; it then decodes with the Video Config of the newest earlier
    group that had one, or else `config`. `config` is the track's Video Config as the reader knows
    it beforehand, such as from the catalog (`moq_wasm.media.Catalog.init_data`): the groups without
    one of their own and without an earlier group that had one decode with it. A group decodes
    without holding while no object of the track carried a Video Config, as one with its parameter
    sets in band, and when no earlier group arrived, as the first group of a subscription.

    Raises ValueError at once when no decoder takes `codec`; its reads raise the decoder's
    `av.error.FFmpegError` when a group joined at its keyframe does not decode: the decoder refused
    `MAX_REFUSED_OBJECTS` of its objects in a row, or the group ended without a picture after it
    refused one. An H.264 or H.265 object of a group without a Video Config is Annex B when it
    starts with a start code, 4-byte or 3-byte, and else length-prefixed; one that reads either way,
    its start code followed by bytes that 4-byte lengths would cover exactly, is read as the group's
    earlier objects were. A length-prefixed object decodes once the group carried its parameter sets
    (SPS and PPS) in band, as an `avc3` or `hev1` track may. A length-prefixed object before them,
    as of a track whose only Video Config came before the subscription joined, is skipped without
    being decoded, with one warning logged when its group was joined at its keyframe, until a group
    carrying a Video Config arrives; give the track's Video Config as `config` to decode it.
    `follow_pictures` joins at the start of the current group, so it gets that group's Video Config
    when the group carries one.

    `objects` stays the caller's: each read advances it in the reader's own task and context, and
    neither closing nor dropping the decoding cancels or closes it. A read cancelled, as by
    `asyncio.wait_for` timing out, does not end the decoding: the object being decoded in a thread
    goes on for the next read, as a `Following`'s state does; but a cancellation that reaches a
    read of `objects` reaches `objects` itself, which an async generator does not survive. Read
    `RemoteBroadcast.subscriptions()`'s objects, a `moq_wasm.Subscription` or `follow_pictures`
    to read with a timeout. `aclose()` stops the decoding at once: a read waiting on `objects`
    raises StopAsyncIteration once `objects` gives it its object, which is dropped, and no read
    reads `objects` after it. Closed, the decoding keeps no decoder or picture, once a decode in
    flight in a worker finished, also while a read still waits on `objects`."""
    return _DecodedPictures(objects, codec, interval, config)


class _DecodedPictures(Following[DecodedPicture]):
    """What `decode_pictures` returns: the pictures of each object, decoded whole in a thread, the
    object in flight kept across a cancelled read."""

    # Its reads await `objects`, which are the caller's.
    _interrupts = False

    def __init__(self, objects: AsyncIterable[Object], codec: str, interval: float, config: bytes | None):
        super().__init__("the pictures")
        # None once closed: the decoders and the pictures they hold are freed, once a decoding in a
        # worker finished.
        self._decoder: _PictureDecoder | None = _PictureDecoder(codec, interval, config)
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

    def __init__(self, codec: str, interval: float, initial_config: bytes | None):
        # Raises ValueError for an unknown codec.
        VideoDecoder(codec)
        self._codec = codec
        self._interval = interval
        self._initial_config = initial_config
        self._next_picture_at = 0.0
        # In the order the groups opened: each group's decoding, or the objects it holds while the
        # group before it has not arrived.
        self._groups: dict[int, _Group | list[Object]] = {}
        self._finished: deque[int] = deque(maxlen=FINISHED_GROUPS_KEPT)
        # The Video Config each of the latest groups passes on to the group after it, None for one
        # without: the newest it carried, or else the one it inherits, even once the group can no
        # longer decode its objects again with that one; beside it, the group whose own one it is,
        # -1 for none or the initial one.
        # An inherited one changes with its supplier's, and becomes the one of a group between the
        # two that arrives late, as the groups after a lost group do.
        self._configs: dict[int, _PassedConfig] = {}
        # The groups that took the Video Config of the group before them while that one was open,
        # which may change it yet: the objects each decoded since, to decode them again with a
        # changed one.
        self._provisional: dict[int, list[Object]] = {}
        self._carries_configs = False
        self._warned_unconfigured = False

    def take(self, video_object: Object) -> list[DecodedPicture]:
        """The pictures `video_object` completes."""
        groups = self._groups
        group_id = video_object.group_id
        if video_object.status is not None:
            return self._finish(group_id) if video_object.status == END_OF_GROUP_STATUS else []
        if group_id in self._finished:
            return []
        config = _video_config(video_object.extension_headers)
        self._carries_configs = self._carries_configs or config is not None
        group = groups.get(group_id)
        if isinstance(group, _Group):
            return self._decode(group, video_object)
        pictures = []
        if group is None:
            # The groups holding their objects before this one take the groups before them as late.
            holding = sorted(held_id for held_id, held in groups.items() if isinstance(held, list) and held_id < group_id)
            for held_id in holding:
                pictures.extend(self._release(held_id))
            if len(groups) >= MAX_OPEN_GROUPS:
                pictures.extend(self._finish(next(iter(groups))))
            first = all(seen > group_id for seen in (*self._configs, *self._finished))
            if config is None and self._carries_configs and not first and group_id - 1 not in (*self._configs, *self._finished):
                group = groups[group_id] = []
            else:
                pictures.extend(self._start(group_id, [video_object]))
        if isinstance(group, list):
            if config is not None:
                pictures.extend(self._start(group_id, [video_object]))
            else:
                group.append(video_object)
                if len(group) >= MAX_HELD_OBJECTS or _payload_bytes(group) >= MAX_HELD_BYTES:
                    pictures.extend(self._release(group_id))
        return pictures

    def finish(self) -> list[DecodedPicture]:
        """The pictures the groups still open hold back, at the end of the objects."""
        pictures = []
        while self._groups:
            pictures.extend(self._finish(min(self._groups)))
        return pictures

    def _unconfigured(self) -> None:
        """Warns, once, that length-prefixed objects are skipped for want of their parameter
        sets."""
        if not self._warned_unconfigured:
            self._warned_unconfigured = True
            logger.warning(
                "%s objects skipped until a group carries its Video Config: they are length-prefixed "
                "without their parameter sets in band, and the subscription joined past the object "
                "carrying it with none given beforehand",
                self._codec,
            )

    def _sampled(self, pictures: list[tuple[av.VideoFrame, Location | None]]) -> list[DecodedPicture]:
        decoded_at = time.monotonic()
        passed = []
        for picture, location in pictures:
            if location is None or decoded_at < self._next_picture_at:
                continue
            self._next_picture_at = decoded_at + self._interval
            passed.append(DecodedPicture(picture, location, decoded_at))
        return passed

    def _remember(self, group_id: int, passed: _PassedConfig) -> None:
        self._configs[group_id] = passed
        if len(self._configs) > FINISHED_GROUPS_KEPT + MAX_OPEN_GROUPS:
            del self._configs[min(self._configs)]

    def _supplier(self, group_id: int) -> int:
        """The group whose own Video Config `group_id` passes on; itself for one it does not
        remember."""
        passed = self._configs.get(group_id)
        return group_id if passed is None else passed.supplier

    def _inherited(self, group_id: int) -> _PassedConfig:
        """The Video Config of the newest group before `group_id` that had one, with the group whose
        own one it is, or else the initial one, of no group."""
        configs = self._configs
        earlier = [earlier_id for earlier_id, passed in configs.items() if earlier_id < group_id and passed.config is not None]
        return configs[max(earlier)] if earlier else _PassedConfig(self._initial_config, -1)

    def _decode(self, group: _Group, video_object: Object) -> list[DecodedPicture]:
        group_id = video_object.group_id
        location = Location(group_id, video_object.object_id)
        config = _video_config(video_object.extension_headers)
        previous = group.config
        # A group carrying its first Video Config of its own supplies the groups after it from now
        # on, even with the one it inherited: a late group before it no longer reaches them.
        supplied = config is not None and self._supplier(group_id) != group_id
        pictures = group.decode(video_object.payload, location, config)
        if config is not None or self._supplier(group_id) == group_id:
            self._remember(group_id, _PassedConfig(group.config, group_id))
        kept = self._provisional.get(group_id)
        if kept is not None:
            kept.append(video_object)
            # A Video Config of its own, or more decoded than a group holds while it waits, makes
            # the group's final.
            if config is not None or len(kept) > MAX_HELD_OBJECTS or _payload_bytes(kept) > MAX_HELD_BYTES:
                del self._provisional[group_id]
        decoded = self._sampled(pictures)
        if group.config != previous or supplied:
            decoded.extend(self._reconfigure(group_id, group.config))
        return decoded

    def _reconfigure(self, group_id: int, config: bytes | None) -> list[DecodedPicture]:
        """Passes `config`, the new Video Config `group_id` passes on, or the one it now supplies
        itself, to the groups after it that inherit theirs from no later group than it does, across
        the groups that have not arrived: those are the groups until the next one with a Video
        Config of its own. A group whose Video Config is provisional decodes what it decoded with
        the old one again, with `config`, and gives its pictures again; one that decoded too much to
        do so keeps decoding with the old one, but passes `config` on, so the groups after it decode
        with it."""
        passing = self._supplier(group_id)
        pictures = []
        for later_id, passed in sorted(self._configs.items()):
            if later_id <= group_id or passed.supplier == later_id or passed.supplier > passing:
                continue
            self._remember(later_id, _PassedConfig(config, passing))
            if passed.config == config:
                continue
            kept = self._provisional.get(later_id)
            if not kept or not isinstance(self._groups.get(later_id), _Group):
                continue
            group = self._groups[later_id] = _Group(self._codec, config, kept[0].object_id == 0, self._unconfigured)
            for video_object in kept:
                location = Location(later_id, video_object.object_id)
                pictures.extend(group.decode(video_object.payload, location, None))
        return self._sampled(pictures)

    def _start(self, group_id: int, held: list[Object]) -> list[DecodedPicture]:
        """Decodes the group, from its first held object, with the Video Config that object
        carries, or else the one it inherits, then passes the Video Config it passes on to the
        groups after it, which may have started before this one arrived, or starts the group after
        it if that one held its objects."""
        passed = _PassedConfig(_video_config(held[0].extension_headers), group_id)
        own = passed.config is not None
        if not own:
            passed = self._inherited(group_id)
        group = self._groups[group_id] = _Group(self._codec, passed.config, held[0].object_id == 0 or own, self._unconfigured)
        self._remember(group_id, passed)
        if not own and isinstance(self._groups.get(group_id - 1), _Group):
            self._provisional[group_id] = []
        decoded = []
        for video_object in held:
            decoded.extend(self._decode(group, video_object))
        passing = self._configs[group_id].config if group_id in self._configs else group.config
        decoded.extend(self._reconfigure(group_id, passing))
        return decoded + self._release(group_id + 1)

    def _release(self, group_id: int) -> list[DecodedPicture]:
        """Starts the group if it holds its objects, with the Video Config it inherits."""
        held = self._groups.get(group_id)
        return self._start(group_id, held) if isinstance(held, list) else []

    def _finish(self, group_id: int) -> list[DecodedPicture]:
        """Flushes the group, then starts the group after it if that one held its objects."""
        decoded = self._release(group_id)
        self._finished.append(group_id)
        self._provisional.pop(group_id, None)
        self._provisional.pop(group_id + 1, None)
        group = self._groups.pop(group_id, None)
        if group is not None:
            decoded.extend(self._sampled(cast(_Group, group).flush()))
        return decoded + self._release(group_id + 1)


def follow_pictures(
    remote: RemoteBroadcast,
    name: str,
    codec: str,
    *,
    namespace: list[str] | None = None,
    interval: float = 0.0,
    on_subscribed: Callable[[], Awaitable[None]] | None = None,
    config: bytes | None = None,
) -> "FollowedPictures":
    """The pictures of the LOC video track `name` of `remote`, followed as `RemoteBroadcast.follow`
    follows it, with `namespace` and `on_subscribed` as there, each subscription starting at the
    current group's first object (`RemoteBroadcast.subscriptions` with `from_group_start`), so its
    keyframe and the Video Config it may alone carry are decoded: each subscription is decoded by a
    `decode_pictures` of its own, at most one picture every `interval` seconds, so the pictures its
    decoders hold back come as it ends, and the next one carries no state of it but the track's
    Video Config: each `decode_pictures` starts with the one the newest object of the subscriptions
    before it carried, as one the publisher writes only when it changes, or else `config`, as
    `decode_pictures` takes it. Raises ValueError at once when no decoder takes `codec`; its reads
    raise what ended the session, as `follow()` does, and the decoder's error as `decode_pictures`
    does. A read cancelled, as by a timeout, does not end it; read it in `contextlib.aclosing`, as
    `follow()`; closed, it keeps no decoder or picture, only the Video Config it learned. A reader
    that follows the track again, as after a decoder error, passes the Video Config this one learned
    (`FollowedPictures.config`) on as `config`."""
    return FollowedPictures(remote, name, codec, namespace, interval, on_subscribed, config)


class FollowedPictures(Following[DecodedPicture]):
    """What `follow_pictures` returns: the pictures of each subscription, decoded by a
    `decode_pictures` of its own."""

    def __init__(
        self,
        remote: RemoteBroadcast,
        name: str,
        codec: str,
        namespace: list[str] | None,
        interval: float,
        on_subscribed: Callable[[], Awaitable[None]] | None,
        config: bytes | None,
    ):
        super().__init__(f"the pictures of {name}")
        # Raises ValueError for an unknown codec before subscribing.
        VideoDecoder(codec)
        self._codec = codec
        self._interval = interval
        self._config = config
        self._subscriptions = remote._subscriptions(
            name, namespace, on_subscribed, True, None, DEFAULT_DELIVERY_TIMEOUT_SECONDS
        )
        # The decoding of the current subscription, and the Largest Location its SUBSCRIBE_OK
        # reported.
        self._pictures: Following[DecodedPicture] | None = None
        self._largest: Location | None = None

    @property
    def config(self) -> bytes | None:
        """The Video Config the next subscription starts with: the one the newest object that
        carried one brought, or else the `config` given."""
        return self._config

    async def _next(self) -> DecodedPicture:
        while True:
            if self._pictures is None:
                objects = cast(_Objects, await anext(self._subscriptions))
                if self._closing.started:
                    # Closed by the `on_subscribed` callback.
                    raise StopAsyncIteration
                self._largest = objects._subscription.largest
                self._pictures = decode_pictures(
                    _NotedObjects(objects, self), self._codec, interval=self._interval, config=self._config
                )
            try:
                picture = await anext(self._pictures)
            except StopAsyncIteration:
                self._pictures = None
                continue
            return replace(picture, largest=self._largest)

    async def _release(self) -> None:
        try:
            await self._subscriptions.aclose()
        finally:
            # The decoders and the pictures they hold, freed once a decoding a cancelled read left
            # in a worker finished; the Video Config learned stays for `config`.
            self._pictures = None


class _NotedObjects:
    """The objects of one subscription, noting the Video Config of the newest one carrying one as
    the one the next subscription starts with. Its groups may start at 0 again, so its locations
    are not compared with an earlier one's."""

    def __init__(self, objects: Following[Object], pictures: FollowedPictures):
        self._objects = objects
        self._pictures = pictures
        self._newest: tuple[int, int] | None = None

    def __aiter__(self) -> "_NotedObjects":
        return self

    async def __anext__(self) -> Object:
        video_object = await anext(self._objects)
        config = _video_config(video_object.extension_headers)
        location = (video_object.group_id, video_object.object_id)
        if config is not None and (self._newest is None or location >= self._newest):
            self._pictures._config, self._newest = config, location
        return video_object


def _length_prefixed_units(payload: bytes) -> list[bytes] | None:
    """The NAL units of `payload`, each after a 4-byte length, or None when such lengths do not
    cover it exactly."""
    units = []
    offset = 0
    while offset < len(payload):
        length = int.from_bytes(payload[offset : offset + 4], "big")
        offset += 4
        if length == 0 or offset + length > len(payload):
            return None
        units.append(payload[offset : offset + length])
        offset += length
    return units or None


def _payload_bytes(video_objects: list[Object]) -> int:
    return sum(len(video_object.payload) for video_object in video_objects)


def _video_config(extension_headers: list[tuple[int, int | bytes]]) -> bytes | None:
    for header_id, value in extension_headers:
        if header_id == VIDEO_CONFIG_ID and isinstance(value, bytes):
            return value
    return None


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
