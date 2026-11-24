import asyncio
from types import SimpleNamespace
from typing import cast

import pytest
from moq_wasm.media import RemoteBroadcast

from moq_meeting_transcriber.participant import Participant
from tests.helpers import HeldRecognizer, hear_utterances, recorded_timeline, texts

BROADCAST = cast(RemoteBroadcast, SimpleNamespace(namespace=["app-1", "team a", "alice"]))


async def participant_hearing(monkeypatch, recognizer: HeldRecognizer, utterances: int = 1):
    hear_utterances(monkeypatch, utterances)
    timeline, transcript = await recorded_timeline()
    return Participant(BROADCAST, recognizer, timeline), transcript


async def leave_while_the_recognizer_is_busy(participant: Participant, recognizer: HeldRecognizer) -> set[asyncio.Task]:
    """Withdraws the participant while the recognizer works on their first utterance, which it
    answers only after a pipeline being cancelled would have ended, and returns the pipeline the
    participant left once it is unsubscribed."""
    departed: set[asyncio.Task] = set()
    running = asyncio.create_task(participant.run(asyncio.Event(), departed))
    await asyncio.wait_for(recognizer.called.wait(), 5)
    running.cancel()
    await asyncio.wait([running], timeout=0.5)
    pipelines = set(departed)
    recognizer.release.set()
    with pytest.raises(asyncio.CancelledError):
        await asyncio.wait_for(running, 5)
    return pipelines


async def test_a_participant_who_leaves_still_gets_the_utterance_already_heard(monkeypatch):
    # Arrange
    recognizer = HeldRecognizer("こんにちは")
    participant, transcript = await participant_hearing(monkeypatch, recognizer)

    # Act
    pipelines = await leave_while_the_recognizer_is_busy(participant, recognizer)
    await asyncio.wait_for(asyncio.wait(pipelines), 5)

    # Assert
    assert texts(transcript) == ["こんにちは"]


async def test_a_participant_who_leaves_still_gets_each_utterance_heard_while_the_recognizer_was_busy(monkeypatch):
    # Arrange: the second and third utterances close while the recognizer works on the first
    recognizer = HeldRecognizer("一", "二", "三")
    participant, transcript = await participant_hearing(monkeypatch, recognizer, utterances=3)

    # Act
    pipelines = await leave_while_the_recognizer_is_busy(participant, recognizer)
    await asyncio.wait_for(asyncio.wait(pipelines), 5)

    # Assert
    assert texts(transcript) == ["一", "二", "三"]
