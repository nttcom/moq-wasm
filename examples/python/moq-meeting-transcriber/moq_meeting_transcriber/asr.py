import asyncio
import base64
import io
import wave

import aiohttp
from loguru import logger
from moq_bot_rooms.identity_token import IdentityToken

from moq_meeting_transcriber.audio import SAMPLE_RATE
from moq_meeting_transcriber.stt import LANGUAGE

HEALTH_INTERVAL_SECONDS = 5.0
# A health check held past this is retried, like one that fails.
HEALTH_TIMEOUT = aiohttp.ClientTimeout(total=30)
# A cold start of the GPU service holds a request until the model is loaded.
REQUEST_TIMEOUT = aiohttp.ClientTimeout(total=10 * 60)
GRANITE_SPEECH = "granite-speech-5-2b"
# The model card's prompt for speech in a known language, followed by its punctuated transcription prompt.
GRANITE_SPEECH_PROMPT = (
    "The speech is in Japanese. Transcribe it exactly; do not translate. "
    "transcribe the speech with proper punctuation and capitalization."
)


class HttpRecognizer:
    """A speech recognition service with the OpenAI-compatible transcription API, such as vLLM
    serving Qwen3-ASR; `name` is also the model name the service serves."""

    def __init__(
        self,
        name: str,
        url: str,
        session: aiohttp.ClientSession,
        identity_token: IdentityToken | None,
        health_interval_seconds: float = HEALTH_INTERVAL_SECONDS,
    ):
        self.name = name
        self._url = url.rstrip("/")
        self._session = session
        self._identity_token = identity_token
        self._health_interval_seconds = health_interval_seconds

    async def ready(self) -> None:
        while True:
            try:
                async with self._session.get(
                    f"{self._url}/health", headers=await self._headers(), timeout=HEALTH_TIMEOUT
                ) as response:
                    if response.status == 200:
                        return
                    logger.info(f"{self.name}: not ready yet ({response.status})")
            except (aiohttp.ClientError, TimeoutError) as error:
                logger.info(f"{self.name}: not ready yet ({error})")
            await asyncio.sleep(self._health_interval_seconds)

    async def recognize(self, pcm: bytes) -> str:
        form = aiohttp.FormData()
        form.add_field("file", _wav(pcm), filename="utterance.wav", content_type="audio/wav")
        form.add_field("model", self.name)
        form.add_field("language", LANGUAGE.value)
        form.add_field("response_format", "json")
        async with self._session.post(
            f"{self._url}/v1/audio/transcriptions", data=form, headers=await self._headers(), timeout=REQUEST_TIMEOUT
        ) as response:
            response.raise_for_status()
            return (await response.json())["text"]

    async def _headers(self) -> dict[str, str]:
        if self._identity_token is None:
            return {}
        return {"Authorization": f"Bearer {await self._identity_token.value()}"}


class GraniteSpeechRecognizer(HttpRecognizer):
    """Granite Speech 5.0 2b served by vLLM. vLLM's transcription API gives the model a prompt that
    names no language, so the recognizer asks through the chat API with the model card's prompt."""

    async def recognize(self, pcm: bytes) -> str:
        audio = base64.b64encode(_wav(pcm)).decode()
        content = [
            {"type": "audio_url", "audio_url": {"url": f"data:audio/wav;base64,{audio}"}},
            {"type": "text", "text": GRANITE_SPEECH_PROMPT},
        ]
        body = {"model": self.name, "temperature": 0, "messages": [{"role": "user", "content": content}]}
        async with self._session.post(
            f"{self._url}/v1/chat/completions", json=body, headers=await self._headers(), timeout=REQUEST_TIMEOUT
        ) as response:
            response.raise_for_status()
            return (await response.json())["choices"][0]["message"]["content"]


def _wav(pcm: bytes) -> bytes:
    buffer = io.BytesIO()
    with wave.open(buffer, "wb") as file:
        file.setnchannels(1)
        file.setsampwidth(2)
        file.setframerate(SAMPLE_RATE)
        file.writeframes(pcm)
    return buffer.getvalue()
