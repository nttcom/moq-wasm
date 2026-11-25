import asyncio
import gc
import time
import traceback

import pytest

from moq_wasm import ConnectionLost, SessionClosed, TokenExpired
from moq_wasm._session import _Connecting
from moq_wasm import _token
from moq_wasm._token import TokenKeeper

from tests.cancellation import cancel_as_it_resumes
from tests.fake_native import TIMEOUT_SECONDS, FakeRelay, native_future, open_session, unsigned_token, until, within

NAMESPACE = ["anon", "room", "bot"]


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


async def test_a_refreshed_token_is_handed_to_the_relay(virtual_clock):
    # Arrange: the first token is due for refresh halfway through its 0.2 s
    first = unsigned_token("temp-app", virtual_clock.unix_time() + 0.2)
    later = unsigned_token("temp-app", virtual_clock.unix_time() + 3600)

    async def refresh(token: str) -> str:
        return later

    relay = FakeRelay()
    session = await open_session(relay, first, refresh)

    # Act / Assert
    while relay.sessions[0].refreshed != [later]:
        await within(asyncio.sleep(0.01))
    await session.close()


async def test_two_sessions_awaited_from_one_connection_each_refresh_their_own_token(virtual_clock):
    # Arrange: both sessions start with a token due for refresh halfway through its 0.4 s
    first = unsigned_token("temp-app", virtual_clock.unix_time() + 0.4)
    later = unsigned_token("temp-app", virtual_clock.unix_time() + 3600)

    async def refresh(token: str) -> str:
        return later

    relay = FakeRelay()
    connecting = _Connecting(relay.dial, first, refresh)

    # Act
    sessions = [await connecting, await connecting]
    await until(lambda: [native.refreshed for native in relay.sessions] == [[later], [later]])
    await asyncio.sleep(0.4)

    # Assert: neither took the token the other refreshed as its own, so neither expired
    assert [session._done.done() for session in sessions] == [False, False]
    for session in sessions:
        await session.close()


async def test_a_short_lived_token_is_refreshed_halfway_through_its_lifetime(virtual_clock):
    # Arrange
    first = unsigned_token("temp-app", virtual_clock.unix_time() + 0.4)
    later = unsigned_token("temp-app", virtual_clock.unix_time() + 3600)
    asked_at: list[float] = []

    async def refresh(token: str) -> str:
        asked_at.append(virtual_clock.unix_time())
        return later

    started_at = virtual_clock.unix_time()
    relay = FakeRelay()
    session = await open_session(relay, first, refresh)

    # Act
    await until(lambda: relay.sessions[0].refreshed == [later])

    # Assert
    assert asked_at[0] - started_at == pytest.approx(0.2)
    await session.close()


async def test_a_failed_refresh_is_retried_until_the_token_actually_expires(monkeypatch, virtual_clock):
    # Arrange: the first refresh fails with 0.6 s left; the retry, half the time left later, succeeds
    monkeypatch.setattr(_token, "MIN_RETRY_SECONDS", 0.05)
    first = unsigned_token("temp-app", virtual_clock.unix_time() + 0.6)
    later = unsigned_token("temp-app", virtual_clock.unix_time() + 3600)
    answers: list[str | Exception] = [RuntimeError("issuer unavailable"), later]

    async def refresh(token: str) -> str:
        answer = answers.pop(0)
        if isinstance(answer, Exception):
            raise answer
        return answer

    relay = FakeRelay()
    session = await open_session(relay, first, refresh)

    # Act
    await until(lambda: relay.sessions[0].refreshed == [later])

    # Assert
    assert not session._done.done()
    await session.close()


async def test_a_refreshed_token_the_relay_refuses_is_refreshed_again_from_the_accepted_one(monkeypatch, virtual_clock):
    # Arrange: the relay refuses the first refreshed token
    monkeypatch.setattr(_token, "MIN_RETRY_SECONDS", 0.05)
    first = unsigned_token("temp-app", virtual_clock.unix_time() + 0.6)
    refused = unsigned_token("temp-app", virtual_clock.unix_time() + 3600)
    accepted = unsigned_token("temp-app", virtual_clock.unix_time() + 3601)
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
    await until(lambda: keeper.token == accepted)

    # Assert: asked again with the token the relay accepted, before it expired
    assert native.refreshed == [refused, accepted]
    assert asked_with == [first, first]
    assert not session._done.done()
    await session.close()


@pytest.mark.parametrize("refreshed", ["same token", "other app"])
async def test_a_refreshed_token_that_is_not_later_or_of_another_app_is_not_handed_over(monkeypatch, refreshed, virtual_clock):
    # Arrange
    monkeypatch.setattr(_token, "MIN_RETRY_SECONDS", 0.05)
    first = unsigned_token("temp-app", virtual_clock.unix_time() + 0.4)
    other = first if refreshed == "same token" else unsigned_token("other-app", virtual_clock.unix_time() + 3600)
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


async def test_close_returns_after_the_refresh_callback_it_cancelled_cleaned_up(virtual_clock):
    # Arrange: the refresh is in progress when the session is closed, well before it times out
    first = unsigned_token("temp-app", virtual_clock.unix_time() + 0.6)
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


async def test_a_token_the_refresh_returns_after_catching_its_timeout_is_not_handed_over(virtual_clock):
    # Arrange: the refresh outlives the token by catching the timeout at its expiry
    first = unsigned_token("temp-app", virtual_clock.unix_time() + 0.3)
    later = unsigned_token("temp-app", virtual_clock.unix_time() + 3600)

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


async def test_a_refresh_whose_shared_future_someone_else_cancelled_is_retried(monkeypatch, virtual_clock):
    # Arrange: the first refresh hands back a future another task shares
    monkeypatch.setattr(_token, "MIN_RETRY_SECONDS", 0.05)
    first = unsigned_token("temp-app", virtual_clock.unix_time() + 0.6)
    later = unsigned_token("temp-app", virtual_clock.unix_time() + 3600)
    shared = asyncio.get_running_loop().create_future()
    refreshed = asyncio.get_running_loop().create_future()
    refreshed.set_result(later)
    answers = [shared, refreshed]

    def refresh(token: str) -> asyncio.Future[str]:
        return answers.pop(0)

    relay = FakeRelay()
    session = await open_session(relay, first, refresh)
    await until(lambda: len(answers) == 1)

    # Act: its other holder cancels it
    shared.cancel()
    await until(lambda: relay.sessions[0].refreshed == [later])

    # Assert: the cancellation failed that refresh only, so the keeper retried it
    assert not session._done.done()
    await session.close()


async def test_the_token_expires_on_time_while_the_refresh_it_gave_up_on_still_cleans_up(virtual_clock):
    # Arrange: the refresh never answers, and its cleanup waits until the test lets it end
    first = unsigned_token("temp-app", virtual_clock.unix_time() + 0.3)
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


async def test_a_handover_the_relay_does_not_answer_before_the_token_expires_ends_the_session(virtual_clock):
    # Arrange: the relay never answers the refreshed token
    first = unsigned_token("temp-app", virtual_clock.unix_time() + 0.3)
    later = unsigned_token("temp-app", virtual_clock.unix_time() + 3600)

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


async def test_a_refresh_that_returns_after_the_event_loop_stalled_past_the_expiry_is_not_handed_over(virtual_clock):
    # Arrange: the refresh blocks the event loop until the token expired, then returns a later one
    first = unsigned_token("temp-app", virtual_clock.unix_time() + 0.3)
    later = unsigned_token("temp-app", virtual_clock.unix_time() + 3600)

    async def refresh(token: str) -> str:
        virtual_clock.advance(0.3)
        return later

    relay = FakeRelay()
    session = await open_session(relay, first, refresh)

    # Act / Assert
    with pytest.raises(TokenExpired):
        await within(session.closed())
    assert relay.sessions[0].refreshed == []


async def test_a_handover_the_event_loop_stalled_past_the_expiry_before_it_started_is_not_made(virtual_clock):
    # Arrange: the event loop stalls past the token's expiry once the handover was scheduled, before
    # it starts
    first = unsigned_token("temp-app", virtual_clock.unix_time() + 0.3)
    later = unsigned_token("temp-app", virtual_clock.unix_time() + 3600)

    async def refresh(token: str) -> str:
        return later

    class StallingAtTheHandover(set):
        """The keeper's calls, the refresh first and the handover second."""

        added = 0

        def add(self, call: asyncio.Task) -> None:
            super().add(call)
            self.added += 1
            if self.added == 2:
                virtual_clock.advance(0.3)

    relay = FakeRelay()
    session = await open_session(relay, first, refresh)
    assert session._keeper is not None
    session._keeper._calls = StallingAtTheHandover()

    # Act / Assert
    with pytest.raises(TokenExpired):
        await within(session.closed())
    assert relay.sessions[0].refreshed == []


async def test_a_refresh_that_returns_no_str_is_reported_as_a_type_error_naming_what_it_returned(caplog, virtual_clock):
    # Arrange: the refresh returns the issuer's answer instead of the token in it
    class IssuedToken:
        token = unsigned_token("temp-app", virtual_clock.unix_time() + 3600)

    first = unsigned_token("temp-app", virtual_clock.unix_time() + 0.3)

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


async def test_run_cancels_the_work_and_raises_once_the_token_expired(virtual_clock):
    # Arrange: every refresh fails until the token expires
    token = unsigned_token("temp-app", virtual_clock.unix_time() + 0.3)

    async def refresh(token: str) -> str:
        raise RuntimeError("temporary-apps service unavailable")

    relay = FakeRelay()
    session = await open_session(relay, token, refresh)
    work = asyncio.get_running_loop().create_future()

    # Act / Assert
    with pytest.raises(TokenExpired):
        await within(session.run(work))
    assert work.cancelled()


async def test_run_raises_the_expiry_even_when_the_work_returned_because_of_it(virtual_clock):
    # Arrange: the work is a watch, which ends with the session
    token = unsigned_token("temp-app", virtual_clock.unix_time() + 0.3)

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
    await until(left.done)
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


async def test_changing_the_list_a_namespace_was_given_in_does_not_change_what_close_withdraws():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    namespace = list(NAMESPACE)
    writer = await session.publish_track(namespace, "status")
    announcement = await session.announce(namespace)

    # Act
    namespace[-1] = "someone-else"
    await writer.close()
    await announcement.close()

    # Assert
    assert native.ended == [(NAMESPACE, "status")]
    assert native.calls[-1] == ("unannounce", NAMESPACE)
    await session.close()


async def test_announcing_a_namespace_the_session_publishes_raises_until_it_was_withdrawn():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    announcement = await session.announce(NAMESPACE)

    # Act / Assert
    with pytest.raises(RuntimeError, match="already publishes"):
        await session.announce(NAMESPACE)
    await announcement.close()
    await within(session.announce(NAMESPACE))
    await session.close()


async def test_the_first_connection_failure_raises():
    # Arrange
    relay = FakeRelay()
    relay.refuse_next = 1

    # Act / Assert
    with pytest.raises(RuntimeError, match="connection refused"):
        await open_session(relay)


async def test_every_close_of_a_track_writer_waits_for_its_end_even_after_one_was_cancelled():
    # Arrange: ending the track waits for the relay
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    gate = asyncio.Event()
    ending_started = asyncio.Event()
    ending = native.end_track

    async def gated_end_track(namespace, name):
        ending_started.set()
        await gate.wait()
        await ending(namespace, name)

    native.end_track = gated_end_track
    writer = await session.publish_track(NAMESPACE, "status")
    closing = asyncio.create_task(writer.close())
    await within(ending_started.wait())

    # Act
    closing.cancel()
    with pytest.raises(asyncio.CancelledError):
        await closing
    closing_again = asyncio.create_task(writer.close())
    await asyncio.sleep(0)
    gate.set()
    await within(closing_again)

    # Assert
    assert native.ended == [(NAMESPACE, "status")]
    with pytest.raises(RuntimeError, match="closed"):
        await writer.write(b"late")
    await session.close()


async def test_a_track_has_one_writer_until_it_was_closed():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    first = await session.publish_track(NAMESPACE, "status")

    # Act / Assert: closing the old writer again leaves its replacement's subscriptions alone
    with pytest.raises(RuntimeError, match="already publishes"):
        await session.publish_track(NAMESPACE, "status")
    await first.close()
    second = await session.publish_track(NAMESPACE, "status")
    await first.close()
    assert native.ended == [(NAMESPACE, "status")]
    assert await second.write(b"v1") is None
    await session.close()


async def test_a_refused_publish_track_leaves_nothing_waiting_for_the_session_to_end():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    await session.publish_track(NAMESPACE, "status")
    observers = len(session._enders)

    # Act
    for _ in range(24):
        with pytest.raises(RuntimeError, match="already publishes"):
            await session.publish_track(NAMESPACE, "status")

    # Assert
    assert len(session._enders) == observers
    await session.close()


async def test_publishing_a_track_once_the_session_was_closed_raises_session_closed():
    # Arrange: the writer of the track is still open
    relay = FakeRelay()
    session = await open_session(relay)
    await session.publish_track(NAMESPACE, "status")
    await session.close()

    # Act / Assert
    with pytest.raises(SessionClosed):
        await session.publish_track(NAMESPACE, "status")


async def test_a_write_after_the_session_ended_raises_what_ended_it_without_reaching_the_native_writer():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    writer = await session.publish_track(NAMESPACE, "status")
    native = relay.sessions[0].writers["status"]
    await session.close()

    # Act / Assert: the native origin would cache what it is given
    with pytest.raises(SessionClosed):
        await within(writer.write(b"late"))
    assert native.objects == []


async def test_each_write_to_an_ended_writer_raises_an_error_of_its_own_caused_by_what_ended_it():
    # Arrange: the connection was lost, which ended the writer
    relay = FakeRelay()
    session = await open_session(relay)
    writer = await session.publish_track(NAMESPACE, "status")
    relay.sessions[0].drop(code=0x10, reason="gone")
    with pytest.raises(ConnectionLost) as ended:
        await within(session.closed())
    lost = ended.value
    frames = len(traceback.extract_tb(lost.__traceback__))

    # Act
    raised = []
    for _ in range(3):
        with pytest.raises(ConnectionLost) as info:
            await within(writer.write(b"late"))
        raised.append(info.value)

    # Assert: the lost connection keeps its own traceback, so the writes retain no frames of each other
    assert len({id(error) for error in raised}) == 3
    assert all(error.__cause__ is lost and error.close_info is lost.close_info for error in raised)
    assert all(str(error) == str(lost) for error in raised)
    assert len(traceback.extract_tb(lost.__traceback__)) == frames


async def test_a_write_after_close_began_raises_at_once_instead_of_waiting_behind_a_stalled_send():
    # Arrange: a send stalls, and the writer's close waits for it
    relay = FakeRelay()
    session = await open_session(relay)
    writer = await session.publish_track(NAMESPACE, "status")
    native = relay.sessions[0].writers["status"]
    native.send_gate = asyncio.Event()
    stalled = asyncio.create_task(writer.write(b"stalled"))
    await asyncio.sleep(0)
    closing = asyncio.create_task(writer.close())
    await asyncio.sleep(0)

    # Act / Assert
    with pytest.raises(RuntimeError, match="closed"):
        await asyncio.wait_for(writer.write(b"late"), 0.5)
    native.send_gate.set()
    await within(asyncio.gather(stalled, closing))
    assert native.objects == [(b"stalled", False)]
    await session.close()


@pytest.mark.parametrize(
    ("ended_by", "raised"), [("close", RuntimeError), ("connection lost", ConnectionLost)]
)
async def test_writes_queued_behind_a_stalled_send_raise_once_the_writer_ended(ended_by, raised):
    # Arrange: a send stalls with writes queued behind it
    relay = FakeRelay()
    session = await open_session(relay)
    writer = await session.publish_track(NAMESPACE, "status")
    native = relay.sessions[0].writers["status"]
    native.send_gate = asyncio.Event()
    stalled = asyncio.create_task(writer.write(b"stalled"))
    await until(lambda: native.objects != [])
    queued = [asyncio.create_task(writer.write(payload)) for payload in (b"queued", b"queued group")]
    await asyncio.sleep(0)

    # Act
    if ended_by == "close":
        closing = asyncio.create_task(writer.close())
    else:
        relay.sessions[0].drop()
        closing = asyncio.create_task(session.closed())

    # Assert: the queued writes raise while the send under way may still finish
    for write in queued:
        with pytest.raises(raised):
            await asyncio.wait_for(write, 0.5)
    native.send_gate.set()
    await within(asyncio.gather(stalled, closing, return_exceptions=True))
    assert native.objects == [(b"stalled", False)]
    await session.close()


async def test_the_name_of_a_track_writer_cannot_be_changed_to_redirect_its_close():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    writer = await session.publish_track(NAMESPACE, "status")

    # Act
    with pytest.raises(AttributeError):
        writer.name = "other"  # type: ignore[misc]
    await writer.close()

    # Assert
    assert native.ended == [(NAMESPACE, "status")]
    await session.close()


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


async def test_a_subscriber_that_came_as_its_waiter_was_cancelled_goes_to_the_next_waiter():
    # Arrange: a waiter awaits the native notification
    relay = FakeRelay()
    session = await open_session(relay)
    writer = await session.publish_track(NAMESPACE, "status")
    native_writer = relay.sessions[0].writers["status"]
    waiting = asyncio.create_task(writer.next_subscriber())
    await until(lambda: native_writer.subscriber_waits > 0)
    cancelled = cancel_as_it_resumes(waiting)

    # Act
    native_writer.subscribers.put_nowait(True)
    await within(cancelled)

    # Assert
    with pytest.raises(asyncio.CancelledError):
        await waiting
    assert await within(writer.next_subscriber()) is True
    await session.close()
