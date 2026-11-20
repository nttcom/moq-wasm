import asyncio
import time

import pytest

from moq_wasm import ConnectionLost, SessionClosed, TokenExpired
from moq_wasm._session import _Connecting
from moq_wasm._token import TokenKeeper

from tests.fake_native import FakeRelay, open_session, unsigned_token

TIMEOUT_SECONDS = 5
NAMESPACE = ["anon", "room", "bot"]



async def within(awaitable):
    return await asyncio.wait_for(awaitable, TIMEOUT_SECONDS)


async def test_run_raises_connection_lost_once_the_native_session_ends():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    work = asyncio.get_running_loop().create_future()
    running = asyncio.create_task(session.run(work))
    await asyncio.sleep(0)

    # Act: the native session gave up replacing its connection
    relay.sessions[0].drop()

    # Assert
    with pytest.raises(ConnectionLost):
        await within(running)
    assert work.cancelled()


async def test_connection_lost_carries_how_the_last_connection_closed():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)

    # Act
    relay.sessions[0].drop(code=0x10, reason="going away")

    # Assert
    with pytest.raises(ConnectionLost) as lost:
        await within(session.closed())
    assert (lost.value.close_info.code, lost.value.close_info.reason) == (0x10, "going away")


async def test_a_track_s_publisher_priority_reaches_the_native_session():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)

    # Act
    await within(session.publish_track(NAMESPACE, "control", priority=0))
    await within(session.publish_track(NAMESPACE, "video"))

    # Assert
    options = relay.sessions[0].publish_options
    assert (options["control"]["priority"], options["video"]["priority"]) == (0, 128)
    await session.close()


async def test_a_refreshed_token_is_handed_to_the_relay():
    # Arrange: the first token is due for refresh at once
    first = unsigned_token("temp-app", time.time() + 300)
    later = unsigned_token("temp-app", time.time() + 3600)

    async def refresh(token: str) -> str:
        return later

    relay = FakeRelay()
    session = await open_session(relay, TokenKeeper(first, refresh), first)

    # Act / Assert
    while relay.sessions[0].refreshed != [later]:
        await within(asyncio.sleep(0.01))
    await session.close()


async def test_run_cancels_the_work_and_raises_once_the_token_expired():
    # Arrange: every refresh fails and the token expires before the next retry, 30 s later
    token = unsigned_token("temp-app", time.time() + 20)

    async def refresh(token: str) -> str:
        raise RuntimeError("temporary-apps service unavailable")

    relay = FakeRelay()
    session = await open_session(relay, TokenKeeper(token, refresh), token)
    work = asyncio.get_running_loop().create_future()

    # Act / Assert
    with pytest.raises(TokenExpired):
        await within(session.run(work))
    assert work.cancelled()


async def test_run_raises_the_expiry_even_when_the_work_returned_because_of_it():
    # Arrange: the work is a watch, which ends with the session
    token = unsigned_token("temp-app", time.time() + 20)

    async def refresh(token: str) -> str:
        raise RuntimeError("temporary-apps service unavailable")

    relay = FakeRelay()
    session = await open_session(relay, TokenKeeper(token, refresh), token)

    async def watch_until_it_ends() -> None:
        async for _ in await session.watch_namespaces(["anon", "room"]):
            pass

    # Act / Assert
    with pytest.raises(TokenExpired):
        await within(session.run(watch_until_it_ends()))


async def test_run_raises_session_closed_after_close():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    running = asyncio.create_task(session.run(asyncio.Event().wait()))
    await asyncio.sleep(0)

    # Act
    await session.close()

    # Assert
    with pytest.raises(SessionClosed):
        await within(running)


async def test_leaving_async_with_closes_the_connection_and_later_requests_fail():
    # Arrange
    relay = FakeRelay()

    # Act
    async with _Connecting(relay.dial, None, None) as session:
        pass

    # Assert
    await within(relay.sessions[0].closed())
    with pytest.raises(RuntimeError, match="the session closed"):
        await session.announce(NAMESPACE)


async def test_the_first_connection_failure_raises():
    # Arrange
    relay = FakeRelay()
    relay.refuse_next = 1

    # Act / Assert
    with pytest.raises(RuntimeError, match="connection refused"):
        await open_session(relay)
