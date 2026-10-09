import asyncio
import gc

import moq
import pytest

from moq_ptz_tracking.ptz import PanTilt, PtzCommands, PtzSteps
from moq_ptz_tracking.vision import NOT_VISIBLE
from tests.helpers import COMMAND_TRACK_NAME, READ_TIMEOUT_SECONDS, next_command, published_commands

STEPS = PtzSteps(pan=0.05, tilt=0.1)


@pytest.mark.parametrize(
    ("position", "move"),
    [
        ("top left", PanTilt(-0.05, 0.1)),
        ("middle right", PanTilt(0.05, 0.0)),
        ("bottom center", PanTilt(0.0, -0.1)),
        ("center", PanTilt(0.0, 0.0)),
        (NOT_VISIBLE, PanTilt(0.0, 0.0)),
    ],
)
def test_camera_moves_toward_the_grid_cell_of_the_target(position, move):
    # Act / Assert
    assert STEPS.toward(position) == move


async def test_each_move_is_one_relative_move_command_in_its_own_group():
    # Arrange
    commands, track = await published_commands()

    # Act
    commands.relative_move(PanTilt(-0.05, 0.1))
    commands.relative_move(PanTilt(0.05, 0.0))

    # Assert
    assert await next_command(track) == {"type": "relative", "pan": -0.05, "tilt": 0.1, "zoom": 0.0, "speed": 0.5}
    assert (await next_command(track))["pan"] == 0.05


async def test_command_broadcast_stays_announced_without_a_caller_reference():
    # Arrange
    origin = moq.OriginProducer()
    commands = PtzCommands(origin.create_broadcast("anon/onvif/viewer"), COMMAND_TRACK_NAME)

    # Act
    gc.collect()

    # Assert
    announcement = await asyncio.wait_for(anext(origin.consume().announced()), READ_TIMEOUT_SECONDS)
    assert announcement.path == "anon/onvif/viewer"
    assert commands is not None
