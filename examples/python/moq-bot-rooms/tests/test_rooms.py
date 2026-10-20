import asyncio

from moq_bot_rooms.rooms import RoomEnd
from tests.helpers import CLOSED_MOMENT, OPEN_MOMENT, WEEKDAY_HOURS, FakeBots, FakeNow, make_rooms, settle


async def test_pages_in_one_room_share_its_bot():
    # Arrange
    bots = FakeBots()
    rooms = make_rooms(bots)

    # Act
    first = rooms.join("abc")
    second = rooms.join("abc")
    await settle()

    # Assert
    assert bots.started == ["abc"]
    assert bots.running == {"abc"}
    first.leave()
    second.leave()


async def test_bot_stops_once_the_last_page_has_been_gone_for_the_grace_period():
    # Arrange
    bots = FakeBots()
    rooms = make_rooms(bots)
    first = rooms.join("abc")
    second = rooms.join("abc")
    await settle()

    # Act
    first.leave()
    await settle()
    still_held = set(bots.running)
    second.leave()
    await settle()

    # Assert
    assert still_held == {"abc"}
    assert bots.running == set()


async def test_page_that_returns_within_the_grace_period_keeps_the_bot():
    # Arrange
    bots = FakeBots()
    rooms = make_rooms(bots)
    rooms.join("abc").leave()

    # Act
    again = rooms.join("abc")
    await settle()

    # Assert
    assert bots.started == ["abc"]
    assert bots.running == {"abc"}
    again.leave()


async def test_expired_room_refuses_pages_until_they_are_gone_then_starts_anew():
    # Arrange
    bots = FakeBots()
    rooms = make_rooms(bots, max_room_seconds=0.05)
    page = rooms.join("abc")

    # Act
    ended = await asyncio.wait_for(page.ended(), 1)
    reconnect = await rooms.join("abc").ended()
    page.leave()
    await settle()
    later = rooms.join("abc")
    await asyncio.sleep(0)

    # Assert
    assert ended == RoomEnd.EXPIRED
    assert reconnect == RoomEnd.EXPIRED
    assert bots.started == ["abc", "abc"]
    later.leave()


async def test_new_room_beyond_max_rooms_is_refused_but_a_held_room_is_not():
    # Arrange
    bots = FakeBots()
    rooms = make_rooms(bots, max_rooms=1)
    first = rooms.join("abc")

    # Act
    other = await rooms.join("xyz").ended()
    same = rooms.join("abc")
    await settle()

    # Assert
    assert other == RoomEnd.FULL
    assert bots.started == ["abc"]
    first.leave()
    same.leave()


async def test_room_outside_the_opening_hours_is_refused():
    # Arrange
    bots = FakeBots()
    rooms = make_rooms(bots, open_hours=WEEKDAY_HOURS, now=FakeNow(CLOSED_MOMENT))

    # Act
    reason = await rooms.join("abc").ended()

    # Assert
    assert reason == RoomEnd.CLOSED
    assert bots.started == []


async def test_closing_time_stops_running_rooms():
    # Arrange
    bots = FakeBots()
    now = FakeNow(OPEN_MOMENT)
    rooms = make_rooms(bots, open_hours=WEEKDAY_HOURS, now=now)
    page = rooms.join("abc")
    await settle()

    # Act
    now.moment = CLOSED_MOMENT
    rooms.close_outside_hours()
    reason = await page.ended()
    await settle()

    # Assert
    assert reason == RoomEnd.CLOSED
    assert bots.running == set()
    page.leave()


async def test_failed_bot_ends_the_room_for_its_pages():
    # Arrange
    bots = FakeBots(failing={"abc"})
    rooms = make_rooms(bots)

    # Act
    page = rooms.join("abc")
    reason = await asyncio.wait_for(page.ended(), 1)

    # Assert
    assert reason == RoomEnd.FAILED
    page.leave()


async def test_shutdown_stops_every_bot_without_a_reason_and_refuses_new_pages():
    # Arrange
    bots = FakeBots()
    rooms = make_rooms(bots)
    page = rooms.join("abc")
    await settle()

    # Act
    await rooms.shutdown()

    # Assert
    assert await page.ended() is None
    assert bots.running == set()
    assert await rooms.join("xyz").ended() is None
    page.leave()


async def test_the_join_that_starts_a_room_picks_its_bot():
    # Arrange
    default_bots = FakeBots()
    picked_bots = FakeBots()
    rooms = make_rooms(default_bots)

    # Act
    first = rooms.join("abc", picked_bots.run)
    second = rooms.join("abc", default_bots.run)
    await settle()

    # Assert: the second join shares the bot the first one started
    assert picked_bots.started == ["abc"]
    assert default_bots.started == []
    first.leave()
    second.leave()
