import httpx

from ..base import PcmFormat
from .wav_file import pcm_to_wav


class DeepgramStt:
    """Deepgram pre-recorded API: one HTTP request per utterance."""

    def __init__(self, api_key: str, language: str = "ja", model: str = "nova-3") -> None:
        self.api_key = api_key
        self.language = language
        self.model = model
        self._client = httpx.AsyncClient(timeout=30)

    async def transcribe(self, utterance: bytes, pcm_format: PcmFormat) -> str:
        response = await self._client.post(
            "https://api.deepgram.com/v1/listen",
            params={"model": self.model, "language": self.language, "smart_format": "true"},
            headers={"Authorization": f"Token {self.api_key}", "Content-Type": "audio/wav"},
            content=pcm_to_wav(utterance, pcm_format),
        )
        response.raise_for_status()
        alternatives = response.json()["results"]["channels"][0]["alternatives"]
        return alternatives[0]["transcript"] if alternatives else ""
