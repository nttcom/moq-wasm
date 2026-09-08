import wave
from pathlib import Path

from .base import PcmFormat, Transcript, TranscriptCallback


class WavFileBackend:
    """Debug backend: records the decoded PCM instead of transcribing it and
    reports how much audio it has received."""

    def __init__(self, path: Path, pcm_format: PcmFormat = PcmFormat(16000)) -> None:
        self.path = path
        self.pcm_format = pcm_format
        self.samples_written = 0
        self._writer: wave.Wave_write | None = None
        self._on_transcript: TranscriptCallback | None = None

    async def start(self, on_transcript: TranscriptCallback) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._writer = wave.open(str(self.path), "wb")
        self._writer.setnchannels(self.pcm_format.channels)
        self._writer.setsampwidth(2)
        self._writer.setframerate(self.pcm_format.sample_rate)
        self._on_transcript = on_transcript

    async def send_pcm(self, pcm: bytes) -> None:
        if self._writer is None:
            raise RuntimeError("start() must be called before send_pcm()")
        self._writer.writeframes(pcm)
        self.samples_written += len(pcm) // (2 * self.pcm_format.channels)

    async def close(self) -> None:
        if self._writer is None:
            return
        self._writer.close()
        self._writer = None
        seconds = self.samples_written / self.pcm_format.sample_rate
        if self._on_transcript is not None:
            self._on_transcript(Transcript(text=f"[wav] {seconds:.1f}s written to {self.path}", is_final=True))
