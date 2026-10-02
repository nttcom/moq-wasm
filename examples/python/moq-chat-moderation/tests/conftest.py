import pytest
from aiohttp import web
from aiohttp.test_utils import TestServer

from tests.helpers import FakeMetadataServer, FakeJevServer


@pytest.fixture
async def fake_jev_server():
    server = FakeJevServer(noul=0.9)
    app = web.Application()
    app.router.add_post("/v1/systemone", server.handle)
    async with TestServer(app) as test_server:
        server.url = str(test_server.make_url("/v1/systemone"))
        yield server


@pytest.fixture
async def fake_metadata_server():
    server = FakeMetadataServer()
    app = web.Application()
    app.router.add_get("/identity", server.handle)
    async with TestServer(app) as test_server:
        server.url = str(test_server.make_url("/identity"))
        yield server
