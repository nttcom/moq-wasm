import asyncio

import pytest

import moq_wasm
from moq_wasm._subscription import _cancel
from tests.cancellation import cancel_as_it_resumes, read
from tests.relay_sessions import TRACK, close_all, connect, publisher_with_track, room, within

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


def test_the_read_of_an_iterator_collected_after_its_loop_closed_is_left_without_an_error():
    # Arrange
    loop = asyncio.new_event_loop()
    read = loop.create_future()
    loop.close()

    # Act / Assert
    _cancel(read)


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


async def test_reading_groups_after_a_read_by_object_timed_out_raises_at_once(relay_url):
    # Arrange: a read by object timed out with nothing published
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)
    subscription = await within(subscriber.subscribe(namespace, TRACK))
    with pytest.raises(TimeoutError):
        await asyncio.wait_for(anext(subscription), NOTHING_ARRIVES_SECONDS)

    # Act / Assert: no object needs to arrive for the refusal
    with pytest.raises(RuntimeError, match="read by object"):
        await asyncio.wait_for(anext(subscription.groups()), NOTHING_ARRIVES_SECONDS)
    await close_all(subscriber, publisher)


@pytest.mark.parametrize("subscribe", ["subscribe_current_group", "subscribe_group_start"])
async def test_a_joining_subscription_refuses_groups_and_stays_readable_by_object(relay_url, subscribe):
    # Arrange
    namespace = [*room(), "alice"]
    sessions, _, _ = await subscribed_track(relay_url, namespace)
    late = await connect(relay_url)
    arguments = (1000,) if subscribe == "subscribe_group_start" else ()
    subscription = await within(getattr(late, subscribe)(namespace, TRACK, *arguments))

    # Act
    with pytest.raises(RuntimeError, match="read by object"):
        await within(anext(subscription.groups()))
    first = await within(anext(subscription))

    # Assert: the native reader merges the joining FETCH, so reading by object is the valid mode
    assert first.payload == b"o0"
    await close_all(late, *reversed(sessions))


async def test_an_object_a_cancelled_read_took_goes_to_the_next_read(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)
    objects = await within(subscriber.subscribe(namespace, TRACK))
    reading = asyncio.create_task(read(objects))
    cancelled = cancel_as_it_resumes(reading)

    # Act
    await within(track.write(b"one", new_group=True))
    await within(cancelled)
    await within(track.write(b"two"))

    # Assert
    with pytest.raises(asyncio.CancelledError):
        await reading
    assert (await within(anext(objects))).payload == b"one"
    await close_all(subscriber, publisher)


async def test_an_object_a_cancelled_read_of_a_group_took_goes_to_the_next_read(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)
    groups = (await within(subscriber.subscribe(namespace, TRACK))).groups()
    await within(track.write(b"one", new_group=True))
    group = await within(anext(groups))
    reading = asyncio.create_task(read(group))
    cancelled = cancel_as_it_resumes(reading)

    # Act
    await within(cancelled)
    await within(track.write(b"two"))

    # Assert
    with pytest.raises(asyncio.CancelledError):
        await reading
    assert (await within(anext(group))).payload == b"one"
    await close_all(subscriber, publisher)


async def test_a_group_a_cancelled_read_took_goes_to_the_next_groups_call(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)
    subscription = await within(subscriber.subscribe(namespace, TRACK))
    reading = asyncio.create_task(read(subscription.groups()))
    cancelled = cancel_as_it_resumes(reading)

    # Act
    await within(track.write(b"one", new_group=True))
    await within(cancelled)

    # Assert
    with pytest.raises(asyncio.CancelledError):
        await reading
    group = await within(anext(subscription.groups()))
    assert (await within(anext(group))).payload == b"one"
    await close_all(subscriber, publisher)


async def test_an_object_a_cancelled_read_of_a_fetch_took_goes_to_the_next_read(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    sessions, _, group_id = await subscribed_track(relay_url, namespace)
    late = await connect(relay_url)
    whole_group = moq_wasm.Location(group_id, 0)
    fetch = await within(late.fetch(namespace, TRACK, whole_group, whole_group))
    reading = asyncio.create_task(read(fetch))
    cancelled = cancel_as_it_resumes(reading)

    # Act
    await within(cancelled)

    # Assert
    with pytest.raises(asyncio.CancelledError):
        await reading
    assert [obj.payload async for obj in fetch] == [b"o0", b"o1"]
    await close_all(late, *reversed(sessions))


async def reader_after_one_object(subscriber: moq_wasm.Session, track: moq_wasm.TrackWriter, namespace):
    """A subscription to the track that read one object."""
    objects = await within(subscriber.subscribe(namespace, TRACK))
    first = asyncio.ensure_future(anext(objects))
    await track.write(b"o0", new_group=True)
    await within(first)
    return objects


async def test_closing_the_session_ends_a_waiting_read_with_session_closed(relay_url):
    # Arrange: the reader waits for its next object
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)
    objects = await reader_after_one_object(subscriber, track, namespace)
    waiting = asyncio.ensure_future(anext(objects))
    await asyncio.sleep(0.1)

    # Act
    await within(subscriber.close())

    # Assert
    with pytest.raises(moq_wasm.SessionClosed):
        await within(waiting)
    await publisher.close()


async def test_an_object_a_timed_out_read_left_is_not_read_once_the_session_closed(relay_url):
    # Arrange: a read timed out, and the object written after it reached the read it left
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)
    objects = await within(subscriber.subscribe(namespace, TRACK))
    with pytest.raises(TimeoutError):
        await asyncio.wait_for(anext(objects), 0.1)
    await track.write(b"o0", new_group=True)
    await asyncio.sleep(NOTHING_ARRIVES_SECONDS)

    # Act
    await within(subscriber.close())

    # Assert
    with pytest.raises(moq_wasm.SessionClosed):
        await within(anext(objects))
    await publisher.close()


async def test_the_session_closing_lets_go_of_an_object_a_timed_out_read_left(relay_url):
    # Arrange: a read timed out, and the object written after it reached the read it left
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)
    objects = await within(subscriber.subscribe(namespace, TRACK))
    with pytest.raises(TimeoutError):
        await asyncio.wait_for(anext(objects), 0.1)
    await track.write(b"o0", new_group=True)
    await asyncio.sleep(NOTHING_ARRIVES_SECONDS)

    # Act
    await within(subscriber.close())
    await asyncio.sleep(0)

    # Assert: no read is needed for the subscription to drop the object
    assert objects._objects._pending is None
    await publisher.close()
