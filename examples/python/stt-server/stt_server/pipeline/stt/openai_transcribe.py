import httpx

from ..base import PcmFormat
from .wav_file import pcm_to_wav


class OpenAiTranscribeStt:
    """OpenAI audio transcriptions endpoint: one HTTP request per utterance."""

    def __init__(self, api_key: str, language: str = "ja", model: str = "gpt-4o-transcribe") -> None:
        self.api_key = api_key
        self.language = language
        self.model = model
        self._client = httpx.AsyncClient(timeout=60)

    async def transcribe(self, utterance: bytes, pcm_format: PcmFormat) -> str:
        response = await self._client.post(
            "https://api.openai.com/v1/audio/transcriptions",
            headers={"Authorization": f"Bearer {self.api_key}"},
            data={"model": self.model, "language": self.language, "response_format": "json"},
            files={"file": ("utterance.wav", pcm_to_wav(utterance, pcm_format), "audio/wav")},
        )
        response.raise_for_status()
        return response.json().get("text", "")
