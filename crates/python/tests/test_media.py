import asyncio
import json
import uuid

import pytest

import moq_wasm
from moq_wasm.media import Catalog, CatalogTrack, RemoteBroadcast, create_broadcast, watch_broadcasts

from tests.fake_native import FakeRelay, open_session, restored_on

pytestmark = pytest.mark.usefixtures("no_reconnect_delay")

TIMEOUT_SECONDS = 5
ROOM = ["anon", "room"]
BOT = [*ROOM, "bot"]


async def within(awaitable):
    return await asyncio.wait_for(awaitable, TIMEOUT_SECONDS)


def catalog_payload(*tracks: dict) -> bytes:
    return json.dumps({"version": "draft-01", "tracks": list(tracks)}).encode()


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


async def test_a_broadcast_registers_its_tracks_before_it_publishes_and_withdraws_on_exit():
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


async def test_an_event_timeline_opens_each_connection_with_its_records_when_it_replays():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        timeline = bot.event_timeline("verdicts", replay=True)
        await bot.published()
        await timeline.append({"l": [0, 0], "data": "ok"})

        # Act
        relay.sessions[0].drop()
        second = await within(relay.next_session(2))
        await within(restored_on(session, second))

    # Assert: each connection opens a group with the records so far
    assert relay.sessions[0].writers["verdicts"].objects == [(b"[]", True), (b'[{"l":[0,0],"data":"ok"}]', False)]
    assert second.writers["verdicts"].objects == [(b'[{"l":[0,0],"data":"ok"}]', True)]
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
