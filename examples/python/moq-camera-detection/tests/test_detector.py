import asyncio
import time
from dataclasses import dataclass, field

import pytest
from pipecat.tests.utils import SleepFrame, run_test

from moq_camera_detection.detector import STALE_PICTURE_SECONDS, CameraPicture, PersonDetector
from tests.helpers import StubVision, next_records, published_timeline

NO_MORE_RECORDS_WAIT_SECONDS = 0.2


def fresh_picture(location: tuple[int, int], jpeg: bytes = b"jpeg") -> CameraPicture:
    return CameraPicture(jpeg=jpeg, location=location, decoded_at=time.monotonic())


@dataclass
class LateFirstAnswerVision:
    first_answer_released: asyncio.Event = field(default_factory=asyncio.Event)

    async def has_person(self, jpeg: bytes) -> bool:
        if jpeg == b"first":
            await self.first_answer_released.wait()
        else:
            self.first_answer_released.set()
        return jpeg == b"first"


async def test_fresh_picture_verdict_is_recorded_at_its_location():
    # Arrange
    timeline, group = await published_timeline()
    vision = StubVision(person=True)

    # Act
    await run_test(
        PersonDetector(vision, timeline),
        frames_to_send=[fresh_picture((42, 3)), SleepFrame()],
        expected_down_frames=[],
    )

    # Assert
    assert await next_records(group) == [{"l": [42, 3], "data": {"person": True}}]


async def test_stale_picture_is_not_judged():
    # Arrange
    timeline, _ = await published_timeline()
    vision = StubVision(person=True)
    decoded_at = time.monotonic() - STALE_PICTURE_SECONDS - 1

    # Act
    await run_test(
        PersonDetector(vision, timeline),
        frames_to_send=[CameraPicture(jpeg=b"jpeg", location=(43, 0), decoded_at=decoded_at)],
        expected_down_frames=[],
    )

    # Assert
    assert vision.images == []


async def test_verdict_answered_after_a_newer_one_is_not_recorded():
    # Arrange
    timeline, group = await published_timeline()

    # Act
    await run_test(
        PersonDetector(LateFirstAnswerVision(), timeline),
        frames_to_send=[fresh_picture((44, 0), b"first"), fresh_picture((44, 1), b"second"), SleepFrame()],
        expected_down_frames=[],
    )

    # Assert
    assert await next_records(group) == [{"l": [44, 1], "data": {"person": False}}]
    with pytest.raises(TimeoutError):
        await asyncio.wait_for(group.read_frame(), NO_MORE_RECORDS_WAIT_SECONDS)
