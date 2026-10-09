import asyncio
import time
from dataclasses import dataclass

import pytest
from pipecat.tests.utils import SleepFrame, run_test

from moq_ptz_tracking.ptz import PtzCalibration
from moq_ptz_tracking.target import LatestTarget
from moq_ptz_tracking.tracker import SETTLE_SECONDS, CameraPicture, PtzTracker
from moq_ptz_tracking.vision import Answer, Position
from tests.helpers import (
    MUG_TARGET,
    StubVision,
    next_move,
    next_records,
    published_commands,
    published_timeline,
    tracking,
)

CALIBRATION = PtzCalibration(pan_per_second=10.0, tilt_per_second=10.0, dead_zone=0.05)
OFF_CENTER = Position(0.75, 0.5)
NOTHING_MORE_WAIT_SECONDS = 0.2


def picture(location: tuple[int, int], decoded_at: float | None = None):
    return CameraPicture(
        jpeg=b"jpeg",
        location=location,
        decoded_at=time.monotonic() if decoded_at is None else decoded_at,
        target=MUG_TARGET,
    )


@dataclass
class StoppingVision:
    latest_target: LatestTarget

    async def locate(self, jpeg: bytes, description: str) -> Position:
        self.latest_target.set(None)
        return OFF_CENTER


async def tracker_with(vision, latest_target: LatestTarget | None = None):
    timeline, group = await published_timeline()
    commands, command_track = await published_commands()
    latest_target = latest_target or tracking(MUG_TARGET)
    return PtzTracker(vision, timeline, commands, CALIBRATION, latest_target), group, command_track


async def test_off_center_target_moves_the_camera_and_records_where_it_was():
    # Arrange
    tracker, group, command_track = await tracker_with(StubVision(OFF_CENTER))

    # Act
    await run_test(tracker, frames_to_send=[picture((42, 3)), SleepFrame()], expected_down_frames=[])

    # Assert
    assert [command["type"] for command in await next_move(command_track)] == ["continuous", "stop"]
    assert await next_records(group) == [
        {
            "l": [42, 3],
            "data": {"prompt": [7, 0], "position": [0.75, 0.5], "pan_seconds": 0.03, "tilt_seconds": 0.0},
        }
    ]


@pytest.mark.parametrize(
    ("answer", "recorded"),
    [(Position(0.52, 0.47), [0.52, 0.47]), (Answer.NOT_VISIBLE, "not visible"), (Answer.UNREADABLE, None)],
)
async def test_target_near_the_center_or_not_located_leaves_the_camera_still(answer, recorded):
    # Arrange
    tracker, group, command_track = await tracker_with(StubVision(answer))

    # Act
    await run_test(tracker, frames_to_send=[picture((42, 3)), SleepFrame()], expected_down_frames=[])

    # Assert
    assert (await next_records(group))[0]["data"]["position"] == recorded
    with pytest.raises(TimeoutError):
        await asyncio.wait_for(command_track.next_group(), NOTHING_MORE_WAIT_SECONDS)


async def test_after_a_move_pictures_are_located_only_once_the_camera_has_settled():
    # Arrange
    vision = StubVision(OFF_CENTER)
    tracker, _, _ = await tracker_with(vision)

    # Act
    await run_test(
        tracker,
        frames_to_send=[
            picture((42, 0)),
            SleepFrame(),
            picture((42, 1)),
            picture((42, 2), decoded_at=time.monotonic() + SETTLE_SECONDS + 1.0),
            SleepFrame(),
        ],
        expected_down_frames=[],
    )

    # Assert
    assert len(vision.images) == 2


async def test_picture_arriving_while_another_is_located_is_skipped():
    # Arrange
    vision = StubVision(Position(0.5, 0.5))
    tracker, _, _ = await tracker_with(vision)

    # Act
    await run_test(tracker, frames_to_send=[picture((42, 0)), picture((42, 1)), SleepFrame()], expected_down_frames=[])

    # Assert
    assert len(vision.images) == 1


async def test_answer_for_a_target_stopped_meanwhile_does_not_move_the_camera():
    # Arrange
    latest_target = tracking(MUG_TARGET)
    tracker, group, command_track = await tracker_with(StoppingVision(latest_target), latest_target)

    # Act
    await run_test(tracker, frames_to_send=[picture((44, 0)), SleepFrame()], expected_down_frames=[])

    # Assert
    with pytest.raises(TimeoutError):
        await asyncio.wait_for(command_track.next_group(), NOTHING_MORE_WAIT_SECONDS)
    with pytest.raises(TimeoutError):
        await asyncio.wait_for(group.read_frame(), NOTHING_MORE_WAIT_SECONDS)
