import asyncio
import json
from dataclasses import dataclass, field

from moq_wasm import Location
from moq_wasm.pipecat import MoqWasmEventRecord, MoqWasmTrackMessage
from pipecat.frames.frames import (
    InputTransportMessageFrame,
    OutputTransportMessageFrame,
    UserImageRawFrame,
    UserImageRequestFrame,
)
from pipecat.tests.utils import SleepFrame, run_test

from moq_camera_detection.detector import EVENT_TIMELINE_TRACK, CameraDetector, _rgb_to_jpeg
from moq_camera_detection.prompt import Prompt
from tests.helpers import StubVision

SIZE = (16, 16)
PROMPT = json.dumps({"questions": [{"type": "choice", "question": "Is there a person?", "choices": ["yes", "no"]}]}).encode()


def prompt_message(location: tuple[int, int] = (7, 0)) -> InputTransportMessageFrame:
    return InputTransportMessageFrame(message=MoqWasmTrackMessage("camera", "prompt", PROMPT, Location(*location)))


def camera_picture(location: tuple[int, int], value: int = 0) -> UserImageRawFrame:
    frame = UserImageRawFrame(
        image=bytes([value]) * (SIZE[0] * SIZE[1] * 3),
        size=SIZE,
        format="RGB",
        user_id="camera",
        request=UserImageRequestFrame(user_id="camera"),
    )
    frame.metadata["location"] = Location(*location)
    return frame


def records(frames) -> list[dict]:
    return [
        frame.message.record
        for frame in frames
        if isinstance(frame, OutputTransportMessageFrame) and isinstance(frame.message, MoqWasmEventRecord)
    ]


@dataclass
class LateFirstAnswerVision:
    """Answers the first picture (value 0) only after it answered any other."""

    first_jpeg: bytes
    other_answered: asyncio.Event = field(default_factory=asyncio.Event)

    async def answer(self, jpeg: bytes, prompt: Prompt) -> list[str]:
        if jpeg == self.first_jpeg:
            await self.other_answered.wait()
            return ["yes"]
        self.other_answered.set()
        return ["no"]


async def test_a_pictures_answer_is_recorded_at_its_location_with_the_prompts():
    # Arrange
    vision = StubVision(answers=["yes"])

    # Act
    down, _ = await run_test(
        CameraDetector(vision),
        frames_to_send=[prompt_message(), camera_picture((42, 3)), SleepFrame(0.1)],
    )

    # Assert
    assert records(down) == [{"L": [42, 3], "data": {"prompt": (7, 0), "answers": ["yes"]}}]
    assert down[-1].message.track == EVENT_TIMELINE_TRACK


async def test_pictures_are_asked_for_only_once_a_prompt_came():
    # Act
    _, up_without = await run_test(CameraDetector(StubVision(answers=["yes"])), frames_to_send=[SleepFrame(0.3)])
    _, up_with = await run_test(
        CameraDetector(StubVision(answers=["yes"])), frames_to_send=[prompt_message(), SleepFrame(0.3)]
    )

    # Assert: one request waits for its picture before the next is made
    assert not [frame for frame in up_without if isinstance(frame, UserImageRequestFrame)]
    assert [frame.user_id for frame in up_with if isinstance(frame, UserImageRequestFrame)] == ["camera"]


async def test_an_answer_returned_after_a_newer_one_is_not_recorded():
    # Act
    down, _ = await run_test(
        CameraDetector(LateFirstAnswerVision(_rgb_to_jpeg(camera_picture((0, 0)).image, SIZE))),
        frames_to_send=[prompt_message(), camera_picture((44, 0)), camera_picture((44, 1), 10), SleepFrame(0.1)],
    )

    # Assert
    assert records(down) == [{"L": [44, 1], "data": {"prompt": (7, 0), "answers": ["no"]}}]


async def test_a_request_no_picture_answers_drops_the_prompt():
    # Arrange
    detector = CameraDetector(StubVision(answers=["yes"]))

    async def fail_requests():
        await asyncio.sleep(0.15)
        await detector._request_failed({"error": "camera left"})

    failing = asyncio.create_task(fail_requests())

    # Act
    _, up = await run_test(detector, frames_to_send=[prompt_message(), SleepFrame(0.5)])
    await failing

    # Assert: the failed request was the only one
    assert len([frame for frame in up if isinstance(frame, UserImageRequestFrame)]) == 1
