"""The token of the temporary app a page runs in, which the bot keeps alive on the token mint
(services/token-mint) for as long as its room runs (docs/architecture/ai-bots/on-demand-rooms.md
"Temporary apps")."""

import asyncio
import time
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from typing import Protocol, TypeVar
from urllib.parse import urlsplit

import aiohttp
from loguru import logger

LOCAL_TOKEN_MINT_URL = "http://localhost:8083"
CLOUD_TOKEN_MINT_URL = "https://token-mint.moqt.research.skyway.io"
LOCAL_HOSTS = {"localhost", "127.0.0.1", "::1"}
REQUEST_TIMEOUT = aiohttp.ClientTimeout(total=10)
# A token is refreshed this long before it expires, as the pages do (examples/browser/utils/access.ts).
REFRESH_MARGIN_SECONDS = 5 * 60
RETRY_SECONDS = 30.0

T = TypeVar("T")


class TokenRejected(Exception):
    """The token mint refused the token: it did not sign it, or it has expired."""


class TokenMintUnavailable(Exception):
    """The token mint did not answer, or answered something other than a token or a refusal."""


def token_mint_url(configured: str | None, relay_url: str) -> str:
    """`configured`, else the issuer next to the relay: the local one for a local relay."""
    if configured:
        return configured.rstrip("/")
    return LOCAL_TOKEN_MINT_URL if urlsplit(relay_url).hostname in LOCAL_HOSTS else CLOUD_TOKEN_MINT_URL


class TokenMint:
    def __init__(self, http: aiohttp.ClientSession, url: str):
        self._http = http
        self._url = url

    async def refresh(self, token: str) -> "IssuedToken":
        """A new token of `token`'s app (`POST /tokens/refresh`). Refreshing the token a page handed
        over is how the bot, without the mint's secret, knows the token is valid and which app it
        belongs to. Raises `TokenRejected` or `TokenMintUnavailable`."""
        try:
            async with self._http.post(
                f"{self._url}/tokens/refresh", json={"token": token}, timeout=REQUEST_TIMEOUT
            ) as response:
                answer = await response.json(content_type=None)
                if response.status == 401:
                    raise TokenRejected(answer.get("error", "rejected"))
                response.raise_for_status()
                return IssuedToken(str(answer["token"]), str(answer["appId"]), float(answer["expiresAt"]))
        except (aiohttp.ClientError, TimeoutError, ValueError, KeyError, TypeError, AttributeError) as error:
            raise TokenMintUnavailable(f"{self._url}: {error!r}") from error


@dataclass(frozen=True)
class IssuedToken:
    token: str
    app_id: str
    # Unix seconds.
    expires_at: float


class TokenRefreshingSession(Protocol):
    """The part of `moq_wasm.Session` that takes a new token."""

    def refresh_token(self, token: str, *, app_id: str) -> Awaitable[None]: ...


class AppToken:
    """A temporary app's current token, refreshed on the issuer before it expires."""

    def __init__(
        self,
        issuer: TokenMint,
        issued: IssuedToken,
        *,
        retry_seconds: float = RETRY_SECONDS,
        now: Callable[[], float] = time.time,
    ):
        self._issuer = issuer
        self._issued = issued
        self._retry_seconds = retry_seconds
        self._now = now

    @property
    def app_id(self) -> str:
        return self._issued.app_id

    @property
    def token(self) -> str:
        return self._issued.token

    async def keep_fresh(self, session: TokenRefreshingSession) -> None:
        """Refreshes the token `REFRESH_MARGIN_SECONDS` before it expires and hands each new one to
        `session`, until cancelled. Raises `TokenRejected` once the issuer refuses a refresh, and
        `TokenMintUnavailable` once it has not answered until the token expired."""
        while True:
            await asyncio.sleep(max(self._issued.expires_at - REFRESH_MARGIN_SECONDS - self._now(), 0))
            await self._refresh()
            try:
                await session.refresh_token(self.token, app_id=self.app_id)
            except RuntimeError as error:
                # The session expires with its current token and is opened again with this one.
                logger.warning(f"app {self.app_id}: the relay refused the refreshed token: {error}")

    async def _refresh(self) -> None:
        while True:
            try:
                self._issued = await self._issuer.refresh(self.token)
                return
            except TokenMintUnavailable as error:
                if self._now() + self._retry_seconds >= self._issued.expires_at:
                    raise
                logger.warning(f"app {self.app_id}: cannot refresh the token yet: {error}")
                await asyncio.sleep(self._retry_seconds)


async def while_fresh(token: AppToken | None, session: TokenRefreshingSession, work: Awaitable[T]) -> T:
    """Awaits `work` on `session`, keeping `token`, if any, fresh on it meanwhile. Raises what
    `AppToken.keep_fresh` raises, cancelling `work`."""
    if token is None:
        return await work
    working = asyncio.ensure_future(work)
    refreshing = asyncio.ensure_future(token.keep_fresh(session))
    try:
        await asyncio.wait([working, refreshing], return_when=asyncio.FIRST_COMPLETED)
        if refreshing.done():
            refreshing.result()
        return working.result()
    finally:
        for task in (working, refreshing):
            task.cancel()
        # The caller closes the session next, so `work` has to have stopped using it.
        await asyncio.gather(working, refreshing, return_exceptions=True)
