import base64
import re

import aiohttp

from moq_camera_detection.identity_token import GcloudIdentityToken
from moq_camera_detection.prompt import Prompt

MODEL = "djev-dgemma"
MAX_ANSWER_TOKENS = 8
LEADING_NUMBER = re.compile(r"\s*(\d+)")


def parse_choice(content: str, prompt: Prompt) -> str | None:
    # DiffusionGemma prefixes the answer with a "thought" line, and vLLM cannot constrain a
    # diffusion model's output to the choices, so the number is read from the last line.
    lines = content.strip().splitlines()
    number = LEADING_NUMBER.match(lines[-1]) if lines else None
    index = int(number[1]) - 1 if number else -1
    return prompt.choices[index] if 0 <= index < len(prompt.choices) else None


class DjevVisionClient:
    def __init__(
        self,
        session: aiohttp.ClientSession,
        url: str,
        identity_token: GcloudIdentityToken | None = None,
    ):
        self._session = session
        self._url = url
        self._identity_token = identity_token

    async def choose(self, jpeg: bytes, prompt: Prompt) -> str | None:
        image_url = "data:image/jpeg;base64," + base64.b64encode(jpeg).decode()
        request = {
            "model": MODEL,
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "image_url", "image_url": {"url": image_url}},
                        {"type": "text", "text": prompt.text()},
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
        async with self._session.post(self._url, json=request, headers=headers) as response:
            response.raise_for_status()
            body = await response.json()
        return parse_choice(body["choices"][0]["message"]["content"], prompt)
