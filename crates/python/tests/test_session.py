import asyncio
import uuid

import pytest

import moqt

TIMEOUT_SECONDS = 10
# moqt ends a subscription this long after its PUBLISH_DONE (PUBLISH_DONE_GRACE_PERIOD).
PUBLISH_DONE_GRACE_SECONDS = 10
TRACK = "timeline"


def room() -> list[str]:
    return ["anon", f"room-{uuid.uuid4().hex[:8]}"]


async def within(awaitable):
    return await asyncio.wait_for(awaitable, TIMEOUT_SECONDS)


async def connect(relay_url: str) -> moqt.Session:
    # The relay serves this workspace's self-signed certificate.
    return await moqt.connect(relay_url, verify_certificate=False)


async def publisher_with_track(relay_url: str, namespace: list[str]):
    session = await connect(relay_url)
    # Registered before the namespace, since the relay may forward a SUBSCRIBE at once.
    track = await session.track(namespace, TRACK, retention_seconds=60)
    await session.publish_namespace(namespace)
    return session, track


async def test_a_published_namespace_and_its_withdrawal_reach_the_prefix_subscriber(relay_url):
    # Arrange
    prefix = room()
    watcher = await connect(relay_url)
    events = await within(watcher.subscribe_namespace(prefix))
    publisher = await connect(relay_url)

    # Act
    await publisher.publish_namespace([*prefix, "alice"])
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
    late_track = await within(late.subscribe(namespace, TRACK, current_group=True))
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

    # Assert
    await within(session.closed())
    for request in (
        session.subscribe(namespace, TRACK),
        session.subscribe_namespace(namespace),
        session.publish_namespace(namespace),
    ):
        with pytest.raises(RuntimeError, match="connection lost"):
            await within(request)


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

    # Assert: the reset of the open group does not surface; PUBLISH_DONE ends the track
    with pytest.raises(StopAsyncIteration):
        await asyncio.wait_for(objects.__anext__(), PUBLISH_DONE_GRACE_SECONDS + TIMEOUT_SECONDS)
    await subscriber.close()


async def test_a_subscriber_after_an_unsubscribe_inside_an_open_group_gets_its_later_objects(relay_url):
    # Arrange: the relay's UNSUBSCRIBE makes the publisher leave the open group mid-way
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    first = await connect(relay_url)
    first_objects = await within(first.subscribe(namespace, TRACK))
    await track.write(b"v1", new_group=True)
    assert (await within(first_objects.__anext__())).payload == b"v1"
    await first_objects.unsubscribe()
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
    late_objects = await within(late.subscribe(namespace, TRACK, current_group=True))
    await track.write(b"v3")

    # Assert
    assert [(await within(late_objects.__anext__())).payload for _ in range(3)] == [b"v1", b"v2", b"v3"]
    for session in (late, publisher):
        await session.close()
