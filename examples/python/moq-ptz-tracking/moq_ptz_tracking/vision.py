import base64
import enum
from dataclasses import dataclass

import aiohttp
from moq_bot_rooms.identity_token import IdentityToken
from moq_system_one_bot.system_one_status import SystemOneStatus

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


class SystemOneVisionClient:
    def __init__(
        self,
        session: aiohttp.ClientSession,
        url: str,
        status: SystemOneStatus,
        identity_token: IdentityToken | None = None,
    ):
        self._session = session
        self._url = url
        self._status = status
        self._identity_token = identity_token

    async def locate(self, jpeg: bytes, descriptions: tuple[str, ...]) -> Sighting | Answer:
        """Asks for every instance of every description in one request, so more descriptions cost one
        forward pass, not one request each."""
        request = {
            "images": ["data:image/jpeg;base64," + base64.b64encode(jpeg).decode()],
            "questions": {question_id(index): boxes_question(text) for index, text in enumerate(descriptions)},
        }
        headers = (
            {"Authorization": f"Bearer {await self._identity_token.value()}"}
            if self._identity_token
            else {}
        )
        async with (
            self._status.request(),
            self._session.post(self._url, json=request, headers=headers) as response,
        ):
            response.raise_for_status()
            answers = (await response.json())["answers"]
        boxes = tuple(
            TargetBox(index, Box(**found["box"]))
            for index in range(len(descriptions))
            for found in answers[question_id(index)]["boxes"]
        )
        return Sighting(boxes) if boxes else Answer.NOT_VISIBLE
