import aiohttp

from moq_chat_moderation.djev_status import DjevStatus
from moq_chat_moderation.identity_token import gcloud_identity_token
from moq_chat_moderation.jev import ABUSIVE_QUESTION_ID, JevClient


async def test_abusive_probability_is_the_noul_answer(fake_jev_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        client = JevClient(session, fake_jev_server.url, DjevStatus("status"))

        # Act
        probability = await client.abusive_probability("you are an idiot")

    # Assert
    assert probability == 0.9


async def test_request_asks_the_abusive_question_without_credentials(fake_jev_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        client = JevClient(session, fake_jev_server.url, DjevStatus("status"))

        # Act
        await client.abusive_probability("you are an idiot")

    # Assert
    [(authorization, request)] = fake_jev_server.requests
    assert authorization is None
    assert request["state"] == "you are an idiot"
    assert request["questions"][ABUSIVE_QUESTION_ID]["type"] == "noul"


async def test_request_carries_the_identity_token_as_bearer(fake_jev_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        token = gcloud_identity_token(command=("echo", "identity-token"))
        client = JevClient(session, fake_jev_server.url, DjevStatus("status"), token)

        # Act
        await client.abusive_probability("hello")

    # Assert
    [(authorization, _)] = fake_jev_server.requests
    assert authorization == "Bearer identity-token"
