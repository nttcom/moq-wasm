import asyncio
import json
import uuid

import pytest

import moq_wasm
from moq_wasm.media import Catalog, CatalogTrack, RemoteBroadcast, create_broadcast, watch_broadcasts

from tests.fake_native import FakeRelay, open_session

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
