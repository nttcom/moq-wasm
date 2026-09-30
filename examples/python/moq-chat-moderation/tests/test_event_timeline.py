import moq

from moq_chat_moderation.event_timeline import EventTimeline, location_record
from tests.helpers import open_timeline

TRACK_NAME = "eventtimeline"


async def test_group_opens_with_every_record_so_far():
    # Arrange
    timeline = EventTimeline(TRACK_NAME)
    timeline.append(location_record((10, 0), {"abusive": True}))
    broadcast = moq.BroadcastProducer()

    # Act
    timeline.publish_on(broadcast)

    # Assert
    reader = await open_timeline(broadcast, TRACK_NAME)
    assert await reader.next_records() == [{"l": [10, 0], "data": {"abusive": True}}]


async def test_appended_record_follows_as_its_own_object():
    # Arrange
    timeline = EventTimeline(TRACK_NAME)
    broadcast = moq.BroadcastProducer()
    timeline.publish_on(broadcast)
    reader = await open_timeline(broadcast, TRACK_NAME)
    await reader.next_records()

    # Act
    timeline.append(location_record((11, 0), {"abusive": False}))

    # Assert
    assert await reader.next_records() == [{"l": [11, 0], "data": {"abusive": False}}]


async def test_new_broadcast_replays_the_records_in_a_later_group():
    # Arrange
    timeline = EventTimeline(TRACK_NAME)
    first_broadcast = moq.BroadcastProducer()
    timeline.publish_on(first_broadcast)
    first_reader = await open_timeline(first_broadcast, TRACK_NAME)
    timeline.append(location_record((12, 0), {"abusive": True}))
    second_broadcast = moq.BroadcastProducer()

    # Act
    timeline.publish_on(second_broadcast)

    # Assert
    second_reader = await open_timeline(second_broadcast, TRACK_NAME)
    assert await second_reader.next_records() == [{"l": [12, 0], "data": {"abusive": True}}]
    assert second_reader.group_id > first_reader.group_id
