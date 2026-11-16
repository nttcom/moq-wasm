from pipecat.frames.frames import TextFrame
from pipecat.tests.utils import run_test

from moq_meeting_transcriber.stt import LANGUAGE, UtteranceTranscriptionFrame
from moq_meeting_transcriber.transcript import TranscriptWriter
from tests.helpers import recorded_timeline


async def test_each_utterance_is_one_record_at_the_time_it_began():
    # Arrange
    timeline, publication = await recorded_timeline()
    utterance = UtteranceTranscriptionFrame("こんにちは", "alice", "2026-11-16T00:00:00Z", LANGUAGE, started_at_ms=1_792_399_956_554)

    # Act
    await run_test(TranscriptWriter(timeline), frames_to_send=[utterance, TextFrame("other")], expected_down_frames=[UtteranceTranscriptionFrame, TextFrame])

    # Assert
    assert publication.writes == [
        (True, []),
        (False, [{"t": 1_792_399_956_554, "data": {"speaker": "alice", "text": "こんにちは"}}]),
    ]
