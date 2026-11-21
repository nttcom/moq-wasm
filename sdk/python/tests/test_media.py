import asyncio
import gc
import json
import math
import uuid
import weakref

import pytest

import moq_wasm
from moq_wasm.media import Catalog, CatalogTrack, EventTimeline, RemoteBroadcast, create_broadcast, watch_broadcasts

from tests.fake_native import FakeRelay, FakeWriter, open_session, until

pytestmark = pytest.mark.usefixtures("no_resubscribe_delay")

TIMEOUT_SECONDS = 5
ROOM = ["anon", "room"]
BOT = [*ROOM, "bot"]


async def within(awaitable):
    return await asyncio.wait_for(awaitable, TIMEOUT_SECONDS)


def catalog_payload(*tracks: dict) -> bytes:
    return json.dumps({"version": "draft-02", "tracks": list(tracks)}).encode()


def test_select_picks_the_lowest_bitrate_track_whose_codec_starts_with_the_one_given():
    # Arrange
    catalog = Catalog(
        [
            CatalogTrack("video", role="video", packaging="loc", codec="avc1.640028", bitrate=2_000_000),
            CatalogTrack("audio-hi", role="audio", packaging="loc", codec="opus", bitrate=64_000),
            CatalogTrack("audio-lo", role="audio", packaging="loc", codec="opus", bitrate=32_000),
        ]
    )

    # Act / Assert
    assert catalog.select(role="audio", codec="opus", prefer="lowest_bitrate").name == "audio-lo"
    assert catalog.select(codec="avc1").name == "video"
    assert catalog.select(role="audio", codec="mp4a") is None


def test_parse_reads_the_track_fields_and_refuses_another_version():
    # Act
    catalog = Catalog.parse(catalog_payload({"name": "audio", "packaging": "loc", "isLive": True, "samplerate": 48000}))

    # Assert
    assert catalog.tracks[0] == CatalogTrack("audio", packaging="loc", samplerate=48000)
    with pytest.raises(ValueError):
        Catalog.parse(b'{"version":"draft-03","tracks":[]}')


async def test_a_broadcast_registers_its_tracks_before_it_publishes_and_withdraws_them_on_exit():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)

    # Act
    async with create_broadcast(session, BOT) as bot:
        bot.publish_track("status")
        bot.event_timeline("transcript")
        await bot.published()

    # Assert
    assert relay.sessions[0].calls == [
        ("publish_track", "status"),
        ("publish_track", "transcript"),
        ("announce", BOT),
        ("unannounce", BOT),
        ("end_track", "status"),
        ("end_track", "transcript"),
    ]
    await session.close()


async def test_changing_the_list_a_broadcast_was_given_does_not_change_what_leaving_withdraws():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    namespace = list(BOT)
    bot = create_broadcast(session, namespace)
    await bot.__aenter__()
    bot.publish_track("status")
    await bot.published()

    # Act
    namespace[-1] = "someone-else"
    await bot.__aexit__(None, None, None)

    # Assert
    assert relay.sessions[0].calls[-2] == ("unannounce", BOT)
    assert relay.sessions[0].ended == [(BOT, "status")]
    await session.announce(BOT)
    await session.close()


async def test_changing_the_namespace_a_handle_returns_does_not_change_what_it_withdraws():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    announcement = await session.announce(ROOM)
    writer = await session.publish_track(ROOM, "chat")
    bot = create_broadcast(session, BOT)
    await bot.__aenter__()
    await bot.published()

    # Act
    announcement.namespace.append("elsewhere")
    writer.namespace.append("elsewhere")
    bot.namespace.append("elsewhere")
    await announcement.close()
    await writer.close()
    await bot.__aexit__(None, None, None)

    # Assert
    assert native.calls[-3:] == [("unannounce", ROOM), ("end_track", "chat"), ("unannounce", BOT)]
    assert native.ended == [(ROOM, "chat")]
    assert (announcement.namespace, writer.namespace, bot.namespace) == (ROOM, ROOM, BOT)
    await session.close()


async def test_a_writer_kept_past_its_broadcast_cannot_write_into_the_next_one():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as first:
        old_writer = first.publish_track("status")
        await first.published()
    old_announcement = await session.announce(BOT)

    # Act / Assert
    with pytest.raises(RuntimeError, match="closed"):
        await old_writer.write(b"stale")
    await first._publishing.result().close()
    assert ("unannounce", BOT) not in relay.sessions[0].calls[-1:]
    await old_announcement.close()
    assert relay.sessions[0].calls[-1] == ("unannounce", BOT)
    await session.close()


@pytest.mark.parametrize("add", ["publish_track", "event_timeline", "add_media_source"])
async def test_nothing_can_be_added_to_a_broadcast_once_it_was_left(add):
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        await bot.published()

    # Act / Assert
    with pytest.raises(RuntimeError, match="was left"):
        if add == "add_media_source":
            bot.add_media_source(label="bot")
        else:
            getattr(bot, add)("late")
    assert ("publish_track", "late") not in relay.sessions[0].calls
    await session.close()


async def test_a_broadcast_keeps_no_writer_whose_close_finished():
    # Arrange: a long-lived broadcast replaces a track again and again
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        await bot.published()
        kept = bot.publish_track("kept")

        # Act
        for _ in range(3):
            replaced = bot.publish_track("status")
            await within(replaced.registered())
            await within(replaced.close())

        # Assert
        assert bot._writers == [kept]
    await session.close()


async def test_a_broadcast_keeps_no_writer_whose_late_registration_was_refused():
    # Arrange: the relay refuses every late registration
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        await bot.published()

        async def refuse(namespace, name, **options):
            raise RuntimeError("PUBLISH refused")

        relay.sessions[0].publish_track = refuse

        # Act
        for attempt in range(24):
            late = bot.publish_track(f"late-{attempt}")
            with pytest.raises(RuntimeError, match="PUBLISH refused"):
                await within(late.registered())
        await asyncio.sleep(0)

        # Assert
        assert bot._writers == []
    await session.close()


async def test_a_track_closed_while_its_late_registration_is_refused_closes_and_is_forgotten():
    # Arrange: the relay refuses the late registration once the track's close started
    relay = FakeRelay()
    session = await open_session(relay)
    started = asyncio.Event()
    gate = asyncio.Event()

    async def refuse(namespace, name, **options):
        started.set()
        await gate.wait()
        raise RuntimeError("PUBLISH refused")

    async with create_broadcast(session, BOT) as bot:
        await bot.published()
        relay.sessions[0].publish_track = refuse
        late = bot.publish_track("late")
        await within(started.wait())
        closing = asyncio.create_task(late.close())
        await asyncio.sleep(0)

        # Act
        gate.set()
        await within(closing)

        # Assert
        assert bot._writers == []
    await session.close()


async def test_a_broadcast_not_entered_keeps_no_writer_whose_close_finished():
    # Arrange: a broadcast the app holds without entering it
    relay = FakeRelay()
    session = await open_session(relay)
    broadcast = create_broadcast(session, BOT)

    # Act: a track is declared on it and closed again and again
    for _ in range(3):
        await within(broadcast.publish_track("status").close())

    # Assert
    assert (broadcast._pending, broadcast._writers) == ([], [])
    await session.close()


async def test_a_broadcast_left_at_once_keeps_none_of_the_tracks_it_had_not_registered():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    broadcast = create_broadcast(session, BOT)

    # Act: the block exits before the batch registration started
    async with broadcast as bot:
        status = bot.publish_track("status")

    # Assert
    with pytest.raises(RuntimeError, match="not registered"):
        await within(status.registered())
    assert (broadcast._pending, broadcast._writers) == ([], [])
    await session.close()


async def test_leaving_closes_every_track_even_when_withdrawing_the_namespace_fails():
    # Arrange: the PUBLISH_NAMESPACE_DONE fails
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]

    async def refuse(namespace):
        raise RuntimeError("PUBLISH_NAMESPACE_DONE failed")

    native.unannounce = refuse
    bot = create_broadcast(session, BOT)
    await bot.__aenter__()
    status = bot.publish_track("status")
    bot.publish_track("transcript")
    await bot.published()

    # Act
    with pytest.raises(RuntimeError, match="PUBLISH_NAMESPACE_DONE failed"):
        await bot.__aexit__(None, None, None)

    # Assert
    assert native.calls[-2:] == [("end_track", "status"), ("end_track", "transcript")]
    with pytest.raises(RuntimeError, match="closed"):
        await status.write(b"stale")
    await session.close()


async def test_a_refused_registration_of_the_batch_settles_the_tracks_after_it():
    # Arrange: the relay refuses the second track of the batch
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    refused = RuntimeError("PUBLISH refused")
    registering = native.publish_track

    async def refuse_transcript(namespace, name, **options):
        if name == "transcript":
            raise refused
        return await registering(namespace, name, **options)

    native.publish_track = refuse_transcript
    async with create_broadcast(session, BOT) as bot:
        bot.publish_track("status")
        bot.publish_track("transcript")
        chat = bot.publish_track("chat")

        # Act
        with pytest.raises(RuntimeError, match="PUBLISH refused"):
            await within(bot.published())

        # Assert
        with pytest.raises(RuntimeError, match="PUBLISH refused"):
            await within(chat.registered())
        with pytest.raises(RuntimeError, match="PUBLISH refused"):
            await within(bot.publish_track("late").registered())
    await session.close()


def refuse_announce(relay: FakeRelay) -> None:
    """The relay refuses every PUBLISH_NAMESPACE of the session."""

    async def refuse(namespace: list[str]) -> None:
        raise RuntimeError("PUBLISH_NAMESPACE refused")

    relay.sessions[0].announce = refuse


async def test_a_track_registered_before_a_refused_namespace_raises_the_refusal_from_its_writes():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    refuse_announce(relay)
    async with create_broadcast(session, BOT) as bot:
        status = bot.publish_track("status")
        await within(status.registered())
        await asyncio.sleep(0.01)

        # Act / Assert: the relay takes nothing of a namespace it refused
        with pytest.raises(RuntimeError, match="PUBLISH_NAMESPACE refused"):
            await within(status.write(b"ready", new_group=True))
    assert relay.sessions[0].writers["status"].objects == []
    await session.close()


async def test_leaving_a_refused_broadcast_whose_refusal_was_never_raised_raises_it():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    refuse_announce(relay)

    # Act / Assert: the block exits normally, so leaving it reports the refusal
    with pytest.raises(RuntimeError, match="PUBLISH_NAMESPACE refused"):
        async with create_broadcast(session, BOT) as bot:
            bot.publish_track("status")
            await asyncio.sleep(0.01)
    await session.close()


@pytest.mark.parametrize("request_kind", ["keyframe_request", "demand_change"])
async def test_a_refused_media_announce_settles_the_media_sources_waits(request_kind):
    # Arrange: the relay refuses the media broadcast's PUBLISH_NAMESPACE
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    building = native.media_broadcast

    def refusing_broadcast(namespace):
        broadcast = building(namespace)

        async def refuse() -> None:
            raise RuntimeError("PUBLISH_NAMESPACE refused")

        broadcast.announce = refuse
        return broadcast

    native.media_broadcast = refusing_broadcast
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        waiting = asyncio.ensure_future(getattr(media, request_kind)())

        # Act
        with pytest.raises(RuntimeError, match="PUBLISH_NAMESPACE refused"):
            await within(bot.published())

        # Assert
        with pytest.raises(RuntimeError, match="PUBLISH_NAMESPACE refused"):
            await within(waiting)
    await session.close()


async def test_a_track_added_after_publishing_reports_why_it_was_not_registered():
    # Arrange: the relay refuses the late track's registration
    relay = FakeRelay()
    session = await open_session(relay)
    refused = RuntimeError("PUBLISH refused")

    async with create_broadcast(session, BOT) as bot:
        await bot.published()
        native = relay.sessions[0]

        async def refuse(namespace, name, **options):
            raise refused

        native.publish_track = refuse

        # Act
        late = bot.publish_track("late")

        # Assert
        with pytest.raises(RuntimeError, match="PUBLISH refused"):
            await within(late.registered())
    await session.close()


async def test_writes_to_a_track_whose_late_registration_was_refused_raise_the_refusal():
    # Arrange: the relay refuses the late track's registration
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        await bot.published()

        async def refuse(namespace, name, **options):
            raise RuntimeError("PUBLISH refused")

        relay.sessions[0].publish_track = refuse
        late = bot.publish_track("late")
        with pytest.raises(RuntimeError, match="PUBLISH refused"):
            await within(late.registered())

        # Act / Assert
        with pytest.raises(RuntimeError, match="PUBLISH refused"):
            await within(late.write(b"status", new_group=True))
        with pytest.raises(RuntimeError, match="PUBLISH refused"):
            await within(late.write_group(b"status"))
    await session.close()


async def test_a_track_closed_while_it_is_being_registered_ends_once_registered():
    # Arrange: the late track's registration waits on a gate
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    async with create_broadcast(session, BOT) as bot:
        await bot.published()
        gate = asyncio.Event()
        registering = native.publish_track

        async def gated_publish_track(namespace, name, **options):
            await gate.wait()
            return await registering(namespace, name, **options)

        native.publish_track = gated_publish_track
        late = bot.publish_track("late")
        await asyncio.sleep(0)

        # Act
        closing = asyncio.create_task(late.close())
        await asyncio.sleep(0)
        gate.set()
        await within(closing)

        # Assert: the close waited for the registration under way, then ended the track
        with pytest.raises(RuntimeError, match="closed"):
            await within(late.registered())
        assert native.calls[-2:] == [("publish_track", "late"), ("end_track", "late")]
    await session.close()


async def test_leaving_while_a_late_track_is_being_registered_fails_its_observers_without_cancelling_them():
    # Arrange: the late track's registration waits for the relay, and another task waits for it
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    started = asyncio.Event()
    gate = asyncio.Event()
    registering = native.publish_track

    async def gated_publish_track(namespace, name, **options):
        started.set()
        await gate.wait()
        return await registering(namespace, name, **options)

    async with create_broadcast(session, BOT) as bot:
        await bot.published()
        native.publish_track = gated_publish_track
        late = bot.publish_track("late")
        waiting = asyncio.create_task(late.registered())
        await within(started.wait())

        # Act: leaving the block cancels the registration
    gate.set()

    # Assert: the observers raise that the track was not registered, which `except Exception` catches
    with pytest.raises(RuntimeError, match="not registered"):
        await within(waiting)
    with pytest.raises(RuntimeError, match="not registered"):
        await within(late.write(b"status"))
    await session.close()


async def test_a_track_added_while_the_namespace_is_being_published_is_registered():
    # Arrange: the relay has not answered the PUBLISH_NAMESPACE yet
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.announce_gate = asyncio.Event()
    async with create_broadcast(session, BOT) as bot:
        bot.publish_track("status")
        while ("announce", BOT) not in native.calls:
            await asyncio.sleep(0)

        # Act
        bot.publish_track("late")
        native.announce_gate.set()
        await bot.published()
        await asyncio.sleep(0)

    # Assert
    assert ("publish_track", "late") in native.calls
    await session.close()


async def test_leaving_before_the_relay_answered_the_namespace_withdraws_it_once_answered():
    # Arrange: the relay has not answered the PUBLISH_NAMESPACE yet
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.announce_gate = asyncio.Event()
    bot = create_broadcast(session, BOT)
    await bot.__aenter__()
    while ("announce", BOT) not in native.calls:
        await asyncio.sleep(0)
    leaving = asyncio.create_task(bot.__aexit__(None, None, None))
    await asyncio.sleep(0.01)

    # Act
    native.announce_gate.set()
    await within(leaving)

    # Assert
    assert native.calls[-1] == ("unannounce", BOT)
    await session.close()


async def test_a_namespace_is_published_again_only_once_its_broadcast_finished_leaving():
    # Arrange: leaving the first broadcast waits while it ends its tracks
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    gate = asyncio.Event()
    ending = native.end_track

    async def gated_end_track(namespace, name):
        await gate.wait()
        await ending(namespace, name)

    native.end_track = gated_end_track
    first = create_broadcast(session, BOT)
    await first.__aenter__()
    first.publish_track("status")
    await first.published()
    leaving = asyncio.create_task(first.__aexit__(None, None, None))
    while ("unannounce", BOT) not in native.calls:
        await asyncio.sleep(0)

    # Act / Assert
    with pytest.raises(RuntimeError, match="already publishes"):
        await create_broadcast(session, BOT).__aenter__()
    with pytest.raises(RuntimeError, match="already publishes"):
        await session.announce(BOT)
    gate.set()
    await within(leaving)
    async with create_broadcast(session, BOT) as second:
        await within(second.published())
    await session.close()


async def test_a_cancelled_leave_keeps_the_namespace_until_its_withdrawal_finished():
    # Arrange: leaving the first broadcast waits while it ends its track
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
    first = create_broadcast(session, BOT)
    await first.__aenter__()
    first.publish_track("status")
    await first.published()
    leaving = asyncio.create_task(first.__aexit__(None, None, None))
    await within(ending_started.wait())

    # Act
    leaving.cancel()
    with pytest.raises(asyncio.CancelledError):
        await leaving

    # Assert: the next broadcast waits for the withdrawal, which a later leave joins
    with pytest.raises(RuntimeError, match="already publishes"):
        await create_broadcast(session, BOT).__aenter__()
    gate.set()
    await within(first.__aexit__(None, None, None))
    assert native.ended == [(BOT, "status")]
    async with create_broadcast(session, BOT) as second:
        await within(second.published())
    await session.close()


async def test_a_broadcast_that_was_left_cannot_be_entered_again():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    bot = create_broadcast(session, BOT)
    async with bot:
        bot.publish_track("status")
        await bot.published()

    # Act / Assert
    with pytest.raises(RuntimeError, match="entered before"):
        await bot.__aenter__()
    assert relay.sessions[0].calls.count(("announce", BOT)) == 1
    await session.close()


async def test_a_track_declared_on_a_broadcast_that_could_not_be_entered_stays_free_for_the_session():
    # Arrange: an announcement of the session publishes the namespace
    relay = FakeRelay()
    session = await open_session(relay)
    await session.announce(BOT)
    bot = create_broadcast(session, BOT)
    bot.publish_track("status")
    with pytest.raises(RuntimeError, match="already publishes"):
        await bot.__aenter__()

    # Act
    writer = await within(session.publish_track(BOT, "status"))

    # Assert
    assert relay.sessions[0].calls[-1] == ("publish_track", "status")
    await writer.close()
    await session.close()


async def test_the_session_keeps_no_broadcast_that_could_not_be_entered():
    # Arrange: an announcement of the session publishes the namespace
    relay = FakeRelay()
    session = await open_session(relay)
    await session.announce(BOT)
    refused = []
    for _ in range(3):
        bot = create_broadcast(session, BOT)
        bot.publish_track("status")
        bot.add_media_source(label="bot")
        with pytest.raises(RuntimeError, match="already publishes"):
            await bot.__aenter__()
        refused.append(weakref.ref(bot))
        del bot

    # Act
    gc.collect()

    # Assert
    assert [ref() for ref in refused] == [None] * 3
    await session.close()


async def test_the_handles_of_a_broadcast_that_could_not_be_entered_end_with_the_session():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    await session.announce(BOT)
    bot = create_broadcast(session, BOT)
    status = bot.publish_track("status")
    media = bot.add_media_source(label="bot")
    with pytest.raises(RuntimeError, match="already publishes"):
        await bot.__aenter__()
    del bot
    gc.collect()
    subscriber = asyncio.create_task(status.next_subscriber())
    keyframe = asyncio.create_task(media.keyframe_request())
    await asyncio.sleep(0)

    # Act
    await session.close()

    # Assert
    assert await within(subscriber) is False
    with pytest.raises(RuntimeError, match="stopped publishing"):
        await within(keyframe)
    with pytest.raises(moq_wasm.SessionClosed):
        await within(status.write(b"late"))


async def test_entering_a_broadcast_with_a_track_another_writer_publishes_is_refused_and_frees_its_namespace():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    await session.publish_track(BOT, "status")
    bot = create_broadcast(session, BOT)
    bot.publish_track("status")

    # Act
    with pytest.raises(RuntimeError, match="already publishes track status"):
        await bot.__aenter__()

    # Assert: nothing was published, and the namespace may be published
    assert ("announce", BOT) not in relay.sessions[0].calls
    await within(session.announce(BOT))
    await session.close()


async def test_an_event_timeline_opens_one_group_with_no_record_and_adds_one_object_per_record():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        timeline = bot.event_timeline("verdicts")
        await bot.published()

        # Act
        await timeline.append({"L": [0, 0], "data": "ok"})

    # Assert
    assert relay.sessions[0].writers["verdicts"].objects == [(b"[]", True), (b'[{"L":[0,0],"data":"ok"}]', False)]
    await session.close()


@pytest.mark.parametrize("value", [math.nan, math.inf, -math.inf])
async def test_a_record_json_cannot_carry_raises_value_error_without_being_written(value):
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        timeline = bot.event_timeline("verdicts")
        await bot.published()

        # Act / Assert
        with pytest.raises(ValueError):
            await within(timeline.append({"T": 1, "data": value}))
        await within(timeline.append({"T": 2, "data": "ok"}))

    # Assert
    assert relay.sessions[0].writers["verdicts"].objects == [(b"[]", True), (b'[{"T":2,"data":"ok"}]', False)]
    await session.close()


async def test_a_record_appended_as_its_timeline_is_added_to_a_published_broadcast_follows_the_opening():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        await bot.published()
        timeline = bot.event_timeline("verdicts")

        # Act: the track is registered on its own, after this call started
        await within(timeline.append({"L": [0, 0], "data": "ok"}))

    # Assert
    assert relay.sessions[0].writers["verdicts"].objects == [(b"[]", True), (b'[{"L":[0,0],"data":"ok"}]', False)]
    await session.close()


async def test_an_event_timeline_on_a_registered_writer_opens_before_its_first_record():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    writer = await within(session.publish_track(BOT, "verdicts"))

    # Act
    EventTimeline(writer)
    await within(until(lambda: relay.sessions[0].writers["verdicts"].objects))

    # Assert
    assert relay.sessions[0].writers["verdicts"].objects == [(b"[]", True)]
    await session.close()


async def test_an_event_timeline_whose_opening_fails_on_a_registered_writer_closes_the_writer_with_the_failure():
    # Arrange: the native writer refuses every write
    relay = FakeRelay()
    session = await open_session(relay)
    refused = RuntimeError("write refused")
    relay.sessions[0].writers.setdefault("verdicts", FakeWriter()).failure = refused
    writer = await within(session.publish_track(BOT, "verdicts"))

    # Act
    EventTimeline(writer)
    await within(until(lambda: writer._ended.ended))

    # Assert
    with pytest.raises(RuntimeError, match="write refused"):
        await within(writer.write(b"late"))
    await session.close()


async def test_append_raises_what_failed_the_opening_write():
    # Arrange: the native writer refuses every write
    relay = FakeRelay()
    session = await open_session(relay)
    refused = RuntimeError("write refused")
    async with create_broadcast(session, BOT) as bot:
        await bot.published()
        native = relay.sessions[0]
        writer = native.writers.setdefault("verdicts", FakeWriter())
        writer.failure = refused
        timeline = bot.event_timeline("verdicts")

        # Act / Assert
        with pytest.raises(RuntimeError, match="write refused"):
            await within(timeline.append({"L": [0, 0], "data": "ok"}))
    await session.close()


async def test_a_timeline_left_while_its_opening_write_is_sent_is_closed_with_its_broadcast():
    # Arrange: the timeline's opening write is under way as the broadcast is left
    relay = FakeRelay()
    session = await open_session(relay)
    writer = relay.sessions[0].writers.setdefault("verdicts", FakeWriter())
    writer.send_gate = asyncio.Event()
    bot = create_broadcast(session, BOT)
    await bot.__aenter__()
    await within(bot.published())
    timeline = bot.event_timeline("verdicts")
    while not writer.objects:
        await asyncio.sleep(0.01)
    leaving = asyncio.create_task(bot.__aexit__(None, None, None))
    await asyncio.sleep(0.05)

    # Act
    writer.send_gate.set()
    await within(leaving)

    # Assert: nothing is appended after leaving, and the session may publish the track again
    with pytest.raises(RuntimeError, match="closed"):
        await within(timeline.append({"L": [0, 0], "data": "late"}))
    assert writer.objects == [(b"[]", True)]
    await within(session.publish_track(BOT, "verdicts"))
    await session.close()


async def test_watch_broadcasts_runs_a_handler_per_broadcast_until_it_is_withdrawn():
    # Arrange: the session's own broadcast and one the filter refuses are skipped
    relay = FakeRelay()
    session = await open_session(relay)
    await session.announce(BOT)
    started: list[list[str]] = []
    cancelled: list[list[str]] = []

    async def handler(broadcast: RemoteBroadcast) -> None:
        started.append(broadcast.namespace)
        try:
            await asyncio.Event().wait()
        except asyncio.CancelledError:
            cancelled.append(broadcast.namespace)
            raise

    watching = asyncio.create_task(watch_broadcasts(session, ROOM, handler, filter=lambda namespace: len(namespace) == 3))
    await asyncio.sleep(0)
    native = relay.sessions[0]

    # Act
    for namespace in (BOT, [*ROOM, "bots", "other"], [*ROOM, "alice"]):
        native.publish(namespace)
    await asyncio.sleep(0.01)
    native.publish([*ROOM, "alice"], active=False)
    await asyncio.sleep(0.01)

    # Assert
    assert (started, cancelled) == ([[*ROOM, "alice"]], [[*ROOM, "alice"]])
    watching.cancel()
    await session.close()


async def test_watch_broadcasts_returns_once_the_room_stays_empty():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)

    async def handler(broadcast: RemoteBroadcast) -> None:
        await asyncio.Event().wait()

    # Act / Assert
    await within(watch_broadcasts(session, ROOM, handler, empty_timeout=0.05))
    await session.close()


async def test_catalogs_skips_a_catalog_that_does_not_parse_and_unsubscribes_at_the_end():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    relay.sessions[0].tracks["catalog"] = [[b"not json", catalog_payload({"name": "audio", "packaging": "loc", "isLive": True})]]

    # Act
    catalogs = [catalog async for catalog in RemoteBroadcast(session, [*ROOM, "alice"]).catalogs()]

    # Assert
    assert [track.name for catalog in catalogs for track in catalog.tracks] == ["audio"]
    assert relay.sessions[0].subscriptions[0].closed
    await session.close()


async def test_follow_subscribes_again_after_the_track_ends_and_after_a_refused_subscribe():
    # Arrange: the second SUBSCRIBE is refused once, then a third succeeds
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["chat"] = [[b"one"]]
    following = RemoteBroadcast(session, [*ROOM, "alice"]).follow("chat")

    # Act
    first = await within(anext(following))
    second_attempt = asyncio.create_task(anext(following))
    await asyncio.sleep(0.01)
    native.tracks["chat"] = [[b"two"]]
    second = await within(second_attempt)

    # Assert
    assert (first.payload, second.payload) == (b"one", b"two")
    await following.aclose()
    await session.close()


async def test_follow_sends_a_two_second_delivery_timeout_unless_told_otherwise():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["video"] = [[b"frame"]]
    native.tracks["chat"] = [[b"hello"]]
    broadcast = RemoteBroadcast(session, [*ROOM, "alice"])
    video = broadcast.follow("video")
    chat = broadcast.follow("chat", delivery_timeout=None)

    # Act
    await within(anext(video))
    await within(anext(chat))

    # Assert
    assert [options["delivery_timeout"] for options in native.subscribe_options] == [2.0, None]
    await video.aclose()
    await chat.aclose()
    await session.close()


def records_payload(*records: dict) -> bytes:
    return json.dumps(list(records)).encode()


async def test_event_timeline_yields_each_record_once_across_subscriptions_and_skips_a_malformed_object():
    # Arrange: the second SUBSCRIBE's joining FETCH brings the two objects read before again
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    first, second, third = ({"T": t, "data": t} for t in (1, 2, 3))
    native.tracks["chat"] = [
        [records_payload(first), b"not json"],
        [records_payload(first), b"not json", records_payload(second, third)],
    ]
    timeline = RemoteBroadcast(session, [*ROOM, "alice"]).event_timeline("chat")

    # Act
    records = [await within(anext(timeline)) for _ in range(3)]

    # Assert
    assert records == [first, second, third]
    await timeline.aclose()
    await session.close()


async def test_event_timeline_sends_its_priority_and_no_delivery_timeout_unless_told_otherwise():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.tracks["chat"] = [[records_payload({"T": 1, "data": "hi"})]]
    native.tracks["intent"] = [[records_payload({"T": 1, "data": {"take": True}})]]
    broadcast = RemoteBroadcast(session, [*ROOM, "alice"])
    chat = broadcast.event_timeline("chat")
    intent = broadcast.event_timeline("intent", priority=0, delivery_timeout=0.3)

    # Act
    await within(anext(chat))
    await within(anext(intent))

    # Assert
    assert [(options["priority"], options["delivery_timeout"]) for options in native.subscribe_options] == [
        (None, None),
        (0, 0.3),
    ]
    await chat.aclose()
    await intent.aclose()
    await session.close()


async def test_a_watcher_reads_the_catalog_of_a_broadcast_through_a_relay(relay_url):
    # Arrange
    room = ["anon", f"room-{uuid.uuid4().hex[:8]}"]
    publisher = await moq_wasm.connect(relay_url, verify_certificate=False)
    watcher = await moq_wasm.connect(relay_url, verify_certificate=False)
    first_catalog: asyncio.Future[Catalog] = asyncio.get_running_loop().create_future()

    async def read_catalog(broadcast: RemoteBroadcast) -> None:
        async for catalog in broadcast.catalogs():
            first_catalog.set_result(catalog)
            return

    # Act
    async with create_broadcast(publisher, [*room, "alice"]) as alice:
        catalog = alice.publish_track("catalog")
        await alice.published()
        await catalog.write(catalog_payload({"name": "audio", "packaging": "loc", "isLive": True}), new_group=True)
        watching = asyncio.create_task(watch_broadcasts(watcher, room, read_catalog))
        received = await within(first_catalog)
        watching.cancel()

    # Assert
    assert [track.name for track in received.tracks] == ["audio"]
    for session in (watcher, publisher):
        await session.close()


async def test_leaving_a_broadcast_ends_its_subscribers_through_a_relay(relay_url):
    # Arrange
    room = ["anon", f"room-{uuid.uuid4().hex[:8]}"]
    publisher = await moq_wasm.connect(relay_url, verify_certificate=False)
    subscriber = await moq_wasm.connect(relay_url, verify_certificate=False)
    alice = create_broadcast(publisher, [*room, "alice"])
    await alice.__aenter__()
    status = alice.publish_track("status")
    await alice.published()
    subscription = await within(subscriber.subscribe([*room, "alice"], "status"))
    await status.write(b"online", new_group=True)
    await within(anext(subscription))

    # Act
    await alice.__aexit__(None, None, None)

    # Assert: the subscriber gets PUBLISH_DONE and its iteration ends
    assert await within(subscription.done()) is not None
    assert [track_object async for track_object in subscription] == []
    for session in (subscriber, publisher):
        await session.close()


async def test_next_subscriber_is_false_once_the_session_ends_before_the_track_is_registered():
    # Arrange: the broadcast is never entered, so its track is never registered
    relay = FakeRelay()
    session = await open_session(relay)
    status = create_broadcast(session, BOT).publish_track("status")
    waiting = asyncio.ensure_future(status.next_subscriber())

    # Act
    await session.close()

    # Assert
    assert await within(waiting) is False


async def test_next_subscriber_is_false_for_a_track_its_broadcast_left_unregistered():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)

    # Act: leaving before the first await leaves the batch unregistered
    async with create_broadcast(session, BOT) as bot:
        status = bot.publish_track("status")
    subscribed = await within(status.next_subscriber())

    # Assert
    assert subscribed is False
    assert ("publish_track", "status") not in relay.sessions[0].calls
    await session.close()


async def test_next_subscriber_is_false_once_the_broadcast_of_its_registered_track_was_left():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    bot = create_broadcast(session, BOT)
    await bot.__aenter__()
    status = bot.publish_track("status")
    await bot.published()
    waiting = asyncio.ensure_future(status.next_subscriber())
    await asyncio.sleep(0)

    # Act
    await bot.__aexit__(None, None, None)

    # Assert
    assert await within(waiting) is False
    await session.close()
