import aiohttp

from moq_chat_moderation.d3_flash_status import D3FlashStatus
from moq_chat_moderation.identity_token import IdentityToken

ABUSIVE_QUESTION_ID = "abusive"
ABUSIVE_QUESTION = {
    "type": "noul",
    "instructions": "Is this chat message abusive?",
    "criteria": {
        "true": "Insults, harasses, threatens or demeans someone",
        "false": "Ordinary conversation, including mild disagreement",
    },
}


class D3FlashClient:
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

    async def abusive_probability(self, text: str) -> float:
        request = {"state": text, "questions": {ABUSIVE_QUESTION_ID: ABUSIVE_QUESTION}}
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
        return float(body["answers"][ABUSIVE_QUESTION_ID]["noul"])
