import argparse
import asyncio
from collections.abc import Awaitable, Callable

import moq_wasm
from loguru import logger
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
    args: argparse.Namespace, room: BotRoom, in_session: Callable[[moq_wasm.Session], Awaitable[bool]]
) -> None:
    """Runs `in_session` on a relay session with `room`'s token until it returns True. The session
    reconnects and keeps the token fresh by itself; this retries the first connection, and raises
    `moq_wasm.TokenExpired` once the token cannot be kept fresh."""
    log_prefix = f"room {room.key}: "
    while True:
        try:
            session = await moq_wasm.connect(
                args.relay_url,
                verify_certificate=not args.insecure,
                token=room.relay_token,
                refresh_token=room.token.refresh if room.token else None,
            )
        except RuntimeError as error:
            logger.warning(f"{log_prefix}cannot connect to {args.relay_url}: {error}")
            await asyncio.sleep(RECONNECT_DELAY_SECONDS)
            continue
        logger.info(f"{log_prefix}connected to {args.relay_url}")
        try:
            if await session.run(in_session(session)):
                return
            logger.warning(f"{log_prefix}relay session closed")
        except RuntimeError as error:
            logger.warning(f"{log_prefix}relay session failed: {error}")
        finally:
            await session.close()
        await asyncio.sleep(RECONNECT_DELAY_SECONDS)
