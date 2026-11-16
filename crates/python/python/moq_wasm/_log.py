import atexit
import json
import logging
import threading
import time
from collections.abc import Callable
from typing import Any

from moq_wasm import _native

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
# How long exit waits for the sink thread, which sees its stop flag within `wait_log_records`'
# half-second wait.
STOP_TIMEOUT_SECONDS = 1.0

logging.addLevelName(TRACE, "TRACE")


class _Sink:
    def __init__(self, level: str, on_log: RecordCallback | None, on_span: RecordCallback | None):
        self.level = LEVELS[level]
        self.on_log = on_log
        self.on_span = on_span
        self.stopped = threading.Event()

    def pump(self) -> None:
        # Stopped before the interpreter finalizes, so the thread never takes the GIL back after.
        while not self.stopped.is_set():
            for kind, payload in _native.wait_log_records():
                record = json.loads(payload)
                if kind == "log":
                    _to_logging(record)
                    _call(self.on_log, record)
                else:
                    _call(self.on_span, record)


_sink: _Sink | None = None
_sink_lock = threading.Lock()


def init(
    *,
    log_level: str = "warn",
    on_log: RecordCallback | None = None,
    on_span: RecordCallback | None = None,
) -> None:
    """Hands the native module's `tracing` events at `log_level` (`error`, `warn`, `info`, `debug`
    or `trace`) or above to Python's `logging`, under `moq_wasm.native` and the event's target with
    `.` for `::` (`moq_wasm.native.moqt.modules.client.routes`), and to `on_log`. With
    `on_span`, the native module's closed INFO or higher spans go to it, and the requests a session
    sends carry their trace, as the browser SDK's `init({ logLevel, onLog, onSpan })` does. The
    callbacks run on a thread of their own; the second argument is the record's session token,
    None here. The sink is process-wide: only the first call takes effect. Raises ValueError for an
    unknown level."""
    global _sink
    with _sink_lock:
        if _sink is not None:
            return
        if log_level not in LEVELS:
            raise ValueError(f"unknown log level: {log_level}")
        _native.set_log_sink(log_level, on_span is not None)
        sink = _Sink(log_level, on_log, on_span)
        pumping = threading.Thread(target=sink.pump, name="moq_wasm-log-sink", daemon=True)
        pumping.start()

        def stop() -> None:
            sink.stopped.set()
            pumping.join(STOP_TIMEOUT_SECONDS)

        atexit.register(stop)
        _sink = sink


def report_log(level: str, target: str, message: str, **fields: Any) -> None:
    """Hands the app's own log to `init`'s `on_log` at its `log_level` or above, without a token, as
    the browser SDK's `reportLog` does; it is not written to `logging`. Values that are not a
    string, number or boolean are sent as their `str`. Raises ValueError for an unknown level."""
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
    _call(sink.on_log, record)


def _field(value: Any) -> Any:
    return value if isinstance(value, (str, int, float, bool)) else str(value)


def _to_logging(record: Record) -> None:
    fields = record["fields"]
    text = " ".join([record["message"], *(f"{key}={value}" for key, value in fields.items())])
    logging.getLogger("moq_wasm.native." + record["target"].replace("::", ".")).log(
        LEVELS[record["level"]], "%s", text, extra={"moq_fields": fields}
    )


def _call(callback: RecordCallback | None, record: Record) -> None:
    if callback is None:
        return
    try:
        callback(record, None)
    except Exception:
        # A failing callback must not stop the records that follow.
        logger.exception("log sink callback failed")
