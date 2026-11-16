import asyncio
import uuid

import pytest

import moq_wasm
from moq_wasm.codecs import VideoEncoder
from moq_wasm.media import MediaSource, RemoteBroadcast, create_broadcast

from tests.fake_native import FakeRelay, open_session

pytestmark = pytest.mark.usefixtures("no_resubscribe_delay")

TIMEOUT_SECONDS = 5
BOT = ["anon", "room", "bot"]
SIZE = (64, 48)
FRAMERATE = 30
# AAC-LC, 48 kHz, mono (ISO/IEC 14496-3 §1.6.2.1).
AAC_LC_48K_MONO = bytes([0x11, 0x88])


def picture(index: int) -> bytes:
    return bytes([index * 8 % 256]) * (SIZE[0] * SIZE[1] * 3)


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


async def test_a_keyframe_request_of_a_subscriber_resolves_keyframe_request():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        await bot.published()
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


async def test_a_rendition_declared_before_publishing_is_built_with_its_source():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        low = media.add_rendition("low", label="180p", bitrate=200_000)
        low.configure_video("vp8")
        await bot.published()

        # Act
        low.push_video(b"frame", 0)

        # Assert
        native = relay.sessions[0].media_broadcasts[0].sources[0].renditions[0]
        assert native.name == "low"
        assert native.calls == [("configure_video", "vp8"), ("push_video", b"frame")]
    await session.close()


async def test_aac_audio_needs_its_audio_specific_config():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")

        # Act
        with pytest.raises(ValueError, match="AudioSpecificConfig"):
            media.configure_audio("mp4a.40.2", 48000, 1)
        media.configure_audio("mp4a.40.2", 48000, 1, description=AAC_LC_48K_MONO)
        await bot.published()

        # Assert
        assert relay.sessions[0].media_broadcasts[0].sources[0].calls == [
            ("configure_audio", "mp4a.40.2", 48000, 1, AAC_LC_48K_MONO)
        ]
    await session.close()


async def test_a_subscriber_reads_a_rendition_in_another_codec_as_an_alternative_to_the_video(relay_url):
    # Arrange
    namespace = ["anon", f"room-{uuid.uuid4().hex[:8]}", "bot"]
    publisher = await moq_wasm.connect(relay_url, verify_certificate=False)
    subscriber = await moq_wasm.connect(relay_url, verify_certificate=False)
    remote = RemoteBroadcast(subscriber, namespace)
    h264 = VideoEncoder("H264", *SIZE, FRAMERATE)
    vp8 = VideoEncoder("VP8", *SIZE, FRAMERATE)

    async with create_broadcast(publisher, namespace) as bot:
        media = bot.add_media_source(label="bot")
        media.configure_video(h264.codec_string)
        alternative = media.add_rendition("vp8", label="bot vp8")
        alternative.configure_video(vp8.codec_string)
        await bot.published()
        frames = 0

        def push_frame() -> None:
            nonlocal frames
            image, timestamp = picture(frames), frames * 1_000_000 // FRAMERATE
            for packet in h264.encode(image, SIZE, "RGB"):
                media.push_video(packet, timestamp)
            for packet in vp8.encode(image, SIZE, "RGB"):
                alternative.push_video(packet, timestamp)
            frames += 1

        # Act
        push_frame()
        catalogs = remote.catalogs()
        catalog = await within(catalog_listing(catalogs, "video_vp8", push_frame))
        rendition = await within(subscriber.subscribe(namespace, "video_vp8"))
        push_frame()
        received = await within(anext(rendition))

        # Assert: one switching set (draft-ietf-moq-msf-01 altGroup)
        video, listed = catalog.select(codec="avc1"), catalog.select(codec="vp8")
        assert (video.name, listed.name) == ("video", "video_vp8")
        assert video.fields["altGroup"] == listed.fields["altGroup"]
        assert received.payload
        await catalogs.aclose()
    for session in (subscriber, publisher):
        await session.close()


async def test_a_subscriber_gets_aac_audio_listed_with_its_codec(relay_url):
    # Arrange
    namespace = ["anon", f"room-{uuid.uuid4().hex[:8]}", "bot"]
    publisher = await moq_wasm.connect(relay_url, verify_certificate=False)
    subscriber = await moq_wasm.connect(relay_url, verify_certificate=False)
    remote = RemoteBroadcast(subscriber, namespace)

    async with create_broadcast(publisher, namespace) as bot:
        media = bot.add_media_source(label="bot")
        media.configure_audio("mp4a.40.2", 48000, 1, description=AAC_LC_48K_MONO)
        await bot.published()

        # Act
        media.push_audio(b"aac frame 0", 0, level=30, voice_activity=True)
        catalogs = remote.catalogs()
        catalog = await within(anext(catalogs))
        audio = await within(subscriber.subscribe(namespace, catalog.select(role="audio").name))
        media.push_audio(b"aac frame 1", 21_333, level=30, voice_activity=True)

        # Assert
        assert catalog.select(role="audio").codec == "mp4a.40.2"
        assert (await within(anext(audio))).payload == b"aac frame 1"
        await catalogs.aclose()
    for session in (subscriber, publisher):
        await session.close()


async def catalog_listing(catalogs, track: str, push_frame):
    """The first catalog that lists `track`, pushing a frame after each that does not."""
    async for catalog in catalogs:
        if any(listed.name == track for listed in catalog.tracks):
            return catalog
        push_frame()


def test_aac_with_bytes_that_are_no_audio_specific_config_is_refused_at_once():
    # Arrange
    media = MediaSource("video", "audio", label="bot")

    # Act / Assert
    with pytest.raises(ValueError):
        media.configure_audio("mp4a.40.2", 48000, 1, description=b"\xff")


async def test_frames_of_a_rendition_are_dropped_once_the_session_ended(relay_url, caplog):
    # Arrange
    namespace = ["anon", f"room-{uuid.uuid4().hex[:8]}", "bot"]
    publisher = await moq_wasm.connect(relay_url, verify_certificate=False)
    encoder = VideoEncoder("VP8", *SIZE, FRAMERATE)
    async with create_broadcast(publisher, namespace) as bot:
        media = bot.add_media_source(label="bot")
        low = media.add_rendition("low", label="low")
        low.configure_video(encoder.codec_string)
        await bot.published()
        frames = 0

        async def push_until_dropped() -> None:
            nonlocal frames
            while "rendition publishing stopped" not in caplog.text:
                for packet in encoder.encode(picture(frames), SIZE, "RGB"):
                    low.push_video(packet, frames * 1_000_000 // FRAMERATE)
                frames += 1
                await asyncio.sleep(0.05)

        # Act
        await publisher.close()
        await within(push_until_dropped())

        # Assert: the native rendition refuses frames, which the media layer drops with a warning
        assert "frame dropped: rendition publishing stopped" in caplog.text
