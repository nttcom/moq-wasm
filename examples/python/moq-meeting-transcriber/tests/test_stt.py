import asyncio
from dataclasses import dataclass

import numpy as np
from pipecat.frames.frames import VADUserStartedSpeakingFrame, VADUserStoppedSpeakingFrame
from pipecat.processors.frame_processor import FrameDirection

from moq_meeting_transcriber import stt
from moq_meeting_transcriber.audio import SAMPLE_RATE
from moq_meeting_transcriber.stt import (
    NO_SPEECH_PROB,
    TRAILING_SILENCE_SECONDS,
    SharedWhisper,
    UtteranceSTTService,
    UtteranceTranscriptionFrame,
)


@dataclass
class Segment:
    text: str
    no_speech_prob: float


class StubModel:
    def __init__(self, *segments: Segment):
        self.segments = segments
        self.languages: list[str] = []
        self.sample_counts: list[int] = []

    def transcribe(self, samples: np.ndarray, language: str):
        self.languages.append(language)
        self.sample_counts.append(len(samples))
        return iter(self.segments), None


def loaded(model: StubModel) -> asyncio.Future:
    future = asyncio.get_running_loop().create_future()
    future.set_result(model)
    return future


def segment_of(seconds: float) -> bytes:
    return bytes(int((seconds + TRAILING_SILENCE_SECONDS) * SAMPLE_RATE) * 2)


async def transcribe(model: StubModel, segment: bytes, stopped_at: float, monkeypatch) -> list:
    """Closes an utterance as the VAD does, then transcribes `segment` as the service's segment for it."""
    service = UtteranceSTTService(SharedWhisper(loaded(model)), "alice")
    await service.process_frame(VADUserStartedSpeakingFrame(), FrameDirection.DOWNSTREAM)
    monkeypatch.setattr(stt.time, "time", lambda: stopped_at)
    await service.process_frame(VADUserStoppedSpeakingFrame(), FrameDirection.DOWNSTREAM)
    return [frame async for frame in service.run_stt(segment)]


async def test_dates_the_utterance_from_when_its_speech_began(monkeypatch):
    # Arrange
    model = StubModel(Segment(" こんにちは", 0.0), Segment(" 元気", 0.1))

    # Act
    frames = await transcribe(model, segment_of(2.0), stopped_at=1_000.0, monkeypatch=monkeypatch)

    # Assert
    assert frames == [frames[0]]
    assert isinstance(frames[0], UtteranceTranscriptionFrame)
    assert frames[0].text == "こんにちは 元気"
    assert frames[0].user_id == "alice"
    assert frames[0].started_at_ms == 998_000
    assert model.languages == ["ja"]


async def test_a_segment_whisper_rates_as_silence_yields_nothing(monkeypatch):
    # Arrange
    model = StubModel(Segment(" ご視聴ありがとうございました", NO_SPEECH_PROB))

    # Act / Assert
    assert await transcribe(model, segment_of(1.0), stopped_at=1_000.0, monkeypatch=monkeypatch) == []


async def test_whisper_is_ready_once_its_model_has_loaded():
    # Arrange
    model = asyncio.get_running_loop().create_future()
    ready = asyncio.ensure_future(SharedWhisper(model).ready())
    await asyncio.sleep(0)
    waiting = not ready.done()

    # Act
    model.set_result(StubModel())
    await asyncio.wait_for(ready, 1)

    # Assert
    assert waiting
