from moq_system_one_bot.event_timeline import EventTimeline, location_record
from tests.helpers import RecordingPublication

VERDICT = {"labels": [{"name": "暴言", "probability": 0.93}]}


async def test_replaying_timeline_opens_a_group_with_every_record_so_far():
    # Arrange
    timeline = EventTimeline(replay=True)
    await timeline.append(location_record((10, 0), VERDICT))
    publication = RecordingPublication()

    # Act
    await timeline.publish_on(publication)

    # Assert
    assert await publication.next_write() == (True, [{"l": [10, 0], "data": VERDICT}])


async def test_timeline_without_replay_opens_a_group_with_no_record():
    # Arrange
    timeline = EventTimeline()
    await timeline.append(location_record((10, 0), VERDICT))
    publication = RecordingPublication()

    # Act
    await timeline.publish_on(publication)

    # Assert
    assert await publication.next_write() == (True, [])


async def test_appended_record_follows_in_the_group_as_its_own_object():
    # Arrange
    timeline = EventTimeline()
    publication = RecordingPublication()
    await timeline.publish_on(publication)
    await publication.next_write()

    # Act
    await timeline.append(location_record((11, 0), {"labels": []}))

    # Assert
    assert await publication.next_write() == (False, [{"l": [11, 0], "data": {"labels": []}}])
