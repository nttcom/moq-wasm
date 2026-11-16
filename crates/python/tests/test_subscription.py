import asyncio

import pytest

import moq_wasm
from tests.relay_sessions import TRACK, connect, publisher_with_track, room, within

# Long enough for the relay to have forwarded an object it was going to forward.
NOTHING_ARRIVES_SECONDS = 1
# Object Status End of Group (draft-14 §10.2.1.1).
END_OF_GROUP = 3


async def subscribed_track(relay_url: str, namespace: list[str]):
    """A publisher with a track the relay holds through a first subscriber, which has read one
    group of two objects; returns the sessions, the track writer and that group's id."""
    publisher, track = await publisher_with_track(relay_url, namespace)
    holder = await connect(relay_url)
    objects = await within(holder.subscribe(namespace, TRACK))
    await track.write(b"o0", new_group=True)
    await track.write(b"o1")
    group_id = (await within(objects.__anext__())).group_id
    await within(objects.__anext__())
    return (publisher, holder), track, group_id


async def close_all(*sessions: moq_wasm.Session) -> None:
    for session in sessions:
        await session.close()


async def test_a_subscription_without_forward_receives_once_forward_is_set(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)
    objects = await within(subscriber.subscribe(namespace, TRACK, forward=False))
    await track.write(b"v1", new_group=True)
    pending = asyncio.ensure_future(objects.__anext__())
    arrived, _ = await asyncio.wait({pending}, timeout=NOTHING_ARRIVES_SECONDS)

    # Act: on draft-18 set_forward returns once the relay answered the REQUEST_UPDATE (§10.9)
    await within(objects.set_forward(True))
    await track.write(b"v2", new_group=True)

    # Assert
    assert not arrived
    assert (await within(pending)).payload == b"v2"
    await close_all(subscriber, publisher)


async def test_next_group_start_skips_the_rest_of_the_open_group(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    await track.write(b"v1", new_group=True)
    subscriber = await connect(relay_url)
    objects = await within(subscriber.subscribe(namespace, TRACK, filter="next_group_start"))

    # Act
    await track.write(b"v1b")
    await track.write(b"v2", new_group=True)

    # Assert
    assert (await within(objects.__anext__())).payload == b"v2"
    await close_all(subscriber, publisher)


async def test_an_unknown_filter_is_a_value_error(relay_url):
    # Arrange
    subscriber = await connect(relay_url)

    # Act / Assert
    with pytest.raises(ValueError, match="unknown subscribe filter"):
        await subscriber.subscribe([*room(), "alice"], TRACK, filter="latest")
    await subscriber.close()


async def test_subscribe_group_start_reads_the_current_group_from_its_first_object(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    sessions, track, _ = await subscribed_track(relay_url, namespace)
    late = await connect(relay_url)

    # Act
    objects = await within(late.subscribe_group_start(namespace, TRACK, 1000))
    await track.write(b"o2")

    # Assert
    assert [(await within(objects.__anext__())).payload for _ in range(3)] == [b"o0", b"o1", b"o2"]
    await close_all(late, *reversed(sessions))


async def test_track_status_largest_and_fetch_report_the_cached_group(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    sessions, _, group_id = await subscribed_track(relay_url, namespace)
    late = await connect(relay_url)
    whole_group = moq_wasm.Location(group_id, 0)

    # Act
    largest = await within(late.track_status(namespace, TRACK))
    subscription = await within(late.subscribe(namespace, TRACK))
    fetch = await within(late.fetch(namespace, TRACK, whole_group, whole_group))
    payloads = [obj.payload async for obj in fetch]

    # Assert
    assert largest == moq_wasm.Location(group_id, 1)
    assert subscription.largest == moq_wasm.Location(group_id, 1)
    assert payloads == [b"o0", b"o1"]
    await close_all(late, *reversed(sessions))


async def test_groups_yield_each_subgroup_stream_ending_with_end_of_group(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)
    groups = (await within(subscriber.subscribe(namespace, TRACK))).groups()

    # Act
    await track.write(b"v1", new_group=True)
    group = await within(groups.__anext__())
    first = await within(group.__anext__())
    await track.write(b"v2", new_group=True)
    rest = [obj async for obj in group]

    # Assert
    assert (first.group_id, first.subgroup_id, first.object_id, first.status) == (group.group_id, 0, 0, None)
    assert group.subgroup_id == 0
    assert [obj.status for obj in rest] == [END_OF_GROUP]
    await close_all(subscriber, publisher)


async def test_request_new_group_does_not_send_for_a_track_without_dynamic_groups(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, _ = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)
    subscription = await within(subscriber.subscribe(namespace, TRACK))

    # Act
    sent = await within(subscription.request_new_group(None))

    # Assert
    assert sent is False
    await close_all(subscriber, publisher)


async def test_end_before_group_ends_the_subscription_with_publish_done(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)
    subscription = await within(subscriber.subscribe(namespace, TRACK))
    await track.write(b"v1", new_group=True)
    group_id = (await within(subscription.__anext__())).group_id

    # Act
    await within(subscription.end_before_group(group_id + 1))
    await track.write(b"v2", new_group=True)
    done = await within(subscription.done())

    # Assert
    assert isinstance(done, moq_wasm.PublishDone)
    await close_all(subscriber, publisher)


async def test_done_is_none_once_the_subscription_is_closed(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, _ = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)
    subscription = await within(subscriber.subscribe(namespace, TRACK))

    # Act
    await within(subscription.close())

    # Assert
    assert await within(subscription.done()) is None
    await close_all(subscriber, publisher)
