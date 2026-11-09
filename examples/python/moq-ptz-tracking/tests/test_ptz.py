import asyncio

import pytest

from moq_ptz_tracking.ptz import PtzCalibration, PtzCommands, PtzMove
from moq_ptz_tracking.vision import Box
from tests.helpers import published_commands

TAPO = PtzCalibration(pan_per_second=0.27, tilt_per_second=0.4, margin=0.1, dead_zone=0.05)
SHORT_TURN_SECONDS = 0.02


@pytest.mark.parametrize(
    ("box", "move"),
    [
        (Box(0.6, 0.2, 0.85, 0.8), PtzMove(0.0, 0.0)),
        (Box(0.7, 0.3, 0.95, 0.7), PtzMove(1.2, 0.0)),
        (Box(0.4, 0.0, 0.6, 0.2), PtzMove(0.0, 1.0)),
        (Box(0.05, 0.6, 0.35, 0.95), PtzMove(-1.11, -0.69)),
    ],
)
def test_box_leaving_the_margin_turns_that_axis_until_it_is_centered(box, move):
    # Act / Assert
    assert TAPO.toward(box) == move


def test_box_too_large_to_fit_stays_still_once_centered():
    # Act / Assert
    assert TAPO.toward(Box(0.0, 0.02, 0.96, 1.0)) == PtzMove(0.0, 0.0)


async def test_move_is_one_group_turning_each_axis_at_the_move_velocity_until_its_own_stop():
    # Arrange
    commands, publication = await published_commands()

    # Act
    await commands.move(PtzMove(SHORT_TURN_SECONDS, -SHORT_TURN_SECONDS))

    # Assert
    assert await publication.next_group() == [
        {"type": "continuous", "pan": 0.5, "tilt": 0.0, "zoom": 0.0, "speed": 1.0},
        {"type": "stop"},
        {"type": "continuous", "pan": 0.0, "tilt": -0.5, "zoom": 0.0, "speed": 1.0},
        {"type": "stop"},
    ]


async def test_cancelled_turn_still_stops_the_camera():
    # Arrange
    commands, publication = await published_commands()
    moving = asyncio.create_task(commands.move(PtzMove(10.0, 0.0)))
    await publication.next_write()

    # Act
    moving.cancel()

    # Assert
    assert await publication.next_write() == (False, {"type": "stop"})
