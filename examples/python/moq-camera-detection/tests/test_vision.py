import aiohttp
from moq_bot_rooms.identity_token import IdentityToken
from moq_system_one_bot.system_one_status import SystemOneStatus

from moq_camera_detection.prompt import BoxesQuestion, ChoiceQuestion, Prompt
from moq_camera_detection.vision import SystemOneVisionClient
from tests.helpers import YES_NO_PROMPT

PERSON_AND_CUPS = Prompt((ChoiceQuestion("Is there a person?", ("yes", "no")), BoxesQuestion("cup")), (7, 0))


async def identity_token_value() -> str:
    return "identity-token"


async def answer_with(server, prompt: Prompt, token: IdentityToken | None = None):
    async with aiohttp.ClientSession() as session:
        return await SystemOneVisionClient(session, server.url, SystemOneStatus(), token).answer(b"jpeg", prompt)


async def test_every_question_is_asked_in_one_request_and_answered_in_order(fake_vision_server):
    # Arrange
    cup = {"left": 0.1, "top": 0.2, "right": 0.3, "bottom": 0.4}
    fake_vision_server.answers = {"q0": {"choice": "yes"}, "q1": {"boxes": [{"box": cup, "found": 0.9}]}}

    # Act
    answers = await answer_with(fake_vision_server, PERSON_AND_CUPS, IdentityToken(identity_token_value))

    # Assert
    [(authorization, request)] = fake_vision_server.requests
    assert answers == ["yes", [[0.1, 0.2, 0.3, 0.4]]]
    assert authorization == "Bearer identity-token"
    assert request["images"] == ["data:image/jpeg;base64,anBlZw=="]
    assert request["questions"] == {
        "q0": {"type": "choice", "instructions": "Is there a person?", "criteria": {"yes": None, "no": None}},
        "q1": {"type": "boxes", "instructions": "Every cup.", "max": 8},
    }


async def test_answer_outside_the_prompt_choices_is_no_choice(fake_vision_server):
    # Arrange
    fake_vision_server.answers = {"q0": {"choice": "maybe"}}

    # Act / Assert
    assert await answer_with(fake_vision_server, YES_NO_PROMPT) == [None]
