from moq_meeting_transcriber.status import READY, RecognizerStatus
from tests.helpers import RecordingPublication


async def test_each_state_is_a_group_of_its_own_naming_the_recognizer():
    # Arrange
    publication = RecordingPublication()
    status = RecognizerStatus(publication, "qwen3-asr")
    await publication.connect()

    # Act
    await status.set(READY)

    # Assert
    assert publication.writes == [
        (True, {"asr": "qwen3-asr", "state": "starting"}),
        (True, {"asr": "qwen3-asr", "state": "ready"}),
    ]


async def test_a_new_connection_reads_the_current_state_first():
    # Arrange
    publication = RecordingPublication()
    status = RecognizerStatus(publication, "whisper")
    await status.set(READY)
    publication.writes.clear()

    # Act
    await publication.connect()

    # Assert
    assert publication.writes == [(True, {"asr": "whisper", "state": "ready"})]
