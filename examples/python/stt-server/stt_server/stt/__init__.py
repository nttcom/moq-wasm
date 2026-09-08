import os
from pathlib import Path

from .base import PcmFormat, SpeechToText, Transcript, TranscriptCallback
from .deepgram import DeepgramBackend
from .openai_realtime import OpenAiRealtimeBackend
from .wav_file import WavFileBackend

__all__ = [
    "DeepgramBackend",
    "OpenAiRealtimeBackend",
    "PcmFormat",
    "SpeechToText",
    "Transcript",
    "TranscriptCallback",
    "WavFileBackend",
    "create_backend",
]


def create_backend(track_label: str) -> SpeechToText:
    """Backend selection from the environment: STT_BACKEND = wav | deepgram | openai."""
    name = os.environ.get("STT_BACKEND", "wav")
    language = os.environ.get("STT_LANGUAGE", "ja")
    match name:
        case "wav":
            directory = Path(os.environ.get("TRANSCRIPT_DIR", "recordings"))
            return WavFileBackend(directory / f"{track_label}.wav")
        case "deepgram":
            return DeepgramBackend(api_key=os.environ["DEEPGRAM_API_KEY"], language=language)
        case "openai":
            return OpenAiRealtimeBackend(api_key=os.environ["OPENAI_API_KEY"], language=language)
    raise ValueError(f"unknown STT_BACKEND: {name}")
