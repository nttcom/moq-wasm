import base64
import re

import aiohttp

from moq_ptz_tracking.djev_status import DjevStatus
from moq_ptz_tracking.identity_token import IdentityToken

MODEL = "djev-dgemma"
MAX_ANSWER_TOKENS = 8
LEADING_NUMBER = re.compile(r"\s*(\d+)")
POSITIONS = (
    "top left",
    "top center",
    "top right",
    "middle left",
    "center",
    "middle right",
    "bottom left",
    "bottom center",
    "bottom right",
)
NOT_VISIBLE = "not in the picture"
CHOICES = (*POSITIONS, NOT_VISIBLE)


def locate_question(description: str) -> str:
    numbered = "\n".join(f"{number}. {choice}" for number, choice in enumerate(CHOICES, 1))
    return (
        f"Where is {description} in this picture?\n"
        f"Answer with only the number of one of these choices:\n{numbered}"
    )


def parse_position(content: str) -> str | None:
    # DiffusionGemma prefixes the answer with a "thought" line, and vLLM cannot constrain a
    # diffusion model's output to the choices, so the number is read from the last line.
    lines = content.strip().splitlines()
    number = LEADING_NUMBER.match(lines[-1]) if lines else None
    index = int(number[1]) - 1 if number else -1
    return CHOICES[index] if 0 <= index < len(CHOICES) else None


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

    async def locate(self, jpeg: bytes, description: str) -> str | None:
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
