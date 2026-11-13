import asyncio
import time

import pytest

from moq_wasm import NamespaceEvent, SessionClosed, TokenExpired
from moq_wasm._session import _Connecting
from moq_wasm._token import TokenKeeper

from tests.fake_native import FakeRelay, open_session, restored_on, unsigned_token

TIMEOUT_SECONDS = 5
NAMESPACE = ["anon", "room", "bot"]


pytestmark = pytest.mark.usefixtures("no_reconnect_delay")


async def within(awaitable):
    return await asyncio.wait_for(awaitable, TIMEOUT_SECONDS)


async def test_a_new_connection_registers_the_tracks_before_it_publishes_and_watches():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    await session.publish_track(NAMESPACE, "status", retention_seconds=60)
    await session.announce(NAMESPACE)
    await session.watch_namespaces(["anon", "room"])

    # Act
    relay.sessions[0].drop()
    second = await within(relay.next_session(2))
    await within(restored_on(session, second))

    # Assert
    assert second.calls == [
        ("publish_track", "status"),
        ("announce", NAMESPACE),
        ("watch_namespaces", ["anon", "room"]),
    ]
    await session.close()


async def test_a_watch_reports_its_namespaces_withdrawn_with_the_connection_and_published_again():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    watch = await session.watch_namespaces(["anon", "room"])
    relay.sessions[0].publish(["anon", "room", "alice"])
    assert await within(anext(watch)) == NamespaceEvent(["anon", "room", "alice"], True)

    # Act
    relay.sessions[0].drop()
    second = await within(relay.next_session(2))
    await within(restored_on(session, second))
    second.publish(["anon", "room", "alice"])

    # Assert
    assert [await within(anext(watch)) for _ in range(2)] == [
        NamespaceEvent(["anon", "room", "alice"], False),
        NamespaceEvent(["anon", "room", "alice"], True),
    ]
    await session.close()


async def test_a_track_writes_to_the_new_connection_once_it_is_restored():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    writer = await session.publish_track(NAMESPACE, "status", retention_seconds=60)
    relay.sessions[0].drop()
    second = await within(relay.next_session(2))
    await within(restored_on(session, second))

    # Act
    await writer.write(b"ready", new_group=True)

    # Assert
    assert second.writers["status"].objects == [(b"ready", True)]
    await session.close()


async def test_a_refused_redial_is_retried():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    relay.refuse_next = 2

    # Act
    relay.sessions[0].drop()

    # Assert
    second = await within(relay.next_session(2))
    await within(restored_on(session, second))
    await session.close()


async def test_a_refreshed_token_is_handed_to_the_relay_and_presented_on_the_next_connection():
    # Arrange: the first token is due for refresh at once
    first = unsigned_token("temp-app", time.time() + 300)
    later = unsigned_token("temp-app", time.time() + 3600)

    async def refresh(token: str) -> str:
        return later

    relay = FakeRelay()
    session = await open_session(relay, TokenKeeper(first, refresh), first)

    # Act
    while relay.sessions[0].refreshed != [later]:
        await within(asyncio.sleep(0.01))
    relay.sessions[0].drop()
    second = await within(relay.next_session(2))

    # Assert
    assert (relay.sessions[0].token, second.token) == (first, later)
    await session.close()


async def test_run_cancels_the_work_and_raises_once_the_token_expired():
    # Arrange: every refresh fails and the token expires before the next retry, 30 s later
    token = unsigned_token("temp-app", time.time() + 20)

    async def refresh(token: str) -> str:
        raise RuntimeError("token mint unavailable")

    relay = FakeRelay()
    session = await open_session(relay, TokenKeeper(token, refresh), token)
    work = asyncio.get_running_loop().create_future()

    # Act / Assert
    with pytest.raises(TokenExpired):
        await within(session.run(work))
    assert work.cancelled()


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
