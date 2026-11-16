import logging
import threading
import time

import pytest

import moq_wasm
from tests.relay_sessions import TRACK, connect, room, within

WAIT_SECONDS = 5


class Collected:
    """The records a callback was handed, from the sink's thread."""

    def __init__(self):
        self._records: list[dict] = []
        self._lock = threading.Lock()

    def __call__(self, record: dict, token: str | None) -> None:
        with self._lock:
            self._records.append(record)

    def wait_for(self, matches) -> dict:
        deadline = time.monotonic() + WAIT_SECONDS
        while time.monotonic() < deadline:
            with self._lock:
                found = next((record for record in self._records if matches(record)), None)
            if found is not None:
                return found
            time.sleep(0.05)
        raise AssertionError("no matching record")


LOGS = Collected()
SPANS = Collected()


@pytest.fixture(scope="module", autouse=True)
def log_sink():
    """The sink is process-wide and set once, so every test here shares it."""
    moq_wasm.init(log_level="info", on_log=LOGS, on_span=SPANS)


class Handler(logging.Handler):
    def __init__(self):
        super().__init__(logging.INFO)
        self.records: list[logging.LogRecord] = []

    def emit(self, record: logging.LogRecord) -> None:
        self.records.append(record)


def wait_for_logged(handler: Handler, message: str) -> logging.LogRecord:
    deadline = time.monotonic() + WAIT_SECONDS
    while time.monotonic() < deadline:
        found = next((record for record in handler.records if message in record.getMessage()), None)
        if found is not None:
            return found
        time.sleep(0.05)
    raise AssertionError(f"{message} was not logged")


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
