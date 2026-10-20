import asyncio
import io
import re
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
# MOSS-Transcribe-Diarize writes each segment as `[start][S01]text[end]`. A participant's track
# carries one speaker, so the times and speaker tags carry nothing the transcript lacks.
SEGMENT_TAG = re.compile(r"\[(?:\d+(?:\.\d+)?|S\d+)\]")


class HttpRecognizer:
    """A speech recognition service with the OpenAI-compatible transcription API, such as vLLM
    serving Qwen3-ASR or MOSS-Transcribe-Diarize; `name` is also the model name the service serves."""

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
            return SEGMENT_TAG.sub("", (await response.json())["text"])

    async def _headers(self) -> dict[str, str]:
        if self._identity_token is None:
            return {}
        return {"Authorization": f"Bearer {await self._identity_token.value()}"}


def _wav(pcm: bytes) -> bytes:
    buffer = io.BytesIO()
    with wave.open(buffer, "wb") as file:
        file.setnchannels(1)
        file.setsampwidth(2)
        file.setframerate(SAMPLE_RATE)
        file.writeframes(pcm)
    return buffer.getvalue()
