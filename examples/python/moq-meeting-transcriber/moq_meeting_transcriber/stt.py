import asyncio
import time
from collections import deque
from dataclasses import dataclass, field
from typing import Protocol

import numpy as np
from faster_whisper import WhisperModel
from pipecat.frames.frames import Frame, TranscriptionFrame, VADUserStoppedSpeakingFrame
from pipecat.processors.frame_processor import FrameDirection
from pipecat.services.settings import STTSettings
from pipecat.services.stt_service import SegmentedSTTService
from pipecat.transcriptions.language import Language
from pipecat.utils.time import time_now_iso8601

from moq_meeting_transcriber.audio import SAMPLE_RATE

LANGUAGE = Language.JA
# The no-speech probability at or above which pipecat's WhisperSTTService drops a segment.
NO_SPEECH_PROB = 0.4
TRAILING_SILENCE_SECONDS = 0.5


class Recognizer(Protocol):
    """Turns one utterance of 16 kHz mono 16-bit PCM into text."""

    name: str

    async def ready(self) -> None:
        """Returns once the recognizer answers; a GPU service scaled to zero takes minutes."""

    async def recognize(self, pcm: bytes) -> str: ...


@dataclass
class SharedWhisper:
    """The process's one faster-whisper model on the CPU, which the participants' pipelines take turns on."""

    model: asyncio.Future[WhisperModel]
    name: str = "whisper"
    lock: asyncio.Lock = field(default_factory=asyncio.Lock)

    async def ready(self) -> None:
        await asyncio.shield(self.model)

    async def recognize(self, pcm: bytes) -> str:
        model = await asyncio.shield(self.model)
        samples = np.frombuffer(pcm, dtype=np.int16).astype(np.float32) / 32768.0
        async with self.lock:
            return await asyncio.to_thread(self._transcribe, model, samples)

    def _transcribe(self, model: WhisperModel, samples: np.ndarray) -> str:
        # faster-whisper decodes lazily while the segments are iterated, so they are read here, off the loop.
        segments, _ = model.transcribe(samples, language=LANGUAGE.value)
        return "".join(segment.text for segment in segments if segment.no_speech_prob < NO_SPEECH_PROB).strip()


@dataclass
class UtteranceTranscriptionFrame(TranscriptionFrame):
    """A transcription with the wall-clock time, in ms, at which its utterance began."""

    started_at_ms: int = 0


class UtteranceSTTService(SegmentedSTTService):
    """Hands each utterance of one participant to the room's recognizer and dates it. Once `dropped`
    is set, the utterance being recognized and those still queued end without text."""

    def __init__(self, recognizer: Recognizer, speaker: str, dropped: asyncio.Event):
        super().__init__(
            sample_rate=SAMPLE_RATE,
            trailing_silence_secs=TRAILING_SILENCE_SECONDS,
            settings=STTSettings(model=recognizer.name, language=LANGUAGE),
        )
        self._recognizer = recognizer
        self._speaker = speaker
        self._dropped = dropped
        # One entry per queued segment: segments are transcribed in order, after the stop that closed them.
        self._stopped_at: deque[float] = deque()

    @property
    def wants_wav_segments(self) -> bool:
        return False

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        if isinstance(frame, VADUserStoppedSpeakingFrame) and self.is_usable:
            self._stopped_at.append(time.time())
        await super().process_frame(frame, direction)

    async def run_stt(self, audio: bytes):
        stopped_at = self._stopped_at.popleft()
        speech_seconds = len(audio) / (2 * SAMPLE_RATE) - TRAILING_SILENCE_SECONDS
        text = await self._recognize(audio)
        if text:
            yield UtteranceTranscriptionFrame(
                text,
                self._speaker,
                time_now_iso8601(),
                LANGUAGE,
                started_at_ms=int((stopped_at - speech_seconds) * 1000),
            )

    async def _recognize(self, audio: bytes) -> str:
        # A graceful stop of the pipeline waits for every queued utterance, and a CancelFrame queued
        # after it waits too, so dropping the utterances is what ends such a stop at once.
        recognition = asyncio.ensure_future(self._recognizer.recognize(audio))
        dropped = asyncio.ensure_future(self._dropped.wait())
        try:
            await asyncio.wait([recognition, dropped], return_when=asyncio.FIRST_COMPLETED)
        finally:
            recognition.cancel()
            dropped.cancel()
        if self._dropped.is_set():
            return ""
        return recognition.result().strip()
