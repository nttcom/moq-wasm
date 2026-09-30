import pytest
from aiohttp import web

from tests.helpers import FakeJevServer


@pytest.fixture
async def fake_jev_server():
    server = FakeJevServer(noul=0.9)
    app = web.Application()
    app.router.add_post("/v1/systemone", server.handle)
    runner = web.AppRunner(app)
    await runner.setup()
    site = web.TCPSite(runner, "127.0.0.1", 0)
    await site.start()
    port = runner.addresses[0][1]
    server.url = f"http://127.0.0.1:{port}/v1/systemone"
    yield server
    await runner.cleanup()
