import time
from dataclasses import dataclass, field
from typing import Awaitable, Callable, Literal, Protocol

PIPELINE_SAMPLE_RATE = 16000
"""Stages exchange signed 16-bit little-endian mono PCM at this rate."""


class VoiceActivityDetector(Protocol):
    """Cuts the incoming PCM stream into utterances."""

    def push(self, pcm: bytes) -> list[bytes]: ...

    def flush(self) -> bytes | None: ...


class SpeechToText(Protocol):
    async def transcribe(self, utterance: bytes) -> str: ...


class LanguageModel(Protocol):
    """One conversation; implementations keep their own history."""

    async def reply(self, user_text: str) -> str: ...


@dataclass(frozen=True)
class SynthesizedSpeech:
    pcm: bytes
    sample_rate: int


class TextToSpeech(Protocol):
    async def synthesize(self, text: str) -> SynthesizedSpeech: ...


Stage = Literal["vad", "stt", "llm", "tts", "turn"]


@dataclass(frozen=True)
class TurnEvent:
    """One step of one conversation turn. `elapsed_ms` is how long the stage
    took, `text` the transcript or the reply where the stage produces one."""

    turn: int
    stage: Stage
    state: Literal["start", "done", "failed"]
    elapsed_ms: float | None = None
    text: str | None = None
    detail: dict | None = None
    at: float = field(default_factory=time.time)


@dataclass(frozen=True)
class ReplyAudio:
    turn: int
    speech: SynthesizedSpeech


PipelineEvent = TurnEvent | ReplyAudio
EventSink = Callable[[PipelineEvent], Awaitable[None]]
