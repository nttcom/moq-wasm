from types import SimpleNamespace

import pytest

from moq_chat_moderation.chat import ChatMessage


def test_chat_object_carries_its_text_at_its_location():
    # Arrange
    chat_object = SimpleNamespace(group_id=1790000000000000, object_id=0, payload="こんにちは".encode())

    # Act
    chat = ChatMessage.parse(chat_object)

    # Assert
    assert (chat.text, chat.location) == ("こんにちは", (1790000000000000, 0))


@pytest.mark.parametrize("payload", [b"", b"\xff\xfe"])
def test_object_without_utf8_text_is_not_a_chat(payload):
    # Act / Assert
    assert ChatMessage.parse(SimpleNamespace(group_id=1, object_id=0, payload=payload)) is None
