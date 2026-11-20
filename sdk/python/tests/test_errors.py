import pytest

import moq_wasm
from tests.relay_sessions import TRACK, close_all, connect, publisher_with_track, room, within


async def test_a_subscribe_to_a_missing_track_raises_a_request_error_with_its_code(relay_url):
    # Arrange
    subscriber = await connect(relay_url)

    # Act
    with pytest.raises(moq_wasm.RequestError) as raised:
        await within(subscriber.subscribe([*room(), "nobody"], TRACK))

    # Assert: SUBSCRIBE's TRACK_DOES_NOT_EXIST, and a RuntimeError catches it
    rejected = raised.value
    assert (rejected.message_type, rejected.code) == ("SUBSCRIBE", moq_wasm.RequestErrorCode.TRACK_DOES_NOT_EXIST)
    assert rejected.reason
    assert isinstance(rejected, RuntimeError)
    await subscriber.close()


async def test_a_fetch_of_a_range_the_track_does_not_have_raises_a_request_error(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, _ = await publisher_with_track(relay_url, namespace)
    fetcher = await connect(relay_url)
    start = moq_wasm.Location(5, 0)
    end = moq_wasm.Location(1, 0)

    # Act
    with pytest.raises(moq_wasm.RequestError) as raised:
        await within(fetcher.fetch(namespace, TRACK, start, end))

    # Assert
    rejected = raised.value
    assert rejected.message_type == "FETCH"
    assert rejected.code == moq_wasm.RequestErrorCode.INVALID_RANGE
    await close_all(fetcher, publisher)


async def test_transport_stats_count_the_bytes_of_the_traffic(relay_url):
    # Arrange
    namespace = [*room(), "alice"]
    publisher, track = await publisher_with_track(relay_url, namespace)
    subscriber = await connect(relay_url)
    objects = await within(subscriber.subscribe(namespace, TRACK))
    before = subscriber.transport_stats()

    # Act
    await within(track.write(b"x" * 1000, new_group=True))
    await within(objects.__anext__())
    after = subscriber.transport_stats()

    # Assert
    assert after.received_bytes >= before.received_bytes + 1000
    assert after.sent_bytes >= before.sent_bytes
    await close_all(subscriber, publisher)
