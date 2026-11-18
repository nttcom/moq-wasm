import aiohttp
import pytest
from aiohttp import web
from aiohttp.test_utils import TestServer

from moq_bot_rooms.app_token import (
    CLOUD_TEMPORARY_APPS_URL,
    LOCAL_TEMPORARY_APPS_URL,
    AppToken,
    IssuedToken,
    TemporaryApps,
    TemporaryAppsUnavailable,
    TokenRejected,
    temporary_apps_url,
)
from tests.helpers import FakeIssuer

@pytest.fixture
async def issuer_server():
    """A temporary-apps service that answers `POST /tokens/refresh` with `answers[token]` as (status, body)."""
    answers: dict[str, tuple[int, dict]] = {}

    async def refresh(request: web.Request) -> web.Response:
        status, body = answers[(await request.json())["token"]]
        return web.json_response(body, status=status)

    app = web.Application()
    app.router.add_post("/tokens/refresh", refresh)
    async with TestServer(app) as server, aiohttp.ClientSession() as http:
        yield answers, TemporaryApps(http, str(server.make_url("")).rstrip("/"))


@pytest.mark.parametrize(
    ("configured", "relay_url", "expected"),
    [
        (None, "https://127.0.0.1:4433", LOCAL_TEMPORARY_APPS_URL),
        (None, "https://relay-1.moqt.research.skyway.io", CLOUD_TEMPORARY_APPS_URL),
        ("http://issuer.example/", "https://127.0.0.1:4433", "http://issuer.example"),
    ],
)
def test_the_issuer_is_the_configured_one_or_the_one_next_to_the_relay(configured, relay_url, expected):
    # Act / Assert
    assert temporary_apps_url(configured, relay_url) == expected


async def test_a_refresh_answers_the_token_and_its_app(issuer_server):
    # Arrange
    answers, issuer = issuer_server
    answers["page-token"] = (200, {"token": "new", "appId": "temp-0123", "expiresAt": 1759603600})

    # Act
    issued = await issuer.refresh("page-token")

    # Assert
    assert (issued.token, issued.app_id) == ("new", "temp-0123")


@pytest.mark.parametrize(
    ("answer", "error"),
    [
        ((401, {"error": "expired_token"}), TokenRejected),
        ((500, {}), TemporaryAppsUnavailable),
        ((200, {"token": "new"}), TemporaryAppsUnavailable),
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
        issuer = TemporaryApps(http, "http://127.0.0.1:9")

        # Act / Assert
        with pytest.raises(TemporaryAppsUnavailable):
            await issuer.refresh("page-token")


async def test_refresh_returns_the_issuers_later_token():
    # Arrange
    issuer = FakeIssuer({"page-token": "temp-0123"})
    token = AppToken(issuer, IssuedToken("page-token", "temp-0123"))

    # Act / Assert
    assert await token.refresh("page-token") == "page-token+"
