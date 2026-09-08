import asyncio
import logging
from dataclasses import dataclass
from typing import Callable

import numpy as np

from .base import PcmFormat, Transcript, TranscriptCallback
from .segmenter import SpeechSegmenter

log = logging.getLogger("stt.whisper")


@dataclass(frozen=True)
class RecognizedSegment:
    text: str
    no_speech_prob: float


Transcribe = Callable[[np.ndarray], list[RecognizedSegment]]


class WhisperLocalBackend:
    """Runs Whisper on this machine via faster-whisper. Whisper is a batch
    model, so audio is cut into utterances by `SpeechSegmenter` and each
    utterance is transcribed on a worker thread; the reader never waits for
    the model. Segments Whisper itself rates as probably not speech are
    dropped, since they are where it hallucinates stock phrases."""

    pcm_format = PcmFormat(16000)

    def __init__(
        self,
        model_name: str = "small",
        device: str = "cpu",
        compute_type: str = "int8",
        language: str = "ja",
        max_segment_sec: float = 10.0,
        silence_rms: int = 300,
        no_speech_threshold: float = 0.6,
        vad_filter: bool = True,
        transcribe: Transcribe | None = None,
    ) -> None:
        self.model_name = model_name
        self.device = device
        self.compute_type = compute_type
        self.language = language
        self.no_speech_threshold = no_speech_threshold
        self.vad_filter = vad_filter
        self.segmenter = SpeechSegmenter(
            self.pcm_format.sample_rate, max_sec=max_segment_sec, silence_rms=silence_rms
        )
        self._transcribe = transcribe
        self._segments: asyncio.Queue[np.ndarray | None] = asyncio.Queue()
        self._worker: asyncio.Task | None = None

    async def start(self, on_transcript: TranscriptCallback) -> None:
        if self._transcribe is None:
            self._transcribe = await asyncio.get_running_loop().run_in_executor(None, self._load_model)
        self._worker = asyncio.ensure_future(self._run_worker(on_transcript))

    async def send_pcm(self, pcm: bytes) -> None:
        for segment in self.segmenter.push(pcm):
            self._segments.put_nowait(segment)

    async def close(self) -> None:
        remaining = self.segmenter.flush()
        if remaining is not None:
            self._segments.put_nowait(remaining)
        self._segments.put_nowait(None)
        if self._worker is not None:
            await self._worker
            self._worker = None

    def _load_model(self) -> Transcribe:
        from faster_whisper import WhisperModel

        log.info("loading whisper model %s on %s (%s)", self.model_name, self.device, self.compute_type)
        model = WhisperModel(self.model_name, device=self.device, compute_type=self.compute_type)

        def transcribe(audio: np.ndarray) -> list[RecognizedSegment]:
            segments, _ = model.transcribe(
                audio, language=self.language, beam_size=5, vad_filter=self.vad_filter
            )
            return [RecognizedSegment(segment.text.strip(), segment.no_speech_prob) for segment in segments]

        return transcribe

    def _speech_text(self, segments: list[RecognizedSegment]) -> str:
        kept = []
        for segment in segments:
            if segment.no_speech_prob > self.no_speech_threshold:
                log.debug("dropping segment as non-speech (%.2f): %s", segment.no_speech_prob, segment.text)
                continue
            if segment.text:
                kept.append(segment.text)
        return " ".join(kept)

    async def _run_worker(self, on_transcript: TranscriptCallback) -> None:
        assert self._transcribe is not None
        loop = asyncio.get_running_loop()
        while (segment := await self._segments.get()) is not None:
            audio = segment.astype(np.float32) / 32768.0
            segments = await loop.run_in_executor(None, self._transcribe, audio)
            text = self._speech_text(segments)
            if text:
                on_transcript(Transcript(text=text, is_final=True))
