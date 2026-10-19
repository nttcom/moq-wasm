from datetime import datetime

import pytest

from moq_bot_rooms.open_hours import OpenHours
from tests.helpers import WEEKDAY_HOURS


@pytest.mark.parametrize(
    ("moment", "expected"),
    [
        ("2026-10-19T09:00:00+09:00", True),
        ("2026-10-19T17:29:59+09:00", True),
        ("2026-10-19T17:30:00+09:00", False),
        ("2026-10-19T08:59:59+09:00", False),
        ("2026-10-18T12:00:00+09:00", False),
        ("2026-10-19T00:30:00+00:00", True),
    ],
)
def test_contains_compares_local_time_at_the_offset(moment, expected):
    # Act / Assert: Monday 2026-10-19 09:00–17:30 at +09:00; the last case is 09:30 there
    assert WEEKDAY_HOURS.contains(datetime.fromisoformat(moment)) is expected


@pytest.mark.parametrize(
    "text",
    ["Mon-Fri 9:00-17:30 +09:00", "Mon-Fri 09:00-17:30", "Fri-Mon 09:00-17:30 +09:00", "Mon-Fri 18:00-09:00 +09:00"],
)
def test_parse_rejects_malformed_or_wrapping_hours(text):
    # Act / Assert
    with pytest.raises(ValueError):
        OpenHours.parse(text)
