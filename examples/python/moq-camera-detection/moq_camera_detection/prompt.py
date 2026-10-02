import json
from dataclasses import dataclass

from moq_camera_detection.event_timeline import Location

MAX_QUESTION_CHARS = 200
MAX_CHOICE_CHARS = 40
MAX_CHOICES = 9


@dataclass(frozen=True)
class Prompt:
    question: str
    choices: tuple[str, ...]
    location: Location

    def text(self) -> str:
        numbered = "\n".join(f"{number}. {choice}" for number, choice in enumerate(self.choices, 1))
        return f"{self.question}\nAnswer with only the number of one of these choices:\n{numbered}"


def parse_prompt(payload: bytes, location: Location) -> Prompt | None:
    try:
        body = json.loads(payload)
    except ValueError:
        return None
    if not isinstance(body, dict):
        return None
    question = body.get("question")
    choices = body.get("choices")
    if not isinstance(question, str) or not 0 < len(question) <= MAX_QUESTION_CHARS:
        return None
    if not isinstance(choices, list) or not 2 <= len(choices) <= MAX_CHOICES:
        return None
    if not all(isinstance(choice, str) and 0 < len(choice) <= MAX_CHOICE_CHARS for choice in choices):
        return None
    return Prompt(question, tuple(choices), location)
