import aiohttp
import pytest

from moq_camera_detection.djev_status import DjevStatus
from moq_camera_detection.identity_token import gcloud_identity_token
from moq_camera_detection.vision import DjevVisionClient, parse_choice
from tests.helpers import YES_NO_PROMPT


@pytest.mark.parametrize(
    ("content", "choice"),
    [
        ("thought\n1", "yes"),
        ("thought\n2. no", "no"),
        ("2", "no"),
        ("thought\n0", None),
        ("thought\n3", None),
        ("maybe", None),
        ("", None),
    ],
)
def test_choice_is_read_from_the_number_on_the_last_line(content, choice):
    # Act / Assert
    assert parse_choice(content, YES_NO_PROMPT) == choice


async def test_image_is_sent_with_the_numbered_choices(fake_vision_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        token = gcloud_identity_token(command=("echo", "identity-token"))
        client = DjevVisionClient(session, fake_vision_server.url, DjevStatus("status"), token)

        # Act
        choice = await client.choose(b"jpeg", YES_NO_PROMPT)

    # Assert
    [(authorization, request)] = fake_vision_server.requests
    [image, question] = request["messages"][0]["content"]
    assert choice == "no"
    assert authorization == "Bearer identity-token"
    assert image["image_url"]["url"] == "data:image/jpeg;base64,anBlZw=="
    assert question["text"].startswith("Is there a person?\n")
    assert question["text"].endswith("\n1. yes\n2. no")
