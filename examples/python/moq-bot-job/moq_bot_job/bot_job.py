"""A bot that the bot service (services/bot-dispatcher) launched for one room: its arguments, its calls back to
the service, and the run that reports its state and ends it (docs/architecture/bot-dispatcher/architecture.md)."""

import argparse
import asyncio
import json
import os
import signal
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from enum import StrEnum

import aiohttp
from loguru import logger

from moq_bot_job.identity_token import IdentityToken, metadata_identity_token
from moq_bot_job.relay import add_relay_arguments
from moq_bot_job.telemetry import add_telemetry_arguments, report_logs, telemetry_endpoint

BOT_KEY_ENV = "BOT_DISPATCHER_BOT_KEY"
# A stop the app asked for is learnt at the next heartbeat.
HEARTBEAT_SECONDS = 5.0
REQUEST_TIMEOUT = aiohttp.ClientTimeout(total=10)


class BotState(StrEnum):
    RUNNING = "running"
    FINISHED = "finished"
    FAILED = "failed"


class BotEnded(Exception):
    """The bot service refused the call: the bot has ended, or it does not know it."""


@dataclass(frozen=True)
class BotJob:
    """What the bot service starts a bot with, after the bot type's own arguments."""

    bot_type: str
    bot_id: str
    bot_dispatcher_url: str
    namespace_prefix: list[str]
    grace_seconds: float
    options: dict

    @property
    def broadcast(self) -> list[str]:
        """`<room>/bots/<type>`, where the bot's token lets it publish."""
        return [*self.namespace_prefix, "bots", self.bot_type]


def add_bot_job_arguments(parser: argparse.ArgumentParser) -> None:
    """The arguments the bot service appends, plus the relay's. `--max-seconds` is read by the
    service itself, which asks the bot to stop at its longest run."""
    add_telemetry_arguments(parser)
    add_relay_arguments(parser)
    parser.add_argument("--bot-type", required=True)
    parser.add_argument("--bot-id", required=True)
    parser.add_argument("--bot-dispatcher-url", required=True)
    parser.add_argument("--namespace-prefix", required=True, help="the room, such as <app>/<room>")
    parser.add_argument("--grace-seconds", type=float, required=True)
    parser.add_argument("--max-seconds", type=float)
    parser.add_argument("--options", type=json.loads, default={}, help="the bot type's options as JSON")


def bot_job_from_arguments(args: argparse.Namespace) -> BotJob:
    return BotJob(
        bot_type=args.bot_type,
        bot_id=args.bot_id,
        bot_dispatcher_url=args.bot_dispatcher_url.rstrip("/"),
        namespace_prefix=args.namespace_prefix.split("/"),
        grace_seconds=args.grace_seconds,
        options=args.options,
    )


class BotDispatcher:
    """The bot's calls to the bot service. It presents the key the service gave it in `BOT_DISPATCHER_BOT_KEY`
    when it was launched locally, and otherwise the Google ID token of the job's service account for
    the service's URL."""

    def __init__(self, http: aiohttp.ClientSession, job: BotJob, key: str | None, identity: IdentityToken | None):
        self._http = http
        self._url = f"{job.bot_dispatcher_url}/bots/{job.bot_type}/{job.bot_id}"
        self._key = key
        self._identity = identity

    @classmethod
    def from_env(cls, http: aiohttp.ClientSession, job: BotJob) -> "BotDispatcher":
        key = os.environ.get(BOT_KEY_ENV) or None
        return cls(http, job, key, None if key else metadata_identity_token(http, job.bot_dispatcher_url))

    async def token(self) -> str | None:
        """A new token for the relay; None from a service without authorization, whose bots join
        with no token. Raises `BotEnded` once the bot has ended."""
        answer = await self._post("token", None)
        return answer["token"]

    async def refresh(self, _token: str) -> str:
        """`moq_wasm.connect`'s `refresh_token`."""
        token = await self.token()
        if token is None:
            raise RuntimeError("the bot service answered without a token")
        return token

    async def heartbeat(self, state: BotState, error: str | None = None) -> bool:
        """True when the bot should stop: the app asked, or it reached its longest run. Raises
        `BotEnded` once the service takes the bot as ended."""
        answer = await self._post("heartbeat", {"state": state, "error": error})
        return bool(answer.get("stop"))

    async def _post(self, call: str, body: dict | None) -> dict:
        bearer = self._key if self._key else await self._identity.value()
        async with self._http.post(
            f"{self._url}/{call}",
            json=body,
            headers={"Authorization": f"Bearer {bearer}"},
            timeout=REQUEST_TIMEOUT,
        ) as response:
            answer = await response.json(content_type=None) or {}
            if response.status in (404, 410):
                raise BotEnded(answer.get("error", response.reason))
            response.raise_for_status()
            return answer


# Runs the bot in the job's room until it is cancelled, or returns once nobody is in the room. It
# joins with `BotDispatcher.token` and keeps the token fresh with `BotDispatcher.refresh`.
RunRoom = Callable[[BotJob, BotDispatcher], Awaitable[None]]


async def run_bot_job(job: BotJob, dispatcher: BotDispatcher, run_room: RunRoom) -> None:
    """Runs `run_room` and sends heartbeats until it returns or the bot service asks the bot to stop,
    then reports how the bot ended. Cancelling it, as a signal does, reports the bot finished. Raises
    what `run_room` raised, after reporting it."""
    try:
        running = asyncio.ensure_future(run_room(job, dispatcher))
        try:
            while not running.done() and not await _asked_to_stop(dispatcher):
                await asyncio.wait([running], timeout=HEARTBEAT_SECONDS)
        finally:
            running.cancel()
            await asyncio.wait([running])
        if not running.cancelled():
            running.result()
    except asyncio.CancelledError:
        asyncio.current_task().uncancel()
        logger.info("bot stopped by a signal")
    except BotEnded as error:
        logger.info(f"bot ended by the bot service: {error}")
        return
    except Exception as error:
        await _report(dispatcher, BotState.FAILED, repr(error))
        raise
    await _report(dispatcher, BotState.FINISHED)


async def _asked_to_stop(dispatcher: BotDispatcher) -> bool:
    """A heartbeat the service did not answer is not a reason to stop: it takes the bot as failed
    only once heartbeats stop for 90 s."""
    try:
        stop = await dispatcher.heartbeat(BotState.RUNNING)
    except (aiohttp.ClientError, TimeoutError) as error:
        logger.warning(f"cannot send a heartbeat to the bot service: {error!r}")
        return False
    if stop:
        logger.info("the bot service asked the bot to stop")
    return stop


async def _report(dispatcher: BotDispatcher, state: BotState, error: str | None = None) -> None:
    try:
        await dispatcher.heartbeat(state, error)
    except (BotEnded, aiohttp.ClientError, TimeoutError) as report:
        logger.warning(f"cannot report {state} to the bot service: {report!r}")


async def serve_bot_job(args: argparse.Namespace, run_room: RunRoom) -> None:
    """Runs the room of the bot the arguments name (`add_bot_job_arguments`). `args.bot` names the
    bot in its telemetry; the parser must also define `--relay-url`."""
    job = bot_job_from_arguments(args)
    endpoint = telemetry_endpoint(args.telemetry_url, args.no_telemetry, args.relay_url)
    async with report_logs(endpoint, args.bot), aiohttp.ClientSession() as http:
        logger.info(f"bot {job.bot_id} started for {'/'.join(job.namespace_prefix)}")
        task = asyncio.current_task()
        loop = asyncio.get_running_loop()
        for signum in (signal.SIGINT, signal.SIGTERM):
            loop.add_signal_handler(signum, task.cancel)
        await run_bot_job(job, BotDispatcher.from_env(http, job), run_room)
