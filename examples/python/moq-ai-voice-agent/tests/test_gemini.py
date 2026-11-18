from pipecat.transcriptions.language import Language

from moq_ai_voice_agent.gemini import GeminiServices, VertexProject


class OfflineVertexProject(VertexProject):
    def client(self):
        return object()


def test_on_vertex_ai_the_expected_language_goes_in_language_codes():
    # Arrange
    stt = GeminiServices(Language.JA_JP, vertex=OfflineVertexProject("project")).stt()

    # Act
    transcription = stt._build_live_config().input_audio_transcription

    # Assert
    assert transcription.language_codes == ["ja-JP"]
    assert transcription.language_hints is None


def test_on_the_gemini_api_the_expected_language_stays_a_hint():
    # Arrange
    stt = GeminiServices(Language.JA_JP, api_key="key").stt()

    # Act
    transcription = stt._build_live_config().input_audio_transcription

    # Assert
    assert transcription.language_hints.language_codes == ["ja-JP"]
