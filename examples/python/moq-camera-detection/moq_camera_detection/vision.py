import base64

import aiohttp
from moq_bot_rooms.identity_token import IdentityToken
from moq_system_one_bot.system_one_status import SystemOneStatus

from moq_camera_detection.prompt import BoxesQuestion, ChoiceQuestion, Prompt, Question

MAX_BOXES = 8

# The choice picked, None when it is not one of the prompt's choices, or each box found as
# [left, top, right, bottom] fractions of the picture.
Answer = str | None | list[list[float]]


def question_id(index: int) -> str:
    return f"q{index}"


def system_one_question(question: Question) -> dict:
    if isinstance(question, BoxesQuestion):
        return {"type": "boxes", "instructions": f"Every {question.target}.", "max": MAX_BOXES}
    return {
        "type": "choice",
        "instructions": question.question,
        "criteria": {choice: None for choice in question.choices},
    }


def read_answer(question: Question, answer: dict) -> Answer:
    if isinstance(question, ChoiceQuestion):
        choice = answer["choice"]
        return choice if choice in question.choices else None
    return [
        [round(found["box"][edge], 3) for edge in ("left", "top", "right", "bottom")] for found in answer["boxes"]
    ]


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

    async def answer(self, jpeg: bytes, prompt: Prompt) -> list[Answer]:
        """Asks every question of the prompt in one request; the answers follow the questions' order."""
        request = {
            "state": "A camera picture.",
            "images": ["data:image/jpeg;base64," + base64.b64encode(jpeg).decode()],
            "questions": {question_id(index): system_one_question(q) for index, q in enumerate(prompt.questions)},
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
        return [read_answer(q, answers[question_id(index)]) for index, q in enumerate(prompt.questions)]
