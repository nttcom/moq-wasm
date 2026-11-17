"""Identity tokens for a bot that calls a Cloud Run service requiring IAM, such as d1-2B or an ASR service."""

import argparse
import asyncio
import time
from collections.abc import Awaitable, Callable
from urllib.parse import urlsplit

import aiohttp

# Google identity tokens expire after an hour; refresh well before that.
TOKEN_LIFETIME_SECONDS = 50 * 60
GCLOUD_COMMAND = ("gcloud", "auth", "print-identity-token")
METADATA_IDENTITY_URL = (
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/identity"
)
METADATA_TIMEOUT = aiohttp.ClientTimeout(total=10)


class IdentityToken:
    def __init__(self, fetch: Callable[[], Awaitable[str]]):
        self._fetch = fetch
        self._token: str | None = None
        self._fetched_at = 0.0

    async def value(self) -> str:
        if self._token is None or time.monotonic() - self._fetched_at > TOKEN_LIFETIME_SECONDS:
            self._token = await self._fetch()
            self._fetched_at = time.monotonic()
        return self._token


def gcloud_identity_token() -> IdentityToken:
    async def fetch() -> str:
        process = await asyncio.create_subprocess_exec(*GCLOUD_COMMAND, stdout=asyncio.subprocess.PIPE)
        stdout, _ = await process.communicate()
        if process.returncode != 0:
            raise RuntimeError(f"{' '.join(GCLOUD_COMMAND)} exited with {process.returncode}")
        return stdout.decode().strip()

    return IdentityToken(fetch)


def metadata_identity_token(
    session: aiohttp.ClientSession, service_url: str, url: str = METADATA_IDENTITY_URL
) -> IdentityToken:
    parts = urlsplit(service_url)
    audience = f"{parts.scheme}://{parts.netloc}"

    async def fetch() -> str:
        async with session.get(
            url,
            params={"audience": audience},
            headers={"Metadata-Flavor": "Google"},
            timeout=METADATA_TIMEOUT,
        ) as response:
            response.raise_for_status()
            return await response.text()

    return IdentityToken(fetch)


def add_identity_token_arguments(parser: argparse.ArgumentParser) -> None:
    auth = parser.add_mutually_exclusive_group()
    auth.add_argument(
        "--gcloud-auth",
        action="store_true",
        help="send a gcloud identity token, for a Cloud Run service that requires IAM",
    )
    auth.add_argument(
        "--metadata-auth",
        action="store_true",
        help="send the metadata server's identity token, for a bot on Cloud Run or GCE",
    )


def identity_token_from_arguments(
    args: argparse.Namespace, session: aiohttp.ClientSession, service_url: str
) -> IdentityToken | None:
    if args.gcloud_auth:
        return gcloud_identity_token()
    if args.metadata_auth:
        return metadata_identity_token(session, service_url)
    return None
