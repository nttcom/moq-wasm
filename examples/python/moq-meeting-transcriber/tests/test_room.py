import asyncio

import pytest

from moq_meeting_transcriber import room as room_module
from moq_meeting_transcriber.room import BOT_NAMESPACE, Room, participant_name
from tests.helpers import NamedRecognizer, RecordingPublication, SessionRefusingNamespaces, recorded_timeline

ROOM = ["app-1", "team a"]


def test_one_element_under_the_room_is_a_participant():
    # Act / Assert
    assert participant_name([*ROOM, "alice"], ROOM) == "alice"


@pytest.mark.parametrize(
    "namespace",
    [
        [*ROOM, *BOT_NAMESPACE],
        ["app-1", "other", "alice"],
        ["app-2", "team a", "alice"],
        ROOM,
    ],
)
def test_other_namespaces_are_not_participants(namespace):
    # Act / Assert
    assert participant_name(namespace, ROOM) is None


async def refuse_connection():
    raise RuntimeError("no relay")


async def follow_participants(room: Room, session) -> tuple[asyncio.Task, RecordingPublication, RecordingPublication]:
    timeline, transcript = await recorded_timeline()
    transcript.writes.clear()
    status = RecordingPublication()
    return asyncio.create_task(room._follow_participants(session, timeline, status)), transcript, status


async def test_a_failure_to_follow_the_participants_ends_the_room():
    # Arrange
    room = Room(refuse_connection, ROOM, NamedRecognizer("whisper"), 0.1)

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
    following, transcript, status = await follow_participants(Room(refuse_connection, ROOM, recognizer, 0.1), SessionRefusingNamespaces())

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
        Room(refuse_connection, ROOM, NamedRecognizer("whisper"), 0.1), SessionRefusingNamespaces()
    )

    # Act
    with pytest.raises(RuntimeError):
        await following

    # Assert
    assert transcript.writes == []
