import pytest

from moq_d3_flash_bot import d3_flash_status
from moq_d3_flash_bot.d3_flash_status import IDLE_STOP_SECONDS, D3FlashStatus
from tests.helpers import next_status, published_status


class FakeClock:
    def __init__(self):
        self.now = 0.0

    def __call__(self) -> float:
        return self.now


@pytest.fixture
def short_escalation(monkeypatch):
    monkeypatch.setattr(d3_flash_status, "SLOW_SECONDS", 0.05)
    monkeypatch.setattr(d3_flash_status, "COLD_START_SECONDS", 0.1)


async def answered_once(status: D3FlashStatus) -> None:
    async with status.request():
        pass


async def test_first_request_reports_a_cold_start_until_d3_flash_answers():
    # Arrange
    status = D3FlashStatus()
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
    status = D3FlashStatus()
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
    status = D3FlashStatus(clock)
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
    status = D3FlashStatus()
    publication = await published_status(status)
    assert await next_status(publication) == "ready"

    # Act
    with pytest.raises(RuntimeError):
        async with status.request():
            assert await next_status(publication) == "starting"
            raise RuntimeError("d3-flash failed")

    # Assert
    assert await next_status(publication) == "ready"
