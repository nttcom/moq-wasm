import atexit
import json
import logging
import threading
import time
from collections.abc import Callable
from typing import Any

from moq_wasm import _native
from moq_wasm._telemetry import ClientTelemetry, TelemetryOptions

logger = logging.getLogger("moq_wasm")

Record = dict[str, Any]
"""A log or span record in the shape of the observability server's `/api/client-telemetry`: a log
is `{timeMs, level, target, message, fields, traceId?, spanId?}`, a span `{traceId, spanId,
parentSpanId?, name, startTimeMs, endTimeMs, fields}`."""
RecordCallback = Callable[[Record, str | None], None]

TRACE = 5
LEVELS = {
    "error": logging.ERROR,
    "warn": logging.WARNING,
    "info": logging.INFO,
    "debug": logging.DEBUG,
    "trace": TRACE,
}
# How long exit waits for the sink thread to drain the queue: it stops once a half-second
# `wait_log_records` finds the queue empty after the stop flag is set.
STOP_TIMEOUT_SECONDS = 1.5
# How long exit waits for the final telemetry report, past its 2 s request timeout.
TELEMETRY_STOP_TIMEOUT_SECONDS = 3.0

logging.addLevelName(TRACE, "TRACE")


class _Sink:
    def __init__(self, level: str, on_log: list[RecordCallback], on_span: list[RecordCallback]):
        self.level = LEVELS[level]
        self.on_log = on_log
        self.on_span = on_span
        self.stopped = threading.Event()

    def pump(self) -> None:
        # Stopped before the interpreter finalizes, so the thread never takes the GIL back after;
        # once stopped, it drains the queue into the final telemetry report first.
        while True:
            records = _native.wait_log_records()
            for kind, payload, token in records:
                record = json.loads(payload)
                if kind == "log":
                    _to_logging(record)
                    _call(self.on_log, record, token)
                else:
                    _call(self.on_span, record, token)
            if self.stopped.is_set() and not records:
                return


_sink: _Sink | None = None
_sink_lock = threading.Lock()


def init(
    *,
    log_level: str = "warn",
    on_log: RecordCallback | None = None,
    on_span: RecordCallback | None = None,
    telemetry: TelemetryOptions | None = None,
) -> None:
    """Hands the native module's `tracing` events at `log_level` (`error`, `warn`, `info`, `debug`
    or `trace`) or above to Python's `logging`, under `moq_wasm.native` and the event's target with
    `.` for `::` (`moq_wasm.native.moqt.modules.client.routes`), and to `on_log`. With
    `on_span`, the native module's closed INFO or higher spans go to it, and the requests a session
    sends carry their trace, as the browser SDK's `init({ logLevel, onLog, onSpan })` does. The
    callbacks run on a thread of their own; the second argument is the token of the session the
    record belongs to (its newest, after a `refresh_token`), None for a record of no session or of a
    session without one. Put it only in a telemetry report's `token`. `telemetry` sends the logs and
    spans to an observability server every 10 s, and once more at exit with what the sink drained
    by then, each session's under its token, as the browser SDK's does. The sink is process-wide:
    only the first call takes effect. Raises ValueError for an unknown level."""
    global _sink
    with _sink_lock:
        if _sink is not None:
            return
        if log_level not in LEVELS:
            raise ValueError(f"unknown log level: {log_level}")
        logs = [on_log] if on_log is not None else []
        spans = [on_span] if on_span is not None else []
        reports = ClientTelemetry(telemetry) if telemetry is not None else None
        if reports is not None:
            logs.append(reports.log)
            spans.append(reports.span)
        _native.set_log_sink(log_level, bool(spans))
        if reports is not None:
            _start_reporting(reports)
        sink = _Sink(log_level, logs, spans)
        pumping = threading.Thread(target=sink.pump, name="moq_wasm-log-sink", daemon=True)
        pumping.start()

        def stop() -> None:
            sink.stopped.set()
            pumping.join(STOP_TIMEOUT_SECONDS)

        # atexit runs the last registered first, so the sink drains before the final report.
        atexit.register(stop)
        _sink = sink


def _start_reporting(reports: ClientTelemetry) -> None:
    stopped = threading.Event()
    reporting = threading.Thread(
        target=reports.run, args=(stopped,), name="moq_wasm-telemetry", daemon=True
    )
    reporting.start()

    def finish() -> None:
        stopped.set()
        reporting.join(TELEMETRY_STOP_TIMEOUT_SECONDS)

    atexit.register(finish)


def report_log(level: str, target: str, message: str, **fields: Any) -> None:
    """Hands the app's own log to `init`'s `on_log` and `telemetry` at its `log_level` or above,
    without a token, as the browser SDK's `reportLog` does; it is not written to `logging`. Values
    that are not a string, number or boolean are sent as their `str`. Raises ValueError for an
    unknown level."""
    if level not in LEVELS:
        raise ValueError(f"unknown log level: {level}")
    sink = _sink
    if sink is None or LEVELS[level] < sink.level:
        return
    record = {
        "timeMs": int(time.time() * 1000),
        "level": level,
        "target": target,
        "message": message,
        "fields": {key: _field(value) for key, value in fields.items()},
    }
    _call(sink.on_log, record, None)


def _field(value: Any) -> Any:
    return value if isinstance(value, (str, int, float, bool)) else str(value)


def _to_logging(record: Record) -> None:
    fields = record["fields"]
    text = " ".join([record["message"], *(f"{key}={value}" for key, value in fields.items())])
    logging.getLogger("moq_wasm.native." + record["target"].replace("::", ".")).log(
        LEVELS[record["level"]], "%s", text, extra={"moq_fields": fields}
    )


def _call(callbacks: list[RecordCallback], record: Record, token: str | None) -> None:
    for callback in callbacks:
        try:
            callback(record, token)
        except Exception:
            # A failing callback must not stop the records that follow.
            logger.exception("log sink callback failed")
