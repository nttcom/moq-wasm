from moq_meeting_transcriber.transcript import TranscriptRecord, TranscriptTimeline
from tests.helpers import RecordingPublication

ALICE = TranscriptRecord(1_792_399_956_554, "alice", "こんにちは")
BOB = TranscriptRecord(1_792_399_960_000, "bob", "今日は良い天気ですね")


async def test_the_group_opens_empty_and_adds_one_record_per_object():
    # Arrange
    publication = RecordingPublication()
    timeline = TranscriptTimeline(publication)
    await timeline.open()

    # Act
    await timeline.append(ALICE)
    await timeline.append(BOB)

    # Assert
    assert publication.writes == [
        (True, []),
        (False, [{"t": ALICE.started_at_ms, "data": {"speaker": "alice", "text": "こんにちは"}}]),
        (False, [{"t": BOB.started_at_ms, "data": {"speaker": "bob", "text": "今日は良い天気ですね"}}]),
    ]
