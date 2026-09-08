import io
import wave
from pathlib import Path

from ..base import PIPELINE_PCM


def pcm_to_wav(pcm: bytes) -> bytes:
    buffer = io.BytesIO()
    with wave.open(buffer, "wb") as writer:
        writer.setnchannels(1)
        writer.setsampwidth(2)
        writer.setframerate(PIPELINE_PCM.sample_rate)
        writer.writeframes(pcm)
    return buffer.getvalue()


class WavFileStt:
    """Debug stage: writes every utterance to `directory` and reports the
    file name instead of a transcript."""

    def __init__(self, directory: Path) -> None:
        self.directory = directory
        self.count = 0

    async def transcribe(self, utterance: bytes) -> str:
        self.directory.mkdir(parents=True, exist_ok=True)
        self.count += 1
        path = self.directory / f"utterance-{self.count:04d}.wav"
        path.write_bytes(pcm_to_wav(utterance))
        return f"[wav] {len(utterance) / PIPELINE_PCM.bytes_per_second():.1f}s written to {path}"
