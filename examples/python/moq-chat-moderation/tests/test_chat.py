import json
from types import SimpleNamespace

import pytest

from moq_chat_moderation.chat import MAX_LABELS, ChatMessage, Label


def chat_object(body: object) -> SimpleNamespace:
    return SimpleNamespace(group_id=1790000000000000, object_id=0, payload=json.dumps(body).encode())


def test_chat_object_carries_its_text_and_labels_at_its_location():
    # Arrange
    body = {"text": "こんにちは", "labels": [{"name": "喜", "description": "喜んでいる、うれしい"}]}

    # Act
    chat = ChatMessage.parse(chat_object(body))

    # Assert
    assert (chat.text, chat.labels, chat.location) == (
        "こんにちは",
        (Label("喜", "喜んでいる、うれしい"),),
        (1790000000000000, 0),
    )


def test_chat_without_labels_is_a_chat():
    # Act
    chat = ChatMessage.parse(chat_object({"text": "hello", "labels": []}))

    # Assert
    assert chat.labels == ()


@pytest.mark.parametrize(
    "body",
    [
        "hello",
        {"text": "", "labels": []},
        {"text": "hello"},
        {"text": "hello", "labels": [{"name": "", "description": ""}]},
        {"text": "hello", "labels": [{"name": "a" * 21, "description": ""}]},
        {"text": "hello", "labels": [{"name": "a", "description": "a" * 101}]},
        {"text": "hello", "labels": [{"name": "a", "description": ""}, {"name": "a", "description": "b"}]},
        {"text": "hello", "labels": [{"name": f"l{index}", "description": ""} for index in range(MAX_LABELS + 1)]},
    ],
)
def test_invalid_chat_is_rejected(body):
    # Act / Assert
    with pytest.raises(ValueError):
        ChatMessage.parse(chat_object(body))


def test_object_without_utf8_text_is_rejected():
    # Act / Assert
    with pytest.raises(ValueError):
        ChatMessage.parse(SimpleNamespace(group_id=1, object_id=0, payload=b"\xff\xfe"))
