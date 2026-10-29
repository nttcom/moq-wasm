import aiohttp
import pytest
from moq_bot_rooms.identity_token import IdentityToken
from moq_system_one_bot.system_one_status import SystemOneStatus

from moq_ptz_tracking.vision import Answer, Box, Sighting, SystemOneVisionClient, TargetBox

MUG = {"left": 0.6, "top": 0.2, "right": 0.8, "bottom": 0.4}
CAP = {"left": 0.1, "top": 0.5, "right": 0.3, "bottom": 0.9}


async def identity_token_value() -> str:
    return "identity-token"


async def locate_with(server, descriptions: tuple[str, ...] = ("red mug", "blue cap")):
    async with aiohttp.ClientSession() as session:
        token = IdentityToken(identity_token_value)
        client = SystemOneVisionClient(session, server.url, SystemOneStatus(), token)
        return await client.locate(b"jpeg", descriptions)


async def test_every_target_is_asked_for_all_its_boxes_in_one_request(fake_vision_server):
    # Arrange
    fake_vision_server.answers = {"t0": {"boxes": [{"box": MUG}]}, "t1": {"boxes": [{"box": CAP}, {"box": MUG}]}}

    # Act
    answer = await locate_with(fake_vision_server)

    # Assert
    [(authorization, request)] = fake_vision_server.requests
    assert authorization == "Bearer identity-token"
    assert request == {
        "images": ["data:image/jpeg;base64,anBlZw=="],
        "questions": {
            "t0": {"type": "boxes", "instructions": "Every red mug.", "max": 8},
            "t1": {"type": "boxes", "instructions": "Every blue cap.", "max": 8},
        },
    }
    assert answer == Sighting((TargetBox(0, Box(**MUG)), TargetBox(1, Box(**CAP)), TargetBox(1, Box(**MUG))))


async def test_no_box_for_any_target_is_not_visible(fake_vision_server):
    # Act / Assert
    assert await locate_with(fake_vision_server) is Answer.NOT_VISIBLE


def test_union_encloses_every_box():
    # Arrange
    sighting = Sighting((TargetBox(0, Box(**MUG)), TargetBox(1, Box(**CAP))))

    # Act / Assert
    assert sighting.union() == Box(0.1, 0.2, 0.8, 0.9)


@pytest.mark.parametrize(
    ("box", "center"), [(Box(0.6, 0.2, 0.8, 0.4), (0.7, 0.3)), (Box(0.0, 0.0, 1.0, 0.5), (0.5, 0.25))]
)
def test_center_is_halfway_between_the_edges(box, center):
    # Act / Assert
    assert (box.center.x, box.center.y) == pytest.approx(center)
