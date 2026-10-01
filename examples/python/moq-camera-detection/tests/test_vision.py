import aiohttp
import pytest

from moq_camera_detection.identity_token import GcloudIdentityToken
from moq_camera_detection.vision import DjevVisionClient, parse_answer


@pytest.mark.parametrize(
    ("content", "person"),
    [("thought\nperson", True), ("thought\nno_person", False), ("no_person", False), ("maybe", None), ("", None)],
)
def test_answer_is_read_from_the_last_line(content, person):
    # Act / Assert
    assert parse_answer(content) is person


async def test_image_is_sent_with_the_question(fake_vision_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        token = GcloudIdentityToken(command=("echo", "identity-token"))
        client = DjevVisionClient(session, fake_vision_server.url, token)

        # Act
        person = await client.has_person(b"jpeg")

    # Assert
    [(authorization, request)] = fake_vision_server.requests
    [image, question] = request["messages"][0]["content"]
    assert person is True
    assert authorization == "Bearer identity-token"
    assert image["image_url"]["url"] == "data:image/jpeg;base64,anBlZw=="
    assert "no_person" in question["text"]
