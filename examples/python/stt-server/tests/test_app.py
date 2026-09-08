import asyncio
import json
import struct

import moqt
import pytest

from stt_server.app import VoiceHub
from stt_server.audio import AudioCodec, audio_tracks_from_catalog, parse_audio_object
from stt_server.pipeline.runner import VoicePipeline
from stt_server.pipeline.vad.energy import EnergyVad
from tests.conftest import free_udp_port, opus_packets, silence, tone
from tests.test_pipeline import FakeLlm, FakeStt, FakeTts

TIMEOUT_SEC = 30
NAMESPACE = "room/alice"
CATALOG = json.dumps(
    {
        "version": 1,
        "tracks": [
            {"name": "video", "packaging": "loc", "codec": "avc1.42E01E", "role": "video"},
            {"name": "audio_64kbps", "packaging": "loc", "codec": "opus", "role": "audio", "samplerate": 48000, "channelConfig": "mono"},
        ],
    }
).encode()


@pytest.fixture
def fake_pipeline(monkeypatch):
    def build(sink):
        return VoicePipeline(EnergyVad(), FakeStt(), FakeLlm(), FakeTts(), sink)

    monkeypatch.setattr("stt_server.app.build_pipeline", build)


@pytest.fixture
async def hub_and_client(self_signed_cert, fake_pipeline):
    port = free_udp_port()
    server = moqt.listen(port, *self_signed_cert)
    hub = VoiceHub()
    serve = asyncio.ensure_future(hub.serve(server))
    client = await asyncio.wait_for(moqt.connect(f"moqt://127.0.0.1:{port}", insecure=True), TIMEOUT_SEC)
    yield hub, client
    serve.cancel()


async def wait(awaitable):
    return await asyncio.wait_for(awaitable, TIMEOUT_SEC)


def speech_packets() -> list[bytes]:
    return opus_packets(tone(1.5, sample_rate=48000) + silence(1.0, 48000))


async def write_audio(writer: moqt.TrackWriter, packets: list[bytes]) -> None:
    await writer.start_group()
    for packet in packets:
        await writer.write(packet)
    await writer.finish()


def test_catalog_lists_only_audio_tracks():
    # Act
    tracks = audio_tracks_from_catalog(NAMESPACE, CATALOG)

    # Assert
    assert [track.name for track in tracks] == ["audio_64kbps"]
    assert tracks[0].codec == AudioCodec(name="opus", sample_rate=48000, channels=1)


def test_live_ingest_payload_framing_is_unwrapped():
    # Arrange
    meta = json.dumps({"codec": "mp4a.40.2", "sampleRate": 44100, "channels": 2, "descriptionBase64": "EhA="}).encode()
    payload = struct.pack(">I", len(meta)) + meta + b"\x01\x02\x03"

    # Act
    packet = parse_audio_object(payload)

    # Assert
    assert packet.data == b"\x01\x02\x03"
    assert packet.codec == AudioCodec(name="mp4a.40.2", sample_rate=44100, channels=2, description=b"\x12\x10")


def test_bare_payload_is_a_codec_packet_without_metadata():
    # Act
    packet = parse_audio_object(b"\xf8\x78\x72")

    # Assert
    assert packet.data == b"\xf8\x78\x72" and packet.codec is None


async def read_turn(reader: moqt.TrackReader) -> tuple[list[dict], list[int]]:
    """Collects one turn's pipeline objects, up to and including its `turn` event."""
    events, groups = [], []
    while True:
        obj = await wait(reader.next_object())
        events.append(json.loads(obj.payload))
        groups.append(obj.group_id)
        if events[-1].get("stage") == "turn":
            return events, groups


def stage(events: list[dict], name: str, state: str) -> dict:
    return next(
        event for event in events if event.get("stage") == name and event.get("state") == state
    )


async def test_the_topology_is_the_first_object_of_the_pipeline_track(hub_and_client):
    # Arrange
    _hub, client = hub_and_client

    # Act
    pipeline_reader = await wait(client.subscribe(NAMESPACE, "pipeline"))
    first = await wait(pipeline_reader.next_object())
    topology = json.loads(first.payload)

    # Assert
    assert first.group_id == 0
    assert topology["type"] == "topology"
    assert [node["id"] for node in topology["nodes"]] == [
        "mic", "audio", "vad", "stt", "llm", "tts", "reply", "player",
    ]
    assert ["vad", "stt"] in topology["edges"]


async def test_a_turn_reports_every_stage_in_the_group_named_after_it(hub_and_client):
    # Arrange
    hub, client = hub_and_client
    pipeline_reader = await wait(client.subscribe(NAMESPACE, "pipeline"))
    reply_reader = await wait(client.subscribe(NAMESPACE, "reply"))
    await wait(pipeline_reader.next_object())
    catalog_writer = await wait(client.publish(NAMESPACE, "catalog"))
    await catalog_writer.write_group(CATALOG)
    audio_writer = await wait(client.publish(NAMESPACE, "audio_64kbps", first_group_id=100))

    # Act
    await write_audio(audio_writer, speech_packets())
    events, groups = await read_turn(pipeline_reader)
    reply_audio = await wait(reply_reader.next_object())

    # Assert: every object of turn 1 rides in group 1, audio included
    assert set(groups) == {1} and reply_audio.group_id == 1
    assert {event["turn"] for event in events} == {1}
    assert [(event.get("stage"), event.get("state")) for event in events if event["type"] == "turn_event"] == [
        ("vad", "done"),
        ("stt", "start"),
        ("stt", "done"),
        ("llm", "start"),
        ("llm", "done"),
        ("tts", "start"),
        ("tts", "done"),
        ("turn", "done"),
    ]
    vad, transcript = stage(events, "vad", "done"), stage(events, "stt", "done")
    assert vad["detail"]["audio"]["group_id"] == 100 and vad["detail"]["utterance_sec"] > 1.0
    assert stage(events, "llm", "done")["text"] == f"reply to '{transcript['text']}'"
    assert stage(events, "turn", "done")["elapsed_ms"] > 0
    announcement = next(event for event in events if event["type"] == "reply_audio")
    assert announcement["packets"] > 0 and len(reply_audio.payload) > 0
    assert hub.stats()["subscribers"] == {f"{NAMESPACE}/pipeline": 1, f"{NAMESPACE}/reply": 1}


async def test_publish_namespace_makes_the_server_subscribe_audio(hub_and_client):
    # Arrange
    hub, client = hub_and_client
    pipeline_reader = await wait(client.subscribe(NAMESPACE, "pipeline"))
    await wait(pipeline_reader.next_object())
    announce = asyncio.ensure_future(client.publish_namespace(NAMESPACE))
    writers: dict[str, moqt.TrackWriter] = {}
    while len(writers) < 2:
        request = await wait(client.next_event())
        assert isinstance(request, moqt.SubscribeRequest)
        writers[request.name] = await request.accept()
        if request.name == "catalog":
            await writers["catalog"].write_group(CATALOG)
    await wait(announce)

    # Act
    await write_audio(writers["audio_64kbps"], speech_packets())
    events, _groups = await read_turn(pipeline_reader)

    # Assert
    assert set(writers) == {"catalog", "audio_64kbps"}
    assert stage(events, "stt", "done")["text"]


async def test_other_tracks_cannot_be_subscribed(hub_and_client):
    # Arrange
    _hub, client = hub_and_client

    # Act / Assert
    with pytest.raises(RuntimeError):
        await wait(client.subscribe(NAMESPACE, "audio"))
