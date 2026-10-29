import json
from dataclasses import dataclass

from moq_system_one_bot.event_timeline import Location

MAX_QUESTIONS = 8
MAX_QUESTION_CHARS = 200
MAX_CHOICE_CHARS = 40
MAX_CHOICES = 9
MAX_TARGET_CHARS = 100


@dataclass(frozen=True)
class ChoiceQuestion:
    question: str
    choices: tuple[str, ...]


@dataclass(frozen=True)
class BoxesQuestion:
    """Asks for a box around every instance of `target` in the picture."""

    target: str


Question = ChoiceQuestion | BoxesQuestion


@dataclass(frozen=True)
class Prompt:
    questions: tuple[Question, ...]
    location: Location


def parse_prompt(payload: bytes, location: Location) -> Prompt | None:
    try:
        body = json.loads(payload)
    except ValueError:
        return None
    questions = body.get("questions") if isinstance(body, dict) else None
    if not isinstance(questions, list) or not 0 < len(questions) <= MAX_QUESTIONS:
        return None
    parsed = [parse_question(question) for question in questions]
    if None in parsed:
        return None
    return Prompt(tuple(parsed), location)


def parse_question(body: object) -> Question | None:
    if not isinstance(body, dict):
        return None
    if body.get("type") == "boxes":
        target = body.get("target")
        if not isinstance(target, str) or not 0 < len(target) <= MAX_TARGET_CHARS:
            return None
        return BoxesQuestion(target)
    if body.get("type") != "choice":
        return None
    question = body.get("question")
    choices = body.get("choices")
    if not isinstance(question, str) or not 0 < len(question) <= MAX_QUESTION_CHARS:
        return None
    if not isinstance(choices, list) or not 2 <= len(choices) <= MAX_CHOICES:
        return None
    if not all(isinstance(choice, str) and 0 < len(choice) <= MAX_CHOICE_CHARS for choice in choices):
        return None
    return ChoiceQuestion(question, tuple(choices))
