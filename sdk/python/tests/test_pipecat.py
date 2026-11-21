import asyncio
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
import weakref
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
from moq_wasm.codecs import OpusDecoder, OpusEncoder, VideoDecoder, VideoEncoder, decode_pictures
from moq_wasm.media import RemoteBroadcast
from moq_wasm.pipecat import MoqWasmInputParams, MoqWasmParams, MoqWasmParticipantInput, MoqWasmTrack, MoqWasmTransport
from moq_wasm.pipecat import MoqWasmEventRecord, MoqWasmTrackMessage, MoqWasmTrackObject
from moq_wasm.pipecat._participant import ParticipantReceiver, ReceiveOptions
from moq_wasm.pipecat._room import Room, RoomOptions
from pipecat.frames.frames import (
    ErrorFrame,
    OutputTransportMessageFrame,
    OutputTransportMessageUrgentFrame,
    UserAudioRawFrame,
    UserImageRawFrame,
    UserImageRequestFrame,
)
from pipecat.transports.base_transport import VideoInSourceParams
from pipecat.processors.frame_processor import FrameDirection
from pipecat.tests.utils import SleepFrame, run_test

from tests.fake_native import STAY_OPEN, FakeRelay, FakeSubscription, open_session, until

pytestmark = pytest.mark.usefixtures("no_resubscribe_delay")

TIMEOUT_SECONDS = 5
ROOM = ["anon", "room"]
SAMPLE_RATE = 16_000
SIZE = (64, 48)


async def within(awaitable):
    return await asyncio.wait_for(awaitable, TIMEOUT_SECONDS)


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
        self.reported_error = asyncio.Event()

    def audio_in_sample_rate(self) -> int:
        return SAMPLE_RATE

    async def push_audio(self, pcm: bytes, sample_rate: int, participant_id: str) -> None:
        self.audio.append((pcm, sample_rate, participant_id))
        self.received.set()

    async def push_picture(self, picture) -> None:
        self.pictures.append(picture)
        self.received.set()

    async def push_message(self, message: MoqWasmTrackMessage) -> None:
        self.messages.append(message)
        self.received.set()

    async def answer_image_requests(self, requests: list[UserImageRequestFrame], error: str) -> None:
        self.unanswered.extend((request, error) for request in requests)

    async def event(self, name: str, *args: object) -> None:
        self.events.append((name, *args))
        if name == "on_error":
            self.reported_error.set()


AUDIO_CATALOG = catalog_payload({"name": "audio", "role": "audio", "packaging": "loc", "codec": "opus", "bitrate": 64000, "samplerate": 48000, "channelConfig": "mono", "isLive": True})


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


def test_the_opus_encoder_takes_pcm_at_another_rate_than_the_call_before():
    # Arrange: 200 ms of speech at 48 kHz
    encoder = OpusEncoder()
    packets = encoder.encode(tone(0.2, 48_000), 48_000)

    # Act: 100 ms of silence at 16 kHz
    packets += encoder.encode(bytes(2 * SAMPLE_RATE // 10), SAMPLE_RATE)

    # Assert: 300 ms, less the samples short of a packet
    assert len(packets) == 14


@pytest.mark.parametrize("sample_rate", [0, -16_000, 16_000.0])
def test_pcm_at_a_sample_rate_the_encoder_cannot_take_is_refused_and_the_encoder_goes_on(sample_rate):
    # Arrange
    encoder = OpusEncoder()
    with pytest.raises(ValueError, match="sample rate"):
        encoder.encode(tone(0.02), sample_rate)

    # Act: 200 ms of speech at 16 kHz
    packets = encoder.encode(tone(0.2), SAMPLE_RATE)

    # Assert: nine 20 ms packets, the rest short of a packet; the refused PCM is taken as none
    assert len(packets) == 9


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


@pytest.mark.parametrize("buffer", [bytearray, memoryview, lambda image: memoryview(image).cast("I")])
def test_the_video_encoder_reads_a_picture_in_a_contiguous_buffer_other_than_bytes(buffer):
    # Arrange
    encoder = VideoEncoder("H264", *SIZE, 30)

    # Act
    packets = encoder.encode(buffer(picture(0)), SIZE, "RGB")

    # Assert
    assert packets[0].startswith(b"\x00\x00\x00\x01\x67")


@pytest.mark.parametrize(
    ("width", "height", "framerate", "codec_string"),
    [
        (64, 48, 30, "avc1.42C00A"),
        (1280, 720, 30, "avc1.42C01F"),
        (1920, 1080, 30, "avc1.42C028"),
        (1920, 1080, 60, "avc1.42C02A"),
    ],
)
def test_the_h264_encoders_codec_string_names_the_profile_and_level_of_its_sps(width, height, framerate, codec_string):
    # Arrange
    encoder = VideoEncoder("H264", width, height, framerate)

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


async def test_a_refused_room_watch_reaches_on_error():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]

    async def refuse(prefix: list[str]):
        raise RuntimeError("NamespacePrefixOverlap")

    native.watch_namespaces = refuse
    sink = RecordingSink()
    room = Room(session, room_options(audio_in=True), sink)

    # Act
    await room.start("input")
    await within(sink.reported_error.wait())

    # Assert
    assert sink.events[0] == ("on_connected",)
    name, message, error = sink.events[1]
    assert (name, message, str(error)) == ("on_error", f"cannot watch {ROOM}", "NamespacePrefixOverlap")
    await room.stop("input")
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
    await session.close()


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
    await listener.start("input")
    await within(listener_sink.received.wait())

    # Assert
    assert listener_sink.audio[0][1:] == (SAMPLE_RATE, "speaker")
    talking.cancel()
    for each in (listener, speaker):
        await each.stop("input")
    for session in (listener_session, speaker_session):
        await session.close()


async def test_writing_speech_waits_while_it_runs_ahead_of_the_clock():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    room = Room(session, room_options(audio_out=True), RecordingSink())
    await room.start("output")
    started = time.monotonic()

    # Act
    await room.write_audio(tone(0.5), SAMPLE_RATE)

    # Assert: half a second of speech is written ahead by at most 100 ms
    assert time.monotonic() - started >= 0.35
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
    await within(until(lambda: len(by_subscription.get(2, [])) >= 4))

    # Assert: the pictures the decoders held back came as the first subscription ended, and the
    # second's group 0 was not taken as the first's
    assert len(by_subscription[1]) == 30
    assert {picture.location.group_id for picture in by_subscription[2]} == {0}
    running.cancel()
    await asyncio.gather(running, return_exceptions=True)
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
