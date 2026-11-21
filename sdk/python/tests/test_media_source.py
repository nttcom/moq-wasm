import asyncio
import signal
import time
import uuid

import pytest

import moq_wasm
from moq_wasm.codecs import VideoDecoder, VideoEncoder
from moq_wasm.media import MediaSource, RemoteBroadcast, create_broadcast

from tests.conftest import started_relay
from tests.fake_native import FakeRelay, called, open_session

pytestmark = pytest.mark.usefixtures("no_resubscribe_delay")

TIMEOUT_SECONDS = 5
BOT = ["anon", "room", "bot"]
SIZE = (64, 48)
FRAMERATE = 30
# AAC-LC, 48 kHz, mono (ISO/IEC 14496-3 §1.6.2.1).
AAC_LC_48K_MONO = bytes([0x11, 0x88])
# The LOC Audio Level property (draft-ietf-moq-loc-04 §2.3.3.2).
AUDIO_LEVEL_ID = 0x0C
# The frames a rendition queues for a slow relay (`rendition_task.rs`).
RENDITION_QUEUE_CAPACITY = 300


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
        ("end_track", "status"),
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
    taken = media.push_audio(b"late", 0)

    # Assert
    assert taken is False
    assert relay.sessions[0].media_broadcasts[0].sources[0].calls == [("configure_audio", "opus", 48000, 1)]
    await session.close()


async def test_a_push_tells_whether_the_native_source_took_it():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    bot = create_broadcast(session, BOT)
    media = bot.add_media_source(label="bot")
    media.configure_audio("opus", 48000, 1)
    low = media.add_rendition("low", label="180p")
    low.configure_video("vp8")

    # Act
    before_entering = (media.push_audio(b"early", 0), low.push_video(b"early", 0))
    async with bot:
        await bot.published()
        published = (media.push_audio(b"frame", 0), low.push_video(b"frame", 0))

    # Assert
    assert before_entering == (False, False)
    assert published == (True, True)
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
        await within(wanted(media, "audio"))
        media.push_audio(b"opus frame 1", 20_000)
        await status.write(b"ready", new_group=True)

        # Assert
        assert catalog.select(role="audio").codec == "opus"
        assert (await within(anext(audio))).payload == b"opus frame 1"
        assert (await within(anext(statuses))).payload == b"ready"
        await catalogs.aclose()
    for session in (subscriber, publisher):
        await session.close()


async def test_leaving_a_media_broadcast_ends_its_media_and_catalog_for_their_subscribers(relay_url):
    # Arrange
    namespace = ["anon", f"room-{uuid.uuid4().hex[:8]}", "bot"]
    publisher = await moq_wasm.connect(relay_url, verify_certificate=False)
    subscriber = await moq_wasm.connect(relay_url, verify_certificate=False)
    bot = create_broadcast(publisher, namespace)
    await bot.__aenter__()
    media = bot.add_media_source(label="bot")
    media.configure_audio("opus", 48000, 1)
    await bot.published()
    media.push_audio(b"opus frame 0", 0)
    catalogs = RemoteBroadcast(subscriber, namespace).catalogs()
    await within(anext(catalogs))
    catalog = await within(subscriber.subscribe(namespace, "catalog"))
    audio = await within(subscriber.subscribe(namespace, "audio"))
    await within(wanted(media, "audio"))
    media.push_audio(b"opus frame 1", 20_000)
    await within(anext(audio))

    # Act
    await bot.__aexit__(None, None, None)

    # Assert: PUBLISH_DONE for each, so a later broadcast of the namespace does not reach them
    assert await within(audio.done()) is not None
    assert await within(catalog.done()) is not None
    await catalogs.aclose()
    for session in (subscriber, publisher):
        await session.close()


async def test_a_media_track_that_sent_audio_is_refused_as_a_datagram_track_on_every_attempt(relay_url):
    # Arrange: a media broadcast whose audio reached a subscriber, then was left
    namespace = ["anon", f"room-{uuid.uuid4().hex[:8]}", "bot"]
    publisher = await moq_wasm.connect(relay_url, verify_certificate=False)
    subscriber = await moq_wasm.connect(relay_url, verify_certificate=False)
    async with create_broadcast(publisher, namespace) as bot:
        media = bot.add_media_source(label="bot")
        media.configure_audio("opus", 48000, 1)
        await bot.published()
        media.push_audio(b"opus frame 0", 0)
        catalogs = RemoteBroadcast(subscriber, namespace).catalogs()
        await within(anext(catalogs))
        audio = await within(subscriber.subscribe(namespace, "audio"))
        await within(wanted(media, "audio"))
        media.push_audio(b"opus frame 1", 20_000)
        await within(anext(audio))
        await catalogs.aclose()

    # Act / Assert: the audio went on subgroup streams, which the track keeps
    for _ in range(2):
        with pytest.raises(ValueError, match="sends its objects on subgroup streams"):
            await within(publisher.publish_track(namespace, "audio", datagram=True))
    for session in (subscriber, publisher):
        await session.close()


async def test_a_media_track_name_never_configured_is_free_for_a_datagram_track_once_left(relay_url):
    # Arrange: an audio-only source whose video name was never configured, then was left
    namespace = ["anon", f"room-{uuid.uuid4().hex[:8]}", "bot"]
    publisher = await moq_wasm.connect(relay_url, verify_certificate=False)
    subscriber = await moq_wasm.connect(relay_url, verify_certificate=False)
    async with create_broadcast(publisher, namespace) as bot:
        media = bot.add_media_source("unused-video", label="bot")
        media.configure_audio("opus", 48000, 1)
        await within(bot.published())

    # Act
    track = await within(publisher.publish_track(namespace, "unused-video", datagram=True))
    await within(publisher.announce(namespace))
    objects = await within(subscriber.subscribe(namespace, "unused-video"))
    await within(track.write(b"d1", new_group=True))
    received = await within(anext(objects))

    # Assert: the native origin never registered the name, so it took the datagram delivery
    assert track.datagram
    assert received.payload == b"d1"
    for session in (subscriber, publisher):
        await session.close()


async def test_a_media_source_is_refused_a_track_the_session_sent_in_datagrams(relay_url):
    # Arrange: a datagram track named as the default audio, opened, then closed
    namespace = ["anon", f"room-{uuid.uuid4().hex[:8]}", "bot"]
    publisher = await moq_wasm.connect(relay_url, verify_certificate=False)
    track = await within(publisher.publish_track(namespace, "audio", datagram=True))
    await within(track.close())
    bot = create_broadcast(publisher, namespace)

    # Act / Assert: the native origin would send the media audio in datagrams too
    with pytest.raises(ValueError, match="track audio of .* sends its objects in datagrams"):
        bot.add_media_source(label="bot")
    await publisher.close()


async def test_entering_a_broadcast_refuses_a_media_track_registered_in_datagrams_since_it_was_declared():
    # Arrange: a media source declared, then its audio registered as a datagram track and closed
    relay = FakeRelay()
    session = await open_session(relay)
    bot = create_broadcast(session, BOT)
    bot.add_media_source("camera", "mic", label="bot")
    track = await within(session.publish_track(BOT, "mic", datagram=True))
    await within(track.close())

    # Act / Assert
    with pytest.raises(ValueError, match="track mic of .* sends its objects in datagrams"):
        await bot.__aenter__()

    # Assert: the refused broadcast left the namespace free
    await within(session.announce(BOT))
    await session.close()


async def test_the_cmaf_audio_of_an_aac_configuration_an_opus_one_replaced_keeps_its_retention_once_left():
    # Arrange: a CMAF source whose AAC audio reached the native source, then was configured as Opus,
    # which has no CMAF sibling, then was left
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot", cmaf=True)
        media.configure_audio("mp4a.40.2", 48000, 1, description=AAC_LC_48K_MONO)
        await within(bot.published())
        media.configure_audio("opus", 48000, 1)

    # Act / Assert: the native origin keeps the retention it registered the AAC's CMAF sibling with
    with pytest.raises(ValueError, match="register it again with retention_seconds=60.0"):
        await within(session.publish_track(BOT, "audio_cmaf", retention_seconds=0))
    await session.close()


async def test_a_media_source_is_refused_a_track_the_session_registered_with_another_retention():
    # Arrange: a track named as a CMAF source's audio sibling, registered keeping its newest object
    # only, then closed
    relay = FakeRelay()
    session = await open_session(relay)
    track = await within(session.publish_track(BOT, "audio_cmaf", retention_seconds=0))
    await within(track.close())
    bot = create_broadcast(session, BOT)

    # Act / Assert: the native origin would keep the media track's newest object only too
    with pytest.raises(ValueError, match="track audio_cmaf of .* keeps its objects for 0 s"):
        bot.add_media_source(label="bot", cmaf=True)
    await session.close()


async def test_a_media_catalog_registered_again_with_another_retention_is_refused(relay_url):
    # Arrange: a media broadcast whose catalog went out, then was left
    namespace = ["anon", f"room-{uuid.uuid4().hex[:8]}", "bot"]
    publisher = await moq_wasm.connect(relay_url, verify_certificate=False)
    async with create_broadcast(publisher, namespace) as bot:
        media = bot.add_media_source(label="bot")
        media.configure_audio("opus", 48000, 1)
        await within(bot.published())

    # Act / Assert: the native origin keeps the newest catalog only, whatever a later writer asks
    with pytest.raises(ValueError, match="register it again with retention_seconds=0.0"):
        await within(publisher.publish_track(namespace, "catalog", retention_seconds=3600))
    await publisher.close()


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


async def test_source_options_the_native_broadcast_would_refuse_raise_before_anything_is_built():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="camera")

        # Act / Assert
        with pytest.raises(ValueError, match="video_bitrate"):
            bot.add_media_source("screen", "screen_audio", label="screen", video_bitrate=-1)
        with pytest.raises(ValueError, match="framerate"):
            media.add_rendition("low", label="180p", framerate=-1)
        with pytest.raises(ValueError, match="channels"):
            media.configure_audio("opus", 48000, 256)
        await bot.published()
        assert [source.video for source in relay.sessions[0].media_broadcasts[0].sources] == ["video"]
    await session.close()


async def test_a_media_broadcast_is_not_entered_while_a_writer_of_the_session_publishes_its_catalog():
    # Arrange: the writer keeps the newest catalog only, as the media broadcast does
    relay = FakeRelay()
    session = await open_session(relay)
    await session.publish_track(BOT, "catalog", retention_seconds=0)
    bot = create_broadcast(session, BOT)
    bot.add_media_source(label="bot")

    # Act
    with pytest.raises(RuntimeError, match="already publishes track catalog"):
        await bot.__aenter__()

    # Assert
    assert relay.sessions[0].media_broadcasts == []
    await session.close()


@pytest.mark.parametrize("name", ["catalog", "video", "audio", "video_timeline", "video_cmaf", "video_low", "video_low_cmaf"])
async def test_no_writer_takes_a_track_of_a_media_broadcast_until_it_was_left(name):
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot", cmaf=True)
        await bot.published()
        media.add_rendition("low", label="180p")

        # Act / Assert
        with pytest.raises(RuntimeError, match=f"already publishes track {name} "):
            await session.publish_track(BOT, name)
        with pytest.raises(RuntimeError, match=f"already publishes track {name} "):
            bot.publish_track(name)
    # The native origin keeps the newest catalog only.
    writer = await within(session.publish_track(BOT, name, retention_seconds=0 if name == "catalog" else 60))
    assert writer.name == name
    await session.close()


@pytest.mark.parametrize(
    ("video", "audio", "implied"),
    [("video", "audio", "chat"), ("camera", "audio", "chat"), ("video", "mic", "chat"), ("video", "mic", "audio")],
)
async def test_no_writer_takes_a_track_the_native_catalog_lists_next_to_a_source(video, audio, implied):
    # Arrange: the native catalog lists the chat next to the default video or audio, and an audio
    # placeholder next to the default video
    relay = FakeRelay()
    session = await open_session(relay)
    declared = create_broadcast(session, BOT)
    declared.publish_track(implied)

    # Act / Assert
    with pytest.raises(RuntimeError, match=f"already publishes track {implied} "):
        declared.add_media_source(video, audio, label="bot")
    async with create_broadcast(session, BOT) as bot:
        bot.add_media_source(video, audio, label="bot")
        with pytest.raises(RuntimeError, match=f"already publishes track {implied} "):
            bot.publish_track(implied)
        await within(bot.published())
        with pytest.raises(RuntimeError, match=f"already publishes track {implied} "):
            await session.publish_track(BOT, implied)
    await session.close()


async def test_sources_share_the_tracks_the_native_catalog_lists_next_to_them():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    bot = create_broadcast(session, BOT)

    # Act: the second source's audio takes the place of the first one's audio placeholder
    bot.add_media_source("video", "mic", label="camera")
    bot.add_media_source("screen", "audio", label="screen")

    # Assert: a source naming a track as the chat is refused
    with pytest.raises(RuntimeError, match="already publishes track chat "):
        bot.add_media_source("chat", "chat_audio", label="chat")
    with pytest.raises(RuntimeError, match="names two of its tracks alike"):
        create_broadcast(session, ["anon", "room", "other"]).add_media_source("video", "chat", label="chat")
    async with bot:
        await within(bot.published())
    for name in ("chat", "audio"):
        assert (await within(session.publish_track(BOT, name))).name == name
    await session.close()


@pytest.mark.parametrize(
    ("first", "second"), [(("video", "mic"), ("audio", "aux")), (("audio", "aux"), ("video", "mic"))]
)
async def test_only_a_sources_audio_takes_the_place_of_the_audio_placeholder(first, second):
    # Arrange: one source's catalog lists an audio placeholder, the other's video is named audio
    relay = FakeRelay()
    session = await open_session(relay)
    bot = create_broadcast(session, BOT)
    bot.add_media_source(*first, label="first")

    # Act / Assert
    with pytest.raises(RuntimeError, match="already publishes track audio "):
        bot.add_media_source(*second, label="second")
    await session.close()


async def test_a_source_naming_its_video_as_its_audio_is_refused_while_an_audio_placeholder_is_listed():
    # Arrange: the first source's catalog lists an audio placeholder
    relay = FakeRelay()
    session = await open_session(relay)
    bot = create_broadcast(session, BOT)
    bot.add_media_source("video", "mic", label="camera")

    # Act / Assert: its video and its default audio are both named audio
    with pytest.raises(RuntimeError, match="names two of its tracks alike"):
        bot.add_media_source("audio", label="second")
    bot.add_media_source("screen", "audio", label="screen")
    await session.close()


async def test_every_track_of_a_native_catalog_is_one_the_broadcast_claimed(relay_url):
    # Arrange: a source with the default video next to a source with other names
    namespace = ["anon", f"room-{uuid.uuid4().hex[:8]}", "bot"]
    publisher = await moq_wasm.connect(relay_url, verify_certificate=False)
    subscriber = await moq_wasm.connect(relay_url, verify_certificate=False)
    remote = RemoteBroadcast(subscriber, namespace)
    encoder = VideoEncoder("VP8", *SIZE, FRAMERATE)

    async with create_broadcast(publisher, namespace) as bot:
        camera = bot.add_media_source("video", "mic", label="camera", cmaf=True)
        camera.configure_video(encoder.codec_string)
        screen = bot.add_media_source("screen", "screen_audio", label="screen")
        screen.configure_audio("opus", 48000, 1)
        await bot.published()
        frames = 0

        def push_frame() -> None:
            nonlocal frames
            for packet in encoder.encode(picture(frames), SIZE, "RGB"):
                camera.push_video(packet, frames * 1_000_000 // FRAMERATE)
            screen.push_audio(b"opus frame", frames * 1_000_000 // FRAMERATE)
            frames += 1

        # Act
        push_frame()
        catalogs = remote.catalogs()
        catalog = await within(catalog_listing(catalogs, {"video", "screen_audio"}, push_frame))
        await catalogs.aclose()

        # Assert: the catalog lists the chat and the audio placeholder too
        listed = {track.name for track in catalog.tracks}
        assert {"chat", "audio"} <= listed
        for name in listed - {"catalog"}:
            with pytest.raises(RuntimeError, match=f"already publishes track {name} "):
                await publisher.publish_track(namespace, name)
    for session in (subscriber, publisher):
        await session.close()


async def test_a_media_broadcast_is_refused_a_track_until_a_cancelled_publish_track_of_it_was_undone():
    # Arrange: the relay registers a raw "audio" track after its caller was cancelled, and holds
    # back the PUBLISH_DONE that undoes it
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    answer, end = asyncio.Event(), asyncio.Event()
    registering, ending = native.publish_track, native.end_track

    async def gated_publish_track(namespace, name, **options):
        await answer.wait()
        return await registering(namespace, name, **options)

    async def gated_end_track(namespace, name):
        await end.wait()
        await ending(namespace, name)

    native.publish_track, native.end_track = gated_publish_track, gated_end_track
    publishing = asyncio.create_task(session.publish_track(BOT, "audio"))
    await asyncio.sleep(0)
    publishing.cancel()
    await asyncio.gather(publishing, return_exceptions=True)
    answer.set()
    await within(called(native, ("publish_track", "audio")))
    bot = create_broadcast(session, BOT)
    bot.add_media_source(label="bot")

    # Act / Assert: that PUBLISH_DONE would end the media broadcast's audio for its subscribers
    with pytest.raises(RuntimeError, match="already publishes track audio"):
        await bot.__aenter__()
    end.set()
    await within(called(native, ("end_track", "audio")))
    async with create_broadcast(session, BOT) as again:
        again.add_media_source(label="bot")
        await within(again.published())
    await session.close()


async def test_a_track_or_rendition_named_as_another_track_of_the_broadcast_is_refused():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    bot = create_broadcast(session, BOT)
    bot.publish_track("screen_timeline")
    media = bot.add_media_source(label="camera")

    # Act / Assert
    with pytest.raises(RuntimeError, match="already publishes track screen_timeline"):
        bot.add_media_source("screen", "screen_audio", label="screen")
    with pytest.raises(RuntimeError, match="already publishes track video_timeline"):
        media.add_rendition("timeline", label="timeline")
    with pytest.raises(RuntimeError, match="already publishes track catalog"):
        bot.publish_track("catalog")
    await session.close()


async def test_the_track_names_of_a_media_source_and_its_renditions_cannot_be_changed_after_their_claim():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    bot = create_broadcast(session, BOT)
    media = bot.add_media_source(label="camera")
    rendition = media.add_rendition("low", label="180p")

    # Act / Assert: renamed, they would publish under a name another track of the broadcast owns
    with pytest.raises(AttributeError):
        media.audio = "messages"  # type: ignore[misc]
    with pytest.raises(AttributeError):
        media.video = "screen"  # type: ignore[misc]
    with pytest.raises(AttributeError):
        rendition.name = "high"  # type: ignore[misc]
    assert (media.video, media.audio, rendition.name) == ("video", "audio", "low")
    await session.close()


async def test_source_options_of_a_type_the_native_broadcast_would_refuse_raise_before_anything_is_built():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="camera")

        # Act / Assert
        with pytest.raises(TypeError, match="video_framerate"):
            bot.add_media_source("screen", "screen_audio", label="screen", video_framerate=29.97)  # type: ignore[arg-type]
        with pytest.raises(TypeError, match="cmaf"):
            bot.add_media_source("screen", "screen_audio", label="screen", cmaf=1)  # type: ignore[arg-type]
        with pytest.raises(TypeError, match="bitrate"):
            media.add_rendition("low", label="180p", bitrate=2e5)  # type: ignore[arg-type]
        with pytest.raises(TypeError, match="sample_rate"):
            media.configure_audio("opus", 48000.0, 1)  # type: ignore[arg-type]
        await bot.published()
        assert [source.video for source in relay.sessions[0].media_broadcasts[0].sources] == ["video"]
        assert relay.sessions[0].media_broadcasts[0].sources[0].renditions == []
    await session.close()


@pytest.mark.parametrize("sample_rate, channels", [(None, 1), (48000, None)])
async def test_an_audio_configuration_without_its_sample_rate_or_channels_is_refused_at_once(sample_rate, channels):
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    bot = create_broadcast(session, BOT)
    status = bot.publish_track("status")
    media = bot.add_media_source(label="bot")

    # Act
    with pytest.raises(TypeError, match="sample_rate|channels"):
        media.configure_audio("opus", sample_rate, channels)  # type: ignore[arg-type]

    # Assert: the broadcast and its other track publish as if it was never given
    async with bot:
        await within(bot.published())
        await within(status.write(b"ok"))
    assert relay.sessions[0].media_broadcasts[0].sources[0].calls == []
    await session.close()


async def test_a_media_broadcast_that_fails_to_build_withdraws_its_namespace_and_closes():
    # Arrange: the native broadcast refuses the second source
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    building = native.media_broadcast

    def media_broadcast(namespace):
        broadcast = building(namespace)
        adding = broadcast.add_source

        def add_source(video, audio, **options):
            if video == "screen":
                raise RuntimeError("the native broadcast refused the source")
            return adding(video, audio, **options)

        broadcast.add_source = add_source
        return broadcast

    native.media_broadcast = media_broadcast
    bot = create_broadcast(session, BOT)
    await bot.__aenter__()
    bot.add_media_source(label="camera")
    bot.add_media_source("screen", "screen_audio", label="screen")

    # Act
    with pytest.raises(RuntimeError, match="refused the source"):
        await within(bot.published())
    await within(called(native, ("unannounce", BOT)))

    # Assert: closing first settles what a source built before published
    assert native.calls[-2:] == [("media_close", BOT), ("unannounce", BOT)]
    await bot.__aexit__(None, None, None)
    await session.close()


async def test_a_media_broadcast_that_fails_to_build_after_a_source_published_leaves_nothing_published(
    draft18_relay_url,
):
    # Arrange: the configured camera publishes on its own once built; the screen then fails
    namespace = ["anon", f"room-{uuid.uuid4().hex[:8]}", "bot"]
    publisher = await moq_wasm.connect(draft18_relay_url, verify_certificate=False)
    watcher = await moq_wasm.connect(draft18_relay_url, verify_certificate=False)
    # Watched before anything is published: the withdrawal goes on the publisher's connection, so
    # a watch sent after leaving may reach the relay before it.
    watch = await within(watcher.watch_namespaces(namespace[:2]))
    bot = create_broadcast(publisher, namespace)
    await bot.__aenter__()
    camera = bot.add_media_source(label="camera")
    camera.configure_video("avc1.42e01f")
    camera.configure_audio("opus", 48000, 1)
    screen = bot.add_media_source("screen", "screen_audio", label="screen")

    def refuse(broadcast, session_ended):
        raise RuntimeError("the native broadcast refused the source")

    screen._build = refuse
    with pytest.raises(RuntimeError, match="refused the source"):
        await within(bot.published())

    # Act
    await within(bot.__aexit__(None, None, None))

    # Assert: the namespace the camera published was withdrawn
    published = False
    while True:
        try:
            event = await asyncio.wait_for(anext(watch), 1)
        except TimeoutError:
            break
        published = event.active
    assert not published
    for session in (watcher, publisher):
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
        await within(wanted(media, "video_vp8"))
        push_frame()
        received = await within(anext(rendition))

        # Assert: one switching set (draft-ietf-moq-msf-02 altGroup)
        video, listed = catalog.select(codec="avc1"), catalog.select(codec="vp8")
        assert (video.name, listed.name) == ("video", "video_vp8")
        assert video.fields["altGroup"] == listed.fields["altGroup"]
        assert received.payload
        await catalogs.aclose()
    for session in (subscriber, publisher):
        await session.close()


async def test_a_subscriber_gets_aac_audio_listed_with_its_codec_and_loc_properties(relay_url):
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
        await within(wanted(media, "audio"))
        media.push_audio(b"aac frame 1", 21_333, level=30, voice_activity=True)

        # Assert: the frame comes with its LOC Timestamp, which the broadcast puts on the wall
        # clock, and its Audio Level
        assert catalog.select(role="audio").codec == "mp4a.40.2"
        received = await within(anext(audio))
        assert received.payload == b"aac frame 1"
        assert abs(received.timestamp_micros - time.time() * 1_000_000) < 60_000_000
        assert (AUDIO_LEVEL_ID, 0x80 | 30) in received.extension_headers
        await catalogs.aclose()
    for session in (subscriber, publisher):
        await session.close()


async def test_demand_changes_a_producer_has_not_read_are_kept_as_each_tracks_latest(relay_url):
    # Arrange: the audio track is published, and nobody reads its demand changes
    namespace = ["anon", f"room-{uuid.uuid4().hex[:8]}", "bot"]
    publisher = await moq_wasm.connect(relay_url, verify_certificate=False)
    subscriber = await moq_wasm.connect(relay_url, verify_certificate=False)
    async with create_broadcast(publisher, namespace) as bot:
        media = bot.add_media_source(label="bot")
        media.configure_audio("opus", 48000, 1)
        await bot.published()
        media.push_audio(b"opus frame", 0)
        await asyncio.sleep(0.2)

        # Act: a subscriber comes and goes, and comes again
        for _ in range(2):
            audio = await within(subscriber.subscribe(namespace, "audio"))
            await audio.close()
            await asyncio.sleep(0.2)
        await within(subscriber.subscribe(namespace, "audio"))
        await asyncio.sleep(0.2)

        # Assert: one unread change, the latest
        first = await within(media.demand_change())
        assert (first.track_name, first.wanted) == ("audio", True)
        with pytest.raises(TimeoutError):
            await asyncio.wait_for(media.demand_change(), 0.3)
    for session in (subscriber, publisher):
        await session.close()


async def wanted(media: MediaSource, track_name: str) -> None:
    """Returns once `media` was told a subscriber wants `track_name`: a source published on demand
    sends nothing of a track before, so a frame pushed earlier never reaches that subscriber."""
    while True:
        demand = await media.demand_change()
        if (demand.track_name, demand.wanted) == (track_name, True):
            return


async def catalog_without(catalogs, tracks: set[str]):
    """The first catalog that lists none of `tracks`."""
    async for catalog in catalogs:
        if not tracks & {listed.name for listed in catalog.tracks}:
            return catalog


async def catalog_listing(catalogs, track: str | set[str], push_frame):
    """The first catalog that lists `track`, or every track of a set, pushing a frame after each
    that does not."""
    tracks = {track} if isinstance(track, str) else track
    async for catalog in catalogs:
        if tracks <= {listed.name for listed in catalog.tracks}:
            return catalog
        push_frame()


def sized_picture(size: tuple[int, int], index: int) -> bytes:
    return bytes([index * 8 % 256]) * (size[0] * size[1] * 3)


async def test_a_rendition_configuration_pushed_while_its_queue_is_full_is_published_later(relay_auth):
    # Arrange: the relay stalls, so the rendition's first configuration waits for PUBLISH_OK and
    # the frames after it fill the rendition's queue
    with started_relay(relay_auth, "relay-stalled") as relay:
        namespace = ["anon", f"room-{uuid.uuid4().hex[:8]}", "bot"]
        publisher = await moq_wasm.connect(relay.url, verify_certificate=False)
        subscriber = await moq_wasm.connect(relay.url, verify_certificate=False)
        async with create_broadcast(publisher, namespace) as bot:
            media = bot.add_media_source(label="bot")
            low = media.add_rendition("low", label="low")
            low.configure_video("avc1.42e01f")
            await bot.published()
            native = low._rendition
            assert native is not None
            frames = 0

            def push(encoder: VideoEncoder, size: tuple[int, int]) -> None:
                nonlocal frames
                for packet in encoder.encode(sized_picture(size, frames), size, "RGB"):
                    native.push_video(packet, frames * 1_000_000 // FRAMERATE)
                frames += 1

            relay.process.send_signal(signal.SIGSTOP)
            try:
                first = VideoEncoder("H264", *SIZE, FRAMERATE)
                push(first, SIZE)
                await asyncio.sleep(0.2)
                for _ in range(RENDITION_QUEUE_CAPACITY + 10):
                    push(first, SIZE)

                # Act: keyframes of alternating sizes, each a new configuration
                sizes = [(32, 24), SIZE] * 3 + [(48, 32)]
                for size in sizes:
                    push(VideoEncoder("H264", *size, FRAMERATE), size)
            finally:
                relay.process.send_signal(signal.SIGCONT)

            # Assert: the latest configuration goes out before a keyframe that fits
            last = VideoEncoder("H264", *sizes[-1], FRAMERATE)
            catalogs = RemoteBroadcast(subscriber, namespace).catalogs()

            async def listed_width() -> int | None:
                async for catalog in catalogs:
                    track = next((track for track in catalog.tracks if track.name == "video_low"), None)
                    if track is not None and track.width == sizes[-1][0]:
                        return track.width
                    last.request_keyframe()
                    push(last, sizes[-1])
                return None

            push(last, sizes[-1])
            assert await asyncio.wait_for(listed_width(), 20) == sizes[-1][0]
            await catalogs.aclose()
        for session in (subscriber, publisher):
            await session.close()


async def test_a_rendition_push_its_full_queue_drops_reports_it_was_not_taken(relay_auth):
    # Arrange: the relay stalls, so the rendition's frames fill its queue
    with started_relay(relay_auth, "relay-stalled") as relay:
        namespace = ["anon", f"room-{uuid.uuid4().hex[:8]}", "bot"]
        publisher = await moq_wasm.connect(relay.url, verify_certificate=False)
        encoder = VideoEncoder("VP8", *SIZE, FRAMERATE)
        async with create_broadcast(publisher, namespace) as bot:
            media = bot.add_media_source(label="bot")
            low = media.add_rendition("low", label="low")
            low.configure_video(encoder.codec_string)
            await bot.published()
            relay.process.send_signal(signal.SIGSTOP)
            try:
                # Act
                taken = [
                    low.push_video(packet, index * 1_000_000 // FRAMERATE)
                    for index in range(RENDITION_QUEUE_CAPACITY + 10)
                    for packet in encoder.encode(picture(index), SIZE, "RGB")
                ]
            finally:
                relay.process.send_signal(signal.SIGCONT)

            # Assert: the first frames were queued, and those the full queue dropped were not taken
            assert taken[0] is True
            assert False in taken
        await publisher.close()


def test_aac_with_bytes_that_are_no_audio_specific_config_is_refused_at_once():
    # Arrange
    media = MediaSource("video", "audio", label="bot")

    # Act / Assert
    with pytest.raises(ValueError):
        media.configure_audio("mp4a.40.2", 48000, 1, description=b"\xff")


async def test_frames_of_a_rendition_are_dropped_once_the_session_ended(relay_url):
    # Arrange
    namespace = ["anon", f"room-{uuid.uuid4().hex[:8]}", "bot"]
    publisher = await moq_wasm.connect(relay_url, verify_certificate=False)
    encoder = VideoEncoder("VP8", *SIZE, FRAMERATE)
    async with create_broadcast(publisher, namespace) as bot:
        media = bot.add_media_source(label="bot")
        low = media.add_rendition("low", label="low")
        low.configure_video(encoder.codec_string)
        await bot.published()

        # Act
        await publisher.close()
        taken = [
            low.push_video(packet, index * 1_000_000 // FRAMERATE)
            for index in range(FRAMERATE)
            for packet in encoder.encode(picture(index), SIZE, "RGB")
        ]

        # Assert: none raises, and none is taken
        assert taken and not any(taken)


async def test_an_h264_access_unit_without_nal_units_is_not_taken(relay_url):
    # Arrange
    namespace = ["anon", f"room-{uuid.uuid4().hex[:8]}", "bot"]
    publisher = await moq_wasm.connect(relay_url, verify_certificate=False)
    encoder = VideoEncoder("H264", *SIZE, FRAMERATE)
    async with create_broadcast(publisher, namespace) as bot:
        media = bot.add_media_source(label="bot")
        media.configure_video(encoder.codec_string)
        low = media.add_rendition("low", label="low")
        low.configure_video(encoder.codec_string)
        await bot.published()
        keyframe = encoder.encode(picture(0), SIZE, "RGB")[0]

        # Act
        empty = (media.push_video(b"\x00\x00\x00\x01", 0), low.push_video(b"\x00\x00\x00\x01", 0))
        pictured = (media.push_video(keyframe, 0), low.push_video(keyframe, 0))

        # Assert
        assert empty == (False, False)
        assert pictured == (True, True)
    await publisher.close()


def annex_b_units(access_unit: bytes) -> list[bytes]:
    """The NAL units of an Annex B access unit, without their start codes."""
    return [unit.rstrip(b"\x00") for unit in access_unit.split(b"\x00\x00\x01") if unit.rstrip(b"\x00")]


def annex_b(*units: bytes) -> bytes:
    return b"".join(b"\x00\x00\x00\x01" + unit for unit in units)


async def test_an_h264_access_unit_without_a_slice_is_not_taken_and_its_parameter_sets_reach_the_next_keyframe(
    relay_url,
):
    # Arrange: a keyframe split into an access unit of its parameter sets and one of its slice
    namespace = ["anon", f"room-{uuid.uuid4().hex[:8]}", "bot"]
    publisher = await moq_wasm.connect(relay_url, verify_certificate=False)
    subscriber = await moq_wasm.connect(relay_url, verify_certificate=False)
    encoder = VideoEncoder("H264", *SIZE, FRAMERATE)
    units = annex_b_units(b"".join(encoder.encode(picture(0), SIZE, "RGB")))
    parameter_sets = annex_b(*(unit for unit in units if unit[0] & 0x1F in (7, 8)))
    slice_only = annex_b(*(unit for unit in units if unit[0] & 0x1F not in (7, 8)))
    delimiter = annex_b(b"\x09\xf0")
    async with create_broadcast(publisher, namespace) as bot:
        media = bot.add_media_source(label="bot")
        media.configure_video(encoder.codec_string)
        await bot.published()

        # Act: the slice after the parameter sets configures the track, which is listed and
        # subscribed to before the next slice comes
        taken = [media.push_video(unit, 0) for unit in (delimiter, parameter_sets, slice_only)]
        catalogs = RemoteBroadcast(subscriber, namespace).catalogs()
        await within(catalog_listing(catalogs, "video", lambda: None))
        await catalogs.aclose()
        subscription = await within(subscriber.subscribe(namespace, "video"))
        await within(wanted(media, "video"))
        taken.append(media.push_video(slice_only, 1_000_000 // FRAMERATE))
        received = await within(anext(subscription))

    # Assert: neither unit without a slice is published, and the keyframe carries the parameter
    # sets the earlier unit brought
    assert taken == [False, False, True, True]
    decoded = VideoDecoder(encoder.codec_string).decode(received.payload)
    assert [(picture.width, picture.height) for picture, _ in decoded] == [SIZE]
    await subscriber.close()
    await publisher.close()


async def test_a_source_whose_native_publishing_failed_raises_from_its_pushes_and_requests():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        media.configure_video("avc1.42e01f")
        low = media.add_rendition("low", label="180p")
        low.configure_video("vp8")
        await bot.published()
        native = relay.sessions[0].media_broadcasts[0].sources[0]
        waiting = asyncio.create_task(media.keyframe_request())
        await asyncio.sleep(0)
        native.renditions[0].failure = RuntimeError("rendition publishing stopped")
        native.failure = RuntimeError("media publishing stopped")

        # Act / Assert
        with pytest.raises(RuntimeError, match="rendition publishing stopped"):
            low.push_video(b"frame", 0)
        with pytest.raises(RuntimeError, match="media publishing stopped"):
            media.push_video(b"frame", 0)
        with pytest.raises(RuntimeError, match="media publishing stopped"):
            await within(waiting)
        with pytest.raises(RuntimeError, match="media publishing stopped"):
            await within(media.keyframe_request())
        with pytest.raises(RuntimeError, match="stopped publishing"):
            media.push_video(b"frame", 1)
        with pytest.raises(RuntimeError, match="stopped publishing"):
            low.push_video(b"frame", 1)
    await session.close()


async def test_once_a_push_failed_a_new_rendition_and_the_configurations_raise():
    # Arrange: a push failed while the session is live
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        media.configure_video("avc1.42e01f")
        low = media.add_rendition("low", label="180p")
        await bot.published()
        relay.sessions[0].media_broadcasts[0].sources[0].failure = RuntimeError("media publishing stopped")
        with pytest.raises(RuntimeError, match="media publishing stopped"):
            media.push_video(b"frame", 0)

        # Act / Assert
        with pytest.raises(RuntimeError, match="stopped publishing"):
            media.add_rendition("high", label="720p")
        with pytest.raises(RuntimeError, match="stopped publishing"):
            low.configure_video("vp8")
        with pytest.raises(RuntimeError, match="stopped publishing"):
            media.configure_video("avc1.42e01f")
        with pytest.raises(RuntimeError, match="stopped publishing"):
            media.configure_audio("opus", 48000, 1)
    await session.close()


async def test_once_a_push_failed_remove_track_raises_the_failure_instead_of_doing_nothing():
    # Arrange: a push failed while the session is live, which detached the native source
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        media.configure_audio("opus", 48000, 1)
        await bot.published()
        native = relay.sessions[0].media_broadcasts[0].sources[0]
        native.failure = RuntimeError("media publishing stopped")
        with pytest.raises(RuntimeError, match="media publishing stopped"):
            media.push_audio(b"frame", 0)

        # Act / Assert
        with pytest.raises(RuntimeError, match="stopped publishing"):
            await within(media.remove_track("audio"))
        assert ("remove_track", "audio") not in native.calls
    await session.close()


async def test_a_configuration_the_native_source_refuses_in_a_live_session_stops_the_source():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        await bot.published()
        relay.sessions[0].media_broadcasts[0].sources[0].failure = RuntimeError("media publishing stopped")

        # Act
        with pytest.raises(RuntimeError, match="media publishing stopped"):
            media.configure_audio("opus", 48000, 1)

        # Assert
        with pytest.raises(RuntimeError, match="media publishing stopped"):
            await within(media.keyframe_request())
        with pytest.raises(RuntimeError, match="stopped publishing"):
            media.configure_video("avc1.42e01f")
    await session.close()


async def test_a_frame_the_native_source_refuses_once_the_session_ended_is_dropped():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        media.configure_video("avc1.42e01f")
        await bot.published()
        relay.sessions[0].media_broadcasts[0].sources[0].failure = RuntimeError("media publishing stopped")

        # Act
        await session.close()

        # Assert
        assert media.push_video(b"frame", 0) is False


async def test_a_frame_pushed_once_the_session_ended_is_dropped_before_the_native_source_takes_it():
    # Arrange: the native source and rendition would still take frames
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        media.configure_audio("opus", 48000, 1)
        low = media.add_rendition("low", label="180p")
        low.configure_video("vp8")
        await bot.published()
        native = relay.sessions[0].media_broadcasts[0].sources[0]

        # Act
        await session.close()

        # Assert
        assert (media.push_audio(b"late", 0), low.push_video(b"late", 0)) == (False, False)
        assert ("push_audio", b"late") not in native.calls
        assert native.renditions[0].calls == [("configure_video", "vp8")]


async def test_demand_changes_reach_a_source_published_on_demand():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot", cmaf=True)
        await bot.published()
        native = relay.sessions[0].media_broadcasts[0].sources[0]

        # Act
        native.demands.put_nowait("audio wanted")
        demand = await within(media.demand_change())

        # Assert
        assert demand == "audio wanted"
        assert (native.options["cmaf"], native.options["on_demand"]) == (True, True)
    await session.close()


async def test_a_cancelled_demand_change_leaves_the_next_change_to_the_next_caller():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        await bot.published()
        native = relay.sessions[0].media_broadcasts[0].sources[0]
        abandoned = asyncio.create_task(media.demand_change())
        await asyncio.sleep(0.01)
        abandoned.cancel()
        await asyncio.gather(abandoned, return_exceptions=True)

        # Act
        native.demands.put_nowait("audio wanted")
        demand = await within(media.demand_change())

        # Assert
        assert demand == "audio wanted"
    await session.close()


@pytest.mark.parametrize("request_kind", ["demand_change", "keyframe_request"])
async def test_an_answer_that_came_as_its_caller_was_cancelled_goes_to_the_next_caller(request_kind):
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        await bot.published()
        native = relay.sessions[0].media_broadcasts[0].sources[0]
        abandoned = asyncio.create_task(getattr(media, request_kind)())
        await asyncio.sleep(0.01)

        # Act: the answer and the cancellation come in the same turn
        if request_kind == "demand_change":
            native.demands.put_nowait("audio wanted")
        else:
            native.keyframe_requested.set()
        abandoned.cancel()
        await asyncio.gather(abandoned, return_exceptions=True)
        answer = await within(getattr(media, request_kind)())

        # Assert
        assert answer == ("audio wanted" if request_kind == "demand_change" else None)
    await session.close()


@pytest.mark.parametrize("request_kind", ["demand_change", "keyframe_request"])
async def test_a_second_caller_while_one_waits_is_refused_and_the_first_gets_the_answer(request_kind):
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        await bot.published()
        native = relay.sessions[0].media_broadcasts[0].sources[0]
        first = asyncio.ensure_future(getattr(media, request_kind)())
        await asyncio.sleep(0.01)

        # Act
        with pytest.raises(RuntimeError, match="already"):
            await within(getattr(media, request_kind)())
        if request_kind == "demand_change":
            native.demands.put_nowait("audio wanted")
        else:
            native.keyframe_requested.set()

        # Assert
        assert await within(first) == ("audio wanted" if request_kind == "demand_change" else None)
    await session.close()


async def test_a_source_is_published_on_demand_unless_it_opts_out():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)

    # Act
    async with create_broadcast(session, BOT) as bot:
        bot.add_media_source("video", "audio", label="camera")
        bot.add_media_source("cache", "cache_audio", label="cached", on_demand=False)
        await bot.published()

    # Assert
    sources = relay.sessions[0].media_broadcasts[0].sources
    assert [source.options["on_demand"] for source in sources] == [True, False]
    await session.close()


async def test_demand_change_of_a_source_not_published_on_demand_is_refused():
    # Arrange
    media = MediaSource("video", "audio", label="bot", on_demand=False)

    # Act / Assert
    with pytest.raises(RuntimeError, match="on_demand"):
        await media.demand_change()


async def test_a_cmaf_source_sends_the_video_as_cmaf_to_its_subscriber(relay_url):
    # Arrange
    namespace = ["anon", f"room-{uuid.uuid4().hex[:8]}", "bot"]
    publisher = await moq_wasm.connect(relay_url, verify_certificate=False)
    subscriber = await moq_wasm.connect(relay_url, verify_certificate=False)
    remote = RemoteBroadcast(subscriber, namespace)
    encoder = VideoEncoder("H264", *SIZE, FRAMERATE)

    async with create_broadcast(publisher, namespace) as bot:
        media = bot.add_media_source(label="bot", cmaf=True)
        media.configure_video(encoder.codec_string)
        await bot.published()
        frames = 0

        def push_frame() -> None:
            nonlocal frames
            for packet in encoder.encode(picture(frames), SIZE, "RGB"):
                media.push_video(packet, frames * 1_000_000 // FRAMERATE)
            frames += 1

        # Act
        push_frame()
        catalogs = remote.catalogs()
        catalog = await within(catalog_listing(catalogs, "video_cmaf", push_frame))
        cmaf = await within(subscriber.subscribe(namespace, "video_cmaf"))
        await within(wanted(media, "video_cmaf"))
        encoder.request_keyframe()
        push_frame()
        received = await within(anext(cmaf))

        # Assert
        assert catalog.select(packaging="cmaf").name == "video_cmaf"
        assert received.payload
        await catalogs.aclose()
    for session in (subscriber, publisher):
        await session.close()


async def test_a_source_published_on_demand_reports_a_subscriber_coming_and_going(relay_url):
    # Arrange
    namespace = ["anon", f"room-{uuid.uuid4().hex[:8]}", "bot"]
    publisher = await moq_wasm.connect(relay_url, verify_certificate=False)
    subscriber = await moq_wasm.connect(relay_url, verify_certificate=False)

    async with create_broadcast(publisher, namespace) as bot:
        media = bot.add_media_source(label="bot")
        media.configure_audio("opus", 48000, 1)
        await bot.published()
        media.push_audio(b"opus frame 0", 0)
        published = await within(media.demand_change())

        # Act
        audio = await within(subscriber.subscribe(namespace, "audio"))
        taken = await within(media.demand_change())
        await within(audio.close())
        left = await within(media.demand_change())

        # Assert
        assert [(demand.track_name, demand.wanted) for demand in (published, taken, left)] == [
            ("audio", False),
            ("audio", True),
            ("audio", False),
        ]
    for session in (subscriber, publisher):
        await session.close()


async def test_demand_change_raises_once_the_session_ends():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        await bot.published()
        waiting = asyncio.ensure_future(media.demand_change())
        await asyncio.sleep(0)

        # Act
        relay.sessions[0].drop()

        # Assert
        with pytest.raises(RuntimeError, match="stopped publishing"):
            await within(waiting)


@pytest.mark.parametrize("request_kind", ["keyframe_request", "demand_change"])
async def test_a_request_waiting_as_the_broadcast_is_left_raises_that_the_source_stopped(request_kind):
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        await bot.published()
        waiting = asyncio.ensure_future(getattr(media, request_kind)())
        await asyncio.sleep(0.01)

    # Act / Assert: leaving the block stopped the source
    with pytest.raises(RuntimeError, match="stopped publishing"):
        await within(waiting)
    await session.close()


async def test_media_pushed_while_the_namespace_is_being_published_is_dropped_and_its_configuration_is_built():
    # Arrange: the relay has not answered the media broadcast's namespace yet
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.media_announce_gate = asyncio.Event()
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        media.configure_audio("opus", 48000, 1)
        low = media.add_rendition("low", label="180p")
        low.configure_video("vp8")
        await within(called(native, ("media_announce", BOT)))
        media.configure_video("vp8")

        # Act
        pending = (media.push_audio(b"early", 0), media.push_video(b"early", 0), low.push_video(b"early", 0))
        native.media_announce_gate.set()
        await within(bot.published())
        published = (media.push_audio(b"frame", 0), low.push_video(b"frame", 0))

        # Assert
        source = native.media_broadcasts[0].sources[0]
        assert pending == (False, False, False)
        assert published == (True, True)
        assert source.calls == [
            ("configure_audio", "opus", 48000, 1),
            ("configure_video", "vp8"),
            ("push_audio", b"frame"),
        ]
        assert source.renditions[0].calls == [("configure_video", "vp8"), ("push_video", b"frame")]
    await session.close()


async def test_a_refused_media_announce_fails_the_pushes_the_renditions_and_the_bound_writers():
    # Arrange: a raw track is registered before the relay refuses the media broadcast's namespace
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    native.media_announce_gate = asyncio.Event()
    native.media_announce_failure = RuntimeError("PUBLISH_NAMESPACE refused")
    async with create_broadcast(session, BOT) as bot:
        status = bot.publish_track("status")
        media = bot.add_media_source(label="bot")
        media.configure_audio("opus", 48000, 1)
        low = media.add_rendition("low", label="180p")
        low.configure_video("vp8")
        await within(status.registered())
        waiting = asyncio.ensure_future(status.next_subscriber())

        # Act
        native.media_announce_gate.set()
        with pytest.raises(RuntimeError, match="PUBLISH_NAMESPACE refused"):
            await within(bot.published())

        # Assert
        with pytest.raises(RuntimeError, match="stopped publishing") as pushed:
            media.push_audio(b"frame", 0)
        assert str(pushed.value.__cause__) == "PUBLISH_NAMESPACE refused"
        with pytest.raises(RuntimeError, match="stopped publishing"):
            low.push_video(b"frame", 0)
        with pytest.raises(RuntimeError, match="stopped publishing"):
            await within(media.remove_track("audio"))
        assert await within(waiting) is False
    await session.close()


async def test_frames_pushed_once_the_session_ended_before_the_broadcast_was_published_are_dropped():
    # Arrange: the session ends before the media broadcast's namespace is published
    relay = FakeRelay()
    session = await open_session(relay)
    bot = create_broadcast(session, BOT)
    media = bot.add_media_source(label="bot")
    media.configure_audio("opus", 48000, 1)
    low = media.add_rendition("low", label="180p")
    low.configure_video("vp8")
    await bot.__aenter__()
    await session.close()
    with pytest.raises(moq_wasm.SessionClosed):
        await within(bot.published())

    # Act / Assert: as after leaving the broadcast, nothing raises and nothing is taken
    assert media.push_audio(b"frame", 0) is False
    assert low.push_video(b"frame", 0) is False
    await within(bot.__aexit__(None, None, None))


@pytest.mark.parametrize("request_kind", ["keyframe_request", "demand_change"])
async def test_a_request_waiting_as_the_relay_refuses_the_media_announce_raises_the_refusal(request_kind):
    # Arrange: the relay refuses the media broadcast's PUBLISH_NAMESPACE once the gate opens
    relay = FakeRelay()
    session = await open_session(relay)
    native = relay.sessions[0]
    gate = asyncio.Event()
    building = native.media_broadcast

    def refusing_broadcast(namespace):
        broadcast = building(namespace)

        async def refuse() -> None:
            await gate.wait()
            raise RuntimeError("PUBLISH_NAMESPACE refused")

        broadcast.announce = refuse
        return broadcast

    native.media_broadcast = refusing_broadcast
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        waiting = asyncio.ensure_future(getattr(media, request_kind)())
        await asyncio.sleep(0.01)

        # Act
        gate.set()

        # Assert
        with pytest.raises(RuntimeError, match="PUBLISH_NAMESPACE refused"):
            await within(waiting)
    await session.close()


async def test_remove_track_before_publishing_forgets_the_kind_and_after_it_reaches_the_native_source():
    # Arrange
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        media.configure_video("vp8")
        low = media.add_rendition("low", label="low")
        low.configure_video("vp8")
        media.configure_audio("opus", 48000, 1)
        await media.remove_track("video")
        await bot.published()

        # Act
        await media.remove_track("audio")

        # Assert: the removal before publishing reached no native source; the video was never configured
        native = relay.sessions[0].media_broadcasts[0].sources[0]
        assert native.calls == [("configure_audio", "opus", 48000, 1), ("remove_track", "audio")]
        assert native.renditions[0].calls == []
        with pytest.raises(ValueError, match="screen"):
            await media.remove_track("screen")
    await session.close()


async def test_a_removal_whose_caller_timed_out_runs_to_its_end_and_a_retry_joins_it():
    # Arrange: the video's removal waits for the relay before its renditions' removals
    relay = FakeRelay()
    session = await open_session(relay)
    async with create_broadcast(session, BOT) as bot:
        media = bot.add_media_source(label="bot")
        media.configure_video("vp8")
        media.add_rendition("low", label="low")
        await bot.published()
        native = relay.sessions[0].media_broadcasts[0].sources[0]
        native.remove_gate = asyncio.Event()

        # Act
        with pytest.raises(TimeoutError):
            await asyncio.wait_for(media.remove_track("video"), 0.05)
        retry = asyncio.create_task(media.remove_track("video"))
        await asyncio.sleep(0)
        native.remove_gate.set()
        await within(retry)

        # Assert: the renditions were removed, and the retry started no second removal
        assert native.calls[-2:] == [("remove_track", "video"), ("remove_renditions",)]
        assert native.calls.count(("remove_track", "video")) == 1
    await session.close()


async def test_removing_the_video_ends_it_and_its_renditions_and_drops_them_from_the_catalog(relay_url):
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
        low = media.add_rendition("vp8", label="bot vp8")
        low.configure_video(vp8.codec_string)
        media.configure_audio("opus", 48000, 1)
        await bot.published()
        frames = 0

        def push_frame() -> None:
            nonlocal frames
            image, timestamp = picture(frames), frames * 1_000_000 // FRAMERATE
            for packet in h264.encode(image, SIZE, "RGB"):
                media.push_video(packet, timestamp)
            for packet in vp8.encode(image, SIZE, "RGB"):
                low.push_video(packet, timestamp)
            media.push_audio(b"opus frame", timestamp)
            frames += 1

        push_frame()
        catalogs = remote.catalogs()
        await within(catalog_listing(catalogs, "video_vp8", push_frame))
        video = await within(subscriber.subscribe(namespace, "video"))

        # Act
        await within(media.remove_track("video"))
        done = await within(video.done())
        catalog = await within(catalog_without(catalogs, {"video", "video_vp8"}))

        # Assert: the audio stays
        assert done is not None
        assert "audio" in {track.name for track in catalog.tracks}
        await catalogs.aclose()
    for session in (subscriber, publisher):
        await session.close()
