import pytest
from aiohttp import web
from aiohttp.test_utils import TestServer

from tests.helpers import FakeVisionServer, grid_probabilities


@pytest.fixture
async def fake_vision_server():
    server = FakeVisionServer(
        answers={"visible": {"noul": 0.9}, "where": {"probabilities": grid_probabilities({"r1c3": 1.0})}}
    )
    app = web.Application()
    app.router.add_post("/v1/systemone", server.handle)
    async with TestServer(app) as test_server:
        server.url = str(test_server.make_url("/v1/systemone"))
        yield server
