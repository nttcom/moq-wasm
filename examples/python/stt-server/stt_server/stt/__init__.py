import os
from pathlib import Path

from .base import PcmFormat, SpeechToText, Transcript, TranscriptCallback
from .deepgram import DeepgramBackend
from .openai_realtime import OpenAiRealtimeBackend
from .wav_file import WavFileBackend
from .whisper_local import WhisperLocalBackend

__all__ = [
    "DeepgramBackend",
    "OpenAiRealtimeBackend",
    "PcmFormat",
    "SpeechToText",
    "Transcript",
    "TranscriptCallback",
    "WavFileBackend",
    "WhisperLocalBackend",
    "create_backend",
]


def create_backend(track_label: str) -> SpeechToText:
    """Backend selection from the environment: STT_BACKEND = wav | deepgram | openai | whisper."""
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
        case "whisper":
            return WhisperLocalBackend(
                model_name=os.environ.get("WHISPER_MODEL", "small"),
                device=os.environ.get("WHISPER_DEVICE", "cpu"),
                compute_type=os.environ.get("WHISPER_COMPUTE_TYPE", "int8"),
                language=language,
                max_segment_sec=float(os.environ.get("WHISPER_MAX_SEGMENT_SEC", "10")),
                silence_rms=int(os.environ.get("WHISPER_SILENCE_RMS", "300")),
                no_speech_threshold=float(os.environ.get("WHISPER_NO_SPEECH_THRESHOLD", "0.6")),
                vad_filter=os.environ.get("WHISPER_VAD_FILTER", "1") != "0",
            )
    raise ValueError(f"unknown STT_BACKEND: {name}")
