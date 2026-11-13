import aiohttp
import pytest
from aiohttp import web
from aiohttp.test_utils import TestServer

from moq_bot_rooms.app_token import (
    CLOUD_TOKEN_MINT_URL,
    LOCAL_TOKEN_MINT_URL,
    AppToken,
    IssuedToken,
    TokenMint,
    TokenMintUnavailable,
    TokenRejected,
    token_mint_url,
)
from tests.helpers import FakeIssuer

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


async def test_refresh_returns_the_issuers_later_token():
    # Arrange
    issuer = FakeIssuer({"page-token": "temp-0123"})
    token = AppToken(issuer, IssuedToken("page-token", "temp-0123", 2**40))

    # Act / Assert
    assert await token.refresh("page-token") == "page-token+"
