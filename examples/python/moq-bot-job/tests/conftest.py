import pytest
from aiohttp import web
from aiohttp.test_utils import TestServer

from tests.helpers import FakeMetadataServer


@pytest.fixture
async def fake_metadata_server():
    server = FakeMetadataServer()
    app = web.Application()
    app.router.add_get("/identity", server.handle)
    async with TestServer(app) as test_server:
        server.url = str(test_server.make_url("/identity"))
        yield server
