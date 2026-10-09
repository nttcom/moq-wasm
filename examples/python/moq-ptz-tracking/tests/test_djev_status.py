import pytest

from moq_ptz_tracking import djev_status
from moq_ptz_tracking.djev_status import IDLE_STOP_SECONDS, DjevStatus
from tests.helpers import STATUS_TRACK_NAME, next_status, published_status


class FakeClock:
    def __init__(self):
        self.now = 0.0

    def __call__(self) -> float:
        return self.now


@pytest.fixture
def short_escalation(monkeypatch):
    monkeypatch.setattr(djev_status, "SLOW_SECONDS", 0.05)
    monkeypatch.setattr(djev_status, "COLD_START_SECONDS", 0.1)


async def answered_once(status: DjevStatus) -> None:
    async with status.request():
        pass


async def test_first_request_reports_a_cold_start_until_djev_answers():
    # Arrange
    status = DjevStatus(STATUS_TRACK_NAME)
    track = await published_status(status)
    assert await next_status(track) == "ready"

    # Act
    async with status.request():
        starting = await next_status(track)

    # Assert
    assert starting == "starting"
    assert await next_status(track) == "ready"


async def test_late_answer_while_warm_reports_slow_then_a_cold_start(short_escalation):
    # Arrange
    status = DjevStatus(STATUS_TRACK_NAME)
    await answered_once(status)
    track = await published_status(status)
    assert await next_status(track) == "ready"

    # Act
    async with status.request():
        reported = [await next_status(track), await next_status(track)]

    # Assert
    assert reported == ["slow", "starting"]
    assert await next_status(track) == "ready"


async def test_request_after_an_idle_stop_reports_a_cold_start():
    # Arrange
    clock = FakeClock()
    status = DjevStatus(STATUS_TRACK_NAME, clock)
    await answered_once(status)
    track = await published_status(status)
    assert await next_status(track) == "ready"
    clock.now += IDLE_STOP_SECONDS + 1

    # Act
    async with status.request():
        starting = await next_status(track)

    # Assert
    assert starting == "starting"


async def test_failed_request_reports_ready_again():
    # Arrange
    status = DjevStatus(STATUS_TRACK_NAME)
    track = await published_status(status)
    assert await next_status(track) == "ready"

    # Act
    with pytest.raises(RuntimeError):
        async with status.request():
            assert await next_status(track) == "starting"
            raise RuntimeError("djev failed")

    # Assert
    assert await next_status(track) == "ready"
