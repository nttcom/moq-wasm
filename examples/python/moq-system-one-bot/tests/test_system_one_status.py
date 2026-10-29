import pytest

from moq_system_one_bot import system_one_status
from moq_system_one_bot.system_one_status import IDLE_STOP_SECONDS, SystemOneStatus
from tests.helpers import next_status, published_status


class FakeClock:
    def __init__(self):
        self.now = 0.0

    def __call__(self) -> float:
        return self.now


@pytest.fixture
def short_escalation(monkeypatch):
    monkeypatch.setattr(system_one_status, "SLOW_SECONDS", 0.05)
    monkeypatch.setattr(system_one_status, "COLD_START_SECONDS", 0.1)


async def answered_once(status: SystemOneStatus) -> None:
    async with status.request():
        pass


async def test_first_request_reports_a_cold_start_until_system_one_answers():
    # Arrange
    status = SystemOneStatus()
    publication = await published_status(status)
    assert await next_status(publication) == "ready"

    # Act
    async with status.request():
        starting = await next_status(publication)

    # Assert
    assert starting == "starting"
    assert await next_status(publication) == "ready"


async def test_late_answer_while_warm_reports_slow_then_a_cold_start(short_escalation):
    # Arrange
    status = SystemOneStatus()
    await answered_once(status)
    publication = await published_status(status)
    assert await next_status(publication) == "ready"

    # Act
    async with status.request():
        reported = [await next_status(publication), await next_status(publication)]

    # Assert
    assert reported == ["slow", "starting"]
    assert await next_status(publication) == "ready"


async def test_request_after_an_idle_stop_reports_a_cold_start():
    # Arrange
    clock = FakeClock()
    status = SystemOneStatus(clock)
    await answered_once(status)
    publication = await published_status(status)
    assert await next_status(publication) == "ready"
    clock.now += IDLE_STOP_SECONDS + 1

    # Act
    async with status.request():
        starting = await next_status(publication)

    # Assert
    assert starting == "starting"


async def test_failed_request_reports_ready_again():
    # Arrange
    status = SystemOneStatus()
    publication = await published_status(status)
    assert await next_status(publication) == "ready"

    # Act
    with pytest.raises(RuntimeError):
        async with status.request():
            assert await next_status(publication) == "starting"
            raise RuntimeError("d3-flash failed")

    # Assert
    assert await next_status(publication) == "ready"
