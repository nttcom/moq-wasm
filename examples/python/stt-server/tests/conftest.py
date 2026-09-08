import math
import socket
import struct
import subprocess

import av
import pytest

SAMPLE_RATE_16K = 16000


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


def tone(seconds: float, sample_rate: int = SAMPLE_RATE_16K, amplitude: int = 8000, frequency: int = 300) -> bytes:
    return b"".join(
        struct.pack("<h", int(amplitude * math.sin(2 * math.pi * frequency * i / sample_rate)))
        for i in range(int(seconds * sample_rate))
    )


def silence(seconds: float, sample_rate: int = SAMPLE_RATE_16K) -> bytes:
    return bytes(2 * int(seconds * sample_rate))


def opus_packets(pcm_48k_mono: bytes) -> list[bytes]:
    encoder = av.CodecContext.create("libopus", "w")
    encoder.sample_rate = 48000
    encoder.layout = "mono"
    encoder.format = "s16"
    encoder.open()
    frame_size = encoder.frame_size or 960
    packets = []
    for offset in range(0, len(pcm_48k_mono), frame_size * 2):
        chunk = pcm_48k_mono[offset : offset + frame_size * 2]
        frame = av.AudioFrame(format="s16", layout="mono", samples=len(chunk) // 2)
        frame.sample_rate = 48000
        frame.pts = offset // 2
        frame.planes[0].update(chunk)
        packets += [bytes(packet) for packet in encoder.encode(frame)]
    packets += [bytes(packet) for packet in encoder.encode(None)]
    return packets
