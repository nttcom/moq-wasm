from pipecat.frames.frames import TextFrame, UserAudioRawFrame

from moq_ai_voice_agent.listener import Listener


def audio_of(participant_id: str) -> UserAudioRawFrame:
    return UserAudioRawFrame(audio=b"\0\0", sample_rate=16000, num_channels=1, user_id=participant_id)


async def test_listens_to_the_earliest_participant_only():
    # Arrange
    listener = Listener()
    listener.joined("alice")
    listener.joined("bob")

    # Act
    accepted = [await listener.accepts(audio_of("alice")), await listener.accepts(audio_of("bob"))]

    # Assert
    assert accepted == [True, False]


async def test_listens_to_the_next_participant_once_the_earliest_leaves():
    # Arrange
    listener = Listener()
    listener.joined("alice")
    listener.joined("bob")

    # Act
    listener.left("alice")

    # Assert
    assert listener.current == "bob"
    assert await listener.accepts(audio_of("bob"))


async def test_passes_frames_other_than_participant_audio():
    # Arrange
    listener = Listener()

    # Act / Assert
    assert await listener.accepts(TextFrame("hello"))
