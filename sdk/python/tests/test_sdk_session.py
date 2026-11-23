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


async def test_run_waits_for_the_cancelled_work_to_clean_up_before_it_raises():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    cleaned_up = asyncio.Event()

    async def work() -> None:
        try:
            await asyncio.Event().wait()
        finally:
            await asyncio.sleep(0.01)
            cleaned_up.set()

    running = asyncio.create_task(session.run(work()))
    await asyncio.sleep(0)

    # Act
    relay.sessions[0].drop()

    # Assert
    with pytest.raises(ConnectionLost):
        await within(running)
    assert cleaned_up.is_set()


async def test_run_cancelled_while_the_work_cleans_up_after_a_connection_loss_returns_once_it_finished():
    # Arrange: the work's cleanup waits for the gate
    relay = FakeRelay()
    session = await open_session(relay)
    cleaning_up = asyncio.Event()
    gate = asyncio.Event()

    async def work() -> None:
        try:
            await asyncio.Event().wait()
        finally:
            cleaning_up.set()
            await gate.wait()

    running = asyncio.create_task(session.run(work()))
    await asyncio.sleep(0)
    relay.sessions[0].drop()
    await within(cleaning_up.wait())

    # Act
    running.cancel()
    await asyncio.sleep(0.05)
    returned_early = running.done()
    gate.set()

    # Assert
    with pytest.raises(asyncio.CancelledError):
        await within(running)
    assert not returned_early


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


async def test_close_returns_after_the_refresh_callback_it_cancelled_cleaned_up():
    # Arrange: the refresh is in progress when the session is closed
    first = unsigned_token("temp-app", time.time() + 300)
    refreshing, cleaned_up = asyncio.Event(), asyncio.Event()

    async def refresh(token: str) -> str:
        refreshing.set()
        try:
            await asyncio.Event().wait()
        finally:
            await asyncio.sleep(0.05)
            cleaned_up.set()
        return token

    relay = FakeRelay()
    session = await open_session(relay, TokenKeeper(first, refresh), first)
    await within(refreshing.wait())

    # Act
    await within(session.close())

    # Assert
    assert cleaned_up.is_set()


async def test_close_returns_when_the_refresh_callback_it_cancelled_closes_the_session():
    # Arrange: the refresh is due at once and never answers
    first = unsigned_token("temp-app", time.time() + 300)
    refreshing = asyncio.Event()
    session = None

    async def refresh(token: str) -> str:
        refreshing.set()
        try:
            await asyncio.Event().wait()
        finally:
            assert session is not None
            await session.close()
        return token

    relay = FakeRelay()
    session = await open_session(relay, TokenKeeper(first, refresh), first)
    await within(refreshing.wait())

    # Act / Assert
    await within(session.close())


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


@pytest.mark.parametrize("ended_by", ["close", "connection lost"])
async def test_run_after_the_session_ended_raises_without_starting_the_work(ended_by):
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    started = False

    async def work() -> str:
        nonlocal started
        started = True
        return "done"

    if ended_by == "close":
        await session.close()
    else:
        relay.sessions[0].drop()
        with pytest.raises(ConnectionLost):
            await within(session.closed())

    # Act / Assert
    with pytest.raises(SessionClosed if ended_by == "close" else ConnectionLost):
        await within(session.run(work()))
    assert not started


async def test_run_after_the_session_ended_cancels_a_scheduled_task_and_waits_for_its_cleanup():
    # Arrange: a worker already scheduled through `asyncio.gather`, whose cleanup takes a turn
    relay = FakeRelay()
    session = await open_session(relay)
    cleaned_up = asyncio.Event()

    async def worker() -> None:
        try:
            await asyncio.Event().wait()
        finally:
            await asyncio.sleep(0)
            cleaned_up.set()

    work = asyncio.gather(worker())
    await asyncio.sleep(0)
    await session.close()

    # Act
    with pytest.raises(SessionClosed):
        await within(session.run(work))

    # Assert
    assert work.done()
    assert cleaned_up.is_set()


async def test_leaving_async_with_closes_the_connection_and_later_requests_fail():
    # Arrange
    relay = FakeRelay()

    # Act
    async with _Connecting(relay.dial, None, None) as session:
        pass

    # Assert
    await within(relay.sessions[0].closed())
    with pytest.raises(SessionClosed):
        await session.announce(NAMESPACE)


async def test_entering_a_connection_while_it_is_entered_raises_without_connecting():
    # Arrange
    relay = FakeRelay()
    connecting = _Connecting(relay.dial, None, None)

    async with connecting:
        # Act / Assert
        with pytest.raises(RuntimeError, match="already entered"):
            async with connecting:
                pass

        # Assert
        assert len(relay.sessions) == 1


async def test_every_close_waits_until_the_native_session_closed_even_after_one_was_cancelled():
    # Arrange: the native close waits on a gate
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    gate = asyncio.Event()
    closing_natively = native.close

    async def gated_close() -> None:
        await gate.wait()
        await closing_natively()

    native.close = gated_close
    first = asyncio.create_task(session.close())
    await asyncio.sleep(0)
    first.cancel()
    await asyncio.gather(first, return_exceptions=True)

    # Act
    second = asyncio.create_task(session.close())
    await asyncio.sleep(0.01)
    waited = not second.done()
    gate.set()
    await within(second)

    # Assert
    assert waited
    assert native._closed.done()


async def test_the_first_connection_failure_raises():
    # Arrange
    relay = FakeRelay()
    relay.refuse_next = 1

    # Act / Assert
    with pytest.raises(RuntimeError, match="connection refused"):
        await open_session(relay)
