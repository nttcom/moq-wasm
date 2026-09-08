import numpy as np

from ..base import PIPELINE_SAMPLE_RATE

WINDOW_SAMPLES = 512
CONTEXT_SAMPLES = 64
SPEECH_PAD_MS = 200
MIN_SPEECH_MS = 250
MAX_SPEECH_SEC = 15.0


class SileroVad:
    """Streaming Silero VAD on the ONNX model bundled with faster-whisper.
    The model scores 32 ms windows; an utterance starts when the probability
    crosses `threshold` and ends after `min_silence_ms` below it, padded with
    `SPEECH_PAD_MS` of audio on both sides."""

    def __init__(self, threshold: float, min_silence_ms: int, session=None) -> None:
        self.threshold = threshold
        sample_rate = PIPELINE_SAMPLE_RATE
        self.min_silence_windows = max(1, int(min_silence_ms * sample_rate / 1000 / WINDOW_SAMPLES))
        self.pad_samples = int(SPEECH_PAD_MS * sample_rate / 1000)
        self.min_speech_samples = int(MIN_SPEECH_MS * sample_rate / 1000)
        self.max_speech_samples = int(MAX_SPEECH_SEC * sample_rate)
        self.session = session if session is not None else self._onnx_session()
        self._pending = np.zeros(0, dtype=np.int16)
        self._history = np.zeros(0, dtype=np.int16)
        self._utterance: np.ndarray | None = None
        self._silent_windows = 0
        self._h = np.zeros((1, 1, 128), dtype=np.float32)
        self._c = np.zeros((1, 1, 128), dtype=np.float32)
        self._context = np.zeros(CONTEXT_SAMPLES, dtype=np.float32)

    @staticmethod
    def _onnx_session():
        from faster_whisper.vad import get_vad_model

        return get_vad_model().session

    def push(self, pcm: bytes) -> list[bytes]:
        self._pending = np.concatenate([self._pending, np.frombuffer(pcm, dtype=np.int16)])
        utterances = []
        while len(self._pending) >= WINDOW_SAMPLES:
            window = self._pending[:WINDOW_SAMPLES]
            self._pending = self._pending[WINDOW_SAMPLES:]
            completed = self._consume_window(window, self._speech_probability(window))
            if completed is not None:
                utterances.append(completed)
        return utterances

    def flush(self) -> bytes | None:
        utterance = self._utterance
        self._utterance = None
        self._silent_windows = 0
        if utterance is None or len(utterance) - self.pad_samples < self.min_speech_samples:
            return None
        return utterance.tobytes()

    def _speech_probability(self, window: np.ndarray) -> float:
        audio = window.astype(np.float32) / 32768.0
        model_input = np.concatenate([self._context, audio])[np.newaxis, :]
        probabilities, self._h, self._c = self.session.run(
            None, {"input": model_input, "h": self._h, "c": self._c}
        )
        self._context = audio[-CONTEXT_SAMPLES:]
        return float(np.asarray(probabilities).reshape(-1)[0])

    def _consume_window(self, window: np.ndarray, probability: float) -> bytes | None:
        is_speech = probability >= self.threshold
        if self._utterance is None:
            self._history = np.concatenate([self._history, window])[-self.pad_samples :]
            if is_speech:
                self._utterance = np.concatenate([self._history, window])
                self._silent_windows = 0
            return None
        self._utterance = np.concatenate([self._utterance, window])
        self._silent_windows = 0 if is_speech else self._silent_windows + 1
        ended_by_pause = self._silent_windows >= self.min_silence_windows
        if not ended_by_pause and len(self._utterance) < self.max_speech_samples:
            return None
        utterance = self._utterance
        self._utterance = None
        self._silent_windows = 0
        self._history = window[-self.pad_samples :]
        if len(utterance) - self.pad_samples < self.min_speech_samples:
            return None
        return utterance.tobytes()
