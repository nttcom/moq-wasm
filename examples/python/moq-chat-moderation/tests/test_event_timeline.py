from moq_chat_moderation.event_timeline import EventTimeline, location_record
from tests.helpers import RecordingPublication, published_timeline


async def test_publishing_opens_a_group_with_every_record_so_far():
    # Arrange
    timeline = EventTimeline()
    await timeline.append(location_record((10, 0), {"labels": [{"name": "暴言", "probability": 0.93}]}))
    publication = RecordingPublication()

    # Act
    await timeline.publish_on(publication)

    # Assert
    assert await publication.next_write() == (True, [{"l": [10, 0], "data": {"labels": [{"name": "暴言", "probability": 0.93}]}}])


async def test_appended_record_follows_in_the_group_as_its_own_object():
    # Arrange
    timeline, publication = await published_timeline()

    # Act
    await timeline.append(location_record((11, 0), {"labels": []}))

    # Assert
    assert await publication.next_write() == (False, [{"l": [11, 0], "data": {"labels": []}}])
