import pytest
from aiohttp import web
from aiohttp.test_utils import TestServer

from tests.helpers import FakeSystemOneServer


@pytest.fixture
async def fake_system_one_server():
    server = FakeSystemOneServer(noul=0.9)
    app = web.Application()
    app.router.add_post("/v1/systemone", server.handle)
    async with TestServer(app) as test_server:
        server.url = str(test_server.make_url("/v1/systemone"))
        yield server
