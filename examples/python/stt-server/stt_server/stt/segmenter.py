import numpy as np


class SpeechSegmenter:
    """Cuts a PCM16 mono stream into utterances for a batch recognizer: a
    segment ends after `silence_sec` of low energy once at least `min_sec`
    is buffered, or unconditionally at `max_sec`."""

    FRAME_SEC = 0.02

    def __init__(
        self,
        sample_rate: int,
        min_sec: float = 1.0,
        max_sec: float = 10.0,
        silence_sec: float = 0.4,
        silence_rms: int = 300,
    ) -> None:
        self.sample_rate = sample_rate
        self.min_samples = int(min_sec * sample_rate)
        self.max_samples = int(max_sec * sample_rate)
        self.silence_frames = int(silence_sec / self.FRAME_SEC)
        self.silence_rms = silence_rms
        self.frame_samples = int(self.FRAME_SEC * sample_rate)
        self._buffer = np.zeros(0, dtype=np.int16)
        self._trailing_silent_frames = 0

    def push(self, pcm: bytes) -> list[np.ndarray]:
        samples = np.frombuffer(pcm, dtype=np.int16)
        segments = []
        for start in range(0, len(samples), self.frame_samples):
            frame = samples[start : start + self.frame_samples]
            self._buffer = np.concatenate([self._buffer, frame])
            rms = np.sqrt(np.mean(frame.astype(np.float32) ** 2)) if len(frame) else 0.0
            self._trailing_silent_frames = self._trailing_silent_frames + 1 if rms < self.silence_rms else 0
            pause = self._trailing_silent_frames >= self.silence_frames
            if len(self._buffer) >= self.max_samples or (pause and len(self._buffer) >= self.min_samples):
                segments.append(self._take())
        return segments

    def flush(self) -> np.ndarray | None:
        if len(self._buffer) < self.frame_samples:
            self._buffer = np.zeros(0, dtype=np.int16)
            return None
        return self._take()

    def _take(self) -> np.ndarray:
        segment = self._buffer
        self._buffer = np.zeros(0, dtype=np.int16)
        self._trailing_silent_frames = 0
        return segment
