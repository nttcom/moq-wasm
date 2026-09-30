import pytest

from moq_chat_moderation.chat import ChatMessage


def test_chat_record_carries_its_text_and_location():
    # Act
    chat = ChatMessage.parse({"text": "hello", "location": [1790000000000000, 0]})

    # Assert
    assert chat == ChatMessage(text="hello", location=(1790000000000000, 0))


@pytest.mark.parametrize(
    "message",
    [
        "hello",
        {"location": [1, 0]},
        {"text": "hello"},
        {"text": "hello", "location": [1]},
        {"text": "hello", "location": [True, 0]},
        {"text": "hello", "location": ["1", 0]},
    ],
)
def test_message_without_text_and_location_is_not_a_chat(message):
    # Act / Assert
    assert ChatMessage.parse(message) is None
