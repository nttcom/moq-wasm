"""Stage selection from the environment.

PIPELINE_VAD = silero | energy
PIPELINE_STT = whisper | deepgram | openai | wav
PIPELINE_LLM = gemini | echo | none
PIPELINE_TTS = gemini | none
"""

import os
from pathlib import Path
from typing import Callable

from .base import (
    EventSink,
    LanguageModel,
    PIPELINE_PCM,
    PcmFormat,
    PipelineEvent,
    ReplyAudioEvent,
    ReplyTextEvent,
    SpeechToText,
    SynthesizedSpeech,
    TextToSpeech,
    TranscriptEvent,
    VoiceActivityDetector,
)
from .llm.echo import EchoLlm
from .llm.gemini import DEFAULT_SYSTEM_PROMPT, GeminiLlm
from .runner import VoicePipeline
from .stt.deepgram import DeepgramStt
from .stt.openai_transcribe import OpenAiTranscribeStt
from .stt.wav_file import WavFileStt
from .stt.whisper_local import WhisperLocalStt
from .tts.gemini import GeminiTts
from .vad.energy import EnergyVad
from .vad.silero import SileroVad

__all__ = [
    "EventSink",
    "LanguageModel",
    "PIPELINE_PCM",
    "PcmFormat",
    "PipelineEvent",
    "ReplyAudioEvent",
    "ReplyTextEvent",
    "SpeechToText",
    "SynthesizedSpeech",
    "TextToSpeech",
    "TranscriptEvent",
    "VoicePipeline",
    "VoiceActivityDetector",
    "build_pipeline",
    "pipeline_summary",
]


def _env(name: str, default: str) -> str:
    return os.environ.get(name, default)


def _language() -> str:
    return _env("STT_LANGUAGE", "ja")


def _gemini_api_key() -> str:
    return os.environ.get("GEMINI_API_KEY") or os.environ["GOOGLE_API_KEY"]


VAD_FACTORIES: dict[str, Callable[[], VoiceActivityDetector]] = {
    "silero": lambda: SileroVad(
        threshold=float(_env("SILERO_THRESHOLD", "0.5")),
        min_silence_ms=int(_env("SILERO_MIN_SILENCE_MS", "500")),
    ),
    "energy": lambda: EnergyVad(silence_rms=int(_env("ENERGY_SILENCE_RMS", "300"))),
}

STT_FACTORIES: dict[str, Callable[[str], SpeechToText]] = {
    "whisper": lambda _label: WhisperLocalStt(
        model_name=_env("WHISPER_MODEL", "small"),
        device=_env("WHISPER_DEVICE", "cpu"),
        compute_type=_env("WHISPER_COMPUTE_TYPE", "int8"),
        language=_language(),
        no_speech_threshold=float(_env("WHISPER_NO_SPEECH_THRESHOLD", "0.6")),
    ),
    "deepgram": lambda _label: DeepgramStt(api_key=os.environ["DEEPGRAM_API_KEY"], language=_language()),
    "openai": lambda _label: OpenAiTranscribeStt(api_key=os.environ["OPENAI_API_KEY"], language=_language()),
    "wav": lambda label: WavFileStt(Path(_env("TRANSCRIPT_DIR", "recordings")) / label),
}

LLM_FACTORIES: dict[str, Callable[[], LanguageModel | None]] = {
    "gemini": lambda: GeminiLlm(
        api_key=_gemini_api_key(),
        model=_env("GEMINI_MODEL", "gemini-2.5-flash"),
        system_prompt=_env("LLM_SYSTEM_PROMPT", DEFAULT_SYSTEM_PROMPT),
    ),
    "echo": lambda: EchoLlm(),
    "none": lambda: None,
}

TTS_FACTORIES: dict[str, Callable[[], TextToSpeech | None]] = {
    "gemini": lambda: GeminiTts(
        api_key=_gemini_api_key(),
        model=_env("GEMINI_TTS_MODEL", "gemini-2.5-flash-preview-tts"),
        voice=_env("GEMINI_TTS_VOICE", "Kore"),
    ),
    "none": lambda: None,
}


def _select(factories: dict, variable: str, default: str):
    name = _env(variable, default)
    try:
        return name, factories[name]
    except KeyError:
        raise ValueError(f"{variable}={name!r}; expected one of {sorted(factories)}") from None


def pipeline_summary() -> dict[str, str]:
    return {
        "vad": _env("PIPELINE_VAD", "silero"),
        "stt": _env("PIPELINE_STT", "whisper"),
        "llm": _env("PIPELINE_LLM", "gemini"),
        "tts": _env("PIPELINE_TTS", "gemini"),
    }


def build_pipeline(track_label: str, sink: EventSink) -> VoicePipeline:
    _, vad = _select(VAD_FACTORIES, "PIPELINE_VAD", "silero")
    _, stt = _select(STT_FACTORIES, "PIPELINE_STT", "whisper")
    _, llm = _select(LLM_FACTORIES, "PIPELINE_LLM", "gemini")
    _, tts = _select(TTS_FACTORIES, "PIPELINE_TTS", "gemini")
    return VoicePipeline(vad=vad(), stt=stt(track_label), llm=llm(), tts=tts(), sink=sink)
