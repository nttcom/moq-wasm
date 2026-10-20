import aiohttp
import pytest
from aiohttp import web
from aiohttp.test_utils import TestServer

from moq_bot_rooms.identity_token import IdentityToken
from moq_meeting_transcriber.asr import HttpRecognizer


class FakeAsrService:
    """Answers like vLLM's OpenAI-compatible server, unhealthy for its first `cold_checks` checks."""

    def __init__(self, cold_checks: int = 0, text: str = " こんにちは "):
        self.cold_checks = cold_checks
        self.text = text
        self.health_checks = 0
        self.requests: list[dict] = []

    async def health(self, request: web.Request) -> web.Response:
        self.health_checks += 1
        return web.Response(status=503 if self.health_checks <= self.cold_checks else 200)

    async def transcribe(self, request: web.Request) -> web.Response:
        form = await request.post()
        self.requests.append(
            {
                "authorization": request.headers.get("Authorization"),
                "model": form["model"],
                "language": form["language"],
                "wav": form["file"].file.read()[:4],
            }
        )
        return web.json_response({"text": self.text})


@pytest.fixture
async def asr_service():
    service = FakeAsrService(cold_checks=2)
    app = web.Application()
    app.router.add_get("/health", service.health)
    app.router.add_post("/v1/audio/transcriptions", service.transcribe)
    async with TestServer(app) as server:
        service.url = str(server.make_url(""))
        yield service


async def static_token() -> str:
    return "identity-token"


async def test_recognize_posts_the_utterance_as_wav_with_the_identity_token(asr_service):
    # Arrange
    async with aiohttp.ClientSession() as session:
        recognizer = HttpRecognizer("qwen3-asr", asr_service.url, session, IdentityToken(static_token))

        # Act
        text = await recognizer.recognize(bytes(3200))

    # Assert
    assert text == " こんにちは "
    assert asr_service.requests == [
        {"authorization": "Bearer identity-token", "model": "qwen3-asr", "language": "ja", "wav": b"RIFF"}
    ]


async def test_ready_waits_until_the_service_answers_its_health_check(asr_service):
    # Arrange
    async with aiohttp.ClientSession() as session:
        recognizer = HttpRecognizer("qwen3-asr", asr_service.url, session, None, health_interval_seconds=0.01)

        # Act
        await recognizer.ready()

    # Assert
    assert asr_service.health_checks == 3


async def test_recognize_drops_the_times_and_speaker_tags_of_diarized_segments(asr_service):
    # Arrange
    asr_service.text = "[0.48][S01]こんにちは[1.66][2.10][S01]今日は[3.02]"
    async with aiohttp.ClientSession() as session:
        recognizer = HttpRecognizer("moss-transcribe-diarize", asr_service.url, session, None)

        # Act
        text = await recognizer.recognize(bytes(3200))

    # Assert
    assert text == "こんにちは今日は"
