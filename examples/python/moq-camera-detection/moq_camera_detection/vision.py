import base64

import aiohttp

from moq_camera_detection.djev_status import DjevStatus
from moq_camera_detection.identity_token import IdentityToken
from moq_camera_detection.prompt import Prompt

QUESTION_ID = "answer"


def choice_question(prompt: Prompt) -> dict:
    return {
        "type": "choice",
        "instructions": prompt.question,
        "criteria": {choice: None for choice in prompt.choices},
    }


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

    async def choose(self, jpeg: bytes, prompt: Prompt) -> str | None:
        request = {
            "state": "A camera picture.",
            "images": ["data:image/jpeg;base64," + base64.b64encode(jpeg).decode()],
            "questions": {QUESTION_ID: choice_question(prompt)},
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
        choice = body["answers"][QUESTION_ID]["choice"]
        return choice if choice in prompt.choices else None
