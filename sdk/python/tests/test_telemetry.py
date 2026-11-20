import json

from moq_wasm import TelemetryOptions
from moq_wasm._telemetry import MAX_RECORDS, ClientTelemetry


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
