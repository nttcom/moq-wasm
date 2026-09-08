import asyncio
import logging
from functools import lru_cache
from typing import Callable, Sequence

import numpy as np

log = logging.getLogger("stt.whisper")

Recognizer = Callable[[np.ndarray], Sequence]


@lru_cache(maxsize=None)
def _load_model(model_name: str, device: str, compute_type: str):
    """One model per configuration: a process can serve several tracks, and
    the weights are hundreds of megabytes."""
    from faster_whisper import WhisperModel

    log.info("loading whisper model %s on %s (%s)", model_name, device, compute_type)
    return WhisperModel(model_name, device=device, compute_type=compute_type)


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

    async def transcribe(self, utterance: bytes) -> str:
        audio = np.frombuffer(utterance, dtype=np.int16).astype(np.float32) / 32768.0
        recognizer = self._recognizer or self._recognize
        segments = await asyncio.get_running_loop().run_in_executor(None, recognizer, audio)
        return " ".join(
            segment.text.strip()
            for segment in segments
            if segment.text.strip() and segment.no_speech_prob <= self.no_speech_threshold
        )

    def _recognize(self, audio: np.ndarray) -> Sequence:
        model = _load_model(self.model_name, self.device, self.compute_type)
        return list(model.transcribe(audio, language=self.language, beam_size=5)[0])
