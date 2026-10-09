import aiohttp
import pytest

from moq_ptz_tracking.djev_status import DjevStatus
from moq_ptz_tracking.identity_token import gcloud_identity_token
from moq_ptz_tracking.vision import NOT_VISIBLE, DjevVisionClient, parse_position


@pytest.mark.parametrize(
    ("content", "position"),
    [
        ("thought\n1", "top left"),
        ("thought\n5. center", "center"),
        ("9", "bottom right"),
        ("thought\n10", NOT_VISIBLE),
        ("thought\n0", None),
        ("thought\n11", None),
        ("maybe", None),
        ("", None),
    ],
)
def test_position_is_read_from_the_number_on_the_last_line(content, position):
    # Act / Assert
    assert parse_position(content) == position


async def test_image_is_sent_with_the_numbered_grid_positions(fake_vision_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        token = gcloud_identity_token(command=("echo", "identity-token"))
        client = DjevVisionClient(session, fake_vision_server.url, DjevStatus("status"), token)

        # Act
        position = await client.locate(b"jpeg", "a red mug")

    # Assert
    [(authorization, request)] = fake_vision_server.requests
    [image, question] = request["messages"][0]["content"]
    assert position == "top right"
    assert authorization == "Bearer identity-token"
    assert image["image_url"]["url"] == "data:image/jpeg;base64,anBlZw=="
    assert question["text"].startswith("Where is a red mug in this picture?\n")
    assert question["text"].endswith("\n9. bottom right\n10. not in the picture")
