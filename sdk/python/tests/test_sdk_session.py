import asyncio
import gc
import time
from collections.abc import Callable

import pytest

from moq_wasm import ConnectionLost, SessionClosed, TokenExpired
from moq_wasm._session import _Connecting
from moq_wasm import _token
from moq_wasm._token import TokenKeeper

from tests.cancellation import cancel_as_it_resumes
from tests.fake_native import FakeRelay, native_future, open_session, unsigned_token

TIMEOUT_SECONDS = 5
NAMESPACE = ["anon", "room", "bot"]



async def within(awaitable):
    return await asyncio.wait_for(awaitable, TIMEOUT_SECONDS)


async def eventually(condition: Callable[[], bool]) -> None:
    """Waits until `condition()` holds, for at most `TIMEOUT_SECONDS` in all."""

    async def poll() -> None:
        while not condition():
            await asyncio.sleep(0.01)

    await within(poll())


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
    # Arrange: the first token is due for refresh halfway through its 0.2 s
    first = unsigned_token("temp-app", time.time() + 0.2)
    later = unsigned_token("temp-app", time.time() + 3600)

    async def refresh(token: str) -> str:
        return later

    relay = FakeRelay()
    session = await open_session(relay, first, refresh)

    # Act / Assert
    while relay.sessions[0].refreshed != [later]:
        await within(asyncio.sleep(0.01))
    await session.close()


async def test_two_sessions_awaited_from_one_connection_each_refresh_their_own_token():
    # Arrange: both sessions start with a token due for refresh halfway through its 0.4 s
    first = unsigned_token("temp-app", time.time() + 0.4)
    later = unsigned_token("temp-app", time.time() + 3600)

    async def refresh(token: str) -> str:
        return later

    relay = FakeRelay()
    connecting = _Connecting(relay.dial, first, refresh)

    # Act
    sessions = [await connecting, await connecting]
    await eventually(lambda: [native.refreshed for native in relay.sessions] == [[later], [later]])
    await asyncio.sleep(0.4)

    # Assert: neither took the token the other refreshed as its own, so neither expired
    assert [session._done.done() for session in sessions] == [False, False]
    for session in sessions:
        await session.close()


async def test_a_short_lived_token_is_refreshed_halfway_through_its_lifetime():
    # Arrange
    first = unsigned_token("temp-app", time.time() + 0.4)
    later = unsigned_token("temp-app", time.time() + 3600)
    asked_at: list[float] = []

    async def refresh(token: str) -> str:
        asked_at.append(time.time())
        return later

    started_at = time.time()
    relay = FakeRelay()
    session = await open_session(relay, first, refresh)

    # Act
    await eventually(lambda: relay.sessions[0].refreshed == [later])

    # Assert
    assert 0.15 <= asked_at[0] - started_at < 0.35
    await session.close()


async def test_a_failed_refresh_is_retried_until_the_token_actually_expires(monkeypatch):
    # Arrange: the first refresh fails with 0.6 s left; the retry, half the time left later, succeeds
    monkeypatch.setattr(_token, "MIN_RETRY_SECONDS", 0.05)
    first = unsigned_token("temp-app", time.time() + 0.6)
    later = unsigned_token("temp-app", time.time() + 3600)
    answers: list[str | Exception] = [RuntimeError("issuer unavailable"), later]

    async def refresh(token: str) -> str:
        answer = answers.pop(0)
        if isinstance(answer, Exception):
            raise answer
        return answer

    relay = FakeRelay()
    session = await open_session(relay, first, refresh)

    # Act
    await eventually(lambda: relay.sessions[0].refreshed == [later])

    # Assert
    assert not session._done.done()
    await session.close()


async def test_a_refreshed_token_the_relay_refuses_is_refreshed_again_from_the_accepted_one(monkeypatch):
    # Arrange: the relay refuses the first refreshed token
    monkeypatch.setattr(_token, "MIN_RETRY_SECONDS", 0.05)
    first = unsigned_token("temp-app", time.time() + 0.6)
    refused = unsigned_token("temp-app", time.time() + 3600)
    accepted = unsigned_token("temp-app", time.time() + 3601)
    answers = [refused, accepted]
    asked_with: list[str] = []

    async def refresh(token: str) -> str:
        asked_with.append(token)
        return answers.pop(0)

    relay = FakeRelay()
    session = await open_session(relay, first, refresh)
    keeper = session._keeper
    assert keeper is not None
    native = relay.sessions[0]
    handing_over = native.refresh_token

    async def refuse_once(token: str, *, app_id: str) -> None:
        await handing_over(token, app_id=app_id)
        if token == refused:
            raise RuntimeError("the relay refused the token")

    native.refresh_token = refuse_once

    # Act
    await eventually(lambda: keeper.token == accepted)

    # Assert: asked again with the token the relay accepted, before it expired
    assert native.refreshed == [refused, accepted]
    assert asked_with == [first, first]
    assert not session._done.done()
    await session.close()


@pytest.mark.parametrize("refreshed", ["same token", "other app"])
async def test_a_refreshed_token_that_is_not_later_or_of_another_app_is_not_handed_over(monkeypatch, refreshed):
    # Arrange
    monkeypatch.setattr(_token, "MIN_RETRY_SECONDS", 0.05)
    first = unsigned_token("temp-app", time.time() + 0.4)
    other = first if refreshed == "same token" else unsigned_token("other-app", time.time() + 3600)
    calls = 0

    async def refresh(token: str) -> str:
        nonlocal calls
        calls += 1
        return other

    relay = FakeRelay()
    session = await open_session(relay, first, refresh)

    # Act
    with pytest.raises(TokenExpired):
        await within(session.closed())

    # Assert: retried at a bounded rate, never handed over
    assert relay.sessions[0].refreshed == []
    assert 1 <= calls <= 6


async def test_close_returns_after_the_refresh_callback_it_cancelled_cleaned_up():
    # Arrange: the refresh is in progress when the session is closed, well before it times out
    first = unsigned_token("temp-app", time.time() + 0.6)
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
    session = await open_session(relay, first, refresh)
    await within(refreshing.wait())

    # Act
    await within(session.close())

    # Assert
    assert cleaned_up.is_set()


async def test_close_returns_when_the_refresh_callback_it_cancelled_raises_from_its_cleanup(monkeypatch):
    # Arrange: the refresh is due at once, and a failed one would be retried only after 30 s
    monkeypatch.setattr(TokenKeeper, "_due", staticmethod(lambda claims: time.time()))
    first = unsigned_token("temp-app", time.time() + 3600)
    refreshing = asyncio.Event()

    async def refresh(token: str) -> str:
        refreshing.set()
        try:
            await asyncio.Event().wait()
        finally:
            raise RuntimeError("the issuer's client failed while closing")
        return token

    relay = FakeRelay()
    session = await open_session(relay, first, refresh)
    await within(refreshing.wait())

    # Act / Assert
    await within(session.close())


async def test_close_returns_when_the_refresh_callback_it_cancelled_closes_the_session(monkeypatch):
    # Arrange: the refresh is due at once and would time out only after 30 s
    monkeypatch.setattr(TokenKeeper, "_due", staticmethod(lambda claims: time.time()))
    first = unsigned_token("temp-app", time.time() + 3600)
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
    session = await open_session(relay, first, refresh)
    await within(refreshing.wait())

    # Act / Assert
    await within(session.close())


async def test_close_returns_when_the_refresh_callback_it_cancelled_returns_a_token(monkeypatch):
    # Arrange: the refresh is due at once, and a refreshed token's next refresh is an hour away
    monkeypatch.setattr(TokenKeeper, "_due", staticmethod(lambda claims: time.time()))
    first = unsigned_token("temp-app", time.time() + 3600)
    later = unsigned_token("temp-app", time.time() + 7200)
    refreshing = asyncio.Event()

    async def refresh(token: str) -> str:
        refreshing.set()
        try:
            await asyncio.Event().wait()
        except asyncio.CancelledError:
            pass
        monkeypatch.setattr(TokenKeeper, "_due", staticmethod(lambda claims: time.time() + 3600))
        return later

    relay = FakeRelay()
    session = await open_session(relay, first, refresh)
    await within(refreshing.wait())

    # Act
    await within(session.close())

    # Assert
    assert relay.sessions[0].refreshed == []


async def test_a_token_the_refresh_returns_after_catching_its_timeout_is_not_handed_over():
    # Arrange: the refresh outlives the token by catching the timeout at its expiry
    first = unsigned_token("temp-app", time.time() + 0.3)
    later = unsigned_token("temp-app", time.time() + 3600)

    async def refresh(token: str) -> str:
        try:
            await asyncio.Event().wait()
        except asyncio.CancelledError:
            await asyncio.sleep(0.05)
        return later

    relay = FakeRelay()
    session = await open_session(relay, first, refresh)

    # Act / Assert
    with pytest.raises(TokenExpired):
        await within(session.closed())
    assert relay.sessions[0].refreshed == []


async def test_a_refresh_whose_shared_future_someone_else_cancelled_is_retried(monkeypatch):
    # Arrange: the first refresh hands back a future another task shares
    monkeypatch.setattr(_token, "MIN_RETRY_SECONDS", 0.05)
    first = unsigned_token("temp-app", time.time() + 0.6)
    later = unsigned_token("temp-app", time.time() + 3600)
    shared = asyncio.get_running_loop().create_future()
    refreshed = asyncio.get_running_loop().create_future()
    refreshed.set_result(later)
    answers = [shared, refreshed]

    def refresh(token: str) -> asyncio.Future[str]:
        return answers.pop(0)

    relay = FakeRelay()
    session = await open_session(relay, first, refresh)
    await eventually(lambda: len(answers) == 1)

    # Act: its other holder cancels it
    shared.cancel()
    await eventually(lambda: relay.sessions[0].refreshed == [later])

    # Assert: the cancellation failed that refresh only, so the keeper retried it
    assert not session._done.done()
    await session.close()


async def test_the_token_expires_on_time_while_the_refresh_it_gave_up_on_still_cleans_up():
    # Arrange: the refresh never answers, and its cleanup waits until the test lets it end
    first = unsigned_token("temp-app", time.time() + 0.3)
    cleaned_up = asyncio.Event()
    cleanup_may_end = asyncio.Event()

    async def refresh(token: str) -> str:
        try:
            await asyncio.Event().wait()
        finally:
            await cleanup_may_end.wait()
            cleaned_up.set()
        return token

    relay = FakeRelay()
    session = await open_session(relay, first, refresh)

    # Act / Assert: the session ends at the token's expiry, not once the cleanup ended
    with pytest.raises(TokenExpired):
        await asyncio.wait_for(session.closed(), 1)
    cleanup_may_end.set()
    await within(session.close())
    assert cleaned_up.is_set()


async def test_a_handover_the_relay_does_not_answer_before_the_token_expires_ends_the_session():
    # Arrange: the relay never answers the refreshed token
    first = unsigned_token("temp-app", time.time() + 0.3)
    later = unsigned_token("temp-app", time.time() + 3600)

    async def refresh(token: str) -> str:
        return later

    relay = FakeRelay()
    session = await open_session(relay, first, refresh)

    async def unanswered(token: str, *, app_id: str) -> None:
        await asyncio.Event().wait()

    relay.sessions[0].refresh_token = unanswered

    # Act / Assert
    with pytest.raises(TokenExpired):
        await within(session.closed())


async def test_a_refresh_that_returns_after_the_event_loop_stalled_past_the_expiry_is_not_handed_over():
    # Arrange: the refresh blocks the event loop until the token expired, then returns a later one
    first = unsigned_token("temp-app", time.time() + 0.3)
    later = unsigned_token("temp-app", time.time() + 3600)

    async def refresh(token: str) -> str:
        time.sleep(0.3)
        return later

    relay = FakeRelay()
    session = await open_session(relay, first, refresh)

    # Act / Assert
    with pytest.raises(TokenExpired):
        await within(session.closed())
    assert relay.sessions[0].refreshed == []


async def test_a_handover_the_event_loop_stalled_past_the_expiry_before_it_started_is_not_made():
    # Arrange: the event loop stalls past the token's expiry once the handover was scheduled, before
    # it starts
    first = unsigned_token("temp-app", time.time() + 0.3)
    later = unsigned_token("temp-app", time.time() + 3600)

    async def refresh(token: str) -> str:
        return later

    class StallingAtTheHandover(set):
        """The keeper's calls, the refresh first and the handover second."""

        added = 0

        def add(self, call: asyncio.Task) -> None:
            super().add(call)
            self.added += 1
            if self.added == 2:
                time.sleep(0.3)

    relay = FakeRelay()
    session = await open_session(relay, first, refresh)
    assert session._keeper is not None
    session._keeper._calls = StallingAtTheHandover()

    # Act / Assert
    with pytest.raises(TokenExpired):
        await within(session.closed())
    assert relay.sessions[0].refreshed == []


async def test_a_refresh_that_returns_no_str_is_reported_as_a_type_error_naming_what_it_returned(caplog):
    # Arrange: the refresh returns the issuer's answer instead of the token in it
    class IssuedToken:
        token = unsigned_token("temp-app", time.time() + 3600)

    first = unsigned_token("temp-app", time.time() + 0.3)

    async def refresh(token: str) -> str:
        return IssuedToken()  # type: ignore[return-value]

    relay = FakeRelay()
    session = await open_session(relay, first, refresh)

    # Act
    with caplog.at_level("WARNING", logger="moq_wasm"), pytest.raises(TokenExpired):
        await within(session.closed())

    # Assert
    assert "TypeError('refresh_token returned IssuedToken, not a str')" in caplog.text
    assert relay.sessions[0].refreshed == []


async def test_run_cancels_the_work_and_raises_once_the_token_expired():
    # Arrange: every refresh fails until the token expires
    token = unsigned_token("temp-app", time.time() + 0.3)

    async def refresh(token: str) -> str:
        raise RuntimeError("temporary-apps service unavailable")

    relay = FakeRelay()
    session = await open_session(relay, token, refresh)
    work = asyncio.get_running_loop().create_future()

    # Act / Assert
    with pytest.raises(TokenExpired):
        await within(session.run(work))
    assert work.cancelled()


async def test_run_raises_the_expiry_even_when_the_work_returned_because_of_it():
    # Arrange: the work is a watch, which ends with the session
    token = unsigned_token("temp-app", time.time() + 0.3)

    async def refresh(token: str) -> str:
        raise RuntimeError("temporary-apps service unavailable")

    relay = FakeRelay()
    session = await open_session(relay, token, refresh)

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
    async with _Connecting(relay.dial, None) as session:
        pass

    # Assert
    await within(relay.sessions[0].closed())
    with pytest.raises(SessionClosed):
        await session.announce(NAMESPACE)


async def test_entering_a_connection_while_it_is_entered_raises_without_connecting():
    # Arrange
    relay = FakeRelay()
    connecting = _Connecting(relay.dial, None)

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


async def test_a_second_read_of_a_watch_while_one_waits_is_refused_and_the_first_gets_the_event():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    watch = await session.watch_namespaces(NAMESPACE[:-1])
    first = asyncio.ensure_future(anext(watch))
    await asyncio.sleep(0)

    # Act
    with pytest.raises(RuntimeError, match="already"):
        await within(anext(watch))
    relay.sessions[0].publish(NAMESPACE)

    # Assert
    assert (await within(first)).namespace == NAMESPACE
    await session.close()


async def test_a_second_read_of_a_subscription_while_one_waits_is_refused_and_the_first_gets_the_object():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    gate = asyncio.Event()
    relay.sessions[0].tracks["chat"] = [[gate, b"hello"]]
    subscription = await session.subscribe(NAMESPACE, "chat")
    first = asyncio.ensure_future(anext(subscription))
    await asyncio.sleep(0)

    # Act
    with pytest.raises(RuntimeError, match="already"):
        await within(anext(subscription))
    gate.set()

    # Assert
    assert (await within(first)).payload == b"hello"
    await session.close()


async def test_a_subscription_dropped_after_a_timed_out_read_cancels_the_read_it_left():
    # Arrange: a read timed out, leaving its native read waiting
    relay = FakeRelay()
    session = await open_session(relay)
    relay.sessions[0].tracks["chat"] = [[asyncio.Event()]]
    subscription = await session.subscribe(NAMESPACE, "chat")
    with pytest.raises(TimeoutError):
        await asyncio.wait_for(anext(subscription), 0.05)
    left = subscription._objects._pending
    assert left is not None

    # Act
    del subscription
    gc.collect()

    # Assert
    await eventually(left.done)
    assert left.cancelled()
    await session.close()


async def test_closing_a_subscription_sends_the_unsubscribe_at_once_and_a_cancelled_close_leaves_it_going():
    # Arrange: the relay holds the UNSUBSCRIBE back until the gate opens
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["chat"] = [[b"hello"]]
    gate = native.unsubscribe_gates["chat"] = asyncio.Event()
    subscription = await session.subscribe(NAMESPACE, "chat")
    notified: list[asyncio.Future] = []

    # Act: started without being awaited, as `closing = sub.close(); await sub.done()` does
    closing = subscription.close()
    closing.add_done_callback(notified.append)
    await asyncio.sleep(0)
    sent_at_once = native.subscriptions[0].unsubscribing
    closing.cancel()
    closing_again = subscription.close()
    gate.set()
    await within(closing_again)

    # Assert
    assert isinstance(closing, asyncio.Future)
    assert sent_at_once
    assert native.subscriptions[0].closed
    assert notified == [closing]
    await session.close()


async def test_every_close_of_a_watch_waits_for_the_unsubscribe_even_after_one_was_cancelled():
    # Arrange: the UNSUBSCRIBE_NAMESPACE waits on a gate
    relay = FakeRelay()
    session = await open_session(relay)
    watch = await session.watch_namespaces(NAMESPACE[:-1])
    gate, unsubscribed = asyncio.Event(), asyncio.Event()

    async def gated_close() -> None:
        await gate.wait()
        unsubscribed.set()

    watch._events.close = gated_close
    first = asyncio.create_task(watch.close())
    await asyncio.sleep(0)
    first.cancel()
    await asyncio.gather(first, return_exceptions=True)

    # Act
    second = asyncio.create_task(watch.close())
    await asyncio.sleep(0.01)
    waited = not second.done()
    gate.set()
    await within(second)

    # Assert
    assert waited
    assert unsubscribed.is_set()
    await session.close()


async def test_the_first_connection_failure_raises():
    # Arrange
    relay = FakeRelay()
    relay.refuse_next = 1

    # Act / Assert
    with pytest.raises(RuntimeError, match="connection refused"):
        await open_session(relay)


async def test_a_track_publish_cancelled_as_the_relay_accepted_it_ends_before_the_next_writer_registers():
    # Arrange: the registration is a native request the caller awaits
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    registering = native.publish_track
    native.publish_track = lambda namespace, name, **options: native_future(registering(namespace, name, **options))
    publishing = asyncio.create_task(session.publish_track(NAMESPACE, "status", publish=True))
    await within(cancel_as_it_resumes(publishing))
    with pytest.raises(asyncio.CancelledError):
        await publishing

    # Act
    await within(session.publish_track(NAMESPACE, "status", publish=True))

    # Assert
    assert native.calls == [("publish_track", "status"), ("end_track", "status"), ("publish_track", "status")]
    await session.close()
