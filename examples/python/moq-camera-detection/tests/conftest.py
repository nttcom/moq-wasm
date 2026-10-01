import pytest
from aiohttp import web
from aiohttp.test_utils import TestServer

from tests.helpers import FakeVisionServer


@pytest.fixture
async def fake_vision_server():
    server = FakeVisionServer(content="thought\nperson")
    app = web.Application()
    app.router.add_post("/v1/chat/completions", server.handle)
    async with TestServer(app) as test_server:
        server.url = str(test_server.make_url("/v1/chat/completions"))
        yield server
