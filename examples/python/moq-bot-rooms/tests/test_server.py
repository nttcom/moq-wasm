import asyncio

import pytest
from aiohttp.test_utils import TestClient, TestServer

from moq_bot_rooms.server import room_app
from tests.helpers import FakeBots, make_rooms, settle

SHORT_HEARTBEAT_SECONDS = 0.05
PAGE_ORIGIN = "https://pages.example"


@pytest.fixture
async def open_client():
    clients = []

    async def open(bots: FakeBots, **options) -> TestClient:
        rooms_options = {key: options.pop(key) for key in ("max_room_seconds",) if key in options}
        app = room_app(make_rooms(bots, **rooms_options), heartbeat_seconds=SHORT_HEARTBEAT_SECONDS, **options)
        client = TestClient(TestServer(app))
        await client.start_server()
        clients.append(client)
        return client

    yield open
    for client in clients:
        await client.close()


async def test_stream_ends_with_the_reason_the_room_ended(open_client):
    # Arrange
    bots = FakeBots()
    client = await open_client(bots, allowed_origins=[], max_room_seconds=0.2)

    # Act
    response = await client.get("/rooms/abc")
    body = await asyncio.wait_for(response.text(), 2)

    # Assert
    assert response.headers["Content-Type"] == "text/event-stream"
    assert body.endswith("event: expired\ndata: expired\n\n")
    assert ": heartbeat\n\n" in body


async def test_page_that_closes_the_stream_releases_the_room(open_client):
    # Arrange
    bots = FakeBots()
    client = await open_client(bots, allowed_origins=[])
    response = await client.get("/rooms/abc")
    await settle()

    # Act
    response.close()
    await settle()
    await settle()

    # Assert
    assert bots.started == ["abc"]
    assert bots.running == set()


async def test_origin_outside_the_allowed_ones_is_refused(open_client):
    # Arrange
    bots = FakeBots()
    client = await open_client(bots, allowed_origins=[PAGE_ORIGIN])

    # Act
    refused = await client.get("/rooms/abc", headers={"Origin": "https://elsewhere.example"})
    allowed = await client.get("/rooms/abc", headers={"Origin": PAGE_ORIGIN})

    # Assert
    assert refused.status == 403
    assert allowed.status == 200
    assert allowed.headers["Access-Control-Allow-Origin"] == PAGE_ORIGIN
    allowed.close()


@pytest.mark.parametrize("room_id", ["ABC", "a" * 33, "a.b"])
async def test_room_id_outside_one_namespace_element_is_not_found(open_client, room_id):
    # Arrange
    bots = FakeBots()
    client = await open_client(bots, allowed_origins=[])

    # Act
    response = await client.get(f"/rooms/{room_id}")

    # Assert
    assert response.status == 404
    assert bots.started == []


async def test_server_for_one_room_refuses_any_other(open_client):
    # Arrange
    bots = FakeBots()
    client = await open_client(bots, allowed_origins=[], only_room="onvif")

    # Act
    other = await client.get("/rooms/abc")
    fixed = await client.get("/rooms/onvif")

    # Assert
    assert other.status == 404
    assert fixed.status == 200
    fixed.close()
