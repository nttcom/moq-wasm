import aiohttp
import pytest

from moq_ptz_tracking.d3_flash_status import D3FlashStatus
from moq_ptz_tracking.identity_token import gcloud_identity_token
from moq_ptz_tracking.vision import Answer, D3FlashVisionClient, Position, read_position
from tests.helpers import grid_probabilities


@pytest.mark.parametrize(
    ("given", "answer"),
    [
        ({"r1c3": 1.0}, Position(0.7, 0.3)),
        ({"r0c0": 0.5, "r0c1": 0.5}, Position(0.2, 0.1)),
        ({"r4c4": 0.3}, Position(0.9, 0.9)),
    ],
)
def test_position_is_the_probability_weighted_center_of_the_grid_cells(given, answer):
    # Act
    position = read_position(grid_probabilities(given))

    # Assert
    assert (position.x, position.y) == pytest.approx((answer.x, answer.y))


async def locate_with(server, description: str = "a red mug"):
    async with aiohttp.ClientSession() as session:
        token = gcloud_identity_token(command=("echo", "identity-token"))
        client = D3FlashVisionClient(session, server.url, D3FlashStatus("status"), token)
        return await client.locate(b"jpeg", description)


async def test_visible_target_is_asked_whether_then_where(fake_vision_server):
    # Act
    answer = await locate_with(fake_vision_server)

    # Assert
    [(authorization, visible), (_, where)] = fake_vision_server.requests
    assert answer == Position(0.7, 0.3)
    assert authorization == "Bearer identity-token"
    assert visible["images"] == where["images"] == ["data:image/jpeg;base64,anBlZw=="]
    assert visible["questions"] == {"visible": {"type": "noul", "instructions": "Is a red mug in this picture?"}}
    assert where["questions"]["where"]["instructions"] == "Where is a red mug in this picture?"
    assert len(where["questions"]["where"]["criteria"]) == 25


async def test_target_judged_absent_is_not_asked_where(fake_vision_server):
    # Arrange
    fake_vision_server.answers["visible"] = {"noul": 0.04}

    # Act
    answer = await locate_with(fake_vision_server)

    # Assert
    assert answer is Answer.NOT_VISIBLE
    assert len(fake_vision_server.requests) == 1
