import aiohttp
from moq_bot_rooms.identity_token import IdentityToken
from moq_system_one_bot.system_one_status import SystemOneStatus

from moq_chat_moderation.system_one import ABUSIVE_QUESTION_ID, SystemOneClient


async def identity_token_value() -> str:
    return "identity-token"


async def test_abusive_probability_is_the_noul_answer(fake_system_one_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        client = SystemOneClient(session, fake_system_one_server.url, SystemOneStatus())

        # Act
        probability = await client.abusive_probability("you are an idiot")

    # Assert
    assert probability == 0.9


async def test_request_asks_the_abusive_question_without_credentials(fake_system_one_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        client = SystemOneClient(session, fake_system_one_server.url, SystemOneStatus())

        # Act
        await client.abusive_probability("you are an idiot")

    # Assert
    [(authorization, request)] = fake_system_one_server.requests
    assert authorization is None
    assert request["state"] == "you are an idiot"
    assert request["questions"][ABUSIVE_QUESTION_ID]["type"] == "noul"


async def test_request_carries_the_identity_token_as_bearer(fake_system_one_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        token = IdentityToken(identity_token_value)
        client = SystemOneClient(session, fake_system_one_server.url, SystemOneStatus(), token)

        # Act
        await client.abusive_probability("hello")

    # Assert
    [(authorization, _)] = fake_system_one_server.requests
    assert authorization == "Bearer identity-token"
