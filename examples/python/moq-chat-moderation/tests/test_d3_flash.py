import aiohttp

from moq_chat_moderation.d3_flash import ABUSIVE_QUESTION_ID, D3FlashClient
from moq_chat_moderation.d3_flash_status import D3FlashStatus
from moq_chat_moderation.identity_token import gcloud_identity_token


async def test_abusive_probability_is_the_noul_answer(fake_d3_flash_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        client = D3FlashClient(session, fake_d3_flash_server.url, D3FlashStatus("status"))

        # Act
        probability = await client.abusive_probability("you are an idiot")

    # Assert
    assert probability == 0.9


async def test_request_asks_the_abusive_question_without_credentials(fake_d3_flash_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        client = D3FlashClient(session, fake_d3_flash_server.url, D3FlashStatus("status"))

        # Act
        await client.abusive_probability("you are an idiot")

    # Assert
    [(authorization, request)] = fake_d3_flash_server.requests
    assert authorization is None
    assert request["state"] == "you are an idiot"
    assert request["questions"][ABUSIVE_QUESTION_ID]["type"] == "noul"


async def test_request_carries_the_identity_token_as_bearer(fake_d3_flash_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        token = gcloud_identity_token(command=("echo", "identity-token"))
        client = D3FlashClient(session, fake_d3_flash_server.url, D3FlashStatus("status"), token)

        # Act
        await client.abusive_probability("hello")

    # Assert
    [(authorization, _)] = fake_d3_flash_server.requests
    assert authorization == "Bearer identity-token"
