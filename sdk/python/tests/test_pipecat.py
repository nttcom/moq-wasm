import asyncio
import json
import time
import uuid
from types import SimpleNamespace

import numpy as np
import pytest

import moq_wasm
from moq_wasm.codecs import OpusDecoder, OpusEncoder, VideoEncoder, decode_pictures
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

from tests.fake_native import STAY_OPEN, FakeRelay, open_session

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
            yield SimpleNamespace(group_id=0, object_id=object_id, payload=payload)
        for object_id, payload in list(enumerate(second))[1:]:
            yield SimpleNamespace(group_id=1, object_id=object_id, payload=payload)

    # Act
    every = [decoded async for decoded in decode_pictures(objects(), "avc1.42e01f")]
    thinned = [decoded async for decoded in decode_pictures(objects(), "avc1.42e01f", interval=60)]

    # Assert: a group joined without its keyframe decodes to nothing
    assert [(d.location.group_id, d.location.object_id) for d in every] == [(0, 0), (0, 1), (0, 2)]
    assert [(d.location.group_id, d.location.object_id) for d in thinned] == [(0, 0)]
    assert every[0].picture.width == SIZE[0]


def test_a_video_codec_other_than_h264_and_vp8_is_refused():
    # Act / Assert
    with pytest.raises(ValueError, match="H265"):
        VideoEncoder("H265", *SIZE, 30)


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
