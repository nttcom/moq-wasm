import asyncio
import gc

import moq
import pytest

from moq_ptz_tracking.ptz import PtzCalibration, PtzCommands, PtzMove
from moq_ptz_tracking.vision import Position
from tests.helpers import COMMAND_TRACK_NAME, READ_TIMEOUT_SECONDS, next_command, next_move, published_commands

TAPO = PtzCalibration(pan_per_second=0.27, tilt_per_second=0.4, dead_zone=0.05)
SHORT_TURN_SECONDS = 0.02


@pytest.mark.parametrize(
    ("position", "move"),
    [
        (Position(0.54, 0.46), PtzMove(0.0, 0.0)),
        (Position(0.77, 0.5), PtzMove(1.0, 0.0)),
        (Position(0.5, 0.1), PtzMove(0.0, 1.0)),
        (Position(0.2, 0.7), PtzMove(-1.11, -0.5)),
    ],
)
def test_turn_lasts_in_proportion_to_the_offset_outside_the_dead_zone(position, move):
    # Act / Assert
    assert TAPO.toward(position) == move


async def test_move_is_one_group_turning_each_axis_at_the_move_velocity_until_its_own_stop():
    # Arrange
    commands, track = await published_commands()

    # Act
    await commands.move(PtzMove(SHORT_TURN_SECONDS, -SHORT_TURN_SECONDS))

    # Assert
    assert await next_move(track) == [
        {"type": "continuous", "pan": 0.5, "tilt": 0.0, "zoom": 0.0, "speed": 1.0},
        {"type": "stop"},
        {"type": "continuous", "pan": 0.0, "tilt": -0.5, "zoom": 0.0, "speed": 1.0},
        {"type": "stop"},
    ]


async def test_cancelled_turn_still_stops_the_camera():
    # Arrange
    commands, track = await published_commands()
    moving = asyncio.create_task(commands.move(PtzMove(10.0, 0.0)))
    group = await asyncio.wait_for(track.next_group(), READ_TIMEOUT_SECONDS)
    await next_command(group)

    # Act
    moving.cancel()

    # Assert
    assert await next_command(group) == {"type": "stop"}
    assert await next_command(group) is None


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
