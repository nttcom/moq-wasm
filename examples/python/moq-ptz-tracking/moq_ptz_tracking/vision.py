import base64
import enum
import re
from dataclasses import dataclass

import aiohttp

from moq_ptz_tracking.djev_status import DjevStatus
from moq_ptz_tracking.identity_token import IdentityToken

MODEL = "djev-dgemma"
MAX_ANSWER_TOKENS = 16
COORDINATE_SCALE = 1000
COORDINATES = re.compile(r"(\d+)\D+(\d+)")


@dataclass(frozen=True)
class Position:
    """Fractions of the picture's width and height, from its top-left corner."""

    x: float
    y: float


class Answer(enum.Enum):
    NOT_VISIBLE = "not visible"
    UNREADABLE = "unreadable"


def locate_question(description: str) -> str:
    return (
        f"Where is the center of {description} in this picture?\n"
        f"Answer with only two numbers from 0 to {COORDINATE_SCALE} separated by a space: how far across from the"
        f" left edge (0) to the right edge ({COORDINATE_SCALE}), and how far down from the top edge (0) to the"
        f" bottom edge ({COORDINATE_SCALE}). If {description} is not in the picture, answer none."
    )


def parse_position(content: str) -> Position | Answer:
    # DiffusionGemma prefixes the answer with a "thought" line, so the answer is read from the last line.
    lines = content.strip().splitlines()
    last = lines[-1] if lines else ""
    if "none" in last.lower():
        return Answer.NOT_VISIBLE
    numbers = COORDINATES.search(last)
    if numbers is None:
        return Answer.UNREADABLE
    x, y = int(numbers[1]), int(numbers[2])
    if x > COORDINATE_SCALE or y > COORDINATE_SCALE:
        return Answer.UNREADABLE
    return Position(x / COORDINATE_SCALE, y / COORDINATE_SCALE)


class DjevVisionClient:
    def __init__(
        self,
        session: aiohttp.ClientSession,
        url: str,
        status: DjevStatus,
        identity_token: IdentityToken | None = None,
    ):
        self._session = session
        self._url = url
        self._status = status
        self._identity_token = identity_token

    async def locate(self, jpeg: bytes, description: str) -> Position | Answer:
        image_url = "data:image/jpeg;base64," + base64.b64encode(jpeg).decode()
        request = {
            "model": MODEL,
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "image_url", "image_url": {"url": image_url}},
                        {"type": "text", "text": locate_question(description)},
                    ],
                }
            ],
            "max_tokens": MAX_ANSWER_TOKENS,
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
            body = await response.json()
        return parse_position(body["choices"][0]["message"]["content"])
