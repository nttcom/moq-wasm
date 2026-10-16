import base64
import enum
from dataclasses import dataclass

import aiohttp

from moq_ptz_tracking.d3_flash_status import D3FlashStatus
from moq_ptz_tracking.identity_token import IdentityToken

COLUMNS = ("far left", "left", "center", "right", "far right")
ROWS = ("top", "upper middle", "middle", "lower middle", "bottom")
VISIBLE_THRESHOLD = 0.5


@dataclass(frozen=True)
class Position:
    """Fractions of the picture's width and height, from its top-left corner."""

    x: float
    y: float


class Answer(enum.Enum):
    NOT_VISIBLE = "not visible"


def cell_key(row: int, column: int) -> str:
    return f"r{row}c{column}"


def visible_question(description: str) -> dict:
    return {"type": "noul", "instructions": f"Is {description} in this picture?"}


def locate_question(description: str) -> dict:
    """Asks only where: offering "not in the picture" among the 25 cells let an absent target's spread
    probability outweigh that option."""
    return {
        "type": "choice",
        "instructions": f"Where is {description} in this picture?",
        "criteria": {
            cell_key(row, column): f"Its center is in the {row_name} row, {column_name} column"
            for row, row_name in enumerate(ROWS)
            for column, column_name in enumerate(COLUMNS)
        },
    }


def read_position(probabilities: dict[str, float]) -> Position:
    cells = [
        (probabilities[cell_key(row, column)], (column + 0.5) / len(COLUMNS), (row + 0.5) / len(ROWS))
        for row in range(len(ROWS))
        for column in range(len(COLUMNS))
    ]
    total = sum(p for p, _, _ in cells)
    return Position(sum(p * x for p, x, _ in cells) / total, sum(p * y for p, _, y in cells) / total)


class D3FlashVisionClient:
    def __init__(
        self,
        session: aiohttp.ClientSession,
        url: str,
        status: D3FlashStatus,
        identity_token: IdentityToken | None = None,
    ):
        self._session = session
        self._url = url
        self._status = status
        self._identity_token = identity_token

    async def locate(self, jpeg: bytes, description: str) -> Position | Answer:
        """Asks visibility first and location only when visible, so a search sweep costs one question
        per picture."""
        image = "data:image/jpeg;base64," + base64.b64encode(jpeg).decode()
        visible = await self._ask(image, "A camera picture.", "visible", visible_question(description))
        if visible["noul"] < VISIBLE_THRESHOLD:
            return Answer.NOT_VISIBLE
        state = f"A PTZ camera picture divided into a {len(ROWS)} by {len(COLUMNS)} grid."
        where = await self._ask(image, state, "where", locate_question(description))
        return read_position(where["probabilities"])

    async def _ask(self, image: str, state: str, question_id: str, question: dict) -> dict:
        request = {"state": state, "images": [image], "questions": {question_id: question}}
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
        return body["answers"][question_id]
