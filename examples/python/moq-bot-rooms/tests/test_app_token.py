import asyncio

import aiohttp
import pytest
from aiohttp import web
from aiohttp.test_utils import TestServer

from moq_bot_rooms.app_token import (
    CLOUD_TOKEN_MINT_URL,
    LOCAL_TOKEN_MINT_URL,
    REFRESH_MARGIN_SECONDS,
    AppToken,
    IssuedToken,
    TokenMint,
    TokenMintUnavailable,
    TokenRejected,
    token_mint_url,
    while_fresh,
)
from tests.helpers import FakeIssuer, settle

NOW = 1_000_000.0
SHORT_RETRY_SECONDS = 0.01


class FakeSession:
    def __init__(self, refusing: bool = False):
        self.tokens: list[tuple[str, str]] = []
        self._refusing = refusing

    async def refresh_token(self, token: str, *, app_id: str) -> None:
        self.tokens.append((token, app_id))
        if self._refusing:
            raise RuntimeError("UNAUTHORIZED")


def due_token(issuer: FakeIssuer, token: str = "page-token") -> AppToken:
    """A token due for refresh now; the issuer's refreshed tokens are not due for a long time."""
    issued = IssuedToken(token, issuer.apps.get(token, "temp-0123"), NOW + REFRESH_MARGIN_SECONDS)
    return AppToken(issuer, issued, retry_seconds=SHORT_RETRY_SECONDS, now=lambda: NOW)


@pytest.fixture
async def issuer_server():
    """A token mint that answers `POST /tokens/refresh` with `answers[token]` as (status, body)."""
    answers: dict[str, tuple[int, dict]] = {}

    async def refresh(request: web.Request) -> web.Response:
        status, body = answers[(await request.json())["token"]]
        return web.json_response(body, status=status)

    app = web.Application()
    app.router.add_post("/tokens/refresh", refresh)
    async with TestServer(app) as server, aiohttp.ClientSession() as http:
        yield answers, TokenMint(http, str(server.make_url("")).rstrip("/"))


@pytest.mark.parametrize(
    ("configured", "relay_url", "expected"),
    [
        (None, "https://127.0.0.1:4433", LOCAL_TOKEN_MINT_URL),
        (None, "https://relay-1.moqt.research.skyway.io", CLOUD_TOKEN_MINT_URL),
        ("http://issuer.example/", "https://127.0.0.1:4433", "http://issuer.example"),
    ],
)
def test_the_issuer_is_the_configured_one_or_the_one_next_to_the_relay(configured, relay_url, expected):
    # Act / Assert
    assert token_mint_url(configured, relay_url) == expected


async def test_a_refresh_answers_the_token_its_app_and_expiry(issuer_server):
    # Arrange
    answers, issuer = issuer_server
    answers["page-token"] = (200, {"token": "new", "appId": "temp-0123", "expiresAt": 1759603600})

    # Act
    issued = await issuer.refresh("page-token")

    # Assert
    assert (issued.token, issued.app_id, issued.expires_at) == ("new", "temp-0123", 1759603600)


@pytest.mark.parametrize(
    ("answer", "error"),
    [
        ((401, {"error": "expired_token"}), TokenRejected),
        ((500, {}), TokenMintUnavailable),
        ((200, {"token": "new"}), TokenMintUnavailable),
    ],
)
async def test_a_refusal_and_a_broken_answer_are_told_apart(issuer_server, answer, error):
    # Arrange
    answers, issuer = issuer_server
    answers["page-token"] = answer

    # Act / Assert
    with pytest.raises(error):
        await issuer.refresh("page-token")


async def test_an_issuer_that_is_not_listening_is_unavailable():
    # Arrange
    async with aiohttp.ClientSession() as http:
        issuer = TokenMint(http, "http://127.0.0.1:9")

        # Act / Assert
        with pytest.raises(TokenMintUnavailable):
            await issuer.refresh("page-token")


async def test_a_token_due_for_refresh_reaches_the_session():
    # Arrange
    issuer = FakeIssuer({"page-token": "temp-0123"})
    token = due_token(issuer)
    session = FakeSession()

    # Act
    refreshing = asyncio.create_task(token.keep_fresh(session))
    await settle()
    refreshing.cancel()

    # Assert
    assert session.tokens == [("page-token+", "temp-0123")]
    assert token.token == "page-token+"


async def test_a_refusal_by_the_issuer_ends_the_refreshing():
    # Arrange
    issuer = FakeIssuer()
    token = due_token(issuer)

    # Act / Assert
    with pytest.raises(TokenRejected):
        await asyncio.wait_for(token.keep_fresh(FakeSession()), 1)


async def test_an_issuer_down_until_the_token_expires_ends_the_refreshing():
    # Arrange: half the margin passes with each failed refresh
    issuer = FakeIssuer({"page-token": "temp-0123"})
    issuer.failure = TokenMintUnavailable("down")
    token = AppToken(
        issuer,
        IssuedToken("page-token", "temp-0123", NOW + REFRESH_MARGIN_SECONDS),
        retry_seconds=SHORT_RETRY_SECONDS,
        now=lambda: NOW + len(issuer.refreshed) * REFRESH_MARGIN_SECONDS / 2,
    )

    # Act / Assert: the first failure is retried, the one at the expiry is not
    with pytest.raises(TokenMintUnavailable):
        await asyncio.wait_for(token.keep_fresh(FakeSession()), 1)
    assert len(issuer.refreshed) == 2


async def test_a_relay_that_refuses_the_new_token_does_not_end_the_refreshing():
    # Arrange
    issuer = FakeIssuer({"page-token": "temp-0123"})
    token = due_token(issuer)

    # Act
    refreshing = asyncio.create_task(token.keep_fresh(FakeSession(refusing=True)))
    await settle()

    # Assert
    assert not refreshing.done()
    refreshing.cancel()


async def test_while_fresh_raises_the_refresh_failure_and_cancels_the_work():
    # Arrange
    token = due_token(FakeIssuer())
    work = asyncio.ensure_future(asyncio.sleep(60))

    # Act / Assert
    with pytest.raises(TokenRejected):
        await asyncio.wait_for(while_fresh(token, FakeSession(), work), 1)
    await settle()
    assert work.cancelled()


async def test_while_fresh_without_a_token_only_awaits_the_work():
    # Act / Assert
    assert await while_fresh(None, FakeSession(), asyncio.sleep(0, "done")) == "done"
