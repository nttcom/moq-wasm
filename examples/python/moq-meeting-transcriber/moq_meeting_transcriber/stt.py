import asyncio
import time
from collections import deque
from dataclasses import dataclass, field

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


@dataclass
class SharedWhisper:
    """The process's one Whisper model, which the participants' pipelines take turns on."""

    model: WhisperModel
    name: str
    lock: asyncio.Lock = field(default_factory=asyncio.Lock)


@dataclass
class UtteranceTranscriptionFrame(TranscriptionFrame):
    """A transcription with the wall-clock time, in ms, at which its utterance began."""

    started_at_ms: int = 0


class SharedWhisperSTTService(SegmentedSTTService):
    """Transcribes one participant's speech with the Whisper model the whole process shares.

    pipecat's WhisperSTTService loads a model per instance, which would hold one copy per participant.
    """

    def __init__(self, whisper: SharedWhisper, speaker: str):
        super().__init__(
            sample_rate=SAMPLE_RATE,
            trailing_silence_secs=TRAILING_SILENCE_SECONDS,
            settings=STTSettings(model=whisper.name, language=LANGUAGE),
        )
        self._whisper = whisper
        self._speaker = speaker
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
        samples = np.frombuffer(audio, dtype=np.int16).astype(np.float32) / 32768.0
        async with self._whisper.lock:
            text = await asyncio.to_thread(self._transcribe, samples)
        if text:
            yield UtteranceTranscriptionFrame(
                text,
                self._speaker,
                time_now_iso8601(),
                LANGUAGE,
                started_at_ms=int((stopped_at - speech_seconds) * 1000),
            )

    def _transcribe(self, samples: np.ndarray) -> str:
        # faster-whisper decodes lazily while the segments are iterated, so they are read here, off the loop.
        segments, _ = self._whisper.model.transcribe(samples, language=LANGUAGE.value)
        return "".join(segment.text for segment in segments if segment.no_speech_prob < NO_SPEECH_PROB).strip()
