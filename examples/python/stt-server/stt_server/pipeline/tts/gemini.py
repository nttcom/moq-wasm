from google import genai
from google.genai import types

from ..base import PcmFormat, SynthesizedSpeech

GEMINI_TTS_PCM = PcmFormat(24000)


class GeminiTts:
    """Gemini speech generation; the API returns raw 24 kHz mono PCM16."""

    def __init__(
        self,
        api_key: str,
        model: str = "gemini-2.5-flash-preview-tts",
        voice: str = "Kore",
    ) -> None:
        self._client = genai.Client(api_key=api_key)
        self.model = model
        self.voice = voice

    async def synthesize(self, text: str) -> SynthesizedSpeech:
        response = await self._client.aio.models.generate_content(
            model=self.model,
            contents=text,
            config=types.GenerateContentConfig(
                response_modalities=["AUDIO"],
                speech_config=types.SpeechConfig(
                    voice_config=types.VoiceConfig(
                        prebuilt_voice_config=types.PrebuiltVoiceConfig(voice_name=self.voice)
                    )
                ),
            ),
        )
        for candidate in response.candidates or []:
            for part in candidate.content.parts or []:
                if part.inline_data is not None and part.inline_data.data:
                    return SynthesizedSpeech(pcm=part.inline_data.data, pcm_format=GEMINI_TTS_PCM)
        raise RuntimeError("Gemini TTS returned no audio")
