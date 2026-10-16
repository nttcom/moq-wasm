import aiohttp

from moq_camera_detection.djev_status import DjevStatus
from moq_camera_detection.identity_token import gcloud_identity_token
from moq_camera_detection.vision import DjevVisionClient
from tests.helpers import YES_NO_PROMPT


async def test_image_is_asked_as_a_choice_between_the_prompt_choices(fake_vision_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        token = gcloud_identity_token(command=("echo", "identity-token"))
        client = DjevVisionClient(session, fake_vision_server.url, DjevStatus("status"), token)

        # Act
        choice = await client.choose(b"jpeg", YES_NO_PROMPT)

    # Assert
    [(authorization, request)] = fake_vision_server.requests
    assert choice == "no"
    assert authorization == "Bearer identity-token"
    assert request["images"] == ["data:image/jpeg;base64,anBlZw=="]
    assert request["questions"]["answer"] == {
        "type": "choice",
        "instructions": "Is there a person?",
        "criteria": {"yes": None, "no": None},
    }


async def test_answer_outside_the_prompt_choices_is_no_choice(fake_vision_server):
    # Arrange
    fake_vision_server.choice = "maybe"
    async with aiohttp.ClientSession() as session:
        client = DjevVisionClient(session, fake_vision_server.url, DjevStatus("status"))

        # Act
        choice = await client.choose(b"jpeg", YES_NO_PROMPT)

    # Assert
    assert choice is None
