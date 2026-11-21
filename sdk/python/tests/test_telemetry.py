import json
from collections.abc import Callable

from moq_wasm import TelemetryOptions
from moq_wasm._telemetry import MAX_QUEUED_TOKENS, MAX_RECORDS, MAX_REJECTED_TOKENS, ClientTelemetry
from tests.failing_logging import failing_handler


class Sent:
    """Answers each report with `status` and keeps it, parsed and as the bytes sent."""

    def __init__(self, status: int = 202):
        self.status = status
        self.reports: list[tuple[str, dict]] = []
        self.bodies: list[bytes] = []

    def __call__(self, url: str, body: bytes, timeout: float) -> int:
        self.reports.append((url, json.loads(body)))
        self.bodies.append(body)
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


def test_a_rejected_token_is_not_kept_once_its_report_was_sent():
    # Arrange
    sent = Sent(status=401)
    telemetry = ClientTelemetry(TelemetryOptions(url="http://observability", page="bot"), sent)
    telemetry.log(log("first"), "expired-secret")

    # Act
    telemetry.flush()

    # Assert: only its digest is remembered
    assert "expired-secret" not in repr(vars(telemetry))


def test_only_the_latest_rejected_tokens_are_remembered():
    # Arrange: one more token rejected than are remembered
    sent = Sent(status=401)
    telemetry = ClientTelemetry(TelemetryOptions(url="http://observability", page="bot"), sent)
    for index in range(MAX_REJECTED_TOKENS + 1):
        telemetry.log(log("rejected"), f"token-{index}")
        telemetry.flush()
    sent.reports.clear()

    # Act
    telemetry.log(log("again"), "token-0")
    telemetry.log(log("again"), f"token-{MAX_REJECTED_TOKENS}")
    telemetry.flush()

    # Assert: the oldest was forgotten and is sent again
    assert sorted(report["token"] or "" for _, report in sent.reports) == ["", "token-0"]


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


def test_the_records_of_the_tokens_queued_first_are_dropped_past_the_queued_tokens_and_logged(caplog):
    # Arrange: each of more sessions than are queued at once logs twice before a flush
    sent = Sent()
    telemetry = ClientTelemetry(TelemetryOptions(url="http://observability", page="bot"), sent)
    for index in range(MAX_QUEUED_TOKENS + 2):
        telemetry.log(log("first"), f"token-{index}")
        telemetry.log(log("second"), f"token-{index}")

    # Act
    with caplog.at_level("WARNING", logger="moq_wasm"):
        telemetry.flush()

    # Assert: the two tokens queued first are dropped with their four records
    assert [report["token"] for _, report in sent.reports] == [f"token-{index}" for index in range(2, MAX_QUEUED_TOKENS + 2)]
    assert [record.getMessage() for record in caplog.records] == [
        f"telemetry dropped 4 records queued past {MAX_QUEUED_TOKENS} tokens"
    ]


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


def strict_json(body: bytes) -> dict:
    """Parses `body` refusing NaN and infinity, as the observability server does."""

    def refuse(constant: str):
        raise ValueError(f"{constant} is not JSON")

    return json.loads(body, parse_constant=refuse)


def test_a_non_finite_field_is_sent_as_its_string_in_a_valid_report():
    # Arrange: a log with a NaN, and a redactor that returns an infinity and a list holding a NaN
    sent = Sent()
    redact = lambda fields: {**fields, "jitter": float("inf"), "samples": [1.0, float("nan")]}
    telemetry = ClientTelemetry(TelemetryOptions(url="http://observability", page="bot", redact=redact), sent)
    telemetry.log(log("rtt measured", rtt=float("nan"), attempt=2), None)

    # Act
    telemetry.flush()

    # Assert
    fields = strict_json(sent.bodies[0])["logs"][0]["fields"]
    assert fields == {"rtt": "nan", "attempt": 2, "jitter": "inf", "samples": "[1.0, nan]"}


def test_a_field_is_sent_as_it_was_when_its_record_was_queued_and_one_with_a_key_json_cannot_hold_as_its_string():
    # Arrange: a redactor that returns a list it changes later, and a dict with a tuple key
    sent = Sent()
    samples = [1.0]
    redact = lambda fields: {**fields, "samples": samples, "by_pair": {(1, 2): "a"}}
    telemetry = ClientTelemetry(TelemetryOptions(url="http://observability", page="bot", redact=redact), sent)
    telemetry.log(log("rtt measured"), None)
    samples.append(float("nan"))

    # Act
    telemetry.flush()

    # Assert
    fields = strict_json(sent.bodies[0])["logs"][0]["fields"]
    assert fields == {"samples": [1.0], "by_pair": "{(1, 2): 'a'}"}
