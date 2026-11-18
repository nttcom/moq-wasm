import json
import logging
import threading
import traceback
import urllib.error
import urllib.request
import uuid
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

logger = logging.getLogger("moq_wasm")

Fields = dict[str, Any]
# The observability server takes at most this many logs and spans each per report.
MAX_RECORDS = 100
FLUSH_INTERVAL_SECONDS = 10.0
REQUEST_TIMEOUT_SECONDS = 5.0
# The final report at exit waits less, so a server that does not answer delays exit by 2 s per
# token at most.
EXIT_REQUEST_TIMEOUT_SECONDS = 2.0
SERVICE = "python"


@dataclass(frozen=True)
class TelemetryOptions:
    """Where `moq_wasm.init` sends the native module's logs and spans, as the browser SDK's
    `telemetry` option does."""

    url: str | Callable[[], str]
    """The observability server's origin (`https://observability.example`); reports go to its
    `/api/client-telemetry`. A callable is called at each send."""
    page: str
    """Names the app in each report, at most 128 characters."""
    redact: Callable[[Fields], Fields] | None = None
    """Returns the fields a log or span is sent with, to mask or drop a value such as a
    `track_namespace` that holds a user id. Messages and span names are sent as they are."""


def send_report(url: str, body: bytes, timeout: float) -> int | None:
    """The response's HTTP status, or None when there is none to read. Telemetry is best effort: an
    app whose observability server is down keeps working."""
    headers = {"content-type": "application/json"}
    request = urllib.request.Request(url, data=body, method="POST", headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.status
    except urllib.error.HTTPError as error:
        return error.code
    except OSError:
        return None


class ClientTelemetry:
    """Queues the native module's logs and spans per session token and posts them to the
    observability server's `/api/client-telemetry`, which verifies the token and forwards them to
    the collector."""

    def __init__(
        self,
        options: TelemetryOptions,
        send: Callable[[str, bytes, float], int | None] = send_report,
    ):
        self.client_id = str(uuid.uuid4())
        self._options = options
        self._send = send
        self._batches: dict[str | None, tuple[list, list]] = {}
        self._rejected_tokens: set[str] = set()
        self._lock = threading.Lock()

    def log(self, record: dict, token: str | None) -> None:
        self._keep(0, record, token)

    def span(self, record: dict, token: str | None) -> None:
        self._keep(1, record, token)

    def flush(self, timeout: float = REQUEST_TIMEOUT_SECONDS) -> None:
        """Sends one report per token, each waiting `timeout` seconds at most. A failed report's
        records are dropped, not retried; the failure, including the `url` callable raising, is
        logged, not raised."""
        with self._lock:
            batches, self._batches = self._batches, {}
        if not batches:
            return
        try:
            url = self._options.url if isinstance(self._options.url, str) else self._options.url()
        except Exception:
            logger.exception("telemetry url failed; %d reports dropped", len(batches))
            return
        endpoint = f"{url.rstrip('/')}/api/client-telemetry"
        for batch_token, (logs, spans) in batches.items():
            try:
                self._send_batch(endpoint, batch_token, logs, spans, timeout)
            except Exception:
                logger.exception("telemetry report dropped")

    def _send_batch(self, endpoint: str, batch_token: str | None, logs: list, spans: list, timeout: float) -> None:
        token = None if batch_token in self._rejected_tokens else batch_token
        report = {
            "service": SERVICE,
            "token": token,
            "clientId": self.client_id,
            "page": self._options.page,
            "logs": logs,
            "spans": spans,
        }
        status = self._send(endpoint, json.dumps(report, default=str).encode(), timeout)
        if status == 401 and token is not None:
            # Sent without it from now on, so the server takes the records as anonymous instead
            # of dropping them.
            self._rejected_tokens.add(token)

    def run(self, stopped: threading.Event) -> None:
        """Flushes every 10 s until `stopped` is set, then once more with the exit timeout. A flush
        that raises, as when a `logging` handler fails to log a failed report, is printed to stderr,
        and the next flush goes on."""
        while True:
            stopping = stopped.wait(FLUSH_INTERVAL_SECONDS)
            try:
                self.flush(EXIT_REQUEST_TIMEOUT_SECONDS if stopping else REQUEST_TIMEOUT_SECONDS)
            except Exception:
                traceback.print_exc()
            if stopping:
                return

    def _keep(self, kind: int, record: dict, token: str | None) -> None:
        if self._options.redact is not None:
            record = {**record, "fields": self._options.redact(record["fields"])}
        with self._lock:
            records = self._batches.setdefault(token, ([], []))[kind]
            records.append(record)
            if len(records) > MAX_RECORDS:
                del records[0]
