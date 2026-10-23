import asyncio
import threading

import numpy as np

from moq_meeting_transcriber.audio import SAMPLE_RATE
from moq_meeting_transcriber.granite import SharedGranite


class StubPipeline:
    def __init__(self, text: str = "こんにちは"):
        self.text = text
        self.inputs: list[dict] = []

    def __call__(self, inputs: dict) -> dict:
        self.inputs.append(inputs)
        return {"text": self.text}


async def test_the_model_loads_once_when_a_room_first_asks_for_it():
    # Arrange
    loads: list[StubPipeline] = []

    def load() -> StubPipeline:
        loads.append(StubPipeline())
        return loads[-1]

    granite = SharedGranite(load)
    loaded_before_ready = list(loads)

    # Act
    await asyncio.gather(granite.ready(), granite.ready())
    text = await granite.recognize(bytes(3200))

    # Assert
    assert loaded_before_ready == []
    assert len(loads) == 1
    assert text == "こんにちは"


async def test_recognize_hands_the_pipeline_float_samples_at_the_sample_rate():
    # Arrange
    model = StubPipeline()
    granite = SharedGranite(lambda: model)
    pcm = np.array([0, 16384, -32768], dtype=np.int16).tobytes()

    # Act
    await granite.recognize(pcm)

    # Assert
    [inputs] = model.inputs
    assert inputs["sampling_rate"] == SAMPLE_RATE
    assert inputs["raw"].tolist() == [0.0, 0.5, -1.0]


async def test_ready_waits_until_the_model_has_loaded():
    # Arrange
    release = threading.Event()

    def load() -> StubPipeline:
        release.wait()
        return StubPipeline()

    granite = SharedGranite(load)
    ready = asyncio.ensure_future(granite.ready())
    await asyncio.sleep(0.05)
    waiting = not ready.done()

    # Act
    release.set()
    await asyncio.wait_for(ready, 1)

    # Assert
    assert waiting
