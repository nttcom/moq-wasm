import asyncio

import pytest

from moq_bot_rooms.rooms import BotRoom

from moq_meeting_transcriber import room as room_module
from moq_meeting_transcriber.room import BOT_NAMESPACE, Room, participant_name
from tests.helpers import NamedRecognizer, RecordingPublication, SessionRefusingNamespaces, recorded_timeline

ROOM = BotRoom("team a")


def test_one_element_under_the_room_is_a_participant():
    # Act / Assert
    assert participant_name(["anon", ROOM.room_id, "alice"], ROOM) == "alice"


def test_a_participant_of_a_temporary_app_is_under_its_root():
    # Act / Assert
    assert participant_name(["temp-0123", "team a", "alice"], BotRoom("team a", "temp-0123")) == "alice"


@pytest.mark.parametrize(
    "namespace",
    [
        ["anon", ROOM.room_id, *BOT_NAMESPACE],
        ["anon", "other", "alice"],
        ["app-1", ROOM.room_id, "alice"],
        ["anon", ROOM.room_id],
    ],
)
def test_other_namespaces_are_not_participants(namespace):
    # Act / Assert
    assert participant_name(namespace, ROOM) is None


async def follow_participants(room: Room, session) -> tuple[asyncio.Task, RecordingPublication, RecordingPublication]:
    timeline, transcript = await recorded_timeline()
    transcript.writes.clear()
    status = RecordingPublication()
    return asyncio.create_task(room._follow_participants(session, timeline, status)), transcript, status


async def test_a_failure_to_follow_the_participants_ends_the_room():
    # Arrange
    room = Room("https://relay.invalid", True, NamedRecognizer("whisper"), ROOM)

    # Act
    following, _, _ = await follow_participants(room, SessionRefusingNamespaces())

    # Assert
    with pytest.raises(RuntimeError, match="SUBSCRIBE_NAMESPACE refused"):
        await following


async def test_a_recognizer_that_is_starting_gets_a_notice_and_no_audio_until_it_answers(monkeypatch):
    # Arrange
    monkeypatch.setattr(room_module, "STARTING_NOTICE_AFTER_SECONDS", 0.01)
    started = asyncio.Event()
    recognizer = NamedRecognizer("qwen3-asr")
    recognizer.ready = started.wait
    following, transcript, status = await follow_participants(Room("https://relay.invalid", True, recognizer, ROOM), SessionRefusingNamespaces())

    # Act
    await asyncio.sleep(0.1)
    before_ready = list(transcript.writes)
    still_waiting = not following.done()
    started.set()

    # Assert: only the notice is written before it answers; the refused SUBSCRIBE_NAMESPACE comes after
    assert still_waiting
    assert [record["data"]["speaker"] for _, records in before_ready for record in records] == ["transcriber"]
    with pytest.raises(RuntimeError, match="SUBSCRIBE_NAMESPACE refused"):
        await following
    assert status.writes == [(True, {"asr": "qwen3-asr", "state": "ready"})]


async def test_a_recognizer_that_answers_at_once_gets_no_notice():
    # Arrange
    following, transcript, _ = await follow_participants(
        Room("https://relay.invalid", True, NamedRecognizer("whisper"), ROOM), SessionRefusingNamespaces()
    )

    # Act
    with pytest.raises(RuntimeError):
        await following

    # Assert
    assert transcript.writes == []
