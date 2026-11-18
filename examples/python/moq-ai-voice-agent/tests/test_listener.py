import asyncio

import pytest
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


async def test_returns_once_the_room_stays_empty():
    # Arrange
    listener = Listener()

    # Act / Assert
    await asyncio.wait_for(listener.until_empty_for(0.01), 1)


async def test_waits_while_someone_is_in_the_room():
    # Arrange
    listener = Listener()
    listener.joined("alice")

    # Act / Assert
    with pytest.raises(TimeoutError):
        await asyncio.wait_for(listener.until_empty_for(0.01), 0.1)


async def test_returns_after_the_last_participant_leaves():
    # Arrange
    listener = Listener()
    listener.joined("alice")
    waiting = asyncio.create_task(listener.until_empty_for(0.01))
    await asyncio.sleep(0.05)

    # Act
    listener.left("alice")

    # Assert
    await asyncio.wait_for(waiting, 1)
