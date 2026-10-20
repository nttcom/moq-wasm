from moq_meeting_transcriber.status import READY, STARTING, write_status
from tests.helpers import RecordingPublication


async def test_each_state_is_a_group_of_its_own_naming_the_recognizer():
    # Arrange
    publication = RecordingPublication()

    # Act
    await write_status(publication, "qwen3-asr", STARTING)
    await write_status(publication, "qwen3-asr", READY)

    # Assert
    assert publication.writes == [
        (True, {"asr": "qwen3-asr", "state": "starting"}),
        (True, {"asr": "qwen3-asr", "state": "ready"}),
    ]
