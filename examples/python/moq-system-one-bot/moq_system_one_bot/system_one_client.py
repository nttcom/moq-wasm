import aiohttp
from moq_bot_rooms.identity_token import IdentityToken

from moq_system_one_bot.system_one_status import SystemOneStatus

# The System One service scales to zero; a cold start copies its weights before it answers.
REQUEST_TIMEOUT = aiohttp.ClientTimeout(total=10 * 60)


class SystemOneClient:
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

    async def ask(self, request: dict) -> dict:
        """Returns the answers keyed by question id."""
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
            return (await response.json())["answers"]
