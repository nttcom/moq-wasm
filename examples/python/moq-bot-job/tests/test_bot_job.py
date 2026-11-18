import argparse
import asyncio

import aiohttp
import pytest
from aiohttp import web
from aiohttp.test_utils import TestServer

from moq_bot_job import bot_job
from moq_bot_job.bot_job import BotJob, Bots, add_bot_job_arguments, bot_job_from_arguments, run_bot_job


class FakeBotsService:
    """The bot service's bot routes: answers token calls with `token` and heartbeats with `stop`, and
    keeps each call's path, `Authorization` and body."""

    def __init__(self, token: str | None = "jwt", stop: bool = False):
        self.token = token
        self.stop = stop
        self.calls: list[tuple[str, str, dict | None]] = []

    async def handle(self, request: web.Request) -> web.Response:
        body = await request.json() if request.can_read_body else None
        self.calls.append((request.path, request.headers.get("Authorization", ""), body))
        if request.path.endswith("/token"):
            return web.json_response({"token": self.token})
        return web.json_response({"stop": self.stop})

    def heartbeats(self) -> list[tuple[str, str | None]]:
        return [(body["state"], body["error"]) for path, _, body in self.calls if path.endswith("/heartbeat")]


@pytest.fixture
async def open_bots(monkeypatch):
    monkeypatch.setattr(bot_job, "HEARTBEAT_SECONDS", 0.01)
    sessions = []

    async def open(service: FakeBotsService) -> tuple[BotJob, Bots]:
        app = web.Application()
        app.router.add_post("/bots/{type}/{id}/{call}", service.handle)
        server = TestServer(app)
        await server.start_server()
        http = aiohttp.ClientSession()
        sessions.append((server, http))
        job = BotJob("transcriber", "b1", str(server.make_url("")).rstrip("/"), ["app", "room"], 30.0, {})
        return job, Bots(http, job, "key", None)

    yield open
    for server, http in sessions:
        await http.close()
        await server.close()


def test_the_arguments_name_the_room_and_the_options():
    # Arrange
    parser = argparse.ArgumentParser()
    add_bot_job_arguments(parser)

    # Act
    job = bot_job_from_arguments(
        parser.parse_args(
            [
                "--bot-type", "transcriber",
                "--bot-id", "b1",
                "--bots-url", "http://bots.example/",
                "--namespace-prefix", "app/team a",
                "--grace-seconds", "30",
                "--max-seconds", "3600",
                "--options", '{"asr": "whisper"}',
            ]
        )
    )

    # Assert
    assert job == BotJob("transcriber", "b1", "http://bots.example", ["app", "team a"], 30.0, {"asr": "whisper"})


async def test_a_bot_presents_its_key_for_its_token(open_bots):
    # Arrange
    service = FakeBotsService()
    _, bots = await open_bots(service)

    # Act
    token = await bots.token()

    # Assert
    assert token == "jwt"
    assert service.calls == [("/bots/transcriber/b1/token", "Bearer key", None)]


async def test_a_room_that_empties_reports_the_bot_finished(open_bots):
    # Arrange
    service = FakeBotsService()
    job, bots = await open_bots(service)

    async def run_room(job: BotJob, bots: Bots) -> None:
        await asyncio.sleep(0.05)

    # Act
    await run_bot_job(job, bots, run_room)

    # Assert: running while it is in the room, then finished
    assert service.heartbeats()[0] == ("running", None)
    assert service.heartbeats()[-1] == ("finished", None)


async def test_a_bot_asked_to_stop_leaves_its_room_and_reports_finished(open_bots):
    # Arrange
    service = FakeBotsService(stop=True)
    job, bots = await open_bots(service)
    left = asyncio.Event()

    async def run_room(job: BotJob, bots: Bots) -> None:
        try:
            await asyncio.Event().wait()
        finally:
            left.set()

    # Act
    await asyncio.wait_for(run_bot_job(job, bots, run_room), 2)

    # Assert
    assert left.is_set()
    assert service.heartbeats()[-1] == ("finished", None)


async def test_a_room_that_fails_reports_the_bot_failed(open_bots):
    # Arrange
    service = FakeBotsService()
    job, bots = await open_bots(service)

    async def run_room(job: BotJob, bots: Bots) -> None:
        raise ValueError("unknown asr")

    # Act / Assert
    with pytest.raises(ValueError, match="unknown asr"):
        await run_bot_job(job, bots, run_room)
    assert service.heartbeats()[-1] == ("failed", "ValueError('unknown asr')")
