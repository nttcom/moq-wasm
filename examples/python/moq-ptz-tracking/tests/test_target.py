import pytest

from moq_ptz_tracking.target import parse_prompt
from tests.helpers import MUG_TARGET


def test_prompt_with_targets_starts_tracking_them_on_its_video_track():
    # Act
    target = parse_prompt(b'{"targets": ["a red mug"], "video": "video/profile_1"}', (7, 0))

    # Assert
    assert target == MUG_TARGET


def test_repeated_targets_are_asked_for_once():
    # Act
    target = parse_prompt('{"targets": ["人", "椅子", "人"], "video": "video/profile_1"}'.encode(), (7, 0))

    # Assert
    assert target.descriptions == ("人", "椅子")


def test_prompt_without_targets_stops_tracking():
    # Act / Assert
    assert parse_prompt(b'{"targets": []}', (8, 0)) is None


@pytest.mark.parametrize(
    "payload",
    [
        b"not json",
        b'["a red mug"]',
        b'{"target": "a red mug", "video": "video/profile_1"}',
        b'{"targets": "a red mug", "video": "video/profile_1"}',
        b'{"targets": [""], "video": "video/profile_1"}',
        b'{"targets": ["' + b"x" * 101 + b'"], "video": "video/profile_1"}',
        b'{"targets": [' + b",".join(b'"%d"' % n for n in range(17)) + b'], "video": "video/profile_1"}',
        b'{"targets": ["a red mug"]}',
        b'{"targets": ["a red mug"], "video": ""}',
    ],
)
def test_invalid_prompt_is_rejected(payload):
    # Act / Assert
    with pytest.raises(ValueError):
        parse_prompt(payload, (9, 0))
