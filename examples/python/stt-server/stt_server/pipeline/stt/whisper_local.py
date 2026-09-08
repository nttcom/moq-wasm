import asyncio
import logging
from dataclasses import dataclass
from typing import Callable

import numpy as np

from ..base import PcmFormat

log = logging.getLogger("stt.whisper")


@dataclass(frozen=True)
class RecognizedSegment:
    text: str
    no_speech_prob: float


Recognizer = Callable[[np.ndarray], list[RecognizedSegment]]


class WhisperLocalStt:
    """faster-whisper on this machine. Each utterance is transcribed on a
    worker thread; segments Whisper rates as probably not speech are dropped,
    since they are where it hallucinates stock phrases."""

    def __init__(
        self,
        model_name: str = "small",
        device: str = "cpu",
        compute_type: str = "int8",
        language: str = "ja",
        no_speech_threshold: float = 0.6,
        recognizer: Recognizer | None = None,
    ) -> None:
        self.model_name = model_name
        self.device = device
        self.compute_type = compute_type
        self.language = language
        self.no_speech_threshold = no_speech_threshold
        self._recognizer = recognizer

    async def transcribe(self, utterance: bytes, pcm_format: PcmFormat) -> str:
        if pcm_format.sample_rate != 16000 or pcm_format.channels != 1:
            raise ValueError(f"whisper expects 16 kHz mono PCM, got {pcm_format}")
        loop = asyncio.get_running_loop()
        if self._recognizer is None:
            self._recognizer = await loop.run_in_executor(None, self._load_model)
        audio = np.frombuffer(utterance, dtype=np.int16).astype(np.float32) / 32768.0
        segments = await loop.run_in_executor(None, self._recognizer, audio)
        return " ".join(
            segment.text
            for segment in segments
            if segment.text and segment.no_speech_prob <= self.no_speech_threshold
        )

    def _load_model(self) -> Recognizer:
        from faster_whisper import WhisperModel

        log.info("loading whisper model %s on %s (%s)", self.model_name, self.device, self.compute_type)
        model = WhisperModel(self.model_name, device=self.device, compute_type=self.compute_type)

        def recognize(audio: np.ndarray) -> list[RecognizedSegment]:
            segments, _ = model.transcribe(audio, language=self.language, beam_size=5)
            return [RecognizedSegment(segment.text.strip(), segment.no_speech_prob) for segment in segments]

        return recognize
