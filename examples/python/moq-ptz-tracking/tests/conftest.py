import pytest
from aiohttp import web
from aiohttp.test_utils import TestServer

from tests.helpers import FakeMetadataServer, FakeVisionServer


@pytest.fixture
async def fake_vision_server():
    server = FakeVisionServer(content="thought\n3. top right")
    app = web.Application()
    app.router.add_post("/v1/chat/completions", server.handle)
    async with TestServer(app) as test_server:
        server.url = str(test_server.make_url("/v1/chat/completions"))
        yield server


@pytest.fixture
async def fake_metadata_server():
    server = FakeMetadataServer()
    app = web.Application()
    app.router.add_get("/identity", server.handle)
    async with TestServer(app) as test_server:
        server.url = str(test_server.make_url("/identity"))
        yield server
