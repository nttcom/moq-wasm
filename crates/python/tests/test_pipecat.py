import asyncio
import json
import time
import uuid

import numpy as np
import pytest

import moq_wasm
from moq_wasm.pipecat import MoqWasmParams, MoqWasmTrack, MoqWasmTransport
from moq_wasm.pipecat._codecs import OpusDecoder, OpusEncoder, VideoEncoder
from moq_wasm.pipecat._room import Room, RoomOptions

from tests.fake_native import STAY_OPEN, FakeRelay, open_session

pytestmark = pytest.mark.usefixtures("no_reconnect_delay")

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


def catalog_payload(*tracks: dict) -> bytes:
    return json.dumps({"version": "draft-01", "tracks": list(tracks)}).encode()


class RecordingSink:
    def __init__(self):
        self.audio: list[tuple[bytes, int, str]] = []
        self.pictures: list[tuple[tuple[int, int], str]] = []
        self.events: list[tuple] = []
        self.received = asyncio.Event()

    def audio_in_sample_rate(self) -> int:
        return SAMPLE_RATE

    async def push_audio(self, pcm: bytes, sample_rate: int, participant_id: str) -> None:
        self.audio.append((pcm, sample_rate, participant_id))
        self.received.set()

    async def push_picture(self, rgb: bytes, size: tuple[int, int], participant_id: str) -> None:
        self.pictures.append((size, participant_id))
        self.received.set()

    async def event(self, name: str, *args: object) -> None:
        self.events.append((name, *args))


def room_options(**overrides) -> RoomOptions:
    options = dict(
        room=ROOM,
        participant_id="bot",
        audio_in=False,
        video_in=False,
        video_in_max_fps=1000.0,
        video_in_track=None,
        audio_out=False,
        video_out=False,
        video_out_codec="H264",
        video_out_size=SIZE,
        video_out_framerate=30,
    )
    return RoomOptions(**{**options, **overrides})


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


def test_a_video_codec_other_than_h264_and_vp8_is_refused():
    # Act / Assert
    with pytest.raises(ValueError, match="H265"):
        VideoEncoder("H265", *SIZE, 30)


async def test_a_participants_audio_reaches_the_sink_with_its_participant_id():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["catalog"] = [[catalog_payload({"name": "audio", "role": "audio", "packaging": "loc", "codec": "opus", "isLive": True}), STAY_OPEN]]
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


async def test_a_participants_video_reaches_the_sink_as_pictures():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    encoder = VideoEncoder("H264", *SIZE, 30)
    frames = [packet for value in (0, 10, 20) for packet in encoder.encode(picture(value), SIZE, "RGB")]
    native.tracks["catalog"] = [[catalog_payload({"name": "video", "role": "video", "packaging": "loc", "codec": "avc1.42e01f", "isLive": True}), STAY_OPEN]]
    native.tracks["video"] = [frames]
    sink = RecordingSink()
    room = Room(session, room_options(video_in=True), sink)
    await room.start("input")

    # Act
    native.publish([*ROOM, "alice"])
    await within(sink.received.wait())

    # Assert
    assert sink.pictures[0] == (SIZE, "alice")
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
