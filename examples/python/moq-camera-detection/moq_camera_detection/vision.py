import base64

import aiohttp

from moq_camera_detection.identity_token import GcloudIdentityToken

MODEL = "djev-dgemma"
QUESTION = "Is there a person in this image? Answer with exactly one of: person, no_person."
ANSWERS = {"person": True, "no_person": False}
MAX_ANSWER_TOKENS = 8


def parse_answer(content: str) -> bool | None:
    # DiffusionGemma prefixes the answer with a "thought" line, and vLLM cannot constrain a
    # diffusion model's output to the choices, so the last line is matched against them.
    lines = content.strip().splitlines()
    return ANSWERS.get(lines[-1].strip()) if lines else None


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

    async def has_person(self, jpeg: bytes) -> bool | None:
        image_url = "data:image/jpeg;base64," + base64.b64encode(jpeg).decode()
        request = {
            "model": MODEL,
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "image_url", "image_url": {"url": image_url}},
                        {"type": "text", "text": QUESTION},
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
        return parse_answer(body["choices"][0]["message"]["content"])
