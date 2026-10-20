import pytest

from moq_meeting_transcriber.room import BOT_NAMESPACE, Room, participant_name
from tests.helpers import NamedRecognizer, SessionRefusingNamespaces

ROOM = "team a"


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
