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


async def test_published_audio_yields_transcript_reply_and_speech(hub_and_client):
    # Arrange
    hub, client = hub_and_client
    transcript_reader = await wait(client.subscribe(NAMESPACE, "transcript"))
    reply_reader = await wait(client.subscribe(NAMESPACE, "reply"))
    catalog_writer = await wait(client.publish(NAMESPACE, "catalog"))
    await catalog_writer.write_group(CATALOG)
    audio_writer = await wait(client.publish(NAMESPACE, "audio_64kbps"))

    # Act
    await write_audio(audio_writer, speech_packets())
    transcript = json.loads((await wait(transcript_reader.next_object())).payload)
    reply = json.loads((await wait(transcript_reader.next_object())).payload)
    reply_audio = await wait(reply_reader.next_object())

    # Assert
    assert transcript["type"] == "transcript" and transcript["track"] == "audio_64kbps"
    assert reply == {**reply, "type": "reply", "text": f"reply to '{transcript['text']}'"}
    assert reply_audio.object_id == 0 and len(reply_audio.payload) > 0
    assert hub.stats()["subscribers"] == {f"{NAMESPACE}/transcript": 1, f"{NAMESPACE}/reply": 1}


async def test_publish_namespace_makes_the_server_subscribe_audio(hub_and_client):
    # Arrange
    hub, client = hub_and_client
    transcript_reader = await wait(client.subscribe(NAMESPACE, "transcript"))
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
    transcript = json.loads((await wait(transcript_reader.next_object())).payload)

    # Assert
    assert set(writers) == {"catalog", "audio_64kbps"}
    assert transcript["type"] == "transcript"


async def test_other_tracks_cannot_be_subscribed(hub_and_client):
    # Arrange
    _hub, client = hub_and_client

    # Act / Assert
    with pytest.raises(RuntimeError):
        await wait(client.subscribe(NAMESPACE, "audio"))
