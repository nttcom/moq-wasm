import aiohttp
import pytest

from moq_ptz_tracking.djev_status import DjevStatus
from moq_ptz_tracking.identity_token import gcloud_identity_token
from moq_ptz_tracking.vision import Answer, DjevVisionClient, Position, parse_position


@pytest.mark.parametrize(
    ("content", "answer"),
    [
        ("thought\n412 417", Position(0.412, 0.417)),
        ("750, 250", Position(0.75, 0.25)),
        ("thought\nnone", Answer.NOT_VISIBLE),
        ("thought\n1200 300", Answer.UNREADABLE),
        ("maybe", Answer.UNREADABLE),
        ("", Answer.UNREADABLE),
    ],
)
def test_position_is_read_per_mille_from_the_last_line(content, answer):
    # Act / Assert
    assert parse_position(content) == answer


async def test_image_is_sent_with_a_question_for_per_mille_coordinates(fake_vision_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        token = gcloud_identity_token(command=("echo", "identity-token"))
        client = DjevVisionClient(session, fake_vision_server.url, DjevStatus("status"), token)

        # Act
        answer = await client.locate(b"jpeg", "a red mug")

    # Assert
    [(authorization, request)] = fake_vision_server.requests
    [image, question] = request["messages"][0]["content"]
    assert answer == Position(0.75, 0.25)
    assert authorization == "Bearer identity-token"
    assert image["image_url"]["url"] == "data:image/jpeg;base64,anBlZw=="
    assert question["text"].startswith("Where is the center of a red mug in this picture?\n")
    assert "from 0 to 1000" in question["text"]
