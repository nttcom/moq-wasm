import asyncio
import base64
import json
import logging
import time
from collections.abc import Awaitable, Callable
from dataclasses import dataclass

logger = logging.getLogger("moq_wasm")

# A token is refreshed this long before it expires, as the pages do (examples/browser/utils/access.ts).
REFRESH_MARGIN_SECONDS = 5 * 60
RETRY_SECONDS = 30.0

RefreshToken = Callable[[str], Awaitable[str]]


@dataclass(frozen=True)
class TokenClaims:
    app_id: str
    # Unix seconds; None for a token that does not expire.
    expires_at: float | None


def read_claims(token: str) -> TokenClaims:
    """Reads `app_id` and `exp` from a JWT without verifying it: the relay verifies the token, and
    the session only needs to know when to refresh it. Raises ValueError for what is not a JWT."""
    try:
        payload = token.split(".")[1]
        claims = json.loads(base64.urlsafe_b64decode(payload + "=" * (-len(payload) % 4)))
        return TokenClaims(app_id=claims["app_id"], expires_at=claims.get("exp"))
    except (IndexError, KeyError, TypeError, ValueError) as error:
        raise ValueError(f"not a token with an app_id claim: {error}") from error


class TokenExpired(Exception):
    """The session's token expired before `refresh_token` returned a later one."""


class TokenKeeper:
    def __init__(
        self,
        token: str,
        refresh: RefreshToken,
        *,
        retry_seconds: float = RETRY_SECONDS,
        now: Callable[[], float] = time.time,
    ):
        self.token = token
        self._claims = read_claims(token)
        self._refresh = refresh
        self._retry_seconds = retry_seconds
        self._now = now

    @property
    def app_id(self) -> str:
        return self._claims.app_id

    async def keep_fresh(self, hand_over: Callable[[str], Awaitable[None]]) -> None:
        """Refreshes the token `REFRESH_MARGIN_SECONDS` before it expires and calls `hand_over` with
        each new one, until cancelled. Returns at once for a token that does not expire. Raises
        `TokenExpired` once the token expired while `refresh` kept failing."""
        while self._claims.expires_at is not None:
            await asyncio.sleep(max(self._claims.expires_at - REFRESH_MARGIN_SECONDS - self._now(), 0))
            await self._renew()
            await hand_over(self.token)

    async def _renew(self) -> None:
        while True:
            try:
                token = await self._refresh(self.token)
                self._claims = read_claims(token)
                self.token = token
                return
            except Exception as error:
                expires_at = self._claims.expires_at
                if expires_at is not None and self._now() + self._retry_seconds >= expires_at:
                    raise TokenExpired(f"app {self.app_id}: the token expired: {error!r}") from error
                logger.warning("app %s: cannot refresh the token yet: %r", self.app_id, error)
                await asyncio.sleep(self._retry_seconds)
