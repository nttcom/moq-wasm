import base64
import enum
from dataclasses import dataclass

from moq_system_one_bot import system_one_client

MAX_BOXES_PER_TARGET = 8


@dataclass(frozen=True)
class Position:
    """Fractions of the picture's width and height, from its top-left corner."""

    x: float
    y: float


@dataclass(frozen=True)
class Box:
    """Fractions of the picture's width and height, from its top-left corner."""

    left: float
    top: float
    right: float
    bottom: float

    @property
    def center(self) -> Position:
        return Position((self.left + self.right) / 2, (self.top + self.bottom) / 2)


@dataclass(frozen=True)
class TargetBox:
    """`target` is the index of the description in `Target.descriptions`."""

    target: int
    box: Box


@dataclass(frozen=True)
class Sighting:
    """Every box found in one picture, for any of the target's descriptions; never empty."""

    boxes: tuple[TargetBox, ...]

    def union(self) -> Box:
        return Box(
            min(found.box.left for found in self.boxes),
            min(found.box.top for found in self.boxes),
            max(found.box.right for found in self.boxes),
            max(found.box.bottom for found in self.boxes),
        )


class Answer(enum.Enum):
    NOT_VISIBLE = "not visible"


def question_id(target: int) -> str:
    return f"t{target}"


def boxes_question(description: str) -> dict:
    return {"type": "boxes", "instructions": f"Every {description}.", "max": MAX_BOXES_PER_TARGET}


class SystemOneVisionClient(system_one_client.SystemOneClient):
    async def locate(self, jpeg: bytes, descriptions: tuple[str, ...]) -> Sighting | Answer:
        """Asks for every instance of every description in one request, so more descriptions cost one
        forward pass, not one request each."""
        request = {
            "images": ["data:image/jpeg;base64," + base64.b64encode(jpeg).decode()],
            "questions": {question_id(index): boxes_question(text) for index, text in enumerate(descriptions)},
        }
        answers = await self.ask(request)
        boxes = tuple(
            TargetBox(index, Box(**found["box"]))
            for index in range(len(descriptions))
            for found in answers[question_id(index)]["boxes"]
        )
        return Sighting(boxes) if boxes else Answer.NOT_VISIBLE
