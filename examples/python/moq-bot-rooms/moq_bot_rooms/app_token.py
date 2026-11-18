"""The token of the temporary app a page runs in, which the bot keeps alive on the temporary-apps service
(services/temporary-apps) for as long as its room runs (docs/architecture/ai-bots/on-demand-rooms.md
"Temporary apps")."""

from dataclasses import dataclass
from urllib.parse import urlsplit

import aiohttp

LOCAL_TEMPORARY_APPS_URL = "http://localhost:8083"
CLOUD_TEMPORARY_APPS_URL = "https://token-mint.moqt.research.skyway.io"
LOCAL_HOSTS = {"localhost", "127.0.0.1", "::1"}
REQUEST_TIMEOUT = aiohttp.ClientTimeout(total=10)

class TokenRejected(Exception):
    """The temporary-apps service refused the token: it did not sign it, or it has expired."""


class TemporaryAppsUnavailable(Exception):
    """The temporary-apps service did not answer, or answered something other than a token or a refusal."""


def temporary_apps_url(configured: str | None, relay_url: str) -> str:
    """`configured`, else the issuer next to the relay: the local one for a local relay."""
    if configured:
        return configured.rstrip("/")
    return LOCAL_TEMPORARY_APPS_URL if urlsplit(relay_url).hostname in LOCAL_HOSTS else CLOUD_TEMPORARY_APPS_URL


class TemporaryApps:
    def __init__(self, http: aiohttp.ClientSession, url: str):
        self._http = http
        self._url = url

    async def refresh(self, token: str) -> "IssuedToken":
        """A new token of `token`'s app (`POST /tokens/refresh`). Refreshing the token a page handed
        over is how the bot, without the mint's secret, knows the token is valid and which app it
        belongs to. Raises `TokenRejected` or `TemporaryAppsUnavailable`."""
        try:
            async with self._http.post(
                f"{self._url}/tokens/refresh", json={"token": token}, timeout=REQUEST_TIMEOUT
            ) as response:
                answer = await response.json(content_type=None)
                if response.status == 401:
                    raise TokenRejected(answer.get("error", "rejected"))
                response.raise_for_status()
                return IssuedToken(str(answer["token"]), str(answer["appId"]))
        except (aiohttp.ClientError, TimeoutError, ValueError, KeyError, TypeError, AttributeError) as error:
            raise TemporaryAppsUnavailable(f"{self._url}: {error!r}") from error


@dataclass(frozen=True)
class IssuedToken:
    token: str
    app_id: str


class AppToken:
    """A temporary app's token as the temporary-apps service issued it. The bot's relay session keeps it fresh
    through `refresh` (`moq_wasm.connect`'s `refresh_token`)."""

    def __init__(self, issuer: TemporaryApps, issued: IssuedToken):
        self._issuer = issuer
        self._issued = issued

    @property
    def app_id(self) -> str:
        return self._issued.app_id

    @property
    def token(self) -> str:
        return self._issued.token

    async def refresh(self, token: str) -> str:
        """A later token of the app. Raises `TokenRejected` or `TemporaryAppsUnavailable`."""
        return (await self._issuer.refresh(token)).token
