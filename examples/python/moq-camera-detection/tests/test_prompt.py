import json

import pytest

from moq_camera_detection.prompt import (
    MAX_CHOICES,
    MAX_QUESTION_CHARS,
    MAX_QUESTIONS,
    MAX_TARGET_CHARS,
    BoxesQuestion,
    ChoiceQuestion,
    Prompt,
    parse_prompt,
)

COLOR = {"type": "choice", "question": "何色ですか？", "choices": ["赤", "青"]}


def test_prompt_is_read_with_its_questions_in_order_and_its_location():
    # Arrange
    payload = json.dumps({"questions": [COLOR, {"type": "boxes", "target": "人"}]}).encode()

    # Act
    prompt = parse_prompt(payload, (5, 0))

    # Assert
    assert prompt == Prompt((ChoiceQuestion("何色ですか？", ("赤", "青")), BoxesQuestion("人")), (5, 0))


@pytest.mark.parametrize(
    "questions",
    [
        [],
        [COLOR] * (MAX_QUESTIONS + 1),
        [{"type": "count", "question": "q"}],
        [{"type": "choice", "question": "", "choices": ["a", "b"]}],
        [{"type": "choice", "question": "q" * (MAX_QUESTION_CHARS + 1), "choices": ["a", "b"]}],
        [{"type": "choice", "question": "q", "choices": ["only"]}],
        [{"type": "choice", "question": "q", "choices": ["c"] * (MAX_CHOICES + 1)}],
        [{"type": "choice", "question": "q", "choices": ["a", ""]}],
        [{"type": "choice", "question": "q", "choices": ["a", 2]}],
        [{"type": "boxes", "target": ""}],
        [{"type": "boxes", "target": "t" * (MAX_TARGET_CHARS + 1)}],
        [COLOR, "not a question"],
    ],
)
def test_prompt_with_a_malformed_question_is_rejected(questions):
    # Act / Assert
    assert parse_prompt(json.dumps({"questions": questions}).encode(), (5, 0)) is None


@pytest.mark.parametrize("payload", [b"not json", b"\xff", b"[]", json.dumps({"question": "q"}).encode()])
def test_malformed_prompt_is_rejected(payload):
    # Act / Assert
    assert parse_prompt(payload, (5, 0)) is None
