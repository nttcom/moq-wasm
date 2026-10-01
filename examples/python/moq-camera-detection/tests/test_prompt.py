import json

import pytest

from moq_camera_detection.prompt import MAX_CHOICES, MAX_QUESTION_CHARS, Prompt, parse_prompt


def test_prompt_is_read_with_its_location():
    # Arrange
    payload = json.dumps({"question": "何色ですか？", "choices": ["赤", "青"]}).encode()

    # Act
    prompt = parse_prompt(payload, (5, 0))

    # Assert
    assert prompt == Prompt("何色ですか？", ("赤", "青"), (5, 0))


@pytest.mark.parametrize(
    "payload",
    [
        b"not json",
        b"\xff",
        b"[]",
        json.dumps({"question": "", "choices": ["a", "b"]}).encode(),
        json.dumps({"question": "q" * (MAX_QUESTION_CHARS + 1), "choices": ["a", "b"]}).encode(),
        json.dumps({"question": "q", "choices": ["only"]}).encode(),
        json.dumps({"question": "q", "choices": ["c"] * (MAX_CHOICES + 1)}).encode(),
        json.dumps({"question": "q", "choices": ["a", ""]}).encode(),
        json.dumps({"question": "q", "choices": ["a", 2]}).encode(),
    ],
)
def test_malformed_prompt_is_rejected(payload):
    # Act / Assert
    assert parse_prompt(payload, (5, 0)) is None
