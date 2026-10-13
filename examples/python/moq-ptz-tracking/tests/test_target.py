import asyncio

import pytest

from moq_ptz_tracking.target import LatestTarget, Target, parse_prompt
from tests.helpers import MUG_TARGET, READ_TIMEOUT_SECONDS, tracking


def test_prompt_with_a_target_starts_tracking_on_its_video_track():
    # Act
    target = parse_prompt(b'{"target": "a red mug", "video": "video/profile_1"}', (7, 0))

    # Assert
    assert target == MUG_TARGET


def test_prompt_without_a_target_stops_tracking():
    # Act / Assert
    assert parse_prompt(b'{"target": null}', (8, 0)) is None


@pytest.mark.parametrize(
    "payload",
    [
        b"not json",
        b'["a red mug"]',
        b'{"target": "", "video": "video/profile_1"}',
        b'{"target": "' + b"x" * 101 + b'", "video": "video/profile_1"}',
        b'{"target": "a red mug"}',
        b'{"target": "a red mug", "video": ""}',
    ],
)
def test_invalid_prompt_is_rejected(payload):
    # Act / Assert
    with pytest.raises(ValueError):
        parse_prompt(payload, (9, 0))


async def test_a_new_target_on_the_same_video_track_keeps_watching_it():
    # Arrange
    latest_target = tracking(MUG_TARGET)
    leaving = asyncio.create_task(latest_target.leaves(MUG_TARGET.video_track))
    await asyncio.sleep(0)

    # Act
    latest_target.set(Target("a blue cap", MUG_TARGET.video_track, (8, 0)))
    await asyncio.sleep(0)

    # Assert
    assert not leaving.done()
    leaving.cancel()


@pytest.mark.parametrize("next_target", [None, Target("a red mug", "video/profile_2", (8, 0))])
async def test_stopping_or_switching_video_tracks_leaves_the_watched_track(next_target):
    # Arrange
    latest_target = tracking(MUG_TARGET)
    leaving = asyncio.create_task(latest_target.leaves(MUG_TARGET.video_track))
    await asyncio.sleep(0)

    # Act
    latest_target.set(next_target)

    # Assert
    await asyncio.wait_for(leaving, READ_TIMEOUT_SECONDS)


async def test_waiting_for_a_change_returns_when_a_target_is_set():
    # Arrange
    latest_target = LatestTarget()
    changed = asyncio.create_task(latest_target.changed())
    await asyncio.sleep(0)

    # Act
    latest_target.set(MUG_TARGET)

    # Assert
    await asyncio.wait_for(changed, READ_TIMEOUT_SECONDS)
