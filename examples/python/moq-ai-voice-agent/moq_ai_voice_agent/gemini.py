from dataclasses import dataclass

from google import genai
from google.genai.types import AudioTranscriptionConfig, LiveConnectConfig
from pipecat.services.google.gemini_live.stt import GeminiSTTService
from pipecat.services.google.llm import GoogleLLMService
from pipecat.services.google.tts import GeminiTTSService
from pipecat.services.google.vertex.llm import GoogleVertexLLMService
from pipecat.transcriptions.language import Language

# The Gemini transcription, chat and TTS models are served from Vertex AI's global endpoint only.
VERTEX_LOCATION = "global"
# pipecat's Gemini API and Vertex AI LLM services default to different models.
LLM_MODEL = "gemini-3.6-flash"


@dataclass(frozen=True)
class VertexProject:
    """Called with the application default credentials, such as a Cloud Run service account's."""

    project: str

    def client(self) -> genai.Client:
        return genai.Client(vertexai=True, project=self.project, location=VERTEX_LOCATION)


@dataclass(frozen=True)
class GeminiServices:
    """pipecat's Gemini STT and TTS reach the Gemini API only, so on Vertex AI subclasses give them a
    Vertex AI client."""

    language: Language
    api_key: str | None = None
    vertex: VertexProject | None = None

    def stt(self, **kwargs) -> GeminiSTTService:
        settings = GeminiSTTService.Settings(languages=[self.language])
        if self.vertex is None:
            return GeminiSTTService(api_key=self.api_key, settings=settings, **kwargs)
        return _VertexGeminiSTTService(self.vertex, settings=settings, **kwargs)

    def llm(self, system_instruction: str) -> GoogleLLMService:
        if self.vertex is None:
            settings = GoogleLLMService.Settings(model=LLM_MODEL, system_instruction=system_instruction)
            return GoogleLLMService(api_key=self.api_key, settings=settings)
        settings = GoogleVertexLLMService.Settings(model=LLM_MODEL, system_instruction=system_instruction)
        return GoogleVertexLLMService(project_id=self.vertex.project, location=VERTEX_LOCATION, settings=settings)

    def tts(self, voice: str) -> GeminiTTSService:
        settings = GeminiTTSService.Settings(voice=voice, language=self.language)
        if self.vertex is None:
            return GeminiTTSService(api_key=self.api_key, settings=settings)
        return _VertexGeminiTTSService(self.vertex, settings=settings)


class _VertexGeminiSTTService(GeminiSTTService):
    """Vertex AI takes the expected languages as `language_codes`, and refuses the Gemini API's
    `language_hints`."""

    def __init__(self, vertex: VertexProject, **kwargs):
        # Set before the base class creates its client.
        self._vertex = vertex
        super().__init__(api_key="", **kwargs)

    def _create_client(self):
        self._client = self._vertex.client()

    def _build_live_config(self) -> LiveConnectConfig:
        config = super()._build_live_config()
        transcription = config.input_audio_transcription
        if transcription is not None and transcription.language_hints is not None:
            fields = transcription.model_dump(exclude_none=True, exclude={"language_hints"})
            config.input_audio_transcription = AudioTranscriptionConfig(
                **fields, language_codes=transcription.language_hints.language_codes
            )
        return config


class _VertexGeminiTTSService(GeminiTTSService):
    """The Gemini API path of `GeminiTTSService` on a Vertex AI client: its other backend is Cloud
    Text-to-Speech, a separate API."""

    def __init__(self, vertex: VertexProject, **kwargs):
        # Set before the base class creates its client.
        self._vertex = vertex
        super().__init__(use_genai=True, **kwargs)

    def _create_client(self, credentials: str | None, credentials_path: str | None) -> genai.Client:
        return self._vertex.client()
