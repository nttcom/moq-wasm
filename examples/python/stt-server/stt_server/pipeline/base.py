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


@dataclass(frozen=True)
class TextEvent:
    kind: Literal["transcript", "reply"]
    text: str
    at: float = field(default_factory=time.time)


PipelineEvent = TextEvent | SynthesizedSpeech
EventSink = Callable[[PipelineEvent], Awaitable[None]]
