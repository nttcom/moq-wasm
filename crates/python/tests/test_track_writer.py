import asyncio

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
