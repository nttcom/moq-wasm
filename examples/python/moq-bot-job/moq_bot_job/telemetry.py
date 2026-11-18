"""Sends a bot's logs to the observability server's `/api/client-telemetry`, which forwards them
to HyperDX as `service.name=python`."""

import argparse
import asyncio
import contextlib
import uuid
from collections import deque
from collections.abc import AsyncIterator
from urllib.parse import urlsplit

import aiohttp
from loguru import logger

LOCAL_TELEMETRY_URL = "http://localhost:8095"
CLOUD_TELEMETRY_URL = "https://observability.moqt.research.skyway.io"
LOCAL_HOSTS = {"localhost", "127.0.0.1", "::1"}
REPORT_SECONDS = 10.0
REQUEST_TIMEOUT = aiohttp.ClientTimeout(total=5)
# The observability server takes at most this many logs per report.
MAX_LOGS = 100
# loguru's level names mapped to the `tracing` names the server reads; any other level is sent as `error`.
LEVELS = {"INFO": "info", "SUCCESS": "info", "WARNING": "warn"}


def add_telemetry_arguments(parser: argparse.ArgumentParser) -> None:
    group = parser.add_mutually_exclusive_group()
    group.add_argument(
        "--telemetry-url",
        help="observability server that receives the logs; "
        "the local one for a local relay and the deployed one otherwise",
    )
    group.add_argument("--no-telemetry", action="store_true", help="send no logs to an observability server")


def telemetry_endpoint(telemetry_url: str | None, no_telemetry: bool, relay_url: str) -> str | None:
    if no_telemetry:
        return None
    if telemetry_url:
        base = telemetry_url.rstrip("/")
    elif urlsplit(relay_url).hostname in LOCAL_HOSTS:
        base = LOCAL_TELEMETRY_URL
    else:
        base = CLOUD_TELEMETRY_URL
    return f"{base}/api/client-telemetry"


def log_entry(record) -> dict:
    return {
        "timeMs": int(record["time"].timestamp() * 1000),
        "level": LEVELS.get(record["level"].name, "error"),
        "target": record["name"] or "",
        "message": record["message"],
        "fields": {key: field_value(value) for key, value in record["extra"].items()},
    }


def field_value(value):
    return value if isinstance(value, str | bool | int | float) else str(value)


@contextlib.asynccontextmanager
async def report_logs(endpoint: str | None, page: str) -> AsyncIterator[None]:
    """Reports the `INFO` and higher logs every `REPORT_SECONDS` and on exit; a failed report is dropped."""
    if endpoint is None:
        yield
        return
    # The newest `MAX_LOGS` records since the last report.
    logs: deque[dict] = deque(maxlen=MAX_LOGS)
    sink_id = logger.add(lambda message: logs.append(log_entry(message.record)), level="INFO", format="{message}")
    client_id = str(uuid.uuid4())
    async with aiohttp.ClientSession(timeout=REQUEST_TIMEOUT) as http:

        async def send() -> None:
            if not logs:
                return
            body = {"service": "python", "clientId": client_id, "page": page, "logs": list(logs)}
            logs.clear()
            try:
                async with http.post(endpoint, json=body):
                    pass
            except (aiohttp.ClientError, TimeoutError):
                pass

        async def send_periodically() -> None:
            while True:
                await asyncio.sleep(REPORT_SECONDS)
                await send()

        reporting = asyncio.create_task(send_periodically())
        try:
            yield
        finally:
            reporting.cancel()
            logger.remove(sink_id)
            await send()
