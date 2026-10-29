import aiohttp
from moq_bot_rooms.identity_token import IdentityToken
from moq_system_one_bot.system_one_status import SystemOneStatus

from moq_chat_moderation.chat import Label
from moq_chat_moderation.system_one import SystemOneClient

LABELS = (Label("暴言", "相手を侮辱・攻撃・脅す"), Label("スパム", ""))


async def identity_token_value() -> str:
    return "identity-token"


async def test_label_probabilities_are_the_noul_answers_in_label_order(fake_system_one_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        client = SystemOneClient(session, fake_system_one_server.url, SystemOneStatus())

        # Act
        probabilities = await client.label_probabilities("you are an idiot", LABELS)

    # Assert
    assert probabilities == [0.9, 0.1]


async def test_request_asks_one_noul_per_label_without_credentials(fake_system_one_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        client = SystemOneClient(session, fake_system_one_server.url, SystemOneStatus())

        # Act
        await client.label_probabilities("you are an idiot", LABELS)

    # Assert
    [(authorization, request)] = fake_system_one_server.requests
    assert authorization is None
    assert request["state"] == "you are an idiot"
    assert request["questions"] == {
        "l0": {"type": "noul", "instructions": "このチャットメッセージは「暴言」（相手を侮辱・攻撃・脅す）に当てはまりますか？"},
        "l1": {"type": "noul", "instructions": "このチャットメッセージは「スパム」に当てはまりますか？"},
    }


async def test_request_carries_the_identity_token_as_bearer(fake_system_one_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        token = IdentityToken(identity_token_value)
        client = SystemOneClient(session, fake_system_one_server.url, SystemOneStatus(), token)

        # Act
        await client.label_probabilities("hello", LABELS)

    # Assert
    [(authorization, _)] = fake_system_one_server.requests
    assert authorization == "Bearer identity-token"
