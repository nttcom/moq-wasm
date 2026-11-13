import asyncio
import uuid

import pytest

import moq_wasm
from moq_wasm.media import MediaSource, RemoteBroadcast, create_broadcast

from tests.fake_native import FakeRelay, open_session, restored_on

pytestmark = pytest.mark.usefixtures("no_reconnect_delay")

TIMEOUT_SECONDS = 5
BOT = ["anon", "room", "bot"]


async def within(awaitable):
    return await asyncio.wait_for(awaitable, TIMEOUT_SECONDS)


async def test_a_broadcast_with_media_publishes_its_namespace_through_the_media_broadcast():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]

    # Act
    async with create_broadcast(session, BOT) as bot:
        bot.publish_track("status")
        media = bot.add_media_source(label="bot")
        media.configure_audio("opus", 48000, 1)
        await bot.published()
        media.push_audio(b"frame", 0)

    # Assert
    assert native.calls == [
        ("publish_track", "status"),
        ("media_announce", BOT),
        ("unannounce", BOT),
        ("media_close", BOT),
    ]
    assert native.media_broadcasts[0].sources[0].calls == [("configure_audio", "opus", 48000, 1), ("push_audio", b"frame")]
    await session.close()


async def test_a_new_connection_rebuilds_the_source_with_its_configuration_and_asks_for_a_keyframe():
    # Arrange: the first keyframe request is answered by the first build
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        media.configure_video("avc1.42e01f")
        await bot.published()
        await within(media.keyframe_request())
        requested = asyncio.create_task(media.keyframe_request())

        # Act
        relay.sessions[0].drop()
        second = await within(relay.next_session(2))
        await within(restored_on(session, second))

        # Assert
        await within(requested)
        assert second.media_broadcasts[0].sources[0].calls == [("configure_video", "avc1.42e01f")]
    await session.close()


async def test_a_keyframe_request_of_a_subscriber_resolves_keyframe_request():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        await bot.published()
        await within(media.keyframe_request())
        requested = asyncio.create_task(media.keyframe_request())
        await asyncio.sleep(0)

        # Act
        relay.sessions[0].media_broadcasts[0].sources[0].keyframe_requested.set()

        # Assert
        await within(requested)
    await session.close()


async def test_a_media_source_added_after_the_broadcast_is_published_is_refused():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        await bot.published()

        # Act / Assert
        with pytest.raises(RuntimeError, match="before the broadcast is published"):
            bot.add_media_source(label="bot")
    await session.close()


async def test_a_frame_pushed_after_leaving_the_broadcast_is_not_sent():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        media.configure_audio("opus", 48000, 1)
        await bot.published()

    # Act
    media.push_audio(b"late", 0)

    # Assert
    assert relay.sessions[0].media_broadcasts[0].sources[0].calls == [("configure_audio", "opus", 48000, 1)]
    await session.close()


def test_a_codec_the_broadcast_does_not_publish_is_refused_at_once():
    # Arrange
    media = MediaSource("video", "audio", label="bot")

    # Act / Assert
    with pytest.raises(ValueError, match="h264"):
        media.configure_video("h264")
    with pytest.raises(ValueError, match="aac"):
        media.configure_audio("aac", 48000, 1)


async def test_a_frame_before_its_configuration_is_refused():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        await bot.published()

        # Act / Assert
        with pytest.raises(RuntimeError, match="configure_video"):
            media.push_video(b"frame", 0)
    await session.close()


async def test_a_subscriber_gets_the_media_with_its_catalog_and_the_broadcasts_own_tracks(relay_url):
    # Arrange
    namespace = ["anon", f"room-{uuid.uuid4().hex[:8]}", "bot"]
    publisher = await moq_wasm.connect(relay_url, verify_certificate=False)
    subscriber = await moq_wasm.connect(relay_url, verify_certificate=False)
    remote = RemoteBroadcast(subscriber, namespace)

    async with create_broadcast(publisher, namespace) as bot:
        status = bot.publish_track("status")
        media = bot.add_media_source(label="bot")
        media.configure_audio("opus", 48000, 1)
        await bot.published()

        # Act: the catalog lists the audio once its configuration is published
        media.push_audio(b"opus frame 0", 0)
        catalogs = remote.catalogs()
        catalog = await within(anext(catalogs))
        audio = await within(subscriber.subscribe(namespace, catalog.select(role="audio").name))
        statuses = await within(subscriber.subscribe(namespace, "status"))
        media.push_audio(b"opus frame 1", 20_000)
        await status.write(b"ready", new_group=True)

        # Assert
        assert catalog.select(role="audio").codec == "opus"
        assert (await within(anext(audio))).payload == b"opus frame 1"
        assert (await within(anext(statuses))).payload == b"ready"
        await catalogs.aclose()
    for session in (subscriber, publisher):
        await session.close()
