import asyncio
import base64
import contextlib
import contextvars
import gc
import json
import os
import subprocess
import sys
import threading
import time
import uuid
import warnings
import weakref
from concurrent.futures import ThreadPoolExecutor
from fractions import Fraction
from pathlib import Path
from types import SimpleNamespace

import av
import numpy as np
from av.error import FFmpegError
import pytest

import moq_wasm
from moq_wasm import codecs
from moq_wasm import Location, Subscription
from moq_wasm.codecs import OpusDecoder, OpusEncoder, VideoDecoder, VideoEncoder, decode_pictures, follow_pictures
from moq_wasm.media import RemoteBroadcast, create_broadcast
from moq_wasm.pipecat import MoqWasmInputParams, MoqWasmParams, MoqWasmParticipantInput, MoqWasmTrack, MoqWasmTransport
from moq_wasm.pipecat import MoqWasmEventRecord, MoqWasmTrackMessage, MoqWasmTrackObject
from moq_wasm.pipecat import _participant, _room, _transport
from moq_wasm.pipecat._participant import ParticipantReceiver, ReceiveOptions
from moq_wasm.pipecat._transport import LIVE_VIDEO_DRAIN_TIMEOUT_SECONDS
from moq_wasm.pipecat._room import MAX_AUDIO_AHEAD_MICROS, OPUS_FRAME_MICROS, Room, RoomOptions, _now_micros
from pipecat.audio.filters.base_audio_filter import BaseAudioFilter
from pipecat.audio.mixers.base_audio_mixer import BaseAudioMixer
from pipecat.audio.mixers.soundfile_mixer import SoundfileMixer
from pipecat.clocks.system_clock import SystemClock
from pipecat.frames.frames import (
    AssistantImageRawFrame,
    CancelFrame,
    EndFrame,
    ErrorFrame,
    Frame,
    InterruptionFrame,
    LLMFullResponseEndFrame,
    LLMMessagesUpdateFrame,
    StartFrame,
    StopFrame,
    InputTransportMessageFrame,
    InputTransportStartAudioStreamingFrame,
    OutputAudioRawFrame,
    OutputImageRawFrame,
    OutputTransportMessageFrame,
    OutputTransportMessageUrgentFrame,
    PipelineFlushFrame,
    SpeechOutputAudioRawFrame,
    SpriteFrame,
    TTSAudioRawFrame,
    TTSStoppedFrame,
    UserAudioRawFrame,
    UserImageRawFrame,
    UserImageRequestFrame,
)
from pipecat.pipeline.pipeline import Pipeline
from pipecat.processors.aggregators.llm_context import LLMContext
from pipecat.processors.aggregators.llm_response_universal import LLMAssistantAggregator
from pipecat.pipeline.worker import PipelineParams, PipelineWorker, ProcessorUnusablePolicy
from pipecat.transports.base_transport import VideoInSourceParams
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor, FrameProcessorSetup
from pipecat.transports.base_input import BaseInputTransport
from pipecat.transports import base_output, video_in_sampler
from pipecat.transports.base_output import BaseOutputTransport
from pipecat.utils import deprecation
from pipecat.utils.asyncio.task_manager import TaskManager
from pipecat.workers.runner import WorkerRunner
from pipecat.tests.utils import SleepFrame, run_test

from tests.cancellation import cancel_as_it_resumes
from tests.fake_native import (
    STAY_OPEN,
    TIMEOUT_SECONDS,
    FakeMediaSource,
    FakeRelay,
    FakeSubscription,
    FakeWriter,
    open_session,
    until,
    within,
)
from tests.virtual_time import VirtualTimeLoop

pytestmark = pytest.mark.usefixtures("no_resubscribe_delay")

ROOM = ["anon", "room"]
SAMPLE_RATE = 16_000
SIZE = (64, 48)


def tone(seconds: float, sample_rate: int = SAMPLE_RATE) -> bytes:
    samples = np.arange(int(seconds * sample_rate))
    return (np.sin(2 * np.pi * 440 * samples / sample_rate) * 8000).astype(np.int16).tobytes()


def picture(value: int) -> bytes:
    return bytes([value]) * (SIZE[0] * SIZE[1] * 3)


# The Object Status of an object that ends its group (draft-ietf-moq-transport-14 §10.2.1.1).
END_OF_GROUP = 3


def video_object(group_id: int, object_id: int, payload: bytes, *, status: int | None = None, extension_headers=()):
    return SimpleNamespace(
        group_id=group_id,
        object_id=object_id,
        payload=payload,
        status=status,
        extension_headers=list(extension_headers),
    )


async def start_alone(processor: BaseInputTransport | BaseOutputTransport) -> None:
    """Sets `processor` up and starts it as a pipeline does, with no pipeline around it to stop
    it."""
    setup = FrameProcessorSetup(clock=SystemClock(), task_manager=TaskManager(), pipeline_worker=SimpleNamespace())
    await processor.setup(setup)
    await processor.start(StartFrame())


class FrameRecorder(FrameProcessor):
    """Records the frames a processor driven alone pushes to it."""

    def __init__(self):
        super().__init__()
        self.frames: list[Frame] = []

    async def queue_frame(self, frame: Frame, direction: FrameDirection = FrameDirection.DOWNSTREAM, callback=None):
        self.frames.append(frame)


def link_recorders(processor: FrameProcessor) -> tuple[FrameRecorder, FrameRecorder]:
    """Links recorders before and after `processor`, for the frames it pushes upstream and
    downstream."""
    upstream, downstream = FrameRecorder(), FrameRecorder()
    upstream.link(processor)
    processor.link(downstream)
    return upstream, downstream


def catalog_payload(*tracks: dict) -> bytes:
    return json.dumps({"version": "draft-02", "tracks": list(tracks)}).encode()


class RecordingSink:
    def __init__(self):
        self.audio: list[tuple[bytes, int, str]] = []
        self.pictures: list = []
        self.messages: list[MoqWasmTrackMessage] = []
        self.unanswered: list[tuple[UserImageRequestFrame, str]] = []
        self.events: list[tuple] = []
        self.received = asyncio.Event()

    def audio_in_sample_rate(self) -> int:
        return SAMPLE_RATE

    async def push_audio(self, pcm: bytes, sample_rate: int, participant_id: str) -> None:
        self.audio.append((pcm, sample_rate, participant_id))
        self.received.set()

    async def push_picture(self, picture) -> bool:
        self.pictures.append(picture)
        self.received.set()
        return True

    async def push_message(self, message: MoqWasmTrackMessage) -> None:
        self.messages.append(message)
        self.received.set()

    async def answer_image_requests(self, requests: list[UserImageRequestFrame], error: str) -> None:
        self.unanswered.extend((request, error) for request in requests)

    async def event(self, name: str, *args: object) -> None:
        self.events.append((name, *args))


AUDIO_CATALOG = catalog_payload({"name": "audio", "role": "audio", "packaging": "loc", "codec": "opus", "bitrate": 64000, "samplerate": 48000, "channelConfig": "mono", "isLive": True})


class KeptMedia:
    """Weak references to every Opus decoder a participant receiver made and every picture it
    decoded, to count those still kept."""

    def __init__(self):
        self.decoders: list[weakref.ref] = []
        self.pictures: list[weakref.ref] = []

    def alive(self) -> tuple[int, int]:
        """The Opus decoders and the decoded pictures still kept."""
        gc.collect()
        return (
            sum(decoder() is not None for decoder in self.decoders),
            sum(picture() is not None for picture in self.pictures),
        )


@pytest.fixture
def kept_media(monkeypatch) -> KeptMedia:
    kept = KeptMedia()

    class TrackedDecoder(OpusDecoder):
        def __init__(self, sample_rate: int, channels: int = 1):
            super().__init__(sample_rate, channels)
            kept.decoders.append(weakref.ref(self))

    class TrackedPicture(codecs.DecodedPicture):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            kept.pictures.append(weakref.ref(self))

    monkeypatch.setattr(_participant, "OpusDecoder", TrackedDecoder)
    monkeypatch.setattr(codecs, "DecodedPicture", TrackedPicture)
    return kept


def receive_options(audio_in: bool = False, camera: bool = False, **overrides) -> ReceiveOptions:
    sources = {"camera": 1000.0} if camera else {}
    options = dict(audio_in=audio_in, video_sources=sources, video_in_track=None, video_in=camera)
    return ReceiveOptions(**{**options, **overrides})


def room_options(audio_in: bool = False, video_in: bool = False, **overrides) -> RoomOptions:
    options = dict(
        room=ROOM,
        participant_id="bot",
        receive=receive_options(audio_in=audio_in, camera=video_in),
        audio_out=False,
        video_out=False,
        video_out_codec="H264",
        video_out_size=SIZE,
        video_out_framerate=30,
    )
    options = {**options, **overrides}
    options.setdefault("broadcast", [*options["room"], options["participant_id"]])
    return RoomOptions(**options)


def test_opus_encoded_speech_decodes_back_to_pcm():
    # Arrange
    encoder = OpusEncoder()

    # Act
    packets = encoder.encode(tone(0.1), SAMPLE_RATE)
    decoder = OpusDecoder(SAMPLE_RATE)
    pcm = b"".join(decoder.decode(packet) for packet in packets)

    # Assert: the resampler keeps a few samples of the last packet, and the decoder drops Opus's pre-skip
    assert len(packets) == 4
    assert 0 < len(pcm) <= len(packets) * SAMPLE_RATE // 50 * 2


@pytest.mark.parametrize("codec", ["H264", "VP8"])
def test_the_video_encoder_opens_with_a_keyframe_and_makes_one_on_request(codec):
    # Arrange
    encoder = VideoEncoder(codec, *SIZE, 30)
    first = encoder.encode(picture(0), SIZE, "RGB")
    second = encoder.encode(picture(10), SIZE, "RGB")

    # Act
    encoder.request_keyframe()
    requested = encoder.encode(picture(20), SIZE, "RGB")

    # Assert: an H.264 keyframe opens with its SPS; a VP8 keyframe has bit 0 of its tag clear
    def is_keyframe(packet: bytes) -> bool:
        return packet.startswith(b"\x00\x00\x00\x01\x67") if codec == "H264" else packet[0] & 1 == 0

    assert [is_keyframe(packet) for packet in (*first, *second, *requested)] == [True, False, True]


@pytest.mark.parametrize(
    ("color_format", "blue"),
    [("RGB", b"\x00\x00\xff"), ("RGBA", b"\x00\x00\xff\xff"), ("BGR", b"\xff\x00\x00"), ("BGRA", b"\xff\x00\x00\xff")],
)
def test_the_video_encoder_reads_a_picture_in_each_color_format_it_takes(color_format, blue):
    # Arrange
    encoder = VideoEncoder("H264", *SIZE, 30)

    # Act
    packets = encoder.encode(blue * (SIZE[0] * SIZE[1]), SIZE, color_format)

    # Assert: the picture decodes blue
    decoder = VideoDecoder("avc1.42e01f")
    decoded = [picture for packet in packets for picture, _ in decoder.decode(packet)] + [p for p, _ in decoder.flush()]
    red, green, blue_level = decoded[0].to_ndarray(format="rgb24").reshape(-1, 3).mean(axis=0)
    assert blue_level > 200 and red < 60 and green < 60


@pytest.mark.parametrize(
    ("image", "color_format"),
    [
        (picture(0), "YUV"),
        (picture(0)[:-1], "RGB"),
        (picture(0), "RGBA"),
        (picture(0) + b"\x00" * 3, "BGR"),
        (memoryview(picture(0) * 2)[::2], "RGB"),
        ("\x00" * len(picture(0)), "RGB"),
    ],
)
def test_the_video_encoder_refuses_a_color_format_it_does_not_take_or_a_picture_of_another_length(image, color_format):
    # Arrange
    encoder = VideoEncoder("H264", *SIZE, 30)

    # Act / Assert
    with pytest.raises(ValueError):
        encoder.encode(image, SIZE, color_format)


def test_the_video_encoder_refuses_a_size_that_is_no_integers():
    # Arrange
    encoder = VideoEncoder("H264", *SIZE, 30)

    # Act / Assert
    with pytest.raises(ValueError, match="width and a height"):
        encoder.encode(picture(0), (float(SIZE[0]), SIZE[1]), "RGB")


@pytest.mark.parametrize(
    ("width", "height", "framerate", "bitrate", "codec_string"),
    [
        (64, 48, 30, None, "avc1.42C00A"),
        (1280, 720, 30, None, "avc1.42C01F"),
        (1920, 1080, 30, None, "avc1.42C028"),
        (1920, 1080, 30, 30_000_000, "avc1.42C029"),
        (1920, 1080, 60, None, "avc1.42C02A"),
    ],
)
def test_the_h264_encoders_codec_string_names_the_profile_and_level_of_its_sps(width, height, framerate, bitrate, codec_string):
    # Arrange
    encoder = VideoEncoder("H264", width, height, framerate, bitrate)

    # Act
    keyframe = b"".join(encoder.encode(picture(0), SIZE, "RGB"))

    # Assert: profile_idc, the constraint flags and level_idc follow the SPS's NAL unit header
    sps = keyframe[keyframe.index(b"\x00\x00\x01\x67") + 4 :]
    assert encoder.codec_string == codec_string
    assert f"avc1.{sps[:3].hex().upper()}" == codec_string


async def test_decoded_pictures_carry_the_location_of_their_object_and_are_thinned_out_by_the_interval():
    # Arrange: two groups of three pictures; the second group is joined at its second object
    encoder = VideoEncoder("H264", *SIZE, 30)

    def group(values) -> list[bytes]:
        encoder.request_keyframe()
        return [b"".join(encoder.encode(picture(value), SIZE, "RGB")) for value in values]

    first, second = group((0, 10, 20)), group((30, 40, 50))

    async def objects():
        for object_id, payload in enumerate(first):
            yield video_object(0, object_id, payload)
        for object_id, payload in list(enumerate(second))[1:]:
            yield video_object(1, object_id, payload)

    # Act
    every = [decoded async for decoded in decode_pictures(objects(), "avc1.42e01f")]
    thinned = [decoded async for decoded in decode_pictures(objects(), "avc1.42e01f", interval=60)]

    # Assert: a group joined without its keyframe decodes to nothing
    assert [(d.location.group_id, d.location.object_id) for d in every] == [(0, 0), (0, 1), (0, 2)]
    assert [(d.location.group_id, d.location.object_id) for d in thinned] == [(0, 0)]
    assert every[0].picture.width == SIZE[0]


# The LOC Video Config property (draft-ietf-moq-loc-04 §2.3.2.1).
VIDEO_CONFIG_ID = 0x0D


def nal_units(annex_b: bytes) -> list[bytes]:
    """The NAL units of an H.264 access unit in Annex B."""
    return [unit.rstrip(b"\x00") for unit in annex_b.split(b"\x00\x00\x01") if unit.rstrip(b"\x00")]


def length_prefixed(annex_b: bytes) -> tuple[bytes, bytes, bytes]:
    """An H.264 access unit's SPS, PPS and its other NAL units, each after a 4-byte length."""
    units = nal_units(annex_b)
    sps = next((unit for unit in units if unit[0] & 0x1F == 7), b"")
    pps = next((unit for unit in units if unit[0] & 0x1F == 8), b"")
    rest = b"".join(len(unit).to_bytes(4, "big") + unit for unit in units if unit[0] & 0x1F not in (7, 8))
    return sps, pps, rest


def avc_decoder_configuration(sps: bytes, pps: bytes) -> bytes:
    """The avcC record of one SPS and one PPS, with 4-byte lengths (ISO/IEC 14496-15 §5.3.3.1)."""
    return bytes([1, *sps[1:4], 0xFF, 0xE1]) + len(sps).to_bytes(2, "big") + sps + b"\x01" + len(pps).to_bytes(2, "big") + pps


def b_frame_groups(groups: int, pictures_per_group: int) -> list[list[tuple[bytes, int]]]:
    """H.264 groups with B-frames, each object as (payload, the value of the picture it carries)."""
    encoder = av.CodecContext.create("libx264", "w")
    encoder.width, encoder.height, encoder.pix_fmt = *SIZE, "yuv420p"
    encoder.time_base = Fraction(1, 30)
    encoder.options = {
        "preset": "ultrafast",
        "x264-params": f"bframes=2:b-adapt=0:keyint={pictures_per_group}:min-keyint={pictures_per_group}:scenecut=0:repeat-headers=1",
    }
    encoder.open()
    packets = []
    for index in range(groups * pictures_per_group):
        frame = av.VideoFrame.from_ndarray(np.frombuffer(picture(index * 10 % 250), np.uint8).reshape(SIZE[1], SIZE[0], 3), format="rgb24")
        frame = frame.reformat(format="yuv420p")
        frame.pts = index
        packets.extend(encoder.encode(frame))
    packets.extend(encoder.encode(None))
    split: list[list[tuple[bytes, int]]] = []
    for packet in packets:
        if packet.is_keyframe:
            split.append([])
        split[-1].append((bytes(packet), packet.pts * 10 % 250))
    return split


async def test_every_picture_of_groups_with_b_frames_is_decoded_with_the_location_it_came_in():
    # Arrange: each group ends with its End of Group
    groups = b_frame_groups(2, 6)

    async def objects():
        for group_id, group in enumerate(groups):
            for object_id, (payload, _) in enumerate(group):
                yield video_object(group_id, object_id, payload)
            yield video_object(group_id, len(group), b"", status=END_OF_GROUP)

    # Act
    decoded = [d async for d in decode_pictures(objects(), "avc1.64001f")]

    # Assert: in presentation order, each picture at the object that carried it (lossy, to the 10)
    values = [round(d.picture.to_ndarray(format="rgb24").mean() / 10) * 10 for d in decoded]
    assert values == [index * 10 for index in range(12)]
    carried = {value: (group_id, object_id) for group_id, group in enumerate(groups) for object_id, (_, value) in enumerate(group)}
    assert [(d.location.group_id, d.location.object_id) for d in decoded] == [carried[value] for value in values]


def test_the_decoders_remember_no_more_than_one_entry_per_supported_codec_family():
    # Arrange: what the codec module remembered before
    cached = [value for value in vars(codecs).values() if hasattr(value, "cache_clear")]
    for function in cached:
        function.cache_clear()

    # Act: catalogs name many codec strings, supported or not
    for index in range(64):
        for codec in (f"avc1.64{index:04x}", f"vp09.00.{index}.08", f"unknown{index}.1"):
            with contextlib.suppress(ValueError):
                VideoDecoder(codec)

    # Assert
    assert sum(function.cache_info().currsize for function in cached) <= len(codecs.VIDEO_DECODERS)


def test_a_video_codec_other_than_h264_and_vp8_is_refused():
    # Act / Assert
    with pytest.raises(ValueError, match="H265"):
        VideoEncoder("H265", *SIZE, 30)


AV1_ENCODERS = [name for name in ("libaom-av1", "libsvtav1") if name in av.codecs_available]


@pytest.mark.skipif(not AV1_ENCODERS, reason="this PyAV build has no AV1 encoder to make the track with")
async def test_an_av1_track_decodes_in_software():
    # Arrange: three AV1 pictures in one group, as the browser's screenshare sends by default
    encoder = av.CodecContext.create(AV1_ENCODERS[0], "w")
    encoder.width, encoder.height, encoder.pix_fmt = *SIZE, "yuv420p"
    encoder.time_base = Fraction(1, 30)
    encoder.framerate = 30
    encoder.open()
    payloads = []
    for index in range(3):
        frame = av.VideoFrame.from_ndarray(np.frombuffer(picture(index * 40), np.uint8).reshape(SIZE[1], SIZE[0], 3), format="rgb24")
        frame = frame.reformat(format="yuv420p")
        frame.pts = index
        payloads.extend(bytes(packet) for packet in encoder.encode(frame))
    payloads.extend(bytes(packet) for packet in encoder.encode(None))

    async def objects():
        for object_id, payload in enumerate(payloads):
            yield video_object(0, object_id, payload)

    # Act
    decoded = [d async for d in decode_pictures(objects(), "av01.0.04M.08")]

    # Assert
    assert len(decoded) == 3
    assert {(d.picture.width, d.picture.height) for d in decoded} == {SIZE}


# Decodes the first objects of a low-delay 720p AV1 group, as a live track sends them, and drops the
# decoder, again and again, as `decode_pictures` closed mid-group does.
AV1_TEARDOWNS = """
import sys
from fractions import Fraction

import av
import numpy as np

from moq_wasm import Location
from moq_wasm.codecs import VideoDecoder

WIDTH, HEIGHT, PICTURES, TEARDOWNS = 1280, 720, 30, 150
name = sys.argv[1]
encoder = av.CodecContext.create(name, "w")
encoder.width, encoder.height, encoder.pix_fmt = WIDTH, HEIGHT, "yuv420p"
encoder.time_base = Fraction(1, 30)
encoder.framerate = 30
if name == "libsvtav1":
    encoder.options = {"preset": "12", "svtav1-params": "pred-struct=1"}
else:
    encoder.options = {"cpu-used": "8", "usage": "realtime"}
encoder.open()
noise = np.random.default_rng(0).integers(0, 255, (HEIGHT, WIDTH, 3), np.uint8)
payloads = []
for index in range(PICTURES):
    frame = av.VideoFrame.from_ndarray(np.roll(noise, index * 4, axis=1), format="rgb24").reformat(format="yuv420p")
    frame.pts = index
    payloads.extend(bytes(packet) for packet in encoder.encode(frame))
payloads.extend(bytes(packet) for packet in encoder.encode(None))
decoded = np.random.default_rng(1).integers(1, len(payloads) + 1, TEARDOWNS)
for count in decoded:
    decoder = VideoDecoder("av01.0.08M.08")
    for object_id, payload in enumerate(payloads[:count]):
        decoder.decode(payload, Location(0, object_id))
    del decoder
"""


@pytest.mark.skipif(not AV1_ENCODERS, reason="this PyAV build has no AV1 encoder to make the track with")
@pytest.mark.skipif(codecs._decoder_name("av01") != "libdav1d", reason="this PyAV build decodes AV1 without dav1d")
def test_dropping_an_av1_decoder_mid_group_does_not_deadlock():
    # Arrange: a process that may deadlock, so it runs apart from the tests
    command = [sys.executable, "-c", AV1_TEARDOWNS, "libsvtav1" if "libsvtav1" in AV1_ENCODERS else AV1_ENCODERS[0]]
    environment = {**os.environ, "PYTHONPATH": os.pathsep.join([str(Path(codecs.__file__).parents[1]), os.environ.get("PYTHONPATH", "")])}

    # Act
    try:
        finished = subprocess.run(command, env=environment, capture_output=True, text=True, timeout=60)
    except subprocess.TimeoutExpired:
        finished = None

    # Assert
    assert finished is not None, "the decoder's teardown deadlocked"
    assert finished.returncode == 0, finished.stderr


async def test_a_participants_audio_reaches_the_sink_with_its_participant_id():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN]]
    native.tracks["audio"] = [OpusEncoder().encode(tone(0.04), SAMPLE_RATE)]
    sink = RecordingSink()
    room = Room(session, room_options(audio_in=True), sink)
    await room.start_audio()
    await room.start("input")

    # Act
    native.publish([*ROOM, "alice"])
    await within(sink.received.wait())

    # Assert
    assert sink.audio[0][1:] == (SAMPLE_RATE, "alice")
    assert sink.events[:3] == [
        ("on_connected",),
        ("on_client_connected", "alice"),
        ("on_track_subscribed", MoqWasmTrack("alice", "audio", "audio")),
    ]
    await room.stop("input")
    await session.close()


@pytest.mark.parametrize(
    ("subscriptions", "decoded"),
    [
        ([[(1, 0, b"a"), (1, 1, b"b"), (0, 0, b"late"), (1, 1, b"again"), (1, 2, b"c")]], [[b"a", b"b", b"c"]]),
        ([[(0, 0, b"a"), (0, 1, b"b"), (1, 0, b"c")]], [[b"a", b"b", b"c"]]),
        ([[(0, 0, b"a"), (0, 1, b"b"), (2, 0, b"c")]], [[b"a", b"b"], [b"c"]]),
        ([[(0, 0, b"a"), (0, 2, b"b")]], [[b"a"], [b"b"]]),
        ([[(5, 0, b"a")], [(0, 0, b"b")]], [[b"a"], [b"b"]]),
    ],
    ids=["older objects dropped", "next group", "missing group", "missing object", "subscribed again"],
)
async def test_a_participants_audio_is_decoded_in_order_with_a_fresh_decoder_after_a_gap(monkeypatch, subscriptions, decoded):
    # Arrange: each decoder records the packets it decodes
    decoders: list[list[bytes]] = []

    class RecordingDecoder:
        def __init__(self, sample_rate: int, channels: int = 1):
            self.packets: list[bytes] = []
            decoders.append(self.packets)

        def decode(self, packet: bytes) -> bytes:
            self.packets.append(packet)
            return bytes(2)

    monkeypatch.setattr(_participant, "OpusDecoder", RecordingDecoder)
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN]]
    native.tracks["audio"] = [*subscriptions[:-1], [*subscriptions[-1], STAY_OPEN]]
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(audio_in=True), sink)

    # Act
    running = asyncio.create_task(receiver.run())
    await asyncio.sleep(0.1)

    # Assert
    assert [packets for packets in decoders if packets] == decoded
    running.cancel()
    await asyncio.gather(running, return_exceptions=True)
    await session.close()


async def test_a_track_the_catalog_puts_in_another_namespace_is_subscribed_there():
    # Arrange: alice's catalog names her audio in a namespace of its own
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    audio = {"name": "audio", "namespace": "anon/room/alice-mic", "role": "audio", "packaging": "loc", "codec": "opus", "bitrate": 64000, "samplerate": 48000, "channelConfig": "mono", "isLive": True}
    native.tracks["catalog"] = [[catalog_payload(audio), STAY_OPEN]]
    native.tracks["audio"] = [OpusEncoder().encode(tone(0.04), SAMPLE_RATE)]
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(audio_in=True), sink)

    # Act
    running = asyncio.create_task(receiver.run())
    await within(sink.received.wait())

    # Assert
    assert native.subscribed[0] == (["anon", "room", "alice-mic"], "audio")
    running.cancel()
    await asyncio.gather(running, return_exceptions=True)
    await session.close()


def renamed_audio_catalog(name: str) -> bytes:
    return catalog_payload({"name": name, "role": "audio", "packaging": "loc", "codec": "opus", "bitrate": 64000, "samplerate": 48000, "channelConfig": "mono", "isLive": True})


async def test_a_track_a_new_catalog_replaces_is_subscribed_to_once_the_old_one_is_unsubscribed():
    # Arrange: alice's second catalog renames her audio track; the relay holds the answer to its
    # UNSUBSCRIBE back
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    renaming = asyncio.Event()
    native.tracks["catalog"] = [[AUDIO_CATALOG, renaming, renamed_audio_catalog("mic"), STAY_OPEN]]
    native.tracks["audio"] = [[STAY_OPEN]]
    native.tracks["mic"] = [[STAY_OPEN]]
    unsubscribing = native.unsubscribe_gates["audio"] = asyncio.Event()
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(audio_in=True), sink)
    running = asyncio.create_task(receiver.run())
    while len(native.subscriptions) < 2:
        await asyncio.sleep(0.01)

    # Act
    renaming.set()
    await asyncio.sleep(0.05)
    subscribed_while_unsubscribing = [name for _, name in native.subscribed]
    unsubscribing.set()
    while len(native.subscribed) < 2:
        await within(asyncio.sleep(0.01))

    # Assert
    assert subscribed_while_unsubscribing == ["audio"]
    assert [name for _, name in native.subscribed] == ["audio", "mic"]
    running.cancel()
    await asyncio.gather(running, return_exceptions=True)
    await session.close()


@pytest.mark.parametrize("kind", ["audio", "video"])
async def test_a_track_replaced_while_its_media_is_being_pushed_is_unsubscribed_before_its_replacement(kind):
    # Arrange: the sink holds alice's first audio or picture back; her second catalog renames the
    # track; the relay holds the answer to its UNSUBSCRIBE back
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    renaming = asyncio.Event()
    if kind == "audio":
        catalogs = [AUDIO_CATALOG, renaming, renamed_audio_catalog("renamed")]
        native.tracks["audio"] = [[*OpusEncoder().encode(tone(0.1), SAMPLE_RATE), STAY_OPEN]]
        options = receive_options(audio_in=True)
    else:
        renamed = {"name": "renamed", "role": "video", "packaging": "loc", "codec": "avc1.42e01f", "bitrate": 1000000, "framerate": 30, "isLive": True}
        catalogs = [VIDEO_CATALOG, renaming, catalog_payload(renamed)]
        native.tracks["video"] = [[*video_frames(3), STAY_OPEN]]
        options = receive_options(camera=True)
    native.tracks["catalog"] = [[*catalogs, STAY_OPEN]]
    native.tracks["renamed"] = [[STAY_OPEN]]
    unsubscribing = native.unsubscribe_gates[kind] = asyncio.Event()
    sink = RecordingSink()
    pushing = asyncio.Event()

    async def hold(*args) -> bool:
        pushing.set()
        await asyncio.Event().wait()
        return True

    sink.push_audio = hold
    sink.push_picture = hold
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), options, sink)
    running = asyncio.create_task(receiver.run())
    await within(pushing.wait())

    # Act
    renaming.set()
    await asyncio.sleep(0.05)
    subscribed_while_unsubscribing = [name for _, name in native.subscribed]
    unsubscribing.set()
    while len(native.subscribed) < 2:
        await within(asyncio.sleep(0.01))

    # Assert
    assert subscribed_while_unsubscribing == [kind]
    assert [name for _, name in native.subscribed] == [kind, "renamed"]
    running.cancel()
    await asyncio.gather(running, return_exceptions=True)
    await session.close()


async def test_a_participant_ends_only_once_the_tracks_a_new_catalog_replaced_are_unsubscribed():
    # Arrange: alice's second catalog renames her audio track; the relay holds the answer to its
    # UNSUBSCRIBE back
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    renaming = asyncio.Event()
    native.tracks["catalog"] = [[AUDIO_CATALOG, renaming, renamed_audio_catalog("mic"), STAY_OPEN]]
    native.tracks["audio"] = [[STAY_OPEN]]
    unsubscribing = native.unsubscribe_gates["audio"] = asyncio.Event()
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(audio_in=True), sink)
    running = asyncio.create_task(receiver.run())
    while len(native.subscriptions) < 2:
        await asyncio.sleep(0.01)
    renaming.set()
    await asyncio.sleep(0.05)

    # Act: alice leaves
    running.cancel()
    await asyncio.sleep(0.05)
    ended_while_unsubscribing = running.done()
    unsubscribing.set()
    await within(asyncio.gather(running, return_exceptions=True))

    # Assert
    assert not ended_while_unsubscribing
    assert all(subscription.closed for subscription in native.subscriptions)
    await session.close()


async def test_a_participant_ends_only_once_a_released_track_is_unsubscribed():
    # Arrange: alice's camera is received; the relay holds the answer to its UNSUBSCRIBE back
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [[STAY_OPEN]]
    unsubscribing = native.unsubscribe_gates["video"] = asyncio.Event()
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(camera=True), sink)
    running = asyncio.create_task(receiver.run())
    await until(lambda: len(native.subscriptions) == 2)
    releasing = asyncio.create_task(receiver.release("camera"))
    await asyncio.sleep(0.05)

    # Act: alice leaves while her camera is being unsubscribed
    running.cancel()
    await asyncio.sleep(0.05)
    ended_while_unsubscribing = running.done()
    unsubscribing.set()
    await within(asyncio.gather(running, releasing, return_exceptions=True))

    # Assert
    assert not ended_while_unsubscribing
    assert all(subscription.closed for subscription in native.subscriptions)
    await session.close()


async def test_a_release_retried_after_a_cancelled_one_returns_once_the_track_is_unsubscribed():
    # Arrange: alice's camera is received; the relay holds the answer to its UNSUBSCRIBE back
    # while a release of it is cancelled
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [[STAY_OPEN]]
    unsubscribing = native.unsubscribe_gates["video"] = asyncio.Event()
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(camera=True), sink)
    running = asyncio.create_task(receiver.run())
    await until(lambda: len(native.subscriptions) == 2)
    cancelled = asyncio.create_task(receiver.release("camera"))
    await until(lambda: native.subscriptions[1].unsubscribing)
    cancelled.cancel()
    await asyncio.gather(cancelled, return_exceptions=True)

    # Act
    retried = asyncio.create_task(receiver.release("camera"))
    await asyncio.sleep(0.05)
    released_while_unsubscribing = retried.done()
    unsubscribing.set()
    await within(retried)

    # Assert
    assert not released_while_unsubscribing
    assert native.subscriptions[1].closed
    running.cancel()
    await within(asyncio.gather(running, return_exceptions=True))
    await session.close()


class HeldAnswerSink(RecordingSink):
    """A sink whose answers to image requests wait until `finish` is set."""

    def __init__(self):
        super().__init__()
        self.answering = asyncio.Event()
        self.finish = asyncio.Event()

    async def answer_image_requests(self, requests: list[UserImageRequestFrame], error: str) -> None:
        self.answering.set()
        await self.finish.wait()
        await super().answer_image_requests(requests, error)


async def test_a_release_whose_answer_is_being_delivered_keeps_no_picture_of_the_released_track(kept_media):
    # Arrange: alice's camera is passing a picture on, which the sink holds, and a request waits
    # for the next one
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [[*video_frames(2), STAY_OPEN]]
    sink = HeldAnswerSink()
    pushing = asyncio.Event()

    async def hold_picture(picture) -> bool:
        sink.pictures.append(picture)
        pushing.set()
        await asyncio.Event().wait()
        return True

    sink.push_picture = hold_picture
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(camera=True), sink)
    running = asyncio.create_task(receiver.run())
    await within(pushing.wait())
    await receiver.request_image(UserImageRequestFrame(user_id="alice"))

    # Act: the camera is released, and the error answering the request is being delivered
    releasing = asyncio.create_task(receiver.release("camera"))
    await within(sink.answering.wait())

    # Assert
    assert kept_media.alive() == (0, 0)
    sink.finish.set()
    await within(releasing)
    running.cancel()
    await within(asyncio.gather(running, return_exceptions=True))
    await session.close()


async def test_released_video_sources_leave_no_sampler_behind_once_their_requests_expired(monkeypatch):
    # Arrange: each of alice's ONVIF profiles is captured, asked for a picture and released
    monkeypatch.setattr(_participant, "IMAGE_REQUEST_TIMEOUT_SECONDS", 0.1)
    relay = FakeRelay()
    session = await open_session(relay)
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(), sink)
    running = asyncio.create_task(receiver.run())
    deadlines = []
    for index in range(3):
        await within(receiver.capture(f"profile{index}", 1000.0))
        await within(receiver.request_image(UserImageRequestFrame(user_id="alice", video_source=f"profile{index}")))
        deadlines.extend(receiver._deadlines)
        await within(receiver.release(f"profile{index}"))

    # Act: the deadlines of the requests the releases answered expire
    await within(asyncio.gather(*deadlines))

    # Assert
    assert len(sink.unanswered) == 3
    assert receiver._samplers == {}
    running.cancel()
    await within(asyncio.gather(running, return_exceptions=True))
    await session.close()


async def test_an_image_request_made_while_another_source_is_released_returns_at_once_and_expires_on_time(monkeypatch):
    # Arrange: alice's camera is received; the relay holds the answer to its UNSUBSCRIBE back as the
    # camera is released
    monkeypatch.setattr(_participant, "IMAGE_REQUEST_TIMEOUT_SECONDS", 0.1)
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [[STAY_OPEN]]
    unsubscribing = native.unsubscribe_gates["video"] = asyncio.Event()
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(camera=True), sink)
    running = asyncio.create_task(receiver.run())
    await until(lambda: len(native.subscriptions) == 2)
    releasing = asyncio.create_task(receiver.release("camera"))
    await until(lambda: native.subscriptions[1].unsubscribing)
    request = UserImageRequestFrame(user_id="alice", video_source="screen")

    # Act: the input's frame processing awaits the request
    await asyncio.wait_for(receiver.request_image(request), 1)
    await until(lambda: sink.unanswered)

    # Assert: the deadline answered it while the camera's release still waits
    assert [answered for answered, _ in sink.unanswered] == [request]
    assert not releasing.done()
    unsubscribing.set()
    await within(releasing)
    running.cancel()
    await within(asyncio.gather(running, return_exceptions=True))
    await session.close()


async def test_an_image_request_made_while_its_source_is_released_is_answered_by_the_subscription_it_starts():
    # Arrange: alice's camera is received, and its next subscription sends pictures; the relay
    # holds the answer to the first one's UNSUBSCRIBE back as the camera is released
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [[STAY_OPEN], [*video_frames(3), STAY_OPEN]]
    unsubscribing = native.unsubscribe_gates["video"] = asyncio.Event()
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(camera=True), sink)
    running = asyncio.create_task(receiver.run())
    await until(lambda: len(native.subscriptions) == 2)
    releasing = asyncio.create_task(receiver.release("camera"))
    await until(lambda: native.subscriptions[1].unsubscribing)
    request = UserImageRequestFrame(user_id="alice")

    # Act
    requesting = asyncio.create_task(receiver.request_image(request))
    await asyncio.sleep(0.05)
    unsubscribing.set()
    await within(asyncio.gather(releasing, requesting))
    await until(lambda: sink.pictures or sink.unanswered)

    # Assert
    assert sink.unanswered == []
    assert [picture.request for picture in sink.pictures] == [request]
    running.cancel()
    await within(asyncio.gather(running, return_exceptions=True))
    await session.close()


async def test_a_request_a_cancelled_release_answers_is_not_answered_by_the_subscription_a_later_request_starts():
    # Arrange: alice's camera is received, and its next subscription sends pictures; a request
    # waits as the camera is released, the relay holds the answer to its UNSUBSCRIBE back, and a
    # request made meanwhile starts the camera again
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [[STAY_OPEN], [*video_frames(3), STAY_OPEN]]
    unsubscribing = native.unsubscribe_gates["video"] = asyncio.Event()
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(camera=True), sink)
    running = asyncio.create_task(receiver.run())
    await until(lambda: len(native.subscriptions) == 2)
    released = UserImageRequestFrame(user_id="alice")
    await within(receiver.request_image(released))
    releasing = asyncio.create_task(receiver.release("camera"))
    await until(lambda: native.subscriptions[1].unsubscribing)
    later = UserImageRequestFrame(user_id="alice")
    await within(receiver.request_image(later))

    # Act: a timeout cancels the release while the camera's track is unsubscribed
    releasing.cancel()
    await asyncio.gather(releasing, return_exceptions=True)
    unsubscribing.set()
    await until(lambda: sink.pictures and sink.unanswered)

    # Assert
    assert sink.unanswered == [(released, "camera of alice released")]
    assert [picture.request for picture in sink.pictures] == [later]
    running.cancel()
    await within(asyncio.gather(running, return_exceptions=True))
    await session.close()


async def test_an_image_request_made_after_a_queued_release_was_called_is_answered_by_the_subscription_it_starts():
    # Arrange: alice's camera is received, and its next subscriptions, which each release ends
    # and the request starts again, send pictures; the relay holds the answer to the first one's
    # UNSUBSCRIBE back as the camera is released, and a second release of it waits for the first
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [[STAY_OPEN], *[[*video_frames(3), STAY_OPEN]] * 2]
    unsubscribing = native.unsubscribe_gates["video"] = asyncio.Event()
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(camera=True), sink)
    running = asyncio.create_task(receiver.run())
    await until(lambda: len(native.subscriptions) == 2)
    first = asyncio.create_task(receiver.release("camera"))
    await until(lambda: native.subscriptions[1].unsubscribing)
    second = asyncio.create_task(receiver.release("camera"))
    await asyncio.sleep(0.05)
    request = UserImageRequestFrame(user_id="alice")

    # Act
    await within(receiver.request_image(request))
    unsubscribing.set()
    await within(asyncio.gather(first, second))
    await until(lambda: sink.pictures or sink.unanswered)

    # Assert
    assert sink.unanswered == []
    assert [picture.request for picture in sink.pictures] == [request]
    running.cancel()
    await within(asyncio.gather(running, return_exceptions=True))
    await session.close()


async def test_an_image_request_made_after_a_queued_release_was_called_is_answered_once_the_release_was_cancelled(
    monkeypatch,
):
    # Arrange: alice's camera and screen are received, and her camera's next subscription sends
    # pictures; a release of the screen, whose UNSUBSCRIBE the relay holds back, holds a release
    # of the camera queued as a request on the camera is made
    monkeypatch.setattr(_participant, "IMAGE_REQUEST_TIMEOUT_SECONDS", 1.0)
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    screen = {"name": "screen", "role": "video", "packaging": "loc", "codec": "avc1.42e01f", "bitrate": 2000000, "framerate": 30, "isLive": True}
    native.tracks["catalog"] = [[catalog_payload(json.loads(VIDEO_CATALOG)["tracks"][0], screen), STAY_OPEN]]
    native.tracks["video"] = [[STAY_OPEN], [*video_frames(3), STAY_OPEN]]
    native.tracks["screen"] = [[STAY_OPEN]]
    camera_unsubscribing = native.unsubscribe_gates["video"] = asyncio.Event()
    screen_unsubscribing = native.unsubscribe_gates["screen"] = asyncio.Event()
    sink = RecordingSink()
    options = receive_options(video_sources={"camera": 1000.0, "screen": 1000.0})
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), options, sink)
    running = asyncio.create_task(receiver.run())
    await until(lambda: len(native.subscriptions) == 3)
    camera_track, screen_track = native.subscriptions[1:3]
    releasing_screen = asyncio.create_task(receiver.release("screen"))
    await until(lambda: screen_track.unsubscribing)
    releasing_camera = asyncio.create_task(receiver.release("camera"))
    await asyncio.sleep(0.05)
    request = UserImageRequestFrame(user_id="alice")
    await within(receiver.request_image(request))
    screen_unsubscribing.set()
    await within(releasing_screen)
    await until(lambda: camera_track.unsubscribing)

    # Act: a timeout cancels the camera's release while its track is unsubscribed
    releasing_camera.cancel()
    await asyncio.gather(releasing_camera, return_exceptions=True)
    camera_unsubscribing.set()
    await until(lambda: sink.pictures or sink.unanswered)

    # Assert
    assert sink.unanswered == []
    assert [picture.request for picture in sink.pictures] == [request]
    running.cancel()
    await within(asyncio.gather(running, return_exceptions=True))
    await session.close()


async def test_an_image_request_made_after_an_earlier_release_dropped_its_sampler_is_answered_once_a_queued_release_ran(
    monkeypatch,
):
    # Arrange: alice's camera and screen are received, and her camera's next subscription sends
    # pictures; a release of the camera, then one of the screen, whose UNSUBSCRIBE the relay holds
    # back, and another one of the camera are queued; the first release dropped the camera's
    # sampler as a request on the camera is made
    monkeypatch.setattr(_participant, "IMAGE_REQUEST_TIMEOUT_SECONDS", 1.0)
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    screen = {"name": "screen", "role": "video", "packaging": "loc", "codec": "avc1.42e01f", "bitrate": 2000000, "framerate": 30, "isLive": True}
    native.tracks["catalog"] = [[catalog_payload(json.loads(VIDEO_CATALOG)["tracks"][0], screen), STAY_OPEN]]
    native.tracks["video"] = [[STAY_OPEN], [*video_frames(3), STAY_OPEN]]
    native.tracks["screen"] = [[STAY_OPEN]]
    screen_unsubscribing = native.unsubscribe_gates["screen"] = asyncio.Event()
    sink = RecordingSink()
    options = receive_options(video_sources={"camera": 1000.0, "screen": 1000.0})
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), options, sink)
    running = asyncio.create_task(receiver.run())
    await until(lambda: len(native.subscriptions) == 3)
    screen_track = native.subscriptions[2]
    first_camera_release = asyncio.create_task(receiver.release("camera"))
    releasing_screen = asyncio.create_task(receiver.release("screen"))
    second_camera_release = asyncio.create_task(receiver.release("camera"))
    await within(first_camera_release)
    await until(lambda: screen_track.unsubscribing)
    request = UserImageRequestFrame(user_id="alice")
    await within(receiver.request_image(request))

    # Act
    screen_unsubscribing.set()
    await within(asyncio.gather(releasing_screen, second_camera_release))
    await until(lambda: sink.pictures or sink.unanswered)

    # Assert
    assert sink.unanswered == []
    assert [picture.request for picture in sink.pictures] == [request]
    running.cancel()
    await within(asyncio.gather(running, return_exceptions=True))
    await session.close()


async def test_a_source_captured_while_the_participant_leaves_is_not_subscribed_to():
    # Arrange: alice's catalog has audio and a camera; the relay holds the answer to her audio's
    # UNSUBSCRIBE back
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    tracks = json.loads(AUDIO_CATALOG)["tracks"] + json.loads(VIDEO_CATALOG)["tracks"]
    native.tracks["catalog"] = [[catalog_payload(*tracks), STAY_OPEN]]
    native.tracks["audio"] = [[STAY_OPEN]]
    native.tracks["video"] = [[STAY_OPEN]]
    unsubscribing = native.unsubscribe_gates["audio"] = asyncio.Event()
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(audio_in=True), sink)
    running = asyncio.create_task(receiver.run())
    await until(lambda: len(native.subscriptions) == 2)
    running.cancel()
    await asyncio.sleep(0.05)

    # Act: her camera is captured while she leaves
    await within(receiver.capture("camera", None))
    await asyncio.sleep(0.05)
    unsubscribing.set()
    await within(asyncio.gather(running, return_exceptions=True))

    # Assert
    assert [name for _, name in native.subscribed] == ["audio"]
    assert all(subscription.closed for subscription in native.subscriptions)
    await session.close()


@pytest.mark.parametrize("failure", [KeyError("video"), RuntimeError("no camera")])
async def test_a_video_in_track_selector_that_fails_on_a_later_catalog_is_reported_and_keeps_the_tracks(failure):
    # Arrange: a participant's audio and camera, then a later catalog the selector fails on
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    tracks = json.loads(AUDIO_CATALOG)["tracks"] + json.loads(VIDEO_CATALOG)["tracks"]
    later = asyncio.Event()
    native.tracks["catalog"] = [[catalog_payload(*tracks), later, catalog_payload(*tracks), STAY_OPEN]]
    native.tracks["audio"] = [[STAY_OPEN]]
    native.tracks["video"] = [[STAY_OPEN]]
    selections = []

    def select(catalog):
        selections.append(catalog)
        if len(selections) > 1:
            raise failure
        return catalog.select(role="video")

    sink = RecordingSink()
    options = receive_options(audio_in=True, video_sources={"camera": None}, video_in_track=select)
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), options, sink)
    running = asyncio.create_task(receiver.run())
    await until(lambda: len(native.subscriptions) == 3)

    # Act
    later.set()
    await until(lambda: any(event[0] == "on_error" for event in sink.events))

    # Assert: the failure was reported, and neither track was unsubscribed
    assert [event[2] for event in sink.events if event[0] == "on_error"] == [failure]
    assert not running.done()
    assert not any(subscription.unsubscribing for subscription in native.subscriptions)
    running.cancel()
    await within(asyncio.gather(running, return_exceptions=True))
    await session.close()


async def test_an_on_error_handler_capturing_a_fallback_source_after_the_selector_raised_is_called_once():
    # Arrange: the selector of alice's camera raises, and the on_error handler captures another of
    # her sources on each error, at most three times
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]

    def select(catalog):
        raise ValueError("no camera for the bot")

    class CapturingSink(RecordingSink):
        def __init__(self):
            super().__init__()
            self.captures = 0
            self.captured = asyncio.Event()

        async def event(self, name: str, *args: object) -> None:
            await super().event(name, *args)
            if name == "on_error" and self.captures < 3:
                self.captures += 1
                await receiver.capture("screen", None)
                self.captured.set()

    sink = CapturingSink()
    options = receive_options(video_sources={"camera": None}, video_in_track=select)
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), options, sink)

    # Act
    running = asyncio.create_task(receiver.run())
    await within(sink.captured.wait())
    await asyncio.sleep(0.05)

    # Assert: the capture picked the screen alone and did not run the camera's selector again
    assert [event[0] for event in sink.events] == ["on_error"]
    running.cancel()
    await within(asyncio.gather(running, return_exceptions=True))
    await session.close()


async def test_a_refused_subscribe_reports_no_subscribed_track():
    # Arrange: alice's catalog names an audio track the relay does not have
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN]]
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(audio_in=True), sink)

    # Act
    running = asyncio.create_task(receiver.run())
    await asyncio.sleep(0.05)

    # Assert
    assert [event for event in sink.events if event[0] == "on_track_subscribed"] == []
    running.cancel()
    await asyncio.gather(running, return_exceptions=True)
    await session.close()


async def test_a_track_that_failed_to_decode_is_received_again_with_a_fresh_decoder(monkeypatch):
    # Arrange: alice's first audio object is no Opus packet, which leaves an Opus decoder unusable
    monkeypatch.setattr(_participant, "RETRY_DELAY_SECONDS", 0.01)
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN]]
    native.tracks["audio"] = [[b"\xff\xff\xff", STAY_OPEN], [*OpusEncoder().encode(tone(0.04), SAMPLE_RATE), STAY_OPEN]]
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(audio_in=True), sink)

    # Act
    running = asyncio.create_task(receiver.run())
    await within(sink.received.wait())

    # Assert: the catalog did not change, and the track came back after one failure
    assert [event[0] for event in sink.events].count("on_error") == 1
    assert sink.audio[0][2] == "alice"
    running.cancel()
    await asyncio.gather(running, return_exceptions=True)
    await session.close()


async def test_a_video_track_received_again_after_a_decoder_error_decodes_with_the_video_config_it_learned(monkeypatch):
    # Arrange: only the first object of alice's avcC H.264 track carries the Video Config, which her
    # catalog does not; the group after it does not decode, and the next subscription joins at a
    # group without one
    monkeypatch.setattr(_participant, "RETRY_DELAY_SECONDS", 0.01)
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    encoder = VideoEncoder("H264", *SIZE, 30)
    config, group_0 = length_prefixed_group(encoder, 1)
    _, group_2 = length_prefixed_group(encoder, 2)
    undecodable = [video_object(1, object_id, b"\xff" * 8) for object_id in range(codecs.MAX_REFUSED_OBJECTS)]
    first = [video_object(0, 0, group_0[0], extension_headers=[(VIDEO_CONFIG_ID, config)]), *undecodable, STAY_OPEN]
    second = [*(video_object(2, object_id, payload) for object_id, payload in enumerate(group_2)), STAY_OPEN]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [first, second]
    sink = RecordingSink()
    options = receive_options(video_sources={"camera": float("inf")})
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), options, sink)

    # Act
    running = asyncio.create_task(receiver.run())
    await until(lambda: any(picture.location.group_id == 2 for picture in sink.pictures))

    # Assert
    assert [event[0] for event in sink.events].count("on_error") == 1
    assert [(picture.location.group_id, picture.location.object_id) for picture in sink.pictures][:2] == [(0, 0), (2, 0)]
    running.cancel()
    await asyncio.gather(running, return_exceptions=True)
    await session.close()


async def test_a_track_that_keeps_failing_is_received_again_less_and_less_often(monkeypatch):
    # Arrange: a video track no decoder takes
    monkeypatch.setattr(_participant, "RETRY_DELAY_SECONDS", 0.05)
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    catalog = catalog_payload({"name": "video", "role": "video", "packaging": "loc", "codec": "xyz1", "bitrate": 1000000, "framerate": 30, "isLive": True})
    native.tracks["catalog"] = [[catalog, STAY_OPEN]]
    sink = RecordingSink()
    options = receive_options(video_sources={"camera": None}, video_in_track=lambda catalog: catalog.tracks[0])
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), options, sink)

    # Act
    running = asyncio.create_task(receiver.run())
    await asyncio.sleep(0.25)

    # Assert: attempts at 0, 0.05 and 0.15 s, and the next at 0.35 s; a fixed delay would make seven
    assert [event[0] for event in sink.events].count("on_error") == 3
    running.cancel()
    await asyncio.gather(running, return_exceptions=True)
    await session.close()


async def test_a_participants_video_reaches_the_sink_as_pictures():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    encoder = VideoEncoder("H264", *SIZE, 30)
    frames = [packet for value in (0, 10, 20) for packet in encoder.encode(picture(value), SIZE, "RGB")]
    native.tracks["catalog"] = [[catalog_payload({"name": "video", "role": "video", "packaging": "loc", "codec": "avc1.42e01f", "bitrate": 1000000, "framerate": 30, "isLive": True}), STAY_OPEN]]
    native.tracks["video"] = [frames]
    sink = RecordingSink()
    room = Room(session, room_options(video_in=True), sink)
    await room.start("input")

    # Act
    native.publish([*ROOM, "alice"])
    await within(sink.received.wait())

    # Assert
    received = sink.pictures[0]
    assert (received.size, received.participant_id, received.video_source) == (SIZE, "alice", "camera")
    assert (received.location.group_id, received.location.object_id) == (0, 0)
    await room.stop("input")
    await session.close()


async def test_a_participants_length_prefixed_video_decodes_with_the_video_config_of_its_catalog():
    # Arrange: no object of the avcC H.264 track carries the Video Config, which the catalog does
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    encoder = VideoEncoder("H264", *SIZE, 30)
    config, group = length_prefixed_group(encoder, 2)
    track = {"name": "video", "role": "video", "packaging": "loc", "codec": "avc1.42e01f", "bitrate": 1000000, "framerate": 30, "isLive": True, "initRef": "video"}
    catalog = {"version": "draft-02", "tracks": [track], "initDataList": [{"id": "video", "type": "inline", "data": base64.b64encode(config).decode()}]}
    native.tracks["catalog"] = [[json.dumps(catalog).encode(), STAY_OPEN]]
    native.tracks["video"] = [[*(video_object(3, object_id, payload) for object_id, payload in enumerate(group)), STAY_OPEN]]
    sink = RecordingSink()
    options = receive_options(video_sources={"camera": float("inf")})
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), options, sink)

    # Act
    running = asyncio.create_task(receiver.run())
    await until(lambda: len(sink.pictures) >= 2)

    # Assert
    assert [(picture.location.group_id, picture.location.object_id) for picture in sink.pictures[:2]] == [(3, 0), (3, 1)]
    running.cancel()
    await asyncio.gather(running, return_exceptions=True)
    await session.close()


async def test_a_catalog_changing_only_a_tracks_inline_video_config_receives_the_track_again_with_it():
    # Arrange: alice's length-prefixed track carries no Video Config of its own; her second catalog
    # keeps its name, codec and initRef but gives it the Video Config of 32x24 pictures instead of
    # 64x48, and the publisher's objects change with it
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    large_config, large = length_prefixed_group(VideoEncoder("H264", *SIZE, 30), 2)
    small_config, small = length_prefixed_group(VideoEncoder("H264", 32, 24, 30), 2)
    track = {"name": "video", "role": "video", "packaging": "loc", "codec": "avc1.42e01f", "bitrate": 1000000, "framerate": 30, "isLive": True, "initRef": "video"}

    def catalog(config: bytes) -> bytes:
        init = [{"id": "video", "type": "inline", "data": base64.b64encode(config).decode()}]
        return json.dumps({"version": "draft-02", "tracks": [track], "initDataList": init}).encode()

    changing = asyncio.Event()
    native.tracks["catalog"] = [[catalog(large_config), changing, catalog(small_config), STAY_OPEN]]
    native.tracks["video"] = [
        [*(video_object(0, index, payload) for index, payload in enumerate(large)), STAY_OPEN],
        [*(video_object(1, index, payload) for index, payload in enumerate(small)), STAY_OPEN],
    ]
    sink = RecordingSink()
    options = receive_options(video_sources={"camera": float("inf")})
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), options, sink)
    running = asyncio.create_task(receiver.run())
    await until(lambda: len(sink.pictures) >= 1)

    # Act
    changing.set()

    # Assert: the track was received again and decoded with the new Video Config
    await until(lambda: any(picture.size == (32, 24) for picture in sink.pictures))
    assert sink.pictures[0].size == SIZE
    running.cancel()
    await asyncio.gather(running, return_exceptions=True)
    await session.close()


async def test_each_subscription_to_a_video_track_is_decoded_afresh():
    # Arrange: the first subscription brings five groups with B-frames and ends, as when alice's
    # page leaves; the page taking over restarts its groups at 0
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    groups = b_frame_groups(5, 6)
    first = [(group_id, object_id, payload) for group_id, group in enumerate(groups) for object_id, (payload, _) in enumerate(group)]
    second = [(0, object_id, payload) for object_id, (payload, _) in enumerate(groups[0])]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [first, [*second, STAY_OPEN]]
    sink = RecordingSink()
    by_subscription: dict[int, list] = {}

    async def push_picture(picture) -> bool:
        subscriptions = sum(1 for _, name in native.subscribed if name == "video")
        by_subscription.setdefault(subscriptions, []).append(picture)
        return True

    sink.push_picture = push_picture
    options = receive_options(video_sources={"camera": float("inf")})
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), options, sink)

    # Act
    running = asyncio.create_task(receiver.run())
    await until(lambda: len(by_subscription.get(2, [])) >= 4)

    # Assert: the pictures the decoders held back came as the first subscription ended, and the
    # second's group 0 was not taken as the first's
    assert len(by_subscription[1]) == 30
    assert {picture.location.group_id for picture in by_subscription[2]} == {0}
    running.cancel()
    await asyncio.gather(running, return_exceptions=True)
    await session.close()


async def test_following_pictures_decodes_each_subscription_afresh():
    # Arrange: the first subscription brings five groups with B-frames and ends, as when alice's
    # page leaves; the page taking over restarts its groups at 0
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    groups = b_frame_groups(5, 6)
    first = [(group_id, object_id, payload) for group_id, group in enumerate(groups) for object_id, (payload, _) in enumerate(group)]
    second = [(0, object_id, payload) for object_id, (payload, _) in enumerate(groups[0])]
    native.tracks["camera"] = [first, [*second, STAY_OPEN]]
    remote = RemoteBroadcast(session, [*ROOM, "alice"])
    by_subscription: dict[int, list] = {}

    async def follow() -> None:
        async with contextlib.aclosing(follow_pictures(remote, "camera", "avc1.64001f")) as pictures:
            async for decoded in pictures:
                subscriptions = sum(1 for _, name in native.subscribed if name == "camera")
                by_subscription.setdefault(subscriptions, []).append(decoded)
                if len(by_subscription.get(2, [])) >= 4:
                    return

    # Act
    await within(follow())

    # Assert: the pictures the decoders held back came as the first subscription ended, and the
    # second's group 0 was not taken as the first's
    assert len(by_subscription[1]) == 30
    assert {decoded.location.group_id for decoded in by_subscription[2]} == {0}
    await session.close()


async def test_the_bots_audio_and_video_go_into_its_media_source_and_a_keyframe_request_is_answered():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    sink = RecordingSink()
    room = Room(session, room_options(audio_out=True, video_out=True), sink)
    await room.start("input")
    source = relay.sessions[0].media_broadcasts[0].sources[0]

    # Act
    await room.write_audio(tone(0.04), SAMPLE_RATE)
    await room.write_picture(picture(0), SIZE, "RGB")
    await room.write_picture(picture(10), SIZE, "RGB")
    source.keyframe_requested.set()
    await asyncio.sleep(0.01)
    await room.write_picture(picture(20), SIZE, "RGB")

    # Assert
    kinds = [call[0] for call in source.calls]
    keyframes = [call[1].startswith(b"\x00\x00\x00\x01\x67") for call in source.calls if call[0] == "push_video"]
    assert kinds[:2] == ["configure_video", "configure_audio"]
    assert "push_audio" in kinds
    assert keyframes == [True, False, True]
    await room.stop("input")
    await session.close()


async def test_the_bots_media_write_that_finds_its_media_source_failed_raises_and_later_ones_are_dropped():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    room = Room(session, room_options(audio_out=True, video_out=True), RecordingSink())
    await room.start("input")
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    source.failure = RuntimeError("media publishing stopped")

    # Act / Assert
    with pytest.raises(RuntimeError, match="media publishing stopped"):
        await room.write_picture(picture(0), SIZE, "RGB")
    assert await room.write_audio(tone(0.04), SAMPLE_RATE) is False
    room.finish_audio()
    assert await room.write_picture(picture(10), SIZE, "RGB") is False
    assert [call[0] for call in source.calls if call[0].startswith("push")] == []
    await room.stop("input")
    await session.close()


async def test_the_transport_encodes_and_lists_its_media_at_the_bitrates_and_framerate_of_its_params():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM,
        audio_out_enabled=True,
        audio_out_bitrate=64_000,
        video_out_enabled=True,
        video_out_bitrate=500_000,
        video_out_framerate=15,
        video_out_width=SIZE[0],
        video_out_height=SIZE[1],
    )
    room = MoqWasmTransport(session, params)._room

    # Act: an interruption replaces the Opus encoder
    await room.start("output")
    room.interrupt_audio()

    # Assert
    options = relay.sessions[0].media_broadcasts[0].sources[0].options
    assert (options["video_bitrate"], options["video_framerate"]) == (500_000, 15)
    opened = room._opened
    assert opened is not None and opened.opus is not None and opened.opus._encoder.bit_rate == 64_000
    assert opened.video is not None and opened.video._encoder.bit_rate == 500_000
    await room.stop("output")
    await session.close()


async def test_a_picture_whose_write_was_cancelled_is_encoded_alone_and_the_next_one_is_a_keyframe():
    # Arrange: the second picture's encode holds its thread until the gate opens
    relay = FakeRelay()
    session = await open_session(relay)
    room = Room(session, room_options(video_out=True), RecordingSink())
    await room.start("input")
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    await room.write_picture(picture(0), SIZE, "RGB")
    assert room._opened is not None
    video = room._opened.video
    assert video is not None
    encode = video.encode
    entered, gate, counting = threading.Event(), threading.Event(), threading.Lock()
    encoding = most = 0

    def gated(*args):
        nonlocal encoding, most
        with counting:
            encoding += 1
            most = max(most, encoding)
        entered.set()
        gate.wait(TIMEOUT_SECONDS)
        try:
            return encode(*args)
        finally:
            with counting:
                encoding -= 1

    video.encode = gated
    cancelled = asyncio.create_task(room.write_picture(picture(10), SIZE, "RGB"))
    await within(asyncio.to_thread(entered.wait))

    # Act
    cancelled.cancel()
    following = asyncio.create_task(room.write_picture(picture(20), SIZE, "RGB"))
    await asyncio.sleep(0.05)
    gate.set()
    await within(following)
    await within(asyncio.gather(cancelled, return_exceptions=True))

    # Assert: one encode at a time; the cancelled picture is not sent, and the next opens a group
    keyframes = [call[1].startswith(b"\x00\x00\x00\x01\x67") for call in source.calls if call[0] == "push_video"]
    assert most == 1
    assert keyframes == [True, True]
    await room.stop("input")
    await session.close()


async def test_a_picture_whose_write_was_cancelled_as_its_encode_finished_is_sent_before_the_next():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    room = Room(session, room_options(video_out=True), RecordingSink())
    await room.start("input")
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    await room.write_picture(picture(0), SIZE, "RGB")
    writing = asyncio.create_task(room.write_picture(picture(10), SIZE, "RGB"))

    # Act
    await within(cancel_as_it_resumes(writing))
    await asyncio.gather(writing, return_exceptions=True)
    await room.write_picture(picture(20), SIZE, "RGB")

    # Assert: the third picture refers to the second, which was sent
    keyframes = [call[1].startswith(b"\x00\x00\x00\x01\x67") for call in source.calls if call[0] == "push_video"]
    assert keyframes == [True, False, False]
    await room.stop("input")
    await session.close()


async def test_the_room_ends_once_both_processors_stopped_it_however_often_each_did():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    sink = RecordingSink()
    room = Room(session, room_options(audio_in=True, audio_out=True), sink)
    await room.start("input")
    await room.start("output")

    # Act: pipecat stops and then cancels the input processor
    await room.stop("input")
    await room.stop("input")
    first_stop = list(relay.sessions[0].calls)
    await room.stop("output")

    # Assert
    assert ("unannounce", [*ROOM, "bot"]) not in first_stop
    assert ("unannounce", [*ROOM, "bot"]) in relay.sessions[0].calls
    assert sink.events[-1] == ("on_disconnected",)
    await session.close()


async def test_the_bot_publishes_under_the_broadcast_it_names():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    voice = [*ROOM, "bots", "voice-agent"]
    room = Room(session, room_options(audio_out=True, broadcast=voice), RecordingSink())

    # Act
    await room.start("input")
    await room.stop("input")

    # Assert
    assert ("media_announce", voice) in relay.sessions[0].calls
    await session.close()


async def test_the_second_processor_starting_the_room_waits_until_it_is_published():
    # Arrange: the relay holds the PUBLISH_NAMESPACE back
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.announce_gate = asyncio.Event()
    room = Room(session, room_options(data_out_tracks={"chat": 1.0}), RecordingSink())
    first = asyncio.create_task(room.start("input"))
    await asyncio.sleep(0.01)

    # Act
    second = asyncio.create_task(room.start("output"))
    await asyncio.sleep(0.01)
    waited = not second.done()
    native.announce_gate.set()
    await within(asyncio.gather(first, second))

    # Assert
    assert waited
    await room.stop("input")
    await room.stop("output")
    await session.close()


async def test_a_room_whose_only_starter_was_cancelled_does_not_open_after_it_stopped():
    # Arrange: the relay holds the PUBLISH_NAMESPACE back
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.announce_gate = asyncio.Event()
    sink = RecordingSink()
    room = Room(session, room_options(audio_in=True), sink)
    starting = asyncio.create_task(room.start("input"))
    await asyncio.sleep(0.01)

    # Act: pipecat cancels the start, then stops the processor; the relay answers meanwhile
    starting.cancel()
    stopping = asyncio.create_task(room.stop("input"))
    await asyncio.sleep(0.01)
    native.announce_gate.set()
    await within(asyncio.gather(starting, stopping, return_exceptions=True))
    await asyncio.sleep(0.05)

    # Assert: the namespace the relay accepted after the cancellation is withdrawn
    assert ("on_connected",) not in sink.events
    assert ("watch_namespaces", ROOM) not in native.calls
    assert [call[0] for call in native.calls if call[0] in ("announce", "unannounce")] == ["announce", "unannounce"]
    await session.close()


async def test_a_room_whose_only_starter_was_cancelled_as_it_opened_closes():
    # Arrange: the relay holds the PUBLISH_NAMESPACE back
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.announce_gate = asyncio.Event()
    sink = RecordingSink()
    room = Room(session, room_options(audio_in=True), sink)
    starting = asyncio.create_task(room.start("input"))
    await asyncio.sleep(0.01)

    # Act: pipecat cancels the start as the room finishes opening; nobody stops it after that
    cancelled = cancel_as_it_resumes(starting)
    native.announce_gate.set()
    await within(cancelled)
    await within(asyncio.gather(starting, return_exceptions=True))

    # Assert
    assert ("unannounce", [*ROOM, "bot"]) in native.calls
    assert ("unsubscribe_namespace", ROOM) in native.calls
    assert sink.events[-1] == ("on_disconnected",)
    await session.close()


async def test_a_room_started_again_while_its_stop_tears_it_down_opens_once_the_teardown_finished():
    # Arrange: the relay holds the answer to the room watch's UNSUBSCRIBE_NAMESPACE back
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    room = Room(session, room_options(data_out_tracks={"chat": 1.0}), RecordingSink())
    await within(room.start("output"))
    native.watch_close_gate = asyncio.Event()
    stopping = asyncio.create_task(room.stop("output"))
    await asyncio.sleep(0.01)

    # Act
    restarting = asyncio.create_task(room.start("output"))
    await asyncio.sleep(0.01)
    started_during_teardown = restarting.done()
    native.watch_close_gate.set()
    await within(asyncio.gather(stopping, restarting))

    # Assert: the old stop withdrew only its own broadcast, and the new one writes
    assert not started_during_teardown
    assert native.calls.count(("unannounce", [*ROOM, "bot"])) == 1
    assert await room.write_message(MoqWasmTrackObject("chat", b"hello"))
    await room.stop("output")
    await session.close()


async def test_a_room_whose_teardowns_fail_again_and_again_keeps_only_the_last_openings_encoders(monkeypatch):
    # Arrange: the relay refuses to withdraw the room's broadcast, whose speech has an encoder
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]

    async def refuse_unannounce(namespace: list[str]) -> None:
        raise RuntimeError("withdrawal refused")

    native.unannounce = refuse_unannounce
    encoders: list[weakref.ref] = []

    class TrackedEncoder(OpusEncoder):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            encoders.append(weakref.ref(self))

    monkeypatch.setattr(_room, "OpusEncoder", TrackedEncoder)
    room = Room(session, room_options(audio_out=True), RecordingSink())

    # Act: the room opens and fails to close three times
    for _ in range(3):
        await within(room.start("output"))
        with pytest.raises(RuntimeError, match="withdrawal refused"):
            await within(room.stop("output"))
    gc.collect()

    # Assert: only the encoder of the opening whose teardown failed last, which a stop raises
    assert len(encoders) == 3
    assert sum(encoder() is not None for encoder in encoders) == 1
    await session.close()


async def test_a_room_whose_openings_and_rollbacks_fail_again_and_again_keeps_only_the_last_openings_encoders(
    monkeypatch,
):
    # Arrange: the relay refuses the room watch, failing each opening, and the withdrawal of the
    # broadcast, failing each opening's rollback
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.refuse_watch = True

    async def refuse_unannounce(namespace: list[str]) -> None:
        raise RuntimeError("withdrawal refused")

    native.unannounce = refuse_unannounce
    encoders: list[weakref.ref] = []

    class TrackedEncoder(OpusEncoder):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            encoders.append(weakref.ref(self))

    monkeypatch.setattr(_room, "OpusEncoder", TrackedEncoder)
    sink = RecordingSink()
    room = Room(session, room_options(audio_out=True), sink)

    # Act: the room fails to open, and to roll the opening back, three times; the application
    # keeps none of the errors reported
    for _ in range(3):
        with pytest.raises(RuntimeError, match="withdrawal refused"):
            await within(room.start("output"))
    sink.events.clear()
    gc.collect()

    # Assert: only the encoder of the opening whose rollback failed last, which the room keeps
    assert len(encoders) == 3
    assert sum(encoder() is not None for encoder in encoders) == 1
    await session.close()


async def test_stopping_a_restart_that_waits_for_the_teardown_leaves_the_teardown_running():
    # Arrange: the relay holds the answer to the room watch's UNSUBSCRIBE_NAMESPACE back while the
    # room is started again
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    room = Room(session, room_options(data_out_tracks={"chat": 1.0}), RecordingSink())
    await within(room.start("output"))
    native.watch_close_gate = asyncio.Event()
    stopping = asyncio.create_task(room.stop("output"))
    await asyncio.sleep(0.01)
    restarting = asyncio.create_task(room.start("output"))
    await asyncio.sleep(0.01)

    # Act: the restart is stopped before the teardown finished, and the room is started once more
    stopping_restart = asyncio.create_task(room.stop("output"))
    await asyncio.sleep(0.01)
    starting = asyncio.create_task(room.start("output"))
    await asyncio.sleep(0.01)
    started_during_teardown = starting.done()
    native.watch_close_gate.set()
    await within(asyncio.gather(stopping, restarting, stopping_restart, return_exceptions=True))
    await within(starting)

    # Assert: the teardown withdrew the broadcast before the room opened again
    assert not started_during_teardown
    assert native.calls.count(("unannounce", [*ROOM, "bot"])) == 1
    assert await room.write_message(MoqWasmTrackObject("chat", b"hello"))
    await room.stop("output")
    await session.close()


async def test_a_room_with_an_unsupported_video_codec_fails_its_start_with_the_configuration_error():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    room = Room(session, room_options(video_out=True, video_out_codec="H266"), RecordingSink())

    # Act / Assert
    with pytest.raises(ValueError):
        await within(room.start("output"))
    await session.close()


async def test_a_refused_room_watch_fails_both_processors_starts():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    relay.sessions[0].refuse_watch = True
    sink = RecordingSink()
    room = Room(session, room_options(audio_in=True), sink)

    # Act
    results = await within(asyncio.gather(room.start("input"), room.start("output"), return_exceptions=True))

    # Assert: the room is withdrawn and reported
    assert [type(result) for result in results] == [RuntimeError, RuntimeError]
    assert [event[0] for event in sink.events] == ["on_error"]
    assert ("unannounce", [*ROOM, "bot"]) in relay.sessions[0].calls
    await session.close()


async def test_stopping_a_room_whose_start_failed_waits_until_it_withdrew_its_broadcast(relay_url):
    # Arrange: the session watches the room, so the relay refuses the room's own watch once its
    # broadcast was published
    room = ["anon", f"room-{uuid.uuid4().hex[:8]}"]
    session = await moq_wasm.connect(relay_url, verify_certificate=False)
    watch = await within(session.watch_namespaces(room))
    sink = RecordingSink()
    failing = Room(session, room_options(room=room, audio_in=True, audio_out=True), sink)
    starting = asyncio.create_task(failing.start("input"))

    async def reported() -> None:
        # Polled at every step: the withdrawal is under way from the step that reported the error.
        while not sink.events:
            await asyncio.sleep(0)

    await within(reported())

    # Act: the room is stopped while it withdraws that broadcast, and the start is cancelled, as
    # pipecat cancels a processor that reported an error
    stopping = asyncio.create_task(failing.stop("input"))
    starting.cancel()
    await within(stopping)

    # Assert: a fresh room on the session publishes the same broadcast; the session's watch is closed
    # first, as the relay would refuse the fresh room's watch like the room's
    await within(watch.close())
    fresh = Room(session, room_options(room=room, audio_in=True, audio_out=True), RecordingSink())
    await within(fresh.start("input"))
    await within(fresh.stop("input"))
    await asyncio.gather(starting, return_exceptions=True)
    await session.close()


async def test_a_room_that_cannot_open_leaves_both_processors_unusable_and_ends_the_pipeline():
    # Arrange: the relay refuses the room's SUBSCRIBE_NAMESPACE
    relay = FakeRelay()
    session = await open_session(relay)
    relay.sessions[0].refuse_watch = True
    transport = MoqWasmTransport(session, MoqWasmParams(room=ROOM, audio_in_enabled=True, audio_out_enabled=True))
    processors = (transport.input(), transport.output())
    worker = PipelineWorker(
        Pipeline(list(processors)),
        cancel_on_idle_timeout=False,
        processor_unusable_policy=ProcessorUnusablePolicy.CANCEL,
    )
    runner = WorkerRunner(handle_sigint=False)

    # Act
    await runner.add_workers(worker)
    await within(runner.run())

    # Assert
    assert not any(processor.is_usable for processor in processors)
    await session.close()


async def test_an_output_whose_room_never_opened_passes_on_the_frames_it_has_no_media_sender_for():
    # Arrange: the relay refuses the room's SUBSCRIBE_NAMESPACE, and the output keeps running, as
    # pipecat's default ProcessorUnusablePolicy.CONTINUE keeps it
    relay = FakeRelay()
    session = await open_session(relay)
    relay.sessions[0].refuse_watch = True
    output = MoqWasmTransport(session, MoqWasmParams(room=ROOM, audio_out_enabled=True)).output()
    _, downstream = link_recorders(output)
    await within(start_alone(output))
    frames = [OutputAudioRawFrame(audio=tone(0.04), sample_rate=SAMPLE_RATE, num_channels=1), TTSStoppedFrame(), StopFrame()]

    # Act
    for frame in frames:
        await within(output.process_frame(frame, FrameDirection.DOWNSTREAM))

    # Assert: the StopFrame went on, so the pipeline stops; the speech, not written, did not
    assert not output.is_usable
    assert [type(frame) for frame in downstream.frames] == [TTSStoppedFrame, StopFrame]
    await within(output.cleanup())
    await session.close()


async def test_cleaning_up_the_transports_processors_withdraws_the_room_that_no_end_frame_stopped():
    # Arrange: both processors started, and neither an EndFrame nor a CancelFrame reaches them
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    transport = MoqWasmTransport(session, MoqWasmParams(room=ROOM, audio_in_enabled=True, audio_out_enabled=True))
    for processor in (transport.input(), transport.output()):
        await within(start_alone(processor))

    # Act: the pipeline tears down; the second cleanup of the input changes nothing
    await within(transport.input().cleanup())
    await within(transport.output().cleanup())
    await within(transport.input().cleanup())

    # Assert
    assert native.calls.count(("unannounce", [*ROOM, "bot"])) == 1
    assert ("unsubscribe_namespace", ROOM) in native.calls
    await session.close()


async def test_cleaning_up_an_input_whose_stop_was_cancelled_returns_once_the_room_is_withdrawn():
    # Arrange: the transport's input alone uses the room, and the relay holds the answer to the
    # room watch's UNSUBSCRIBE_NAMESPACE back as the EndFrame stops the input
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    transport = MoqWasmTransport(session, MoqWasmParams(room=ROOM, audio_in_enabled=True))
    room_input = transport.input()
    await within(start_alone(room_input))
    native.watch_close_gate = asyncio.Event()
    stopping = asyncio.create_task(room_input.stop(EndFrame()))
    await asyncio.sleep(0.05)

    # Act: pipecat's cleanup cancels the stop, then cleans the input up; the relay answers meanwhile
    stopping.cancel()
    cleaning = asyncio.create_task(room_input.cleanup())
    await asyncio.sleep(0.05)
    cleaned_while_withdrawing = cleaning.done()
    native.watch_close_gate.set()
    await within(asyncio.gather(stopping, cleaning, return_exceptions=True))

    # Assert
    assert not cleaned_while_withdrawing
    assert native.calls.count(("unannounce", [*ROOM, "bot"])) == 1
    await session.close()


class FailingFilter(BaseAudioFilter):
    """An audio filter whose `stop`, which pipecat's `cleanup` calls, raises."""

    async def start(self, sample_rate: int):
        pass

    async def stop(self):
        raise OSError("the filter's device is gone")

    async def process_frame(self, frame):
        pass

    async def filter(self, audio: bytes) -> bytes:
        return audio


async def test_cleaning_up_an_output_whose_audio_filter_fails_to_stop_withdraws_the_room():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    params = MoqWasmParams(room=ROOM, audio_out_enabled=True, audio_out_filter=FailingFilter())
    started = MoqWasmTransport(session, params).output()
    await within(start_alone(started))

    # Act
    with pytest.raises(OSError):
        await within(started.cleanup())

    # Assert
    assert ("unannounce", [*ROOM, "bot"]) in native.calls
    await session.close()


async def test_cleaning_up_a_participant_input_whose_audio_filter_fails_to_stop_unsubscribes_its_tracks():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN]]
    native.tracks["audio"] = [[STAY_OPEN]]
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(session, [*ROOM, "alice"]),
        MoqWasmInputParams(audio_in_enabled=True, audio_in_sample_rate=SAMPLE_RATE, audio_in_filter=FailingFilter()),
    )
    await within(start_alone(participant_input))
    while len(native.subscriptions) < 2:
        await asyncio.sleep(0.01)

    # Act
    with pytest.raises(OSError):
        await within(participant_input.cleanup())

    # Assert: the catalog and the audio track
    assert [subscription.closed for subscription in native.subscriptions] == [True, True]
    await session.close()


async def test_cleaning_up_a_participant_input_that_no_end_frame_stopped_unsubscribes_its_tracks():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN]]
    native.tracks["audio"] = [[STAY_OPEN]]
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(session, [*ROOM, "alice"]),
        MoqWasmInputParams(audio_in_enabled=True, audio_in_sample_rate=SAMPLE_RATE),
    )
    await within(start_alone(participant_input))
    while len(native.subscriptions) < 2:
        await asyncio.sleep(0.01)

    # Act
    await within(participant_input.cleanup())

    # Assert: the catalog and the audio track
    assert [subscription.closed for subscription in native.subscriptions] == [True, True]
    await session.close()


async def test_a_cleaned_up_participant_input_keeps_no_decoder_or_picture(kept_media):
    # Arrange: alice's audio and camera are received
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    tracks = json.loads(AUDIO_CATALOG)["tracks"] + json.loads(VIDEO_CATALOG)["tracks"]
    native.tracks["catalog"] = [[catalog_payload(*tracks), STAY_OPEN]]
    native.tracks["audio"] = [[*OpusEncoder().encode(tone(0.04), SAMPLE_RATE), STAY_OPEN]]
    native.tracks["video"] = [[*video_frames(2), STAY_OPEN]]
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(session, [*ROOM, "alice"]),
        MoqWasmInputParams(
            audio_in_enabled=True,
            audio_in_sample_rate=SAMPLE_RATE,
            video_in_enabled=True,
            video_in_sources={"camera": VideoInSourceParams(framerate=30)},
        ),
    )
    await within(start_alone(participant_input))
    await until(lambda: kept_media.decoders and kept_media.pictures)

    # Act
    await within(participant_input.cleanup())

    # Assert: the application still holds the input; a decoding step still in flight in a worker frees its pictures when it finishes
    await until(lambda: kept_media.alive() == (0, 0))
    await session.close()


async def test_a_cleaned_up_participant_input_keeps_no_catalog_subscription(monkeypatch):
    # Arrange: alice's audio is received; each read of her catalogs is followed by a weak reference
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN]]
    native.tracks["audio"] = [[STAY_OPEN]]
    catalogs: list[weakref.ref] = []
    read_catalogs = RemoteBroadcast.catalogs

    def tracked_catalogs(remote: RemoteBroadcast):
        read = read_catalogs(remote)
        catalogs.append(weakref.ref(read))
        return read

    monkeypatch.setattr(RemoteBroadcast, "catalogs", tracked_catalogs)
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(session, [*ROOM, "alice"]),
        MoqWasmInputParams(audio_in_enabled=True, audio_in_sample_rate=SAMPLE_RATE),
    )
    await within(start_alone(participant_input))
    while len(native.subscriptions) < 2:
        await asyncio.sleep(0.01)

    # Act
    await within(participant_input.cleanup())
    gc.collect()

    # Assert: the application still holds the input
    assert len(catalogs) == 1
    assert catalogs[0]() is None
    await session.close()


async def test_a_cleaned_up_participant_input_keeps_neither_the_broadcast_nor_its_catalog(monkeypatch):
    # Arrange: alice's audio is received; her broadcast and each catalog read are followed by a weak
    # reference
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN]]
    native.tracks["audio"] = [[STAY_OPEN]]
    catalogs: list[weakref.ref] = []
    read_catalogs = RemoteBroadcast.catalogs

    async def tracked_catalogs(remote: RemoteBroadcast):
        async with contextlib.aclosing(read_catalogs(remote)) as read:
            async for catalog in read:
                catalogs.append(weakref.ref(catalog))
                yield catalog

    monkeypatch.setattr(RemoteBroadcast, "catalogs", tracked_catalogs)
    remote = RemoteBroadcast(session, [*ROOM, "alice"])
    broadcast = weakref.ref(remote)
    participant_input = MoqWasmParticipantInput(
        remote, MoqWasmInputParams(audio_in_enabled=True, audio_in_sample_rate=SAMPLE_RATE, video_in_enabled=True)
    )
    del remote
    answers: list[dict] = []

    async def answered(result: dict) -> None:
        answers.append(result)

    await within(start_alone(participant_input))
    await until(lambda: len(native.subscriptions) == 2)

    # Act
    await within(participant_input.cleanup())
    gc.collect()
    await within(participant_input.request_image(UserImageRequestFrame(user_id="alice", result_callback=answered)))

    # Assert: the application still holds the input, which answers a request with alice's departure
    assert broadcast() is None
    assert len(catalogs) == 1
    assert catalogs[0]() is None
    assert answers == [{"error": "alice left"}]
    await session.close()


async def test_a_participant_input_whose_stop_was_cancelled_answers_its_image_requests_before_its_cleanup_returns():
    # Arrange: alice's audio is received, a request waits for a camera her catalog has no track
    # for, and the relay holds the answer to her audio's UNSUBSCRIBE back as the EndFrame stops the
    # input
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN]]
    native.tracks["audio"] = [[STAY_OPEN]]
    unsubscribing = native.unsubscribe_gates["audio"] = asyncio.Event()
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(session, [*ROOM, "alice"]),
        MoqWasmInputParams(audio_in_enabled=True, audio_in_sample_rate=SAMPLE_RATE, video_in_enabled=True),
    )
    answers: list[dict] = []

    async def answered(result: dict) -> None:
        answers.append(result)

    await within(start_alone(participant_input))
    await until(lambda: len(native.subscriptions) == 2)
    await within(participant_input.request_image(UserImageRequestFrame(user_id="alice", result_callback=answered)))
    stopping = asyncio.create_task(participant_input.stop(EndFrame()))
    await until(lambda: native.subscriptions[1].unsubscribing)

    # Act: pipecat's cleanup cancels the stop, then cleans the input up; the relay answers meanwhile
    stopping.cancel()
    cleaning = asyncio.create_task(participant_input.cleanup())
    await asyncio.sleep(0.05)
    cleaned_while_unsubscribing = cleaning.done()
    unsubscribing.set()
    await within(asyncio.gather(stopping, cleaning, return_exceptions=True))

    # Assert
    assert not cleaned_while_unsubscribing
    assert answers == [{"error": "alice left"}]
    assert [subscription.closed for subscription in native.subscriptions] == [True, True]
    await session.close()


async def test_a_participant_input_whose_session_closed_answers_its_image_request_before_its_worker_returns(relay_url):
    # Arrange: through a relay, a worker run by the bot's session receives alice, whose catalog has
    # no camera, and a request for her camera waits; its callback takes 300 ms
    room = ["anon", f"room-{uuid.uuid4().hex[:8]}"]
    alice_session = await moq_wasm.connect(relay_url, verify_certificate=False)
    bot_session = await moq_wasm.connect(relay_url, verify_certificate=False)
    alice = create_broadcast(alice_session, [*room, "alice"])
    catalog = alice.publish_track("catalog")
    await within(alice.__aenter__())
    await within(alice.published())
    await catalog.write(AUDIO_CATALOG, new_group=True)
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(bot_session, [*room, "alice"]), MoqWasmInputParams(video_in_enabled=True)
    )
    worker = PipelineWorker(Pipeline([participant_input]), cancel_on_idle_timeout=False)
    answers: list[dict] = []

    async def answered(result: dict) -> None:
        await asyncio.sleep(0.3)
        answers.append(result)

    @worker.event_handler("on_pipeline_started")
    async def on_pipeline_started(worker: PipelineWorker, frame: StartFrame) -> None:
        await worker.queue_frame(UserImageRequestFrame(user_id="alice", result_callback=answered))

    runner = WorkerRunner(handle_sigint=False)
    await runner.add_workers(worker)
    running = asyncio.create_task(bot_session.run(runner.run()))
    receiver = participant_input._receiver
    await until(lambda: receiver._catalog is not None and receiver._sampler("camera").requests())

    # Act: the session's end starts the receiver's teardown, and the worker's cancel stops the input
    await bot_session.close()
    await within(asyncio.gather(running, return_exceptions=True))

    # Assert
    assert answers == [{"error": "alice left"}]
    await alice.__aexit__(None, None, None)
    await alice_session.close()


class HeldCallback:
    """An image request's `result_callback` that holds its answer until `finish` is set, recording
    it, or "cancelled" when it was cancelled before."""

    def __init__(self, outcomes: list):
        self.outcomes = outcomes
        self.delivering = asyncio.Event()
        self.finish = asyncio.Event()

    async def __call__(self, result: dict) -> None:
        self.delivering.set()
        try:
            await self.finish.wait()
        except asyncio.CancelledError:
            self.outcomes.append("cancelled")
            raise
        self.outcomes.append(result)


async def test_an_end_frame_lets_the_timeout_answer_being_delivered_finish_before_the_stop_returns(monkeypatch):
    # Arrange: a request for a camera alice's catalog has no track for timed out, and its callback
    # holds the error it is given
    monkeypatch.setattr(_participant, "IMAGE_REQUEST_TIMEOUT_SECONDS", 0.05)
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN]]
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(session, [*ROOM, "alice"]), MoqWasmInputParams(video_in_enabled=True)
    )
    outcomes: list = []
    held = HeldCallback(outcomes)
    await within(start_alone(participant_input))
    await within(participant_input.request_image(UserImageRequestFrame(user_id="alice", result_callback=held)))
    await within(held.delivering.wait())

    # Act
    stopping = asyncio.create_task(participant_input.stop(EndFrame()))
    await asyncio.sleep(0.05)
    stopped_while_delivering = stopping.done()
    held.finish.set()
    await within(stopping)

    # Assert
    assert not stopped_while_delivering
    assert outcomes == [{"error": "alice has no camera track"}]
    await within(participant_input.cleanup())
    await session.close()


async def test_a_release_retried_after_one_cancelled_while_answering_returns_once_every_request_was_answered():
    # Arrange: two requests wait on alice's camera, which sends no picture; the first one's
    # callback holds the release's error, and the release is cancelled meanwhile
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [[STAY_OPEN]]
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(session, [*ROOM, "alice"]),
        MoqWasmInputParams(video_in_enabled=True, video_in_sources={"camera": VideoInSourceParams(on_request_only=True)}),
    )
    outcomes: list = []
    held = HeldCallback(outcomes)

    async def answered(result: dict) -> None:
        outcomes.append(result)

    await within(start_alone(participant_input))
    await until(lambda: len(native.subscriptions) == 2)
    for callback in (held, answered):
        await within(participant_input.request_image(UserImageRequestFrame(user_id="alice", result_callback=callback)))
    releasing = asyncio.create_task(participant_input._receiver.release("camera"))
    await within(held.delivering.wait())
    releasing.cancel()
    await asyncio.gather(releasing, return_exceptions=True)

    # Act
    retried = asyncio.create_task(participant_input._receiver.release("camera"))
    await asyncio.sleep(0.05)
    returned_while_answering = retried.done()
    held.finish.set()
    await within(retried)

    # Assert
    assert not returned_while_answering
    assert outcomes == [{"error": "camera of alice released"}] * 2
    await within(participant_input.cleanup())
    await session.close()


async def test_an_image_requests_error_callback_releasing_its_own_source_returns():
    # Arrange: a request waits on alice's camera, which sends no picture; its callback releases
    # the camera once it gets the error
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [[STAY_OPEN]]
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(session, [*ROOM, "alice"]),
        MoqWasmInputParams(video_in_enabled=True, video_in_sources={"camera": VideoInSourceParams(on_request_only=True)}),
    )
    outcomes: list = []
    released = asyncio.Event()

    async def release_on_error(result: dict) -> None:
        outcomes.append(result)
        await participant_input._receiver.release("camera")
        released.set()

    await within(start_alone(participant_input))
    await until(lambda: len(native.subscriptions) == 2)
    request = UserImageRequestFrame(user_id="alice", result_callback=release_on_error)
    await within(participant_input.request_image(request))

    # Act: a release from outside answers the request
    await within(participant_input._receiver.release("camera"))

    # Assert
    assert released.is_set()
    assert outcomes == [{"error": "camera of alice released"}]
    await within(participant_input.cleanup())
    await session.close()


async def test_two_timed_out_image_requests_whose_error_callbacks_release_their_source_both_return(monkeypatch):
    # Arrange: two requests wait on alice's camera, which sends no picture; each one's callback
    # releases the camera once both got their timeout error, so neither release answers the other
    # request first
    monkeypatch.setattr(_participant, "IMAGE_REQUEST_TIMEOUT_SECONDS", 0.05)
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [[STAY_OPEN]]
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(session, [*ROOM, "alice"]),
        MoqWasmInputParams(video_in_enabled=True, video_in_sources={"camera": VideoInSourceParams(on_request_only=True)}),
    )
    answered: list[dict] = []
    both_answered = asyncio.Event()
    released: list[dict] = []

    async def release_on_error(result: dict) -> None:
        answered.append(result)
        if len(answered) == 2:
            both_answered.set()
        await both_answered.wait()
        await participant_input._receiver.release("camera")
        released.append(result)

    await within(start_alone(participant_input))
    await until(lambda: len(native.subscriptions) == 2)

    # Act
    for _ in range(2):
        await within(participant_input.request_image(UserImageRequestFrame(user_id="alice", result_callback=release_on_error)))
    await within(until(lambda: len(released) == 2))

    # Assert
    assert released == [{"error": "no picture of camera of alice came in time"}] * 2
    await within(participant_input.stop(EndFrame()))
    await within(participant_input.cleanup())
    await session.close()


async def test_the_transport_registers_the_events_of_moq_transport():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)

    # Act
    transport = MoqWasmTransport(session, MoqWasmParams(room=ROOM, audio_in_enabled=True))

    # Assert
    for event in ("on_connected", "on_disconnected", "on_client_connected", "on_client_disconnected", "on_track_subscribed", "on_error"):
        transport.event_handler(event)(lambda *args: None)
    assert transport.input() is transport.input()
    await session.close()


async def test_the_transport_gives_the_id_of_a_client_its_on_client_connected_passed():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    transport = MoqWasmTransport(session, MoqWasmParams(room=ROOM, audio_out_enabled=True))
    connected = asyncio.Event()
    client_ids = []

    @transport.event_handler("on_client_connected")
    async def on_client_connected(transport, client):
        client_ids.append(transport.get_client_id(client))
        connected.set()

    await within(start_alone(transport.output()))

    # Act
    relay.sessions[0].publish([*ROOM, "alice"])
    await within(connected.wait())

    # Assert
    assert client_ids == ["alice"]
    await transport.output().cleanup()
    await session.close()


async def test_a_participant_input_pushes_only_its_participants_audio_and_publishes_nothing():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN]]
    native.tracks["audio"] = [OpusEncoder().encode(tone(0.04), SAMPLE_RATE)]
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(session, [*ROOM, "alice"]),
        MoqWasmInputParams(audio_in_enabled=True, audio_in_sample_rate=SAMPLE_RATE),
    )
    subscribed = []
    participant_input.event_handler("on_track_subscribed")(lambda _, track: subscribed.append(track))

    # Act
    down, _ = await within(run_test(participant_input, frames_to_send=[SleepFrame(0.5)]))

    # Assert
    audio = [frame for frame in down if isinstance(frame, UserAudioRawFrame)]
    assert audio and {frame.user_id for frame in audio} == {"alice"}
    assert subscribed == [MoqWasmTrack("alice", "audio", "audio")]
    assert not native.media_broadcasts
    assert not [call for call in native.calls if call[0] in ("announce", "watch_namespaces", "publish_track")]
    await session.close()


@pytest.mark.parametrize("input_kind", ["transport", "participant"])
@pytest.mark.parametrize(
    "source_channels, frequencies", [(2, (440, 1000)), (1, (440, 440))], ids=["stereo", "mono upmixed"]
)
async def test_a_stereo_input_pushes_stereo_audio_of_a_participant(input_kind, source_channels, frequencies):
    # Arrange: alice publishes a 440 Hz tone on the left and a 1 kHz tone on the right, or a 440 Hz
    # tone in mono, to an input taking stereo
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN]]
    if source_channels == 2:
        pcm = stereo_tone(0.2, 48_000)
    else:
        pcm = sine(0.2, 48_000, frequency=440.0).tobytes()
    packets = OpusEncoder(channels=source_channels).encode(pcm, 48_000, source_channels)
    native.tracks["audio"] = [[*packets, STAY_OPEN]]
    native.publish([*ROOM, "alice"])
    if input_kind == "transport":
        params = MoqWasmParams(room=ROOM, audio_in_enabled=True, audio_in_sample_rate=48_000, audio_in_channels=2)
        audio_input = MoqWasmTransport(session, params).input()
    else:
        params = MoqWasmInputParams(audio_in_enabled=True, audio_in_sample_rate=48_000, audio_in_channels=2)
        audio_input = MoqWasmParticipantInput(RemoteBroadcast(session, [*ROOM, "alice"]), params)

    # Act
    down, _ = await within(run_test(audio_input, frames_to_send=[SleepFrame(0.5)]))

    # Assert: two interleaved channels, each with its own tone
    audio = [frame for frame in down if isinstance(frame, UserAudioRawFrame)]
    left, right = np.frombuffer(b"".join(frame.audio for frame in audio), np.int16).reshape(-1, 2).T
    assert audio and {frame.num_channels for frame in audio} == {2}
    assert len(left) >= 0.15 * 48_000
    assert abs(dominant_frequency(left, 48_000) - frequencies[0]) < 20
    assert abs(dominant_frequency(right, 48_000) - frequencies[1]) < 20
    await session.close()


@pytest.mark.parametrize("input_kind", ["transport", "participant"])
async def test_an_input_keeps_the_params_it_was_built_with(input_kind):
    # Arrange: a mono input whose params the caller turns to stereo once it is built
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN]]
    packets = OpusEncoder().encode(sine(0.2, 48_000, frequency=440.0).tobytes(), 48_000)
    native.tracks["audio"] = [[*packets, STAY_OPEN]]
    native.publish([*ROOM, "alice"])
    if input_kind == "transport":
        params = MoqWasmParams(room=ROOM, audio_in_enabled=True, audio_in_sample_rate=48_000)
        audio_input = MoqWasmTransport(session, params).input()
    else:
        params = MoqWasmInputParams(audio_in_enabled=True, audio_in_sample_rate=48_000)
        audio_input = MoqWasmParticipantInput(RemoteBroadcast(session, [*ROOM, "alice"]), params)
    params.audio_in_channels = 2

    # Act
    down, _ = await within(run_test(audio_input, frames_to_send=[SleepFrame(0.5)]))

    # Assert: the audio is decoded and labelled mono, as when the input was built
    audio = [frame for frame in down if isinstance(frame, UserAudioRawFrame)]
    assert audio and {frame.num_channels for frame in audio} == {1}
    await session.close()


@pytest.mark.parametrize("terminal", [EndFrame, CancelFrame])
async def test_a_participant_input_set_up_in_a_second_pipeline_fails_its_setup_and_receives_nothing(terminal):
    # Arrange: the input ran in a pipeline that an EndFrame or a CancelFrame ended
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN] for _ in range(2)]
    native.tracks["audio"] = [[*OpusEncoder().encode(tone(0.04), SAMPLE_RATE), STAY_OPEN] for _ in range(2)]
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(session, [*ROOM, "alice"]),
        MoqWasmInputParams(audio_in_enabled=True, audio_in_sample_rate=SAMPLE_RATE),
    )
    await within(run_test(participant_input, frames_to_send=[SleepFrame(0.2), terminal()], send_end_frame=False))
    worker = PipelineWorker(
        Pipeline([participant_input]),
        cancel_on_idle_timeout=False,
        processor_unusable_policy=ProcessorUnusablePolicy.CANCEL,
    )
    errors: list[ErrorFrame] = []
    worker.event_handler("on_pipeline_error")(lambda _, frame: errors.append(frame))
    runner = WorkerRunner(handle_sigint=False)

    # Act: a CancelFrame left pipecat's processor dropping every frame, so this pipeline cannot
    # start
    await runner.add_workers(worker)
    await within(runner.run())

    # Assert: the setup failed and the policy ended the pipeline
    assert len(errors) == 1 and "ran in a pipeline already" in errors[0].error
    assert not participant_input.is_usable
    assert [name for _, name in native.subscribed] == ["audio"]
    await session.close()


async def test_a_pipeline_built_again_from_the_transport_after_a_cancel_frame_opens_the_room_again():
    # Arrange: the transport's processors ran in a pipeline that a CancelFrame ended
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    transport = MoqWasmTransport(session, MoqWasmParams(room=ROOM, audio_in_enabled=True, data_out_tracks={"status": 60}))
    first = (transport.input(), transport.output())
    await within(run_test(Pipeline(list(first)), frames_to_send=[SleepFrame(0.1), CancelFrame()], send_end_frame=False))
    message = OutputTransportMessageFrame(message=MoqWasmTrackObject("status", b"again"))

    # Act
    second = (transport.input(), transport.output())
    await within(run_test(Pipeline(list(second)), frames_to_send=[SleepFrame(0.1), message, SleepFrame(0.1)]))

    # Assert: the second pipeline took new processors, which opened the room again
    assert all(new is not old for new, old in zip(second, first))
    assert native.writers["status"].objects == [(b"again", True)]
    assert native.calls.count(("watch_namespaces", ROOM)) == 2
    assert native.calls.count(("unannounce", [*ROOM, "bot"])) == 2
    await session.close()


async def test_an_output_run_again_after_a_stop_frame_ends_the_media_senders_of_its_first_run():
    # Arrange: a StopFrame stopped the output's first pipeline, which keeps it running
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM, audio_out_enabled=True, audio_out_sample_rate=SAMPLE_RATE, audio_out_end_silence_secs=0
    )
    transport = MoqWasmTransport(session, params)
    output = transport.output()
    await within(run_test(output, frames_to_send=[SleepFrame(0.1), StopFrame()], send_end_frame=False))
    first_audio_task = output._media_senders[None]._audio_task
    speech = OutputAudioRawFrame(audio=tone(0.1), sample_rate=SAMPLE_RATE, num_channels=1)

    # Act: the transport gives the same output to the next pipeline, which an EndFrame ends
    again = transport.output()
    await within(run_test(again, frames_to_send=[speech, SleepFrame(0.3)]))

    # Assert: the first run's audio task ended, and the second run wrote the speech
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert again is output
    assert first_audio_task is not None and first_audio_task.done()
    assert len(source.audio_timestamps) * OPUS_FRAME_MICROS >= 80_000
    await session.close()


async def test_a_participant_input_run_again_after_a_stop_frame_keeps_one_subscription_to_each_track():
    # Arrange: a StopFrame stopped the input's first pipeline, which keeps it running
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["prompt"] = [[b"first", STAY_OPEN] for _ in range(2)]
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(session, [*ROOM, "alice"]), MoqWasmInputParams(data_in_tracks=["prompt"])
    )
    await within(run_test(participant_input, frames_to_send=[SleepFrame(0.1), StopFrame()], send_end_frame=False))

    # Act: the next pipeline runs the same input, and an EndFrame ends it
    await within(run_test(participant_input, frames_to_send=[SleepFrame(0.1)]))

    # Assert: the track was subscribed to once, and that subscription ended with the input
    assert [name for _, name in native.subscribed] == ["prompt"]
    assert all(subscription.closed for subscription in native.subscriptions)
    await session.close()


async def test_a_pipeline_built_again_from_the_transport_after_its_room_was_refused_is_usable():
    # Arrange: the relay refused the room's SUBSCRIBE_NAMESPACE, which cancelled the first pipeline
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.refuse_watch = True
    transport = MoqWasmTransport(session, MoqWasmParams(room=ROOM, audio_in_enabled=True, data_out_tracks={"status": 60}))
    runner = WorkerRunner(handle_sigint=False)
    await runner.add_workers(
        PipelineWorker(
            Pipeline([transport.input(), transport.output()]),
            cancel_on_idle_timeout=False,
            processor_unusable_policy=ProcessorUnusablePolicy.CANCEL,
        )
    )
    await within(runner.run())
    native.refuse_watch = False
    message = OutputTransportMessageFrame(message=MoqWasmTrackObject("status", b"accepted"))

    # Act
    processors = (transport.input(), transport.output())
    _, up = await within(run_test(Pipeline(list(processors)), frames_to_send=[SleepFrame(0.1), message, SleepFrame(0.1)]))

    # Assert
    assert not [frame for frame in up if isinstance(frame, ErrorFrame)]
    assert all(processor.is_usable for processor in processors)
    assert native.writers["status"].objects == [(b"accepted", True)]
    await session.close()


@pytest.mark.parametrize("asked_for", [False, True])
async def test_a_participant_input_not_streaming_on_start_pushes_audio_only_once_asked(asked_for):
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN]]
    speaking = asyncio.Event()
    native.tracks["audio"] = [[speaking, *OpusEncoder().encode(tone(0.4), SAMPLE_RATE), STAY_OPEN]]
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(session, [*ROOM, "alice"]),
        MoqWasmInputParams(audio_in_enabled=True, audio_in_sample_rate=SAMPLE_RATE, audio_in_stream_on_start=False),
    )
    frames = [InputTransportStartAudioStreamingFrame()] if asked_for else []
    asyncio.get_running_loop().call_later(0.1, speaking.set)

    # Act: alice speaks after the pipeline asked for audio, if it did
    down, _ = await within(run_test(participant_input, frames_to_send=[*frames, SleepFrame(0.3)]))

    # Assert
    assert any(isinstance(frame, UserAudioRawFrame) for frame in down) == asked_for
    await session.close()


async def test_the_rooms_audio_is_subscribed_to_only_once_streaming_starts_for_present_and_later_participants():
    # Arrange: alice is in the room
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN], [AUDIO_CATALOG, STAY_OPEN]]
    native.tracks["audio"] = [[*OpusEncoder().encode(tone(0.04), SAMPLE_RATE), STAY_OPEN] for _ in range(2)]
    sink = RecordingSink()
    room = Room(session, room_options(audio_in=True), sink)
    await room.start("input")
    native.publish([*ROOM, "alice"])
    await asyncio.sleep(0.05)
    subscribed_before = list(native.subscribed)

    # Act: bob joins once the pipeline asked for audio
    await room.start_audio()
    native.publish([*ROOM, "bob"])
    await asyncio.sleep(0.1)

    # Assert
    assert subscribed_before == []
    assert {participant for _, _, participant in sink.audio} == {"alice", "bob"}
    await room.stop("input")
    await session.close()


@pytest.mark.parametrize("asked_for", [False, True])
async def test_a_transport_not_streaming_on_start_subscribes_to_audio_only_once_asked(asked_for):
    # Arrange: the pipeline turns streaming on start off after making the transport; alice is in
    # the room
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN]]
    native.tracks["audio"] = [[*OpusEncoder().encode(tone(0.04), SAMPLE_RATE), STAY_OPEN]]
    native.publish([*ROOM, "alice"])
    transport = MoqWasmTransport(session, MoqWasmParams(room=ROOM, audio_in_enabled=True, audio_in_sample_rate=SAMPLE_RATE))
    transport.input().enable_audio_in_stream_on_start(False)
    frames = [SleepFrame(0.1), InputTransportStartAudioStreamingFrame()] if asked_for else []

    # Act
    down, _ = await within(run_test(transport.input(), frames_to_send=[*frames, SleepFrame(0.2)]))

    # Assert
    assert any(isinstance(frame, UserAudioRawFrame) for frame in down) == asked_for
    assert bool(native.subscribed) == asked_for
    await session.close()


async def test_a_participant_input_reads_whether_to_stream_audio_on_start_when_it_starts():
    # Arrange: the pipeline turns streaming on start off after making the input
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN]]
    native.tracks["audio"] = [[*OpusEncoder().encode(tone(0.04), SAMPLE_RATE), STAY_OPEN]]
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(session, [*ROOM, "alice"]),
        MoqWasmInputParams(audio_in_enabled=True, audio_in_sample_rate=SAMPLE_RATE),
    )
    participant_input.enable_audio_in_stream_on_start(False)

    # Act
    down, _ = await within(run_test(participant_input, frames_to_send=[SleepFrame(0.2)]))

    # Assert
    assert not any(isinstance(frame, UserAudioRawFrame) for frame in down)
    assert native.subscribed == []
    await session.close()


async def test_a_participant_inputs_failed_track_goes_upstream_as_a_non_fatal_error_frame():
    # Arrange: a video track no decoder takes
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[catalog_payload({"name": "video", "role": "video", "packaging": "loc", "codec": "xyz1", "bitrate": 1000000, "framerate": 30, "isLive": True}), STAY_OPEN]]
    native.tracks["video"] = [[b"frame"]]
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(session, [*ROOM, "alice"]),
        MoqWasmInputParams(
            video_in_enabled=True,
            video_in_sources={"camera": VideoInSourceParams(framerate=30)},
            video_in_track=lambda catalog: catalog.tracks[0],
        ),
    )

    # Act
    _, up = await within(run_test(participant_input, frames_to_send=[SleepFrame(0.5)]))

    # Assert
    errors = [frame for frame in up if isinstance(frame, ErrorFrame)]
    assert len(errors) == 1 and "receiving video of alice failed" in errors[0].error
    assert participant_input.is_usable
    await session.close()


VIDEO_CATALOG = catalog_payload({"name": "video", "role": "video", "packaging": "loc", "codec": "avc1.42e01f", "bitrate": 1000000, "framerate": 30, "isLive": True})


def video_frames(count: int) -> list[bytes]:
    encoder = VideoEncoder("H264", *SIZE, 30)
    return [b"".join(encoder.encode(picture(value * 10), SIZE, "RGB")) for value in range(count)]


async def test_an_image_request_is_answered_by_the_next_picture_of_a_source_received_for_requests_only():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [video_frames(3)]
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(video_sources={"camera": None}), sink)
    request = UserImageRequestFrame(user_id="alice", text="what is there?")

    # Act
    await receiver.request_image(request)
    running = asyncio.create_task(receiver.run())
    await within(sink.received.wait())
    await asyncio.sleep(0.2)

    # Assert: only the requested picture passes, the first one decoded
    assert [(p.request, p.location.object_id) for p in sink.pictures] == [(request, 0)]
    running.cancel()
    await within(asyncio.gather(running, return_exceptions=True))
    await session.close()


async def test_an_image_request_is_answered_by_the_newest_picture_and_not_by_the_older_ones_replayed_before_it():
    # Arrange: the relay holds objects 0 to 2 of the current group, which the subscription replays
    # for the decoder before the live object 3
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    frames = video_frames(4)
    native.current_groups["video"] = [video_object(0, index, frames[index]) for index in range(3)]
    native.tracks["video"] = [[video_object(0, 3, frames[3]), STAY_OPEN]]
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(video_sources={"camera": None}), sink)
    request = UserImageRequestFrame(user_id="alice", text="what is there?")

    # Act
    await receiver.request_image(request)
    running = asyncio.create_task(receiver.run())
    await within(sink.received.wait())
    await asyncio.sleep(0.2)

    # Assert: the picture of object 2, the newest when the request was made, answered it
    assert [(p.request, p.location.object_id) for p in sink.pictures] == [(request, 2)]
    running.cancel()
    await within(asyncio.gather(running, return_exceptions=True))
    await session.close()


async def test_an_image_request_is_not_answered_by_a_delayed_group_older_than_the_pictures_seen_before_it():
    # Arrange: a source passed on at every picture, whose group 3 arrives before group 2, delayed,
    # and group 4
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    encoder = VideoEncoder("H264", *SIZE, 30)

    def keyframe(value: int) -> bytes:
        encoder.request_keyframe()
        return b"".join(encoder.encode(picture(value), SIZE, "RGB"))

    delayed = asyncio.Event()
    native.tracks["video"] = [
        [video_object(3, 0, keyframe(30)), delayed, video_object(2, 0, keyframe(20)), video_object(4, 0, keyframe(40)), STAY_OPEN]
    ]
    sink = RecordingSink()
    options = receive_options(video_sources={"camera": float("inf")})
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), options, sink)
    running = asyncio.create_task(receiver.run())
    await until(lambda: len(sink.pictures) == 1)
    request = UserImageRequestFrame(user_id="alice", text="what is there?")

    # Act
    await receiver.request_image(request)
    delayed.set()
    await until(lambda: any(picture.request is request for picture in sink.pictures))

    # Assert: the picture of group 4 answered the request, not the one of group 2
    answering = next(picture for picture in sink.pictures if picture.request is request)
    assert answering.location.group_id == 4
    running.cancel()
    await within(asyncio.gather(running, return_exceptions=True))
    await session.close()


async def test_an_image_request_made_before_a_new_subscription_is_answered_by_its_pictures_from_group_0():
    # Arrange: a source passed on at every picture, whose first subscription brings group 5 and
    # ends, as when alice's page leaves; the page taking over starts its groups at 0 again
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    first, second = (b"".join(VideoEncoder("H264", *SIZE, 30).encode(picture(value), SIZE, "RGB")) for value in (50, 0))
    leaving = asyncio.Event()
    native.tracks["video"] = [[video_object(5, 0, first), leaving], [video_object(0, 0, second), STAY_OPEN]]
    sink = RecordingSink()
    options = receive_options(video_sources={"camera": float("inf")})
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), options, sink)
    running = asyncio.create_task(receiver.run())
    await until(lambda: len(sink.pictures) == 1)
    request = UserImageRequestFrame(user_id="alice", text="what is there?")

    # Act
    await receiver.request_image(request)
    leaving.set()

    # Assert
    await until(lambda: any(picture.request is request for picture in sink.pictures))
    answering = next(picture for picture in sink.pictures if picture.request is request)
    assert answering.location.group_id == 0
    running.cancel()
    await within(asyncio.gather(running, return_exceptions=True))
    await session.close()


async def test_a_request_whose_picture_is_being_delivered_when_the_participant_leaves_gets_an_error():
    # Arrange: the sink holds the picture answering the request
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [[*video_frames(3), STAY_OPEN]]
    sink = RecordingSink()
    delivering = asyncio.Event()

    async def push_picture(picture) -> bool:
        delivering.set()
        await asyncio.Event().wait()
        return True

    sink.push_picture = push_picture
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(video_sources={"camera": None}), sink)
    request = UserImageRequestFrame(user_id="alice")
    await receiver.request_image(request)
    running = asyncio.create_task(receiver.run())
    await within(delivering.wait())

    # Act: alice leaves
    running.cancel()
    await asyncio.gather(running, return_exceptions=True)

    # Assert
    assert sink.unanswered == [(request, "alice left")]
    await session.close()


async def test_a_request_whose_picture_a_paused_input_dropped_gets_an_error_after_a_while(monkeypatch):
    # Arrange: a StopFrame paused the input before alice's camera sends its pictures
    monkeypatch.setattr(_participant, "IMAGE_REQUEST_TIMEOUT_SECONDS", 0.1)
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    sending = asyncio.Event()
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [[sending, *video_frames(3), STAY_OPEN]]
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(session, [*ROOM, "alice"]),
        MoqWasmInputParams(video_in_enabled=True, video_in_sources={"camera": VideoInSourceParams(on_request_only=True)}),
    )
    await within(start_alone(participant_input))
    await participant_input.pause(StopFrame())
    results = []

    async def result_callback(result) -> None:
        results.append(result)

    # Act
    await participant_input.request_image(UserImageRequestFrame(user_id="alice", result_callback=result_callback))
    sending.set()
    await asyncio.sleep(0.3)

    # Assert
    assert results == [{"error": "no picture of camera of alice came in time"}]
    await within(participant_input.cleanup())
    await session.close()


async def test_a_request_whose_delivery_outlived_its_wait_and_whose_track_was_replaced_gets_an_error(monkeypatch):
    # Arrange: the sink holds the picture answering the request past its wait; alice's second
    # catalog renames her camera track, which sends no picture
    monkeypatch.setattr(_participant, "IMAGE_REQUEST_TIMEOUT_SECONDS", 0.05)
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    renaming = asyncio.Event()
    renamed = catalog_payload({"name": "camera2", "role": "video", "packaging": "loc", "codec": "avc1.42e01f", "bitrate": 1000000, "framerate": 30, "isLive": True})
    native.tracks["catalog"] = [[VIDEO_CATALOG, renaming, renamed, STAY_OPEN]]
    native.tracks["video"] = [[*video_frames(3), STAY_OPEN]]
    native.tracks["camera2"] = [[STAY_OPEN]]
    sink = RecordingSink()
    delivering = asyncio.Event()

    async def push_picture(picture) -> bool:
        delivering.set()
        await asyncio.Event().wait()
        return True

    sink.push_picture = push_picture
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(video_sources={"camera": None}), sink)
    request = UserImageRequestFrame(user_id="alice")
    await receiver.request_image(request)
    running = asyncio.create_task(receiver.run())
    await within(delivering.wait())
    await asyncio.sleep(0.1)

    # Act
    renaming.set()
    await asyncio.sleep(0.2)

    # Assert
    assert sink.unanswered == [(request, "no picture of camera of alice came in time")]
    running.cancel()
    await asyncio.gather(running, return_exceptions=True)
    await session.close()


async def test_an_image_request_for_a_source_not_received_starts_receiving_that_track():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [[*video_frames(2), STAY_OPEN]]
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(video_in=True), sink)
    running = asyncio.create_task(receiver.run())
    await asyncio.sleep(0.1)

    # Act
    await receiver.request_image(UserImageRequestFrame(user_id="alice", video_source="video"))
    await within(sink.received.wait())

    # Assert
    assert sink.pictures[0].video_source == "video"
    assert ("on_track_subscribed", MoqWasmTrack("alice", "video", "video")) in sink.events
    running.cancel()
    await within(asyncio.gather(running, return_exceptions=True))
    await session.close()


async def test_a_request_for_someone_not_in_the_room_and_one_waiting_when_the_participant_leaves_get_an_error():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[catalog_payload(), STAY_OPEN]]
    sink = RecordingSink()
    room = Room(session, room_options(video_in=True), sink)
    await room.start("input")
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "bob"]), receive_options(camera=True), sink)
    waiting = UserImageRequestFrame(user_id="bob")

    # Act: bob's catalog has no video track, and bob leaves
    await room.request_image(UserImageRequestFrame(user_id="carol"))
    await receiver.request_image(waiting)
    running = asyncio.create_task(receiver.run())
    await asyncio.sleep(0.05)
    running.cancel()
    await asyncio.gather(running, return_exceptions=True)

    # Assert
    assert [error for _, error in sink.unanswered] == ["no participant carol", "bob left"]
    assert sink.unanswered[1][0] is waiting
    await room.stop("input")
    await session.close()


async def test_a_request_for_a_source_the_catalog_has_no_track_for_gets_an_error_after_a_while(monkeypatch):
    # Arrange: alice's catalog has no video track
    monkeypatch.setattr(_participant, "IMAGE_REQUEST_TIMEOUT_SECONDS", 0.05)
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN]]
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(video_in=True), sink)
    running = asyncio.create_task(receiver.run())
    await asyncio.sleep(0.01)
    request = UserImageRequestFrame(user_id="alice", video_source="screen")

    # Act
    await receiver.request_image(request)
    await asyncio.sleep(0.02)
    waited = list(sink.unanswered)
    await asyncio.sleep(0.1)

    # Assert: alice is still in the room
    assert waited == []
    assert sink.unanswered == [(request, "alice has no screen track")]
    running.cancel()
    await asyncio.gather(running, return_exceptions=True)
    await session.close()


@pytest.mark.parametrize(
    ("packaging", "codec"), [("cmaf", "avc1.42e01f"), ("loc", "xyz1")], ids=["not loc", "not decodable"]
)
async def test_a_named_video_track_the_transport_cannot_decode_is_not_subscribed_to(monkeypatch, packaging, codec):
    # Arrange: alice's screen track is CMAF, or in a codec PyAV does not decode
    monkeypatch.setattr(_participant, "IMAGE_REQUEST_TIMEOUT_SECONDS", 0.05)
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    screen = {"name": "screen", "role": "video", "packaging": packaging, "codec": codec, "bitrate": 1000000, "framerate": 30, "isLive": True}
    native.tracks["catalog"] = [[catalog_payload(screen), STAY_OPEN]]
    native.tracks["screen"] = [[STAY_OPEN]]
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(video_in=True), sink)
    running = asyncio.create_task(receiver.run())
    await asyncio.sleep(0.01)
    request = UserImageRequestFrame(user_id="alice", video_source="screen")

    # Act
    await receiver.request_image(request)
    await asyncio.sleep(0.1)

    # Assert
    assert ("subscribe", "screen") not in native.calls
    assert sink.unanswered == [(request, "alice has no screen track")]
    running.cancel()
    await asyncio.gather(running, return_exceptions=True)
    await session.close()


@pytest.mark.parametrize(
    ("catalogs", "error"),
    [
        ([], "alice has published no catalog"),
        ([[VIDEO_CATALOG, STAY_OPEN]], "no picture of camera of alice came in time"),
    ],
    ids=["no catalog", "refused subscribe"],
)
async def test_a_request_no_picture_answers_gets_an_error_after_a_while(monkeypatch, catalogs, error):
    # Arrange: alice publishes no catalog, or the relay refuses the SUBSCRIBE to her camera
    monkeypatch.setattr(_participant, "IMAGE_REQUEST_TIMEOUT_SECONDS", 0.1)
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = catalogs
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(video_in=True), sink)
    running = asyncio.create_task(receiver.run())
    request = UserImageRequestFrame(user_id="alice")

    # Act
    await receiver.request_image(request)
    await asyncio.sleep(0.05)
    waited = list(sink.unanswered)
    await asyncio.sleep(0.2)

    # Assert: alice is still in the room
    assert waited == []
    assert sink.unanswered == [(request, error)]
    running.cancel()
    await asyncio.gather(running, return_exceptions=True)
    await session.close()


async def test_requests_waiting_on_a_failed_track_or_made_after_the_participant_left_get_an_error():
    # Arrange: a video track no decoder takes
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[catalog_payload({"name": "video", "role": "video", "packaging": "loc", "codec": "xyz1", "bitrate": 1000000, "framerate": 30, "isLive": True}), STAY_OPEN]]
    native.tracks["video"] = [[b"frame"]]
    sink = RecordingSink()
    options = receive_options(video_sources={"camera": None}, video_in_track=lambda catalog: catalog.tracks[0])
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), options, sink)
    waiting = UserImageRequestFrame(user_id="alice")
    await receiver.request_image(waiting)

    # Act
    running = asyncio.create_task(receiver.run())
    await asyncio.sleep(0.1)
    running.cancel()
    await asyncio.gather(running, return_exceptions=True)
    await receiver.request_image(UserImageRequestFrame(user_id="alice"))

    # Assert
    assert [error for _, error in sink.unanswered] == ["receiving video of alice failed", "alice left"]
    assert sink.unanswered[0][0] is waiting
    await session.close()


async def test_a_captured_source_passes_pictures_until_it_is_released():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [[*video_frames(2), STAY_OPEN]]
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(video_in=True), sink)
    running = asyncio.create_task(receiver.run())
    await asyncio.sleep(0.05)

    # Act
    await receiver.capture("video", 1000.0)
    await within(sink.received.wait())
    waiting = UserImageRequestFrame(user_id="alice", video_source="video")
    await receiver.request_image(waiting)
    await receiver.release("video")
    await asyncio.sleep(0.05)

    # Assert: the live track sends no more pictures, so the request waits until the release
    assert sink.pictures[0].video_source == "video"
    assert sink.unanswered == [(waiting, "video of alice released")]
    assert native.subscriptions[-1].closed
    running.cancel()
    await within(asyncio.gather(running, return_exceptions=True))
    await session.close()


@pytest.mark.parametrize(
    ("framerate", "on_request_only", "rate"),
    [(15, False, 15), (None, False, float("inf")), (30, True, None), (0, False, None)],
)
async def test_capturing_a_participants_video_receives_it_at_the_framerate_pipecat_means(
    monkeypatch, framerate, on_request_only, rate
):
    # Arrange: a framerate of 0 is pipecat's deprecated way to ask for request-only pictures
    relay = FakeRelay()
    session = await open_session(relay)
    transport = MoqWasmTransport(session, MoqWasmParams(room=ROOM, video_in_enabled=True))
    captured = []

    async def capture(room, participant_id, source, rate):
        captured.append((participant_id, source, rate))

    monkeypatch.setattr(Room, "capture", capture)
    deprecation._warned_sites.clear()

    # Act
    with warnings.catch_warnings(record=True) as warned:
        warnings.simplefilter("always")
        await transport.capture_participant_video("alice", framerate, on_request_only=on_request_only)

    # Assert: the deprecation names the caller's line
    assert captured == [("alice", "camera", rate)]
    deprecated = [warning for warning in warned if issubclass(warning.category, DeprecationWarning)]
    assert [warning.filename for warning in deprecated] == ([__file__] if framerate == 0 else [])
    await session.close()


def test_a_camera_captured_at_its_own_framerate_passes_on_as_many_jittery_pictures_as_pipecats_sampler(monkeypatch):
    # Arrange: five seconds of a 29.97 fps camera whose pictures come up to 10 ms early or late,
    # captured at 30 pictures a second
    jitter = np.random.default_rng(31).uniform(-0.01, 0.01, 150)
    arrivals = [100 + index / 29.97 + jitter[index] for index in range(150)]
    sampler = _participant._Sampler(30)
    clock = SimpleNamespace(now=0.0)
    monkeypatch.setattr(video_in_sampler, "time", SimpleNamespace(time=lambda: clock.now))
    pipecat_sampler = video_in_sampler._VideoInSampler(30)

    # Act
    passed = 0
    pipecat_passed = 0
    for index, arrival in enumerate(arrivals):
        clock.now = arrival
        passed += sampler.sample(arrival, (index // 30, index % 30))[0]
        pipecat_passed += pipecat_sampler.sample()[0]

    # Assert: pipecat's sampler drops at most a picture or two of them
    assert pipecat_passed >= 148
    assert passed >= pipecat_passed


async def test_an_input_without_video_passes_image_requests_on_and_an_output_passes_on_messages_it_does_not_write():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    transport = MoqWasmTransport(session, MoqWasmParams(room=ROOM, data_out_tracks={"status": 60}))
    request = UserImageRequestFrame(user_id="camera")
    message = OutputTransportMessageFrame(message=MoqWasmTrackObject("command", b"stop"))
    urgent = OutputTransportMessageUrgentFrame(message=MoqWasmTrackObject("command", b"stop"))

    # Act
    _, up = await within(run_test(transport.input(), frames_to_send=[request], frames_to_send_direction=FrameDirection.UPSTREAM))
    down, _ = await within(run_test(transport.output(), frames_to_send=[message, urgent, SleepFrame(0.1)]))

    # Assert
    assert request in up
    assert down.count(message) == 1 and down.count(urgent) == 1
    await session.close()


async def test_an_urgent_message_going_upstream_reaches_the_output_upstream_that_writes_it():
    # Arrange: two transports' outputs in one pipeline, the first writing the transcript track
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    first = MoqWasmTransport(session, MoqWasmParams(room=ROOM, participant_id="a", data_out_tracks={"transcript": 60}))
    second = MoqWasmTransport(session, MoqWasmParams(room=ROOM, participant_id="b", data_out_tracks={"status": 60}))
    urgent = OutputTransportMessageUrgentFrame(message=MoqWasmTrackObject("transcript", b"hello"))

    # Act
    down, _ = await within(
        run_test(
            Pipeline([first.output(), second.output()]),
            frames_to_send=[urgent],
            frames_to_send_direction=FrameDirection.UPSTREAM,
        )
    )

    # Assert: the second output passed it on upstream, not back downstream
    assert native.writers["transcript"].objects == [(b"hello", True)]
    assert urgent not in down
    await session.close()


async def test_a_data_track_reaches_the_sink_as_messages_with_their_location():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["prompt"] = [[b"first", b"second", STAY_OPEN]]
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(data_tracks=["prompt"]), sink)

    # Act
    running = asyncio.create_task(receiver.run())
    await within(sink.received.wait())
    await asyncio.sleep(0.05)

    # Assert: a participant only data tracks are read from may publish no catalog
    assert ("subscribe_current_group", "catalog") not in native.calls
    assert [(m.participant_id, m.track, m.payload, m.location.object_id) for m in sink.messages] == [
        ("alice", "prompt", b"first", 0),
        ("alice", "prompt", b"second", 1),
    ]
    running.cancel()
    await within(asyncio.gather(running, return_exceptions=True))
    await session.close()


async def test_transport_messages_write_the_bots_tracks_and_event_timelines():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    room = Room(session, room_options(data_out_tracks={"status": 60}, event_timelines_out={"events": 60}), RecordingSink())
    await room.start("output")

    # Act
    written = [
        await room.write_message(MoqWasmTrackObject("status", b"ready")),
        await room.write_message(MoqWasmEventRecord("events", {"L": [1, 2], "data": {"answer": "yes"}})),
        await room.write_message(MoqWasmTrackObject("missing", b"lost")),
        await room.write_message({"type": "rtvi"}),
    ]

    # Assert: the event timeline opens with an empty record list
    assert written == [True, True, False, False]
    assert native.writers["status"].objects == [(b"ready", True)]
    assert [json.loads(payload) for payload, _ in native.writers["events"].objects] == [[], [{"L": [1, 2], "data": {"answer": "yes"}}]]
    await room.stop("output")
    await session.close()


async def test_a_transport_with_video_but_no_listed_source_follows_the_room_for_requests():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [[*video_frames(2), STAY_OPEN]]
    sink = RecordingSink()
    room = Room(session, room_options(receive=receive_options(video_in=True)), sink)
    await room.start("input")
    native.publish([*ROOM, "alice"])
    await asyncio.sleep(0.1)

    # Act
    await room.request_image(UserImageRequestFrame(user_id="alice"))
    await within(sink.received.wait())

    # Assert
    assert sink.pictures[0].request.user_id == "alice"
    await room.stop("input")
    await session.close()


async def test_the_room_reports_when_no_participant_came_for_the_empty_timeout():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    sink = RecordingSink()
    room = Room(session, room_options(audio_in=True, empty_timeout=0.05), sink)

    # Act
    await room.start("input")
    await asyncio.sleep(0.3)

    # Assert
    assert ("on_room_empty",) in sink.events
    await room.stop("input")
    await session.close()


async def test_a_room_receiving_nothing_reports_its_participants_and_when_it_is_empty_without_subscribing():
    # Arrange: the bot only speaks
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    sink = RecordingSink()
    room = Room(session, room_options(audio_out=True, empty_timeout=0.1), sink)
    await room.start("output")

    # Act
    native.publish([*ROOM, "alice"])
    await asyncio.sleep(0.05)
    native.publish([*ROOM, "alice"], active=False)
    await asyncio.sleep(0.3)

    # Assert
    assert [event for event in sink.events if event[0] != "on_connected"] == [
        ("on_client_connected", "alice"),
        ("on_client_disconnected", "alice"),
        ("on_room_empty",),
    ]
    assert not [call for call in native.calls if call[0] in ("subscribe", "subscribe_current_group")]
    await room.stop("output")
    await session.close()


async def test_the_room_does_not_report_empty_when_the_connection_is_lost():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    sink = RecordingSink()
    room = Room(session, room_options(audio_in=True, empty_timeout=0.2), sink)
    await room.start("input")

    # Act
    relay.sessions[0].drop()
    await asyncio.sleep(0.4)

    # Assert
    assert ("on_room_empty",) not in sink.events
    await room.stop("input")
    await session.close()


async def test_a_participant_inputs_pictures_carry_their_location_and_video_source():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [[*video_frames(2), STAY_OPEN]]
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(session, [*ROOM, "alice"]),
        MoqWasmInputParams(video_in_enabled=True, video_in_sources={"camera": VideoInSourceParams(framerate=30)}),
    )

    # Act
    down, _ = await within(run_test(participant_input, frames_to_send=[SleepFrame(0.5)]))

    # Assert
    pictures = [frame for frame in down if isinstance(frame, UserImageRawFrame)]
    assert pictures and pictures[0].transport_source == "camera"
    assert pictures[0].metadata["location"].object_id == 0
    await session.close()


async def test_the_bots_speech_reaches_another_room_through_a_relay(relay_url):
    # Arrange: the speaker keeps talking until the listener hears it
    room = ["anon", f"room-{uuid.uuid4().hex[:8]}"]
    speaker_session = await moq_wasm.connect(relay_url, verify_certificate=False)
    listener_session = await moq_wasm.connect(relay_url, verify_certificate=False)
    speaker = Room(speaker_session, room_options(room=room, participant_id="speaker", audio_out=True), RecordingSink())
    listener_sink = RecordingSink()
    listener = Room(listener_session, room_options(room=room, participant_id="listener", audio_in=True), listener_sink)
    await speaker.start("input")

    async def talk() -> None:
        while True:
            await speaker.write_audio(tone(0.1), SAMPLE_RATE)
            await asyncio.sleep(0.1)

    talking = asyncio.create_task(talk())

    # Act
    await listener.start_audio()
    await listener.start("input")
    await within(listener_sink.received.wait())

    # Assert
    assert listener_sink.audio[0][1:] == (SAMPLE_RATE, "speaker")
    talking.cancel()
    await within(asyncio.gather(talking, return_exceptions=True))
    for each in (listener, speaker):
        await each.stop("input")
    for session in (listener_session, speaker_session):
        await session.close()


def opus_packets() -> list[bytes]:
    """A second of speech, in 20 ms Opus packets."""
    return OpusEncoder().encode(tone(1.0), SAMPLE_RATE)


async def publish_tracks_until_cancelled(tracks: dict) -> None:
    """Writes 20 ms of speech to each audio track and a keyframe to each video track of `tracks`,
    each in a group of its own, every 20 ms, until cancelled."""
    packets = opus_packets()
    encoders = {name: VideoEncoder("H264", *SIZE, 30) for name in tracks if name.startswith("camera")}
    count = 0
    while True:
        for name, writer in tracks.items():
            if name in encoders:
                encoders[name].request_keyframe()
                payload = b"".join(encoders[name].encode(picture(count % 200), SIZE, "RGB"))
            else:
                payload = packets[count % len(packets)]
            await writer.write(payload, new_group=True)
        count += 1
        await asyncio.sleep(0.02)


def track_pair_catalog(pair: str) -> bytes:
    """A catalog listing alice's audio and camera tracks of `pair`."""
    audio = {**json.loads(AUDIO_CATALOG)["tracks"][0], "name": f"audio-{pair}"}
    video = {**json.loads(VIDEO_CATALOG)["tracks"][0], "name": f"camera-{pair}"}
    return catalog_payload(audio, video)


async def test_a_participant_switching_its_tracks_again_and_again_keeps_only_the_current_ones_media_through_a_relay(
    relay_url, kept_media
):
    # Arrange: alice publishes two pairs of audio and camera tracks, and her catalog lists the first
    room = ["anon", f"room-{uuid.uuid4().hex[:8]}"]
    alice_session = await moq_wasm.connect(relay_url, verify_certificate=False)
    bot_session = await moq_wasm.connect(relay_url, verify_certificate=False)
    alice = create_broadcast(alice_session, [*room, "alice"])
    catalog = alice.publish_track("catalog")
    tracks = {name: alice.publish_track(name) for name in ("audio-a", "camera-a", "audio-b", "camera-b")}
    await within(alice.__aenter__())
    await within(alice.published())
    await catalog.write(track_pair_catalog("a"), new_group=True)
    publishing = asyncio.create_task(publish_tracks_until_cancelled(tracks))
    sink = RecordingSink()
    receiver = ParticipantReceiver(
        RemoteBroadcast(bot_session, [*room, "alice"]), receive_options(audio_in=True, camera=True), sink
    )
    running = asyncio.create_task(receiver.run())

    async def received_pair(decoders: int) -> None:
        """Waits until the pair's audio has its decoder and its camera passed two pictures on."""
        await until(lambda: len(kept_media.decoders) >= decoders)
        pictures = len(sink.pictures)
        await until(lambda: len(sink.pictures) >= pictures + 2)

    await received_pair(1)

    # Act: her catalog switches to the other pair seven times
    for switch in range(1, 8):
        await catalog.write(track_pair_catalog("ab"[switch % 2]), new_group=True)
        await received_pair(switch + 1)
    publishing.cancel()
    await asyncio.gather(publishing, return_exceptions=True)

    # Assert: the current pair's decoder and last picture only
    assert kept_media.alive() == (1, 1)
    running.cancel()
    await within(asyncio.gather(running, return_exceptions=True))
    await within(alice.__aexit__(None, None, None))
    for session in (bot_session, alice_session):
        await session.close()


async def test_each_packet_of_speech_is_written_at_most_100_ms_ahead_of_the_clock():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    room = Room(session, room_options(audio_out=True), RecordingSink())
    await room.start("output")
    native = relay.sessions[0].media_broadcasts[0].sources[0]
    ahead_when_written: list[int] = []
    push_audio = native.push_audio

    def record_ahead(data: bytes, timestamp_micros: int, **level) -> None:
        ahead_when_written.append(timestamp_micros - _now_micros())
        push_audio(data, timestamp_micros, **level)

    native.push_audio = record_ahead

    # Act: half a second of speech in one chunk
    await room.write_audio(tone(0.5), SAMPLE_RATE)

    # Assert: each packet ends at most 100 ms (and a scheduling margin) ahead
    assert len(ahead_when_written) >= 24
    assert max(ahead_when_written) <= MAX_AUDIO_AHEAD_MICROS + OPUS_FRAME_MICROS + 15_000
    await room.stop("output")
    await session.close()


async def test_an_interruption_stops_the_speech_being_written_but_not_uninterruptible_speech():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    room = Room(session, room_options(audio_out=True), RecordingSink())
    await room.start("output")
    native = relay.sessions[0].media_broadcasts[0].sources[0]

    # Act
    speaking = asyncio.create_task(room.write_audio(tone(1.0), SAMPLE_RATE))
    await asyncio.sleep(0.15)
    room.interrupt_audio()
    await within(speaking)
    interrupted = len(native.audio_timestamps)
    announcing = asyncio.create_task(room.write_audio(tone(0.3), SAMPLE_RATE, interruptible=False))
    await asyncio.sleep(0.05)
    room.interrupt_audio()
    await within(announcing)

    # Assert: about 250 ms of the second of speech was written, and all of the uninterruptible one
    assert interrupted * OPUS_FRAME_MICROS <= 400_000
    assert len(native.audio_timestamps) - interrupted >= 14
    await room.stop("output")
    await session.close()


async def test_speech_an_interruption_cut_short_is_reported_as_not_written():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    room = Room(session, room_options(audio_out=True), RecordingSink())
    await room.start("output")
    native = relay.sessions[0].media_broadcasts[0].sources[0]

    # Act: the interruption comes while the speech waits for the clock
    speaking = asyncio.create_task(room.write_audio(tone(1.0), SAMPLE_RATE))
    await asyncio.sleep(0.15)
    room.interrupt_audio()
    written = await within(speaking)

    # Assert
    assert written is False
    assert 0 < len(native.audio_timestamps) * OPUS_FRAME_MICROS < 1_000_000
    await room.stop("output")
    await session.close()


async def test_media_the_media_source_dropped_once_the_session_ended_is_reported_as_not_written():
    # Arrange: the native source refuses the pushes once the session ended, which drops them
    relay = FakeRelay()
    session = await open_session(relay)
    room = Room(session, room_options(audio_out=True, video_out=True), RecordingSink())
    await room.start("output")
    native = relay.sessions[0].media_broadcasts[0].sources[0]
    native.failure = RuntimeError("media publishing stopped")
    await session.close()

    # Act
    speech = await room.write_audio(tone(0.04), SAMPLE_RATE)
    shown = await room.write_picture(picture(0), SIZE, "RGB")

    # Assert
    assert (speech, shown) == (False, False)
    await room.stop("output")


async def test_speech_too_short_for_a_packet_written_once_the_session_closed_is_reported_as_not_written():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    room = Room(session, room_options(audio_out=True), RecordingSink())
    await room.start("output")
    await session.close()

    # Act: 10 ms, short of an Opus packet of 20 ms
    written = await room.write_audio(tone(0.01), SAMPLE_RATE)

    # Assert
    assert written is False
    await room.stop("output")


def sine(seconds: float, sample_rate: int, start: float = 0.0, frequency: float = 1000.0) -> np.ndarray:
    """A sine from `start` seconds on, so consecutive pieces continue its phase."""
    times = start + np.arange(int(seconds * sample_rate)) / sample_rate
    return (np.sin(2 * np.pi * frequency * times) * 8000).astype(np.int16)


def frames_switching_channels() -> list[OutputAudioRawFrame]:
    """A 1 kHz tone at 16 kHz in 10 ms frames, every other one stereo."""
    frames = []
    for index in range(20):
        mono = sine(0.01, SAMPLE_RATE, start=index * 0.01)
        stereo = index % 2 == 0
        audio = np.repeat(mono, 2).tobytes() if stereo else mono.tobytes()
        frames.append(OutputAudioRawFrame(audio=audio, sample_rate=SAMPLE_RATE, num_channels=2 if stereo else 1))
    return frames


def frames_switching_rates() -> list[OutputAudioRawFrame]:
    """A 1 kHz tone in 20 ms frames, at 16 kHz then at 24 kHz."""
    return [
        OutputAudioRawFrame(audio=sine(0.02, rate, start=index * 0.02).tobytes(), sample_rate=rate, num_channels=1)
        for index, rate in enumerate([SAMPLE_RATE] * 5 + [24_000] * 5)
    ]


@pytest.mark.parametrize(
    "frames, output_rate", [(frames_switching_channels, SAMPLE_RATE), (frames_switching_rates, 48_000)]
)
async def test_speech_whose_channel_count_or_rate_changes_is_published_at_its_pitch(frames, output_rate):
    # Arrange: 200 ms of a 1 kHz tone, in frames whose channel count or rate changes
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM, audio_out_enabled=True, audio_out_sample_rate=output_rate, audio_out_end_silence_secs=0
    )
    transport = MoqWasmTransport(session, params)

    # Act
    _, up = await within(run_test(transport.output(), frames_to_send=[*frames(), TTSStoppedFrame(), SleepFrame(0.3)]))

    # Assert: all of it was written, and nearly all its energy is at 1 kHz
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    decoder = OpusDecoder(SAMPLE_RATE)
    decoded = np.frombuffer(b"".join(decoder.decode(call[1]) for call in source.calls if call[0] == "push_audio"), np.int16)
    spectrum = np.abs(np.fft.rfft(decoded.astype(np.float64))) ** 2
    frequencies = np.fft.rfftfreq(len(decoded), 1 / SAMPLE_RATE)
    assert not [frame for frame in up if isinstance(frame, ErrorFrame)]
    assert len(source.audio_timestamps) * OPUS_FRAME_MICROS >= 200_000
    assert spectrum[np.abs(frequencies - 1000) < 100].sum() > 0.9 * spectrum.sum()
    await session.close()


def stereo_tone(seconds: float, sample_rate: int) -> bytes:
    """A 440 Hz tone on the left and a 1 kHz tone on the right, interleaved."""
    return np.column_stack([sine(seconds, sample_rate, frequency=440.0), sine(seconds, sample_rate)]).tobytes()


def dominant_frequency(samples: np.ndarray, sample_rate: int) -> float:
    spectrum = np.abs(np.fft.rfft(samples.astype(np.float64)))
    return float(np.fft.rfftfreq(len(samples), 1 / sample_rate)[spectrum.argmax()])


async def test_a_stereo_output_publishes_stereo_opus_also_after_the_speech_ended():
    # Arrange: two utterances of a 440 Hz tone on the left and a 1 kHz tone on the right, the
    # second written by the encoder that replaced the first one's at its end
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM,
        audio_out_enabled=True,
        audio_out_sample_rate=48_000,
        audio_out_channels=2,
        audio_out_end_silence_secs=0,
    )
    transport = MoqWasmTransport(session, params)
    speech = OutputAudioRawFrame(audio=stereo_tone(0.2, 48_000), sample_rate=48_000, num_channels=2)
    utterance = [speech, TTSStoppedFrame()]

    # Act
    await within(run_test(transport.output(), frames_to_send=[*utterance, *utterance, SleepFrame(0.3)]))

    # Assert: the catalog says stereo, every packet is stereo (the TOC byte's s flag, RFC 6716
    # §3.1), and each channel kept its own tone
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    packets = [call[1] for call in source.calls if call[0] == "push_audio"]
    decoder = av.CodecContext.create("opus", "r")
    left, right = np.concatenate(
        [frame.to_ndarray() for packet in packets for frame in decoder.decode(av.Packet(packet))], axis=1
    )
    assert ("configure_audio", "opus", 48_000, 2) in source.calls
    assert len(packets) * OPUS_FRAME_MICROS >= 400_000
    assert all(packet[0] & 0x04 for packet in packets)
    assert abs(dominant_frequency(left, 48_000) - 440) < 20
    assert abs(dominant_frequency(right, 48_000) - 1000) < 20
    await session.close()


async def test_resampled_speech_keeps_its_frame_class_and_interruptible_flag():
    # Arrange: uninterruptible TTS speech at 24 kHz through a 16 kHz output
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM, audio_out_enabled=True, audio_out_sample_rate=SAMPLE_RATE, audio_out_end_silence_secs=0
    )
    transport = MoqWasmTransport(session, params)
    speech = TTSAudioRawFrame(audio=sine(0.1, 24_000).tobytes(), sample_rate=24_000, num_channels=1, context_id="reply")
    speech.interruptible = False

    # Act
    down, _ = await within(run_test(transport.output(), frames_to_send=[speech, TTSStoppedFrame(), SleepFrame(0.3)]))

    # Assert: pipecat passed on the chunks it wrote as TTS speech, uninterruptible,
    # at the output's rate
    chunks = [frame for frame in down if isinstance(frame, OutputAudioRawFrame)]
    assert sum(chunk.num_frames for chunk in chunks) >= 0.1 * SAMPLE_RATE
    assert all(isinstance(chunk, TTSAudioRawFrame) and not chunk.interruptible for chunk in chunks)
    assert {chunk.sample_rate for chunk in chunks} == {SAMPLE_RATE}
    await session.close()


async def test_audio_that_is_no_pcm_of_one_or_two_channels_is_reported_and_the_audio_after_it_is_written():
    # Arrange: three channels, then a tone
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM, audio_out_enabled=True, audio_out_sample_rate=SAMPLE_RATE, audio_out_end_silence_secs=0
    )
    transport = MoqWasmTransport(session, params)
    three_channels = OutputAudioRawFrame(audio=bytes(6 * 160), sample_rate=SAMPLE_RATE, num_channels=3)
    speech = OutputAudioRawFrame(audio=tone(0.1), sample_rate=SAMPLE_RATE, num_channels=1)

    # Act
    _, up = await within(run_test(transport.output(), frames_to_send=[three_channels, speech, SleepFrame(0.3)]))

    # Assert
    errors = [frame for frame in up if isinstance(frame, ErrorFrame)]
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert len(errors) == 1 and "cannot write audio" in errors[0].error and not errors[0].fatal
    assert len(source.audio_timestamps) * OPUS_FRAME_MICROS >= 80_000
    await session.close()


async def test_an_output_ending_gracefully_writes_the_last_partial_chunk_of_its_speech():
    # Arrange: 40 ms of silence, pipecat's chunk, then 10 ms of tone, less than a chunk, and no
    # silence after it
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM, audio_out_enabled=True, audio_out_sample_rate=SAMPLE_RATE, audio_out_end_silence_secs=0
    )
    transport = MoqWasmTransport(session, params)
    silence = bytes(int(0.04 * SAMPLE_RATE) * 2)
    speech = OutputAudioRawFrame(audio=silence + tone(0.01), sample_rate=SAMPLE_RATE, num_channels=1)

    # Act: run_test ends the pipeline with an EndFrame
    await within(run_test(transport.output(), frames_to_send=[speech]))

    # Assert: the tone was written
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    decoder = OpusDecoder(SAMPLE_RATE)
    decoded = b"".join(decoder.decode(call[1]) for call in source.calls if call[0] == "push_audio")
    assert len(source.audio_timestamps) * OPUS_FRAME_MICROS >= 50_000
    assert np.abs(np.frombuffer(decoded, np.int16)).max() > 2000
    await session.close()


def fail_media_publishing_soon(relay: FakeRelay) -> None:
    """Fails publishing the media source of the relay's first session in 100 ms."""

    def fail_publishing() -> None:
        relay.sessions[0].media_broadcasts[0].sources[0].failure = RuntimeError("media publishing stopped")

    asyncio.get_running_loop().call_later(0.1, fail_publishing)


async def test_an_output_whose_last_packet_cannot_be_written_reports_it_and_still_ends_the_pipeline():
    # Arrange: publishing the bot's audio fails while 10 ms of speech waits for the EndFrame
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM, audio_out_enabled=True, audio_out_sample_rate=SAMPLE_RATE, audio_out_end_silence_secs=0
    )
    transport = MoqWasmTransport(session, params)
    speech = OutputAudioRawFrame(audio=tone(0.01), sample_rate=SAMPLE_RATE, num_channels=1)

    fail_media_publishing_soon(relay)

    # Act: run_test ends the pipeline with an EndFrame and waits for it to come out
    _, up = await within(run_test(transport.output(), frames_to_send=[SleepFrame(0.2), speech]))

    # Assert: the failure went upstream as an ErrorFrame, and the room was left
    assert any(isinstance(frame, ErrorFrame) and "media publishing stopped" in frame.error for frame in up)
    assert ("unannounce", [*ROOM, "bot"]) in relay.sessions[0].calls
    await session.close()


@pytest.mark.parametrize("processor", ["input", "output"])
@pytest.mark.parametrize("terminal", [EndFrame, CancelFrame])
async def test_a_room_that_fails_to_be_withdrawn_is_reported_and_the_terminal_frame_still_goes_on(processor, terminal):
    # Arrange: the relay refuses to withdraw the transport's broadcast
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]

    async def refuse_unannounce(namespace: list[str]) -> None:
        raise RuntimeError("withdrawal refused")

    native.unannounce = refuse_unannounce
    transport = MoqWasmTransport(session, MoqWasmParams(room=ROOM))
    started = transport.input() if processor == "input" else transport.output()
    upstream, downstream = link_recorders(started)
    await within(start_alone(started))
    frame = terminal()

    # Act
    await within(started.process_frame(frame, FrameDirection.DOWNSTREAM))

    # Assert: the frame went on, so the pipeline ends, and the failure went upstream
    errors = [error for error in upstream.frames if isinstance(error, ErrorFrame)]
    assert frame in downstream.frames
    assert len(errors) == 1 and "cannot leave the room" in errors[0].error and "withdrawal refused" in errors[0].error
    assert started.is_usable
    await session.close()


async def test_an_input_only_worker_reports_the_room_it_fails_to_withdraw_at_an_end_frame():
    # Arrange: the relay refuses to withdraw the broadcast of a transport whose only processor is
    # its input
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]

    async def refuse_unannounce(namespace: list[str]) -> None:
        raise RuntimeError("withdrawal refused")

    native.unannounce = refuse_unannounce
    transport = MoqWasmTransport(session, MoqWasmParams(room=ROOM, audio_in_enabled=True))
    worker = PipelineWorker(Pipeline([transport.input()]), cancel_on_idle_timeout=False)
    errors: list[ErrorFrame] = []

    @worker.event_handler("on_pipeline_error")
    async def on_pipeline_error(worker: PipelineWorker, frame: ErrorFrame) -> None:
        errors.append(frame)

    @worker.event_handler("on_pipeline_started")
    async def on_pipeline_started(worker: PipelineWorker, frame: StartFrame) -> None:
        await worker.queue_frame(EndFrame())

    runner = WorkerRunner(handle_sigint=False)
    await runner.add_workers(worker)

    # Act
    await within(runner.run())

    # Assert
    assert [error.error for error in errors] == ["cannot leave the room: withdrawal refused"]
    await session.close()


async def test_a_message_that_cannot_be_written_is_reported_and_the_messages_after_it_are_written():
    # Arrange: a record JSON cannot carry, then an object of a data track
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(room=ROOM, data_out_tracks={"status": 60}, event_timelines_out={"events": 60})
    transport = MoqWasmTransport(session, params)
    unserializable = OutputTransportMessageFrame(message=MoqWasmEventRecord("events", {"data": object()}))
    status = OutputTransportMessageFrame(message=MoqWasmTrackObject("status", b"ready"))

    # Act
    _, up = await within(run_test(transport.output(), frames_to_send=[unserializable, status, SleepFrame(0.1)]))

    # Assert
    errors = [frame for frame in up if isinstance(frame, ErrorFrame)]
    assert len(errors) == 1 and "cannot write the message" in errors[0].error
    assert relay.sessions[0].writers["status"].objects == [(b"ready", True)]
    await session.close()


async def test_a_picture_that_cannot_be_encoded_is_reported_and_the_pictures_after_it_are_written():
    # Arrange: a grayscale picture, which the encoder does not take, then an RGB one
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM,
        video_out_enabled=True,
        video_out_is_live=True,
        video_out_width=SIZE[0],
        video_out_height=SIZE[1],
    )
    transport = MoqWasmTransport(session, params)
    grayscale = OutputImageRawFrame(image=bytes(SIZE[0] * SIZE[1]), size=SIZE, format="L")
    rgb = OutputImageRawFrame(image=picture(0), size=SIZE, format="RGB")

    # Act
    _, up = await within(run_test(transport.output(), frames_to_send=[grayscale, rgb, SleepFrame(0.2)]))

    # Assert
    errors = [frame for frame in up if isinstance(frame, ErrorFrame)]
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert len(errors) == 1 and "cannot write a picture" in errors[0].error
    assert [call[0] for call in source.calls].count("push_video") == 1
    await session.close()


BGR_PICTURE = bytes(320 * 240 * 3)


@pytest.mark.parametrize(
    "malformed, live",
    [
        (OutputImageRawFrame(image=bytes(3), size=(320, 240), format="RGB"), True),
        (OutputImageRawFrame(image=BGR_PICTURE, size=(320, 240), format="BGR"), True),
        (SpriteFrame(images=[OutputImageRawFrame(image=bytes(3), size=(320, 240), format="RGB")]), False),
        (SpriteFrame(images=[]), False),
        (OutputImageRawFrame(image=bytes(320 * 240 * 3), size=(320.0, 240), format="RGB"), False),
        (AssistantImageRawFrame(image=memoryview(bytes(320 * 240 * 3 * 2))[::2], size=(320, 240), format="RGB"), False),
        (OutputImageRawFrame(image="\x00" * (320 * 240 * 3), size=(320, 240), format="RGB"), True),
    ],
)
async def test_a_picture_pipecat_cannot_resize_is_dropped_before_its_video_sender_and_the_pictures_after_it_are_written(
    malformed, live
):
    # Arrange: a picture pipecat's video sender would fail to resize to the output's size, then a
    # valid one
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM,
        video_out_enabled=True,
        video_out_is_live=live,
        video_out_width=SIZE[0],
        video_out_height=SIZE[1],
    )
    transport = MoqWasmTransport(session, params)
    rgb = OutputImageRawFrame(image=picture(0), size=SIZE, format="RGB")

    # Act
    _, up = await within(run_test(transport.output(), frames_to_send=[malformed, SleepFrame(0.1), rgb, SleepFrame(0.2)]))

    # Assert
    errors = [frame for frame in up if isinstance(frame, ErrorFrame)]
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert len(errors) == 1 and "cannot write a picture" in errors[0].error and not errors[0].fatal
    assert "push_video" in [call[0] for call in source.calls]
    await session.close()


@pytest.mark.parametrize("color_format", [None, ""])
@pytest.mark.parametrize("live", [True, False])
async def test_a_picture_without_a_color_format_is_resized_and_written_as_rgb(color_format, live):
    # Arrange: an RGB picture half the output's size, without its color format
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM,
        video_out_enabled=True,
        video_out_is_live=live,
        video_out_width=SIZE[0],
        video_out_height=SIZE[1],
    )
    transport = MoqWasmTransport(session, params)
    half = (SIZE[0] // 2, SIZE[1] // 2)
    unlabelled = OutputImageRawFrame(image=bytes(half[0] * half[1] * 3), size=half, format=color_format)

    # Act
    _, up = await within(run_test(transport.output(), frames_to_send=[unlabelled, SleepFrame(0.2)]))

    # Assert
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert not [frame for frame in up if isinstance(frame, ErrorFrame)]
    assert "push_video" in [call[0] for call in source.calls]
    await session.close()


async def test_media_whose_publishing_failed_is_reported_once_and_dropped_after_it():
    # Arrange: publishing the bot's media fails before it speaks and shows pictures; the output
    # writes a data track too
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM,
        audio_out_enabled=True,
        audio_out_sample_rate=SAMPLE_RATE,
        audio_out_end_silence_secs=0,
        video_out_enabled=True,
        video_out_is_live=True,
        video_out_width=SIZE[0],
        video_out_height=SIZE[1],
        data_out_tracks={"status": 60},
    )
    transport = MoqWasmTransport(session, params)
    output = transport.output()

    fail_media_publishing_soon(relay)
    speech = [OutputAudioRawFrame(audio=tone(0.1), sample_rate=SAMPLE_RATE, num_channels=1) for _ in range(2)]
    rgb = OutputImageRawFrame(image=picture(0), size=SIZE, format="RGB")

    # Act
    _, up = await within(run_test(output, frames_to_send=[SleepFrame(0.2), *speech, SleepFrame(0.3), rgb, SleepFrame(0.1)]))

    # Assert: only the first failed push was reported, and the output, which still writes its data
    # track, stays usable
    errors = [frame for frame in up if isinstance(frame, ErrorFrame)]
    assert len(errors) == 1 and "media publishing stopped" in errors[0].error
    assert output.is_usable
    await session.close()


async def test_an_output_whose_media_publishing_failed_with_nothing_else_to_write_ends_the_pipeline():
    # Arrange: an output that publishes speech alone, in a worker that cancels its pipeline once a
    # processor is unusable
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    params = MoqWasmParams(
        room=ROOM, audio_out_enabled=True, audio_out_sample_rate=SAMPLE_RATE, audio_out_end_silence_secs=0
    )
    output = MoqWasmTransport(session, params).output()
    worker = PipelineWorker(
        Pipeline([output]),
        cancel_on_idle_timeout=False,
        processor_unusable_policy=ProcessorUnusablePolicy.CANCEL,
    )
    errors: list[ErrorFrame] = []
    worker.event_handler("on_pipeline_error")(lambda _, frame: errors.append(frame))
    runner = WorkerRunner(handle_sigint=False)
    await runner.add_workers(worker)
    running = asyncio.create_task(runner.run())
    await until(lambda: ("watch_namespaces", ROOM) in native.calls)
    await asyncio.sleep(0.1)
    native.media_broadcasts[0].sources[0].failure = RuntimeError("media publishing stopped")

    # Act
    await worker.queue_frames([OutputAudioRawFrame(audio=tone(0.1), sample_rate=SAMPLE_RATE, num_channels=1)])

    # Assert: the failure left the output unusable, and the worker cancelled the pipeline
    try:
        await within(asyncio.shield(running))
    finally:
        running.cancel()
        await asyncio.gather(running, return_exceptions=True)
    assert len(errors) == 1 and "media publishing stopped" in errors[0].error
    assert not output.is_usable
    await session.close()


async def test_speech_dropped_after_its_media_source_failed_is_not_passed_on_as_sent():
    # Arrange: publishing the bot's audio fails before it speaks
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM, audio_out_enabled=True, audio_out_sample_rate=SAMPLE_RATE, audio_out_end_silence_secs=0
    )
    transport = MoqWasmTransport(session, params)

    fail_media_publishing_soon(relay)
    speech = [OutputAudioRawFrame(audio=tone(0.1), sample_rate=SAMPLE_RATE, num_channels=1) for _ in range(2)]

    # Act
    down, up = await within(run_test(transport.output(), frames_to_send=[SleepFrame(0.2), *speech, SleepFrame(0.3)]))

    # Assert: the first chunk's failure was reported once, and no chunk went on as sent
    errors = [frame for frame in up if isinstance(frame, ErrorFrame)]
    assert len(errors) == 1 and "media publishing stopped" in errors[0].error
    assert not [frame for frame in down if isinstance(frame, OutputAudioRawFrame)]
    await session.close()


async def test_a_finished_utterance_writes_its_ending_before_a_later_interruption():
    # Arrange: one chunk of speech, 20 ms of silence then 20 ms of tone, ends with a
    # TTSStoppedFrame; an interruption comes once the output wrote it
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM, audio_out_enabled=True, audio_out_sample_rate=SAMPLE_RATE, audio_out_end_silence_secs=0
    )
    transport = MoqWasmTransport(session, params)
    silence = bytes(int(0.02 * SAMPLE_RATE) * 2)
    speech = OutputAudioRawFrame(audio=silence + tone(0.02), sample_rate=SAMPLE_RATE, num_channels=1)

    # Act: run_test ends the pipeline with an EndFrame
    frames = [speech, TTSStoppedFrame(), SleepFrame(0.2), InterruptionFrame()]
    await within(run_test(transport.output(), frames_to_send=frames))

    # Assert: the tone was written
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    decoder = OpusDecoder(SAMPLE_RATE)
    decoded = b"".join(decoder.decode(call[1]) for call in source.calls if call[0] == "push_audio")
    assert np.abs(np.frombuffer(decoded, np.int16)).max() > 2000
    await session.close()


async def test_an_interruption_between_uninterruptible_chunks_keeps_what_the_encoder_holds_of_them():
    # Arrange: two 15 ms chunks of uninterruptible speech make one packet together
    relay = FakeRelay()
    session = await open_session(relay)
    room = Room(session, room_options(audio_out=True), RecordingSink())
    await room.start("output")
    native = relay.sessions[0].media_broadcasts[0].sources[0]
    reference = OpusEncoder()
    expected = [*reference.encode(tone(0.015), SAMPLE_RATE), *reference.encode(tone(0.015), SAMPLE_RATE)]

    # Act
    await room.write_audio(tone(0.015), SAMPLE_RATE, interruptible=False)
    room.interrupt_audio()
    await room.write_audio(tone(0.015), SAMPLE_RATE, interruptible=False)

    # Assert
    assert len(expected) == 1
    assert len(native.audio_timestamps) == len(expected)
    await room.stop("output")
    await session.close()


async def test_an_interruption_after_interruptible_speech_keeps_the_uninterruptible_speech_before_it():
    # Arrange: 30 ms of uninterruptible speech, a packet and a half, then 5 ms of
    # interruptible speech
    relay = FakeRelay()
    session = await open_session(relay)
    room = Room(session, room_options(audio_out=True), RecordingSink())
    await room.start("output")
    native = relay.sessions[0].media_broadcasts[0].sources[0]
    await room.write_audio(tone(0.03), SAMPLE_RATE, interruptible=False)
    await room.write_audio(tone(0.005), SAMPLE_RATE)

    # Act
    room.interrupt_audio()

    # Assert: all of the uninterruptible speech was written
    assert len(native.audio_timestamps) * OPUS_FRAME_MICROS >= 30_000
    await room.stop("output")
    await session.close()


async def test_a_decode_read_that_timed_out_leaves_the_decoding_to_the_next_read():
    # Arrange: the group's second object is held back until the gate opens, read from a subscription,
    # whose reads a timeout does not end
    encoder = VideoEncoder("H264", *SIZE, 30)
    payloads = [b"".join(encoder.encode(picture(value), SIZE, "RGB")) for value in (0, 10, 20)]
    gate = asyncio.Event()
    objects = [video_object(0, 0, payloads[0]), gate, video_object(0, 1, payloads[1]), video_object(0, 2, payloads[2])]
    session = await open_session(FakeRelay())
    pictures = decode_pictures(Subscription(FakeSubscription(objects), session), "avc1.42e01f")
    first = await within(anext(pictures))
    with pytest.raises(TimeoutError):
        await asyncio.wait_for(anext(pictures), 0.05)

    # Act
    gate.set()
    rest = [await within(anext(pictures)) for _ in range(2)]

    # Assert
    assert [(d.location.group_id, d.location.object_id) for d in (first, *rest)] == [(0, 0), (0, 1), (0, 2)]
    await pictures.aclose()
    await session.close()


async def test_decode_pictures_reads_the_objects_in_the_readers_own_task_and_context():
    # Arrange: the objects' generator sets a context variable and a timeout across its yields
    encoder = VideoEncoder("H264", *SIZE, 30)
    payloads = [b"".join(encoder.encode(picture(value), SIZE, "RGB")) for value in (0, 10, 20)]
    variable: contextvars.ContextVar[str] = contextvars.ContextVar("variable")

    async def objects():
        token = variable.set("objects")
        async with asyncio.timeout(TIMEOUT_SECONDS):
            for object_id, payload in enumerate(payloads):
                yield video_object(0, object_id, payload)
        variable.reset(token)

    # Act
    decoded = [d async for d in decode_pictures(objects(), "avc1.42e01f")]

    # Assert
    assert [(d.location.group_id, d.location.object_id) for d in decoded] == [(0, 0), (0, 1), (0, 2)]


async def test_closing_a_decoding_leaves_its_objects_to_the_caller():
    # Arrange: a read of the decoding waits on the caller's objects, whose second one is held back
    # until the gate opens
    encoder = VideoEncoder("H264", *SIZE, 30)
    payloads = [b"".join(encoder.encode(picture(value), SIZE, "RGB")) for value in (0, 10, 20)]
    gate = asyncio.Event()

    async def objects():
        yield video_object(0, 0, payloads[0])
        await gate.wait()
        yield video_object(0, 1, payloads[1])
        yield video_object(0, 2, payloads[2])

    source = objects()
    pictures = decode_pictures(source, "avc1.42e01f")
    await within(anext(pictures))
    reading = asyncio.ensure_future(anext(pictures))
    await asyncio.sleep(0.01)

    # Act
    await within(pictures.aclose())
    gate.set()

    # Assert: the read in flight ends, and the caller reads its objects on
    with pytest.raises(StopAsyncIteration):
        await within(reading)
    assert (await within(anext(source))).object_id == 2


async def test_a_decoding_closed_while_an_object_without_a_picture_is_decoded_reads_no_more_objects(monkeypatch):
    # Arrange: the worker decodes the first object, which gives no picture, until the gate opens
    gate = threading.Event()
    entered = threading.Event()

    def take(self, video_object) -> list:
        entered.set()
        gate.wait(TIMEOUT_SECONDS)
        return []

    monkeypatch.setattr(codecs._PictureDecoder, "take", take)
    reads = 0

    async def objects():
        nonlocal reads
        for object_id in range(2):
            reads += 1
            yield video_object(0, object_id, b"")

    pictures = decode_pictures(objects(), "avc1.42e01f")
    reading = asyncio.ensure_future(anext(pictures))
    await within(asyncio.to_thread(entered.wait, TIMEOUT_SECONDS))

    # Act
    await within(pictures.aclose())
    gate.set()

    # Assert
    with pytest.raises(StopAsyncIteration):
        await within(reading)
    assert reads == 1


@pytest.fixture
def video_decoders(monkeypatch) -> list[weakref.ref]:
    """Weak references to every `VideoDecoder` `moq_wasm.codecs` makes in the test."""
    decoders = []

    class TrackedDecoder(VideoDecoder):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            decoders.append(weakref.ref(self))

    monkeypatch.setattr(codecs, "VideoDecoder", TrackedDecoder)
    return decoders


async def test_a_closed_decoding_keeps_no_decoder_once_the_decode_in_flight_finished(monkeypatch, video_decoders):
    # Arrange: a group left open, whose second object the worker decodes until the gate opens, as
    # the decoding is closed
    gate = threading.Event()
    entered = threading.Event()
    take = codecs._PictureDecoder.take

    def gated(self, video_object) -> list:
        if video_object.object_id == 1:
            entered.set()
            gate.wait(TIMEOUT_SECONDS)
        return take(self, video_object)

    monkeypatch.setattr(codecs._PictureDecoder, "take", gated)
    encoder = VideoEncoder("H264", *SIZE, 30)
    payloads = [b"".join(encoder.encode(picture(value), SIZE, "RGB")) for value in (0, 10, 20)]

    async def objects():
        for object_id, payload in enumerate(payloads):
            yield video_object(0, object_id, payload)

    source = objects()
    pictures = decode_pictures(source, "avc1.42e01f")
    await within(anext(pictures))
    reading = asyncio.ensure_future(anext(pictures))
    await within(asyncio.to_thread(entered.wait, TIMEOUT_SECONDS))

    # Act
    await within(pictures.aclose())
    gate.set()
    with pytest.raises(StopAsyncIteration):
        await within(reading)
    gc.collect()

    # Assert: the decoder is freed, while the caller still holds the closed decoding, and the
    # caller reads its objects on
    assert [decoder for decoder in video_decoders if decoder() is not None] == []
    assert (await within(anext(source))).object_id == 2


async def test_a_closed_decoding_whose_read_waits_on_the_objects_keeps_no_decoder(video_decoders):
    # Arrange: a group left open by its keyframe, then an object that does not exist, which gives
    # no picture, so the read waits on the caller's next object until the gate opens
    encoder = VideoEncoder("H264", *SIZE, 30)
    keyframe = b"".join(encoder.encode(picture(0), SIZE, "RGB"))
    gate = asyncio.Event()
    waiting = asyncio.Event()

    async def objects():
        yield video_object(0, 0, keyframe)
        # The Object Status of an object that does not exist (draft-ietf-moq-transport-14 §10.2.1.1).
        yield video_object(0, 1, b"", status=1)
        waiting.set()
        await gate.wait()
        yield video_object(0, 2, keyframe)

    pictures = decode_pictures(objects(), "avc1.42e01f")
    await within(anext(pictures))
    reading = asyncio.ensure_future(anext(pictures))
    await within(waiting.wait())

    # Act
    await within(pictures.aclose())
    gc.collect()

    # Assert: the decoder is freed while the read still waits on the objects
    assert [decoder for decoder in video_decoders if decoder() is not None] == []
    gate.set()
    with pytest.raises(StopAsyncIteration):
        await within(reading)


async def test_a_closed_decoding_keeps_no_decoder_while_a_worker_still_holds_what_it_ran(video_decoders):
    # Arrange: workers that keep every callable they ran and its future
    ran = []

    class RetainingExecutor(ThreadPoolExecutor):
        def submit(self, fn, /, *args, **kwargs):
            future = super().submit(fn, *args, **kwargs)
            ran.append((fn, future))
            return future

    asyncio.get_running_loop().set_default_executor(RetainingExecutor())
    encoder = VideoEncoder("H264", *SIZE, 30)
    keyframe = b"".join(encoder.encode(picture(0), SIZE, "RGB"))

    async def objects():
        yield video_object(0, 0, keyframe)

    pictures = decode_pictures(objects(), "avc1.42e01f")
    await within(anext(pictures))

    # Act
    await within(pictures.aclose())
    gc.collect()

    # Assert
    assert ran
    assert [decoder for decoder in video_decoders if decoder() is not None] == []


async def test_length_prefixed_h264_decodes_with_the_video_config_of_the_first_keyframe():
    # Arrange: two groups of avcC H.264; only the first object carries the Video Config
    encoder = VideoEncoder("H264", *SIZE, 30)
    groups = []
    for values in ((0, 10, 20), (30, 40, 50)):
        encoder.request_keyframe()
        groups.append([length_prefixed(b"".join(encoder.encode(picture(value), SIZE, "RGB"))) for value in values])
    sps, pps, _ = groups[0][0]
    config = avc_decoder_configuration(sps, pps)

    async def objects():
        for group_id, group in enumerate(groups):
            for object_id, (_, _, payload) in enumerate(group):
                headers = [(VIDEO_CONFIG_ID, config)] if (group_id, object_id) == (0, 0) else []
                yield video_object(group_id, object_id, payload, extension_headers=headers)

    # Act
    decoded = [d async for d in decode_pictures(objects(), "avc1.42e01f")]

    # Assert
    assert [(d.location.group_id, d.location.object_id) for d in decoded] == [(g, o) for g in (0, 1) for o in range(3)]


async def test_length_prefixed_h264_joined_after_its_only_video_config_decodes_to_nothing():
    # Arrange: two groups of avcC H.264, only the first object carrying the Video Config, joined at
    # the first group's second object
    encoder = VideoEncoder("H264", *SIZE, 30)
    groups = []
    for values in ((0, 10, 20), (30, 40, 50)):
        encoder.request_keyframe()
        groups.append([length_prefixed(b"".join(encoder.encode(picture(value), SIZE, "RGB")))[2] for value in values])

    async def objects():
        for group_id, group in enumerate(groups):
            for object_id, payload in enumerate(group):
                if (group_id, object_id) != (0, 0):
                    yield video_object(group_id, object_id, payload)

    # Act
    decoded = [d async for d in decode_pictures(objects(), "avc1.42e01f")]

    # Assert: without the Video Config, its keyframe group is skipped rather than raising
    assert decoded == []


async def test_length_prefixed_h264_joined_past_its_video_config_is_skipped_with_one_warning_until_a_group_brings_one(caplog):
    # Arrange: joined at the second object of group 0, whose first alone carries the Video Config;
    # group 1 carries none, and group 2 carries one again
    encoder = VideoEncoder("H264", *SIZE, 30)
    _, group_0 = length_prefixed_group(encoder, 3)
    _, group_1 = length_prefixed_group(encoder, 2)
    config, group_2 = length_prefixed_group(encoder, 2)

    async def objects():
        yield video_object(0, 1, group_0[1])
        yield video_object(0, 2, group_0[2])
        for object_id, payload in enumerate(group_1):
            yield video_object(1, object_id, payload)
        yield video_object(2, 0, group_2[0], extension_headers=[(VIDEO_CONFIG_ID, config)])
        yield video_object(2, 1, group_2[1])

    # Act
    with caplog.at_level("WARNING", logger="moq_wasm"):
        decoded = [d async for d in decode_pictures(objects(), "avc1.42e01f")]

    # Assert: the warning comes from group 1, joined at its keyframe without parameter sets
    assert [(d.location.group_id, d.location.object_id) for d in decoded] == [(2, 0), (2, 1)]
    assert len([record for record in caplog.records if "Video Config" in record.getMessage()]) == 1


async def test_length_prefixed_h264_with_its_parameter_sets_in_band_decodes_without_a_video_config(caplog):
    # Arrange: an avc3 group whose keyframe carries its SPS and PPS, each NAL unit after a 4-byte
    # length, and no object a Video Config
    encoder = VideoEncoder("H264", *SIZE, 30)
    encoder.request_keyframe()
    payloads = [
        b"".join(len(unit).to_bytes(4, "big") + unit for unit in nal_units(b"".join(encoder.encode(picture(value), SIZE, "RGB"))))
        for value in (0, 10, 20, 30, 40)
    ]

    async def objects():
        for object_id, payload in enumerate(payloads):
            yield video_object(0, object_id, payload)

    # Act
    with caplog.at_level("WARNING", logger="moq_wasm"):
        decoded = [d async for d in decode_pictures(objects(), "avc3.42e01f")]

    # Assert
    assert [(d.location.group_id, d.location.object_id) for d in decoded] == [(0, object_id) for object_id in range(5)]
    assert not [record for record in caplog.records if "Video Config" in record.getMessage()]


async def test_annex_b_h264_with_3_byte_start_codes_decodes_without_a_video_config():
    # Arrange: a group whose every NAL unit follows a 3-byte start code
    encoder = VideoEncoder("H264", *SIZE, 30)
    encoder.request_keyframe()
    payloads = [
        b"".join(b"\x00\x00\x01" + unit for unit in nal_units(b"".join(encoder.encode(picture(value), SIZE, "RGB"))))
        for value in (0, 10, 20)
    ]

    async def objects():
        for object_id, payload in enumerate(payloads):
            yield video_object(0, object_id, payload)

    # Act
    decoded = [d async for d in decode_pictures(objects(), "avc1.42e01f")]

    # Assert
    assert [(d.location.group_id, d.location.object_id) for d in decoded] == [(0, 0), (0, 1), (0, 2)]


async def test_an_annex_b_object_whose_3_byte_start_code_reads_as_a_fitting_length_decodes_as_annex_b():
    # Arrange: a group of 3-byte start codes whose second object, a P-slice padded with trailing
    # zeros to 325 bytes, starts `00 00 01 41`, a 4-byte length of 321 that covers it exactly
    encoder = VideoEncoder("H264", *SIZE, 30)
    encoder.request_keyframe()
    keyframe, p_slice = (nal_units(b"".join(encoder.encode(picture(value), SIZE, "RGB"))) for value in (0, 10))
    padded = (b"\x00\x00\x01" + p_slice[0]).ljust(325, b"\x00")
    assert p_slice[0][0] == 0x41 and codecs._length_prefixed_units(padded) is not None

    async def objects():
        yield video_object(0, 0, b"".join(b"\x00\x00\x01" + unit for unit in keyframe))
        yield video_object(0, 1, padded)

    # Act
    decoded = [d async for d in decode_pictures(objects(), "avc1.42e01f")]

    # Assert
    assert [(d.location.group_id, d.location.object_id) for d in decoded] == [(0, 0), (0, 1)]


async def test_length_prefixed_h264_without_a_video_config_decodes_with_the_one_given():
    # Arrange: two groups of avcC H.264 joined past the only object carrying the Video Config,
    # whose record the reader knows beforehand, as from the catalog
    encoder = VideoEncoder("H264", *SIZE, 30)
    config, group_0 = length_prefixed_group(encoder, 3)
    _, group_1 = length_prefixed_group(encoder, 3)

    async def objects():
        for group_id, group in enumerate((group_0, group_1)):
            for object_id, payload in enumerate(group):
                if (group_id, object_id) != (0, 0):
                    yield video_object(group_id, object_id, payload)

    # Act
    decoded = [d async for d in decode_pictures(objects(), "avc1.42e01f", config=config)]

    # Assert
    assert [(d.location.group_id, d.location.object_id) for d in decoded] == [(1, 0), (1, 1), (1, 2)]


async def test_following_pictures_decodes_length_prefixed_h264_with_the_video_config_given():
    # Arrange: no object of the avcC H.264 track carries the Video Config
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    encoder = VideoEncoder("H264", *SIZE, 30)
    config, group = length_prefixed_group(encoder, 2)
    native.tracks["camera"] = [[video_object(4, object_id, payload) for object_id, payload in enumerate(group)] + [STAY_OPEN]]
    remote = RemoteBroadcast(session, [*ROOM, "alice"])

    # Act
    async with contextlib.aclosing(follow_pictures(remote, "camera", "avc1.42e01f", config=config)) as pictures:
        decoded = [await within(anext(pictures)) for _ in range(2)]

    # Assert
    assert [(d.location.group_id, d.location.object_id) for d in decoded] == [(4, 0), (4, 1)]
    await session.close()


async def test_following_pictures_decodes_a_later_subscription_with_the_video_config_an_earlier_one_brought():
    # Arrange: only the first object of the avcC H.264 track carries the Video Config; the first
    # subscription brings it and ends, and the next one joins at a group without one
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    encoder = VideoEncoder("H264", *SIZE, 30)
    config, group_0 = length_prefixed_group(encoder, 1)
    _, group_1 = length_prefixed_group(encoder, 2)
    first = [video_object(0, 0, group_0[0], extension_headers=[(VIDEO_CONFIG_ID, config)])]
    second = [video_object(1, object_id, payload) for object_id, payload in enumerate(group_1)]
    native.tracks["camera"] = [first, [*second, STAY_OPEN]]
    remote = RemoteBroadcast(session, [*ROOM, "alice"])

    # Act
    async with contextlib.aclosing(follow_pictures(remote, "camera", "avc1.42e01f")) as pictures:
        decoded = [await within(anext(pictures)) for _ in range(3)]

    # Assert
    assert [(d.location.group_id, d.location.object_id) for d in decoded] == [(0, 0), (1, 0), (1, 1)]
    await session.close()


async def test_following_pictures_starts_at_the_group_whose_first_object_carries_the_only_video_config():
    # Arrange: the relay holds the first object of the group, which alone carries the Video Config of
    # the avcC H.264 track; live objects come from the group's second on
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    encoder = VideoEncoder("H264", *SIZE, 30)
    units = [length_prefixed(b"".join(encoder.encode(picture(value), SIZE, "RGB"))) for value in (0, 10, 20)]
    sps, pps, _ = units[0]
    config = avc_decoder_configuration(sps, pps)
    native.current_groups["camera"] = [video_object(7, 0, units[0][2], extension_headers=[(VIDEO_CONFIG_ID, config)])]
    native.tracks["camera"] = [[video_object(7, 1, units[1][2]), video_object(7, 2, units[2][2]), STAY_OPEN]]
    remote = RemoteBroadcast(session, [*ROOM, "alice"])

    # Act
    async with contextlib.aclosing(follow_pictures(remote, "camera", "avc1.42e01f")) as pictures:
        decoded = [await within(anext(pictures)) for _ in range(2)]

    # Assert
    assert [(d.location.group_id, d.location.object_id) for d in decoded] == [(7, 0), (7, 1)]
    await session.close()


async def test_closed_followed_pictures_keep_no_decoder_but_the_video_config_they_learned(video_decoders):
    # Arrange: the group stays open, so its decoder lives until the pictures are closed
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    config, group = length_prefixed_group(VideoEncoder("H264", *SIZE, 30), 2)
    native.tracks["camera"] = [
        [video_object(0, 0, group[0], extension_headers=[(VIDEO_CONFIG_ID, config)]), video_object(0, 1, group[1]), STAY_OPEN]
    ]
    pictures = follow_pictures(RemoteBroadcast(session, [*ROOM, "alice"]), "camera", "avc1.42e01f")
    for _ in range(2):
        await within(anext(pictures))

    # Act
    await within(pictures.aclose())
    gc.collect()

    # Assert
    assert [decoder for decoder in video_decoders if decoder() is not None] == []
    assert pictures.config == config
    await session.close()


HALF_SIZE = (SIZE[0] // 2, SIZE[1] // 2)


def length_prefixed_group(encoder: VideoEncoder, count: int) -> tuple[bytes, list[bytes]]:
    """The avcC record and the length-prefixed objects of a group of `count` pictures."""
    encoder.request_keyframe()
    units = [length_prefixed(b"".join(encoder.encode(picture(value * 10), SIZE, "RGB"))) for value in range(count)]
    sps, pps, _ = units[0]
    return avc_decoder_configuration(sps, pps), [payload for _, _, payload in units]


async def test_each_group_decodes_with_the_video_config_of_its_own_or_of_the_group_before_it():
    # Arrange: group 0 is full size; groups 1 and 2 are half size, and only group 1 carries its config;
    # group 0 arrives late, after group 1 opened
    full = VideoEncoder("H264", *SIZE, 30)
    half = VideoEncoder("H264", *HALF_SIZE, 30)
    full_config, group_0 = length_prefixed_group(full, 2)
    half_config, group_1 = length_prefixed_group(half, 2)
    _, group_2 = length_prefixed_group(half, 2)

    async def objects():
        yield video_object(1, 0, group_1[0], extension_headers=[(VIDEO_CONFIG_ID, half_config)])
        yield video_object(0, 0, group_0[0], extension_headers=[(VIDEO_CONFIG_ID, full_config)])
        yield video_object(0, 1, group_0[1])
        yield video_object(1, 1, group_1[1])
        for object_id, payload in enumerate(group_2):
            yield video_object(2, object_id, payload)

    # Act
    decoded = [d async for d in decode_pictures(objects(), "avc1.42e01f")]

    # Assert
    sizes = {(d.location.group_id, d.location.object_id): (d.picture.width, d.picture.height) for d in decoded}
    assert sizes == {(0, 0): SIZE, (0, 1): SIZE, (1, 0): HALF_SIZE, (1, 1): HALF_SIZE, (2, 0): HALF_SIZE, (2, 1): HALF_SIZE}


async def test_a_group_without_a_video_config_opened_before_the_group_before_it_decodes_with_that_ones():
    # Arrange: group 0 is full size; groups 1 and 2 are half size, and only group 1 carries its config;
    # group 2 opens before group 1
    full = VideoEncoder("H264", *SIZE, 30)
    half = VideoEncoder("H264", *HALF_SIZE, 30)
    full_config, group_0 = length_prefixed_group(full, 2)
    half_config, group_1 = length_prefixed_group(half, 2)
    _, group_2 = length_prefixed_group(half, 2)

    async def objects():
        yield video_object(0, 0, group_0[0], extension_headers=[(VIDEO_CONFIG_ID, full_config)])
        yield video_object(0, 1, group_0[1])
        for object_id, payload in enumerate(group_2):
            yield video_object(2, object_id, payload)
        yield video_object(1, 0, group_1[0], extension_headers=[(VIDEO_CONFIG_ID, half_config)])
        yield video_object(1, 1, group_1[1])

    # Act
    decoded = [d async for d in decode_pictures(objects(), "avc1.42e01f")]

    # Assert
    sizes = {(d.location.group_id, d.location.object_id): (d.picture.width, d.picture.height) for d in decoded}
    assert sizes == {(0, 0): SIZE, (0, 1): SIZE, (1, 0): HALF_SIZE, (1, 1): HALF_SIZE, (2, 0): HALF_SIZE, (2, 1): HALF_SIZE}


async def test_a_late_group_carrying_the_first_video_config_of_its_own_passes_it_on_to_the_groups_after_it():
    # Arrange: groups 0 and 2 are full size and groups 1 and 3 half size; group 2 holds as many
    # objects as it may before group 1, the first with the half-size config, arrives
    full = VideoEncoder("H264", *SIZE, 30)
    half = VideoEncoder("H264", *HALF_SIZE, 30)
    full_config, group_0 = length_prefixed_group(full, 2)
    _, group_2 = length_prefixed_group(full, codecs.MAX_HELD_OBJECTS)
    half_config, group_1 = length_prefixed_group(half, 2)
    _, group_3 = length_prefixed_group(half, 2)

    async def objects():
        yield video_object(0, 0, group_0[0], extension_headers=[(VIDEO_CONFIG_ID, full_config)])
        yield video_object(0, 1, group_0[1])
        for object_id, payload in enumerate(group_2):
            yield video_object(2, object_id, payload)
        yield video_object(1, 0, group_1[0], extension_headers=[(VIDEO_CONFIG_ID, half_config)])
        yield video_object(1, 1, group_1[1])
        for object_id, payload in enumerate(group_3):
            yield video_object(3, object_id, payload)

    # Act
    decoded = [d async for d in decode_pictures(objects(), "avc1.42e01f")]

    # Assert: group 2 had decoded with group 0's config, and passes group 1's on
    sizes = {(d.location.group_id, d.location.object_id): (d.picture.width, d.picture.height) for d in decoded}
    assert {location: size for location, size in sizes.items() if location[0] == 3} == {(3, 0): HALF_SIZE, (3, 1): HALF_SIZE}
    assert {size for location, size in sizes.items() if location[0] in (0, 2)} == {SIZE}


async def test_a_late_groups_video_config_reaches_the_groups_after_a_lost_one():
    # Arrange: group 2 is lost; groups 3 and 4 open before group 1, which changes the config to half
    # size, and groups 5 and 6 open after it; groups 1 to 6 are half size
    full = VideoEncoder("H264", *SIZE, 30)
    half = VideoEncoder("H264", *HALF_SIZE, 30)
    full_config, group_0 = length_prefixed_group(full, 2)
    half_config, group_1 = length_prefixed_group(half, 2)
    _, group_3 = length_prefixed_group(half, 2)
    _, group_4 = length_prefixed_group(half, 2)
    _, group_5 = length_prefixed_group(half, 2)
    _, group_6 = length_prefixed_group(half, 2)

    async def objects():
        yield video_object(0, 0, group_0[0], extension_headers=[(VIDEO_CONFIG_ID, full_config)])
        yield video_object(0, 1, group_0[1])
        for group_id, group in ((3, group_3), (4, group_4)):
            for object_id, payload in enumerate(group):
                yield video_object(group_id, object_id, payload)
        yield video_object(1, 0, group_1[0], extension_headers=[(VIDEO_CONFIG_ID, half_config)])
        yield video_object(1, 1, group_1[1])
        for group_id, group in ((5, group_5), (6, group_6)):
            for object_id, payload in enumerate(group):
                yield video_object(group_id, object_id, payload)

    # Act
    decoded = [d async for d in decode_pictures(objects(), "avc1.42e01f")]

    # Assert: group 3 had decoded with group 0's config, and passes group 1's on
    sizes = {(d.location.group_id, d.location.object_id): (d.picture.width, d.picture.height) for d in decoded}
    assert {location: size for location, size in sizes.items() if location[0] in (5, 6)} == {
        (group_id, object_id): HALF_SIZE for group_id in (5, 6) for object_id in (0, 1)
    }


async def test_a_late_groups_video_config_does_not_reach_the_groups_after_a_newer_one_carrying_its_own():
    # Arrange: group 2 is lost; groups 3 and 4 open before group 1 and inherit group 0's full-size
    # config, which group 3 then carries itself; late group 1 brings a half-size one; groups 0 and
    # 3 to 5 are full size
    full = VideoEncoder("H264", *SIZE, 30)
    half = VideoEncoder("H264", *HALF_SIZE, 30)
    full_config, group_0 = length_prefixed_group(full, 2)
    _, group_3 = length_prefixed_group(full, 3)
    _, group_4 = length_prefixed_group(full, 2)
    _, group_5 = length_prefixed_group(full, 2)
    half_config, group_1 = length_prefixed_group(half, 2)

    async def objects():
        yield video_object(0, 0, group_0[0], extension_headers=[(VIDEO_CONFIG_ID, full_config)])
        yield video_object(0, 1, group_0[1])
        yield video_object(3, 0, group_3[0])
        yield video_object(3, 1, group_3[1])
        yield video_object(4, 0, group_4[0])
        yield video_object(3, 2, group_3[2], extension_headers=[(VIDEO_CONFIG_ID, full_config)])
        yield video_object(4, 1, group_4[1])
        yield video_object(1, 0, group_1[0], extension_headers=[(VIDEO_CONFIG_ID, half_config)])
        yield video_object(1, 1, group_1[1])
        for object_id, payload in enumerate(group_5):
            yield video_object(5, object_id, payload)

    # Act
    decoded = [d async for d in decode_pictures(objects(), "avc1.42e01f")]

    # Assert: groups 4 and 5 keep group 3's config
    sizes = [((d.location.group_id, d.location.object_id), (d.picture.width, d.picture.height)) for d in decoded]
    assert {size for location, size in sizes if location[0] in (4, 5)} == {SIZE}
    assert {location for location, _ in sizes if location[0] in (4, 5)} == {(4, 0), (4, 1), (5, 0), (5, 1)}


@pytest.mark.parametrize("end_of_group", [True, False])
async def test_the_groups_after_a_lost_group_decode_with_the_video_config_of_an_earlier_group(end_of_group):
    # Arrange: only group 0 carries the Video Config, and group 1 never comes; with End of Group
    # status objects or without
    encoder = VideoEncoder("H264", *SIZE, 30)
    groups = [length_prefixed_group(encoder, 2) for _ in range(9)]
    config = groups[0][0]

    async def objects():
        for group_id, (_, payloads) in enumerate(groups):
            if group_id == 1:
                continue
            for object_id, payload in enumerate(payloads):
                headers = [(VIDEO_CONFIG_ID, config)] if (group_id, object_id) == (0, 0) else []
                yield video_object(group_id, object_id, payload, extension_headers=headers)
            if end_of_group:
                yield video_object(group_id, len(payloads), b"", status=END_OF_GROUP)

    # Act
    decoded = [d async for d in decode_pictures(objects(), "avc1.42e01f")]

    # Assert
    locations = sorted((d.location.group_id, d.location.object_id) for d in decoded)
    assert locations == [(g, o) for g in (0, *range(2, 9)) for o in range(2)]


async def test_a_group_whose_predecessor_was_lost_starts_at_a_keyframe_carrying_its_own_video_config():
    # Arrange: group 0 is full size; group 1 never comes; group 2 is half size, and its second
    # keyframe carries its Video Config
    full = VideoEncoder("H264", *SIZE, 30)
    half = VideoEncoder("H264", *HALF_SIZE, 30)
    full_config, group_0 = length_prefixed_group(full, 2)
    half_config, first = length_prefixed_group(half, 1)
    _, second = length_prefixed_group(half, 2)

    async def objects():
        yield video_object(0, 0, group_0[0], extension_headers=[(VIDEO_CONFIG_ID, full_config)])
        yield video_object(0, 1, group_0[1])
        yield video_object(2, 0, first[0])
        yield video_object(2, 1, second[0], extension_headers=[(VIDEO_CONFIG_ID, half_config)])
        yield video_object(2, 2, second[1])

    # Act
    decoded = [d async for d in decode_pictures(objects(), "avc1.42e01f")]

    # Assert: the object before the keyframe is dropped
    sizes = {(d.location.group_id, d.location.object_id): (d.picture.width, d.picture.height) for d in decoded}
    assert sizes == {(0, 0): SIZE, (0, 1): SIZE, (2, 1): HALF_SIZE, (2, 2): HALF_SIZE}


@pytest.mark.parametrize("bound", ["objects", "bytes"])
async def test_a_group_whose_predecessor_was_lost_decodes_once_it_holds_too_much(monkeypatch, bound):
    # Arrange: only group 0 carries the Video Config, group 1 never comes, and group 2 holds as
    # many objects as it may, or more bytes, before the track stalls
    if bound == "bytes":
        monkeypatch.setattr(codecs, "MAX_HELD_BYTES", 1)
    encoder = VideoEncoder("H264", *SIZE, 30)
    config, group_0 = length_prefixed_group(encoder, 1)
    _, group_2 = length_prefixed_group(encoder, codecs.MAX_HELD_OBJECTS)
    stalled = asyncio.Event()

    async def objects():
        yield video_object(0, 0, group_0[0], extension_headers=[(VIDEO_CONFIG_ID, config)])
        for object_id, payload in enumerate(group_2):
            yield video_object(2, object_id, payload)
        await stalled.wait()

    pictures = decode_pictures(objects(), "avc1.42e01f")

    # Act
    decoded = [await within(anext(pictures)) for _ in range(2)]

    # Assert
    assert [(d.location.group_id, d.location.object_id) for d in decoded] == [(0, 0), (2, 0)]
    await pictures.aclose()


async def test_a_group_opened_before_its_predecessor_changed_its_video_config_decodes_with_the_new_one():
    # Arrange: group 0 starts full size and its second keyframe changes to half size, which group 1
    # is, without a Video Config of its own; group 1 opens before that keyframe arrives, and no End
    # of Group closes group 0, as a followed track skips status objects
    full = VideoEncoder("H264", *SIZE, 30)
    half = VideoEncoder("H264", *HALF_SIZE, 30)
    full_config, first = length_prefixed_group(full, 1)
    half_config, second = length_prefixed_group(half, 1)
    _, group_1 = length_prefixed_group(half, 2)

    async def objects():
        yield video_object(0, 0, first[0], extension_headers=[(VIDEO_CONFIG_ID, full_config)])
        yield video_object(1, 0, group_1[0])
        yield video_object(0, 1, second[0], extension_headers=[(VIDEO_CONFIG_ID, half_config)])
        yield video_object(1, 1, group_1[1])

    # Act
    decoded = [d async for d in decode_pictures(objects(), "avc1.42e01f")]

    # Assert: the picture group 1 gave before the change is given again, at the new size
    given = [((d.location.group_id, d.location.object_id), (d.picture.width, d.picture.height)) for d in decoded]
    assert given[2:] == [((0, 1), HALF_SIZE), ((1, 0), HALF_SIZE), ((1, 1), HALF_SIZE)]


async def test_the_groups_after_one_that_can_no_longer_take_its_predecessors_new_video_config_decode_with_it():
    # Arrange: as above, but group 1 decoded more objects than it can decode again before group
    # 0's keyframe changes the Video Config; groups 2 and 3 open after the change, without one of
    # their own
    full = VideoEncoder("H264", *SIZE, 30)
    half = VideoEncoder("H264", *HALF_SIZE, 30)
    full_config, first = length_prefixed_group(full, 1)
    half_config, second = length_prefixed_group(half, 1)
    _, group_1 = length_prefixed_group(half, codecs.MAX_HELD_OBJECTS + 1)
    _, group_2 = length_prefixed_group(half, 2)
    _, group_3 = length_prefixed_group(half, 2)

    async def objects():
        yield video_object(0, 0, first[0], extension_headers=[(VIDEO_CONFIG_ID, full_config)])
        for object_id, payload in enumerate(group_1):
            yield video_object(1, object_id, payload)
        yield video_object(0, 1, second[0], extension_headers=[(VIDEO_CONFIG_ID, half_config)])
        for group_id, group in ((2, group_2), (3, group_3)):
            for object_id, payload in enumerate(group):
                yield video_object(group_id, object_id, payload)

    # Act
    decoded = [d async for d in decode_pictures(objects(), "avc1.42e01f")]

    # Assert
    sizes = {(d.location.group_id, d.location.object_id): (d.picture.width, d.picture.height) for d in decoded if d.location.group_id >= 2}
    assert sizes == {(2, 0): HALF_SIZE, (2, 1): HALF_SIZE, (3, 0): HALF_SIZE, (3, 1): HALF_SIZE}


async def test_a_keyframe_carrying_another_video_config_within_a_group_reconfigures_its_decoder():
    # Arrange: one group whose second keyframe changes the size
    full = VideoEncoder("H264", *SIZE, 30)
    half = VideoEncoder("H264", *HALF_SIZE, 30)
    full_config, first = length_prefixed_group(full, 2)
    half_config, second = length_prefixed_group(half, 2)

    async def objects():
        yield video_object(0, 0, first[0], extension_headers=[(VIDEO_CONFIG_ID, full_config)])
        yield video_object(0, 1, first[1])
        yield video_object(0, 2, second[0], extension_headers=[(VIDEO_CONFIG_ID, half_config)])
        yield video_object(0, 3, second[1])

    # Act
    decoded = [d async for d in decode_pictures(objects(), "avc1.42e01f")]

    # Assert
    assert [(d.location.object_id, (d.picture.width, d.picture.height)) for d in decoded] == [
        (0, SIZE),
        (1, SIZE),
        (2, HALF_SIZE),
        (3, HALF_SIZE),
    ]


class TrackedLocation:
    """Stands in for an object's `Location`, which takes no weak reference, to count those kept."""


@pytest.mark.parametrize("kind", ["refused", "invisible"])
def test_a_decoder_keeps_no_location_of_an_object_that_makes_no_picture(kind):
    # Arrange: length-prefixed H.264 without its Video Config, which the decoder refuses, or VP8
    # frames after a keyframe that are not shown, which make no picture
    if kind == "refused":
        decoder = VideoDecoder("avc1.42e01f")
        annex_b = b"".join(VideoEncoder("H264", *SIZE, 30).encode(picture(0), SIZE, "RGB"))
        payloads = [length_prefixed(annex_b)[2]] * 32
    else:
        decoder = VideoDecoder("vp8")
        keyframe, *payloads = invisible_vp8_frames(32)
        decoder.decode(keyframe, Location(0, 0))
    locations = [TrackedLocation() for _ in payloads]
    kept = [weakref.ref(location) for location in locations]

    # Act
    pictures = [decoded for payload, location in zip(payloads, locations) for decoded in decoder.decode(payload, location)]
    pictures.extend(decoder.flush())
    del locations
    gc.collect()

    # Assert: FFmpeg keeps only the properties of the last packet it was given and of the reference
    # frames the codec keeps, three for VP8
    assert pictures == []
    assert sum(location() is not None for location in kept) <= 4


def invisible_vp8_frames(count: int) -> list[bytes]:
    """A VP8 keyframe, then `count` inter frames whose show_frame flag is cleared (RFC 6386 §9.1)."""
    encoder = VideoEncoder("VP8", *SIZE, 30)
    frames = [b"".join(encoder.encode(picture(value * 7 % 256), SIZE, "RGB")) for value in range(count + 1)]
    return [frames[0], *(bytes([frame[0] & ~0x10]) + frame[1:] for frame in frames[1:])]


def undecodable_group(count: int) -> list[bytes]:
    """Annex B H.264 pictures without their parameter sets, which the decoder refuses."""
    encoder = VideoEncoder("H264", *SIZE, 30)
    return [
        without_parameter_sets(b"".join(encoder.encode(picture(value * 10), SIZE, "RGB"))) for value in range(count)
    ]


def without_parameter_sets(annex_b: bytes) -> bytes:
    """An H.264 access unit's NAL units other than its SPS and PPS, each after a 4-byte start code."""
    return b"".join(b"\x00\x00\x00\x01" + unit for unit in nal_units(annex_b) if unit[0] & 0x1F not in (7, 8))


@pytest.mark.parametrize("count", [5, 2], ids=["refused in a row", "ended without a picture"])
async def test_a_group_joined_at_its_keyframe_that_does_not_decode_fails_the_track(count):
    # Arrange: the group ends with its End of Group
    payloads = undecodable_group(count)

    async def objects():
        for object_id, payload in enumerate(payloads):
            yield video_object(0, object_id, payload)
        yield video_object(0, count, b"", status=END_OF_GROUP)

    # Act / Assert
    with pytest.raises(FFmpegError):
        _ = [d async for d in decode_pictures(objects(), "avc1.42e01f")]


async def test_a_group_joined_midway_that_does_not_decode_is_skipped():
    # Arrange: the objects from the second on
    payloads = undecodable_group(5)

    async def objects():
        for object_id, payload in list(enumerate(payloads))[1:]:
            yield video_object(0, object_id, payload)
        yield video_object(0, len(payloads), b"", status=END_OF_GROUP)

    # Act
    decoded = [d async for d in decode_pictures(objects(), "avc1.42e01f")]

    # Assert
    assert decoded == []


async def test_a_request_waiting_on_a_track_that_does_not_decode_gets_its_failure():
    # Arrange: alice's camera sends H.264 without its parameter sets
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [[*undecodable_group(5), STAY_OPEN]]
    sink = RecordingSink()
    receiver = ParticipantReceiver(RemoteBroadcast(session, [*ROOM, "alice"]), receive_options(video_sources={"camera": None}), sink)
    request = UserImageRequestFrame(user_id="alice")
    await receiver.request_image(request)

    # Act
    running = asyncio.create_task(receiver.run())
    await asyncio.sleep(0.2)

    # Assert
    assert sink.unanswered == [(request, "receiving video of alice failed")]
    assert [event[0] for event in sink.events].count("on_error") == 1
    running.cancel()
    await asyncio.gather(running, return_exceptions=True)
    await session.close()


async def raise_connection_error(result: dict) -> None:
    raise ConnectionError("the bot's backend is gone")


async def await_a_cancelled_future(result: dict) -> None:
    """Raises CancelledError, as a callback awaiting what another task cancelled does."""
    cancelled = asyncio.get_running_loop().create_future()
    cancelled.cancel()
    await cancelled


@pytest.mark.parametrize("raising", [raise_connection_error, await_a_cancelled_future])
async def test_a_raising_result_callback_leaves_the_other_requests_answered_and_the_failed_track_reported_and_retried(
    monkeypatch, raising
):
    # Arrange: two requests wait on alice's camera, which sends H.264 without its parameter sets;
    # the first one's callback raises
    monkeypatch.setattr(_participant, "RETRY_DELAY_SECONDS", 0.05)
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[VIDEO_CATALOG, STAY_OPEN]]
    native.tracks["video"] = [[*undecodable_group(5), STAY_OPEN], [STAY_OPEN]]
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(session, [*ROOM, "alice"]),
        MoqWasmInputParams(video_in_enabled=True, video_in_sources={"camera": VideoInSourceParams(on_request_only=True)}),
    )
    errors: list[str] = []
    answers: list[dict] = []

    async def push_error(message: str, *args, **kwargs) -> None:
        errors.append(message)

    async def answered(result: dict) -> None:
        answers.append(result)

    monkeypatch.setattr(participant_input, "push_error", push_error)
    for callback in (raising, answered):
        await participant_input.request_image(UserImageRequestFrame(user_id="alice", result_callback=callback))

    # Act
    await within(start_alone(participant_input))
    await until(lambda: len(native.subscriptions) == 3)

    # Assert
    assert answers == [{"error": "receiving video of alice failed"}]
    assert errors == ["receiving video of alice failed"]
    await within(participant_input.stop(EndFrame()))
    await within(participant_input.cleanup())
    await session.close()


async def test_cancelling_the_answers_to_image_requests_stops_them_at_the_callback_being_awaited():
    # Arrange: the first of two requests' callbacks waits
    relay = FakeRelay()
    session = await open_session(relay)
    participant_input = MoqWasmParticipantInput(RemoteBroadcast(session, [*ROOM, "alice"]), MoqWasmInputParams())
    called = asyncio.Event()
    answers: list[dict] = []

    async def wait(result: dict) -> None:
        called.set()
        await asyncio.Event().wait()

    async def answered(result: dict) -> None:
        answers.append(result)

    requests = [UserImageRequestFrame(user_id="alice", result_callback=callback) for callback in (wait, answered)]
    answering = asyncio.create_task(participant_input.answer_image_requests(requests, "alice left"))
    await within(called.wait())

    # Act
    answering.cancel()
    await asyncio.gather(answering, return_exceptions=True)

    # Assert
    assert answering.cancelled()
    assert answers == []
    await session.close()


async def test_pictures_of_groups_whose_objects_interleave_are_all_decoded():
    # Arrange: the objects of two groups arrive in turn, as their streams do
    groups = b_frame_groups(2, 6)

    async def objects():
        for object_id in range(6):
            for group_id, group in enumerate(groups):
                yield video_object(group_id, object_id, group[object_id][0])

    # Act
    decoded = [d async for d in decode_pictures(objects(), "avc1.64001f")]

    # Assert
    values = [round(d.picture.to_ndarray(format="rgb24").mean() / 10) * 10 for d in decoded]
    assert sorted(values) == [index * 10 for index in range(12)]


async def test_the_pictures_a_group_holds_back_come_at_its_end_of_group():
    # Arrange: the track stays open after the group, as a followed track does
    (group,) = b_frame_groups(1, 6)

    async def objects():
        for object_id, (payload, _) in enumerate(group):
            yield video_object(0, object_id, payload)
        yield video_object(0, len(group), b"", status=END_OF_GROUP)
        await asyncio.Event().wait()

    pictures = decode_pictures(objects(), "avc1.64001f")

    # Act
    decoded = [await within(anext(pictures)) for _ in range(6)]

    # Assert
    values = [round(d.picture.to_ndarray(format="rgb24").mean() / 10) * 10 for d in decoded]
    assert values == [index * 10 for index in range(6)]
    await pictures.aclose()


def test_finishing_the_encoder_turns_a_clip_shorter_than_a_packet_into_audio():
    # Arrange
    encoder = OpusEncoder()
    decoder = OpusDecoder(48_000)

    # Act: 20 ms at 16 kHz, held back by the resampler
    packets = [*encoder.encode(tone(0.02), SAMPLE_RATE), *encoder.finish()]

    # Assert
    assert len(b"".join(decoder.decode(packet) for packet in packets)) >= 960 * 2


def test_finishing_the_encoder_turns_a_clip_the_resampler_holds_entirely_into_audio():
    # Arrange: 1 ms at 16 kHz, all of it held inside the resampler
    encoder = OpusEncoder()
    decoder = OpusDecoder(48_000)

    # Act
    packets = [*encoder.encode(tone(0.001), SAMPLE_RATE), *encoder.finish()]

    # Assert: the tone (peak 8000) is heard, not silence in its place
    pcm = np.frombuffer(b"".join(decoder.decode(packet) for packet in packets), np.int16)
    assert np.abs(pcm).max() > 4000


def test_stereo_speech_is_downmixed_into_mono_packets_of_its_duration():
    # Arrange
    mono = np.frombuffer(tone(0.2, 48_000), np.int16)
    stereo = np.repeat(mono, 2).tobytes()
    encoder = OpusEncoder()
    decoder = OpusDecoder(48_000)

    # Act
    packets = encoder.encode(stereo, 48_000, 2)

    # Assert: 200 ms of stereo is ten 20 ms packets, not twenty
    assert len(packets) == 10
    assert len(b"".join(decoder.decode(packet) for packet in packets)) == 10 * 960 * 2


def test_the_opus_encoder_takes_pcm_at_another_rate_than_the_call_before():
    # Arrange: 200 ms of speech at 48 kHz
    encoder = OpusEncoder()
    packets = encoder.encode(tone(0.2, 48_000), 48_000)

    # Act: 100 ms of silence at 16 kHz
    packets += encoder.encode(bytes(2 * SAMPLE_RATE // 10), SAMPLE_RATE)

    # Assert: 300 ms, less the samples short of a packet
    assert len(packets) == 14


def test_the_encoder_takes_mono_pcm_at_another_rate_after_stereo_speech():
    # Arrange: 200 ms of stereo speech at 48 kHz, as pipecat sends before its mono end silence
    stereo = np.repeat(np.frombuffer(tone(0.2, 48_000), np.int16), 2).tobytes()
    encoder = OpusEncoder()
    packets = encoder.encode(stereo, 48_000, 2)

    # Act: 100 ms of mono silence at 16 kHz
    packets += encoder.encode(bytes(2 * SAMPLE_RATE // 10), SAMPLE_RATE)
    packets += encoder.finish()

    # Assert: 300 ms, the last packet padded
    assert len(packets) in (15, 16)


@pytest.mark.parametrize("sample_rate", [0, -16_000, 16_000.0])
def test_pcm_at_a_sample_rate_the_encoder_cannot_take_is_refused_and_the_encoder_goes_on(sample_rate):
    # Arrange
    encoder = OpusEncoder()
    with pytest.raises(ValueError, match="sample rate"):
        encoder.encode(tone(0.02), sample_rate)

    # Act: 200 ms of speech at 16 kHz
    packets = [*encoder.encode(tone(0.2), SAMPLE_RATE), *encoder.finish()]

    # Assert: ten 20 ms packets, the refused PCM taken as none
    assert len(packets) in (10, 11)


def test_the_transport_refuses_output_destinations():
    # Act / Assert
    with pytest.raises(ValueError, match="destinations"):
        MoqWasmTransport(SimpleNamespace(), MoqWasmParams(room=ROOM, audio_out_destinations=["es"]))


def test_the_transport_refuses_output_audio_of_more_than_two_channels():
    # Act / Assert
    with pytest.raises(ValueError, match="mono or stereo"):
        MoqWasmTransport(SimpleNamespace(), MoqWasmParams(room=ROOM, audio_out_channels=3))


@pytest.mark.parametrize("by_destination", [False, True], ids=["mixer", "mixer by destination"])
def test_the_transport_refuses_a_soundfile_mixer_with_stereo_output_audio(by_destination):
    # Arrange: pipecat's SoundfileMixer mixes one sample of its mono file into each
    # sample of the audio
    mixer = SoundfileMixer(sound_files={"office": "office.wav"}, default_sound="office")
    params = MoqWasmParams(room=ROOM, audio_out_channels=2, audio_out_mixer={None: mixer} if by_destination else mixer)

    # Act / Assert
    with pytest.raises(ValueError, match="SoundfileMixer"):
        MoqWasmTransport(SimpleNamespace(), params)


def test_the_transport_refuses_an_input_audio_filter():
    # Arrange: the transport's input carries every participant's audio through the one filter
    params = MoqWasmParams(room=ROOM, audio_in_enabled=True, audio_in_filter=RecordingFilter())

    # Act / Assert
    with pytest.raises(ValueError, match="MoqWasmParticipantInput"):
        MoqWasmTransport(SimpleNamespace(), params)


class RecordingMixer(BaseAudioMixer):
    """An audio mixer recording whether it mixed audio."""

    def __init__(self):
        self.mixed = False

    async def start(self, sample_rate: int):
        pass

    async def stop(self):
        pass

    async def process_frame(self, frame):
        pass

    async def mix(self, audio: bytes) -> bytes:
        self.mixed = True
        return audio


async def test_the_transport_keeps_the_audio_mixers_by_destination_it_was_built_with():
    # Arrange: a stereo output whose mixer mapping the caller changes once it is built, as to a
    # SoundfileMixer the transport would have refused
    relay = FakeRelay()
    session = await open_session(relay)
    kept, replaced = RecordingMixer(), RecordingMixer()
    params = MoqWasmParams(room=ROOM, audio_out_enabled=True, audio_out_channels=2, audio_out_mixer={None: kept})
    transport = MoqWasmTransport(session, params)
    assert isinstance(params.audio_out_mixer, dict)
    params.audio_out_mixer[None] = replaced
    speech = OutputAudioRawFrame(audio=tone(0.1), sample_rate=SAMPLE_RATE, num_channels=1)

    # Act
    await within(run_test(transport.output(), frames_to_send=[speech, SleepFrame(0.3)]))

    # Assert: the output mixed with the mixer it was built with
    assert kept.mixed and not replaced.mixed
    await session.close()


async def test_the_transport_keeps_the_room_it_was_built_with():
    # Arrange: a caller that changes the room's list once the transport is built
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(room=list(ROOM))
    transport = MoqWasmTransport(session, params)
    params.room[-1] = "elsewhere"

    # Act
    await within(run_test(transport.input(), frames_to_send=[SleepFrame(0.2)]))

    # Assert: the transport published and followed the room it was built with
    native = relay.sessions[0]
    assert ("watch_namespaces", ROOM) in native.calls
    assert ("announce", [*ROOM, "bot"]) in native.calls
    await session.close()


@pytest.mark.parametrize("input_kind", ["transport", "participant"])
def test_an_input_refuses_input_audio_of_more_than_two_channels(input_kind):
    # Act / Assert
    with pytest.raises(ValueError, match="mono or stereo"):
        if input_kind == "transport":
            MoqWasmTransport(SimpleNamespace(), MoqWasmParams(room=ROOM, audio_in_channels=3))
        else:
            remote = RemoteBroadcast(SimpleNamespace(), [*ROOM, "alice"])  # type: ignore[arg-type]
            MoqWasmParticipantInput(remote, MoqWasmInputParams(audio_in_channels=3))


@pytest.mark.parametrize("chunks", [0, -1])
def test_the_transport_refuses_audio_chunks_of_no_10_ms(chunks):
    # Act / Assert
    with pytest.raises(ValueError, match="audio_out_10ms_chunks"):
        MoqWasmTransport(SimpleNamespace(), MoqWasmParams(room=ROOM, audio_out_10ms_chunks=chunks))


async def test_an_output_whose_audio_chunks_hold_no_sample_starts_no_sender_and_is_unusable():
    # Arrange: at 50 Hz, 10 ms of audio holds no sample, so pipecat's chunks would be empty
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(room=ROOM, audio_out_enabled=True, audio_out_sample_rate=50)
    output = MoqWasmTransport(session, params).output()

    # Act
    _, up = await within(run_test(output, frames_to_send=[SleepFrame(0.1)]))

    # Assert: no audio sender started, and the output reported it can write nothing
    errors = [frame for frame in up if isinstance(frame, ErrorFrame)]
    assert output._media_senders == {}
    assert len(errors) == 1 and "no sample in 10 ms" in errors[0].error
    assert not output.is_usable
    await session.close()


def message_payloads(frames) -> list[bytes]:
    return [frame.message.payload for frame in frames if isinstance(frame, InputTransportMessageFrame)]


async def test_a_participant_input_a_stop_frame_paused_pushes_no_message_until_the_next_pipeline_resumes_it():
    # Arrange: alice's prompt track sends a message in the first run, one while a StopFrame paused
    # the input, and one once the next pipeline resumed it
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    paused, resumed = asyncio.Event(), asyncio.Event()
    native.tracks["prompt"] = [[b"first", paused, b"paused", resumed, b"resumed", STAY_OPEN]]
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(session, [*ROOM, "alice"]), MoqWasmInputParams(data_in_tracks=["prompt"])
    )
    first_run, _ = await within(
        run_test(participant_input, frames_to_send=[SleepFrame(0.1), StopFrame()], send_end_frame=False)
    )
    _, while_paused = link_recorders(participant_input)
    paused.set()
    await asyncio.sleep(0.1)

    # Act: the next pipeline resumes the input, which an EndFrame ends
    asyncio.get_running_loop().call_later(0.1, resumed.set)
    second_run, _ = await within(run_test(participant_input, frames_to_send=[SleepFrame(0.3)]))

    # Assert
    assert message_payloads(first_run) == [b"first"]
    assert message_payloads(while_paused.frames) == []
    assert message_payloads(second_run) == [b"resumed"]
    await session.close()


async def test_audio_at_no_sample_rate_is_reported_and_the_speech_after_it_is_written_to_its_end():
    # Arrange: 10 ms at a sample rate of 0, then 50 ms of speech, the last 10 ms of it short of
    # pipecat's chunk
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM, audio_out_enabled=True, audio_out_sample_rate=SAMPLE_RATE, audio_out_end_silence_secs=0
    )
    transport = MoqWasmTransport(session, params)
    no_rate = OutputAudioRawFrame(audio=tone(0.01), sample_rate=0, num_channels=1)
    speech = OutputAudioRawFrame(audio=tone(0.05), sample_rate=SAMPLE_RATE, num_channels=1)

    # Act: run_test ends the pipeline with an EndFrame and waits for it to come out
    _, up = await within(run_test(transport.output(), frames_to_send=[no_rate, speech]))

    # Assert: the frame without a rate was reported, and all of the speech was written
    errors = [frame for frame in up if isinstance(frame, ErrorFrame)]
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert len(errors) == 1 and "sample rate" in errors[0].error and not errors[0].fatal
    assert len(source.audio_timestamps) * OPUS_FRAME_MICROS >= 50_000
    assert ("unannounce", [*ROOM, "bot"]) in relay.sessions[0].calls
    await session.close()


async def test_an_empty_audio_frame_at_another_rate_is_written_as_nothing_between_the_speech():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM, audio_out_enabled=True, audio_out_sample_rate=SAMPLE_RATE, audio_out_end_silence_secs=0
    )
    transport = MoqWasmTransport(session, params)
    speech = OutputAudioRawFrame(audio=tone(0.05), sample_rate=SAMPLE_RATE, num_channels=1)
    empty = OutputAudioRawFrame(audio=b"", sample_rate=24_000, num_channels=1)
    more_speech = OutputAudioRawFrame(audio=tone(0.05), sample_rate=SAMPLE_RATE, num_channels=1)

    # Act
    _, up = await within(run_test(transport.output(), frames_to_send=[speech, empty, more_speech]))

    # Assert
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert [frame for frame in up if isinstance(frame, ErrorFrame)] == []
    assert len(source.audio_timestamps) * OPUS_FRAME_MICROS >= 100_000
    await session.close()


async def test_an_output_whose_resampler_fails_at_the_end_of_the_speech_reports_it_and_still_ends(monkeypatch):
    # Arrange: flushing a resampler fails, as the output's EndFrame flushes what it holds of 100 ms
    # of speech
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM, audio_out_enabled=True, audio_out_sample_rate=SAMPLE_RATE, audio_out_end_silence_secs=0
    )
    output = MoqWasmTransport(session, params).output()
    upstream, downstream = link_recorders(output)
    await within(start_alone(output))
    await within(output.process_frame(OutputAudioRawFrame(audio=tone(0.1), sample_rate=24_000, num_channels=1), FrameDirection.DOWNSTREAM))

    def fail_flush(self) -> bytes:
        raise RuntimeError("flush failed")

    monkeypatch.setattr(codecs._PcmResampler, "flush", fail_flush)
    end = EndFrame()

    # Act
    await within(output.process_frame(end, FrameDirection.DOWNSTREAM))

    # Assert: the EndFrame went on, so the pipeline ends, the failure went upstream as a non-fatal
    # ErrorFrame, and the room was left
    errors = [frame for frame in upstream.frames if isinstance(frame, ErrorFrame)]
    assert end in downstream.frames
    assert errors and all("flush failed" in error.error and not error.fatal for error in errors)
    assert ("unannounce", [*ROOM, "bot"]) in relay.sessions[0].calls
    await within(output.cleanup())
    await session.close()


def decoded_speech(source) -> np.ndarray:
    decoder = OpusDecoder(SAMPLE_RATE)
    return np.frombuffer(b"".join(decoder.decode(call[1]) for call in source.calls if call[0] == "push_audio"), np.int16)


async def test_an_output_a_stop_frame_stopped_writes_the_speech_before_it_and_the_speech_of_the_next_run():
    # Arrange: pipecat's chunks of 40 ms, and 10 ms of speech at 24 kHz before the StopFrame, short
    # of a chunk and resampled to the output's 16 kHz
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM, audio_out_enabled=True, audio_out_sample_rate=SAMPLE_RATE, audio_out_end_silence_secs=0
    )
    transport = MoqWasmTransport(session, params)
    before_stop = OutputAudioRawFrame(audio=sine(0.01, 24_000).tobytes(), sample_rate=24_000, num_channels=1)
    after_resume = OutputAudioRawFrame(audio=tone(0.01), sample_rate=SAMPLE_RATE, num_channels=1)

    # Act: the first pipeline stops at the StopFrame, and the next one resumes the output and ends
    await within(
        run_test(transport.output(), frames_to_send=[before_stop, SleepFrame(0.1), StopFrame()], send_end_frame=False)
    )
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    stopped = decoded_speech(source)
    await within(run_test(transport.output(), frames_to_send=[SleepFrame(0.1), after_resume]))

    # Assert: the speech before the StopFrame was written at it, and the next run's after it
    assert np.abs(stopped).max() > 2000
    assert np.abs(decoded_speech(source)[len(stopped) :]).max() > 2000
    await session.close()


async def test_a_participant_input_resumed_by_a_pipeline_at_another_rate_pushes_audio_at_that_rate():
    # Arrange: alice's speech, two packets in a first pipeline at 16 kHz that a StopFrame stops,
    # and two once a pipeline at 24 kHz resumed the input
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    resumed = asyncio.Event()
    packets = OpusEncoder().encode(tone(0.08), SAMPLE_RATE)
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN]]
    native.tracks["audio"] = [[*packets[:2], resumed, *packets[2:4], STAY_OPEN]]
    participant_input = MoqWasmParticipantInput(
        RemoteBroadcast(session, [*ROOM, "alice"]), MoqWasmInputParams(audio_in_enabled=True)
    )
    first_run, _ = await within(
        run_test(
            participant_input,
            frames_to_send=[SleepFrame(0.3), StopFrame()],
            send_end_frame=False,
            pipeline_params=PipelineParams(audio_in_sample_rate=SAMPLE_RATE),
        )
    )

    # Act
    asyncio.get_running_loop().call_later(0.1, resumed.set)
    second_run, _ = await within(
        run_test(
            participant_input,
            frames_to_send=[SleepFrame(0.4)],
            pipeline_params=PipelineParams(audio_in_sample_rate=24_000),
        )
    )

    # Assert: each run's audio came at its pipeline's rate
    def audio(frames) -> list[UserAudioRawFrame]:
        return [frame for frame in frames if isinstance(frame, UserAudioRawFrame)]

    assert audio(first_run) and {frame.sample_rate for frame in audio(first_run)} == {SAMPLE_RATE}
    assert audio(second_run) and {frame.sample_rate for frame in audio(second_run)} == {24_000}
    await session.close()


class RecordingFilter(BaseAudioFilter):
    """An audio filter recording its starts, with their sample rate, its stops, and that it
    filtered audio in between."""

    def __init__(self):
        self.calls: list[tuple] = []

    async def start(self, sample_rate: int):
        self.calls.append(("start", sample_rate))

    async def stop(self):
        self.calls.append(("stop",))

    async def process_frame(self, frame):
        pass

    async def filter(self, audio: bytes) -> bytes:
        if self.calls[-1] != ("filter",):
            self.calls.append(("filter",))
        return audio


class BufferingFilter(BaseAudioFilter):
    """An audio filter returning nothing for one chunk and two chunks for the next."""

    def __init__(self):
        self._held = b""

    async def start(self, sample_rate: int):
        pass

    async def stop(self):
        pass

    async def process_frame(self, frame):
        pass

    async def filter(self, audio: bytes) -> bytes:
        if not self._held:
            self._held = audio
            return b""
        filtered, self._held = self._held + audio, b""
        return filtered


async def test_a_participant_input_counts_the_samples_its_audio_filter_returned():
    # Arrange: alice speaks to an input whose filter buffers every other chunk
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN]]
    native.tracks["audio"] = [[*OpusEncoder().encode(tone(0.2), SAMPLE_RATE), STAY_OPEN]]
    params = MoqWasmInputParams(audio_in_enabled=True, audio_in_sample_rate=SAMPLE_RATE, audio_in_filter=BufferingFilter())
    participant_input = MoqWasmParticipantInput(RemoteBroadcast(session, [*ROOM, "alice"]), params)

    # Act
    down, _ = await within(run_test(participant_input, frames_to_send=[SleepFrame(0.5)]))

    # Assert: each frame counts the samples it carries, two chunks' worth
    audio = [frame for frame in down if isinstance(frame, UserAudioRawFrame)]
    assert audio
    assert [frame.num_frames for frame in audio] == [len(frame.audio) // 2 for frame in audio]
    await session.close()


@pytest.mark.parametrize("processor", ["input", "output"])
async def test_a_processor_stopped_resumed_and_ended_stops_its_audio_filter_after_each_start(processor):
    # Arrange: speech in a first pipeline at 16 kHz that a StopFrame stops, and in the pipeline at
    # 24 kHz that resumes the processor and ends
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    audio_filter = RecordingFilter()
    resumed = asyncio.Event()
    if processor == "input":
        packets = OpusEncoder().encode(tone(0.08), SAMPLE_RATE)
        native.tracks["catalog"] = [[AUDIO_CATALOG, STAY_OPEN]]
        native.tracks["audio"] = [[*packets[:2], resumed, *packets[2:4], STAY_OPEN]]
        started = MoqWasmParticipantInput(
            RemoteBroadcast(session, [*ROOM, "alice"]),
            MoqWasmInputParams(audio_in_enabled=True, audio_in_filter=audio_filter),
        )
        speech = []
    else:
        params = MoqWasmParams(room=ROOM, audio_out_enabled=True, audio_out_end_silence_secs=0, audio_out_filter=audio_filter)
        started = MoqWasmTransport(session, params).output()
        speech = [OutputAudioRawFrame(audio=tone(0.1), sample_rate=SAMPLE_RATE, num_channels=1)]
    await within(
        run_test(
            started,
            frames_to_send=[*speech, SleepFrame(0.3), StopFrame()],
            send_end_frame=False,
            pipeline_params=PipelineParams(audio_in_sample_rate=SAMPLE_RATE, audio_out_sample_rate=SAMPLE_RATE),
        )
    )

    # Act
    asyncio.get_running_loop().call_later(0.1, resumed.set)
    await within(
        run_test(
            started,
            frames_to_send=[*speech, SleepFrame(0.4)],
            pipeline_params=PipelineParams(audio_in_sample_rate=24_000, audio_out_sample_rate=24_000),
        )
    )

    # Assert: the filter filtered each run's speech at that run's rate, and was stopped after each
    assert audio_filter.calls == [("start", SAMPLE_RATE), ("filter",), ("stop",), ("start", 24_000), ("filter",), ("stop",)]
    await session.close()


async def test_a_live_output_writes_a_picture_synchronized_with_the_audio_once_the_speech_before_it_was_written():
    # Arrange: 200 ms of speech, then a picture synchronized with it
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM,
        audio_out_enabled=True,
        audio_out_sample_rate=SAMPLE_RATE,
        audio_out_end_silence_secs=0,
        video_out_enabled=True,
        video_out_is_live=True,
        video_out_width=SIZE[0],
        video_out_height=SIZE[1],
    )
    transport = MoqWasmTransport(session, params)
    speech = OutputAudioRawFrame(audio=tone(0.2), sample_rate=SAMPLE_RATE, num_channels=1)
    synced = OutputImageRawFrame(image=picture(0), size=SIZE, format="RGB")
    synced.sync_with_audio = True

    # Act
    down, _ = await within(run_test(transport.output(), frames_to_send=[speech, synced, SleepFrame(0.5)]))

    # Assert: the picture was written after the speech and passed on
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    kinds = [call[0] for call in source.calls]
    assert "push_video" in kinds
    assert kinds[: kinds.index("push_video")].count("push_audio") >= 9
    assert [frame.id for frame in down if isinstance(frame, OutputImageRawFrame)] == [synced.id]
    await session.close()


async def test_a_live_output_writes_each_picture_of_a_sprite():
    # Arrange: a sprite of two pictures
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM, video_out_enabled=True, video_out_is_live=True, video_out_width=SIZE[0], video_out_height=SIZE[1]
    )
    transport = MoqWasmTransport(session, params)
    sprite = SpriteFrame(images=[OutputImageRawFrame(image=picture(value), size=SIZE, format="RGB") for value in (0, 255)])

    # Act
    _, up = await within(run_test(transport.output(), frames_to_send=[sprite, SleepFrame(0.3)]))

    # Assert
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert not [frame for frame in up if isinstance(frame, ErrorFrame)]
    assert [call[0] for call in source.calls].count("push_video") == 2
    await session.close()


async def test_a_live_output_writes_the_assistant_images_of_a_sprite_and_passes_none_on(monkeypatch):
    # Arrange: a sprite of a picture between two assistant images
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM, video_out_enabled=True, video_out_is_live=True, video_out_width=SIZE[0], video_out_height=SIZE[1]
    )
    transport = MoqWasmTransport(session, params)
    values = record_encoded_values(monkeypatch)
    middle = OutputImageRawFrame(image=picture(20), size=SIZE, format="RGB")
    sprite = SpriteFrame(images=[assistant_image(10), middle, assistant_image(30)])

    # Act
    down, _ = await within(run_test(transport.output(), frames_to_send=[sprite, SleepFrame(0.3)]))

    # Assert
    assert values == [10, 20, 30]
    assert not [frame for frame in down if isinstance(frame, AssistantImageRawFrame)]
    await session.close()


async def test_an_interruption_keeps_the_pictures_of_an_uninterruptible_sprite_a_live_output_queued():
    # Arrange: a live output at 10 pictures a second, which waits 100 ms before its first picture,
    # given an uninterruptible sprite of three pictures; the interruption comes before the first
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM,
        video_out_enabled=True,
        video_out_is_live=True,
        video_out_framerate=10,
        video_out_width=SIZE[0],
        video_out_height=SIZE[1],
    )
    transport = MoqWasmTransport(session, params)
    sprite = SpriteFrame(images=[OutputImageRawFrame(image=picture(value), size=SIZE, format="RGB") for value in (0, 128, 255)])
    sprite.interruptible = False

    # Act
    await within(run_test(transport.output(), frames_to_send=[sprite, SleepFrame(0.03), InterruptionFrame(), SleepFrame(0.4)]))

    # Assert
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert len(source.video_timestamps) == 3
    await session.close()


async def test_an_uninterruptible_picture_queued_as_an_interruption_replaces_the_live_video_task_is_written():
    # Arrange: a live output, and an uninterruptible picture that comes once the interruption
    # cancelled the video task, before it created it again
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM, video_out_enabled=True, video_out_is_live=True, video_out_width=SIZE[0], video_out_height=SIZE[1]
    )
    output = MoqWasmTransport(session, params).output()
    await within(start_alone(output))
    sender = output._media_senders[None]
    late = OutputImageRawFrame(image=picture(0), size=SIZE, format="RGB")
    late.interruptible = False
    cancel_video_task = sender._cancel_video_task

    async def cancel_as_a_picture_comes() -> None:
        sender._cancel_video_task = cancel_video_task
        await cancel_video_task()
        await sender.handle_image_frame(late)

    sender._cancel_video_task = cancel_as_a_picture_comes

    # Act
    await within(sender.handle_interruptions(InterruptionFrame()))

    # Assert
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    await until(lambda: len(source.video_timestamps) == 1)
    await within(output.cleanup())
    await session.close()


async def test_an_uninterruptible_picture_the_live_video_task_took_as_an_interruption_waited_is_written(monkeypatch):
    # Arrange: a live output whose encodes take 300 ms, writing an uninterruptible picture, and a
    # second one that comes while the interruption waits for the first, which the video task then
    # takes before the interruption cancels it
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM, video_out_enabled=True, video_out_is_live=True, video_out_width=SIZE[0], video_out_height=SIZE[1]
    )
    output = MoqWasmTransport(session, params).output()
    slow_encodes(monkeypatch, 0.3)
    values = record_encoded_values(monkeypatch)
    await within(start_alone(output))
    sender = output._media_senders[None]
    first, second = (OutputImageRawFrame(image=picture(value), size=SIZE, format="RGB") for value in (1, 2))
    first.interruptible = second.interruptible = False
    await sender.handle_image_frame(first)
    await until(lambda: values == [1])

    # Act
    interrupting = asyncio.create_task(sender.handle_interruptions(InterruptionFrame()))
    await asyncio.sleep(0.05)
    await sender.handle_image_frame(second)
    await within(interrupting)

    # Assert: both pictures were pushed, once each
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    await until(lambda: len(source.video_timestamps) >= 2)
    await asyncio.sleep(0.4)
    assert len(source.video_timestamps) == 2
    await within(output.cleanup())
    await session.close()


async def test_an_interruption_during_a_drain_leaves_it_waiting_for_the_uninterruptible_pictures_the_new_video_task_writes():
    # Arrange: a live output at 10 pictures a second, given the eight uninterruptible pictures of a
    # sprite, draining them as at an EndFrame
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM,
        video_out_enabled=True,
        video_out_is_live=True,
        video_out_framerate=10,
        video_out_width=SIZE[0],
        video_out_height=SIZE[1],
    )
    output = MoqWasmTransport(session, params).output()
    await within(start_alone(output))
    sender = output._media_senders[None]
    pictures = [OutputImageRawFrame(image=picture(value), size=SIZE, format="RGB") for value in range(8)]
    for frame in pictures:
        frame.interruptible = False
    await sender.queue_live_pictures(pictures)
    draining = asyncio.create_task(sender.write_given_pictures())
    await asyncio.sleep(0.05)

    # Act: an interruption creates the video task again while the drain waits
    await within(sender.handle_interruptions(InterruptionFrame()))
    await within(draining)

    # Assert: the drain returned once the new video task wrote every picture
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert len(source.video_timestamps) == 8
    await within(output.cleanup())
    await session.close()


async def test_a_drain_that_timed_out_leaves_no_unretrieved_exception(monkeypatch):
    # Arrange: a live output at 10 pictures a second, given eight pictures, whose drain waits 0.1 s
    monkeypatch.setattr(_transport, "LIVE_VIDEO_DRAIN_TIMEOUT_SECONDS", 0.1)
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM,
        video_out_enabled=True,
        video_out_is_live=True,
        video_out_framerate=10,
        video_out_width=SIZE[0],
        video_out_height=SIZE[1],
    )
    output = MoqWasmTransport(session, params).output()
    await within(start_alone(output))
    sender = output._media_senders[None]
    await sender.queue_live_pictures([OutputImageRawFrame(image=picture(value), size=SIZE, format="RGB") for value in range(8)])
    loop = asyncio.get_running_loop()
    reported: list[dict] = []
    loop.set_exception_handler(lambda loop, context: reported.append(context))

    # Act
    try:
        await within(sender.write_given_pictures())
        await asyncio.sleep(0.05)
        gc.collect()
    finally:
        loop.set_exception_handler(None)

    # Assert
    assert [context["message"] for context in reported] == []
    await within(output.cleanup())
    await session.close()


async def test_a_live_output_given_pictures_faster_than_its_framerate_drops_the_oldest_past_two_seconds(monkeypatch):
    # Arrange: a live output at 10 pictures a second, which queues 20 at most, given 40 at once
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM,
        video_out_enabled=True,
        video_out_is_live=True,
        video_out_framerate=10,
        video_out_width=SIZE[0],
        video_out_height=SIZE[1],
    )
    transport = MoqWasmTransport(session, params)
    values = record_encoded_values(monkeypatch)
    pictures = [OutputImageRawFrame(image=picture(value), size=SIZE, format="RGB") for value in range(40)]

    # Act
    _, up = await within(run_test(transport.output(), frames_to_send=pictures))

    # Assert: the newest pictures were written, the oldest dropped, and that was reported once
    errors = [frame for frame in up if isinstance(frame, ErrorFrame)]
    assert 20 <= len(values) <= 22
    assert values[-20:] == list(range(20, 40))
    assert len(errors) == 1 and "faster than 10 a second" in errors[0].error
    await session.close()


def live_pictures(kind: str) -> tuple[Frame, int]:
    """A frame of live pictures of `kind`, and how many pictures it holds."""
    if kind == "sprite":
        return SpriteFrame(images=[OutputImageRawFrame(image=picture(value), size=SIZE, format="RGB") for value in (0, 255)]), 2
    frame = OutputImageRawFrame(image=picture(0), size=SIZE, format="RGB")
    frame.sync_with_audio = kind == "synchronized"
    return frame, 1


@pytest.mark.parametrize("kind", ["picture", "synchronized", "sprite"])
async def test_a_live_output_ending_gracefully_writes_the_pictures_queued_before_its_end_frame(kind):
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM,
        audio_out_enabled=True,
        audio_out_sample_rate=SAMPLE_RATE,
        audio_out_end_silence_secs=0,
        video_out_enabled=True,
        video_out_is_live=True,
        video_out_width=SIZE[0],
        video_out_height=SIZE[1],
    )
    transport = MoqWasmTransport(session, params)
    frame, count = live_pictures(kind)

    # Act: run_test ends the pipeline with an EndFrame right after the pictures
    await within(run_test(transport.output(), frames_to_send=[frame]))

    # Assert
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert [call[0] for call in source.calls].count("push_video") == count
    await session.close()


async def test_an_interruption_keeps_the_uninterruptible_pictures_a_live_output_queued_or_is_writing():
    # Arrange: a live output paced at 10 pictures a second, given three uninterruptible pictures
    # around an interruptible one; the interruption comes as it writes the first
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM,
        video_out_enabled=True,
        video_out_is_live=True,
        video_out_framerate=10,
        video_out_width=SIZE[0],
        video_out_height=SIZE[1],
    )
    transport = MoqWasmTransport(session, params)
    pictures = [OutputImageRawFrame(image=picture(value), size=SIZE, format="RGB") for value in (0, 64, 128, 255)]
    for index, frame in enumerate(pictures):
        frame.interruptible = index == 2

    # Act
    await within(run_test(transport.output(), frames_to_send=[*pictures, SleepFrame(0.05), InterruptionFrame()]))

    # Assert: the uninterruptible pictures were written, the interruptible one was dropped
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert [call[0] for call in source.calls].count("push_video") == 3
    await session.close()


@pytest.mark.parametrize("kind", ["picture", "sprite"])
async def test_an_output_ending_gracefully_writes_the_still_it_was_given_last(kind):
    # Arrange: an output that is not live, which shows a picture or a sprite as a still
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(room=ROOM, video_out_enabled=True, video_out_width=SIZE[0], video_out_height=SIZE[1])
    transport = MoqWasmTransport(session, params)
    frame, _ = live_pictures(kind)

    # Act: run_test ends the pipeline with an EndFrame right after the still
    await within(run_test(transport.output(), frames_to_send=[frame]))

    # Assert
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert "push_video" in [call[0] for call in source.calls]
    await session.close()


@pytest.fixture
def virtual_room_clock(monkeypatch, virtual_clock) -> VirtualTimeLoop:
    """The test's event loop; the room paces the speech on its clock too."""
    monkeypatch.setattr(_room, "_now_micros", lambda: int(virtual_clock.unix_time() * 1_000_000))
    return virtual_clock


def slow_encodes(monkeypatch, seconds: float) -> None:
    encode = VideoEncoder.encode
    loop = asyncio.get_running_loop()
    sleep = loop.sleep_in_job if isinstance(loop, VirtualTimeLoop) else time.sleep

    def slow(self, image: bytes, size, color_format: str) -> list[bytes]:
        sleep(seconds)
        return encode(self, image, size, color_format)

    monkeypatch.setattr(VideoEncoder, "encode", slow)


async def test_an_output_ending_gracefully_writes_its_last_still_at_once_however_slow_its_framerate(monkeypatch):
    # Arrange: an output that is not live, at a picture a second, whose encodes take 70 ms
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM, video_out_enabled=True, video_out_framerate=1, video_out_width=SIZE[0], video_out_height=SIZE[1]
    )
    transport = MoqWasmTransport(session, params)
    slow_encodes(monkeypatch, 0.07)
    still = OutputImageRawFrame(image=picture(0), size=SIZE, format="RGB")

    # Act: run_test ends the pipeline with an EndFrame right after the still
    started = time.monotonic()
    _, up = await within(run_test(transport.output(), frames_to_send=[still]))

    # Assert: the still was written, without waiting for the next frame a second later
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert len(source.video_timestamps) == 1
    assert time.monotonic() - started < 0.8
    assert not [frame for frame in up if isinstance(frame, ErrorFrame)]
    await session.close()


async def test_an_output_ending_gracefully_reports_a_last_still_it_could_not_write_in_time(monkeypatch):
    # Arrange: an output that is not live whose encodes take longer than the second it waits
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(room=ROOM, video_out_enabled=True, video_out_width=SIZE[0], video_out_height=SIZE[1])
    transport = MoqWasmTransport(session, params)
    slow_encodes(monkeypatch, 1.5)
    still = OutputImageRawFrame(image=picture(0), size=SIZE, format="RGB")

    # Act
    _, up = await within(run_test(transport.output(), frames_to_send=[still]))

    # Assert
    errors = [frame for frame in up if isinstance(frame, ErrorFrame)]
    assert len(errors) == 1 and "the last still was not written" in errors[0].error
    await session.close()


@pytest.mark.parametrize("live", [True, False])
async def test_an_output_a_stop_frame_stopped_writes_the_pictures_it_was_given_before_it_runs_again(live):
    # Arrange: a live output at 10 pictures a second given five pictures, or one that is not live at
    # a picture a second given a still, then a StopFrame
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM,
        video_out_enabled=True,
        video_out_is_live=live,
        video_out_framerate=10 if live else 1,
        video_out_width=SIZE[0],
        video_out_height=SIZE[1],
    )
    transport = MoqWasmTransport(session, params)
    count = 5 if live else 1
    pictures = [OutputImageRawFrame(image=picture(value), size=SIZE, format="RGB") for value in range(count)]
    output = transport.output()

    # Act: the next pipeline resumes the output, which replaces its media senders
    await within(run_test(output, frames_to_send=[*pictures, StopFrame()], send_end_frame=False))
    await within(run_test(transport.output(), frames_to_send=[SleepFrame(0.1)]))

    # Assert
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert len(source.video_timestamps) == count
    await session.close()


async def test_a_pipeline_flush_returns_once_the_output_wrote_the_media_given_before_it():
    # Arrange: an output in a running pipeline given 15 ms of speech, short of pipecat's chunk of
    # 40 ms and of an Opus packet, a synchronized picture and 20 live pictures at 30 a second
    relay = FakeRelay()
    session = await open_session(relay)
    output = MoqWasmTransport(session, speaking_params()).output()
    worker = PipelineWorker(Pipeline([output]), cancel_on_idle_timeout=False)
    runner = WorkerRunner(handle_sigint=False)
    await runner.add_workers(worker)
    running = asyncio.create_task(runner.run())
    pictures = [OutputImageRawFrame(image=picture(value), size=SIZE, format="RGB") for value in range(20)]
    speech = OutputAudioRawFrame(audio=tone(0.015), sample_rate=SAMPLE_RATE, num_channels=1)
    await worker.queue_frames([speech, synchronized(255), *pictures])

    # Act
    flushed = await within(worker.flush_pipeline())
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    written = (len(source.audio_timestamps), len(source.video_timestamps))
    await worker.queue_frame(EndFrame())
    await within(running)

    # Assert: the speech, its last chunk and packet padded with silence, and every picture were
    # written by then
    assert flushed
    assert written[0] > 0 and written[1] == 21
    await session.close()


class PassedOn(FrameProcessor):
    """Records the frames that reach it, and passes each on."""

    def __init__(self):
        super().__init__()
        self.frames: list[Frame] = []

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        await super().process_frame(frame, direction)
        self.frames.append(frame)
        await self.push_frame(frame, direction)


async def run_worker(output: FrameProcessor, *after: FrameProcessor) -> tuple[PipelineWorker, asyncio.Task]:
    """Runs a pipeline of `output`, followed by `after`; the task ends once an EndFrame queued to
    the worker ended the pipeline."""
    worker = PipelineWorker(Pipeline([output, *after]), cancel_on_idle_timeout=False)
    runner = WorkerRunner(handle_sigint=False)
    await runner.add_workers(worker)
    return worker, asyncio.create_task(runner.run())


async def test_a_pipeline_flush_shows_the_still_given_before_it_and_not_one_given_after_it_first(monkeypatch):
    # Arrange: an output that is not live, at a picture a second, in a running pipeline given 300 ms
    # of speech and a still A, then a flush, then a still B 90 ms later, while the flush probe waits
    # behind the speech
    relay = FakeRelay()
    session = await open_session(relay)
    output = MoqWasmTransport(session, speaking_params(video_out_is_live=False, video_out_framerate=1)).output()
    values = record_encoded_values(monkeypatch)
    worker, running = await run_worker(output)
    speech = OutputAudioRawFrame(audio=tone(0.3), sample_rate=SAMPLE_RATE, num_channels=1)
    await worker.queue_frames([speech, OutputImageRawFrame(image=picture(10), size=SIZE, format="RGB")])

    # Act
    flushing = asyncio.create_task(worker.flush_pipeline())
    await asyncio.sleep(0.09)
    await worker.queue_frame(OutputImageRawFrame(image=picture(20), size=SIZE, format="RGB"))
    flushed = await within(flushing)
    at_flush = list(values)
    await worker.queue_frame(EndFrame())
    await within(running)

    # Assert: A was shown by the time the flush returned, before B
    assert flushed
    assert at_flush[:1] == [10]
    assert values.index(10) < values.index(20)
    await session.close()


class AroundTheFinalFlushProbe(FrameProcessor):
    """Passes the last of the three probes of `PipelineWorker.flush_pipeline`, its second one
    downstream, on between the frames `before` and `after`."""

    def __init__(self, before: list[Frame], after: list[Frame]):
        super().__init__()
        self._before = before
        self._after = after
        self._downstream_probes = 0

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        await super().process_frame(frame, direction)
        final = False
        if isinstance(frame, PipelineFlushFrame) and direction == FrameDirection.DOWNSTREAM:
            self._downstream_probes += 1
            final = self._downstream_probes == 2
        for before in self._before if final else []:
            await self.push_frame(before)
        await self.push_frame(frame, direction)
        for after in self._after if final else []:
            await self.push_frame(after)


async def test_a_pipeline_flush_leaves_a_still_given_after_its_final_probe_to_the_video_task(monkeypatch):
    # Arrange: an output that is not live, at a picture a second, in a running pipeline given a
    # still A, then a flush whose final probe waits behind 100 ms of speech, then a still B whose
    # encode takes 1.2 s
    relay = FakeRelay()
    session = await open_session(relay)
    output = MoqWasmTransport(session, speaking_params(video_out_is_live=False, video_out_framerate=1)).output()
    encode = VideoEncoder.encode

    def slow_for_b(self, image: bytes, size, color_format: str) -> list[bytes]:
        if image[0] == 20:
            time.sleep(1.2)
        return encode(self, image, size, color_format)

    monkeypatch.setattr(VideoEncoder, "encode", slow_for_b)
    speech = OutputAudioRawFrame(audio=tone(0.1), sample_rate=SAMPLE_RATE, num_channels=1)
    later = OutputImageRawFrame(image=picture(20), size=SIZE, format="RGB")
    worker = PipelineWorker(
        Pipeline([AroundTheFinalFlushProbe([speech], [later]), output]), cancel_on_idle_timeout=False
    )
    runner = WorkerRunner(handle_sigint=False)
    await runner.add_workers(worker)
    running = asyncio.create_task(runner.run())
    await worker.queue_frame(OutputImageRawFrame(image=picture(10), size=SIZE, format="RGB"))

    # Act
    started = time.monotonic()
    flushed = await within(worker.flush_pipeline())
    flush_seconds = time.monotonic() - started
    await worker.queue_frame(EndFrame())
    await within(running)

    # Assert: the flush did not wait for B's encode, as B was given after its final probe
    assert flushed
    assert flush_seconds < 0.7
    await session.close()


async def test_a_pipeline_flush_waits_only_for_the_live_pictures_given_before_it():
    # Arrange: a live output at 10 pictures a second in a running pipeline, given pictures at 20 a
    # second for 300 ms before a flush and on while it waits
    relay = FakeRelay()
    session = await open_session(relay)
    output = MoqWasmTransport(session, speaking_params(video_out_framerate=10)).output()
    _, up = link_recorders(output)
    worker, running = await run_worker(output)
    producing = True

    async def produce() -> None:
        for value in range(80):
            if not producing:
                return
            await worker.queue_frame(OutputImageRawFrame(image=picture(value), size=SIZE, format="RGB"))
            await asyncio.sleep(0.05)

    producer = asyncio.create_task(produce())
    await asyncio.sleep(0.3)

    # Act: each of the flush's two downstream legs would wait three seconds for the queue to empty
    started = time.monotonic()
    flushed = await asyncio.wait_for(worker.flush_pipeline(), 2 * LIVE_VIDEO_DRAIN_TIMEOUT_SECONDS + TIMEOUT_SECONDS)
    elapsed = time.monotonic() - started
    producing = False
    await within(producer)
    await worker.queue_frame(EndFrame())
    await within(running)

    # Assert: the flush returned once the few pictures queued before it were written, well before
    # the three seconds it waits at most, without reporting pictures not written
    assert flushed
    assert elapsed < 1.5
    assert not [frame for frame in up.frames if isinstance(frame, ErrorFrame) and "not written" in frame.error]
    await session.close()


async def test_an_output_whose_speech_paused_writes_the_partial_chunk_the_pause_left_at_its_stop_frame(monkeypatch):
    # Arrange: pipecat's chunks of 40 ms, 50 ms of speech whose last 10 ms are louder, and a pause
    # longer than the one after which pipecat's sender takes the bot as no longer speaking
    monkeypatch.setattr(base_output, "BOT_VAD_STOP_FALLBACK_SECS", 0.2)
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM, audio_out_enabled=True, audio_out_sample_rate=SAMPLE_RATE, audio_out_end_silence_secs=0
    )
    output = MoqWasmTransport(session, params).output()
    quiet = (sine(0.04, SAMPLE_RATE) // 10).astype(np.int16)
    speech = np.concatenate([quiet, sine(0.01, SAMPLE_RATE, start=0.04)])
    frame = SpeechOutputAudioRawFrame(audio=speech.tobytes(), sample_rate=SAMPLE_RATE, num_channels=1)

    try:
        # Act
        await within(run_test(output, frames_to_send=[frame, SleepFrame(0.5), StopFrame()], send_end_frame=False))

        # Assert: the louder 10 ms were written
        source = relay.sessions[0].media_broadcasts[0].sources[0]
        assert np.abs(decoded_speech(source)).max() > 4000
    finally:
        # The StopFrame left the output running, as pipecat keeps a stopped processor.
        await within(output.cleanup())
        await session.close()


def speaking_params(**overrides) -> MoqWasmParams:
    """The params of an output that speaks at `SAMPLE_RATE` and shows live pictures of `SIZE`."""
    params = dict(
        room=ROOM,
        audio_out_enabled=True,
        audio_out_sample_rate=SAMPLE_RATE,
        audio_out_end_silence_secs=0,
        video_out_enabled=True,
        video_out_is_live=True,
        video_out_width=SIZE[0],
        video_out_height=SIZE[1],
    )
    return MoqWasmParams(**{**params, **overrides})


def synchronized(value: int, *, interruptible: bool = True) -> OutputImageRawFrame:
    frame = OutputImageRawFrame(image=picture(value), size=SIZE, format="RGB")
    frame.sync_with_audio = True
    frame.interruptible = interruptible
    return frame


def record_encoded_values(monkeypatch) -> list[int]:
    """Records the value of each `picture(value)` the video encoders encode, in the order they are
    pushed, one packet each."""
    values: list[int] = []
    encode = VideoEncoder.encode

    def recording(self, image: bytes, size, color_format: str) -> list[bytes]:
        values.append(image[0])
        return encode(self, image, size, color_format)

    monkeypatch.setattr(VideoEncoder, "encode", recording)
    return values


@pytest.mark.parametrize("live", [True, False])
@pytest.mark.parametrize("chunks", [4, 1], ids=["partial chunk", "encoder tail"])
async def test_a_synchronized_picture_is_timestamped_where_the_speech_before_it_ends(chunks, live):
    # Arrange: 250 ms of speech, whose last 10 ms pipecat's chunk of 40 ms or, in chunks of 10 ms,
    # the Opus encoder holds as the picture comes, then a synchronized picture and 250 ms more
    relay = FakeRelay()
    session = await open_session(relay)
    transport = MoqWasmTransport(session, speaking_params(audio_out_10ms_chunks=chunks, video_out_is_live=live))
    before = OutputAudioRawFrame(audio=tone(0.25), sample_rate=SAMPLE_RATE, num_channels=1)
    after = OutputAudioRawFrame(audio=tone(0.25), sample_rate=SAMPLE_RATE, num_channels=1)

    # Act
    await within(run_test(transport.output(), frames_to_send=[before, synchronized(0), after]))

    # Assert: the picture is presented 10 ms into the 13th packet, where the speech before it ends
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert source.audio_timestamps[12] == source.audio_timestamps[0] + 12 * OPUS_FRAME_MICROS
    assert abs(source.video_timestamps[0] - (source.audio_timestamps[12] + 10_000)) <= 1_000
    await session.close()


async def test_a_synchronized_picture_after_speech_that_pauses_is_written_at_once(monkeypatch):
    # Arrange: 10 ms of speech, short of pipecat's chunk, a synchronized picture, then a pause with
    # no TTSStoppedFrame
    relay = FakeRelay()
    session = await open_session(relay)
    transport = MoqWasmTransport(session, speaking_params())
    pushed_at: list[float] = []
    push_video = FakeMediaSource.push_video

    def recording(self, data: bytes, timestamp_micros: int) -> None:
        pushed_at.append(time.monotonic())
        push_video(self, data, timestamp_micros)

    monkeypatch.setattr(FakeMediaSource, "push_video", recording)
    speech = OutputAudioRawFrame(audio=tone(0.01), sample_rate=SAMPLE_RATE, num_channels=1)
    started = time.monotonic()

    # Act
    await within(run_test(transport.output(), frames_to_send=[speech, synchronized(0), SleepFrame(0.6)]))

    # Assert: the picture was written well before the pause ended
    assert len(pushed_at) == 1
    assert pushed_at[0] - started < 0.3
    await session.close()


async def test_a_synchronized_picture_carries_its_own_timestamp_past_the_live_pictures_queued(monkeypatch):
    # Arrange: a live output at 5 pictures a second given three pictures, which take it 400 ms to
    # write, then 200 ms of speech and a synchronized picture
    relay = FakeRelay()
    session = await open_session(relay)
    transport = MoqWasmTransport(session, speaking_params(video_out_framerate=5))
    values = record_encoded_values(monkeypatch)
    queued = [OutputImageRawFrame(image=picture(value), size=SIZE, format="RGB") for value in (1, 2, 3)]
    speech = OutputAudioRawFrame(audio=tone(0.2), sample_rate=SAMPLE_RATE, num_channels=1)

    # Act
    await within(run_test(transport.output(), frames_to_send=[*queued, speech, synchronized(9), SleepFrame(0.6)]))

    # Assert: the synchronized picture is presented where the speech ends, and the pictures written
    # after it follow it
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert sorted(values) == [1, 2, 3, 9] and len(source.video_timestamps) == 4
    stamped = dict(zip(values, source.video_timestamps))
    assert abs(stamped[9] - (source.audio_timestamps[0] + 200_000)) <= 1_000
    assert source.video_timestamps == sorted(set(source.video_timestamps))
    await session.close()


async def test_synchronized_pictures_kept_through_an_interruption_keep_their_order_and_timestamps(monkeypatch):
    # Arrange: uninterruptible speech and picture, interruptible speech and picture, the
    # interruption, then uninterruptible speech and picture again
    relay = FakeRelay()
    session = await open_session(relay)
    transport = MoqWasmTransport(session, speaking_params())
    values = record_encoded_values(monkeypatch)

    def speech(seconds: float, *, interruptible: bool) -> OutputAudioRawFrame:
        frame = OutputAudioRawFrame(audio=tone(seconds), sample_rate=SAMPLE_RATE, num_channels=1)
        frame.interruptible = interruptible
        return frame

    frames = [
        speech(0.2, interruptible=False),
        synchronized(1, interruptible=False),
        speech(1.0, interruptible=True),
        synchronized(2),
        SleepFrame(0.3),
        InterruptionFrame(),
        speech(0.1, interruptible=False),
        synchronized(3, interruptible=False),
    ]

    # Act
    await within(run_test(transport.output(), frames_to_send=frames))

    # Assert: the interruptible picture was dropped; the others came in order, the first where the
    # speech before it ends
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert values == [1, 3]
    assert abs(source.video_timestamps[0] - (source.audio_timestamps[0] + 200_000)) <= 1_000
    assert source.video_timestamps[0] < source.video_timestamps[1]
    await session.close()


async def test_a_synchronized_picture_released_before_an_interruption_is_passed_on_once_written():
    # Arrange: 10 ms of speech, short of pipecat's chunk, then a synchronized picture, which pipecat
    # releases at once, and an interruption
    relay = FakeRelay()
    session = await open_session(relay)
    output = MoqWasmTransport(session, speaking_params()).output()
    speech = OutputAudioRawFrame(audio=tone(0.01), sample_rate=SAMPLE_RATE, num_channels=1)
    synced = synchronized(0)

    # Act
    down, _ = await within(run_test(output, frames_to_send=[speech, synced, SleepFrame(0.2), InterruptionFrame()]))

    # Assert: the picture passed on is the one written
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert len(source.video_timestamps) == 1
    assert [frame.id for frame in down if isinstance(frame, OutputImageRawFrame)] == [synced.id]
    await session.close()


async def test_a_synchronized_picture_that_cannot_be_written_is_not_passed_on(monkeypatch):
    # Arrange: an encoder that fails
    relay = FakeRelay()
    session = await open_session(relay)
    output = MoqWasmTransport(session, speaking_params()).output()

    def failing(self, image: bytes, size, color_format: str) -> list[bytes]:
        raise RuntimeError("the encoder failed")

    monkeypatch.setattr(VideoEncoder, "encode", failing)
    speech = OutputAudioRawFrame(audio=tone(0.1), sample_rate=SAMPLE_RATE, num_channels=1)

    # Act
    down, up = await within(run_test(output, frames_to_send=[speech, synchronized(0), SleepFrame(0.2)]))

    # Assert: the failure was reported and the picture not passed on
    assert [frame for frame in down if isinstance(frame, OutputImageRawFrame)] == []
    assert any("the encoder failed" in frame.error for frame in up if isinstance(frame, ErrorFrame))
    await session.close()


@pytest.mark.parametrize("frame_seconds", [0.02, 0.04])
async def test_pictures_synchronized_with_continuous_speech_do_not_lengthen_it(frame_seconds):
    # Arrange: 200 ms of speech in frames of 20 or 40 ms, each followed by a synchronized picture
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM,
        audio_out_enabled=True,
        audio_out_sample_rate=SAMPLE_RATE,
        audio_out_end_silence_secs=0,
        video_out_enabled=True,
        video_out_is_live=True,
        video_out_width=SIZE[0],
        video_out_height=SIZE[1],
    )
    transport = MoqWasmTransport(session, params)
    count = round(0.2 / frame_seconds)
    frames = []
    for index in range(count):
        frames.append(OutputAudioRawFrame(audio=tone(frame_seconds), sample_rate=SAMPLE_RATE, num_channels=1))
        synced = OutputImageRawFrame(image=picture(index), size=SIZE, format="RGB")
        synced.sync_with_audio = True
        frames.append(synced)

    # Act
    await within(run_test(transport.output(), frames_to_send=frames))

    # Assert: the Opus packets last as long as the speech, give or take a packet, and every picture
    # was written
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert abs(len(source.audio_timestamps) * OPUS_FRAME_MICROS - 200_000) <= OPUS_FRAME_MICROS
    assert [call[0] for call in source.calls].count("push_video") == count
    await session.close()


async def test_transport_messages_with_a_presentation_timestamp_are_written_as_pipecat_presents_them():
    # Arrange: a track object and an event record, each presented 1 ns after the pipeline started
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    params = MoqWasmParams(room=ROOM, data_out_tracks={"status": 60}, event_timelines_out={"events": 60})
    transport = MoqWasmTransport(session, params)
    messages = [
        OutputTransportMessageFrame(message=MoqWasmTrackObject("status", b"ready")),
        OutputTransportMessageFrame(message=MoqWasmEventRecord("events", {"L": [1, 2], "data": "yes"})),
    ]
    for message in messages:
        message.pts = 1

    # Act
    down, _ = await within(run_test(transport.output(), frames_to_send=messages))

    # Assert: each was written once and passed on
    assert native.writers["status"].objects == [(b"ready", True)]
    assert [json.loads(payload) for payload, _ in native.writers["events"].objects] == [[], [{"L": [1, 2], "data": "yes"}]]
    assert [frame.id for frame in down] == [message.id for message in messages]
    await session.close()


def track_object_presented_at(seconds: float) -> OutputTransportMessageFrame:
    """A track object on the `status` track presented `seconds` after the pipeline started."""
    message = OutputTransportMessageFrame(message=MoqWasmTrackObject("status", b"ready"))
    message.pts = round(seconds * 1_000_000_000)
    return message


async def test_a_timestamped_transport_message_is_written_before_a_stop_frame_that_comes_before_it_is_due():
    # Arrange: a track object presented 600 ms after the pipeline started, then a StopFrame at once
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    transport = MoqWasmTransport(session, MoqWasmParams(room=ROOM, data_out_tracks={"status": 60}))
    output = transport.output()

    # Act: the next pipeline resumes the output, which replaces its media senders
    await within(run_test(output, frames_to_send=[track_object_presented_at(0.6), StopFrame()], send_end_frame=False))
    await within(run_test(transport.output(), frames_to_send=[SleepFrame(0.1)]))

    # Assert
    assert native.writers["status"].objects == [(b"ready", True)]
    await session.close()


async def test_a_pipeline_flush_returns_once_the_output_wrote_a_timestamped_message_given_before_it():
    # Arrange: an output in a running pipeline given a track object presented 600 ms after the
    # pipeline started
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    output = MoqWasmTransport(session, MoqWasmParams(room=ROOM, data_out_tracks={"status": 60})).output()
    worker = PipelineWorker(Pipeline([output]), cancel_on_idle_timeout=False)
    runner = WorkerRunner(handle_sigint=False)
    await runner.add_workers(worker)
    running = asyncio.create_task(runner.run())
    await worker.queue_frame(track_object_presented_at(0.6))

    # Act
    flushed = await within(worker.flush_pipeline())
    written = list(native.writers["status"].objects)
    await worker.queue_frame(EndFrame())
    await within(running)

    # Assert
    assert flushed
    assert written == [(b"ready", True)]
    await session.close()


async def test_a_timestamped_transport_message_for_another_destination_is_passed_on_unwritten():
    # Arrange: a track object for a destination the transport has no sender for, presented in 60 s
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    params = MoqWasmParams(room=ROOM, data_out_tracks={"status": 60})
    transport = MoqWasmTransport(session, params)
    message = OutputTransportMessageFrame(message=MoqWasmTrackObject("status", b"ready"))
    message.transport_destination = "other"
    message.pts = 60_000_000_000

    # Act
    down, _ = await within(run_test(transport.output(), frames_to_send=[message]))

    # Assert
    assert native.writers["status"].objects == []
    assert [frame.id for frame in down] == [message.id]
    await session.close()


def presented_at(message: OutputTransportMessageFrame, seconds: float) -> OutputTransportMessageFrame:
    message.pts = round(seconds * 1_000_000_000)
    return message


async def test_timed_and_untimed_transport_messages_behind_speech_are_written_once_in_frame_order():
    # Arrange: 300 ms of speech, then an untimed event record and one presented 1 ns after the
    # pipeline started, while pipecat's audio task still writes the speech
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    transport = MoqWasmTransport(session, speaking_params(video_out_enabled=False, event_timelines_out={"events": 60}))
    speech = OutputAudioRawFrame(audio=tone(0.3), sample_rate=SAMPLE_RATE, num_channels=1)
    draft = OutputTransportMessageFrame(message=MoqWasmEventRecord("events", {"data": "draft"}))
    final = presented_at(OutputTransportMessageFrame(message=MoqWasmEventRecord("events", {"data": "final"})), 1e-9)

    # Act
    down, _ = await within(run_test(transport.output(), frames_to_send=[speech, draft, final]))

    # Assert: each record was written once, in frame order, and passed on once
    records = [json.loads(payload) for payload, _ in native.writers["events"].objects]
    assert records == [[], [{"data": "draft"}], [{"data": "final"}]]
    assert sorted(frame.id for frame in down if isinstance(frame, OutputTransportMessageFrame)) == sorted([draft.id, final.id])
    await session.close()


async def test_a_timestamped_object_continues_the_group_an_untimed_one_behind_speech_opened_through_a_relay(relay_url):
    # Arrange: a bot speaking in a room, whose transcript track a listener subscribed to
    room = ["anon", f"room-{uuid.uuid4().hex[:8]}"]
    bot_session = await moq_wasm.connect(relay_url, verify_certificate=False)
    listener = await moq_wasm.connect(relay_url, verify_certificate=False)
    params = speaking_params(room=room, video_out_enabled=False, data_out_tracks={"transcript": 60})
    worker, running = await run_worker(MoqWasmTransport(bot_session, params).output())

    async def subscribed():
        while True:
            try:
                return await listener.subscribe([*room, "bot"], "transcript")
            except moq_wasm.RequestError:
                await asyncio.sleep(0.05)

    objects = await within(subscribed())
    speech = OutputAudioRawFrame(audio=tone(0.3), sample_rate=SAMPLE_RATE, num_channels=1)
    opener = OutputTransportMessageFrame(message=MoqWasmTrackObject("transcript", b"draft", new_group=True))
    continuation = OutputTransportMessageFrame(message=MoqWasmTrackObject("transcript", b"final", new_group=False))

    try:
        # Act
        await worker.queue_frames([speech, opener, presented_at(continuation, 1e-9)])
        received = [await within(objects.__anext__()) for _ in range(2)]

        # Assert: the continuation follows the opener in its group
        assert [(item.payload, item.group_id) for item in received] == [
            (b"draft", received[0].group_id),
            (b"final", received[0].group_id),
        ]
    finally:
        await worker.queue_frame(EndFrame())
        await within(running)
        for each in (listener, bot_session):
            await each.close()


async def test_frames_for_another_destination_go_on_as_they_came_and_write_nothing():
    # Arrange: an urgent track object and speech at another sample rate, both for another
    # destination, then the output's own speech
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    params = MoqWasmParams(
        room=ROOM,
        audio_out_enabled=True,
        audio_out_sample_rate=SAMPLE_RATE,
        audio_out_end_silence_secs=0,
        data_out_tracks={"status": 60},
    )
    transport = MoqWasmTransport(session, params)
    urgent = OutputTransportMessageUrgentFrame(message=MoqWasmTrackObject("status", b"elsewhere"))
    foreign_audio = tone(0.1, 48_000)
    foreign = OutputAudioRawFrame(audio=foreign_audio, sample_rate=48_000, num_channels=1)
    for frame in (urgent, foreign):
        frame.transport_destination = "other"

    # Act
    down, _ = await within(run_test(transport.output(), frames_to_send=[urgent, foreign, SleepFrame(0.2)]))

    # Assert: both went on untouched, and the output wrote neither
    source = native.media_broadcasts[0].sources[0]
    assert native.writers["status"].objects == []
    assert source.audio_timestamps == []
    passed = {frame.id: frame for frame in down}
    assert urgent.id in passed
    assert passed[foreign.id] is foreign and foreign.audio == foreign_audio and foreign.sample_rate == 48_000
    await session.close()


async def test_a_tts_stopped_frame_for_another_destination_leaves_the_outputs_speech_going():
    # Arrange: 30 ms of speech in chunks of 10 ms, of which the Opus encoder holds 10 ms once it
    # wrote a packet, then 10 ms more speech, with and without a TTSStoppedFrame for another
    # destination between them
    foreign = TTSStoppedFrame()
    foreign.transport_destination = "other"

    async def packets(*between: Frame) -> tuple[int, list[Frame]]:
        relay = FakeRelay()
        session = await open_session(relay)
        transport = MoqWasmTransport(session, speaking_params(audio_out_10ms_chunks=1, video_out_enabled=False))
        first = OutputAudioRawFrame(audio=tone(0.03), sample_rate=SAMPLE_RATE, num_channels=1)
        second = OutputAudioRawFrame(audio=tone(0.01), sample_rate=SAMPLE_RATE, num_channels=1)
        down, _ = await within(run_test(transport.output(), frames_to_send=[first, SleepFrame(0.1), *between, second]))
        await session.close()
        return len(relay.sessions[0].media_broadcasts[0].sources[0].audio_timestamps), down

    # Act
    alone, _ = await packets()
    count, down = await packets(foreign)

    # Assert: the foreign frame padded no packet of the speech, and went on
    assert count == alone
    assert foreign.id in [frame.id for frame in down]


async def test_a_cancel_frame_for_another_destination_leaves_the_output_running():
    # Arrange: an output queued frames as in a pipeline, without the processors around it, which
    # pipecat cancels with any CancelFrame; then a CancelFrame for another destination
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM, video_out_enabled=True, video_out_is_live=True, video_out_width=SIZE[0], video_out_height=SIZE[1]
    )
    output = MoqWasmTransport(session, params).output()
    _, down = link_recorders(output)
    setup = FrameProcessorSetup(clock=SystemClock(), task_manager=TaskManager(), pipeline_worker=SimpleNamespace())
    await output.setup(setup)
    await output.queue_frame(StartFrame())
    await until(lambda: None in output._media_senders)
    foreign = CancelFrame()
    foreign.transport_destination = "other"

    # Act
    await output.queue_frame(foreign)
    await output.queue_frame(OutputImageRawFrame(image=picture(0), size=SIZE, format="RGB"))

    # Assert: the output wrote the picture after it, and the foreign frame went on
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    await until(lambda: len(source.video_timestamps) == 1)
    assert foreign.id in [frame.id for frame in down.frames]
    await within(output.cleanup())
    await session.close()


async def test_an_interruption_waiting_for_an_uninterruptible_picture_writes_no_more_interruptible_speech(monkeypatch):
    # Arrange: a live output whose encodes take 300 ms, given a second of interruptible speech and
    # an uninterruptible picture, then an interruption while the picture is encoded
    relay = FakeRelay()
    session = await open_session(relay)
    transport = MoqWasmTransport(session, speaking_params())
    slow_encodes(monkeypatch, 0.3)
    interrupted_at: list[float] = []
    pushed_at: list[float] = []
    interrupt_audio = Room.interrupt_audio
    push_audio = FakeMediaSource.push_audio

    def interrupting(self) -> None:
        interrupted_at.append(time.monotonic())
        interrupt_audio(self)

    def pushing(self, data: bytes, timestamp_micros: int, **level) -> None:
        pushed_at.append(time.monotonic())
        push_audio(self, data, timestamp_micros, **level)

    monkeypatch.setattr(Room, "interrupt_audio", interrupting)
    monkeypatch.setattr(FakeMediaSource, "push_audio", pushing)
    speech = OutputAudioRawFrame(audio=tone(1.0), sample_rate=SAMPLE_RATE, num_channels=1)
    protected = OutputImageRawFrame(image=picture(0), size=SIZE, format="RGB")
    protected.interruptible = False

    # Act
    await within(
        run_test(transport.output(), frames_to_send=[speech, protected, SleepFrame(0.15), InterruptionFrame(), SleepFrame(0.4)])
    )

    # Assert: the picture was written, and of the speech after the interruption, at most the 40 ms
    # chunk pipecat's sender may have taken as it came, two packets
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert len(source.video_timestamps) == 1
    assert len(interrupted_at) == 1
    assert len([at for at in pushed_at if at > interrupted_at[0]]) <= 2
    await session.close()


def assistant_image(value: int) -> AssistantImageRawFrame:
    return AssistantImageRawFrame(image=picture(value), size=SIZE, format="RGB")


async def test_a_live_output_writes_each_assistant_image_and_passes_it_on_once_in_order():
    # Arrange: a live output at 10 pictures a second, which queues 20 live pictures at most, given
    # 40 assistant images at once
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM,
        video_out_enabled=True,
        video_out_is_live=True,
        video_out_framerate=10,
        video_out_width=SIZE[0],
        video_out_height=SIZE[1],
    )
    transport = MoqWasmTransport(session, params)
    images = [assistant_image(value) for value in range(40)]

    # Act
    down, _ = await within(run_test(transport.output(), frames_to_send=images))

    # Assert
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert len(source.video_timestamps) == 40
    assert [frame.id for frame in down if isinstance(frame, AssistantImageRawFrame)] == [image.id for image in images]
    await session.close()


@pytest.mark.parametrize("live", [True, False])
async def test_an_assistant_image_that_cannot_be_written_is_not_passed_on(monkeypatch, live):
    # Arrange: an encoder that fails
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(
        room=ROOM, video_out_enabled=True, video_out_is_live=live, video_out_width=SIZE[0], video_out_height=SIZE[1]
    )
    transport = MoqWasmTransport(session, params)

    def failing(self, image: bytes, size, color_format: str) -> list[bytes]:
        raise RuntimeError("the encoder failed")

    monkeypatch.setattr(VideoEncoder, "encode", failing)

    # Act
    down, up = await within(run_test(transport.output(), frames_to_send=[assistant_image(0), SleepFrame(0.2)]))

    # Assert: the failure was reported and the image not passed on
    assert [frame for frame in down if isinstance(frame, AssistantImageRawFrame)] == []
    assert any("the encoder failed" in frame.error for frame in up if isinstance(frame, ErrorFrame))
    await session.close()


async def test_a_still_assistant_image_is_passed_on_once_however_often_it_is_written():
    # Arrange: an output that is not live, which writes its still at each of its 30 frames a second
    relay = FakeRelay()
    session = await open_session(relay)
    params = MoqWasmParams(room=ROOM, video_out_enabled=True, video_out_width=SIZE[0], video_out_height=SIZE[1])
    transport = MoqWasmTransport(session, params)
    still = assistant_image(0)

    # Act
    down, _ = await within(run_test(transport.output(), frames_to_send=[still, SleepFrame(0.2)]))

    # Assert
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert len(source.video_timestamps) > 1
    assert [frame.id for frame in down if isinstance(frame, AssistantImageRawFrame)] == [still.id]
    await session.close()


def assistant_image_frames(kind: str) -> tuple[MoqWasmParams, list[Frame], AssistantImageRawFrame]:
    """The params of an output that speaks, the frames giving it an assistant image of `kind`, a
    `still` of an output that is not live, a `live` picture or one `synchronized` after 100 ms of
    speech, and that image."""
    image = assistant_image(0)
    image.sync_with_audio = kind == "synchronized"
    frames: list[Frame] = [image]
    if kind == "synchronized":
        frames.insert(0, OutputAudioRawFrame(audio=tone(0.1), sample_rate=SAMPLE_RATE, num_channels=1))
    return speaking_params(video_out_is_live=kind == "live"), frames, image


@pytest.mark.parametrize("kind", ["still", "live", "synchronized"])
async def test_an_assistant_image_given_before_a_context_reset_is_not_in_the_context_after_it(kind):
    # Arrange: the assistant's context aggregator after the output, as in a pipeline
    relay = FakeRelay()
    session = await open_session(relay)
    params, frames, _ = assistant_image_frames(kind)
    context = LLMContext()
    pipeline = Pipeline([MoqWasmTransport(session, params).output(), LLMAssistantAggregator(context)])
    reset = [{"role": "system", "content": "start over"}]

    # Act
    await within(
        run_test(pipeline, frames_to_send=[*frames, LLMMessagesUpdateFrame(messages=reset, run_llm=False), SleepFrame(0.2)])
    )

    # Assert
    assert context.get_messages() == reset
    await session.close()


@pytest.mark.parametrize("kind", ["still", "live", "synchronized"])
async def test_an_assistant_image_is_passed_on_ahead_of_the_frames_given_after_it(kind):
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    params, frames, image = assistant_image_frames(kind)

    # Act
    down, _ = await within(
        run_test(MoqWasmTransport(session, params).output(), frames_to_send=[*frames, LLMFullResponseEndFrame()])
    )

    # Assert
    passed = [frame for frame in down if isinstance(frame, (AssistantImageRawFrame, LLMFullResponseEndFrame))]
    assert [frame.id for frame in passed][:1] == [image.id]
    assert [type(frame) for frame in passed] == [AssistantImageRawFrame, LLMFullResponseEndFrame]
    await session.close()


@pytest.mark.parametrize("kind", ["synchronized", "sprite"])
async def test_an_assistant_image_an_output_that_is_not_live_draws_again_is_passed_on_at_most_once(kind):
    # Arrange: an output that is not live, which draws its still at each of its 30 frames a second,
    # given a synchronized assistant image or a sprite of two
    relay = FakeRelay()
    session = await open_session(relay)
    params, frames, image = assistant_image_frames("synchronized")
    if kind == "sprite":
        frames = [SpriteFrame(images=[assistant_image(0), assistant_image(255)])]
    output = MoqWasmTransport(session, params.model_copy(update={"video_out_is_live": False})).output()

    # Act
    down, _ = await within(run_test(output, frames_to_send=[*frames, SleepFrame(0.25)]))

    # Assert: pipecat passes a sprite's pictures on not at all
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    passed = [frame.id for frame in down if isinstance(frame, AssistantImageRawFrame)]
    assert len(source.video_timestamps) > 2
    assert passed == ([image.id] if kind == "synchronized" else [])
    await session.close()


@pytest.mark.parametrize("interruptible", [True, False])
@pytest.mark.parametrize("live", [True, False])
async def test_an_assistant_image_is_shown_if_and_only_if_it_reaches_the_context_across_an_interruption(
    live, interruptible
):
    # Arrange: the assistant's context aggregator after an output given a second of speech, then an
    # assistant image, then an interruption 100 ms later, before the speech before the image was
    # written
    relay = FakeRelay()
    session = await open_session(relay)
    context = LLMContext()
    output = MoqWasmTransport(session, speaking_params(video_out_is_live=live)).output()
    pipeline = Pipeline([output, LLMAssistantAggregator(context)])
    speech = OutputAudioRawFrame(audio=tone(1.0), sample_rate=SAMPLE_RATE, num_channels=1)
    speech.interruptible = interruptible
    image = assistant_image(0)
    image.interruptible = interruptible

    # Act
    await within(
        run_test(pipeline, frames_to_send=[speech, image, SleepFrame(0.1), InterruptionFrame(), SleepFrame(1.2)])
    )

    # Assert: an interruptible image is dropped from both, an uninterruptible one reaches both
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    shown = len(source.video_timestamps) > 0
    in_context = any(message["role"] == "assistant" for message in context.get_messages())
    assert (shown, in_context) == (not interruptible, not interruptible)
    await session.close()


@pytest.mark.parametrize("ending", ["flush", "end"])
@pytest.mark.parametrize("synchronized", [False, True])
async def test_an_assistant_image_an_output_that_is_not_live_reaches_after_a_later_still_leaves_that_still_last(
    monkeypatch, synchronized, ending
):
    # Arrange: an output that is not live, at a picture a second, given 1.5 s of speech, an
    # assistant image behind it, synchronized with the audio or not, then a still at once, which
    # its video task draws a second in, before pipecat's audio task reaches the image
    relay = FakeRelay()
    session = await open_session(relay)
    output = MoqWasmTransport(session, speaking_params(video_out_is_live=False, video_out_framerate=1)).output()
    values = record_encoded_values(monkeypatch)
    speech = OutputAudioRawFrame(audio=tone(1.5), sample_rate=SAMPLE_RATE, num_channels=1)
    image = assistant_image(10)
    image.sync_with_audio = synchronized
    later = OutputImageRawFrame(image=picture(20), size=SIZE, format="RGB")

    # Act: a pipeline flush, or an EndFrame, right behind them
    passed_on = PassedOn()
    worker, running = await run_worker(output, passed_on)
    await worker.queue_frames([speech, image, later])
    if ending == "flush":
        flushed = await within(worker.flush_pipeline())
        last = values[-1]
        await worker.queue_frame(EndFrame())
        await within(running)
    else:
        await worker.queue_frame(EndFrame())
        await within(running)
        flushed, last = True, values[-1]

    # Assert: the image was written, then the still given after it, which was written last, and the
    # image was passed on once
    assert flushed
    assert 10 in values and last == 20
    assert [frame.id for frame in passed_on.frames if isinstance(frame, AssistantImageRawFrame)] == [image.id]
    await session.close()


async def test_an_assistant_image_drawn_before_the_next_one_queued_is_shown_until_an_interruption_drops_that_one(
    monkeypatch,
):
    # Arrange: an output that is not live showing a still, given 100 ms of speech, an assistant
    # image A, a second of speech and an assistant image B, then an interruption 400 ms later,
    # while the speech before B is being written
    relay = FakeRelay()
    session = await open_session(relay)
    output = MoqWasmTransport(session, speaking_params(video_out_is_live=False)).output()
    values = record_encoded_values(monkeypatch)
    slide = OutputImageRawFrame(image=picture(5), size=SIZE, format="RGB")
    short = OutputAudioRawFrame(audio=tone(0.1), sample_rate=SAMPLE_RATE, num_channels=1)
    long = OutputAudioRawFrame(audio=tone(1.0), sample_rate=SAMPLE_RATE, num_channels=1)
    frames = [slide, short, assistant_image(10), long, assistant_image(20)]

    # Act
    await within(run_test(output, frames_to_send=[*frames, SleepFrame(0.4), InterruptionFrame(), SleepFrame(0.3)]))

    # Assert: A was the still from its draw on, and B, dropped, was never drawn
    assert 10 in values and 20 not in values
    assert set(values[values.index(10) :]) == {10}
    await session.close()


async def test_synchronized_pictures_slow_to_encode_do_not_hold_up_the_speech(monkeypatch, virtual_room_clock):
    # Arrange: a live output whose encodes take 100 ms, given 600 ms of speech in frames of 30 ms,
    # each followed by a synchronized picture, with no drain deadline but the test's
    relay = FakeRelay()
    session = await open_session(relay)
    transport = MoqWasmTransport(session, speaking_params())
    slow_encodes(monkeypatch, 0.1)
    monkeypatch.setattr(_transport, "LIVE_VIDEO_DRAIN_TIMEOUT_SECONDS", TIMEOUT_SECONDS)
    pushed_at: list[float] = []
    push_audio = FakeMediaSource.push_audio

    def pushing(self, data: bytes, timestamp_micros: int, **level) -> None:
        pushed_at.append(virtual_room_clock.time())
        push_audio(self, data, timestamp_micros, **level)

    monkeypatch.setattr(FakeMediaSource, "push_audio", pushing)
    frames = []
    for value in range(20):
        frames.append(OutputAudioRawFrame(audio=tone(0.03), sample_rate=SAMPLE_RATE, num_channels=1))
        frames.append(synchronized(value))

    # Act
    await within(run_test(transport.output(), frames_to_send=frames))

    # Assert: the speech went out in about its own 600 ms, not after the two seconds of encodes,
    # and every picture was written
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert pushed_at[-1] - pushed_at[0] < 1.0
    assert len(source.video_timestamps) == 20
    await session.close()


async def test_synchronized_pictures_past_two_seconds_of_them_waiting_are_dropped(monkeypatch, virtual_room_clock):
    # Arrange: a live output at 10 pictures a second, which holds 20 synchronized pictures at most,
    # whose encodes take 100 ms, given 900 ms of speech in frames of 30 ms, each followed by a
    # synchronized picture, with no drain deadline but the test's
    relay = FakeRelay()
    session = await open_session(relay)
    transport = MoqWasmTransport(session, speaking_params(video_out_framerate=10))
    slow_encodes(monkeypatch, 0.1)
    monkeypatch.setattr(_transport, "LIVE_VIDEO_DRAIN_TIMEOUT_SECONDS", TIMEOUT_SECONDS)
    values = record_encoded_values(monkeypatch)
    frames = []
    for value in range(30):
        frames.append(OutputAudioRawFrame(audio=tone(0.03), sample_rate=SAMPLE_RATE, num_channels=1))
        frames.append(synchronized(value))

    # Act
    _, up = await within(run_test(transport.output(), frames_to_send=frames))

    # Assert: the pictures past the 20 waiting were dropped, which was reported once
    errors = [frame for frame in up if isinstance(frame, ErrorFrame)]
    assert 20 <= len(values) <= 21 and values == list(range(len(values)))
    assert len(errors) == 1 and "synchronized pictures" in errors[0].error
    await session.close()


def alive_frames(ids: set[int]) -> list[Frame]:
    """The frames, or copies of them, with one of `ids` that are still referenced."""
    gc.collect()
    return [frame for frame in gc.get_objects() if isinstance(frame, Frame) and frame.id in ids]


@pytest.mark.parametrize("video", [True, False])
async def test_an_output_keeps_no_assistant_image_once_drawn_passed_on_or_dropped_by_an_interruption(video):
    # Arrange: an output, with or without video, given six assistant images behind 100 ms of
    # speech, then twice six behind a second of speech each interrupted 100 ms later
    relay = FakeRelay()
    session = await open_session(relay)
    output = MoqWasmTransport(session, speaking_params(video_out_enabled=video, video_out_is_live=False)).output()
    frames: list[Frame] = [OutputAudioRawFrame(audio=tone(0.1), sample_rate=SAMPLE_RATE, num_channels=1)]
    frames += [assistant_image(value) for value in range(6)]
    for turn in range(2):
        frames.append(OutputAudioRawFrame(audio=tone(1.0), sample_rate=SAMPLE_RATE, num_channels=1))
        frames += [assistant_image(10 * (turn + 1) + value) for value in range(6)]
        frames += [SleepFrame(0.1), InterruptionFrame()]
    frames.append(SleepFrame(0.3))
    ids = {frame.id for frame in frames if isinstance(frame, AssistantImageRawFrame)}

    # Act
    down, _ = await within(run_test(output, frames_to_send=frames))
    del frames, down

    # Assert: the output, still referenced, holds no image but the still it shows
    assert len(alive_frames(ids)) == (1 if video else 0)
    assert output is not None
    await session.close()


async def test_a_message_sent_with_send_message_is_written_once_and_one_reaching_the_output_once():
    # Arrange: a running output with an event timeline, given 300 ms of speech and a record behind
    # it
    relay = FakeRelay()
    session = await open_session(relay)
    output = MoqWasmTransport(session, speaking_params(video_out_enabled=False, event_timelines_out={"events": 60})).output()
    worker, running = await run_worker(output)
    speech = OutputAudioRawFrame(audio=tone(0.3), sample_rate=SAMPLE_RATE, num_channels=1)
    queued = OutputTransportMessageFrame(message=MoqWasmEventRecord("events", {"data": "queued"}))
    await worker.queue_frames([speech, queued])
    await until(lambda: len(relay.sessions[0].writers.get("events", FakeWriter()).objects) == 2)

    # Act
    await within(output.send_message(OutputTransportMessageFrame(message=MoqWasmEventRecord("events", {"data": "sent"}))))
    await worker.queue_frame(EndFrame())
    await within(running)

    # Assert
    records = [json.loads(payload) for payload, _ in relay.sessions[0].writers["events"].objects]
    assert records == [[], [{"data": "queued"}], [{"data": "sent"}]]
    await session.close()


@pytest.mark.parametrize("message_class", [OutputTransportMessageFrame, OutputTransportMessageUrgentFrame])
async def test_a_message_sent_with_send_message_for_another_destination_is_not_written(message_class):
    # Arrange: a running output with a data track, whose registration a message to it waited for
    relay = FakeRelay()
    session = await open_session(relay)
    output = MoqWasmTransport(session, speaking_params(video_out_enabled=False, data_out_tracks={"status": 60})).output()
    worker, running = await run_worker(output)
    await worker.queue_frame(OutputTransportMessageFrame(message=MoqWasmTrackObject("status", b"ready")))
    await until(lambda: relay.sessions[0].writers.get("status", FakeWriter()).objects != [])
    foreign = message_class(message=MoqWasmTrackObject("status", b"elsewhere"))
    foreign.transport_destination = "other-room"

    # Act
    await within(output.send_message(foreign))
    await within(output.send_message(message_class(message=MoqWasmTrackObject("status", b"here"))))
    await worker.queue_frame(EndFrame())
    await within(running)

    # Assert: the local message after it was written, the foreign one not
    assert [payload for payload, _ in relay.sessions[0].writers["status"].objects] == [b"ready", b"here"]
    await session.close()


@pytest.mark.parametrize("buffer", [bytearray, memoryview, lambda image: memoryview(image).cast("I")])
@pytest.mark.parametrize("live", [True, False])
async def test_a_picture_in_a_contiguous_buffer_is_written(buffer, live):
    # Arrange: an RGB picture half the output's size, in a buffer other than bytes
    relay = FakeRelay()
    session = await open_session(relay)
    params = speaking_params(audio_out_enabled=False, video_out_is_live=live)
    half = (SIZE[0] // 2, SIZE[1] // 2)
    frame = OutputImageRawFrame(image=buffer(bytes(half[0] * half[1] * 3)), size=half, format="RGB")

    # Act
    _, up = await within(run_test(MoqWasmTransport(session, params).output(), frames_to_send=[frame, SleepFrame(0.2)]))

    # Assert
    source = relay.sessions[0].media_broadcasts[0].sources[0]
    assert not [error for error in up if isinstance(error, ErrorFrame)]
    assert "push_video" in [call[0] for call in source.calls]
    await session.close()


async def test_an_assistant_image_that_cannot_be_resized_is_reported_and_the_speech_after_it_is_written(monkeypatch):
    # Arrange: an output that is not live, whose resize fails, given an assistant image half its
    # size between two 100 ms of speech
    relay = FakeRelay()
    session = await open_session(relay)
    output = MoqWasmTransport(session, speaking_params(video_out_is_live=False)).output()

    def failing(frame: OutputImageRawFrame, size: tuple[int, int]) -> OutputImageRawFrame:
        raise ValueError("the resize failed")

    monkeypatch.setattr(_transport, "_resized", failing)
    half = (SIZE[0] // 2, SIZE[1] // 2)
    image = AssistantImageRawFrame(image=bytes(half[0] * half[1] * 3), size=half, format="RGB")
    speech = [OutputAudioRawFrame(audio=tone(0.1), sample_rate=SAMPLE_RATE, num_channels=1) for _ in range(2)]

    # Act
    down, up = await within(run_test(output, frames_to_send=[speech[0], image, speech[1], SleepFrame(0.3)]))

    # Assert: the failure was reported, the image not passed on, and the speech after it written
    errors = [frame for frame in up if isinstance(frame, ErrorFrame)]
    assert len(errors) == 1 and "the resize failed" in errors[0].error and not errors[0].fatal
    assert not [frame for frame in down if isinstance(frame, AssistantImageRawFrame)]
    assert len([frame for frame in down if isinstance(frame, OutputAudioRawFrame)]) >= 2 * 100 // 40
    await session.close()
