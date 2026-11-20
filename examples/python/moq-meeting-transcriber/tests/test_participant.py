import asyncio
from types import SimpleNamespace
from typing import cast

import pytest
from moq_wasm.media import RemoteBroadcast

from moq_meeting_transcriber import participant as participant_module
from moq_meeting_transcriber.participant import Participant
from tests.helpers import HeldRecognizer, OneUtteranceInput, RecordingPublication, passing_vad, recorded_timeline

BROADCAST = cast(RemoteBroadcast, SimpleNamespace(namespace=["app-1", "team a", "alice"]))


async def participant_hearing_one_utterance(monkeypatch, recognizer: HeldRecognizer):
    monkeypatch.setattr(participant_module, "MoqWasmParticipantInput", OneUtteranceInput)
    monkeypatch.setattr(participant_module, "VADProcessor", passing_vad)
    timeline, transcript = await recorded_timeline()
    return Participant(BROADCAST, recognizer, timeline), transcript


def texts(transcript: RecordingPublication) -> list[str]:
    return [record["data"]["text"] for _, records in transcript.writes for record in records]


async def test_a_participant_who_leaves_still_gets_the_utterance_already_heard(monkeypatch):
    # Arrange
    recognizer = HeldRecognizer("こんにちは")
    participant, transcript = await participant_hearing_one_utterance(monkeypatch, recognizer)
    running = asyncio.create_task(participant.run(room=asyncio.current_task()))
    await asyncio.wait_for(recognizer.called.wait(), 5)

    # Act: the namespace is withdrawn while the recognizer works on the utterance, which it answers
    # only after a pipeline being cancelled would have ended
    running.cancel()
    await asyncio.wait([running], timeout=0.5)
    recognizer.release.set()

    # Assert
    with pytest.raises(asyncio.CancelledError):
        await asyncio.wait_for(running, 5)
    assert texts(transcript) == ["こんにちは"]


async def test_a_room_that_ends_drops_the_utterance_the_recognizer_has_not_answered(monkeypatch):
    # Arrange
    recognizer = HeldRecognizer("こんにちは")
    participant, transcript = await participant_hearing_one_utterance(monkeypatch, recognizer)

    async def follow_room():
        await participant.run(room=asyncio.current_task())

    room = asyncio.create_task(follow_room())
    await asyncio.wait_for(recognizer.called.wait(), 5)

    # Act
    room.cancel()
    done, _ = await asyncio.wait([room], timeout=5)
    recognizer.release.set()
    await asyncio.wait([room])

    # Assert: it ended without waiting for the recognizer to answer
    assert done == {room}
    assert room.cancelled()
    assert texts(transcript) == []
