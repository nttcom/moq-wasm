import aiohttp
from moq_bot_rooms.identity_token import IdentityToken
from moq_system_one_bot.system_one_status import SystemOneStatus

from moq_camera_detection.vision import SystemOneVisionClient
from tests.helpers import YES_NO_PROMPT


async def identity_token_value() -> str:
    return "identity-token"


async def test_image_is_asked_as_a_choice_between_the_prompt_choices(fake_vision_server):
    # Arrange
    async with aiohttp.ClientSession() as session:
        token = IdentityToken(identity_token_value)
        client = SystemOneVisionClient(session, fake_vision_server.url, SystemOneStatus(), token)

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
        client = SystemOneVisionClient(session, fake_vision_server.url, SystemOneStatus())

        # Act
        choice = await client.choose(b"jpeg", YES_NO_PROMPT)

    # Assert
    assert choice is None
