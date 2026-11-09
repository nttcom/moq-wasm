import base64
import io
from dataclasses import dataclass, field
from typing import Any

import pytest
from aiohttp.test_utils import TestClient, TestServer
from PIL import Image

from system_one_server.server import SYSTEM_ONE_PATH, create_app

NOUL = {"type": "noul", "instructions": "Is it urgent?"}


@dataclass
class FakeModel:
    rejects: bool = False
    calls: list[tuple[Any, dict, list | None]] = field(default_factory=list)

    def system_one(self, state, questions, media=None):
        if self.rejects:
            raise ValueError('"type" is one of noul, choice')
        self.calls.append((state, questions, media))
        return {"answers": {"urgent": {"type": "noul", "noul": 0.9}}, "usage": {"input_tokens": 7, "output_tokens": 0}}


def jpeg_data_url(size: tuple[int, int]) -> str:
    buffer = io.BytesIO()
    Image.new("RGB", size, "red").save(buffer, format="JPEG")
    return "data:image/jpeg;base64," + base64.b64encode(buffer.getvalue()).decode()


@pytest.fixture
async def client_for():
    clients = []

    async def start(model: FakeModel) -> TestClient:
        client = TestClient(TestServer(create_app(model)))
        await client.start_server()
        clients.append(client)
        return client

    yield start
    for client in clients:
        await client.close()


async def test_answers_are_the_models_reply(client_for):
    # Arrange
    model = FakeModel()
    client = await client_for(model)

    # Act
    response = await client.post(SYSTEM_ONE_PATH, json={"state": "Ship it now", "questions": {"urgent": NOUL}})

    # Assert
    assert response.status == 200
    assert await response.json() == {
        "answers": {"urgent": {"type": "noul", "noul": 0.9}},
        "usage": {"input_tokens": 7, "output_tokens": 0},
    }
    assert model.calls == [("Ship it now", {"urgent": NOUL}, None)]


async def test_images_reach_the_model_as_pictures(client_for):
    # Arrange
    model = FakeModel()
    client = await client_for(model)

    # Act
    response = await client.post(
        SYSTEM_ONE_PATH, json={"images": [jpeg_data_url((32, 16))], "questions": {"urgent": NOUL}}
    )

    # Assert
    assert response.status == 200
    [(state, _, media)] = model.calls
    assert state is None
    assert [picture.size for picture in media] == [(32, 16)]


@pytest.mark.parametrize(
    "body",
    [
        {"state": "no questions"},
        {"images": ["https://example.com/a.jpg"], "questions": {"urgent": NOUL}},
        {"images": ["data:image/jpeg;base64,bm90IGFuIGltYWdl"], "questions": {"urgent": NOUL}},
    ],
)
async def test_malformed_request_is_a_bad_request(client_for, body):
    # Arrange
    model = FakeModel()
    client = await client_for(model)

    # Act
    response = await client.post(SYSTEM_ONE_PATH, json=body)

    # Assert
    assert response.status == 400
    assert model.calls == []


async def test_question_the_model_rejects_is_unprocessable(client_for):
    # Arrange
    client = await client_for(FakeModel(rejects=True))

    # Act
    response = await client.post(SYSTEM_ONE_PATH, json={"questions": {"urgent": {"type": "poll"}}})

    # Assert
    assert response.status == 422


async def test_health_answers_once_serving(client_for):
    # Arrange
    client = await client_for(FakeModel())

    # Act / Assert
    assert (await client.get("/health")).status == 200
