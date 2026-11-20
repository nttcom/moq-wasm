import json
import logging
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

import moq_wasm
from moq_wasm import _telemetry
from tests.relay_sessions import TRACK, connect, room, within

WAIT_SECONDS = 5


def wait_until(find):
    """What `find` returns once it is not None, polled from the test's thread."""
    deadline = time.monotonic() + WAIT_SECONDS
    while time.monotonic() < deadline:
        found = find()
        if found is not None:
            return found
        time.sleep(0.05)
    raise AssertionError("nothing matched in time")


class Collected:
    """The records a callback was handed, from the sink's thread."""

    def __init__(self):
        self._records: list[dict] = []
        self._lock = threading.Lock()

    def __call__(self, record: dict, token: str | None) -> None:
        with self._lock:
            self._records.append({**record, "token": token})

    def wait_for(self, matches) -> dict:
        def find() -> dict | None:
            with self._lock:
                return next((record for record in self._records if matches(record)), None)

        return wait_until(find)


LOGS = Collected()
SPANS = Collected()


class FakeObservability(ThreadingHTTPServer):
    """Keeps each report posted to `/api/client-telemetry`."""

    def __init__(self):
        super().__init__(("127.0.0.1", 0), ReportHandler)
        self.reports: list[dict] = []
        self.url = f"http://127.0.0.1:{self.server_address[1]}"

    def wait_for(self, matches) -> dict:
        return wait_until(lambda: next((report for report in list(self.reports) if matches(report)), None))


class ReportHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:
        body = self.rfile.read(int(self.headers["content-length"]))
        if self.path == "/api/client-telemetry":
            self.server.reports.append(json.loads(body))
        self.send_response(202)
        self.end_headers()

    def log_message(self, *args) -> None:
        pass


OBSERVABILITY = FakeObservability()


@pytest.fixture(scope="module", autouse=True)
def log_sink():
    """The sink is process-wide and set once, so every test here shares it."""
    threading.Thread(target=OBSERVABILITY.serve_forever, daemon=True).start()
    _telemetry.FLUSH_INTERVAL_SECONDS = 0.2
    moq_wasm.init(
        log_level="info",
        on_log=LOGS,
        on_span=SPANS,
        telemetry=moq_wasm.TelemetryOptions(url=OBSERVABILITY.url, page="pytest"),
    )


class Handler(logging.Handler):
    def __init__(self):
        super().__init__(logging.INFO)
        self.records: list[logging.LogRecord] = []

    def emit(self, record: logging.LogRecord) -> None:
        self.records.append(record)


def wait_for_logged(handler: Handler, message: str) -> logging.LogRecord:
    return wait_until(lambda: next((record for record in handler.records if message in record.getMessage()), None))


async def test_a_native_event_reaches_logging_under_its_target_with_its_fields(relay_url):
    # Arrange
    handler = Handler()
    moqt_logger = logging.getLogger("moq_wasm.native.moqt")
    moqt_logger.addHandler(handler)
    moqt_logger.setLevel(logging.INFO)
    subscriber = await connect(relay_url)
    namespace = [*room(), "nobody"]

    # Act
    with pytest.raises(moq_wasm.RequestError):
        await within(subscriber.subscribe(namespace, TRACK))
    record = LOGS.wait_for(lambda record: record["message"] == "SUBSCRIBE_ERROR received")
    logged = wait_for_logged(handler, "SUBSCRIBE_ERROR received")

    # Assert
    assert logged.levelno == logging.INFO
    assert logged.name.startswith("moq_wasm.native.moqt.")
    assert logged.moq_fields["track_name"] == TRACK
    assert record["fields"]["track_namespace"] == "/".join(namespace)
    moqt_logger.removeHandler(handler)
    moqt_logger.setLevel(logging.NOTSET)
    await subscriber.close()


async def test_a_request_span_reaches_on_span(relay_url):
    # Arrange
    subscriber = await connect(relay_url)

    # Act
    with pytest.raises(moq_wasm.RequestError):
        await within(subscriber.subscribe([*room(), "nobody"], TRACK))
    span = SPANS.wait_for(lambda record: record["name"] == "moqt.subscriber.subscribe")

    # Assert
    assert len(span["traceId"]) == 32
    assert span["fields"]["outcome"] == "rejected"
    await subscriber.close()


def test_report_log_refuses_an_unknown_level():
    # Act / Assert
    with pytest.raises(ValueError, match="verbose"):
        moq_wasm.report_log("verbose", "app", "message")


def test_report_log_reaches_on_log_at_the_init_level():
    # Act
    moq_wasm.report_log("debug", "app", "below the level")
    moq_wasm.report_log("warn", "app", "room joined", room="r1", attempt=2, error=ValueError("x"))

    # Assert
    record = LOGS.wait_for(lambda record: record["target"] == "app")
    assert (record["level"], record["message"]) == ("warn", "room joined")
    assert record["fields"] == {"room": "r1", "attempt": 2, "error": "x"}


async def test_the_records_of_a_session_with_a_token_go_to_telemetry_under_it(relay_url, temporary_app_token):
    # Arrange
    app_id = f"temp-{uuid.uuid4().hex}"
    token = temporary_app_token(app_id)
    session = await moq_wasm.connect(relay_url, verify_certificate=False, token=token)
    track_name = f"missing-{uuid.uuid4().hex[:8]}"

    # Act
    with pytest.raises(moq_wasm.RequestError):
        await within(session.subscribe([app_id, "nobody"], track_name))
    report = OBSERVABILITY.wait_for(
        lambda report: any(log["fields"].get("track_name") == track_name for log in report["logs"])
    )

    # Assert
    assert (report["token"], report["service"], report["page"]) == (token, "python", "pytest")
    assert LOGS.wait_for(lambda record: record["fields"].get("track_name") == track_name)["token"] == token
    await session.close()
