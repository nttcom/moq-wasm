import asyncio
import time
from dataclasses import dataclass

import pytest
from pipecat.tests.utils import SleepFrame, run_test

from moq_ptz_tracking.ptz import PtzSteps
from moq_ptz_tracking.target import LatestTarget
from moq_ptz_tracking.tracker import STALE_PICTURE_SECONDS, CameraPicture, PtzTracker
from tests.helpers import (
    MUG_TARGET,
    StubVision,
    next_command,
    next_records,
    published_commands,
    published_timeline,
    tracking,
)

STEPS = PtzSteps(pan=0.05, tilt=0.1)
NOTHING_MORE_WAIT_SECONDS = 0.2


def fresh_picture(location: tuple[int, int]) -> CameraPicture:
    return CameraPicture(jpeg=b"jpeg", location=location, decoded_at=time.monotonic(), target=MUG_TARGET)


@dataclass
class StoppingVision:
    latest_target: LatestTarget

    async def locate(self, jpeg: bytes, description: str) -> str:
        self.latest_target.set(None)
        return "top left"


async def test_off_center_target_moves_the_camera_and_records_the_move():
    # Arrange
    timeline, group = await published_timeline()
    commands, command_track = await published_commands()
    tracker = PtzTracker(StubVision("top left"), timeline, commands, STEPS, tracking(MUG_TARGET))

    # Act
    await run_test(tracker, frames_to_send=[fresh_picture((42, 3)), SleepFrame()], expected_down_frames=[])

    # Assert
    assert (await next_command(command_track))["pan"] == -0.05
    assert await next_records(group) == [
        {"l": [42, 3], "data": {"prompt": [7, 0], "position": "top left", "pan": -0.05, "tilt": 0.1}}
    ]


async def test_centered_target_is_recorded_without_moving_the_camera():
    # Arrange
    timeline, group = await published_timeline()
    commands, command_track = await published_commands()
    tracker = PtzTracker(StubVision("center"), timeline, commands, STEPS, tracking(MUG_TARGET))

    # Act
    await run_test(tracker, frames_to_send=[fresh_picture((42, 3)), SleepFrame()], expected_down_frames=[])

    # Assert
    assert (await next_records(group))[0]["data"]["position"] == "center"
    with pytest.raises(TimeoutError):
        await asyncio.wait_for(command_track.next_group(), NOTHING_MORE_WAIT_SECONDS)


async def test_pictures_decoded_before_the_camera_settles_are_not_located():
    # Arrange
    timeline, _ = await published_timeline()
    commands, _ = await published_commands()
    vision = StubVision("top left")
    tracker = PtzTracker(vision, timeline, commands, STEPS, tracking(MUG_TARGET))

    # Act
    await run_test(
        tracker,
        frames_to_send=[fresh_picture((42, 0)), SleepFrame(), fresh_picture((42, 1)), SleepFrame()],
        expected_down_frames=[],
    )

    # Assert
    assert len(vision.images) == 1


async def test_picture_arriving_while_another_is_located_is_skipped():
    # Arrange
    timeline, _ = await published_timeline()
    commands, _ = await published_commands()
    vision = StubVision("center")
    tracker = PtzTracker(vision, timeline, commands, STEPS, tracking(MUG_TARGET))

    # Act
    await run_test(
        tracker,
        frames_to_send=[fresh_picture((42, 0)), fresh_picture((42, 1)), SleepFrame()],
        expected_down_frames=[],
    )

    # Assert
    assert len(vision.images) == 1


async def test_stale_picture_is_not_located():
    # Arrange
    timeline, _ = await published_timeline()
    commands, _ = await published_commands()
    vision = StubVision("top left")
    tracker = PtzTracker(vision, timeline, commands, STEPS, tracking(MUG_TARGET))
    decoded_at = time.monotonic() - STALE_PICTURE_SECONDS - 1

    # Act
    await run_test(
        tracker,
        frames_to_send=[CameraPicture(jpeg=b"jpeg", location=(43, 0), decoded_at=decoded_at, target=MUG_TARGET)],
        expected_down_frames=[],
    )

    # Assert
    assert vision.images == []


async def test_answer_for_a_target_stopped_meanwhile_does_not_move_the_camera():
    # Arrange
    timeline, group = await published_timeline()
    commands, command_track = await published_commands()
    latest_target = tracking(MUG_TARGET)
    tracker = PtzTracker(StoppingVision(latest_target), timeline, commands, STEPS, latest_target)

    # Act
    await run_test(tracker, frames_to_send=[fresh_picture((44, 0)), SleepFrame()], expected_down_frames=[])

    # Assert
    with pytest.raises(TimeoutError):
        await asyncio.wait_for(command_track.next_group(), NOTHING_MORE_WAIT_SECONDS)
    with pytest.raises(TimeoutError):
        await asyncio.wait_for(group.read_frame(), NOTHING_MORE_WAIT_SECONDS)
