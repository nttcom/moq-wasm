import time
from dataclasses import dataclass, field
from typing import Awaitable, Callable, Protocol


@dataclass(frozen=True)
class PcmFormat:
    """Signed 16-bit little-endian interleaved PCM."""

    sample_rate: int
    channels: int = 1

    def bytes_per_second(self) -> int:
        return self.sample_rate * self.channels * 2


PIPELINE_PCM = PcmFormat(16000)


class VoiceActivityDetector(Protocol):
    """Cuts the incoming PCM stream (`PIPELINE_PCM`) into utterances."""

    def push(self, pcm: bytes) -> list[bytes]: ...

    def flush(self) -> bytes | None: ...


class SpeechToText(Protocol):
    async def transcribe(self, utterance: bytes, pcm_format: PcmFormat) -> str: ...


class LanguageModel(Protocol):
    """One conversation; implementations keep their own history."""

    async def reply(self, user_text: str) -> str: ...


@dataclass(frozen=True)
class SynthesizedSpeech:
    pcm: bytes
    pcm_format: PcmFormat


class TextToSpeech(Protocol):
    async def synthesize(self, text: str) -> SynthesizedSpeech: ...


@dataclass(frozen=True)
class TranscriptEvent:
    text: str
    at: float = field(default_factory=time.time)


@dataclass(frozen=True)
class ReplyTextEvent:
    text: str
    at: float = field(default_factory=time.time)


@dataclass(frozen=True)
class ReplyAudioEvent:
    speech: SynthesizedSpeech
    at: float = field(default_factory=time.time)


PipelineEvent = TranscriptEvent | ReplyTextEvent | ReplyAudioEvent
EventSink = Callable[[PipelineEvent], Awaitable[None]]
