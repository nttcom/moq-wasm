import socket
import subprocess

import numpy as np
import pytest

from stt_server.audio import encode_opus

SAMPLE_RATE_16K = 16000
TONE_AMPLITUDE = 8000
TONE_HZ = 300


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


def tone(seconds: float, sample_rate: int = SAMPLE_RATE_16K) -> bytes:
    samples = np.arange(int(seconds * sample_rate))
    return (TONE_AMPLITUDE * np.sin(2 * np.pi * TONE_HZ * samples / sample_rate)).astype(np.int16).tobytes()


def silence(seconds: float, sample_rate: int = SAMPLE_RATE_16K) -> bytes:
    return bytes(2 * int(seconds * sample_rate))


def opus_packets(pcm_48k_mono: bytes) -> list[bytes]:
    return encode_opus(pcm_48k_mono, 48000)
