import argparse
import asyncio
from collections.abc import Awaitable, Callable

import moqt
from loguru import logger

DEFAULT_RELAY_URL = "https://127.0.0.1:4433"
RECONNECT_DELAY_SECONDS = 2.0


def add_relay_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--relay-url", default=DEFAULT_RELAY_URL)
    parser.add_argument(
        "--insecure",
        action="store_true",
        help="skip TLS verification, for a local relay with a self-signed certificate",
    )


async def stay_connected(
    args: argparse.Namespace, in_session: Callable[[moqt.Session], Awaitable[bool]], log_prefix: str = ""
) -> None:
    """Runs `in_session` on a relay session, reconnecting whenever the session closes or fails, until
    `in_session` returns True. `log_prefix` starts every log line, such as "room 3: "."""
    while True:
        try:
            session = await moqt.connect(args.relay_url, verify_certificate=not args.insecure)
        except RuntimeError as error:
            logger.warning(f"{log_prefix}cannot connect to {args.relay_url}: {error}")
            await asyncio.sleep(RECONNECT_DELAY_SECONDS)
            continue
        logger.info(f"{log_prefix}connected to {args.relay_url}")
        try:
            if await in_session(session):
                return
            logger.warning(f"{log_prefix}relay session closed")
        except RuntimeError as error:
            logger.warning(f"{log_prefix}relay session failed: {error}")
        finally:
            await session.close()
        await asyncio.sleep(RECONNECT_DELAY_SECONDS)
