import shutil
import subprocess
from types import SimpleNamespace

import numpy as np
import pytest

from stt_server.pipeline.base import PIPELINE_PCM, PcmFormat, ReplyAudioEvent, SynthesizedSpeech, TextEvent
from stt_server.pipeline.runner import VoicePipeline
from stt_server.pipeline.stt.whisper_local import WhisperLocalStt
from stt_server.pipeline.vad.energy import EnergyVad
from stt_server.pipeline.vad.silero import SileroVad
from tests.conftest import SAMPLE_RATE_16K, silence, tone


class FakeStt:
    async def transcribe(self, utterance: bytes) -> str:
        return f"{len(utterance) // 2} samples"


class FakeLlm:
    async def reply(self, user_text: str) -> str:
        return f"reply to '{user_text}'"


class FakeTts:
    async def synthesize(self, text: str) -> SynthesizedSpeech:
        return SynthesizedSpeech(pcm=tone(0.1, sample_rate=24000), pcm_format=PcmFormat(24000))


class FailingStt:
    async def transcribe(self, utterance: bytes) -> str:
        raise RuntimeError("service down")


async def collect_events(pipeline: VoicePipeline, pcm: bytes) -> list:
    await pipeline.start()
    await pipeline.feed(pcm)
    await pipeline.close()
    return pipeline.sink.events


class RecordingSink:
    def __init__(self) -> None:
        self.events = []

    async def __call__(self, event) -> None:
        self.events.append(event)


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
        (ReplyAudioEvent, None),
    ]
    assert events[1].text == f"reply to '{events[0].text}'"
    assert events[2].speech.pcm_format == PcmFormat(24000)


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
    pipeline = VoicePipeline(EnergyVad(max_sec=1.0), FailingStt(), None, None, sink)

    # Act
    events = await collect_events(pipeline, tone(2.5))

    # Assert
    assert events == []
    assert pipeline._worker is None


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
    assert [len(utterance) // 2 for utterance in utterances] == [2 * SAMPLE_RATE_16K, 2 * SAMPLE_RATE_16K]
    assert len(vad.flush()) // 2 == SAMPLE_RATE_16K


def test_silero_ignores_silence():
    # Act
    utterances = SileroVad().push(silence(3.0))

    # Assert
    assert utterances == []


def test_silero_cuts_speech_into_an_utterance(tmp_path):
    # Arrange
    if shutil.which("say") is None or shutil.which("ffmpeg") is None:
        pytest.skip("needs macOS `say` and ffmpeg to synthesize speech")
    aiff = tmp_path / "speech.aiff"
    subprocess.run(["say", "-o", str(aiff), "Hello, this is a short test sentence."], check=True)
    speech = subprocess.run(
        ["ffmpeg", "-v", "error", "-i", str(aiff), "-f", "s16le", "-ac", "1", "-ar", "16000", "pipe:1"],
        capture_output=True,
        check=True,
    ).stdout

    # Act
    utterances = SileroVad(min_silence_ms=300).push(silence(1.0) + speech + silence(1.0))

    # Assert
    assert utterances
    assert sum(map(len, utterances)) >= PIPELINE_PCM.bytes_per_second()


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
