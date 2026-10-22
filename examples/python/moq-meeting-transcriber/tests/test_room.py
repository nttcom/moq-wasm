import asyncio

import pytest

from moq_meeting_transcriber import room as room_module
from moq_meeting_transcriber.room import BOT_NAMESPACE, Room, participant_name
from tests.helpers import NamedRecognizer, SessionRefusingNamespaces, SessionWithParticipants

ROOM = "team a"
SHORT_GRACE_SECONDS = 0.1


def test_one_element_under_the_room_is_a_participant():
    # Act / Assert
    assert participant_name(["anon", ROOM, "alice"], ROOM) == "alice"


@pytest.mark.parametrize(
    "namespace",
    [["anon", ROOM, *BOT_NAMESPACE], ["anon", "other", "alice"], ["app-1", ROOM, "alice"], ["anon", ROOM]],
)
def test_other_namespaces_are_not_participants(namespace):
    # Act / Assert
    assert participant_name(namespace, ROOM) is None


async def test_a_failure_to_follow_the_participants_ends_the_room():
    # Arrange
    room = Room("https://relay.invalid", True, NamedRecognizer("whisper"), ROOM)

    # Act / Assert
    with pytest.raises(RuntimeError, match="SUBSCRIBE_NAMESPACE refused"):
        await room._transcribe(SessionRefusingNamespaces())


async def test_a_recognizer_that_is_starting_gets_a_notice_and_no_audio_until_it_answers(monkeypatch):
    # Arrange
    monkeypatch.setattr(room_module, "STARTING_NOTICE_AFTER_SECONDS", 0.01)
    started = asyncio.Event()
    recognizer = NamedRecognizer("qwen3-asr")
    recognizer.ready = started.wait
    session = SessionRefusingNamespaces()
    transcribing = asyncio.create_task(Room("https://relay.invalid", True, recognizer, ROOM)._transcribe(session))

    # Act
    await asyncio.sleep(0.1)
    before_ready = list(session.publications["transcript"].writes)
    still_waiting = not transcribing.done()
    started.set()

    # Assert: only the notice is written before it answers; the refused SUBSCRIBE_NAMESPACE comes after
    assert still_waiting
    assert [record["data"]["speaker"] for _, records in before_ready for record in records] == ["transcriber"]
    with pytest.raises(RuntimeError, match="SUBSCRIBE_NAMESPACE refused"):
        await transcribing


async def test_a_recognizer_that_answers_at_once_gets_no_notice():
    # Arrange
    session = SessionRefusingNamespaces()

    # Act
    with pytest.raises(RuntimeError):
        await Room("https://relay.invalid", True, NamedRecognizer("whisper"), ROOM)._transcribe(session)

    # Assert
    assert session.publications["transcript"].writes == [(True, [])]


async def test_a_room_nobody_is_in_ends_after_the_grace_period(monkeypatch):
    # Arrange
    monkeypatch.setattr(room_module, "GRACE_SECONDS", SHORT_GRACE_SECONDS)
    session = SessionWithParticipants()

    # Act
    room = Room("https://relay.invalid", True, NamedRecognizer("whisper"), ROOM)
    empty = await asyncio.wait_for(room._transcribe(session), 1)

    # Assert
    assert empty


async def test_a_room_ends_once_its_last_participant_has_been_gone_for_the_grace_period(monkeypatch):
    # Arrange
    monkeypatch.setattr(room_module, "GRACE_SECONDS", SHORT_GRACE_SECONDS)
    session = SessionWithParticipants()
    session.publish(["anon", ROOM, "alice"])
    transcribing = asyncio.create_task(
        Room("https://relay.invalid", True, NamedRecognizer("whisper"), ROOM)._transcribe(session)
    )
    await asyncio.sleep(SHORT_GRACE_SECONDS * 3)
    held = not transcribing.done()

    # Act
    session.publish(["anon", ROOM, "alice"], active=False)
    empty = await asyncio.wait_for(transcribing, 1)

    # Assert
    assert held
    assert empty
