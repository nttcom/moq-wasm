from google import genai
from google.genai import types

from ..base import SynthesizedSpeech


def pcm_sample_rate(mime_type: str) -> int:
    """The API reports the PCM rate in the mime type of the audio part, e.g.
    `audio/l16; rate=24000; channels=1`."""
    for parameter in mime_type.split(";"):
        name, _, value = parameter.strip().partition("=")
        if name.lower() == "rate":
            return int(value)
    raise RuntimeError(f"Gemini TTS returned audio without a sample rate: {mime_type}")


class GeminiTts:
    """Gemini speech generation; the API returns raw mono PCM16. The SDK reads
    GEMINI_API_KEY (or GOOGLE_API_KEY) from the environment."""

    def __init__(self, model: str, voice: str) -> None:
        self._client = genai.Client()
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
                    return SynthesizedSpeech(
                        pcm=part.inline_data.data,
                        sample_rate=pcm_sample_rate(part.inline_data.mime_type),
                    )
        raise RuntimeError("Gemini TTS returned no audio")
