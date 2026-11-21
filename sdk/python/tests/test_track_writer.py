import asyncio
import uuid

import pytest

import moq_wasm
from moq_wasm.media import create_broadcast

from tests.relay_sessions import TRACK, close_all, connect, publisher_with_track, room, within


async def test_a_published_track_reaches_the_relay_before_anyone_subscribes(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher = await connect(relay_url)
    await within(publisher.announce(namespace))
    track = await within(publisher.publish_track(namespace, TRACK, publish=True))

    # Act
    taken = await within(track.write(b"v1", new_group=True))
    subscriber = await connect(relay_url)
    objects = await within(subscriber.subscribe(namespace, TRACK))
    await within(track.write(b"v2", new_group=True))

    # Assert: the relay took v1 on the PUBLISH; the subscriber gets what follows its SUBSCRIBE
    assert taken is not None
    assert (await within(objects.__anext__())).payload == b"v2"
    await close_all(subscriber, publisher)


async def test_an_unpublished_track_without_subscribers_takes_no_object(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)

    # Act
    taken = await within(track.write(b"v1", new_group=True))

    # Assert
    assert taken is None
    await publisher.close()


async def test_a_datagram_track_delivers_its_objects(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher = await connect(relay_url)
    track = await within(publisher.publish_track(namespace, TRACK, datagram=True))
    await within(publisher.announce(namespace))
    subscriber = await connect(relay_url)
    objects = await within(subscriber.subscribe(namespace, TRACK))

    # Act
    await within(track.write(b"d1", new_group=True))
    received = await within(objects.__anext__())

    # Assert: a datagram carries no subgroup
    assert (received.payload, received.subgroup_id) == (b"d1", 0)
    await close_all(subscriber, publisher)


async def test_a_closed_datagram_track_registered_again_in_datagrams_delivers_its_objects(relay_url):
    # Arrange: a datagram track that sent an object to a subscriber, then was closed
    namespace = [*room(), "alice"]
    publisher = await connect(relay_url)
    track = await within(publisher.publish_track(namespace, TRACK, datagram=True))
    await within(publisher.announce(namespace))
    subscriber = await connect(relay_url)
    objects = await within(subscriber.subscribe(namespace, TRACK))
    await within(track.write(b"d1", new_group=True))
    await within(objects.__anext__())
    await within(track.close())
    await within(objects.close())

    # Act
    reopened = await within(publisher.publish_track(namespace, TRACK, datagram=True))
    later = await within(subscriber.subscribe(namespace, TRACK))
    await within(reopened.write(b"d2", new_group=True))
    received = await within(later.__anext__())

    # Assert
    assert reopened.datagram
    assert (received.payload, received.subgroup_id) == (b"d2", 0)
    await close_all(subscriber, publisher)


@pytest.mark.parametrize("first", [True, False])
async def test_a_closed_track_registered_again_with_the_other_delivery_is_refused_before_any_write(relay_url, first):
    # Arrange: a track that wrote an object in one delivery, then was closed
    namespace = [*room(), "alice"]
    publisher = await connect(relay_url)
    track = await within(publisher.publish_track(namespace, TRACK, datagram=first))
    await within(publisher.announce(namespace))
    await within(track.write(b"o1", new_group=True))
    await within(track.close())

    # Act / Assert: the native origin keeps the delivery of a track that has an object
    delivery = "in datagrams" if first else "on subgroup streams"
    with pytest.raises(ValueError, match=f"sends its objects {delivery}"):
        await within(publisher.publish_track(namespace, TRACK, datagram=not first))

    # Assert: the refusal left the track free for its own delivery
    reopened = await within(publisher.publish_track(namespace, TRACK, datagram=first))
    assert reopened.datagram == first
    await publisher.close()


async def test_a_closed_track_registered_again_with_another_retention_is_refused_before_any_write(relay_url):
    # Arrange: a track kept for 60 s that wrote an object, then was closed
    namespace = [*room(), "alice"]
    publisher = await connect(relay_url)
    track = await within(publisher.publish_track(namespace, TRACK, retention_seconds=60))
    await within(publisher.announce(namespace))
    await within(track.write(b"o1", new_group=True))
    await within(track.close())

    # Act / Assert: the native origin keeps the retention of the first registration
    with pytest.raises(ValueError, match="register it again with retention_seconds=60"):
        await within(publisher.publish_track(namespace, TRACK, retention_seconds=0))

    # Assert: the refusal left the track free for its own retention
    reopened = await within(publisher.publish_track(namespace, TRACK, retention_seconds=60))
    assert reopened.retention_seconds == 60
    await publisher.close()


async def test_a_datagram_track_whose_retention_was_refused_is_sent_in_datagrams_once_registered(relay_url):
    # Arrange: a datagram registration refused for its retention before it registered anything
    namespace = [*room(), "alice"]
    publisher = await connect(relay_url)
    with pytest.raises(ValueError, match="invalid retention_seconds"):
        await within(publisher.publish_track(namespace, TRACK, datagram=True, retention_seconds=-1))

    # Act
    track = await within(publisher.publish_track(namespace, TRACK, datagram=True))
    await within(publisher.announce(namespace))
    subscriber = await connect(relay_url)
    objects = await within(subscriber.subscribe(namespace, TRACK))
    await within(track.write(b"d1", new_group=True))

    # Assert: the track reports datagrams and sends them, which have no subgroup stream to read
    assert track.datagram
    with pytest.raises(RuntimeError):
        await within(anext(objects.groups()))
    await close_all(subscriber, publisher)


async def test_a_datagram_track_whose_publish_was_refused_keeps_its_datagrams(relay_url, temporary_app_token):
    # Arrange: a temporary app may publish only under its own root, so the relay refuses the PUBLISH
    session = await moq_wasm.connect(
        relay_url, verify_certificate=False, token=temporary_app_token(f"temp-{uuid.uuid4().hex}")
    )
    namespace = [f"temp-{uuid.uuid4().hex}", "room", "bot"]
    with pytest.raises(moq_wasm.RequestError):
        await within(session.publish_track(namespace, TRACK, publish=True, datagram=True))

    # Act / Assert: the native origin registered the track in datagrams before the PUBLISH, which a
    # media broadcast would send its media in
    with pytest.raises(ValueError, match=f"track {TRACK} of .* sends its objects in datagrams"):
        create_broadcast(session, namespace).add_media_source(TRACK, label="bot")
    await session.close()


async def test_writes_report_the_group_each_object_went_into(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)
    objects = await within(subscriber.subscribe(namespace, TRACK))

    # Act
    first = await within(track.write(b"v1", new_group=True))
    same = await within(track.write(b"v1b"))
    second = await within(track.write(b"v2", new_group=True))
    catalog = await within(track.write_group(b"catalog"))
    received = [await within(objects.__anext__()) for _ in range(4)]

    # Assert: objects of different groups may arrive in either order (draft-14 §2.3.1)
    assert [first, same, second, catalog] == [first, first, first + 1, first + 2]
    assert sorted((obj.group_id, obj.payload) for obj in received) == [
        (first, b"v1"),
        (first, b"v1b"),
        (first + 1, b"v2"),
        (first + 2, b"catalog"),
    ]
    await close_all(subscriber, publisher)


async def test_next_subscriber_resolves_when_a_subscribe_is_answered(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    waiting = asyncio.ensure_future(track.next_subscriber())
    subscriber = await connect(relay_url)

    # Act
    await within(subscriber.subscribe(namespace, TRACK))
    subscribed = await within(waiting)

    # Assert
    assert subscribed is True
    await close_all(subscriber, publisher)


async def test_concurrent_writes_each_reach_the_subscriber_in_the_order_they_were_made(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)
    objects = await within(subscriber.subscribe(namespace, TRACK))
    payloads = [f"group {index}".encode() for index in range(32)]

    # Act
    groups = await within(asyncio.gather(*(track.write_group(payload) for payload in payloads)))
    received = [await within(objects.__anext__()) for _ in payloads]

    # Assert: objects of different groups may arrive in either order (draft-14 §2.3.1)
    assert groups == list(range(groups[0], groups[0] + len(payloads)))
    assert sorted((obj.group_id, obj.payload) for obj in received) == list(zip(groups, payloads))
    await close_all(subscriber, publisher)


async def test_a_write_whose_caller_timed_out_mid_send_leaves_later_writes_to_the_subscriber(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)
    objects = await within(subscriber.subscribe(namespace, TRACK))
    large = bytes(16 * 1024 * 1024)

    # Act: the timeout cancels the write once its native send started
    with pytest.raises(TimeoutError):
        await asyncio.wait_for(track.write_group(large), 0.001)
    later = await within(track.write_group(b"later"))

    # Assert
    received = [await within(objects.__anext__()) for _ in range(2)]
    assert later is not None
    assert sorted(obj.payload for obj in received) == sorted([large, b"later"])
    await close_all(subscriber, publisher)
