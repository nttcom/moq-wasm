import aiohttp

from moq_chat_moderation.identity_token import metadata_identity_token

SERVICE_URL = "https://djev.example.run.app/v1/chat/completions"


async def test_metadata_token_is_requested_for_the_service_origin_once(fake_metadata_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        token = metadata_identity_token(session, SERVICE_URL, fake_metadata_server.url)

        # Act
        values = [await token.value(), await token.value()]

    # Assert
    assert values == ["metadata-token", "metadata-token"]
    assert fake_metadata_server.requests == [("Google", "https://djev.example.run.app")]
