import time
from dataclasses import dataclass, field
from typing import Callable, Protocol


@dataclass(frozen=True)
class PcmFormat:
    """Signed 16-bit little-endian interleaved PCM."""

    sample_rate: int
    channels: int = 1


@dataclass(frozen=True)
class Transcript:
    text: str
    is_final: bool
    received_at: float = field(default_factory=time.time)


TranscriptCallback = Callable[[Transcript], None]


class SpeechToText(Protocol):
    """One streaming recognition session per audio track."""

    pcm_format: PcmFormat

    async def start(self, on_transcript: TranscriptCallback) -> None: ...

    async def send_pcm(self, pcm: bytes) -> None: ...

    async def close(self) -> None: ...
