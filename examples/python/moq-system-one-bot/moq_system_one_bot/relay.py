import argparse
import asyncio
from collections.abc import Awaitable, Callable

import moqt
from loguru import logger
from moq_bot_rooms.app_token import while_fresh
from moq_bot_rooms.rooms import BotRoom

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
    args: argparse.Namespace, room: BotRoom, in_session: Callable[[moqt.Session], Awaitable[bool]]
) -> None:
    """Runs `in_session` on a relay session with `room`'s token, reconnecting whenever the session
    closes or fails, until `in_session` returns True. Keeps the token fresh on the session meanwhile,
    and raises once it cannot (`moq_bot_rooms.app_token.AppToken.keep_fresh`)."""
    log_prefix = f"room {room.key}: "
    while True:
        try:
            session = await moqt.connect(
                args.relay_url, verify_certificate=not args.insecure, token=room.relay_token
            )
        except RuntimeError as error:
            logger.warning(f"{log_prefix}cannot connect to {args.relay_url}: {error}")
            await asyncio.sleep(RECONNECT_DELAY_SECONDS)
            continue
        logger.info(f"{log_prefix}connected to {args.relay_url}")
        try:
            if await while_fresh(room.token, session, in_session(session)):
                return
            logger.warning(f"{log_prefix}relay session closed")
        except RuntimeError as error:
            logger.warning(f"{log_prefix}relay session failed: {error}")
        finally:
            await session.close()
        await asyncio.sleep(RECONNECT_DELAY_SECONDS)
