import aiohttp

from moq_chat_moderation.jev import ABUSIVE_QUESTION_ID, JevClient


async def test_abusive_probability_is_the_noul_answer(fake_jev_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        client = JevClient(session, "jv_test_key", url=fake_jev_server.url)

        # Act
        probability = await client.abusive_probability("you are an idiot")

    # Assert
    assert probability == 0.9


async def test_request_asks_the_abusive_question_about_the_text(fake_jev_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        client = JevClient(session, "jv_test_key", url=fake_jev_server.url)

        # Act
        await client.abusive_probability("you are an idiot")

    # Assert
    [(authorization, request)] = fake_jev_server.requests
    assert authorization == "Bearer jv_test_key"
    assert request["state"] == "you are an idiot"
    assert request["questions"][ABUSIVE_QUESTION_ID]["type"] == "noul"
