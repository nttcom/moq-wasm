from moq_meeting_transcriber.transcript import GROUP_SECONDS, TranscriptRecord, TranscriptTimeline
from tests.helpers import FakeClock, RecordingPublication

ALICE = TranscriptRecord(1_792_399_956_554, "alice", "こんにちは")
BOB = TranscriptRecord(1_792_399_960_000, "bob", "今日は良い天気ですね")


async def test_a_group_opens_empty_and_adds_one_record_per_object():
    # Arrange
    publication = RecordingPublication()
    timeline = TranscriptTimeline(publication, now=FakeClock(1_000.0))
    await timeline.open()

    # Act
    await timeline.append(ALICE)

    # Assert
    assert publication.writes == [
        (True, []),
        (False, [{"t": ALICE.started_at_ms, "data": {"speaker": "alice", "text": "こんにちは"}}]),
    ]


async def test_a_new_group_after_the_group_period_repeats_every_record():
    # Arrange
    clock = FakeClock(1_000.0)
    publication = RecordingPublication()
    timeline = TranscriptTimeline(publication, now=clock)
    await timeline.open()
    await timeline.append(ALICE)

    # Act
    clock.now += GROUP_SECONDS
    await timeline.append(BOB)

    # Assert
    new_group, records = publication.writes[-1]
    assert new_group
    assert [record["data"]["speaker"] for record in records] == ["alice", "bob"]
