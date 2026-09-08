import asyncio
import json
import math
import socket
import struct
import subprocess
import wave

import av
import moqt
import pytest

from stt_server.app import SttHub
from stt_server.audio import AudioCodec, audio_tracks_from_catalog, parse_audio_object
from stt_server.stt.base import PcmFormat, Transcript
from stt_server.stt.wav_file import WavFileBackend

TIMEOUT_SEC = 10
NAMESPACE = "room/alice"
SAMPLE_RATE = 48000
CATALOG = json.dumps(
    {
        "version": 1,
        "tracks": [
            {"name": "video", "packaging": "loc", "codec": "avc1.42E01E", "role": "video"},
            {
                "name": "audio_64kbps",
                "packaging": "loc",
                "codec": "opus",
                "role": "audio",
                "samplerate": SAMPLE_RATE,
                "channelConfig": "mono",
            },
        ],
    }
).encode()


@pytest.fixture(scope="session")
def self_signed_cert(tmp_path_factory):
    cert_dir = tmp_path_factory.mktemp("certs")
    cert_path = cert_dir / "cert.pem"
    key_path = cert_dir / "key.pem"
    subprocess.run(
        [
            "openssl", "req", "-x509", "-nodes", "-days", "1",
            "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1",
            "-keyout", str(key_path), "-out", str(cert_path),
            "-subj", "/CN=localhost",
            "-addext", "subjectAltName=DNS:localhost,IP:127.0.0.1",
        ],
        check=True,
        capture_output=True,
    )
    return str(cert_path), str(key_path)


def free_udp_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


@pytest.fixture
def wav_backend(tmp_path, monkeypatch):
    backend = WavFileBackend(tmp_path / "audio.wav")
    monkeypatch.setattr("stt_server.app.create_backend", lambda _label: backend)
    return backend


@pytest.fixture
def hub_and_client(self_signed_cert, wav_backend):
    async def connect():
        port = free_udp_port()
        server = moqt.listen(port, *self_signed_cert)
        hub = SttHub()
        serve = asyncio.ensure_future(hub.serve(server))
        client = await asyncio.wait_for(moqt.connect(f"moqt://127.0.0.1:{port}", insecure=True), TIMEOUT_SEC)
        return hub, client, serve

    return connect


def opus_packets(seconds: float) -> list[bytes]:
    encoder = av.CodecContext.create("libopus", "w")
    encoder.sample_rate = SAMPLE_RATE
    encoder.layout = "mono"
    encoder.format = "s16"
    encoder.open()
    frame_size = encoder.frame_size or 960
    total = int(SAMPLE_RATE * seconds)
    pcm = b"".join(
        struct.pack("<h", int(0.5 * math.sin(2 * math.pi * 440 * i / SAMPLE_RATE) * 32767))
        for i in range(total)
    )
    packets = []
    for offset in range(0, len(pcm), frame_size * 2):
        chunk = pcm[offset : offset + frame_size * 2]
        frame = av.AudioFrame(format="s16", layout="mono", samples=len(chunk) // 2)
        frame.sample_rate = SAMPLE_RATE
        frame.pts = offset // 2
        frame.planes[0].update(chunk)
        packets += [bytes(packet) for packet in encoder.encode(frame)]
    packets += [bytes(packet) for packet in encoder.encode(None)]
    return packets


async def write_audio(writer: moqt.TrackWriter, packets: list[bytes]) -> None:
    await writer.start_group()
    for packet in packets:
        await writer.write(packet)
    await writer.finish()


async def wait_for_pcm(wav_backend: WavFileBackend, min_samples: int) -> None:
    deadline = asyncio.get_running_loop().time() + TIMEOUT_SEC
    while wav_backend.samples_written < min_samples:
        if asyncio.get_running_loop().time() > deadline:
            raise AssertionError(f"only {wav_backend.samples_written} samples decoded")
        await asyncio.sleep(0.05)


def wav_peak(path) -> int:
    with wave.open(str(path), "rb") as recording:
        frames = recording.readframes(recording.getnframes())
    return max(abs(sample) for sample in struct.unpack(f"<{len(frames) // 2}h", frames))


def test_catalog_lists_only_audio_tracks():
    # Act
    tracks = audio_tracks_from_catalog(NAMESPACE, CATALOG)

    # Assert
    assert [track.name for track in tracks] == ["audio_64kbps"]
    assert tracks[0].codec == AudioCodec(name="opus", sample_rate=SAMPLE_RATE, channels=1)


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


async def test_published_opus_track_is_decoded_to_pcm(hub_and_client, wav_backend):
    # Arrange
    hub, client, serve = await hub_and_client()
    catalog_writer = await asyncio.wait_for(client.publish(NAMESPACE, "catalog"), TIMEOUT_SEC)
    await catalog_writer.write_group(CATALOG)
    audio_writer = await asyncio.wait_for(client.publish(NAMESPACE, "audio_64kbps"), TIMEOUT_SEC)

    # Act
    await write_audio(audio_writer, opus_packets(seconds=1.0))
    await wait_for_pcm(wav_backend, min_samples=15000)
    client.close()
    await asyncio.wait_for(asyncio.sleep(0.5), TIMEOUT_SEC)

    # Assert
    stats = hub.stats()["tracks"][f"{NAMESPACE}/audio_64kbps"]
    assert stats["objects_received"] >= 50
    assert wav_backend.pcm_format.sample_rate == 16000
    serve.cancel()


async def test_publish_namespace_makes_the_server_subscribe_audio(hub_and_client, wav_backend):
    # Arrange
    hub, client, serve = await hub_and_client()
    announce = asyncio.ensure_future(client.publish_namespace(NAMESPACE))
    writers: dict[str, moqt.TrackWriter] = {}
    while len(writers) < 2:
        request = await asyncio.wait_for(client.next_event(), TIMEOUT_SEC)
        assert isinstance(request, moqt.SubscribeRequest)
        writers[request.name] = await request.accept()
        if request.name == "catalog":
            await writers["catalog"].write_group(CATALOG)
    await asyncio.wait_for(announce, TIMEOUT_SEC)

    # Act
    await write_audio(writers["audio_64kbps"], opus_packets(seconds=1.0))
    await wait_for_pcm(wav_backend, min_samples=15000)
    await wav_backend.close()

    # Assert
    assert set(writers) == {"catalog", "audio_64kbps"}
    assert wav_peak(wav_backend.path) > 8000
    serve.cancel()


class EchoingBackend:
    """Reports the byte count of every PCM chunk as a final transcript."""

    pcm_format = PcmFormat(16000)

    def __init__(self) -> None:
        self.on_transcript = None

    async def start(self, on_transcript) -> None:
        self.on_transcript = on_transcript

    async def send_pcm(self, pcm: bytes) -> None:
        self.on_transcript(Transcript(text=f"{len(pcm)} bytes", is_final=True))

    async def close(self) -> None:
        pass


async def test_transcripts_are_published_on_the_transcript_track(self_signed_cert, monkeypatch):
    # Arrange
    monkeypatch.setattr("stt_server.app.create_backend", lambda _label: EchoingBackend())
    port = free_udp_port()
    server = moqt.listen(port, *self_signed_cert)
    hub = SttHub()
    serve = asyncio.ensure_future(hub.serve(server))
    client = await asyncio.wait_for(moqt.connect(f"moqt://127.0.0.1:{port}", insecure=True), TIMEOUT_SEC)
    transcript_reader = await asyncio.wait_for(client.subscribe(NAMESPACE, "transcript"), TIMEOUT_SEC)
    audio_writer = await asyncio.wait_for(client.publish(NAMESPACE, "audio"), TIMEOUT_SEC)

    # Act
    await write_audio(audio_writer, opus_packets(seconds=0.5))
    received = await asyncio.wait_for(transcript_reader.next_object(), TIMEOUT_SEC)

    # Assert
    transcript = json.loads(received.payload)
    assert transcript["track"] == "audio"
    assert transcript["final"] is True
    assert transcript["text"].endswith("bytes")
    assert hub.stats()["transcript_subscribers"] == {NAMESPACE: 1}
    serve.cancel()


async def test_other_tracks_cannot_be_subscribed(hub_and_client, wav_backend):
    # Arrange
    hub, client, serve = await hub_and_client()

    # Act / Assert
    with pytest.raises(RuntimeError):
        await asyncio.wait_for(client.subscribe(NAMESPACE, "audio"), TIMEOUT_SEC)
    serve.cancel()
