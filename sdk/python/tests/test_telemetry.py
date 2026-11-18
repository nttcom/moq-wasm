import json
from collections.abc import Callable

from moq_wasm import TelemetryOptions
from moq_wasm._telemetry import MAX_RECORDS, ClientTelemetry
from tests.failing_logging import failing_handler


class Sent:
    """Answers each report with `status` and keeps it."""

    def __init__(self, status: int = 202):
        self.status = status
        self.reports: list[tuple[str, dict]] = []

    def __call__(self, url: str, body: bytes, timeout: float) -> int:
        self.reports.append((url, json.loads(body)))
        return self.status


def log(message: str, **fields) -> dict:
    return {"timeMs": 0, "level": "info", "target": "moqt", "message": message, "fields": fields}


def test_a_flush_posts_one_report_per_token_with_its_logs_and_spans():
    # Arrange
    sent = Sent()
    telemetry = ClientTelemetry(TelemetryOptions(url="http://observability/", page="bot"), sent)
    telemetry.log(log("SUBSCRIBE sent"), "token-a")
    telemetry.span({"name": "moqt.subscriber.subscribe", "fields": {}}, "token-a")
    telemetry.log(log("catalog parsed"), None)

    # Act
    telemetry.flush()

    # Assert
    assert [url for url, _ in sent.reports] == ["http://observability/api/client-telemetry"] * 2
    by_token = {report["token"]: report for _, report in sent.reports}
    assert by_token["token-a"]["logs"][0]["message"] == "SUBSCRIBE sent"
    assert by_token["token-a"]["spans"][0]["name"] == "moqt.subscriber.subscribe"
    assert by_token[None]["logs"][0]["message"] == "catalog parsed"
    assert {report["service"] for report in by_token.values()} == {"python"}
    assert {report["page"] for report in by_token.values()} == {"bot"}
    assert len({report["clientId"] for report in by_token.values()}) == 1


def test_redact_rewrites_the_fields_of_each_record():
    # Arrange
    sent = Sent()
    redact = lambda fields: {key: "***" if key == "track_namespace" else value for key, value in fields.items()}
    telemetry = ClientTelemetry(TelemetryOptions(url="http://observability", page="bot", redact=redact), sent)

    # Act
    telemetry.log(log("SUBSCRIBE sent", track_namespace="users/alice", request_id=2), None)
    telemetry.flush()

    # Assert
    assert sent.reports[0][1]["logs"][0]["fields"] == {"track_namespace": "***", "request_id": 2}


def test_a_token_the_server_rejected_is_left_out_of_later_reports():
    # Arrange
    sent = Sent(status=401)
    telemetry = ClientTelemetry(TelemetryOptions(url="http://observability", page="bot"), sent)
    telemetry.log(log("first"), "expired")
    telemetry.flush()

    # Act
    telemetry.log(log("second"), "expired")
    telemetry.flush()

    # Assert
    assert [report["token"] for _, report in sent.reports] == ["expired", None]


def test_a_report_keeps_the_newest_records_the_server_takes():
    # Arrange
    sent = Sent()
    telemetry = ClientTelemetry(TelemetryOptions(url="http://observability", page="bot"), sent)

    # Act
    for index in range(MAX_RECORDS + 5):
        telemetry.log(log(f"log {index}"), None)
    telemetry.flush()

    # Assert
    logs = sent.reports[0][1]["logs"]
    assert (len(logs), logs[0]["message"]) == (MAX_RECORDS, "log 5")


def test_a_flush_without_records_sends_nothing():
    # Arrange
    sent = Sent()
    telemetry = ClientTelemetry(TelemetryOptions(url=lambda: "http://observability", page="bot"), sent)

    # Act
    telemetry.flush()

    # Assert
    assert sent.reports == []


def url_failing_once() -> Callable[[], str]:
    """A `url` callable that raises at its first call and returns the server's origin after."""
    answers: list[Exception | str] = [RuntimeError("config service down"), "http://observability"]

    def url() -> str:
        answer = answers.pop(0)
        if isinstance(answer, Exception):
            raise answer
        return answer

    return url


class Ticks:
    """Stands in for `run`'s stop event: each wait runs the next action, and the wait that runs the
    last one stops the loop."""

    def __init__(self, *actions: Callable[[], None]):
        self.actions = list(actions)

    def wait(self, timeout: float) -> bool:
        self.actions.pop(0)()
        return not self.actions


def test_a_failing_url_drops_one_flush_and_the_next_one_is_sent():
    # Arrange: the url callable fails once
    sent = Sent()
    telemetry = ClientTelemetry(TelemetryOptions(url=url_failing_once(), page="bot"), sent)
    telemetry.log(log("lost"), None)

    # Act
    telemetry.flush()
    telemetry.log(log("kept"), None)
    telemetry.flush()

    # Assert
    assert [report["logs"][0]["message"] for _, report in sent.reports] == ["kept"]


def test_reporting_goes_on_after_logging_a_failed_report_fails(capsys):
    # Arrange: the url callable fails at the first flush, and the handler that would log it raises
    sent = Sent()
    telemetry = ClientTelemetry(TelemetryOptions(url=url_failing_once(), page="bot"), sent)
    telemetry.log(log("lost"), None)
    ticks = Ticks(lambda: None, lambda: telemetry.log(log("kept"), None))

    # Act
    with failing_handler("moq_wasm"):
        telemetry.run(ticks)

    # Assert: the failure is printed to stderr, and the final flush still sends
    assert "handler failed" in capsys.readouterr().err
    assert [report["logs"][0]["message"] for _, report in sent.reports] == ["kept"]


def test_a_field_json_cannot_hold_is_sent_as_its_string():
    # Arrange: the redactor leaves an object in the fields
    sent = Sent()
    options = TelemetryOptions(url="http://observability", page="bot", redact=lambda fields: {"at": object})
    telemetry = ClientTelemetry(options, sent)
    telemetry.log(log("SUBSCRIBE sent"), None)

    # Act
    telemetry.flush()

    # Assert
    assert sent.reports[0][1]["logs"][0]["fields"] == {"at": str(object)}
