import io
import wave

import aiohttp
import numpy as np
import pytest
from aiohttp.test_utils import TestClient, TestServer

from haas_asr_server.server import SAMPLE_RATE, app


class FakeTranscriber:
    def __init__(self):
        self.audios: list[np.ndarray] = []

    async def transcribe(self, audio: np.ndarray) -> str:
        self.audios.append(audio)
        return "こんにちは"


def wav(samples: list[int], sample_rate: int = SAMPLE_RATE, channels: int = 1) -> bytes:
    """The WAV format moq-meeting-transcriber's `HttpRecognizer` writes."""
    buffer = io.BytesIO()
    with wave.open(buffer, "wb") as file:
        file.setnchannels(channels)
        file.setsampwidth(2)
        file.setframerate(sample_rate)
        file.writeframes(np.array(samples, dtype="<i2").tobytes())
    return buffer.getvalue()


def form(audio: bytes | None) -> aiohttp.FormData:
    data = aiohttp.FormData()
    if audio is not None:
        data.add_field("file", audio, filename="utterance.wav", content_type="audio/wav")
    data.add_field("model", "haas-ja")
    data.add_field("language", "ja")
    data.add_field("response_format", "json")
    return data


@pytest.fixture
async def served():
    transcriber = FakeTranscriber()
    async with TestClient(TestServer(app(transcriber))) as client:
        yield client, transcriber


async def test_health_answers_ok(served):
    # Arrange
    client, _ = served

    # Act
    response = await client.get("/health")

    # Assert
    assert response.status == 200


async def test_transcriptions_hand_the_wav_to_the_model_as_float32(served):
    # Arrange
    client, transcriber = served

    # Act
    response = await client.post("/v1/audio/transcriptions", data=form(wav([0, 16384, -32768])))

    # Assert
    assert response.status == 200
    assert await response.json() == {"text": "こんにちは"}
    [audio] = transcriber.audios
    assert audio.dtype == np.float32
    assert audio.tolist() == [0.0, 0.5, -1.0]


@pytest.mark.parametrize(
    "audio",
    [None, b"not audio", wav([0], sample_rate=48_000), wav([0, 0], channels=2)],
    ids=["no file", "unreadable", "not 16 kHz", "stereo"],
)
async def test_transcriptions_refuse_audio_the_model_cannot_take(served, audio):
    # Arrange
    client, transcriber = served

    # Act
    response = await client.post("/v1/audio/transcriptions", data=form(audio))

    # Assert
    assert response.status == 400
    assert transcriber.audios == []
