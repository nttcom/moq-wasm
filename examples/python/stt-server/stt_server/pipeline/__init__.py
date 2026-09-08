import os
from pathlib import Path
from typing import Callable

from .base import EventSink, LanguageModel, SpeechToText, TextToSpeech, VoiceActivityDetector
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

DEFAULTS = {"vad": "silero", "stt": "whisper", "llm": "gemini", "tts": "gemini"}
LANGUAGE = os.environ.get("STT_LANGUAGE", "ja")


VAD_FACTORIES: dict[str, Callable[[], VoiceActivityDetector]] = {
    "silero": lambda: SileroVad(
        threshold=float(os.environ.get("SILERO_THRESHOLD", "0.5")),
        min_silence_ms=int(os.environ.get("SILERO_MIN_SILENCE_MS", "500")),
    ),
    "energy": lambda: EnergyVad(),
}

STT_FACTORIES: dict[str, Callable[[], SpeechToText]] = {
    "whisper": lambda: WhisperLocalStt(
        model_name=os.environ.get("WHISPER_MODEL", "small"),
        device=os.environ.get("WHISPER_DEVICE", "cpu"),
        compute_type=os.environ.get("WHISPER_COMPUTE_TYPE", "int8"),
        language=LANGUAGE,
        no_speech_threshold=float(os.environ.get("WHISPER_NO_SPEECH_THRESHOLD", "0.6")),
    ),
    "deepgram": lambda: DeepgramStt(api_key=os.environ["DEEPGRAM_API_KEY"], language=LANGUAGE),
    "openai": lambda: OpenAiTranscribeStt(api_key=os.environ["OPENAI_API_KEY"], language=LANGUAGE),
    "wav": lambda: WavFileStt(Path(os.environ.get("TRANSCRIPT_DIR", "recordings"))),
}

LLM_FACTORIES: dict[str, Callable[[], LanguageModel | None]] = {
    "gemini": lambda: GeminiLlm(
        model=os.environ.get("GEMINI_MODEL", "gemini-2.5-flash"),
        system_prompt=os.environ.get("LLM_SYSTEM_PROMPT", DEFAULT_SYSTEM_PROMPT),
    ),
    "echo": lambda: EchoLlm(),
    "none": lambda: None,
}

TTS_FACTORIES: dict[str, Callable[[], TextToSpeech | None]] = {
    "gemini": lambda: GeminiTts(
        model=os.environ.get("GEMINI_TTS_MODEL", "gemini-2.5-flash-preview-tts"),
        voice=os.environ.get("GEMINI_TTS_VOICE", "Kore"),
    ),
    "none": lambda: None,
}


def _selected(stage: str) -> str:
    return os.environ.get(f"PIPELINE_{stage.upper()}", DEFAULTS[stage])


def _factory(stage: str, factories: dict):
    name = _selected(stage)
    if name not in factories:
        raise ValueError(f"PIPELINE_{stage.upper()}={name!r}; expected one of {sorted(factories)}")
    return factories[name]


def pipeline_summary() -> dict[str, str]:
    return {stage: _selected(stage) for stage in DEFAULTS}


def build_pipeline(sink: EventSink) -> VoicePipeline:
    return VoicePipeline(
        vad=_factory("vad", VAD_FACTORIES)(),
        stt=_factory("stt", STT_FACTORIES)(),
        llm=_factory("llm", LLM_FACTORIES)(),
        tts=_factory("tts", TTS_FACTORIES)(),
        sink=sink,
    )
