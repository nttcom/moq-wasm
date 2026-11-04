from aiohttp import web
from aiohttp.test_utils import TestServer
from loguru import logger

from moq_bot_rooms.telemetry import report_logs, telemetry_endpoint


def test_a_local_relay_reports_to_the_local_server():
    # Act / Assert
    assert telemetry_endpoint(None, False, "https://127.0.0.1:4433") == "http://localhost:8095/api/client-telemetry"


def test_a_cloud_relay_reports_to_the_deployed_server():
    # Act / Assert
    assert (
        telemetry_endpoint(None, False, "https://relay.moqt.research.skyway.io:443")
        == "https://observability.moqt.research.skyway.io/api/client-telemetry"
    )


def test_a_configured_server_overrides_the_default():
    # Act / Assert
    assert (
        telemetry_endpoint("http://obs.example:8095/", False, "https://127.0.0.1:4433")
        == "http://obs.example:8095/api/client-telemetry"
    )


def test_no_telemetry_sends_nothing():
    # Act / Assert
    assert telemetry_endpoint(None, True, "https://127.0.0.1:4433") is None


async def test_the_logs_are_posted_as_the_bot_on_exit():
    # Arrange
    reports = []

    async def receive(request: web.Request) -> web.Response:
        reports.append(await request.json())
        return web.Response(status=204)

    app = web.Application()
    app.router.add_post("/api/client-telemetry", receive)
    async with TestServer(app) as server:
        endpoint = str(server.make_url("/api/client-telemetry"))

        # Act
        async with report_logs(endpoint, "moq-camera-detection"):
            logger.bind(room_id="abc").warning("relay session closed")
            logger.debug("not reported")

    # Assert
    assert len(reports) == 1
    report = reports[0]
    assert report["service"] == "python"
    assert report["page"] == "moq-camera-detection"
    assert [(log["level"], log["message"], log["fields"]) for log in report["logs"]] == [
        ("warn", "relay session closed", {"room_id": "abc"})
    ]
    assert report["spans"] == []
