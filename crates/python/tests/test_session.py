import asyncio
import signal
import uuid

import pytest

import moq_wasm

TIMEOUT_SECONDS = 10
# moqt ends a subscription this long after its PUBLISH_DONE (PUBLISH_DONE_GRACE_PERIOD).
PUBLISH_DONE_GRACE_SECONDS = 10
# The relay's default drain timeout bounds how long a client keeps the connection a GOAWAY retired.
DRAIN_SECONDS = 30
TRACK = "timeline"


def room() -> list[str]:
    return ["anon", f"room-{uuid.uuid4().hex[:8]}"]


async def within(awaitable):
    return await asyncio.wait_for(awaitable, TIMEOUT_SECONDS)


async def connect(relay_url: str) -> moq_wasm.Session:
    # The relay serves this workspace's self-signed certificate.
    return await moq_wasm.connect(relay_url, verify_certificate=False)


async def publisher_with_track(relay_url: str, namespace: list[str]):
    session = await connect(relay_url)
    # Registered before the namespace, since the relay may forward a SUBSCRIBE at once.
    track = await session.publish_track(namespace, TRACK, retention_seconds=60)
    await session.announce(namespace)
    return session, track


async def test_a_published_namespace_and_its_withdrawal_reach_the_prefix_subscriber(relay_url):
    # Arrange
    prefix = room()
    watcher = await connect(relay_url)
    events = await within(watcher.watch_namespaces(prefix))
    publisher = await connect(relay_url)

    # Act
    await publisher.announce([*prefix, "alice"])
    published = await within(events.__anext__())
    await publisher.close()
    withdrawn = await within(events.__anext__())

    # Assert
    assert (published.namespace, published.active) == ([*prefix, "alice"], True)
    assert (withdrawn.namespace, withdrawn.active) == ([*prefix, "alice"], False)
    await watcher.close()


async def test_current_group_brings_the_object_the_relay_already_holds(relay_url):
    # Arrange: a first subscriber makes the relay hold the track before the second one comes
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    first = await connect(relay_url)
    first_track = await within(first.subscribe(namespace, TRACK))
    await track.write(b"v1", new_group=True)
    assert (await within(first_track.__anext__())).payload == b"v1"
    late = await connect(relay_url)

    # Act
    late_track = await within(late.subscribe_current_group(namespace, TRACK))
    await track.write(b"v2", new_group=True)

    # Assert
    assert [(await within(late_track.__anext__())).payload for _ in range(2)] == [b"v1", b"v2"]
    for session in (late, first, publisher):
        await session.close()


async def test_a_subscribe_to_a_track_the_publisher_does_not_have_fails(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, _ = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)

    # Act / Assert
    with pytest.raises(RuntimeError, match="TrackDoesNotExist"):
        await within(subscriber.subscribe(namespace, "unknown"))
    for session in (subscriber, publisher):
        await session.close()


async def test_close_resolves_closed_and_fails_later_calls(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    session = await connect(relay_url)

    # Act
    await within(session.close())

    # Assert: draft-14 fails writing the control stream; draft-18 fails opening the request's stream
    await within(session.closed())
    for request in (
        session.subscribe(namespace, TRACK),
        session.watch_namespaces(namespace),
        session.announce(namespace),
    ):
        with pytest.raises(RuntimeError, match="connection lost|the session closed"):
            await within(request)


async def test_a_session_offering_draft18_works_with_a_draft18_relay(draft18_relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    session = await moq_wasm.connect(draft18_relay_url, verify_certificate=False, versions=["draft-18"])

    # Act / Assert: the relay accepts the request, so the session speaks its draft
    await within(session.announce(namespace))
    await session.close()


async def test_an_unknown_version_name_is_a_value_error():
    # Act / Assert
    with pytest.raises(ValueError, match="draft-99"):
        await moq_wasm.connect("moqt://127.0.0.1:4433", versions=["draft-99"])


def temporary_app_id() -> str:
    return f"temp-{uuid.uuid4().hex}"


async def test_a_refreshed_token_keeps_the_session_working_under_its_app(relay_url, temporary_app_token):
    # Arrange
    app_id = temporary_app_id()
    session = await moq_wasm.connect(relay_url, verify_certificate=False, token=temporary_app_token(app_id))

    # Act
    await within(session.refresh_token(temporary_app_token(app_id), app_id=app_id))

    # Assert: a request after the refresh is still authorized under the app's root
    await within(session.announce([app_id, "room", "alice"]))
    await session.close()


async def test_a_token_of_another_app_is_refused_as_a_refresh(relay_url, temporary_app_token):
    # Arrange
    app_id = temporary_app_id()
    session = await moq_wasm.connect(relay_url, verify_certificate=False, token=temporary_app_token(app_id))

    # Act / Assert
    with pytest.raises(RuntimeError):
        await within(session.refresh_token(temporary_app_token(temporary_app_id()), app_id=app_id))
    await session.close()


async def test_a_session_without_a_token_cannot_refresh_one(relay_url, temporary_app_token):
    # Arrange
    app_id = temporary_app_id()
    session = await connect(relay_url)

    # Act / Assert
    with pytest.raises(RuntimeError, match="without a token"):
        session.refresh_token(temporary_app_token(app_id), app_id=app_id)
    await session.close()


async def test_the_subscription_ends_when_the_publisher_closes_inside_an_open_group(relay_url):
    # Arrange: the group stays open until the next write starts a new one
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)
    objects = await within(subscriber.subscribe(namespace, TRACK))
    await track.write(b"v1", new_group=True)
    assert (await within(objects.__anext__())).payload == b"v1"

    # Act
    await publisher.close()

    # Assert: the reset of the open group does not raise; PUBLISH_DONE ends the track
    with pytest.raises(StopAsyncIteration):
        await asyncio.wait_for(objects.__anext__(), PUBLISH_DONE_GRACE_SECONDS + TIMEOUT_SECONDS)
    await subscriber.close()


async def test_a_subscriber_after_an_unsubscribe_inside_an_open_group_gets_its_later_objects(relay_url):
    # Arrange: the relay's UNSUBSCRIBE makes the publisher leave the open group
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    first = await connect(relay_url)
    first_objects = await within(first.subscribe(namespace, TRACK))
    await track.write(b"v1", new_group=True)
    assert (await within(first_objects.__anext__())).payload == b"v1"
    await first_objects.close()
    with pytest.raises(StopAsyncIteration):
        await within(first_objects.__anext__())
    second = await connect(relay_url)
    second_objects = await within(second.subscribe(namespace, TRACK))

    # Act
    await track.write(b"v2")

    # Assert
    assert (await within(second_objects.__anext__())).payload == b"v2"
    for session in (second, first, publisher):
        await session.close()


async def test_current_group_brings_the_rest_of_the_group_being_appended(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    await track.write(b"v1", new_group=True)
    await track.write(b"v2")
    late = await connect(relay_url)

    # Act
    late_objects = await within(late.subscribe_current_group(namespace, TRACK))
    await track.write(b"v3")

    # Assert
    assert [(await within(late_objects.__anext__())).payload for _ in range(3)] == [b"v1", b"v2", b"v3"]
    for session in (late, publisher):
        await session.close()


async def test_a_subscription_continues_on_the_relay_a_draining_relay_names(relays_sharing_redis):
    # Arrange: the publisher stays on relay B
    relay_a, relay_b = relays_sharing_redis
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_b.url, namespace)
    subscriber = await connect(relay_a.url)
    objects = await within(subscriber.subscribe(namespace, TRACK))
    await track.write(b"v1", new_group=True)
    assert (await within(objects.__anext__())).payload == b"v1"

    # Act: v3 is written once relay A is gone, so only relay B can bring it
    relay_a.process.send_signal(signal.SIGINT)
    await track.write(b"v2", new_group=True)
    await asyncio.wait_for(asyncio.to_thread(relay_a.process.wait), DRAIN_SECONDS)
    await track.write(b"v3", new_group=True)

    # Assert
    assert [(await within(objects.__anext__())).payload for _ in range(2)] == [b"v2", b"v3"]
    for session in (subscriber, publisher):
        await session.close()
