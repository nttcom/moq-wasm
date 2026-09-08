from types import SimpleNamespace

import numpy as np

from stt_server.pipeline.base import PIPELINE_SAMPLE_RATE, SynthesizedSpeech, TextEvent
from stt_server.pipeline.runner import VoicePipeline
from stt_server.pipeline.stt.whisper_local import WhisperLocalStt
from stt_server.pipeline.vad.energy import EnergyVad
from stt_server.pipeline.vad.silero import WINDOW_SAMPLES, SileroVad
from tests.conftest import silence, tone


class FakeStt:
    async def transcribe(self, utterance: bytes) -> str:
        return f"{len(utterance) // 2} samples"


class FakeLlm:
    async def reply(self, user_text: str) -> str:
        return f"reply to '{user_text}'"


class FakeTts:
    async def synthesize(self, text: str) -> SynthesizedSpeech:
        return SynthesizedSpeech(pcm=tone(0.1, sample_rate=24000), sample_rate=24000)


class FlakyStt:
    """Fails on the first utterance, then numbers the ones that follow."""

    def __init__(self) -> None:
        self.calls = 0

    async def transcribe(self, utterance: bytes) -> str:
        self.calls += 1
        if self.calls == 1:
            raise RuntimeError("service down")
        return f"utterance {self.calls}"


class RecordingSink:
    def __init__(self) -> None:
        self.events = []

    async def __call__(self, event) -> None:
        self.events.append(event)


class FakeVadSession:
    """Answers each 32 ms window with the next queued speech probability."""

    def __init__(self, probabilities: list[float]) -> None:
        self.probabilities = list(probabilities)

    def run(self, _outputs, inputs):
        probability = self.probabilities.pop(0) if self.probabilities else 0.0
        return np.array([[probability]]), inputs["h"], inputs["c"]


async def collect_events(pipeline: VoicePipeline, pcm: bytes) -> list:
    await pipeline.feed(pcm)
    await pipeline.close()
    return pipeline.sink.events


async def test_every_stage_runs_in_order_for_one_utterance():
    # Arrange
    sink = RecordingSink()
    pipeline = VoicePipeline(EnergyVad(), FakeStt(), FakeLlm(), FakeTts(), sink)

    # Act
    events = await collect_events(pipeline, tone(1.5) + silence(0.6))

    # Assert
    assert [(type(event), getattr(event, "kind", None)) for event in events] == [
        (TextEvent, "transcript"),
        (TextEvent, "reply"),
        (SynthesizedSpeech, None),
    ]
    assert events[1].text == f"reply to '{events[0].text}'"
    assert events[2].sample_rate == 24000


async def test_llm_and_tts_are_optional():
    # Arrange
    sink = RecordingSink()
    pipeline = VoicePipeline(EnergyVad(), FakeStt(), None, None, sink)

    # Act
    events = await collect_events(pipeline, tone(1.5) + silence(0.6))

    # Assert
    assert [event.kind for event in events] == ["transcript"]


async def test_a_failing_stage_does_not_stop_later_utterances():
    # Arrange
    sink = RecordingSink()
    pipeline = VoicePipeline(EnergyVad(max_sec=1.0), FlakyStt(), None, None, sink)

    # Act
    events = await collect_events(pipeline, tone(2.5))

    # Assert
    assert [event.text for event in events] == ["utterance 2", "utterance 3"]


def test_energy_vad_discards_silence_only_buffers():
    # Act
    utterances = EnergyVad(min_sec=1.0, max_sec=2.0).push(silence(5.0))

    # Assert
    assert utterances == []


def test_energy_vad_cuts_continuous_speech_at_max_length():
    # Arrange
    vad = EnergyVad(max_sec=2.0)

    # Act
    utterances = vad.push(tone(5.0))

    # Assert
    assert [len(utterance) // 2 for utterance in utterances] == [
        2 * PIPELINE_SAMPLE_RATE,
        2 * PIPELINE_SAMPLE_RATE,
    ]
    assert len(vad.flush()) // 2 == PIPELINE_SAMPLE_RATE


def test_silero_ends_an_utterance_after_a_pause_and_keeps_the_pre_roll():
    # Arrange
    speech_windows = 20
    probabilities = [0.0] * 10 + [0.9] * speech_windows + [0.0] * 10
    vad = SileroVad(threshold=0.5, min_silence_ms=300, session=FakeVadSession(probabilities))

    # Act
    utterances = vad.push(bytes(2 * len(probabilities) * WINDOW_SAMPLES))

    # Assert
    assert len(utterances) == 1
    assert len(utterances[0]) // 2 > speech_windows * WINDOW_SAMPLES + vad.pad_samples
    assert vad.flush() is None


def test_silero_ignores_a_stream_the_model_never_rates_as_speech():
    # Act
    utterances = SileroVad(threshold=0.5, min_silence_ms=500, session=FakeVadSession([])).push(silence(3.0))

    # Assert
    assert utterances == []


async def test_whisper_drops_segments_rated_as_non_speech():
    # Arrange
    def recognizer(_audio: np.ndarray) -> list:
        return [
            SimpleNamespace(text=" ご視聴ありがとうございました", no_speech_prob=0.95),
            SimpleNamespace(text=" こんにちは", no_speech_prob=0.05),
        ]

    stt = WhisperLocalStt(recognizer=recognizer, no_speech_threshold=0.6)

    # Act
    text = await stt.transcribe(tone(1.0))

    # Assert
    assert text == "こんにちは"
